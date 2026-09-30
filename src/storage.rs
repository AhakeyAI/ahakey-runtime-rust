use crate::{
    configuration::Configuration,
    error::{Error, Result},
    mutation::Mutation,
    wire::Operation,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub request: Mutation,
    pub operation: Operation,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Journal {
    pub version: u8,
    pub operations: BTreeMap<String, Record>,
    // Last ACK-confirmed configurations for audit. A new BLE session cannot trust these.
    pub configurations: BTreeMap<String, Configuration>,
}
impl Default for Journal {
    fn default() -> Self {
        Self {
            version: 1,
            operations: BTreeMap::new(),
            configurations: BTreeMap::new(),
        }
    }
}
#[derive(Clone)]
pub struct Storage {
    path: PathBuf,
}
impl Storage {
    pub async fn open(path: PathBuf) -> Result<(Self, Journal)> {
        let read_path = path.clone();
        let mut journal: Journal = tokio::task::spawn_blocking(move || -> Result<Journal> {
            match fs::read(read_path) {
                Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| Error("STORAGE_CORRUPT")),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
                Err(e) => Err(e.into()),
            }
        })
        .await
        .map_err(|_| Error("STORAGE_ERROR"))??;
        if journal.version != 1 {
            return Err(Error("STORAGE_VERSION_UNSUPPORTED"));
        }
        for record in journal.operations.values_mut() {
            if !record.operation.terminal() {
                record.operation.status = "failed".into();
                record.operation.effect = "unknown".into();
                record.operation.error_code = Some("RUNTIME_RESTARTED".into());
            }
        }
        let store = Self { path };
        store.save(&journal).await?;
        Ok((store, journal))
    }
    pub async fn save(&self, journal: &Journal) -> Result<()> {
        let path = self.path.clone();
        let snapshot = journal.clone();
        tokio::task::spawn_blocking(move || {
            let bytes = serde_json::to_vec(&snapshot).map_err(|_| Error("STORAGE_ERROR"))?;
            atomic_write(&path, &bytes)
        })
        .await
        .map_err(|_| Error("STORAGE_ERROR"))?
    }
}

pub fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
/// Same-directory replacement with file and directory fsync before reporting success.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or(Error("STORAGE_ERROR"))?;
    let temporary = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = private_options().create_new(true).open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
