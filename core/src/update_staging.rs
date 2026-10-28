//! A verified APK must remain readable at the FileProvider URI after the
//! install intent returns. Each attempt owns a separate private cache file.
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Cancel only an installer callback that has not started. Once JNI is running,
/// a timeout cannot safely revoke the content URI it may already have handed off.
#[derive(Default)]
pub(crate) struct InstallerDispatch(std::sync::atomic::AtomicU8);

impl InstallerDispatch {
    pub(crate) fn try_start(&self) -> bool {
        self.0.compare_exchange(0, 1, std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Acquire).is_ok()
    }

    pub(crate) async fn wait(
        &self, mut result: tokio::sync::oneshot::Receiver<Result<(), String>>, deadline: std::time::Duration,
    ) -> Result<(), String> {
        let response = match tokio::time::timeout(deadline, &mut result).await {
            Ok(response) => response,
            Err(_) => {
                if self.0.compare_exchange(0, 2, std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Acquire).is_ok() {
                    return Err("Android installer dispatch timed out".into());
                }
                // The callback won the race and is already running. Await its
                // real result instead of reporting a timeout and deleting its APK.
                result.await
            }
        };
        response.map_err(|_| "Android installer context was closed".to_string())?
    }
}

#[cfg(target_os = "android")]
static CACHE_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

#[cfg(target_os = "android")]
pub(crate) fn configure_cache(path: PathBuf) {
    let _ = CACHE_DIR.set(path);
}

#[cfg(target_os = "android")]
pub(crate) fn stage_for_install(source: &Path, expected_sha256: &str) -> io::Result<PathBuf> {
    let cache = CACHE_DIR.get().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "Android app cache is unavailable")
    })?;
    stage_verified_apk(source, cache, expected_sha256)
}

fn stage_verified_apk(source: &Path, cache: &Path, expected_sha256: &str) -> io::Result<PathBuf> {
    // Open first: a missing source must not publish even an empty APK.
    let mut input = std::fs::File::open(source)?;
    let dir = cache.join("updates");
    std::fs::create_dir_all(&dir)?;
    let mut staged = tempfile::Builder::new().prefix("gipny-").suffix(".apk").tempfile_in(&dir)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 32 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 { break; }
        staged.write_all(&buffer[..read])?;
        digest.update(&buffer[..read]);
    }
    let actual = digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "staged APK checksum mismatch"));
    }
    staged.flush()?;
    staged.as_file().sync_all()?;
    // Dropping a failed copy removes only this attempt's temporary file.
    // A complete copy survives the Rust call so Android can read the URI later.
    let (file, path) = staged.keep().map_err(|error| error.error)?;
    drop(file);
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checksum(bytes: &[u8]) -> String { Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect() }

    #[test]
    fn installer_copy_uses_private_provider_directory_and_keeps_verified_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("verified.apk");
        let cache = dir.path().join("cache");
        std::fs::write(&source, b"verified package").unwrap();
        let copy = stage_verified_apk(&source, &cache, &checksum(b"verified package")).unwrap();
        assert_eq!(copy.parent().unwrap(), cache.join("updates"));
        assert_eq!(copy.extension().unwrap(), "apk");
        assert_eq!(std::fs::read(&copy).unwrap(), b"verified package");
        assert_eq!(std::fs::read(&source).unwrap(), b"verified package");
    }

    #[test]
    fn changed_source_fails_verification_without_leaving_installable_copy() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("verified.apk");
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(cache.join("updates")).unwrap();
        let previous = cache.join("updates/previous.apk");
        std::fs::write(&previous, b"previous package").unwrap();
        std::fs::write(&source, b"changed after download").unwrap();
        let error = stage_verified_apk(&source, &cache, &checksum(b"verified package")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read(&previous).unwrap(), b"previous package");
        assert_eq!(std::fs::read_dir(cache.join("updates")).unwrap().count(), 1);
        assert!(source.exists());
    }

    #[test]
    fn repeated_attempts_do_not_replace_an_apk_the_installer_may_be_reading() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("verified.apk");
        let cache = dir.path().join("cache");
        std::fs::write(&source, b"first").unwrap();
        let first = stage_verified_apk(&source, &cache, &checksum(b"first")).unwrap();
        std::fs::write(&source, b"second").unwrap();
        let second = stage_verified_apk(&source, &cache, &checksum(b"second")).unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(first).unwrap(), b"first");
        assert_eq!(std::fs::read(second).unwrap(), b"second");
    }

    #[test]
    fn missing_source_cannot_publish_an_empty_apk() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        assert!(stage_verified_apk(&dir.path().join("missing"), &cache, &checksum(b"")).is_err());
        assert!(!cache.exists());
    }

    #[tokio::test]
    async fn queued_installer_callback_is_cancelled_when_dispatch_expires() {
        let dispatch = InstallerDispatch::default();
        let (_sender, receiver) = tokio::sync::oneshot::channel();
        assert!(dispatch.wait(receiver, std::time::Duration::ZERO).await.is_err());
        assert!(!dispatch.try_start(), "a late callback must not launch an expired request");
    }

    #[tokio::test]
    async fn running_installer_keeps_its_uri_past_the_queue_deadline() {
        let dispatch = std::sync::Arc::new(InstallerDispatch::default());
        assert!(dispatch.try_start());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let running = dispatch.clone();
        let waiter = tokio::spawn(async move { running.wait(receiver, std::time::Duration::ZERO).await });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!waiter.is_finished(), "a running callback must not become a timeout failure");
        sender.send(Ok(())).unwrap();
        assert!(waiter.await.unwrap().is_ok());
    }
}
