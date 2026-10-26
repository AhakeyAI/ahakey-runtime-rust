//! Resumable field writes. Journal before effects, pause only between complete fields.
use super::Runtime;
use crate::{
    ahakey::hardware::Hardware,
    configuration::transaction::{Checkpoint, Plan},
    error::{Error, Result},
    operation::{ConfigurationProgress, Mutation},
};
use serde_json::json;
use std::time::Duration;
use tokio::sync::watch;

impl Runtime {
    async fn checkpoint(&self, id: &str) -> Result<()> {
        let mut state = self.state.lock().await;
        if self.storage.save(&state.journal).await.is_err() {
            state.storage_healthy = false;
            return Err(Error("STORAGE_ERROR"));
        }
        let operation = state.journal.operations[id].operation.clone();
        state.emit("operation.changed", json!(operation));
        Ok(())
    }
    pub(super) async fn execute_configuration_transaction(
        &self,
        hardware: &mut dyn Hardware,
        request: &Mutation,
        resumed: bool,
    ) {
        let id = request.device_id();
        let operation_id = request.operation_id();
        let planned = hardware.configuration_plan(request);
        let initialized = async {
            let plan = planned?.ok_or(Error("UNSUPPORTED_CAPABILITY"))?;
            let mut state = self.state.lock().await;
            let entry = &state.devices[id];
            if entry.device.connection_state != "ready" {
                return Err(Error("DEVICE_NOT_READY"));
            }
            if !resumed && entry.document.base_token != request.base_token() {
                return Err(Error("BASE_CONFLICT"));
            }
            let pause = entry
                .upload_pause
                .as_ref()
                .ok_or(Error("OPERATION_NOT_PAUSED"))?
                .1
                .subscribe();
            let before = plan
                .groups
                .iter()
                .map(|g| {
                    entry
                        .document
                        .readback
                        .as_ref()
                        .and_then(|r| g.field.observe(r))
                })
                .collect();
            let readback_required = entry.document.readback.is_some();
            let record = state.journal.operations.get_mut(operation_id).unwrap();
            if !resumed {
                record.configuration_transaction = Some(Checkpoint {
                    before,
                    acknowledged: vec![false; plan.groups.len()],
                    in_flight: None,
                    save_acknowledged: false,
                    readback_required,
                });
                record.operation.configuration_progress = Some(ConfigurationProgress {
                    fields_total: plan.groups.len(),
                    fields_acknowledged: 0,
                    fields_verified: 0,
                    phase: "writing".into(),
                    save_acknowledged: false,
                });
            }
            let checkpoint = record
                .configuration_transaction
                .as_ref()
                .ok_or(Error("OPERATION_NOT_PAUSED"))?;
            if checkpoint.before.len() != plan.groups.len()
                || checkpoint.acknowledged.len() != plan.groups.len()
            {
                return Err(Error("STORAGE_CORRUPT"));
            }
            record.operation.status = "running".into();
            record.operation.error_code = None;
            record.operation.effect = "unknown".into();
            record
                .operation
                .configuration_progress
                .as_mut()
                .unwrap()
                .phase = if resumed { "verifying" } else { "writing" }.into();
            if resumed {
                record
                    .operation
                    .configuration_progress
                    .as_mut()
                    .unwrap()
                    .fields_verified = 0;
            }
            Ok((plan, pause))
        }
        .await;
        let (plan, pause) = match initialized {
            Ok(value) => value,
            Err(error) => {
                self.finish_apply(request, Some(error), resumed).await;
                return;
            }
        };
        let result = async {
            self.checkpoint(operation_id).await?;
            self.run_configuration_transaction(hardware, request, &plan, pause, resumed)
                .await
        }
        .await;
        if result.as_ref().is_err_and(|e| {
            matches!(
                e.0,
                "DEVICE_TIMEOUT" | "DEVICE_DISCONNECTED" | "DEVICE_INVALID_RESPONSE"
            )
        }) {
            let _ = hardware.disconnect().await;
            self.disconnected(id).await;
        }
        let paused = result.is_err();
        self.finish_apply(request, result.err(), paused).await;
    }
    async fn run_configuration_transaction(
        &self,
        hardware: &mut dyn Hardware,
        request: &Mutation,
        plan: &Plan,
        pause: watch::Receiver<bool>,
        resumed: bool,
    ) -> Result<()> {
        let operation_id = request.operation_id();
        let mut skip = vec![false; plan.groups.len()];
        if *pause.borrow() {
            return Err(Error("CONFIGURATION_PAUSED"));
        }
        if resumed {
            let read = tokio::time::timeout(Duration::from_secs(50), hardware.read_configuration())
                .await
                .unwrap_or(Err(Error("DEVICE_TIMEOUT")))?
                .ok_or(Error("CONFIGURATION_UNVERIFIABLE"))?;
            let mut state = self.state.lock().await;
            let record = state.journal.operations.get_mut(operation_id).unwrap();
            let checkpoint = record.configuration_transaction.as_mut().unwrap();
            // Validate every field before writing any: a third value is an external conflict.
            for (index, group) in plan.groups.iter().enumerate() {
                let current = group
                    .field
                    .observe(&read)
                    .ok_or(Error("CONFIGURATION_UNVERIFIABLE"))?;
                if current == group.desired {
                    skip[index] = true;
                } else if checkpoint.before[index].as_ref() != Some(&current) {
                    return Err(Error("CONFIGURATION_CONFLICT"));
                }
            }
            checkpoint.acknowledged = skip.clone();
            checkpoint.in_flight = None;
            // If any field needs rewriting, an earlier save ACK cannot cover the new write.
            if skip.iter().any(|v| !v) {
                checkpoint.save_acknowledged = false;
            }
            let progress = record.operation.configuration_progress.as_mut().unwrap();
            progress.fields_verified = skip.iter().filter(|v| **v).count();
            progress.fields_acknowledged = progress.fields_verified;
            progress.save_acknowledged = checkpoint.save_acknowledged;
            progress.phase = "writing".into();
            drop(state);
            self.checkpoint(operation_id).await?;
        }
        for (index, group) in plan.groups.iter().enumerate() {
            if *pause.borrow() {
                return Err(Error("CONFIGURATION_PAUSED"));
            }
            if skip[index] {
                continue;
            }
            {
                let mut state = self.state.lock().await;
                state
                    .journal
                    .operations
                    .get_mut(operation_id)
                    .unwrap()
                    .configuration_transaction
                    .as_mut()
                    .unwrap()
                    .in_flight = Some(index);
            }
            self.checkpoint(operation_id).await?;
            // No pause between the two frames that replace a key's action layer.
            for command in &group.commands {
                hardware.write_confirmed(command).await?;
            }
            {
                let mut state = self.state.lock().await;
                let record = state.journal.operations.get_mut(operation_id).unwrap();
                let checkpoint = record.configuration_transaction.as_mut().unwrap();
                checkpoint.in_flight = None;
                checkpoint.acknowledged[index] = true;
                let count = checkpoint.acknowledged.iter().filter(|v| **v).count();
                record
                    .operation
                    .configuration_progress
                    .as_mut()
                    .unwrap()
                    .fields_acknowledged = count;
                record.operation.progress = count as f64 / (plan.groups.len() + 1) as f64;
                record.operation.effect = "partial".into();
            }
            self.checkpoint(operation_id).await?;
        }
        if *pause.borrow() {
            return Err(Error("CONFIGURATION_PAUSED"));
        }
        // Readable firmware must verify field values before the save command. Legacy firmware
        // stays ACK-only and exposes zero verified fields; it cannot silently claim readback.
        let required = self.state.lock().await.journal.operations[operation_id]
            .configuration_transaction
            .as_ref()
            .unwrap()
            .readback_required
            || resumed;
        if required {
            {
                let mut state = self.state.lock().await;
                state
                    .journal
                    .operations
                    .get_mut(operation_id)
                    .unwrap()
                    .operation
                    .configuration_progress
                    .as_mut()
                    .unwrap()
                    .phase = "verifying".into();
            }
            self.checkpoint(operation_id).await?;
            let read = tokio::time::timeout(Duration::from_secs(50), hardware.read_configuration())
                .await
                .unwrap_or(Err(Error("DEVICE_TIMEOUT")))?
                .ok_or(Error("CONFIGURATION_UNVERIFIABLE"))?;
            for group in &plan.groups {
                if group.field.observe(&read).as_ref() != Some(&group.desired) {
                    return Err(Error("CONFIGURATION_VERIFY_FAILED"));
                }
            }
            let mut state = self.state.lock().await;
            state
                .journal
                .operations
                .get_mut(operation_id)
                .unwrap()
                .operation
                .configuration_progress
                .as_mut()
                .unwrap()
                .fields_verified = plan.groups.len();
            state
                .devices
                .get_mut(request.device_id())
                .unwrap()
                .document
                .configuration = read.complete_configuration();
        }
        if *pause.borrow() {
            return Err(Error("CONFIGURATION_PAUSED"));
        }
        if !self.state.lock().await.journal.operations[operation_id]
            .configuration_transaction
            .as_ref()
            .unwrap()
            .save_acknowledged
        {
            {
                let mut state = self.state.lock().await;
                state
                    .journal
                    .operations
                    .get_mut(operation_id)
                    .unwrap()
                    .operation
                    .configuration_progress
                    .as_mut()
                    .unwrap()
                    .phase = "saving".into();
            }
            self.checkpoint(operation_id).await?;
            hardware.write_confirmed(&plan.save).await?;
            {
                let mut state = self.state.lock().await;
                let record = state.journal.operations.get_mut(operation_id).unwrap();
                record
                    .configuration_transaction
                    .as_mut()
                    .unwrap()
                    .save_acknowledged = true;
                record
                    .operation
                    .configuration_progress
                    .as_mut()
                    .unwrap()
                    .save_acknowledged = true;
            }
            self.checkpoint(operation_id).await?;
        }
        Ok(())
    }
}
