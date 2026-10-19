//! X1 readback matches remote eternal-dev bc4f6e40dc11b486af9cedba528d24844e6225a9.
//! Firmware 1.4.8 / protocol 3.2 exposes resources 00..02 and capability bit 10.
use crate::{
    configuration::{
        Action, ConfigurationReadback, EFFECTS, Light, MODIFIERS, MacroStep, ROLES, ReadbackDevice,
        ReadbackKey, ReadbackMode, STATES,
    },
    error::{Error, Result},
    protocol::x1::{frame, valid_frame},
};
use async_trait::async_trait;

const REQUIRED_CAPABILITY: u32 = 1 << 10;

#[async_trait]
pub(super) trait ReadTransport: Send {
    async fn query(&mut self, command: &[u8]) -> Result<Vec<u8>>;
}
fn invalid() -> Error {
    Error("DEVICE_INVALID_RESPONSE")
}
fn capabilities(response: &[u8]) -> Result<bool> {
    if !valid_frame(response) || response[2] != 0x9f || response.get(3) != Some(&0) {
        return Err(invalid());
    }
    if response.len() == 6 {
        return Ok(false);
    }
    if response.len() < 15 {
        return Err(invalid());
    }
    let flags = u32::from_le_bytes(response[9..13].try_into().unwrap());
    if response[4] != 3 || flags & REQUIRED_CAPABILITY == 0 {
        return Ok(false);
    }
    if response.len() < 17 {
        return Err(invalid());
    }
    Ok(response[14] == 1)
}
async fn resource(
    transport: &mut impl ReadTransport,
    kind: u8,
    index: u8,
    total: u8,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(total as usize);
    while bytes.len() < total as usize {
        let offset = bytes.len() as u8;
        let response = transport
            .query(&frame(0x9d, &[kind, index, offset]))
            .await?;
        let count = (total - offset).min(8);
        if !valid_frame(&response)
            || response.len() != 11 + count as usize
            || response[2..9] != [0x9d, 0, kind, index, total, offset, count]
        {
            return Err(invalid());
        }
        bytes.extend_from_slice(&response[9..9 + count as usize]);
    }
    Ok(bytes)
}
#[derive(PartialEq, Eq)]
struct RawConfiguration {
    bindings: Vec<Vec<u8>>,
    lights: Vec<Vec<u8>>,
    global: Vec<u8>,
}
async fn collect(transport: &mut impl ReadTransport) -> Result<RawConfiguration> {
    let mut bindings = Vec::new();
    let mut lights = Vec::new();
    for index in 0..16 {
        bindings.push(resource(transport, 0, index, 100).await?);
    }
    for mode in 0..4 {
        lights.push(resource(transport, 1, mode, 9).await?);
    }
    let global = resource(transport, 2, 0, 2).await?;
    Ok(RawConfiguration {
        bindings,
        lights,
        global,
    })
}
/// Firmware has no revision field. Publish two matching complete collections,
/// keeping writes serialized. Descriptions are not exposed by this firmware.
pub(super) async fn read(
    transport: &mut impl ReadTransport,
) -> Result<Option<ConfigurationReadback>> {
    if !capabilities(&transport.query(&frame(0x9f, &[])).await?)? {
        return Ok(None);
    }
    let mut previous = collect(transport).await?;
    for _ in 0..2 {
        let current = collect(transport).await?;
        if current == previous {
            return decode(current).map(Some);
        }
        previous = current;
    }
    Err(Error("CONFIGURATION_CHANGED"))
}
fn decode(raw: RawConfiguration) -> Result<ConfigurationReadback> {
    if raw.global[0] > 100 || raw.global[1] > 1 {
        return Err(invalid());
    }
    let mut modes = Vec::new();
    for (mode, light_map) in raw.lights.iter().enumerate() {
        let mut keys = Vec::new();
        for (key, role) in ROLES.iter().enumerate() {
            let raw_binding = raw.bindings[mode * 4 + key].clone();
            // A stored binding can be outside the host model (including an
            // incomplete macro). Keep the bytes rather than losing the device.
            let action = action(&raw_binding).ok();
            keys.push(ReadbackKey {
                role: (*role).into(),
                action,
                description: None,
                raw_binding,
            });
        }
        let lights = STATES
            .iter()
            .zip(light_map)
            .map(|(state, effect)| {
                Ok(Light {
                    state: (*state).into(),
                    effect: EFFECTS
                        .get(*effect as usize)
                        .ok_or_else(invalid)?
                        .to_string(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        modes.push(ReadbackMode {
            mode: mode as u8,
            keys,
            lights,
        });
    }
    Ok(ConfigurationReadback {
        device: ReadbackDevice {
            brightness_percent: raw.global[0],
            task_light_mode: raw.global[1],
        },
        modes,
    })
}

fn action(bytes: &[u8]) -> Result<Action> {
    let unsupported = || Error("UNSUPPORTED_CONFIGURATION");
    if bytes.len() != 100 || bytes[1] > 98 {
        return Err(invalid());
    }
    let payload = &bytes[2..2 + bytes[1] as usize];
    match bytes[0] {
        // Factory defaults can contain an all-zero, unassigned binding.
        0 if bytes.iter().all(|byte| *byte == 0) => Ok(Action::Disabled {}),
        0x73 | 0x74 if payload.is_empty() => Ok(Action::Disabled {}),
        0x73 => {
            let mut modifiers = Vec::new();
            let mut key_code = 0;
            for code in payload {
                if (0xe0..=0xe3).contains(code) {
                    let modifier = MODIFIERS[(code - 0xe0) as usize].to_string();
                    if modifiers.contains(&modifier) {
                        return Err(unsupported());
                    }
                    modifiers.push(modifier);
                } else if *code != 0 && !(0xe4..=0xe7).contains(code) && key_code == 0 {
                    key_code = *code;
                } else {
                    // Right modifiers and multiple ordinary keys cannot be
                    // represented by the current Studio Action contract.
                    return Err(unsupported());
                }
            }
            modifiers.sort();
            Ok(Action::Shortcut {
                modifiers,
                key_code,
            })
        }
        0x74 => {
            if !payload.len().is_multiple_of(2) {
                return Err(invalid());
            }
            let steps = payload
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| match pair {
                    [0, 0] => Ok(MacroStep::NoOp {}),
                    [1, code] => Ok(MacroStep::KeyDown { key_code: *code }),
                    [2, code] => Ok(MacroStep::KeyUp { key_code: *code }),
                    [3, delay] => Ok(MacroStep::Delay {
                        delay_ms: *delay as u16 * 3,
                    }),
                    [4, 0] => Ok(MacroStep::ReleaseAll {}),
                    _ => Err(unsupported()),
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Action::Macro { steps })
        }
        _ => Err(unsupported()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn unhex(value: &str) -> Vec<u8> {
        (0..value.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&value[i..i + 2], 16).unwrap())
            .collect()
    }
    struct Device {
        responses: BTreeMap<Vec<u8>, Vec<u8>>,
        queries: usize,
        change_at: Option<usize>,
        unstable: bool,
        corrupt: Option<usize>,
    }
    impl Device {
        fn new() -> Self {
            let fixture: serde_json::Value =
                serde_json::from_str(include_str!("../../tests/fixtures/x1-readback-1.4.8.json"))
                    .unwrap();
            Self {
                responses: fixture["responses"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(request, response)| (unhex(request), unhex(response.as_str().unwrap())))
                    .collect(),
                queries: 0,
                change_at: None,
                unstable: false,
                corrupt: None,
            }
        }
        fn binding(&mut self, value: &[u8]) {
            let mut bytes = [0; 100];
            bytes[..value.len()].copy_from_slice(value);
            // Mutate captured payloads; frame layout remains the remote C output.
            for (request, response) in &mut self.responses {
                if request[2..5] == [0x9d, 0, 0] {
                    let offset = request[5] as usize;
                    let count = response[8] as usize;
                    response[9..9 + count].copy_from_slice(&bytes[offset..offset + count]);
                }
            }
        }
        fn global(&mut self) -> &mut Vec<u8> {
            self.responses.get_mut(&frame(0x9d, &[2, 0, 0])).unwrap()
        }
    }
    #[async_trait]
    impl ReadTransport for Device {
        async fn query(&mut self, command: &[u8]) -> Result<Vec<u8>> {
            self.queries += 1;
            if self.change_at == Some(self.queries) {
                self.binding(&[0x73, 1, 9]);
            }
            if self.unstable && command == frame(0x9d, &[0, 0, 0]) {
                self.global()[9] ^= 1;
            }
            let mut response = self
                .responses
                .get(command)
                .expect("request must exist in remote fixture")
                .clone();
            if self.queries == 3
                && let Some(field) = self.corrupt
            {
                response[field] ^= 1;
            }
            Ok(response)
        }
    }

    #[tokio::test]
    async fn decodes_remote_c_responses_and_supported_actions_without_inventing_descriptions() {
        let mut device = Device::new();
        let observed = read(&mut device).await.unwrap().unwrap();
        assert_eq!(observed.device.brightness_percent, 35);
        assert_eq!(observed.device.task_light_mode, 1);
        assert_eq!(observed.modes.len(), 4);
        for mode in &observed.modes {
            assert_eq!(mode.keys.len(), 4);
            assert_eq!(mode.lights.len(), 9);
            for (index, key) in mode.keys.iter().enumerate() {
                assert_eq!(
                    key.action,
                    Some(Action::Shortcut {
                        modifiers: vec![],
                        key_code: 7 + mode.mode * 4 + index as u8
                    })
                );
                assert_eq!(key.description, None);
                assert_eq!(key.raw_binding.len(), 100);
            }
            for (index, light) in mode.lights.iter().enumerate() {
                assert_eq!(light.effect, EFFECTS[(mode.mode as usize + index) % 17]);
            }
        }
        assert!(observed.complete_configuration().is_none());

        for (binding, expected) in [
            (vec![0; 100], Action::Disabled {}),
            (
                vec![0x73, 2, 0xe0, 0xe3],
                Action::Shortcut {
                    modifiers: vec!["control".into(), "gui".into()],
                    key_code: 0,
                },
            ),
            (
                vec![0x74, 10, 1, 7, 3, 255, 2, 7, 4, 0, 0, 0],
                Action::Macro {
                    steps: vec![
                        MacroStep::KeyDown { key_code: 7 },
                        MacroStep::Delay { delay_ms: 765 },
                        MacroStep::KeyUp { key_code: 7 },
                        MacroStep::ReleaseAll {},
                        MacroStep::NoOp {},
                    ],
                },
            ),
        ] {
            device.binding(&binding);
            let observed = read(&mut device).await.unwrap().unwrap();
            assert_eq!(observed.modes[0].keys[0].action, Some(expected));
        }
    }

    #[tokio::test]
    async fn legacy_or_unsupported_devices_are_not_queried_for_configuration() {
        for caps in [
            vec![0xaa, 0xbb, 0x9f, 0, 0xcc, 0xdd],
            vec![
                0xaa, 0xbb, 0x9f, 0, 3, 2, 1, 4, 1, 0xff, 3, 0, 0, 8, 1, 0xcc, 0xdd,
            ],
            vec![
                0xaa, 0xbb, 0x9f, 0, 3, 2, 1, 4, 1, 0xff, 7, 0, 0, 8, 2, 0xcc, 0xdd,
            ],
        ] {
            let mut device = Device::new();
            device.responses.insert(frame(0x9f, &[]), caps);
            assert!(read(&mut device).await.unwrap().is_none());
            assert_eq!(device.queries, 1);
        }
    }

    #[tokio::test]
    async fn changed_data_is_retried_and_persistent_changes_return_an_error() {
        let mut device = Device::new();
        device.change_at = Some(100);
        let observed = read(&mut device).await.unwrap().unwrap();
        assert_eq!(
            observed.modes[0].keys[0].action,
            Some(Action::Shortcut {
                modifiers: vec![],
                key_code: 9
            })
        );
        device.unstable = true;
        assert_eq!(
            read(&mut device).await.unwrap_err().0,
            "CONFIGURATION_CHANGED"
        );
    }

    #[tokio::test]
    async fn mismatched_chunk_address_offset_or_length_is_rejected() {
        for field in [5, 6, 7, 8] {
            let mut device = Device::new();
            device.corrupt = Some(field);
            assert_eq!(
                read(&mut device).await.unwrap_err().0,
                "DEVICE_INVALID_RESPONSE"
            );
        }
    }

    #[tokio::test]
    async fn unrepresentable_bindings_and_zero_brightness_remain_observable() {
        for binding in [[0x73, 2, 7, 9], [0x73, 1, 0xe4, 0], [0x74, 1, 1, 0]] {
            let mut device = Device::new();
            device.binding(&binding);
            let observed = read(&mut device).await.unwrap().unwrap();
            assert!(observed.modes[0].keys[0].action.is_none());
            assert_eq!(&observed.modes[0].keys[0].raw_binding[..4], &binding);
        }
        let mut device = Device::new();
        device.global()[9] = 0;
        let observed = read(&mut device).await.unwrap().unwrap();
        assert_eq!(observed.device.brightness_percent, 0);
        assert!(observed.complete_configuration().is_none());
    }
}
