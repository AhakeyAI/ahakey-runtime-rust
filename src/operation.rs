//! Common durable operation identity; legacy configuration records keep their JSON shape.
use crate::{configuration::Apply, configuration::keymap::KeymapApply, error::Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Mutation {
    Configuration(Apply),
    Keymap(KeymapApply),
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
impl Mutation {
    pub fn operation_id(&self) -> &str {
        match self {
            Self::Configuration(r) => &r.operation_id,
            Self::Keymap(r) => &r.operation_id,
            Self::Oled(r) => &r.operation_id,
        }
    }
    pub fn device_id(&self) -> &str {
        match self {
            Self::Configuration(r) => &r.device_id,
            Self::Keymap(r) => &r.device_id,
            Self::Oled(r) => &r.device_id,
        }
    }
    pub fn base_token(&self) -> &str {
        match self {
            Self::Configuration(r) => &r.base_token,
            Self::Keymap(r) => &r.base_token,
            Self::Oled(r) => &r.base_token,
        }
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Configuration(r) => r.validate(),
            Self::Keymap(r) => r.validate(),
            Self::Oled(r) => r.validate(),
        }
    }
    pub fn canonicalize(&mut self) {
        match self {
            Self::Configuration(r) => r.canonicalize(),
            Self::Keymap(r) => r.canonicalize(),
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
}
impl Operation {
    pub fn terminal(&self) -> bool {
        matches!(self.status.as_str(), "completed" | "failed" | "cancelled")
    }
}
