use crate::{
    configuration::{Action, Configuration, EFFECTS, MODIFIERS, MacroStep, ROLES, STATES},
    error::Result,
};

pub fn frame(command: u8, payload: &[u8]) -> Vec<u8> {
    let mut data = vec![0xaa, 0xbb, command];
    data.extend_from_slice(payload);
    data.extend_from_slice(&[0xcc, 0xdd]);
    data
}
pub fn valid_frame(data: &[u8]) -> bool {
    data.len() >= 5 && data.starts_with(&[0xaa, 0xbb]) && data.ends_with(&[0xcc, 0xdd])
}
fn key_frame(subtype: u8, mode: u8, index: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![subtype, mode, index];
    bytes.extend_from_slice(payload);
    frame(0x73, &bytes)
}

/// Complete write groups, including clearing the mutually exclusive action layer.
pub fn configuration_commands(config: &Configuration) -> Result<Vec<Vec<u8>>> {
    config.validate()?;
    let mut commands = vec![frame(0x85, &[config.device.brightness_percent])];
    for mode in &config.modes {
        for key in &mode.keys {
            let index = ROLES.iter().position(|r| *r == key.role).unwrap() as u8;
            commands.extend(action_commands(mode.mode, index, &key.action));
            commands.push(key_frame(
                0x75,
                mode.mode,
                index,
                key.description.as_bytes(),
            ));
        }
        let mut effects = vec![mode.mode];
        for state in STATES {
            let light = mode.lights.iter().find(|l| l.state == state).unwrap();
            effects.push(EFFECTS.iter().position(|e| *e == light.effect).unwrap() as u8);
        }
        commands.push(frame(0x84, &effects));
    }
    commands.push(frame(0x04, &[]));
    Ok(commands)
}

fn action_commands(mode: u8, index: u8, action: &Action) -> Vec<Vec<u8>> {
    let mut commands = Vec::new();
    let (subtype, payload) = match action {
        Action::Disabled {} => (0x73, vec![]),
        Action::Shortcut {
            modifiers,
            key_code,
        } => {
            let mut bytes: Vec<_> = MODIFIERS
                .iter()
                .enumerate()
                .filter(|(_, m)| modifiers.iter().any(|s| s == *m))
                .map(|(i, _)| 0xe0 + i as u8)
                .collect();
            if *key_code != 0 {
                bytes.push(*key_code);
            }
            (0x73, bytes)
        }
        Action::Macro { steps } => (
            0x74,
            steps
                .iter()
                .flat_map(|step| match step {
                    MacroStep::NoOp {} => [0, 0],
                    MacroStep::KeyDown { key_code } => [1, *key_code],
                    MacroStep::KeyUp { key_code } => [2, *key_code],
                    MacroStep::Delay { delay_ms } => [3, (delay_ms / 3) as u8],
                    MacroStep::ReleaseAll {} => [4, 0],
                })
                .collect(),
        ),
    };
    commands.push(key_frame(
        if subtype == 0x73 { 0x74 } else { 0x73 },
        mode,
        index,
        &[],
    ));
    commands.push(key_frame(subtype, mode, index, &payload));
    commands
}

pub fn keymap_commands(
    request: &crate::configuration::keymap::KeymapApply,
) -> Result<Vec<Vec<u8>>> {
    request.validate()?;
    let mut commands = Vec::new();
    for key in &request.keys {
        commands.extend(action_commands(request.mode, key.index, &key.action));
    }
    commands.push(frame(0x04, &[]));
    Ok(commands)
}

pub fn patch_commands(
    request: &crate::configuration::patch::ConfigurationPatch,
) -> Result<Vec<Vec<u8>>> {
    request.validate()?;
    let mut commands = Vec::new();
    if let Some(brightness) = request.brightness_percent {
        commands.push(frame(0x85, &[brightness]));
    }
    for field in &request.descriptions {
        commands.push(key_frame(
            0x75,
            field.mode,
            field.index,
            field.description.as_bytes(),
        ));
    }
    commands.push(frame(0x04, &[]));
    Ok(commands)
}

/// A key action's clear/set frames are indivisible with respect to user pause.
pub fn transaction_plan(
    request: &crate::operation::Mutation,
) -> Result<crate::configuration::transaction::Plan> {
    use crate::{
        configuration::transaction::{Field, Group, Plan, lights_value},
        operation::Mutation,
    };
    use serde_json::json;
    request.validate()?;
    let mut groups = Vec::new();
    let mut brightness = |value| {
        groups.push(Group {
            field: Field::Brightness,
            desired: json!(value),
            commands: vec![frame(0x85, &[value])],
        })
    };
    match request {
        Mutation::Patch(patch) => {
            if let Some(value) = patch.brightness_percent {
                brightness(value);
            }
            for field in &patch.descriptions {
                groups.push(Group {
                    field: Field::Description(field.mode, field.index),
                    desired: json!(field.description),
                    commands: vec![key_frame(
                        0x75,
                        field.mode,
                        field.index,
                        field.description.as_bytes(),
                    )],
                });
            }
        }
        Mutation::Keymap(keys) => {
            for key in &keys.keys {
                groups.push(Group {
                    field: Field::Action(keys.mode, key.index),
                    desired: json!(key.action),
                    commands: action_commands(keys.mode, key.index, &key.action),
                });
            }
        }
        Mutation::Configuration(config) => {
            brightness(config.changes.device.brightness_percent);
            for mode in &config.changes.modes {
                for key in &mode.keys {
                    let index = ROLES.iter().position(|r| *r == key.role).unwrap() as u8;
                    groups.push(Group {
                        field: Field::Action(mode.mode, index),
                        desired: json!(key.action),
                        commands: action_commands(mode.mode, index, &key.action),
                    });
                    groups.push(Group {
                        field: Field::Description(mode.mode, index),
                        desired: json!(key.description),
                        commands: vec![key_frame(
                            0x75,
                            mode.mode,
                            index,
                            key.description.as_bytes(),
                        )],
                    });
                }
                let mut effects = vec![mode.mode];
                for state in STATES {
                    effects.push(
                        EFFECTS
                            .iter()
                            .position(|e| {
                                *e == mode
                                    .lights
                                    .iter()
                                    .find(|l| l.state == state)
                                    .unwrap()
                                    .effect
                            })
                            .unwrap() as u8,
                    );
                }
                groups.push(Group {
                    field: Field::Lights(mode.mode),
                    desired: lights_value(&mode.lights),
                    commands: vec![frame(0x84, &effects)],
                });
            }
        }
        Mutation::Oled(_) => return Err(crate::error::Error("UNSUPPORTED_CAPABILITY")),
    }
    Ok(Plan {
        groups,
        save: frame(0x04, &[]),
    })
}
