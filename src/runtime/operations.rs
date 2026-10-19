use super::{Runtime, state::Job};
use crate::{
    error::{Error, Result},
    operation::{Mutation, Operation},
    storage::Record,
};
use serde_json::json;
use tokio::sync::watch;

const MAX_OPERATIONS: usize = 128;
impl Runtime {
    pub async fn operation(&self, id: &str) -> Result<Operation> {
        self.state
            .lock()
            .await
            .journal
            .operations
            .get(id)
            .map(|r| r.operation.clone())
            .ok_or(Error("OPERATION_NOT_FOUND"))
    }
    pub async fn pause_operation(&self, id: &str) -> Result<Operation> {
        let state = self.state.lock().await;
        let record = state
            .journal
            .operations
            .get(id)
            .ok_or(Error("OPERATION_NOT_FOUND"))?;
        if !matches!(record.request, Mutation::Oled(_)) {
            return Err(Error("UNSUPPORTED_CAPABILITY"));
        }
        if record.operation.status == "paused" || record.operation.terminal() {
            return Ok(record.operation.clone());
        }
        let entry = &state.devices[&record.operation.device_id];
        if let Some((active_id, signal)) = &entry.upload_pause
            && active_id == id
        {
            signal.send_replace(true);
        }
        Ok(record.operation.clone())
    }
    pub async fn resume_operation(&self, id: &str) -> Result<Operation> {
        let mut state = self.state.lock().await;
        let record = state
            .journal
            .operations
            .get(id)
            .ok_or(Error("OPERATION_NOT_FOUND"))?;
        let Mutation::Oled(request) = &record.request else {
            return Err(Error("UNSUPPORTED_CAPABILITY"));
        };
        if record.operation.status != "paused" {
            if matches!(
                record.operation.status.as_str(),
                "accepted" | "running" | "completed"
            ) {
                return Ok(record.operation.clone());
            }
            return Err(Error("OPERATION_NOT_PAUSED"));
        }
        let request = request.clone();
        if state.stopping {
            return Err(Error("RUNTIME_STOPPING"));
        }
        if !state.storage_healthy {
            return Err(Error("STORAGE_ERROR"));
        }
        let entry = state
            .devices
            .get_mut(&request.device_id)
            .ok_or(Error("DEVICE_NOT_FOUND"))?;
        if entry.device.connection_state != "ready" {
            return Err(Error("DEVICE_NOT_READY"));
        }
        if entry.busy {
            return Err(Error("BUSY"));
        }
        entry.busy = true;
        let (signal, _) = watch::channel(false);
        entry.upload_pause = Some((id.to_owned(), signal));
        let sender = entry.sender.clone();
        let operation = &mut state.journal.operations.get_mut(id).unwrap().operation;
        operation.status = "accepted".into();
        operation.error_code = None;
        let operation = operation.clone();
        if self.storage.save(&state.journal).await.is_err() {
            state.storage_healthy = false;
            state.devices.get_mut(&request.device_id).unwrap().busy = false;
            state
                .journal
                .operations
                .get_mut(id)
                .unwrap()
                .operation
                .status = "paused".into();
            return Err(Error("STORAGE_ERROR"));
        }
        if sender.try_send(Job::ResumeOled(request.clone())).is_err() {
            state.devices.get_mut(&request.device_id).unwrap().busy = false;
            state
                .journal
                .operations
                .get_mut(id)
                .unwrap()
                .operation
                .status = "paused".into();
            self.storage.save(&state.journal).await?;
            return Err(Error("DEVICE_NOT_READY"));
        }
        state.emit("operation.changed", json!(operation));
        Ok(operation)
    }
    pub(super) async fn accept(
        &self,
        mut request: Mutation,
        initialize: bool,
    ) -> Result<Operation> {
        request.validate()?;
        request.canonicalize();
        let mut state = self.state.lock().await;
        // Deduplication must precede readiness, busy, and baseline checks.
        if let Some(record) = state.journal.operations.get(request.operation_id()) {
            return if record.request == request {
                Ok(record.operation.clone())
            } else {
                Err(Error("OPERATION_ID_CONFLICT"))
            };
        }
        if state.stopping {
            return Err(Error("RUNTIME_STOPPING"));
        }
        if !state.storage_healthy {
            return Err(Error("STORAGE_ERROR"));
        }
        // No silent eviction: every accepted ID remains recoverable until explicit maintenance.
        if state.journal.operations.len() >= MAX_OPERATIONS {
            return Err(Error("BUSY"));
        }
        let entry = state
            .devices
            .get_mut(request.device_id())
            .ok_or(Error("DEVICE_NOT_FOUND"))?;
        if entry.device.connection_state != "ready" {
            return Err(Error("DEVICE_NOT_READY"));
        }
        if entry.busy {
            return Err(Error("BUSY"));
        }
        if entry.document.base_token != request.base_token() {
            return Err(Error("BASE_CONFLICT"));
        }
        if let Mutation::Configuration(config) = &request {
            if !entry.lighting_supported {
                return Err(Error("UNSUPPORTED_CAPABILITY"));
            }
            if entry.document.configuration.is_none()
                && !(initialize && config.scope.modes == [0, 1, 2, 3])
            {
                return Err(Error("INCOMPLETE_WRITE_GROUP"));
            }
        }
        let sender = entry.sender.clone();
        if matches!(request, Mutation::Oled(_)) {
            let (signal, _) = watch::channel(false);
            entry.upload_pause = Some((request.operation_id().to_owned(), signal));
        }
        entry.busy = true;
        let operation = Operation {
            operation_id: request.operation_id().to_owned(),
            device_id: request.device_id().to_owned(),
            status: "accepted".into(),
            progress: 0.0,
            effect: "none".into(),
            error_code: None,
        };
        state.journal.operations.insert(
            request.operation_id().to_owned(),
            Record {
                request: request.clone(),
                operation: operation.clone(),
                oled_transaction: false,
            },
        );
        if self.storage.save(&state.journal).await.is_err() {
            state.storage_healthy = false;
            state.devices.get_mut(request.device_id()).unwrap().busy = false;
            let failed = &mut state
                .journal
                .operations
                .get_mut(request.operation_id())
                .unwrap()
                .operation;
            failed.status = "failed".into();
            failed.error_code = Some("STORAGE_ERROR".into());
            let _ = self.storage.save(&state.journal).await;
            return Err(Error("STORAGE_ERROR"));
        }
        if sender.try_send(Job::Apply(request.clone())).is_err() {
            state.devices.get_mut(request.device_id()).unwrap().busy = false;
            let failed = &mut state
                .journal
                .operations
                .get_mut(request.operation_id())
                .unwrap()
                .operation;
            failed.status = "failed".into();
            failed.error_code = Some("DEVICE_NOT_READY".into());
            self.storage.save(&state.journal).await?;
            return Err(Error("DEVICE_NOT_READY"));
        }
        state.emit("operation.changed", json!(operation));
        Ok(operation)
    }
}
