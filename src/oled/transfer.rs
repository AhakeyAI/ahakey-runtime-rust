use super::{MAX_CHUNK_BYTES, MAX_SOURCE_BYTES, integrity::valid_id};
use crate::error::{Error, Result};
use serde::Deserialize;
use uuid::Uuid;

/// One bounded, connection-owned transfer. No hardware changes until oled.upload.
pub struct Transfer {
    pub id: Uuid,
    pub total: usize,
    pub hash: String,
    bytes: Vec<u8>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Begin {
    pub byte_length: usize,
    pub sha256: String,
}
impl Transfer {
    pub fn begin(params: Begin) -> Result<Self> {
        if params.byte_length == 0
            || params.byte_length > MAX_SOURCE_BYTES
            || !valid_id(&params.sha256)
        {
            return Err(Error("INVALID_PARAMS"));
        }
        Ok(Self {
            id: Uuid::new_v4(),
            total: params.byte_length,
            hash: params.sha256,
            bytes: Vec::new(),
        })
    }
    pub fn append(&mut self, message: &[u8]) -> Result<usize> {
        if message.len() <= 20
            || message.len() > 20 + MAX_CHUNK_BYTES
            || message[..16] != self.id.as_bytes()[..]
        {
            return Err(Error("INVALID_RESOURCE_CHUNK"));
        }
        let offset = u32::from_le_bytes(message[16..20].try_into().unwrap()) as usize;
        let payload = &message[20..];
        if offset != self.bytes.len() || payload.len() > self.total - self.bytes.len() {
            return Err(Error("INVALID_RESOURCE_CHUNK"));
        }
        self.bytes.extend_from_slice(payload);
        Ok(self.bytes.len())
    }
    pub fn complete(self, id: Uuid) -> Result<(Vec<u8>, String)> {
        if id != self.id || self.bytes.len() != self.total {
            return Err(Error("RESOURCE_INCOMPLETE"));
        }
        Ok((self.bytes, self.hash))
    }
}
