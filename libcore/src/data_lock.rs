//! Advisory interprocess lock on a data directory; the kernel releases it
//! when the process exits, so a crash never leaves the profile locked.
//!
//! Every process that writes a profile (the desktop app, the headless agent)
//! takes it before opening the database: two writers would each keep their
//! own in-memory ratchet state and overwrite the other's, which loses
//! messages and can reuse a chain key.
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

/// Another live process holds the lock. Anything else (a filesystem without
/// advisory locks, a permission problem) is reported but is not proof of a
/// second writer.
pub fn held_elsewhere(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_holder_is_refused_until_the_first_drops() {
        let dir = tempfile::tempdir().unwrap();
        let first = DataDirLock::acquire(dir.path()).unwrap();
        let err = DataDirLock::acquire(dir.path()).unwrap_err();
        assert!(held_elsewhere(&err), "{err}");
        drop(first);
        DataDirLock::acquire(dir.path()).unwrap();
    }
}
