//! Host HID observations are independent of BLE configuration and physical key identities.
use crate::runtime::Runtime;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tokio::{io::AsyncBufReadExt, process::Command};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InputDevice {
    pub input_device_id: String,
    pub name: String,
    pub pressed_usages: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InputState {
    pub status: String,
    pub permission: String,
    pub devices: Vec<InputDevice>,
}
impl Default for InputState {
    fn default() -> Self {
        Self {
            status: "disabled".into(),
            permission: "unknown".into(),
            devices: vec![],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyEvent {
    pub input_device_id: String,
    pub name: String,
    pub usage: u32,
    pub pressed: bool,
    // Decimal string: monotonic HID time, not wall-clock time or a physical switch timestamp.
    pub timestamp_ms: String,
    pub held_ms: Option<f64>,
}

#[derive(Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum InputMessage {
    #[serde(rename = "state")]
    State(InputState),
    #[serde(rename = "key")]
    Key(KeyEvent),
}

impl InputState {
    pub fn apply_key(&mut self, event: &KeyEvent) -> bool {
        if self.status != "listening" || !(4..=255).contains(&event.usage) {
            return false;
        }
        let Some(device) = self
            .devices
            .iter_mut()
            .find(|d| d.input_device_id == event.input_device_id)
        else {
            return false;
        };
        let was_pressed = device.pressed_usages.contains(&event.usage);
        if was_pressed == event.pressed {
            return false;
        }
        if event.pressed {
            device.pressed_usages.push(event.usage);
            device.pressed_usages.sort_unstable();
        } else {
            device.pressed_usages.retain(|usage| *usage != event.usage);
        }
        true
    }
}

/// The native adapter is bundled beside the Runtime, inherits its app identity,
/// and exits on stdin EOF. Dropping this task also kills the child.
pub async fn monitor(runtime: Arc<Runtime>, executable: PathBuf) {
    runtime
        .input_message(InputMessage::State(InputState {
            status: "starting".into(),
            permission: "unknown".into(),
            devices: vec![],
        }))
        .await;
    let result = async {
        let mut child = Command::new(executable)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        while let Some(line) = lines.next_line().await? {
            if line.len() > 16_384 {
                return Err(std::io::Error::other("Oversized input event"));
            }
            let message: InputMessage =
                serde_json::from_str(&line).map_err(std::io::Error::other)?;
            runtime.input_message(message).await;
        }
        child.wait().await?;
        Ok::<(), std::io::Error>(())
    }
    .await;
    if let Err(error) = result {
        eprintln!("Input monitor: {error}");
    }
    runtime
        .input_message(InputMessage::State(InputState {
            status: "error".into(),
            permission: "unknown".into(),
            devices: vec![],
        }))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_each_device_and_rejects_duplicates_and_unknown_sources() {
        let mut state = InputState {
            status: "listening".into(),
            permission: "granted".into(),
            devices: vec![
                InputDevice {
                    input_device_id: "hid-1".into(),
                    name: "AhaKey".into(),
                    pressed_usages: vec![],
                },
                InputDevice {
                    input_device_id: "hid-2".into(),
                    name: "AhaKey".into(),
                    pressed_usages: vec![],
                },
            ],
        };
        let mut event = KeyEvent {
            input_device_id: "hid-1".into(),
            name: "AhaKey".into(),
            usage: 109,
            pressed: true,
            timestamp_ms: "1234".into(),
            held_ms: None,
        };
        assert!(state.apply_key(&event));
        assert!(!state.apply_key(&event));
        assert!(state.devices[1].pressed_usages.is_empty());
        event.input_device_id = "other".into();
        assert!(!state.apply_key(&event));
        event.input_device_id = "hid-1".into();
        event.pressed = false;
        assert!(state.apply_key(&event));
        assert!(state.devices[0].pressed_usages.is_empty());
        state.status = "permissionDenied".into();
        event.pressed = true;
        assert!(!state.apply_key(&event));
    }
}
