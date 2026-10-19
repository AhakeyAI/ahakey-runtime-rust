use crate::configuration::{Configuration, ConfigurationReadback};
use serde::{Deserialize, Serialize};

pub const METHODS: &[&str] = &[
    "runtime.subscribe",
    "configuration.get",
    "configuration.apply",
    "keymap.apply",
    "operation.get",
    "device.connect",
    "device.disconnect",
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub device_id: String,
    pub name: String,
    pub connection_state: String,
    pub battery_percent: Option<u8>,
    pub work_mode: Option<u8>,
    pub lever: Option<String>,
}
impl Device {
    pub fn disconnected(device_id: String, name: String) -> Self {
        Self {
            device_id,
            name,
            connection_state: "disconnected".into(),
            battery_percent: None,
            work_mode: None,
            lever: None,
        }
    }
    pub fn clear(&mut self, state: &str) {
        self.connection_state = state.into();
        self.battery_percent = None;
        self.work_mode = None;
        self.lever = None;
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub operation_id: String,
    pub device_id: String,
    pub status: String,
    pub progress: f64,
    pub effect: String,
    pub error_code: Option<String>,
}
impl Operation {
    pub fn terminal(&self) -> bool {
        matches!(self.status.as_str(), "completed" | "failed" | "cancelled")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationDocument {
    pub device_id: String,
    pub base_token: String,
    pub configuration: Option<Configuration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readback: Option<ConfigurationReadback>,
}
