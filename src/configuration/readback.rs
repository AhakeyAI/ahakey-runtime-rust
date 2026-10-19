use super::{Action, Configuration, DeviceFields, Key, Light, Mode};
use serde::{Deserialize, Serialize};

/// Device-observed fields. Missing descriptions or unrepresentable actions stay unknown.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationReadback {
    pub device: ReadbackDevice,
    pub modes: Vec<ReadbackMode>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReadbackDevice {
    pub brightness_percent: u8,
    pub task_light_mode: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadbackMode {
    pub mode: u8,
    pub keys: Vec<ReadbackKey>,
    pub lights: Vec<Light>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReadbackKey {
    pub role: String,
    pub action: Option<Action>,
    pub description: Option<String>,
    pub raw_binding: Vec<u8>,
}
impl ConfigurationReadback {
    /// A complete baseline requires every writable field to be device-observed.
    pub fn complete_configuration(&self) -> Option<Configuration> {
        if self.modes.len() != 4 {
            return None;
        }
        let config = Configuration {
            device: DeviceFields {
                brightness_percent: self.device.brightness_percent,
            },
            modes: self
                .modes
                .iter()
                .map(|mode| {
                    Some(Mode {
                        mode: mode.mode,
                        keys: mode
                            .keys
                            .iter()
                            .map(|key| {
                                Some(Key {
                                    role: key.role.clone(),
                                    action: key.action.clone()?,
                                    description: key.description.clone()?,
                                })
                            })
                            .collect::<Option<Vec<_>>>()?,
                        lights: mode.lights.clone(),
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        };
        config.validate().ok()?;
        Some(config)
    }
}
