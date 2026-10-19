use super::integrity::valid_id;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

pub const WIDTH: u32 = 160;
pub const HEIGHT: u32 = 80;
pub const FRAME_BYTES: usize = 28_672;
pub const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScreenState {
    Working,
    Waiting,
    Done,
}
impl ScreenState {
    fn index(&self) -> u8 {
        match self {
            Self::Working => 1,
            Self::Waiting => 2,
            Self::Done => 3,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Target {
    ModeDefault { mode: u8 },
    ModeState { mode: u8, state: ScreenState },
    MultiState { state: ScreenState },
}
impl Target {
    pub fn entry(&self) -> Result<u8> {
        match self {
            Self::ModeDefault { mode } if *mode < 4 => Ok(mode * 4),
            Self::ModeState { mode, state } if *mode < 4 => Ok(mode * 4 + state.index()),
            Self::MultiState { state } => Ok(15 + state.index()),
            _ => Err(Error("INVALID_PARAMS")),
        }
    }
    pub fn max_frames(&self) -> u16 {
        if matches!(self, Self::ModeDefault { .. }) {
            8
        } else {
            12
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Upload {
    pub operation_id: String,
    pub device_id: String,
    pub base_token: String,
    pub resource_id: String,
    pub target: Target,
}
impl Upload {
    pub fn validate(&self) -> Result<()> {
        self.target.entry()?;
        if self.operation_id.is_empty()
            || self.operation_id.len() > 128
            || self.device_id.is_empty()
            || self.device_id.len() > 256
            || self.base_token.is_empty()
            || self.base_token.len() > 128
            || !valid_id(&self.resource_id)
        {
            return Err(Error("INVALID_PARAMS"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub resource_id: String,
    pub width: u32,
    pub height: u32,
    pub frame_count: u16,
    pub interval_ms: u16,
    pub byte_length: usize,
    pub crc32: u32,
}
pub struct Prepared {
    pub info: Resource,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub target: Target,
    pub valid: bool,
    pub write_state: String,
    pub frame_count: u16,
    pub byte_length: u32,
    pub crc32: u32,
    pub max_frames: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub profile: String,
    pub width: u32,
    pub height: u32,
    pub interval_ms: u16,
    pub entries: Vec<Entry>,
}
