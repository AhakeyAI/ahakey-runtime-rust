use super::{Runtime, state::Job};
use crate::{
    ahakey::hardware::Hardware,
    error::{Error, Result},
    operation::{Mutation, Operation},
};
use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

impl Runtime {
    pub async fn oled_state(&self, id: &str) -> Result<crate::oled::State> {
        let (send, receive) = oneshot::channel();
        {
            let mut state = self.state.lock().await;
            if state.stopping {
                return Err(Error("RUNTIME_STOPPING"));
            }
            let entry = state.devices.get_mut(id).ok_or(Error("DEVICE_NOT_FOUND"))?;
            if entry.device.connection_state != "ready" {
                return Err(Error("DEVICE_NOT_READY"));
            }
            if entry.busy {
                return Err(Error("BUSY"));
            }
            entry
                .sender
                .try_send(Job::ReadOled(send))
                .map_err(|_| Error("BUSY"))?;
            entry.busy = true;
        }
        receive.await.map_err(|_| Error("DEVICE_DISCONNECTED"))?
    }
    pub async fn upload_oled(&self, request: crate::oled::Upload) -> Result<Operation> {
        request.validate()?;
        {
            let state = self.state.lock().await;
            if let Some(record) = state.journal.operations.get(&request.operation_id) {
                return if record.request == Mutation::Oled(request.clone()) {
                    Ok(record.operation.clone())
                } else {
                    Err(Error("OPERATION_ID_CONFLICT"))
                };
            }
        }
        let asset = self.resources.load(&request.resource_id).await?;
        if asset.info.frame_count > request.target.max_frames() {
            return Err(Error("TOO_MANY_FRAMES"));
        }
        self.accept(Mutation::Oled(request), false).await
    }
    pub(super) async fn execute_oled(
        &self,
        hardware: &mut dyn Hardware,
        request: &crate::oled::Upload,
        resumed: bool,
    ) {
        let id = &request.device_id;
        let operation_id = &request.operation_id;
        let pause = {
            let mut state = self.state.lock().await;
            let entry = state.devices.get_mut(id).unwrap();
            let error = if entry.device.connection_state != "ready" {
                Some("DEVICE_NOT_READY")
            } else if !resumed && entry.document.base_token != request.base_token {
                Some("BASE_CONFLICT")
            } else {
                None
            };
            if let Some(code) = error {
                entry.busy = false;
                entry.upload_pause = None;
                let operation = &mut state
                    .journal
                    .operations
                    .get_mut(operation_id)
                    .unwrap()
                    .operation;
                operation.status = if resumed { "paused" } else { "failed" }.into();
                operation.error_code = Some(code.into());
                let event = json!(operation);
                if self.storage.save(&state.journal).await.is_err() {
                    state.storage_healthy = false;
                }
                state.emit("operation.changed", event);
                return;
            }
            let pause = entry.upload_pause.as_ref().unwrap().1.subscribe();
            let operation = &mut state
                .journal
                .operations
                .get_mut(operation_id)
                .unwrap()
                .operation;
            operation.status = "running".into();
            operation.effect = "unknown".into();
            operation.progress = 0.0;
            let event = json!(operation);
            state.emit("operation.changed", event);
            pause
        };
        let result = async {
                let asset = self.resources.load(&request.resource_id).await?;
                hardware.oled_state().await?;
                if *pause.borrow() { return Err(Error("UPLOAD_PAUSED")); }
                {
                    let mut state = self.state.lock().await;
                    state.journal.operations.get_mut(operation_id).unwrap().oled_transaction = true;
                    if self.storage.save(&state.journal).await.is_err() {
                        state.storage_healthy = false;
                        return Err(Error("STORAGE_ERROR"));
                    }
                }
                let (progress, mut updates) = mpsc::channel(4);
                let upload = self.transfer_oled(hardware, request, &asset, pause, progress);
                tokio::pin!(upload);
                loop {
                    tokio::select! {
                        result = &mut upload => break result,
                        Some(progress) = updates.recv() => {
                            let mut state = self.state.lock().await;
                            let operation = &mut state.journal.operations.get_mut(operation_id).unwrap().operation;
                            operation.progress = progress;
                            operation.effect = "partial".into();
                            let event = json!(operation);
                            state.emit("operation.changed", event);
                        }
                    }
                }
        }.await;
        // The firmware aborts the connection-owned upload when its link closes.
        // The worker reconnects and queries the device before advertising ready.
        if result.is_err() {
            let _ = hardware.disconnect().await;
            self.disconnected(id).await;
        }
        let mut state = self.state.lock().await;
        let armed = state.journal.operations[operation_id].oled_transaction;
        let operation = &mut state
            .journal
            .operations
            .get_mut(operation_id)
            .unwrap()
            .operation;
        match result {
            Ok(()) => {
                operation.status = "completed".into();
                operation.progress = 1.0;
                operation.effect = "complete".into();
                operation.error_code = None;
            }
            Err(error) => {
                let rejected = matches!(
                    error.0,
                    "UNSUPPORTED_CAPABILITY"
                        | "TOO_MANY_FRAMES"
                        | "RESOURCE_NOT_FOUND"
                        | "RESOURCE_CORRUPT"
                );
                operation.status = if rejected { "failed" } else { "paused" }.into();
                operation.effect = if !armed { "none" } else { "unknown" }.into();
                if !armed && error.0 != "UPLOAD_PAUSED" {
                    operation.status = "failed".into();
                }
                operation.error_code = Some(error.0.into());
            }
        }
        if self.storage.save(&state.journal).await.is_err() {
            state.storage_healthy = false;
            let operation = &mut state
                .journal
                .operations
                .get_mut(operation_id)
                .unwrap()
                .operation;
            operation.status = "paused".into();
            operation.effect = "unknown".into();
            operation.error_code = Some("STORAGE_ERROR".into());
        }
        let entry = state.devices.get_mut(id).unwrap();
        entry.busy = false;
        entry.upload_pause = None;
        let operation = state.journal.operations[operation_id].operation.clone();
        if operation.status == "completed" {
            state.devices.get_mut(id).unwrap().document.base_token = Uuid::new_v4().to_string();
            state.emit("oled.changed", json!({"deviceId": id, "target": request.target, "resourceId": request.resource_id}));
        }
        state.emit("operation.changed", json!(operation));
    }
    pub(super) async fn transfer_oled(
        &self,
        hardware: &mut dyn Hardware,
        request: &crate::oled::Upload,
        asset: &crate::oled::Prepared,
        pause: watch::Receiver<bool>,
        progress: mpsc::Sender<f64>,
    ) -> Result<()> {
        for attempt in 0..3 {
            let failure = match hardware
                .upload_oled(&request.target, asset, pause.clone(), progress.clone())
                .await
            {
                Ok(()) => return Ok(()),
                Err(error) if error.0 == "UPLOAD_PAUSED" || *pause.borrow() => {
                    return Err(Error("UPLOAD_PAUSED"));
                }
                Err(error) => error,
            };
            if matches!(failure.0, "UNSUPPORTED_CAPABILITY" | "TOO_MANY_FRAMES") {
                return Err(failure);
            }
            // A lost ACK may hide a successful commit. Reconnect and read first;
            // an unreadable result stops here, rather than blindly resending.
            let _ = hardware.disconnect().await;
            self.disconnected(&request.device_id).await;
            self.set_connecting(&request.device_id).await;
            let status = hardware.connect().await?;
            let actual = hardware.oled_state().await?;
            let entry = actual
                .entries
                .iter()
                .find(|entry| entry.target == request.target)
                .ok_or(Error("DEVICE_INVALID_RESPONSE"))?;
            if actual
                .entries
                .iter()
                .any(|entry| entry.write_state == "writing")
            {
                return Err(Error("RESOURCE_RECOVERY_PENDING"));
            }
            self.publish_hardware(&request.device_id, &status).await;
            if entry.valid
                && entry.frame_count == asset.info.frame_count
                && entry.byte_length as usize == asset.info.byte_length
                && entry.crc32 == asset.info.crc32
            {
                return Ok(());
            }
            if *pause.borrow() {
                return Err(Error("UPLOAD_PAUSED"));
            }
            if attempt == 2 {
                return Err(failure);
            }
        }
        unreachable!("bounded attempts return after the final readback")
    }
}
