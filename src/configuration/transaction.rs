//! Logical fields and device-specific command boundaries for resumable writes.
use super::{ConfigurationReadback, ROLES};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub enum Field {
    Brightness,
    Action(u8, u8),
    Description(u8, u8),
    Lights(u8),
}
#[derive(Clone, Debug)]
pub struct Group {
    pub field: Field,
    pub desired: Value,
    pub commands: Vec<Vec<u8>>,
}
#[derive(Clone, Debug)]
pub struct Plan {
    pub groups: Vec<Group>,
    pub save: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub before: Vec<Option<Value>>,
    pub acknowledged: Vec<bool>,
    pub in_flight: Option<usize>,
    pub save_acknowledged: bool,
    pub readback_required: bool,
}
pub fn lights_value(lights: &[super::Light]) -> Value {
    json!(
        lights
            .iter()
            .map(|l| (l.state.clone(), l.effect.clone()))
            .collect::<std::collections::BTreeMap<_, _>>()
    )
}
impl Field {
    pub fn observe(&self, readback: &ConfigurationReadback) -> Option<Value> {
        if matches!(self, Self::Brightness) {
            return Some(json!(readback.device.brightness_percent));
        }
        let mode = match self {
            Self::Action(m, _) | Self::Description(m, _) | Self::Lights(m) => *m,
            _ => unreachable!(),
        };
        let mode = readback.modes.iter().find(|m| m.mode == mode)?;
        match self {
            Self::Action(_, index) => {
                let mut action = mode
                    .keys
                    .iter()
                    .find(|k| k.role == ROLES[*index as usize])?
                    .action
                    .clone()?;
                if let super::Action::Shortcut { modifiers, .. } = &mut action {
                    modifiers.sort();
                }
                Some(json!(action))
            }
            Self::Description(_, index) => mode
                .keys
                .iter()
                .find(|k| k.role == ROLES[*index as usize])?
                .description
                .as_ref()
                .map(|d| json!(d)),
            Self::Lights(_) if mode.lights.len() == super::STATES.len() => {
                Some(lights_value(&mode.lights))
            }
            _ => None,
        }
    }
}
