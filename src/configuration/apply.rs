use super::{Configuration, model::unique};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub device_fields: Vec<String>,
    pub modes: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Apply {
    pub operation_id: String,
    pub device_id: String,
    pub base_token: String,
    pub scope: Scope,
    pub changes: Configuration,
}

impl Apply {
    pub fn validate(&self) -> Result<()> {
        if self.operation_id.is_empty()
            || self.operation_id.len() > 128
            || self.device_id.is_empty()
            || self.device_id.len() > 256
            || self.base_token.is_empty()
            || self.base_token.len() > 128
            || self.scope.device_fields != ["brightnessPercent"]
            || !unique(self.scope.modes.iter())
        {
            return Err(Error("INVALID_PARAMS"));
        }
        self.changes.validate()?;
        let modes: BTreeSet<_> = self.changes.modes.iter().map(|m| m.mode).collect();
        if modes != self.scope.modes.iter().copied().collect() {
            return Err(Error("INVALID_PARAMS"));
        }
        Ok(())
    }
    pub fn canonicalize(&mut self) {
        self.scope.modes.sort();
        self.changes.canonicalize();
    }
}
