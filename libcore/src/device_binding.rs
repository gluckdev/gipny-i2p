//! OS credential custody. Profile files contain identifiers/ciphertext only.
//! Copying a profile does not provision the destination OS credential provider.
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use thiserror::Error;
use zeroize::Zeroizing;

pub type Result<T> = std::result::Result<T, BindingError>;
const FILE_NAME: &str = ".device-binding-v2.json";
const MAX_METADATA: u64 = 65_536;

#[derive(Debug, Error)]
pub enum BindingError {
    #[error("OS credential provider unavailable: {0}; unlock or provision protected OS credentials before opening GIPNY")]
    Unavailable(&'static str),
    #[error("device credential missing or inaccessible; raw profile copies cannot be opened; use an explicitly encrypted export/import")]
    MissingCredential,
    #[error("device credential does not authenticate this profile; data and password attempt counter were not changed")]
    BindingMismatch,
    #[error("profile belongs to another OS credential provider; use an explicitly encrypted export/import")]
    OtherDevice,
    #[error("legacy unbound profile is unsupported; data was not deleted")]
    LegacyUnbound,
    #[error("legacy plaintext headless database is unsupported; data was not deleted")]
    LegacyPlaintextDatabase,
    #[error("invalid device-binding metadata")]
    InvalidMetadata,
    #[error("protected device credential I/O: {0}")]
    Io(#[from] std::io::Error),
}

/// Implementations must keep their wrapping key outside the profile and never
/// silently fall back to a file, hostname, machine-id or password-derived key.
/// Android installs its Keystore implementation before any Vault operation.
pub trait DeviceKeyProvider: Send + Sync {
    fn kind(&self) -> &'static str;
    fn seal(&self, id: &str, secret: &[u8; 32]) -> Result<Vec<u8>>;
    fn unseal(&self, id: &str, wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>>;
    fn delete(&self, id: &str) -> Result<()>;
}

static REGISTERED: OnceLock<Arc<dyn DeviceKeyProvider>> = OnceLock::new();

pub fn install_provider(provider: Arc<dyn DeviceKeyProvider>) -> Result<()> {
    REGISTERED.set(provider).map_err(|_| BindingError::Unavailable("provider already configured"))
}

fn provider() -> Result<Arc<dyn DeviceKeyProvider>> {
    #[cfg(test)]
    if let Some(provider) = TEST_PROVIDER.with(|slot| slot.borrow().clone()) { return Ok(provider); }
    if let Some(provider) = REGISTERED.get() { return Ok(provider.clone()); }
    #[cfg(target_os = "windows")]
    let native: Arc<dyn DeviceKeyProvider> = Arc::new(windows::Provider);
    #[cfg(target_os = "macos")]
    let native: Arc<dyn DeviceKeyProvider> = Arc::new(macos::Provider);
    #[cfg(target_os = "linux")]
    let native: Arc<dyn DeviceKeyProvider> = Arc::new(linux::Provider);
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    { Ok(native) }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    { Err(BindingError::Unavailable("native protected-credential provider is not configured")) }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    version: u8,
    provider: String,
    id: String,
    wrapped: Vec<u8>,
    check: [u8; 32],
}

fn check_mac(kind: &str, id: &str, key: &[u8; 32]) -> Hmac<Sha256> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("fixed HMAC key length");
    mac.update(b"gipny/device-binding/v2\0");
    mac.update(kind.as_bytes());
    mac.update(&[0]);
    mac.update(id.as_bytes());
    mac
}

fn metadata(dir: &Path) -> Result<Metadata> {
    if dir.join(".device").exists() { return Err(BindingError::LegacyUnbound); }
    let file = File::open(dir.join(FILE_NAME)).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound { BindingError::MissingCredential } else { error.into() }
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_METADATA + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_METADATA { return Err(BindingError::InvalidMetadata); }
    let value: Metadata = serde_json::from_slice(&bytes).map_err(|_| BindingError::InvalidMetadata)?;
    if value.version != 2 || value.id.len() != 64 || !value.id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || value.wrapped.len() > 4096 {
        return Err(BindingError::InvalidMetadata);
    }
    Ok(value)
}

pub fn load(dir: &Path) -> Result<Zeroizing<[u8; 32]>> {
    let value = metadata(dir)?;
    let provider = provider()?;
    if value.provider != provider.kind() { return Err(BindingError::OtherDevice); }
    let key = provider.unseal(&value.id, &value.wrapped)?;
    check_mac(provider.kind(), &value.id, &key).verify_slice(&value.check).map_err(|_| BindingError::BindingMismatch)?;
    Ok(key)
}

/// Provision an OS credential before publishing its public reference. A
/// concurrent enrollment can only use the winning complete metadata record.
pub fn enroll(dir: &Path) -> Result<Zeroizing<[u8; 32]>> {
    if dir.join(FILE_NAME).exists() { return load(dir); }
    if dir.join(".device").exists() || dir.join("vault.bin").exists() { return Err(BindingError::LegacyUnbound); }
    fs::create_dir_all(dir)?;
    let provider = provider()?;
    let mut id_bytes = [0u8; 32];
    crate::crypto::fill_random(&mut id_bytes);
    let id: String = id_bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut key = Zeroizing::new([0u8; 32]);
    crate::crypto::fill_random(key.as_mut_slice());
    let check: [u8; 32] = check_mac(provider.kind(), &id, &key).finalize().into_bytes().into();
    let wrapped = provider.seal(&id, &key)?;
    if wrapped.len() > 4096 {
        let _ = provider.delete(&id);
        return Err(BindingError::InvalidMetadata);
    }
    match provider.unseal(&id, &wrapped) {
        Ok(confirmed) if check_mac(provider.kind(), &id, &confirmed).verify_slice(&check).is_ok() => {}
        _ => { let _ = provider.delete(&id); return Err(BindingError::MissingCredential); }
    }
    let record = Metadata { version: 2, provider: provider.kind().into(), id, wrapped, check };
    let publish = (|| -> Result<_> {
        let mut temporary = tempfile::Builder::new().prefix(".binding-").tempfile_in(dir)?;
        temporary.write_all(&serde_json::to_vec(&record).map_err(|_| BindingError::InvalidMetadata)?)?;
        temporary.as_file().sync_all()?;
        Ok(temporary.persist_noclobber(dir.join(FILE_NAME)))
    })();
    match publish {
        Ok(Ok(_file)) => {
            // Never delete the credential after metadata became visible, even
            // if the directory sync fails: the next attempt must reuse it.
            #[cfg(unix)]
            File::open(dir)?.sync_all()?;
            Ok(key)
        }
        result => {
            let _ = provider.delete(&record.id);
            match result {
                Ok(Err(error)) if error.error.kind() == std::io::ErrorKind::AlreadyExists => load(dir),
                Ok(Err(error)) => Err(error.error.into()),
                Err(error) => Err(error),
                Ok(Ok(_)) => unreachable!(),
            }
        }
    }
}

/// Delete the OS credential before erasing profile metadata/data. Merely
/// opening a copied or unavailable profile never calls this function.
pub fn destroy(dir: &Path) -> Result<()> {
    let value = metadata(dir)?;
    let provider = provider()?;
    if value.provider != provider.kind() { return Err(BindingError::OtherDevice); }
    provider.delete(&value.id)?;
    fs::remove_file(dir.join(FILE_NAME))?;
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    pub(super) struct Provider;
    fn command(action: &str, id: &str, input: Option<&[u8]>) -> Result<Zeroizing<Vec<u8>>> {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
            return Err(BindingError::Unavailable("Linux Secret Service session is missing; unattended services require explicit OS credential provisioning"));
        }
        // No shell, PATH lookup, key in argv, logging of stderr or plaintext
        // keyfile. secret-tool speaks to the existing OS Secret Service.
        let mut command = Command::new("/usr/bin/secret-tool");
        command.arg(action);
        if action == "store" { command.arg("--label=GIPNY protected device credential"); }
        let mut child = command.args(["service", "app.gipny.i2p-device-v2", "binding-id", id])
            .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
            .map_err(|_| BindingError::Unavailable("Linux /usr/bin/secret-tool is unavailable"))?;
        if let Some(bytes) = input {
            let written = child.stdin.take().ok_or(BindingError::MissingCredential)?.write_all(bytes);
            if written.is_err() { let _ = child.kill(); let _ = child.wait(); return Err(BindingError::MissingCredential); }
        }
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() < Duration::from_secs(15) => std::thread::sleep(Duration::from_millis(20)),
                Ok(None) => { let _ = child.kill(); let _ = child.wait(); return Err(BindingError::Unavailable("Linux Secret Service is locked or did not respond")); }
                Err(error) => { let _ = child.kill(); let _ = child.wait(); return Err(error.into()); }
            }
        };
        if !status.success() { return Err(BindingError::MissingCredential); }
        if action != "lookup" { return Ok(Zeroizing::new(Vec::new())); }
        let mut output = Zeroizing::new(vec![0u8; 128]);
        let count = child.stdout.take().ok_or(BindingError::MissingCredential)?.read(&mut output)?;
        output.truncate(count);
        Ok(output)
    }
    impl DeviceKeyProvider for Provider {
        fn kind(&self) -> &'static str { "linux-secret-service-v2" }
        fn seal(&self, id: &str, secret: &[u8; 32]) -> Result<Vec<u8>> {
            let mut text = Zeroizing::new(Vec::with_capacity(65));
            const HEX: &[u8] = b"0123456789abcdef";
            for byte in secret { text.push(HEX[(byte >> 4) as usize]); text.push(HEX[(byte & 15) as usize]); }
            text.push(b'\n');
            command("store", id, Some(&text))?;
            Ok(Vec::new())
        }
        fn unseal(&self, id: &str, wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
            if !wrapped.is_empty() { return Err(BindingError::InvalidMetadata); }
            let bytes = command("lookup", id, None)?;
            let text = std::str::from_utf8(&bytes).map_err(|_| BindingError::MissingCredential)?.trim_end_matches(['\r', '\n']);
            if text.len() != 64 { return Err(BindingError::MissingCredential); }
            let mut key = Zeroizing::new([0u8; 32]);
            for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
                let digit = |byte: u8| match byte { b'0'..=b'9' => Some(byte - b'0'), b'a'..=b'f' => Some(byte - b'a' + 10), _ => None };
                key[index] = (digit(pair[0]).ok_or(BindingError::MissingCredential)? << 4) | digit(pair[1]).ok_or(BindingError::MissingCredential)?;
            }
            Ok(key)
        }
        fn delete(&self, id: &str) -> Result<()> { command("clear", id, None).map(|_| ()) }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use zeroize::Zeroize;
    #[repr(C)]
    struct Blob { length: u32, data: *mut u8 }
    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(input: *const Blob, description: *const u16, entropy: *const Blob, reserved: *mut c_void, prompt: *const c_void, flags: u32, output: *mut Blob) -> i32;
        fn CryptUnprotectData(input: *const Blob, description: *mut *mut u16, entropy: *const Blob, reserved: *mut c_void, prompt: *const c_void, flags: u32, output: *mut Blob) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" { fn LocalFree(memory: *mut c_void) -> *mut c_void; }
    struct OwnedBlob(Blob);
    impl Drop for OwnedBlob {
        fn drop(&mut self) {
            if !self.0.data.is_null() {
                unsafe {
                    std::slice::from_raw_parts_mut(self.0.data, self.0.length as usize).zeroize();
                    LocalFree(self.0.data.cast());
                }
            }
        }
    }
    fn dpapi(bytes: &[u8], id: &str, protect: bool, machine: bool) -> Result<Zeroizing<Vec<u8>>> {
        let input = Blob { length: bytes.len().try_into().map_err(|_| BindingError::InvalidMetadata)?, data: bytes.as_ptr().cast_mut() };
        let entropy = Blob { length: id.len() as u32, data: id.as_ptr().cast_mut() };
        let mut output = OwnedBlob(Blob { length: 0, data: std::ptr::null_mut() });
        // UI_FORBIDDEN; inner user scope plus outer LOCAL_MACHINE closes the
        // roaming-user exception. Full machine/OS compromise is a separate threat.
        let success = unsafe {
            if protect { CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null_mut(), std::ptr::null(), 1 | if machine { 4 } else { 0 }, &mut output.0) }
            else { CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null_mut(), std::ptr::null(), 1, &mut output.0) }
        };
        if success == 0 || output.0.data.is_null() || output.0.length > 4096 { return Err(BindingError::MissingCredential); }
        Ok(Zeroizing::new(unsafe { std::slice::from_raw_parts(output.0.data, output.0.length as usize) }.to_vec()))
    }
    pub(super) struct Provider;
    impl DeviceKeyProvider for Provider {
        fn kind(&self) -> &'static str { "windows-dpapi-user-and-machine-v2" }
        fn seal(&self, id: &str, secret: &[u8; 32]) -> Result<Vec<u8>> {
            let inner = dpapi(secret, id, true, false)?;
            Ok(dpapi(&inner, id, true, true)?.to_vec())
        }
        fn unseal(&self, id: &str, wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
            let inner = dpapi(wrapped, id, false, true)?;
            let secret = dpapi(&inner, id, false, false)?;
            if secret.len() != 32 { return Err(BindingError::MissingCredential); }
            let mut key = Zeroizing::new([0u8; 32]);
            key.copy_from_slice(&secret);
            Ok(key)
        }
        fn delete(&self, _: &str) -> Result<()> { Ok(()) }
    }
}

#[cfg(target_os = "macos")]
mod macos;

#[cfg(test)]
thread_local! { static TEST_PROVIDER: std::cell::RefCell<Option<Arc<dyn DeviceKeyProvider>>> = std::cell::RefCell::new(None); }

#[cfg(test)]
pub(crate) fn with_test_provider<T>(provider: Arc<dyn DeviceKeyProvider>, run: impl FnOnce() -> T) -> T {
    struct Reset(Option<Arc<dyn DeviceKeyProvider>>);
    impl Drop for Reset { fn drop(&mut self) { TEST_PROVIDER.with(|slot| *slot.borrow_mut() = self.0.take()); } }
    let _reset = Reset(TEST_PROVIDER.with(|slot| slot.borrow_mut().replace(provider)));
    run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Barrier, Mutex};
    use crate::security::{ArgonParams, DuressMode, SecurityError, Vault};

    #[derive(Default)]
    struct FakeOs {
        secrets: Mutex<HashMap<String, Zeroizing<[u8; 32]>>>,
        barrier: Option<Arc<Barrier>>,
        block_publication: Option<std::path::PathBuf>,
        unavailable: std::sync::atomic::AtomicBool,
    }
    impl DeviceKeyProvider for FakeOs {
        fn kind(&self) -> &'static str { "test-independent-os-credentials-v2" }
        fn seal(&self, id: &str, secret: &[u8; 32]) -> Result<Vec<u8>> {
            self.secrets.lock().unwrap().insert(id.into(), Zeroizing::new(*secret));
            if let Some(barrier) = &self.barrier { barrier.wait(); }
            if let Some(path) = &self.block_publication { fs::write(path, b"another enrollment owns this path")?; }
            Ok(Vec::new())
        }
        fn unseal(&self, id: &str, wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
            if self.unavailable.load(std::sync::atomic::Ordering::SeqCst) { return Err(BindingError::Unavailable("synthetic unavailable OS provider")); }
            if !wrapped.is_empty() { return Err(BindingError::InvalidMetadata); }
            self.secrets.lock().unwrap().get(id).map(|key| Zeroizing::new(**key)).ok_or(BindingError::MissingCredential)
        }
        fn delete(&self, id: &str) -> Result<()> { self.secrets.lock().unwrap().remove(id); Ok(()) }
    }
    fn params() -> ArgonParams { ArgonParams { m: 1024, t: 1, p: 1 } }
    fn copy_profile(source: &Path, destination: &Path) {
        for item in fs::read_dir(source).unwrap() {
            let item = item.unwrap();
            if item.path().is_file() { fs::copy(item.path(), destination.join(item.file_name())).unwrap(); }
        }
    }

    #[test]
    fn raw_copy_to_another_os_cannot_open_and_missing_credential_never_counts_or_wipes() {
        let original = tempfile::tempdir().unwrap();
        let copied = tempfile::tempdir().unwrap();
        let os_a = Arc::new(FakeOs::default());
        with_test_provider(os_a.clone(), || {
            let vault = Vault::create_with_params(original.path(), "passphrase", None, DuressMode::Wipe, 1, params()).unwrap();
            let vault_before = fs::read(Vault::path_of(original.path())).unwrap();
            let binding_before = fs::read(original.path().join(FILE_NAME)).unwrap();
            assert!(!original.path().join(".device").exists());
            copy_profile(original.path(), copied.path());
            with_test_provider(Arc::new(FakeOs::default()), || {
                assert!(matches!(Vault::open(copied.path()), Err(SecurityError::DeviceBinding(BindingError::MissingCredential))));
            });
            assert_eq!(fs::read(Vault::path_of(copied.path())).unwrap(), vault_before);
            os_a.unavailable.store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(matches!(vault.unlock("wrong password"), Err(SecurityError::DeviceBinding(BindingError::Unavailable(_)))));
            assert_eq!(fs::read(Vault::path_of(original.path())).unwrap(), vault_before);
            os_a.unavailable.store(false, std::sync::atomic::Ordering::SeqCst);
            let value = metadata(original.path()).unwrap();
            let key = os_a.secrets.lock().unwrap().remove(&value.id).unwrap();
            assert!(matches!(vault.unlock("wrong password"), Err(SecurityError::DeviceBinding(BindingError::MissingCredential))));
            assert_eq!(fs::read(Vault::path_of(original.path())).unwrap(), vault_before);
            assert_eq!(fs::read(original.path().join(FILE_NAME)).unwrap(), binding_before);
            os_a.secrets.lock().unwrap().insert(value.id.clone(), Zeroizing::new([0x77; 32]));
            assert!(matches!(vault.unlock("passphrase"), Err(SecurityError::DeviceBinding(BindingError::BindingMismatch))));
            assert_eq!(fs::read(Vault::path_of(original.path())).unwrap(), vault_before);
            os_a.secrets.lock().unwrap().insert(value.id, key);
            assert!(matches!(vault.unlock("passphrase"), Ok(crate::security::UnlockOutcome::Primary(_))));
        });
    }

    #[test]
    fn enrollment_reuses_committed_binding_after_interruption_before_vault_and_rejects_legacy() {
        let directory = tempfile::tempdir().unwrap();
        let provider = Arc::new(FakeOs::default());
        with_test_provider(provider.clone(), || {
            let enrolled = enroll(directory.path()).unwrap();
            // The public reference is durable but the application was killed
            // before vault publication. An unrelated partial file is ignored.
            fs::write(directory.path().join(".vault-interrupted"), b"incomplete ciphertext").unwrap();
            let _vault = Vault::create_with_params(directory.path(), "passphrase", None, DuressMode::Wipe, 0, params()).unwrap();
            assert_eq!(*load(directory.path()).unwrap(), *enrolled);
            assert_eq!(provider.secrets.lock().unwrap().len(), 1);
            let before = fs::read(Vault::path_of(directory.path())).unwrap();
            fs::write(directory.path().join(".device"), "00".repeat(32)).unwrap();
            assert!(matches!(load(directory.path()), Err(BindingError::LegacyUnbound)));
            assert!(matches!(Vault::open(directory.path()), Err(SecurityError::DeviceBinding(BindingError::LegacyUnbound))));
            assert_eq!(fs::read(Vault::path_of(directory.path())).unwrap(), before);
        });
    }

    #[test]
    fn concurrent_enrollment_has_one_credential_and_a_failed_publication_preserves_foreign_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let provider = Arc::new(FakeOs { barrier: Some(Arc::new(Barrier::new(2))), ..Default::default() });
        let workers: Vec<_> = (0..2).map(|_| {
            let provider = provider.clone(); let path = directory.path().to_owned();
            std::thread::spawn(move || with_test_provider(provider, || enroll(&path).unwrap()))
        }).collect();
        let keys: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
        assert_eq!(*keys[0], *keys[1]);
        assert_eq!(provider.secrets.lock().unwrap().len(), 1);
        let other = tempfile::tempdir().unwrap();
        let failed = Arc::new(FakeOs { block_publication: Some(other.path().join(FILE_NAME)), ..Default::default() });
        with_test_provider(failed.clone(), || assert!(enroll(other.path()).is_err()));
        assert!(failed.secrets.lock().unwrap().is_empty());
        assert_eq!(fs::read(other.path().join(FILE_NAME)).unwrap(), b"another enrollment owns this path");
    }

    #[test]
    fn headless_database_is_encrypted_and_plaintext_legacy_is_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.db");
        with_test_provider(Arc::new(FakeOs::default()), || {
            let db = crate::security::open_device_bound_database(directory.path(), &path).unwrap();
            db.set_setting("private fixture", b"private value").unwrap();
            drop(db);
            assert_ne!(&fs::read(&path).unwrap()[..16], b"SQLite format 3\0");
            let db = crate::security::open_device_bound_database(directory.path(), &path).unwrap();
            assert_eq!(db.get_setting("private fixture").unwrap().unwrap(), b"private value");
        });
        let legacy = tempfile::tempdir().unwrap();
        let path = legacy.path().join("agent.db");
        let db = crate::db::Db::open_plain(&path).unwrap();
        db.set_setting("keep", b"data").unwrap(); drop(db);
        let before = fs::read(&path).unwrap();
        assert!(matches!(crate::security::open_device_bound_database(legacy.path(), &path), Err(SecurityError::DeviceBinding(BindingError::LegacyPlaintextDatabase))));
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn native_windows_dpapi_roundtrip_rejects_a_different_profile_and_tampering() {
        let provider = windows::Provider;
        let id = "a".repeat(64);
        let secret = [0x42; 32];
        let mut wrapped = provider.seal(&id, &secret).unwrap();
        assert_eq!(*provider.unseal(&id, &wrapped).unwrap(), secret);
        assert!(provider.unseal(&"b".repeat(64), &wrapped).is_err());
        wrapped[0] ^= 1;
        assert!(provider.unseal(&id, &wrapped).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_macos_missing_device_credential_fails_without_a_portable_fallback() {
        let random: [u8; 32] = crate::crypto::random_array();
        let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        assert!(macos::Provider.unseal(&id, &[]).is_err());
    }
}
