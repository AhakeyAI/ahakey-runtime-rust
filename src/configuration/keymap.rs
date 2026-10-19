//! Explicit key replacements; no complete configuration or lighting support required.
use crate::{
    configuration::Action,
    error::{Error, Result},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyBinding {
    /// Firmware key position, 0..3. Independent of the current binding's HID usage.
    pub index: u8,
    pub action: Action,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeymapApply {
    pub operation_id: String,
    pub device_id: String,
    /// Obtained from configuration.get even when configuration is null.
    pub base_token: String,
    /// Firmware mode 0..3 (Studio labels these 1..4).
    pub mode: u8,
    pub keys: Vec<KeyBinding>,
}
impl KeymapApply {
    pub fn validate(&self) -> Result<()> {
        if self.operation_id.is_empty()
            || self.operation_id.len() > 128
            || self.device_id.is_empty()
            || self.device_id.len() > 256
            || self.base_token.is_empty()
            || self.base_token.len() > 128
            || self.mode > 3
            || self.keys.is_empty()
            || self.keys.len() > 4
        {
            return Err(Error("INVALID_PARAMS"));
        }
        let mut seen = BTreeSet::new();
        for key in &self.keys {
            if key.index > 3 || !seen.insert(key.index) {
                return Err(Error("INVALID_PARAMS"));
            }
            key.action.validate()?;
        }
        Ok(())
    }
    pub fn canonicalize(&mut self) {
        self.keys.sort_by_key(|k| k.index);
        for key in &mut self.keys {
            if let Action::Shortcut { modifiers, .. } = &mut key.action {
                modifiers.sort();
            }
        }
    }
}
