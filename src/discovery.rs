use crate::{
    error::{Error, Result},
    storage::{atomic_write, private_options},
};
use fs2::FileExt;
use serde_json::json;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

pub fn default_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("AHAKEY_RUNTIME_DISCOVERY") {
        return Ok(path.into());
    }
    #[cfg(target_os = "macos")]
    let directory = PathBuf::from(std::env::var_os("HOME").ok_or(Error("HOME_UNAVAILABLE"))?)
        .join("Library/Application Support/AhaKey/runtime");
    #[cfg(target_os = "windows")]
    let directory =
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or(Error("HOME_UNAVAILABLE"))?)
            .join("AhaKey/runtime");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let directory = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or(
            PathBuf::from(std::env::var_os("HOME").ok_or(Error("HOME_UNAVAILABLE"))?)
                .join(".local/state"),
        )
        .join("ahakey");
    Ok(directory.join("discovery.json"))
}

pub struct Discovery {
    path: PathBuf,
    _lock: File,
    published: bool,
}
impl Discovery {
    pub fn acquire(path: PathBuf) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Error("DISCOVERY_PATH_MUST_BE_ABSOLUTE"));
        }
        let parent = path.parent().ok_or(Error("INVALID_DISCOVERY_PATH"))?;
        if !parent.exists() {
            fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::symlink_metadata(parent)?;
            if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
                return Err(Error("DISCOVERY_DIRECTORY_MUST_BE_PRIVATE"));
            }
        }
        let lock_path = path.with_extension("lock");
        if fs::symlink_metadata(&lock_path).is_ok_and(|m| !m.is_file()) {
            return Err(Error("INVALID_LOCK_FILE"));
        }
        let lock = private_options()
            .read(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock.try_lock_exclusive()
            .map_err(|_| Error("RUNTIME_ALREADY_RUNNING"))?;
        Ok(Self {
            path,
            _lock: lock,
            published: false,
        })
    }
    pub fn publish(&mut self, endpoint: &str, token: &str) -> Result<()> {
        atomic_write(
            &self.path,
            &serde_json::to_vec(&json!({"endpoint": endpoint, "token": token})).unwrap(),
        )?;
        self.published = true;
        Ok(())
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn unpublish(&mut self) {
        if self.published {
            let _ = fs::remove_file(&self.path);
            self.published = false;
        }
    }
}
impl Drop for Discovery {
    fn drop(&mut self) {
        if self.published {
            let _ = fs::remove_file(&self.path);
        }
    }
}
