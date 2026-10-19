use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const ROLES: [&str; 4] = ["voice", "approve", "reject", "submit"];
pub const STATES: [&str; 9] = [
    "notification",
    "permissionRequest",
    "postToolUse",
    "preToolUse",
    "sessionStart",
    "stop",
    "taskCompleted",
    "userPromptSubmit",
    "sessionEnd",
];
// Firmware order, intentionally different from the frontend's menu order.
pub const EFFECTS: [&str; 17] = [
    "off",
    "singleMove",
    "rainbowMove",
    "rainbowWave",
    "rainbowWaveSlow",
    "breathing",
    "middleLight",
    "typingRipple",
    "comet",
    "scanBar",
    "pulseCenter",
    "warningBlink",
    "successSweep",
    "blueThinking",
    "lowBattery",
    "chargingFlow",
    "approvalWait",
];
pub const MODIFIERS: [&str; 4] = ["control", "shift", "alt", "gui"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Configuration {
    pub device: DeviceFields,
    pub modes: Vec<Mode>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceFields {
    pub brightness_percent: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Mode {
    pub mode: u8,
    pub keys: Vec<Key>,
    pub lights: Vec<Light>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Key {
    pub role: String,
    pub action: Action,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Light {
    pub state: String,
    pub effect: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    Disabled {},
    Shortcut {
        modifiers: Vec<String>,
        key_code: u8,
    },
    Macro {
        steps: Vec<MacroStep>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum MacroStep {
    KeyDown { key_code: u8 },
    KeyUp { key_code: u8 },
    Delay { delay_ms: u16 },
    ReleaseAll {},
    NoOp {},
}

pub(super) fn unique<T: Ord>(values: impl Iterator<Item = T>) -> bool {
    let mut seen = BTreeSet::new();
    values.into_iter().all(|v| seen.insert(v))
}

impl Configuration {
    pub fn validate(&self) -> Result<()> {
        let invalid = || Error("INVALID_PARAMS");
        if !(1..=100).contains(&self.device.brightness_percent)
            || self.modes.is_empty()
            || self.modes.len() > 4
            || !unique(self.modes.iter().map(|m| m.mode))
        {
            return Err(invalid());
        }
        for mode in &self.modes {
            if mode.mode > 3 {
                return Err(invalid());
            }
            if mode.keys.len() != 4 || mode.lights.len() != 9 {
                return Err(Error("INCOMPLETE_WRITE_GROUP"));
            }
            if !unique(mode.keys.iter().map(|k| &k.role))
                || !unique(mode.lights.iter().map(|l| &l.state))
            {
                return Err(invalid());
            }
            for key in &mode.keys {
                if !ROLES.contains(&key.role.as_str())
                    || !key.description.is_ascii()
                    || key.description.len() > 20
                {
                    return Err(invalid());
                }
                key.action.validate()?;
            }
            for light in &mode.lights {
                if !STATES.contains(&light.state.as_str())
                    || !EFFECTS.contains(&light.effect.as_str())
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
    pub fn canonicalize(&mut self) {
        self.modes.sort_by_key(|m| m.mode);
        for mode in &mut self.modes {
            mode.keys.sort_by(|a, b| a.role.cmp(&b.role));
            mode.lights.sort_by(|a, b| a.state.cmp(&b.state));
            for key in &mut mode.keys {
                if let Action::Shortcut { modifiers, .. } = &mut key.action {
                    modifiers.sort();
                }
            }
        }
    }
    pub fn merge(&mut self, changes: &Self) {
        self.device = changes.device.clone();
        for mode in &changes.modes {
            self.modes.retain(|m| m.mode != mode.mode);
            self.modes.push(mode.clone());
        }
        self.canonicalize();
    }
    pub fn mock_defaults() -> Self {
        let document: crate::wire::ConfigurationDocument = serde_json::from_str(include_str!(
            "../../contracts/runtime-v1/fixtures/configuration.json"
        ))
        .expect("checked Studio fixture");
        document.configuration.expect("fixture configuration")
    }
}

impl Action {
    pub fn validate(&self) -> Result<()> {
        let invalid = || Error("INVALID_PARAMS");
        match self {
            Action::Disabled {} => {}
            Action::Shortcut { modifiers, .. } => {
                if !unique(modifiers.iter())
                    || modifiers.iter().any(|m| !MODIFIERS.contains(&m.as_str()))
                {
                    return Err(invalid());
                }
            }
            Action::Macro { steps } => {
                if steps.is_empty() || steps.len() > 49 {
                    return Err(invalid());
                }
                for step in steps {
                    if let MacroStep::Delay { delay_ms } = step
                        && (*delay_ms > 765 || delay_ms % 3 != 0)
                    {
                        return Err(invalid());
                    }
                }
            }
        }
        Ok(())
    }
}
