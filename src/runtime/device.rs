use super::{Runtime, state::Job};
use crate::{
    ahakey::state::HardwareState,
    error::{Error, Result},
};
use serde_json::json;

impl Runtime {
    pub async fn connect(&self, id: &str, connected: bool) -> Result<()> {
        let mut state = self.state.lock().await;
        if state.stopping {
            return Err(Error("RUNTIME_STOPPING"));
        }
        let entry = state.devices.get_mut(id).ok_or(Error("DEVICE_NOT_FOUND"))?;
        if entry.busy {
            return Err(Error("BUSY"));
        }
        entry
            .sender
            .try_send(if connected {
                Job::Connect
            } else {
                Job::Disconnect
            })
            .map_err(|_| Error("BUSY"))?;
        entry.busy = true;
        entry.desired_connected = connected;
        Ok(())
    }
    pub(super) async fn publish_hardware(&self, id: &str, hardware: &HardwareState) {
        crate::debug::event("device.state", || {
            json!({
                "deviceId":id, "workMode":hardware.work_mode, "lightMode":hardware.light_mode,
                "battery":hardware.battery, "lever":hardware.lever, "brightness":hardware.brightness,
            })
        });
        let mut state = self.state.lock().await;
        let entry = state.devices.get_mut(id).unwrap();
        let old = entry.device.clone();
        entry.device.connection_state = "ready".into();
        entry.device.battery_percent = hardware.battery;
        entry.device.work_mode = Some(hardware.work_mode);
        entry.device.lever = hardware.lever.map(str::to_owned);
        entry.lighting_supported = hardware.brightness.is_some();
        let device = entry.device.clone();
        let known_brightness = entry
            .document
            .configuration
            .as_ref()
            .map(|config| config.device.brightness_percent)
            .or_else(|| {
                entry
                    .document
                    .readback
                    .as_ref()
                    .map(|readback| readback.device.brightness_percent)
            });
        let external_change = known_brightness.is_some_and(|brightness| {
            (1..=100).contains(&brightness).then_some(brightness) != hardware.brightness
        });
        if external_change {
            state.invalidate(id);
        }
        if old != device {
            state.emit("device.changed", json!(device));
        }
    }
    pub(super) async fn disconnected(&self, id: &str) {
        let mut state = self.state.lock().await;
        let entry = state.devices.get_mut(id).unwrap();
        let changed = entry.device.connection_state != "disconnected"
            || entry.document.configuration.is_some()
            || entry.document.readback.is_some();
        entry.device.clear("disconnected");
        entry.lighting_supported = false;
        let device = entry.device.clone();
        if changed {
            state.invalidate(id);
            state.emit("device.changed", json!(device));
        }
    }
    pub(super) async fn set_connecting(&self, id: &str) {
        let mut state = self.state.lock().await;
        let entry = state.devices.get_mut(id).unwrap();
        entry.device.clear("connecting");
        let device = entry.device.clone();
        state.emit("device.changed", json!(device));
    }
}
