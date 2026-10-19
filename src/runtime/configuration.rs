use super::Runtime;
use crate::{
    ahakey::hardware::Hardware,
    configuration::{Apply, keymap::KeymapApply},
    error::{Error, Result},
    operation::{Mutation, Operation},
    wire::ConfigurationDocument,
};
use serde_json::json;
use uuid::Uuid;

impl Runtime {
    pub async fn configuration(&self, id: &str) -> Result<ConfigurationDocument> {
        let state = self.state.lock().await;
        let entry = state.devices.get(id).ok_or(Error("DEVICE_NOT_FOUND"))?;
        if entry.busy
            && (entry.document.configuration.is_some() || entry.document.readback.is_some())
        {
            return Err(Error("BUSY"));
        }
        Ok(entry.document.clone())
    }
    pub async fn apply(&self, request: Apply) -> Result<Operation> {
        self.accept(request.into(), false).await
    }
    pub async fn apply_keymap(&self, request: KeymapApply) -> Result<Operation> {
        self.accept(request.into(), false).await
    }
    pub(super) async fn execute_apply(&self, hardware: &mut dyn Hardware, request: &Mutation) {
        if let Mutation::Oled(upload) = request {
            self.execute_oled(hardware, upload, false).await;
            return;
        }
        let id = request.device_id();
        let operation_id = request.operation_id();
        let plan = match request {
            Mutation::Configuration(config) => hardware.configuration_commands(&config.changes),
            Mutation::Keymap(keys) => hardware.keymap_commands(keys),
            Mutation::Oled(_) => unreachable!(),
        };
        {
            let mut state = self.state.lock().await;
            // A notification received after admission can invalidate the session or baseline.
            let entry = &state.devices[id];
            let preflight = if let Err(error) = &plan {
                Some(error.0)
            } else if entry.device.connection_state != "ready" {
                Some("DEVICE_NOT_READY")
            } else if entry.document.base_token != request.base_token() {
                Some("BASE_CONFLICT")
            } else {
                None
            };
            if let Some(code) = preflight {
                let operation = &mut state
                    .journal
                    .operations
                    .get_mut(operation_id)
                    .unwrap()
                    .operation;
                operation.status = "failed".into();
                operation.error_code = Some(code.into());
                operation.effect = "none".into();
                let event = json!(operation);
                state.devices.get_mut(id).unwrap().busy = false;
                if self.storage.save(&state.journal).await.is_err() {
                    state.storage_healthy = false;
                }
                state.emit("operation.changed", event);
                return;
            }
            let record = state.journal.operations.get_mut(operation_id).unwrap();
            record.operation.status = "running".into();
            // An in-flight command can have effects before its ACK is observed.
            record.operation.effect = "unknown".into();
            let operation = record.operation.clone();
            state.emit("operation.changed", json!(operation));
        }
        let commands = plan.expect("model plan checked before execution");
        let mut failure = None;
        for (index, command) in commands.iter().enumerate() {
            crate::debug::event("operation.command", || {
                json!({
                    "deviceId":id, "operationId":operation_id, "index":index + 1,
                    "total":commands.len(), "opcode":command.get(2),
                })
            });
            if let Err(error) = hardware.write_confirmed(command).await {
                failure = Some(error);
                break;
            }
            let mut state = self.state.lock().await;
            let operation = &mut state
                .journal
                .operations
                .get_mut(operation_id)
                .unwrap()
                .operation;
            operation.progress = (index + 1) as f64 / commands.len() as f64;
            operation.effect = "partial".into();
            let event = json!(operation);
            state.emit("operation.changed", event);
        }
        if failure.is_some() {
            let _ = hardware.disconnect().await;
            self.disconnected(id).await;
        }
        let mut state = self.state.lock().await;
        if let Some(error) = failure {
            let operation = &mut state
                .journal
                .operations
                .get_mut(operation_id)
                .unwrap()
                .operation;
            operation.status = "failed".into();
            operation.error_code = Some(error.0.into());
            // The failing command might have executed even when no ACK arrived.
            operation.effect = "unknown".into();
            state.invalidate(id);
        } else {
            let entry = state.devices.get_mut(id).unwrap();
            match request {
                Mutation::Configuration(request) => {
                    if let Some(config) = &mut entry.document.configuration {
                        config.merge(&request.changes);
                    } else {
                        entry.document.configuration = Some(request.changes.clone());
                    }
                }
                Mutation::Keymap(request) => {
                    // Update only known fields. Never fabricate a complete baseline.
                    if let Some(config) = &mut entry.document.configuration {
                        if let Some(mode) = config.modes.iter_mut().find(|m| m.mode == request.mode)
                        {
                            for binding in &request.keys {
                                if let Some(key) = mode.keys.iter_mut().find(|k| {
                                    k.role == crate::configuration::ROLES[binding.index as usize]
                                }) {
                                    key.action = binding.action.clone();
                                }
                            }
                        } else {
                            entry.document.configuration = None;
                        }
                    }
                }
                Mutation::Oled(_) => unreachable!(),
            }
            entry.document.readback = None;
            entry.document.base_token = Uuid::new_v4().to_string();
            if let Some(config) = entry.document.configuration.clone() {
                state.journal.configurations.insert(id.to_owned(), config);
            } else {
                state.journal.configurations.remove(id);
            }
            let operation = &mut state
                .journal
                .operations
                .get_mut(operation_id)
                .unwrap()
                .operation;
            operation.status = "completed".into();
            operation.effect = "complete".into();
            operation.progress = 1.0;
        }
        if self.storage.save(&state.journal).await.is_err() {
            state.storage_healthy = false;
            let operation = &mut state
                .journal
                .operations
                .get_mut(operation_id)
                .unwrap()
                .operation;
            operation.status = "failed".into();
            operation.effect = "unknown".into();
            operation.error_code = Some("STORAGE_ERROR".into());
            state.invalidate(id);
            let _ = self.storage.save(&state.journal).await;
        }
        state.devices.get_mut(id).unwrap().busy = false;
        let operation = state
            .journal
            .operations
            .get(operation_id)
            .unwrap()
            .operation
            .clone();
        if operation.status == "completed" {
            state.emit("configuration.changed", json!({"deviceId": id}));
        }
        state.emit("operation.changed", json!(operation));
    }
}
