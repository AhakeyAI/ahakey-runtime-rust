//! Common durable operation identity; legacy configuration records keep their JSON shape.
use crate::{configuration::Apply, error::Result, keymap::KeymapApply};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Mutation {
    Configuration(Apply),
    Keymap(KeymapApply),
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
        }
    }
    pub fn device_id(&self) -> &str {
        match self {
            Self::Configuration(r) => &r.device_id,
            Self::Keymap(r) => &r.device_id,
        }
    }
    pub fn base_token(&self) -> &str {
        match self {
            Self::Configuration(r) => &r.base_token,
            Self::Keymap(r) => &r.base_token,
        }
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Configuration(r) => r.validate(),
            Self::Keymap(r) => r.validate(),
        }
    }
    pub fn canonicalize(&mut self) {
        match self {
            Self::Configuration(r) => r.canonicalize(),
            Self::Keymap(r) => r.canonicalize(),
        }
    }
}
