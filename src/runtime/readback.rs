//! Explicit, serialized hardware observation. Reading never sends configuration writes.
use super::{Runtime, state::Job};
use crate::{
    ahakey::hardware::Hardware,
    error::{Error, Result},
    wire::ConfigurationRead,
};
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

impl Runtime {
    pub async fn read_configuration(&self, id: &str) -> Result<ConfigurationRead> {
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
            .try_send(Job::ReadConfiguration)
            .map_err(|_| Error("BUSY"))?;
        entry.busy = true;
        let read = ConfigurationRead {
            status: "reading".into(),
            attempt: 0,
            max_attempts: 3,
            error_code: None,
        };
        entry.document.read = Some(read.clone());
        // Previously known fields must not masquerade as this request's observation.
        state.invalidate(id);
        Ok(read)
    }

    pub(super) async fn execute_read_configuration(&self, id: &str, hardware: &mut dyn Hardware) {
        let mut reconnect = false;
        let mut failure = Error("UNSUPPORTED_CONFIGURATION");
        for attempt in 1..=3 {
            {
                let mut state = self.state.lock().await;
                state
                    .devices
                    .get_mut(id)
                    .unwrap()
                    .document
                    .read
                    .as_mut()
                    .unwrap()
                    .attempt = attempt;
                state.emit("configuration.changed", json!({"deviceId": id}));
            }
            let result = tokio::time::timeout(Duration::from_secs(65), async {
                if reconnect {
                    hardware.connect().await?;
                }
                let readback = hardware
                    .read_configuration()
                    .await?
                    .ok_or(Error("UNSUPPORTED_CONFIGURATION"))?;
                let status = hardware.refresh().await?;
                let expected = (1..=100)
                    .contains(&readback.device.brightness_percent)
                    .then_some(readback.device.brightness_percent);
                if status.brightness != expected {
                    return Err(Error("CONFIGURATION_CHANGED"));
                }
                Ok((status, readback))
            })
            .await
            .unwrap_or(Err(Error("DEVICE_TIMEOUT")));
            match result {
                Ok((status, readback)) => {
                    // Publish status before installing the new baseline; it may invalidate the old one.
                    self.publish_hardware(id, &status).await;
                    let mut state = self.state.lock().await;
                    let entry = state.devices.get_mut(id).unwrap();
                    entry.document.configuration = readback.complete_configuration();
                    entry.document.readback = Some(readback);
                    entry.document.base_token = Uuid::new_v4().to_string();
                    let read = entry.document.read.as_mut().unwrap();
                    read.status = "completed".into();
                    read.error_code = None;
                    entry.busy = false;
                    state.emit("configuration.changed", json!({"deviceId": id}));
                    return;
                }
                Err(error) => {
                    failure = error;
                    if matches!(
                        failure.0,
                        "UNSUPPORTED_CONFIGURATION" | "UNSUPPORTED_CAPABILITY"
                    ) {
                        break;
                    }
                    reconnect = failure.0 != "CONFIGURATION_CHANGED";
                    if reconnect {
                        let _ = hardware.disconnect().await;
                        self.disconnected(id).await;
                    }
                }
            }
            if self.state.lock().await.stopping {
                break;
            }
            if attempt < 3 {
                tokio::time::sleep(Duration::from_millis(300 * attempt as u64)).await;
            }
        }
        let mut state = self.state.lock().await;
        let entry = state.devices.get_mut(id).unwrap();
        let read = entry.document.read.as_mut().unwrap();
        read.status = "failed".into();
        read.error_code = Some(failure.0.into());
        entry.busy = false;
        state.emit("configuration.changed", json!({"deviceId": id}));
    }
}
