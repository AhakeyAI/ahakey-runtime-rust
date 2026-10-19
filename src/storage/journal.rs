use super::atomic::atomic_write;
use crate::{
    configuration::Configuration,
    error::{Error, Result},
    operation::{Mutation, Operation},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub request: Mutation,
    pub operation: Operation,
    /// Persisted before an OLED write can start; controls post-crash recovery checks.
    #[serde(default, skip_serializing_if = "is_false")]
    pub oled_transaction: bool,
}
fn is_false(value: &bool) -> bool {
    !*value
}
impl Record {
    pub fn needs_oled_recovery(&self) -> bool {
        self.oled_transaction
            && self.operation.status == "paused"
            && matches!(self.request, Mutation::Oled(_))
    }
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
                if record.operation.status == "paused" {
                    continue;
                }
                record.operation.status = if matches!(record.request, Mutation::Oled(_)) {
                    "paused"
                } else {
                    "failed"
                }
                .into();
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
