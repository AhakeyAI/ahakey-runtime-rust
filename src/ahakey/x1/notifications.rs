use super::state::X1State;
use crate::ahakey::state::HardwareState;
use crate::{
    ahakey::x1::commands::valid_frame,
    error::{Error, Result},
};
use btleplug::api::{ValueNotification, bleuuid::uuid_from_u16};
use futures_util::{Stream, StreamExt};
use std::pin::Pin;

/// One consumer routes both command replies and unsolicited X1 state updates.
/// The most recent state survives notifications interleaved with command ACKs.
pub(super) struct Notifications {
    pub frames: Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
    pub disconnects: Pin<Box<dyn Stream<Item = ()> + Send>>,
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
            let notification = tokio::select! {
                biased;
                _ = self.disconnects.next() => {
                    crate::debug::event("ble.linkLost", || serde_json::json!({"deviceId":self.device_id}));
                    return Err(Error("DEVICE_DISCONNECTED"));
                },
                frame = self.frames.next() => frame.ok_or(Error("DEVICE_DISCONNECTED"))?,
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
                && valid_frame(&notification.value)
            {
                return Ok(notification.value);
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
mod tests {
    use super::*;
    use futures_util::stream;

    fn session(frames: Vec<Vec<u8>>) -> Notifications {
        Notifications::new(
            Box::pin(
                stream::iter(frames.into_iter().map(|value| ValueNotification {
                    service_uuid: uuid_from_u16(0x7340),
                    uuid: uuid_from_u16(0x7344),
                    value,
                }))
                .chain(stream::pending()),
            ),
            Box::pin(stream::pending()),
        )
    }
    fn status(mode: u8, brightness: u8) -> Vec<u8> {
        vec![
            0xaa, 0xbb, 0, 82, 0, 1, 4, mode, 0, 1, brightness, 0xcc, 0xdd,
        ]
    }
    fn mode(mode: u8, sequence: u8) -> Vec<u8> {
        vec![0xaa, 0xbb, 0x9b, 0, mode, 1, sequence, 0xcc, 0xdd]
    }

    #[tokio::test]
    async fn only_exact_key_read_status_two_and_three_reach_the_decoder() {
        use crate::ahakey::x1::commands::frame;
        for code in [2, 3] {
            let error = frame(0x87, &[code]);
            assert_eq!(
                session(vec![error.clone()]).response(0x87).await.unwrap(),
                error
            );
        }
        for (opcode, payload) in [
            (0x87, vec![1]),
            (0x87, vec![2, 0]),
            (0x87, vec![3, 0]),
            (0x85, vec![3]),
        ] {
            assert_eq!(
                session(vec![frame(opcode, &payload)])
                    .response(opcode)
                    .await
                    .unwrap_err()
                    .0,
                "DEVICE_REJECTED"
            );
        }
    }

    #[tokio::test]
    async fn lever_status_with_connection_flags_survives_unrelated_capability_reply() {
        // Extended status frames may append connection flags.
        let status = |lever| vec![0xaa, 0xbb, 0, 92, 50, 1, 4, 2, 0, lever, 35, 3, 0xcc, 0xdd];
        let up = status(0);
        let mut input = session(vec![
            up.clone(),
            up,
            vec![
                0xaa, 0xbb, 0x9f, 0, 3, 2, 1, 4, 1, 0xff, 7, 0, 0, 6, 1, 0xcc, 0xdd,
            ],
            status(1),
            status(2),
            status(0),
        ]);
        input.response(0).await.unwrap();
        for lever in [Some("automatic"), Some("manual"), None, Some("automatic")] {
            let state =
                tokio::time::timeout(std::time::Duration::from_millis(100), input.next_state())
                    .await
                    .expect("a lever change must not wait for another command")
                    .unwrap();
            assert_eq!(state.lever, lever);
            assert_eq!(state.work_mode, 2);
        }
        // An idle connected device waits; silence is not DEVICE_TIMEOUT.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), input.next_state(),)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn mode_notifications_survive_ack_wait_and_brightness_confirmation() {
        let mut input = session(vec![
            status(0, 35),
            mode(3, 255),
            vec![0xaa, 0xbb, 0x85, 0, 0xcc, 0xdd],
            status(3, 61),
            mode(1, 0),
        ]);
        input.response(0).await.unwrap();
        assert_eq!(input.next_state().await.unwrap().work_mode, 0);
        input.response(0x85).await.unwrap();
        input.response(0).await.unwrap();
        let state = input.next_state().await.unwrap();
        assert_eq!(state.work_mode, 3);
        assert_eq!(state.brightness, Some(61));
        assert_eq!(input.next_state().await.unwrap().work_mode, 1);
    }

    #[tokio::test]
    async fn invalid_mode_frames_do_not_change_state_or_complete_other_commands() {
        let mut invalid = mode(2, 1);
        invalid[3] = 1;
        let mut input = session(vec![
            status(0, 35),
            mode(4, 1),
            invalid,
            mode(2, 2),
            vec![0xaa, 0xbb, 0x85, 0, 0xcc, 0xdd],
        ]);
        input.response(0).await.unwrap();
        input.next_state().await.unwrap();
        input.response(0x85).await.unwrap();
        assert_eq!(input.next_state().await.unwrap().work_mode, 2);
    }

    #[tokio::test]
    async fn link_loss_wakes_idle_notification_reader() {
        let mut input = session(vec![]);
        input.disconnects = Box::pin(stream::iter([()]));
        assert_eq!(
            input.next_state().await.unwrap_err().0,
            "DEVICE_DISCONNECTED"
        );
    }
}
