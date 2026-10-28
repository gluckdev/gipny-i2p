//! Private Windows bootstrap readiness, not a public CLI or network signal.
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const PATH_ENV: &str = "GIPNY_AGENT_READY_PATH";
const NONCE_ENV: &str = "GIPNY_AGENT_READY_NONCE";

pub(crate) struct Readiness {
    path: PathBuf,
    nonce: String,
}

impl Readiness {
    /// Call from synchronous main before constructing Tokio or other threads.
    /// The ticket must not reach shell commands later executed by the agent.
    pub(crate) fn take_environment() -> Result<Option<Self>> {
        let path = std::env::var_os(PATH_ENV);
        let nonce = std::env::var_os(NONCE_ENV);
        std::env::remove_var(PATH_ENV);
        std::env::remove_var(NONCE_ENV);
        Self::from_values(path, nonce)
    }

    fn from_values(path: Option<OsString>, nonce: Option<OsString>) -> Result<Option<Self>> {
        let (path, nonce) = match (path, nonce) {
            (None, None) => return Ok(None),
            (Some(path), Some(nonce)) => (PathBuf::from(path), nonce.into_string().map_err(|_| anyhow::anyhow!("invalid bootstrap readiness ticket"))?),
            _ => bail!("incomplete bootstrap readiness ticket"),
        };
        if nonce.len() != 64 || !nonce.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !path.is_absolute()
            || path.file_name() != Some(OsStr::new(&format!(".gipny-agent-start-{nonce}.ready"))) {
            bail!("invalid bootstrap readiness ticket");
        }
        Ok(Some(Self { path, nonce }))
    }

    /// Called only after provider, SQLCipher, session, relay and command worker
    /// initialization succeeded. Atomic publication is within the data volume.
    pub(crate) fn signal(self, data_dir: &Path) -> Result<()> {
        self.publish(data_dir, std::process::id())
    }

    fn publish(self, data_dir: &Path, pid: u32) -> Result<()> {
        let parent = self.path.parent().context("invalid readiness directory")?;
        if parent.canonicalize().context("resolve readiness directory")?
            != data_dir.canonicalize().context("resolve agent data directory")? {
            bail!("bootstrap readiness path is outside the agent data directory");
        }
        if self.path.symlink_metadata().is_ok() { bail!("bootstrap readiness marker already exists"); }
        let temporary = self.path.with_extension("tmp");
        // create_new rejects an existing file/reparse link before ownership is
        // acquired. Cleanup must never remove a file we failed to create.
        let mut output = OpenOptions::new().create_new(true).write(true).open(&temporary)
            .context("create bootstrap readiness marker")?;
        struct OwnTemporary(PathBuf);
        impl Drop for OwnTemporary { fn drop(&mut self) { let _ = fs::remove_file(&self.0); } }
        let _cleanup = OwnTemporary(temporary.clone());
        write!(output, "gipny-agent-ready-v1\n{}\n{pid}\n", self.nonce).context("write bootstrap readiness marker")?;
        output.sync_all().context("sync bootstrap readiness marker")?;
        drop(output);
        fs::rename(&temporary, &self.path).context("publish bootstrap readiness marker")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!("gipny-ready-test-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&path).unwrap(); path
    }

    #[test]
    fn marker_is_atomic_bound_to_data_directory_nonce_and_pid() {
        let data = directory();
        let nonce = "a".repeat(64);
        let path = data.join(format!(".gipny-agent-start-{nonce}.ready"));
        let readiness = Readiness::from_values(Some(path.clone().into_os_string()), Some(nonce.clone().into())).unwrap().unwrap();
        assert!(!path.exists());
        readiness.publish(&data, 123).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("gipny-agent-ready-v1\n{nonce}\n123\n"));
        assert!(!path.with_extension("tmp").exists());
        // An existing marker is never silently adopted or overwritten.
        assert!(Readiness::from_values(Some(path.clone().into_os_string()), Some(nonce.into())).unwrap().unwrap().publish(&data, 456).is_err());
        assert!(fs::read_to_string(&path).unwrap().ends_with("123\n"));
        fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn invalid_or_external_ticket_preserves_existing_files() {
        assert!(Readiness::from_values(None, None).unwrap().is_none());
        assert!(Readiness::from_values(None, Some("a".repeat(64).into())).is_err());
        assert!(Readiness::from_values(Some("relative.ready".into()), Some("a".repeat(64).into())).is_err());
        let data = directory(); let outside = directory();
        let nonce = "b".repeat(64);
        let path = outside.join(format!(".gipny-agent-start-{nonce}.ready"));
        let ticket = || Readiness::from_values(Some(path.clone().into_os_string()), Some(nonce.clone().into())).unwrap().unwrap();
        assert!(ticket().publish(&data, 1).is_err()); assert!(!path.exists());
        let temporary = path.with_extension("tmp"); fs::write(&temporary, b"keep").unwrap();
        assert!(ticket().publish(&outside, 1).is_err());
        assert_eq!(fs::read(&temporary).unwrap(), b"keep"); assert!(!path.exists());
        fs::remove_dir_all(data).unwrap(); fs::remove_dir_all(outside).unwrap();
    }
}
