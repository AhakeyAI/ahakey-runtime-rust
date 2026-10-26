//! Common durable operation identity; legacy configuration records keep their JSON shape.
use crate::{configuration::Apply, configuration::keymap::KeymapApply, error::Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Mutation {
    Configuration(Apply),
    Keymap(KeymapApply),
    Patch(crate::configuration::patch::ConfigurationPatch),
    Oled(crate::oled::Upload),
}
impl From<Apply> for Mutation {
    fn from(value: Apply) -> Self {
        Self::Configuration(value)
    }
}
impl From<KeymapApply> for Mutation {
    fn from(value: KeymapApply) -> Self {
        Self::Keymap(value)
    }
}
impl From<crate::configuration::patch::ConfigurationPatch> for Mutation {
    fn from(value: crate::configuration::patch::ConfigurationPatch) -> Self {
        Self::Patch(value)
    }
}
impl Mutation {
    pub fn operation_id(&self) -> &str {
        match self {
            Self::Configuration(r) => &r.operation_id,
            Self::Keymap(r) => &r.operation_id,
            Self::Patch(r) => &r.operation_id,
            Self::Oled(r) => &r.operation_id,
        }
    }
    pub fn device_id(&self) -> &str {
        match self {
            Self::Configuration(r) => &r.device_id,
            Self::Keymap(r) => &r.device_id,
            Self::Patch(r) => &r.device_id,
            Self::Oled(r) => &r.device_id,
        }
    }
    pub fn base_token(&self) -> &str {
        match self {
            Self::Configuration(r) => &r.base_token,
            Self::Keymap(r) => &r.base_token,
            Self::Patch(r) => &r.base_token,
            Self::Oled(r) => &r.base_token,
        }
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Configuration(r) => r.validate(),
            Self::Keymap(r) => r.validate(),
            Self::Patch(r) => r.validate(),
            Self::Oled(r) => r.validate(),
        }
    }
    pub fn canonicalize(&mut self) {
        match self {
            Self::Configuration(r) => r.canonicalize(),
            Self::Keymap(r) => r.canonicalize(),
            Self::Patch(r) => r.canonicalize(),
            Self::Oled(_) => {}
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub operation_id: String,
    pub device_id: String,
    pub status: String,
    pub progress: f64,
    pub effect: String,
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_progress: Option<ConfigurationProgress>,
}
impl Operation {
    pub fn terminal(&self) -> bool {
        matches!(self.status.as_str(), "completed" | "failed" | "cancelled")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationProgress {
    pub fields_total: usize,
    pub fields_acknowledged: usize,
    pub fields_verified: usize,
    pub phase: String,
    /// Save command ACK, not proof of Flash durability across power loss.
    pub save_acknowledged: bool,
}
