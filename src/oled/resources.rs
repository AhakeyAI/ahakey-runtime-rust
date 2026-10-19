use super::{
    FRAME_BYTES, MAX_SOURCE_BYTES, Prepared, Resource, codec, integrity::valid_id, sha256,
};
use crate::error::{Error, Result};
use std::path::PathBuf;
const MAX_STORE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct Resources {
    directory: PathBuf,
    writing: std::sync::Arc<tokio::sync::Mutex<()>>,
}
impl Resources {
    pub fn new(directory: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            directory,
            writing: Default::default(),
        })
    }
    pub async fn finish(&self, source: Vec<u8>, expected_sha256: String) -> Result<Resource> {
        if source.is_empty() || source.len() > MAX_SOURCE_BYTES {
            return Err(Error("INVALID_PARAMS"));
        }
        let _writing = self.writing.lock().await;
        let directory = self.directory.clone();
        tokio::task::spawn_blocking(move || {
            if sha256(&source) != expected_sha256 {
                return Err(Error("RESOURCE_HASH_MISMATCH"));
            }
            let asset = codec::decode(&source)?;
            let path = directory.join(format!("{}.rgb565", asset.info.resource_id));
            if !path.exists() {
                let total = std::fs::read_dir(&directory)?.try_fold(
                    0u64,
                    |sum, entry| -> std::io::Result<_> {
                        Ok(sum.saturating_add(entry?.metadata()?.len()))
                    },
                )?;
                if total + asset.bytes.len() as u64 > MAX_STORE_BYTES {
                    return Err(Error("RESOURCE_STORE_FULL"));
                }
                crate::storage::atomic_write(&path, &asset.bytes)?;
            }
            Ok(asset.info)
        })
        .await
        .map_err(|_| Error("RESOURCE_ERROR"))?
    }
    pub async fn load(&self, id: &str) -> Result<Prepared> {
        if !valid_id(id) {
            return Err(Error("INVALID_PARAMS"));
        }
        let path = self.directory.join(format!("{id}.rgb565"));
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            let metadata = std::fs::metadata(&path).map_err(|_| Error("RESOURCE_NOT_FOUND"))?;
            if metadata.len() > (12 * FRAME_BYTES) as u64 {
                return Err(Error("RESOURCE_CORRUPT"));
            }
            let asset = codec::prepared(std::fs::read(path)?)?;
            if asset.info.resource_id != id {
                return Err(Error("RESOURCE_CORRUPT"));
            }
            Ok(asset)
        })
        .await
        .map_err(|_| Error("RESOURCE_ERROR"))?
    }
}
