//! Field replacements that have independent X1 commands. Lighting is deliberately
//! absent: its current command rewrites all nine states in a mode.
use super::{Configuration, ROLES};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DescriptionPatch {
    pub mode: u8,
    pub index: u8,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationPatch {
    pub operation_id: String,
    pub device_id: String,
    pub base_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub descriptions: Vec<DescriptionPatch>,
}
impl ConfigurationPatch {
    pub fn validate(&self) -> Result<()> {
        if self.operation_id.is_empty()
            || self.operation_id.len() > 128
            || self.device_id.is_empty()
            || self.device_id.len() > 256
            || self.base_token.is_empty()
            || self.base_token.len() > 128
            || self
                .brightness_percent
                .is_some_and(|value| !(1..=100).contains(&value))
            || (self.brightness_percent.is_none() && self.descriptions.is_empty())
            || self.descriptions.len() > 16
        {
            return Err(Error("INVALID_PARAMS"));
        }
        let mut seen = BTreeSet::new();
        for field in &self.descriptions {
            if field.mode > 3
                || field.index > 3
                || !seen.insert((field.mode, field.index))
                || !field.description.is_ascii()
                || field.description.len() > 20
            {
                return Err(Error("INVALID_PARAMS"));
            }
        }
        Ok(())
    }
    pub fn canonicalize(&mut self) {
        self.descriptions
            .sort_by_key(|field| (field.mode, field.index));
    }
    /// Merge ACK-confirmed values only into an already known complete baseline.
    /// A patch never constructs missing actions, lights, modes, or brightness.
    pub fn merge_known(&self, configuration: &mut Configuration) {
        if let Some(brightness) = self.brightness_percent {
            configuration.device.brightness_percent = brightness;
        }
        for field in &self.descriptions {
            if let Some(key) = configuration
                .modes
                .iter_mut()
                .find(|mode| mode.mode == field.mode)
                .and_then(|mode| {
                    mode.keys
                        .iter_mut()
                        .find(|key| key.role == ROLES[field.index as usize])
                })
            {
                key.description = field.description.clone();
            }
        }
    }
}
