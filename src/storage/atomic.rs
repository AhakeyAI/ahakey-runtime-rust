use crate::error::{Error, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};
use uuid::Uuid;

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
