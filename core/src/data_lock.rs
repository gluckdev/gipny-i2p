//! Advisory interprocess lock; the kernel releases it when the process exits.
use std::{fs::{File, OpenOptions}, io, path::Path};

#[derive(Debug)]
pub struct DataDirLock {
    _file: File,
}

impl DataDirLock {
    pub fn acquire(dir: &Path) -> io::Result<Self> {
        let acquire = || -> io::Result<Self> {
            std::fs::create_dir_all(dir)?;
            let file = OpenOptions::new().read(true).write(true).create(true)
                .truncate(false).open(dir.join("instance.lock"))?;
            file.try_lock().map_err(io::Error::from)?;
            // Clear a legacy sentinel only after acquiring the kernel lock.
            file.set_len(0)?;
            Ok(Self { _file: file })
        };
        acquire().map_err(|error| io::Error::new(error.kind(), format!(
            "Cannot lock data directory {}: {error}. Another gipny process may be using it.", dir.display())))
    }
}
