use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{aead::Aead, KeyInit, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use crate::crypto::fill_random;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use zeroize::Zeroizing;

pub type Result<T> = std::result::Result<T, SecurityError>;

#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("io")] Io(#[from] std::io::Error),
    #[error("codec")] Codec(#[from] bincode::Error),
    #[error("crypto")] Crypto,
    #[error("invalid passphrase")] InvalidPassphrase,
    #[error("integrity")] Integrity,
    #[error("keystore")] Keystore,
    #[error("state")] State,
    #[error(transparent)] Memory(#[from] crate::memory::MemoryError),
    #[error(transparent)] DeviceBinding(#[from] crate::device_binding::BindingError),
    #[error(transparent)] Database(#[from] crate::db::DbError),
}

const MAGIC: [u8; 8] = *b"GIPNY002";
const VERSION: u16 = 2;
const MK_LEN: usize = 32;
const VAULT_FILE: &str = "vault.bin";
const INFO_KEK: &[u8] = b"gipny/kek/v2";
const THROTTLE_MS: u64 = 400;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct ArgonParams { pub m: u32, pub t: u32, pub p: u32 }

// Desktop: 256 MiB / t=4 — comfortably above OWASP recommendations.
// Mobile: 64 MiB / t=3 — still strong, but fits a phone: the 256 MiB spike on
// top of a WebView made first-run/unlock take many seconds and put the process
// in the OOM-killer's crosshairs (observed as a killed first run on Android).
// The params live in the vault header, so existing vaults keep whatever cost
// they were created with.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl Default for ArgonParams { fn default() -> Self { Self { m: 262144, t: 4, p: 1 } } }
#[cfg(any(target_os = "android", target_os = "ios"))]
impl Default for ArgonParams { fn default() -> Self { Self { m: 65536, t: 3, p: 1 } } }

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DuressMode { Wipe, Decoy }

#[derive(Clone, Serialize, Deserialize)]
struct Header {
    magic: [u8; 8],
    version: u16,
    argon: ArgonParams,
    salt_primary: [u8; 32],
    salt_duress: [u8; 32],
    nonce_primary: [u8; 24],
    nonce_duress: [u8; 24],
    ct_primary: Vec<u8>,
    ct_duress: Vec<u8>,
    attempts: u32,
    max_attempts: u32,
    duress_mode: DuressMode,
}

#[derive(Serialize, Deserialize)]
struct VaultFile { header: Header, hmac: [u8; 32] }

pub struct MasterKey(crate::memory::Sealed<MK_LEN>);
impl MasterKey {
    pub fn expose(&self) -> crate::memory::Result<crate::memory::SecretGuard<'_, MK_LEN>> { self.0.open() }
    fn from_slice(s: &[u8]) -> Result<Self> {
        if s.len() != MK_LEN { return Err(SecurityError::Crypto); }
        Ok(Self(crate::memory::Sealed::from_slice(crate::memory::SecretKind::Master, s)?))
    }
}

pub enum UnlockOutcome {
    Primary(MasterKey),
    Decoy(MasterKey),
    Wiped,
}

struct VaultState { header: Header, last_try: Option<Instant> }

pub struct Vault {
    path: PathBuf,
    dir: PathBuf,
    // Covers throttle, authenticated disk reread and any resulting mutation.
    // The state mutex alone cannot protect a header read before taking it.
    operation: Mutex<()>,
    state: Mutex<VaultState>,
}

impl Vault {
    pub fn path_of(dir: &Path) -> PathBuf { dir.join(VAULT_FILE) }
    pub fn exists(dir: &Path) -> bool { Self::path_of(dir).exists() }

    pub fn create(
        dir: &Path,
        pass: &str,
        duress_pass: Option<&str>,
        duress_mode: DuressMode,
        max_attempts: u32,
    ) -> Result<Self> {
        Self::create_with_params(dir, pass, duress_pass, duress_mode, max_attempts, ArgonParams::default())
    }

    /// Like [`Vault::create`] with explicit Argon2 cost parameters (tests and
    /// benchmarks; production callers use the platform default).
    pub fn create_with_params(
        dir: &Path,
        pass: &str,
        duress_pass: Option<&str>,
        duress_mode: DuressMode,
        max_attempts: u32,
        argon: ArgonParams,
    ) -> Result<Self> {
        if pass.is_empty() { return Err(SecurityError::InvalidPassphrase); }
        fs::create_dir_all(dir)?;
        let path = Self::path_of(dir);
        if path.exists() { return Err(SecurityError::State); }
        let device = DeviceBind::enroll(dir)?;
        let (salt_primary, salt_duress) = (random32(), random32());
        let nonce_primary = random24();

        let mut master = Zeroizing::new([0u8; MK_LEN]);
        fill_random(master.as_mut_slice());
        let kek_p = derive_kek(pass, &salt_primary, &argon, &device)?;
        let ct_primary = seal(&kek_p, &nonce_primary, master.as_slice())?;

        let (ct_duress, nonce_duress) = match duress_pass {
            Some(dp) if !dp.is_empty() && dp != pass => {
                let n = random24();
                let kek_d = derive_kek(dp, &salt_duress, &argon, &device)?;
                let mut mkd = Zeroizing::new([0u8; MK_LEN]);
                fill_random(mkd.as_mut_slice());
                (seal(&kek_d, &n, mkd.as_slice())?, n)
            }
            _ => (Vec::new(), random24()),
        };

        let header = Header {
            magic: MAGIC, version: VERSION, argon,
            salt_primary, salt_duress,
            nonce_primary, nonce_duress,
            ct_primary, ct_duress,
            attempts: 0, max_attempts, duress_mode,
        };
        write_vault_impl(&path, &header, &device, true)?;
        Ok(Self {
            path, dir: dir.to_path_buf(),
            operation: Mutex::new(()),
            state: Mutex::new(VaultState { header, last_try: None }),
        })
    }

    pub fn open(dir: &Path) -> Result<Self> {
        let path = Self::path_of(dir);
        let device = DeviceBind::load(dir)?;
        let header = read_vault(&path, &device)?;
        Ok(Self {
            path, dir: dir.to_path_buf(),
            operation: Mutex::new(()),
            state: Mutex::new(VaultState { header, last_try: None }),
        })
    }

    pub fn unlock(&self, pass: &str) -> Result<UnlockOutcome> {
        let _operation = self.operation.lock().map_err(|_| SecurityError::State)?;
        self.unlock_locked(pass)
    }

    fn unlock_locked(&self, pass: &str) -> Result<UnlockOutcome> {
        self.throttle();
        let device = DeviceBind::ensure(&self.dir)?;
        let verified = read_vault(&self.path, &device)?;
        let mut st = self.state.lock().map_err(|_| SecurityError::State)?;
        st.header = verified;

        {
            let kek = derive_kek(pass, &st.header.salt_primary, &st.header.argon, &device)?;
            if let Ok(mk) = open_vault_seal(&kek, &st.header.nonce_primary, &st.header.ct_primary) {
                st.header.attempts = 0;
                write_vault(&self.path, &st.header, &device)?;
                return Ok(UnlockOutcome::Primary(MasterKey::from_slice(&mk)?));
            }
        }

        if !st.header.ct_duress.is_empty() {
            {
                let kek = derive_kek(pass, &st.header.salt_duress, &st.header.argon, &device)?;
                if let Ok(mk) = open_vault_seal(&kek, &st.header.nonce_duress, &st.header.ct_duress) {
                    let mode = st.header.duress_mode;
                    st.header.attempts = 0;
                    write_vault(&self.path, &st.header, &device)?;
                    drop(st);
                    return match mode {
                        DuressMode::Decoy => Ok(UnlockOutcome::Decoy(MasterKey::from_slice(&mk)?)),
                        DuressMode::Wipe => { self.wipe_locked()?; Ok(UnlockOutcome::Wiped) }
                    };
                }
            }
        }

        st.header.attempts = st.header.attempts.saturating_add(1);
        let over = st.header.max_attempts > 0 && st.header.attempts >= st.header.max_attempts;
        write_vault(&self.path, &st.header, &device)?;
        st.last_try = Some(Instant::now());
        if over { drop(st); self.wipe_locked()?; return Ok(UnlockOutcome::Wiped); }
        Err(SecurityError::InvalidPassphrase)
    }

    pub fn change_passphrase(&self, old: &str, new: &str) -> Result<()> {
        if new.is_empty() { return Err(SecurityError::InvalidPassphrase); }
        let _operation = self.operation.lock().map_err(|_| SecurityError::State)?;
        let mk = match self.unlock_locked(old)? {
            UnlockOutcome::Primary(k) => k,
            _ => return Err(SecurityError::InvalidPassphrase),
        };
        let device = DeviceBind::ensure(&self.dir)?;
        let mut st = self.state.lock().map_err(|_| SecurityError::State)?;
        let salt = random32();
        let nonce = random24();
        let kek = derive_kek(new, &salt, &st.header.argon, &device)?;
        let ct = seal(&kek, &nonce, &*mk.expose()?)?;
        st.header.salt_primary = salt;
        st.header.nonce_primary = nonce;
        st.header.ct_primary = ct;
        write_vault(&self.path, &st.header, &device)
    }

    pub fn set_duress(&self, pass: &str, duress_pass: Option<&str>, mode: DuressMode) -> Result<()> {
        let _operation = self.operation.lock().map_err(|_| SecurityError::State)?;
        if !matches!(self.unlock_locked(pass)?, UnlockOutcome::Primary(_)) {
            return Err(SecurityError::InvalidPassphrase);
        }
        let device = DeviceBind::ensure(&self.dir)?;
        let mut st = self.state.lock().map_err(|_| SecurityError::State)?;
        st.header.duress_mode = mode;
        match duress_pass {
            Some(dp) if !dp.is_empty() && dp != pass => {
                let salt = random32();
                let nonce = random24();
                let kek = derive_kek(dp, &salt, &st.header.argon, &device)?;
                let mut mkd = Zeroizing::new([0u8; MK_LEN]);
                fill_random(mkd.as_mut_slice());
                let ct = seal(&kek, &nonce, mkd.as_slice())?;
                st.header.salt_duress = salt;
                st.header.nonce_duress = nonce;
                st.header.ct_duress = ct;
            }
            None => st.header.ct_duress.clear(),
            _ => return Err(SecurityError::InvalidPassphrase),
        }
        write_vault(&self.path, &st.header, &device)
    }

    pub fn set_max_attempts(&self, pass: &str, max: u32) -> Result<()> {
        let _operation = self.operation.lock().map_err(|_| SecurityError::State)?;
        if !matches!(self.unlock_locked(pass)?, UnlockOutcome::Primary(_)) {
            return Err(SecurityError::InvalidPassphrase);
        }
        let device = DeviceBind::ensure(&self.dir)?;
        let mut st = self.state.lock().map_err(|_| SecurityError::State)?;
        st.header.max_attempts = max;
        write_vault(&self.path, &st.header, &device)
    }

    pub fn wipe_all(&self) -> Result<()> {
        let _operation = self.operation.lock().map_err(|_| SecurityError::State)?;
        self.wipe_locked()
    }

    fn wipe_locked(&self) -> Result<()> {
        DeviceBind::destroy(&self.dir)?;
        secure_wipe_dir(&self.dir)?;
        Ok(())
    }

    fn throttle(&self) {
        let guard = match self.state.lock() { Ok(g) => g, Err(_) => return };
        if let Some(t) = guard.last_try {
            let elapsed = t.elapsed();
            let wait = Duration::from_millis(THROTTLE_MS);
            if elapsed < wait { drop(guard); std::thread::sleep(wait - elapsed); }
        }
    }
}

fn random32() -> [u8; 32] { crate::crypto::random_array() }
fn random24() -> [u8; 24] { crate::crypto::random_array() }

fn derive_kek(pass: &str, salt: &[u8; 32], p: &ArgonParams, device: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(p.m, p.t, p.p, Some(32)).map_err(|_| SecurityError::Crypto)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut raw = Zeroizing::new([0u8; 32]);
    hash_password(&argon, pass.as_bytes(), salt, raw.as_mut_slice())?;
    let hk = Hkdf::<Sha256>::new(Some(device), raw.as_slice());
    let mut kek = Zeroizing::new([0u8; 32]);
    hk.expand(INFO_KEK, kek.as_mut_slice()).map_err(|_| SecurityError::Crypto)?;
    Ok(kek)
}

// Argon2 0.6's allocating Blocks::Drop deallocates without wiping its heap.
// Provide a pre-sized, zeroizing workspace instead. This remains transient
// ordinary memory; locking the whole 64/256 MiB KDF workspace is not attempted.
fn hash_password(argon: &Argon2<'_>, pass: &[u8], salt: &[u8], out: &mut [u8]) -> Result<()> {
    #[cfg(test)]
    assert!(!KDF_FORBIDDEN.with(std::cell::Cell::get), "untrusted backup cost reached KDF allocation");
    let count = argon.params().block_count();
    let mut blocks = Zeroizing::new(Vec::<argon2::Block>::new());
    blocks.try_reserve_exact(count).map_err(|_| SecurityError::Crypto)?;
    blocks.resize_with(count, argon2::Block::default);
    argon.hash_password_into_with_memory(pass, salt, out, &mut *blocks).map_err(|_| SecurityError::Crypto)
}

#[cfg(test)]
thread_local! { static KDF_FORBIDDEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

fn seal(key: &[u8; 32], nonce: &[u8; 24], plain: &[u8]) -> Result<Vec<u8>> {
    XChaCha20Poly1305::new(key.into())
        .encrypt(&XNonce::from(*nonce), plain)
        .map_err(|_| SecurityError::Crypto)
}

fn open_vault_seal(key: &[u8; 32], nonce: &[u8; 24], ct: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    open_seal(key, nonce, ct).map(Zeroizing::new)
}

pub const BACKUP_MAGIC: &[u8; 8] = b"GIPNYBAK";
const BACKUP_VERSION: u8 = 1;
const BACKUP_MIN_MEMORY_KIB: u32 = 8 * 1024;
const BACKUP_MAX_MEMORY_KIB: u32 = 262144;

fn validate_backup_memory(m: u32) -> Result<()> {
    if !(BACKUP_MIN_MEMORY_KIB..=BACKUP_MAX_MEMORY_KIB).contains(&m) { return Err(SecurityError::Crypto); }
    Ok(())
}

pub fn backup_seal(passphrase: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
    let salt = random32();
    let nonce = random24();
    let params = ArgonParams { m: 65536, t: 3, p: 1 };
    let kparams = Params::new(params.m, params.t, params.p, Some(32)).map_err(|_| SecurityError::Crypto)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, kparams);
    let mut key = Zeroizing::new([0u8; 32]);
    hash_password(&argon, passphrase.as_bytes(), &salt, key.as_mut_slice())?;
    let ct = seal(key.as_slice().try_into().unwrap(), &nonce, plaintext)?;
    let mut out = Vec::with_capacity(8 + 1 + 4 + 32 + 24 + ct.len());
    out.extend_from_slice(BACKUP_MAGIC);
    out.push(BACKUP_VERSION);
    out.extend_from_slice(&params.m.to_be_bytes());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn backup_open(passphrase: &str, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if blob.len() < 8 + 1 + 4 + 32 + 24 + 16 { return Err(SecurityError::Crypto); }
    if &blob[..8] != BACKUP_MAGIC { return Err(SecurityError::Crypto); }
    if blob[8] != BACKUP_VERSION { return Err(SecurityError::Crypto); }
    let m = u32::from_be_bytes(blob[9..13].try_into().unwrap());
    validate_backup_memory(m)?;
    let salt: [u8; 32] = blob[13..45].try_into().unwrap();
    let nonce: [u8; 24] = blob[45..69].try_into().unwrap();
    let ct = &blob[69..];
    let params = Params::new(m, 3, 1, Some(32)).map_err(|_| SecurityError::Crypto)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    hash_password(&argon, passphrase.as_bytes(), &salt, key.as_mut_slice())?;
    open_vault_seal(key.as_slice().try_into().unwrap(), &nonce, ct)
}

pub const BACKUP_VERSION_3FACTOR: u8 = 3;

/// 3-Factor key derivation:
/// Factor 1: User passphrase (Argon2id with salt) -> K_pass
/// Factor 2: Server instance key (HKDF salt)
/// Factor 3: Client device secret (HKDF IKM input along with K_pass)
/// PRK = HKDF-Extract(salt = server_key, ikm = K_pass || client_secret)
/// K_final = HKDF-Expand(prk = PRK, info = b"gipny/backup/3factor/v1", length = 32)
fn derive_3factor_key(
    passphrase: &str,
    server_key: &[u8; 32],
    client_secret: &[u8; 32],
    salt: &[u8; 32],
    m: u32,
) -> Result<Zeroizing<[u8; 32]>> {
    validate_backup_memory(m)?;
    let kparams = Params::new(m, 3, 1, Some(32)).map_err(|_| SecurityError::Crypto)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, kparams);
    let mut k_pass = Zeroizing::new([0u8; 32]);
    hash_password(&argon, passphrase.as_bytes(), salt, k_pass.as_mut_slice())?;

    let mut ikm = Zeroizing::new(Vec::with_capacity(64));
    ikm.extend_from_slice(k_pass.as_slice());
    ikm.extend_from_slice(client_secret);

    let hkdf = Hkdf::<Sha256>::new(Some(server_key), &ikm);
    let mut final_key = Zeroizing::new([0u8; 32]);
    hkdf.expand(b"gipny/backup/3factor/v1", final_key.as_mut_slice())
        .map_err(|_| SecurityError::Crypto)?;

    Ok(final_key)
}

pub fn backup_seal_3factor(
    passphrase: &str,
    server_key: &[u8; 32],
    client_secret: &[u8; 32],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let salt = random32();
    let nonce = random24();
    let m = 65536u32;
    let key = derive_3factor_key(passphrase, server_key, client_secret, &salt, m)?;
    let ct = seal(key.as_slice().try_into().unwrap(), &nonce, plaintext)?;
    let mut out = Vec::with_capacity(8 + 1 + 4 + 32 + 24 + ct.len());
    out.extend_from_slice(BACKUP_MAGIC);
    out.push(BACKUP_VERSION_3FACTOR);
    out.extend_from_slice(&m.to_be_bytes());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn backup_open_3factor(
    passphrase: &str,
    server_key: &[u8; 32],
    client_secret: &[u8; 32],
    blob: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    if blob.len() < 8 + 1 + 4 + 32 + 24 + 16 { return Err(SecurityError::Crypto); }
    if &blob[..8] != BACKUP_MAGIC { return Err(SecurityError::Crypto); }
    if blob[8] != BACKUP_VERSION_3FACTOR { return Err(SecurityError::Crypto); }
    let m = u32::from_be_bytes(blob[9..13].try_into().unwrap());
    let salt: [u8; 32] = blob[13..45].try_into().unwrap();
    let nonce: [u8; 24] = blob[45..69].try_into().unwrap();
    let ct = &blob[69..];
    let key = derive_3factor_key(passphrase, server_key, client_secret, &salt, m)?;
    open_vault_seal(key.as_slice().try_into().unwrap(), &nonce, ct)
}

fn open_seal(key: &[u8; 32], nonce: &[u8; 24], ct: &[u8]) -> Result<Vec<u8>> {
    XChaCha20Poly1305::new(key.into())
        .decrypt(&XNonce::from(*nonce), ct)
        .map_err(|_| SecurityError::Crypto)
}

fn hmac_of(header: &Header, device: &[u8; 32]) -> Result<[u8; 32]> {
    let bytes = bincode::serialize(header)?;
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(device).map_err(|_| SecurityError::Crypto)?;
    mac.update(&bytes);
    Ok(mac.finalize().into_bytes().into())
}

fn write_vault(path: &Path, header: &Header, device: &[u8; 32]) -> Result<()> {
    write_vault_impl(path, header, device, false)
}

fn write_vault_impl(path: &Path, header: &Header, device: &[u8; 32], create: bool) -> Result<()> {
    let hmac = hmac_of(header, device)?;
    let file = VaultFile { header: header.clone(), hmac };
    let bytes = bincode::serialize(&file)?;
    let directory = path.parent().ok_or(SecurityError::State)?;
    let mut temporary = tempfile::Builder::new().prefix(".vault-").tempfile_in(directory)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    if create { temporary.persist_noclobber(path).map_err(|error| error.error)?; }
    else { temporary.persist(path).map_err(|error| error.error)?; }
    #[cfg(unix)]
    File::open(directory)?.sync_all()?;
    Ok(())
}

fn read_vault(path: &Path, device: &[u8; 32]) -> Result<Header> {
    use bincode::Options;
    let f = File::open(path)?;
    let mut buf = Vec::new();
    f.take(65_537).read_to_end(&mut buf)?;
    if buf.len() > 65_536 { return Err(SecurityError::Integrity); }
    let file: VaultFile = bincode::DefaultOptions::new().with_fixint_encoding()
        .reject_trailing_bytes().with_limit(65_536).deserialize(&buf)?;
    if file.header.magic == *b"GIPNY001" || file.header.version == 1 {
        return Err(crate::device_binding::BindingError::LegacyUnbound.into());
    }
    if file.header.magic != MAGIC || file.header.version != VERSION {
        return Err(SecurityError::Integrity);
    }
    let expected = hmac_of(&file.header, device)?;
    if !constant_eq(&expected, &file.hmac) { return Err(SecurityError::Integrity); }
    Ok(file.header)
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut r = 0u8;
    for (x, y) in a.iter().zip(b) { r |= x ^ y; }
    r == 0
}

pub fn secure_wipe_dir(dir: &Path) -> Result<()> {
    if !dir.exists() { return Ok(()); }
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_file() { let _ = overwrite_file(&p); let _ = fs::remove_file(&p); }
        else if p.is_dir() { let _ = secure_wipe_dir(&p); let _ = fs::remove_dir(&p); }
    }
    Ok(())
}

fn overwrite_file(path: &Path) -> Result<()> {
    let len = fs::metadata(path)?.len() as usize;
    if len == 0 { return Ok(()); }
    let mut f = OpenOptions::new().write(true).open(path)?;
    let chunk = vec![0u8; len.min(1 << 20)];
    let mut written = 0;
    while written < len {
        let n = (len - written).min(chunk.len());
        f.write_all(&chunk[..n])?;
        written += n;
    }
    f.sync_all()?;
    Ok(())
}

pub struct DeviceBind;

impl DeviceBind {
    pub fn enroll(dir: &Path) -> Result<Zeroizing<[u8; 32]>> { Ok(crate::device_binding::enroll(dir)?) }
    pub fn load(dir: &Path) -> Result<Zeroizing<[u8; 32]>> { Ok(crate::device_binding::load(dir)?) }
    /// Existing call sites only load: losing OS custody never creates a new
    /// secret or turns a provider error into an incorrect-password attempt.
    pub fn ensure(dir: &Path) -> Result<Zeroizing<[u8; 32]>> { Self::load(dir) }
    pub fn destroy(dir: &Path) -> Result<()> { Ok(crate::device_binding::destroy(dir)?) }
}

/// Unattended agent/bot storage still needs an enrolled protected OS credential.
/// No passphrase, hostname, env key or portable 0600 file is a substitute.
pub fn open_device_bound_database(dir: &Path, path: &Path) -> Result<crate::db::Db> {
    if path.parent() != Some(dir) { return Err(SecurityError::State); }
    if path.exists() {
        let mut prefix = [0u8; 16];
        if File::open(path)?.read(&mut prefix)? == 16 && &prefix == b"SQLite format 3\0" {
            return Err(crate::device_binding::BindingError::LegacyPlaintextDatabase.into());
        }
    }
    let device = if path.exists() { DeviceBind::load(dir)? } else { DeviceBind::enroll(dir)? };
    let name = path.file_name().and_then(|name| name.to_str()).ok_or(SecurityError::State)?;
    let mut database_key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(b"gipny/headless-database/v2"), device.as_slice())
        .expand(name.as_bytes(), database_key.as_mut_slice()).map_err(|_| SecurityError::Crypto)?;
    let master = MasterKey::from_slice(database_key.as_slice())?;
    Ok(crate::db::Db::open(path, &master)?)
}

pub struct ServerBind;

const SERVER_KEY_FILE: &str = "server.key";

impl ServerBind {
    pub fn ensure(dir: &Path) -> Result<[u8; 32]> {
        let path = dir.join(SERVER_KEY_FILE);
        if path.exists() {
            let mut f = File::open(&path)?;
            let mut s = String::new();
            f.read_to_string(&mut s)?;
            let bytes = hex_decode(s.trim()).ok_or(SecurityError::Keystore)?;
            <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| SecurityError::Keystore)
        } else {
            fs::create_dir_all(dir)?;
            let s = random32();
            let tmp = path.with_extension("tmp");
            let mut f = OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)?;
            f.write_all(hex_encode(&s).as_bytes())?;
            f.sync_all()?;
            drop(f);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
            }
            fs::rename(tmp, &path)?;
            Ok(s)
        }
    }
}

#[cfg(test)]
mod factor3_tests {
    use super::*;

    #[test]
    fn test_3factor_backup_roundtrip_and_component_isolation() {
        let pass = "user_master_password_secure";
        let server_key = [0x55u8; 32];
        let client_secret = [0xAAu8; 32];
        let data = b"confidential session payload for gipny web messenger";

        let sealed = backup_seal_3factor(pass, &server_key, &client_secret, data).unwrap();

        // 1. Success with all three correct components
        let opened = backup_open_3factor(pass, &server_key, &client_secret, &sealed).unwrap();
        assert_eq!(opened.as_slice(), data);

        // 2. Fails if passphrase is wrong
        assert!(backup_open_3factor("wrong_passphrase", &server_key, &client_secret, &sealed).is_err());

        // 3. Fails if server_key is wrong
        let mut wrong_server = server_key;
        wrong_server[0] ^= 0xFF;
        assert!(backup_open_3factor(pass, &wrong_server, &client_secret, &sealed).is_err());

        // 4. Fails if client_secret is wrong
        let mut wrong_client = client_secret;
        wrong_client[0] ^= 0xFF;
        assert!(backup_open_3factor(pass, &server_key, &wrong_client, &sealed).is_err());

        // 5. Fails if corrupted
        let mut corrupt = sealed.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 0xFF;
        assert!(backup_open_3factor(pass, &server_key, &client_secret, &corrupt).is_err());
    }

    #[test]
    fn test_server_bind_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let key1 = ServerBind::ensure(dir.path()).unwrap();
        let key2 = ServerBind::ensure(dir.path()).unwrap();
        assert_eq!(key1, key2);
        assert_ne!(key1, [0u8; 32]);
    }

    #[test]
    fn password_backup_returns_owned_zeroizing_plaintext() {
        let data = b"private exported identity";
        let sealed = backup_seal("backup passphrase", data).unwrap();
        let opened: Zeroizing<Vec<u8>> = backup_open("backup passphrase", &sealed).unwrap();
        assert_eq!(opened.as_slice(), data);
        assert!(backup_open("wrong passphrase", &sealed).is_err());
    }
}

fn hex_encode(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b { s.push(H[(x >> 4) as usize] as char); s.push(H[(x & 0xf) as usize] as char); }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 { return None; }
    let b = s.as_bytes();
    let h = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in (0..b.len()).step_by(2) { out.push((h(b[i])? << 4) | h(b[i + 1])?); }
    Some(out)
}

pub use i2p_embed::process_security::harden_process;

#[cfg(test)]
mod backup_resource_tests {
    use super::*;

    #[test]
    fn untrusted_backup_cost_is_rejected_before_kdf_or_allocation() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) { KDF_FORBIDDEN.with(|flag| flag.set(false)); }
        }
        KDF_FORBIDDEN.with(|flag| flag.set(true));
        let _reset = Reset;
        for version in [BACKUP_VERSION, BACKUP_VERSION_3FACTOR] {
            for m in [0, BACKUP_MIN_MEMORY_KIB - 1, BACKUP_MAX_MEMORY_KIB + 1, u32::MAX] {
                let mut blob = vec![0; 8 + 1 + 4 + 32 + 24 + 16];
                blob[..8].copy_from_slice(BACKUP_MAGIC);
                blob[8] = version;
                blob[9..13].copy_from_slice(&m.to_be_bytes());
                let result = if version == BACKUP_VERSION {
                    backup_open("passphrase", &blob)
                } else {
                    backup_open_3factor("passphrase", &[1; 32], &[2; 32], &blob)
                };
                assert!(matches!(result, Err(SecurityError::Crypto)));
            }
        }
    }
}
