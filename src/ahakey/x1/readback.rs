//! Configuration readback for Fireware-harness 076382b (production sources 70ca334).
use super::{
    commands::{frame, valid_frame},
    transport::CommandTransport,
};
use crate::{
    configuration::{
        Action, ConfigurationReadback, EFFECTS, Light, MODIFIERS, MacroStep, ROLES, ReadbackDevice,
        ReadbackKey, ReadbackMode, STATES,
    },
    error::{Error, Result},
};

#[cfg(test)]
mod tests;

fn invalid() -> Error {
    Error("DEVICE_INVALID_RESPONSE")
}

fn structured(response: &[u8], opcode: u8) -> Result<()> {
    if response == frame(opcode, &[0]) {
        // The catalog is shared with earlier builds that ACK unknown commands.
        return Err(Error("UNSUPPORTED_CONFIGURATION"));
    }
    if !valid_frame(response) || response[2] != opcode || response.get(3) != Some(&0) {
        return Err(invalid());
    }
    Ok(())
}

fn key(response: &[u8], mode: u8, index: u8) -> Result<ReadbackKey> {
    // Only locally valid addresses are queried, with one outstanding exchange.
    // Firmware status 2/3 has no address and leaves this key unknown.
    if response == frame(0x87, &[2]) || response == frame(0x87, &[3]) {
        return Ok(ReadbackKey {
            role: ROLES[index as usize].into(),
            action: None,
            description: None,
            raw_binding: vec![],
            raw_description: None,
            read_status: Some(response[3]),
        });
    }
    structured(response, 0x87)?;
    if !(11..=64).contains(&response.len())
        || response[4..6] != [mode, index]
        || !matches!(response[6], 0 | 0x73 | 0x74)
    {
        return Err(invalid());
    }
    let action_len = response[7] as usize;
    let desc_offset = 8 + action_len;
    if action_len > 98 || desc_offset >= response.len() - 2 {
        return Err(invalid());
    }
    let desc_len = response[desc_offset] as usize;
    if desc_len > 20
        || response.len() != 11 + action_len + desc_len
        || (response[6] == 0 && action_len != 0)
    {
        return Err(invalid());
    }
    let mut raw_binding = vec![0; 100];
    raw_binding[..2].copy_from_slice(&response[6..8]);
    raw_binding[2..2 + action_len].copy_from_slice(&response[8..desc_offset]);
    let raw_description = response[desc_offset + 1..desc_offset + 1 + desc_len].to_vec();
    let description = raw_description
        .is_ascii()
        .then(|| String::from_utf8(raw_description.clone()).unwrap());
    Ok(ReadbackKey {
        role: ROLES[index as usize].into(),
        action: action(&raw_binding).ok(),
        description,
        raw_binding,
        raw_description: Some(raw_description),
        read_status: Some(0),
    })
}

async fn collect(io: &mut impl CommandTransport) -> Result<ConfigurationReadback> {
    let brightness = io.query(&frame(0x88, &[0])).await?;
    structured(&brightness, 0x88)?;
    if brightness.len() != 8 || brightness[4] != 0 || brightness[5] > 100 {
        return Err(invalid());
    }
    let mut modes = Vec::new();
    for mode in 0..4 {
        let mut keys = Vec::new();
        for index in 0..4 {
            keys.push(key(
                &io.query(&frame(0x87, &[mode, index])).await?,
                mode,
                index,
            )?);
        }
        let mut lights = Vec::new();
        for (state, name) in STATES.iter().enumerate() {
            let response = io.query(&frame(0x88, &[1, mode, state as u8])).await?;
            structured(&response, 0x88)?;
            if response.len() != 10 || response[4..7] != [1, mode, state as u8] {
                return Err(invalid());
            }
            lights.push(Light {
                state: (*name).into(),
                effect: EFFECTS
                    .get(response[7] as usize)
                    .ok_or_else(invalid)?
                    .to_string(),
            });
        }
        modes.push(ReadbackMode { mode, keys, lights });
    }
    Ok(ConfigurationReadback {
        device: ReadbackDevice {
            brightness_percent: brightness[5],
            task_light_mode: None,
        },
        modes,
    })
}

pub(super) async fn read_bounded(
    io: &mut impl CommandTransport,
    deadline: std::time::Duration,
) -> Result<Option<ConfigurationReadback>> {
    tokio::time::timeout(deadline, read(io))
        .await
        .unwrap_or(Err(Error("DEVICE_TIMEOUT")))
}

pub(super) async fn read(io: &mut impl CommandTransport) -> Result<Option<ConfigurationReadback>> {
    let identity = io.query(&frame(0x9f, &[])).await?;
    if !valid_frame(&identity) || identity[2] != 0x9f || identity.get(3) != Some(&0) {
        return Err(invalid());
    }
    if identity != frame(0x9f, &[&[0][..], super::oled::CATALOG].concat()) {
        return Ok(None);
    }
    let mut previous = collect(io).await?;
    for _ in 0..2 {
        let current = collect(io).await?;
        // Compare raw observations too: distinct unknown values must not collapse.
        if current == previous {
            return Ok(Some(current));
        }
        previous = current;
    }
    Err(Error("CONFIGURATION_CHANGED"))
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
