use super::state::X1State;
use crate::ahakey::state::HardwareState;
use crate::{
    ahakey::x1::commands::valid_frame,
    error::{Error, Result},
};
use btleplug::api::{ValueNotification, bleuuid::uuid_from_u16};
use futures_util::{Stream, StreamExt};
use std::pin::Pin;

mod framing;

/// One consumer routes both command replies and unsolicited X1 state updates.
/// The most recent state survives notifications interleaved with command ACKs.
pub(super) struct Notifications {
    pub frames: Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
    pub disconnects: Pin<Box<dyn Stream<Item = ()> + Send>>,
    decoder: framing::Frames,
    state: Option<HardwareState>,
    changed: bool,
    device_id: Option<String>,
}

impl Notifications {
    pub fn new(
        frames: Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
        disconnects: Pin<Box<dyn Stream<Item = ()> + Send>>,
    ) -> Self {
        Self {
            frames,
            disconnects,
            decoder: framing::Frames::default(),
            state: None,
            changed: false,
            device_id: None,
        }
    }

    pub fn with_device(mut self, device_id: String) -> Self {
        self.device_id = Some(device_id);
        self
    }

    async fn next_frame(&mut self) -> Result<Vec<u8>> {
        loop {
            if let Some(frame) = self.decoder.next() {
                crate::debug::event("ble.frame", || {
                    serde_json::json!({
                        "deviceId":self.device_id, "frame":crate::debug::bytes(&frame),
                    })
                });
                return Ok(frame);
            }
            let notification = tokio::select! {
                biased;
                _ = self.disconnects.next() => {
                    self.decoder = framing::Frames::default();
                    crate::debug::event("ble.linkLost", || serde_json::json!({"deviceId":self.device_id}));
                    return Err(Error("DEVICE_DISCONNECTED"));
                },
                frame = self.frames.next() => match frame {
                    Some(frame) => frame,
                    None => {
                        self.decoder = framing::Frames::default();
                        return Err(Error("DEVICE_DISCONNECTED"));
                    }
                },
            };
            crate::debug::event("ble.rx", || {
                serde_json::json!({
                    "deviceId":self.device_id, "service":notification.service_uuid.to_string(),
                    "characteristic":notification.uuid.to_string(),
                    "validFrame":valid_frame(&notification.value),
                    "frame":crate::debug::bytes(&notification.value),
                })
            });
            if notification.service_uuid == uuid_from_u16(0x7340)
                && notification.uuid == uuid_from_u16(0x7344)
                && let Err(error) = self.decoder.push(&notification.value)
            {
                self.decoder = framing::Frames::default();
                return Err(error);
            }
        }
    }

    fn observe(&mut self, data: &[u8]) {
        if let Some(state) = X1State::from_bytes(data) {
            self.changed |= self.state.as_ref() != Some(&state.0);
            self.state = Some(state.0);
        } else if data.len() == 9
            && data[2] == 0x9b
            && data[3] == 0
            && data[4] < 4
            && data[5] <= 2
            && let Some(state) = &mut self.state
        {
            // Sequence is one byte and wraps. BLE delivers notifications in order;
            // never compare it as an unbounded monotonically increasing counter.
            self.changed |= state.work_mode != data[4];
            state.work_mode = data[4];
        }
    }

    pub async fn response(&mut self, opcode: u8) -> Result<Vec<u8>> {
        loop {
            let data = self.next_frame().await?;
            self.observe(&data);
            if data[2] != opcode {
                crate::debug::event("ble.interleaved", || {
                    serde_json::json!({
                        "deviceId":self.device_id, "awaitingOpcode":opcode, "receivedOpcode":data[2],
                    })
                });
                continue;
            }
            if opcode == 0 {
                if X1State::from_bytes(&data).is_some() {
                    return Ok(data);
                }
                crate::debug::event(
                    "ble.invalidStatus",
                    || serde_json::json!({"deviceId":self.device_id}),
                );
            } else if data.len() >= 6 {
                return if data[3] == 0
                    || (opcode == 0x87 && data.len() == 6 && matches!(data[3], 2 | 3))
                {
                    Ok(data)
                } else {
                    Err(Error("DEVICE_REJECTED"))
                };
            }
        }
    }

    /// Cancellation-safe: no await between consuming a frame and recording its state.
    pub async fn next_state(&mut self) -> Result<HardwareState> {
        loop {
            if self.changed {
                self.changed = false;
                if let Some(state) = &self.state {
                    return Ok(state.clone());
                }
            }
            let data = self.next_frame().await?;
            self.observe(&data);
        }
    }
}

#[cfg(test)]
mod tests;
