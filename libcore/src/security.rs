use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{aead::Aead, KeyInit, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
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
}

const MAGIC: [u8; 8] = *b"GIPNY001";
const VERSION: u16 = 1;
const MK_LEN: usize = 32;
const VAULT_FILE: &str = "vault.bin";
const INFO_KEK: &[u8] = b"gipny/kek/v1";
const THROTTLE_MS: u64 = 400;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct ArgonParams { pub m: u32, pub t: u32, pub p: u32 }
impl Default for ArgonParams { fn default() -> Self { Self { m: 262144, t: 4, p: 1 } } }

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

pub struct MasterKey(Zeroizing<[u8; MK_LEN]>);
impl MasterKey {
    pub fn as_bytes(&self) -> &[u8; MK_LEN] { &self.0 }
    fn from_slice(s: &[u8]) -> Result<Self> {
        if s.len() != MK_LEN { return Err(SecurityError::Crypto); }
        let mut a = [0u8; MK_LEN]; a.copy_from_slice(s);
        Ok(Self(Zeroizing::new(a)))
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
        if pass.is_empty() { return Err(SecurityError::InvalidPassphrase); }
        fs::create_dir_all(dir)?;
        let path = Self::path_of(dir);
        let device = DeviceBind::ensure(dir)?;
        let argon = ArgonParams::default();
        let (salt_primary, salt_duress) = (random32(), random32());
        let nonce_primary = random24();

        let mut master = Zeroizing::new([0u8; MK_LEN]);
        OsRng.fill_bytes(master.as_mut_slice());
        let kek_p = derive_kek(pass, &salt_primary, &argon, &device)?;
        let ct_primary = seal(&kek_p, &nonce_primary, master.as_slice())?;

        let (ct_duress, nonce_duress) = match duress_pass {
            Some(dp) if !dp.is_empty() && dp != pass => {
                let n = random24();
                let kek_d = derive_kek(dp, &salt_duress, &argon, &device)?;
                let mut mkd = Zeroizing::new([0u8; MK_LEN]);
                OsRng.fill_bytes(mkd.as_mut_slice());
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
        write_vault(&path, &header, &device)?;
        Ok(Self {
            path, dir: dir.to_path_buf(),
            state: Mutex::new(VaultState { header, last_try: None }),
        })
    }

    pub fn open(dir: &Path) -> Result<Self> {
        let path = Self::path_of(dir);
        let device = DeviceBind::ensure(dir)?;
        let header = read_vault(&path, &device)?;
        Ok(Self {
            path, dir: dir.to_path_buf(),
            state: Mutex::new(VaultState { header, last_try: None }),
        })
    }

    pub fn unlock(&self, pass: &str) -> Result<UnlockOutcome> {
        self.throttle();
        let device = DeviceBind::ensure(&self.dir)?;
        let mut st = self.state.lock().map_err(|_| SecurityError::State)?;

        if let Ok(kek) = derive_kek(pass, &st.header.salt_primary, &st.header.argon, &device) {
            if let Ok(mk) = open_seal(&kek, &st.header.nonce_primary, &st.header.ct_primary) {
                st.header.attempts = 0;
                write_vault(&self.path, &st.header, &device)?;
                return Ok(UnlockOutcome::Primary(MasterKey::from_slice(&mk)?));
            }
        }

        if !st.header.ct_duress.is_empty() {
            if let Ok(kek) = derive_kek(pass, &st.header.salt_duress, &st.header.argon, &device) {
                if let Ok(mk) = open_seal(&kek, &st.header.nonce_duress, &st.header.ct_duress) {
                    let mode = st.header.duress_mode;
                    st.header.attempts = 0;
                    write_vault(&self.path, &st.header, &device)?;
                    drop(st);
                    return match mode {
                        DuressMode::Decoy => Ok(UnlockOutcome::Decoy(MasterKey::from_slice(&mk)?)),
                        DuressMode::Wipe => { self.wipe_all()?; Ok(UnlockOutcome::Wiped) }
                    };
                }
            }
        }

        st.header.attempts = st.header.attempts.saturating_add(1);
        let over = st.header.max_attempts > 0 && st.header.attempts >= st.header.max_attempts;
        write_vault(&self.path, &st.header, &device)?;
        st.last_try = Some(Instant::now());
        if over { drop(st); self.wipe_all()?; return Ok(UnlockOutcome::Wiped); }
        Err(SecurityError::InvalidPassphrase)
    }

    pub fn change_passphrase(&self, old: &str, new: &str) -> Result<()> {
        if new.is_empty() { return Err(SecurityError::InvalidPassphrase); }
        let mk = match self.unlock(old)? {
            UnlockOutcome::Primary(k) => k,
            _ => return Err(SecurityError::InvalidPassphrase),
        };
        let device = DeviceBind::ensure(&self.dir)?;
        let mut st = self.state.lock().map_err(|_| SecurityError::State)?;
        let salt = random32();
        let nonce = random24();
        let kek = derive_kek(new, &salt, &st.header.argon, &device)?;
        let ct = seal(&kek, &nonce, mk.as_bytes())?;
        st.header.salt_primary = salt;
        st.header.nonce_primary = nonce;
        st.header.ct_primary = ct;
        write_vault(&self.path, &st.header, &device)
    }

    pub fn set_duress(&self, pass: &str, duress_pass: Option<&str>, mode: DuressMode) -> Result<()> {
        if !matches!(self.unlock(pass)?, UnlockOutcome::Primary(_)) {
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
                OsRng.fill_bytes(mkd.as_mut_slice());
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
        if !matches!(self.unlock(pass)?, UnlockOutcome::Primary(_)) {
            return Err(SecurityError::InvalidPassphrase);
        }
        let device = DeviceBind::ensure(&self.dir)?;
        let mut st = self.state.lock().map_err(|_| SecurityError::State)?;
        st.header.max_attempts = max;
        write_vault(&self.path, &st.header, &device)
    }

    pub fn wipe_all(&self) -> Result<()> {
        secure_wipe_dir(&self.dir)?;
        let _ = DeviceBind::destroy(&self.dir);
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

fn random32() -> [u8; 32] { let mut s = [0u8; 32]; OsRng.fill_bytes(&mut s); s }
fn random24() -> [u8; 24] { let mut s = [0u8; 24]; OsRng.fill_bytes(&mut s); s }

fn derive_kek(pass: &str, salt: &[u8; 32], p: &ArgonParams, device: &[u8; 32]) -> Result<[u8; 32]> {
    let params = Params::new(p.m, p.t, p.p, Some(32)).map_err(|_| SecurityError::Crypto)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut raw = Zeroizing::new([0u8; 32]);
    argon.hash_password_into(pass.as_bytes(), salt, raw.as_mut_slice()).map_err(|_| SecurityError::Crypto)?;
    let hk = Hkdf::<Sha256>::new(Some(device), raw.as_slice());
    let mut kek = [0u8; 32];
    hk.expand(INFO_KEK, &mut kek).map_err(|_| SecurityError::Crypto)?;
    Ok(kek)
}

fn seal(key: &[u8; 32], nonce: &[u8; 24], plain: &[u8]) -> Result<Vec<u8>> {
    XChaCha20Poly1305::new(key.into())
        .encrypt(XNonce::from_slice(nonce), plain)
        .map_err(|_| SecurityError::Crypto)
}

pub const BACKUP_MAGIC: &[u8; 8] = b"GIPNYBAK";
const BACKUP_VERSION: u8 = 1;

pub fn backup_seal(passphrase: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
    let salt = random32();
    let nonce = random24();
    let params = ArgonParams { m: 65536, t: 3, p: 1 };
    let kparams = Params::new(params.m, params.t, params.p, Some(32)).map_err(|_| SecurityError::Crypto)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, kparams);
    let mut key = Zeroizing::new([0u8; 32]);
    argon.hash_password_into(passphrase.as_bytes(), &salt, key.as_mut_slice()).map_err(|_| SecurityError::Crypto)?;
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

pub fn backup_open(passphrase: &str, blob: &[u8]) -> Result<Vec<u8>> {
    if blob.len() < 8 + 1 + 4 + 32 + 24 + 16 { return Err(SecurityError::Crypto); }
    if &blob[..8] != BACKUP_MAGIC { return Err(SecurityError::Crypto); }
    if blob[8] != BACKUP_VERSION { return Err(SecurityError::Crypto); }
    let m = u32::from_be_bytes(blob[9..13].try_into().unwrap());
    let salt: [u8; 32] = blob[13..45].try_into().unwrap();
    let nonce: [u8; 24] = blob[45..69].try_into().unwrap();
    let ct = &blob[69..];
    let params = Params::new(m, 3, 1, Some(32)).map_err(|_| SecurityError::Crypto)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon.hash_password_into(passphrase.as_bytes(), &salt, key.as_mut_slice()).map_err(|_| SecurityError::Crypto)?;
    open_seal(key.as_slice().try_into().unwrap(), &nonce, ct)
}

fn open_seal(key: &[u8; 32], nonce: &[u8; 24], ct: &[u8]) -> Result<Vec<u8>> {
    XChaCha20Poly1305::new(key.into())
        .decrypt(XNonce::from_slice(nonce), ct)
        .map_err(|_| SecurityError::Crypto)
}

fn hmac_of(header: &Header, device: &[u8; 32]) -> Result<[u8; 32]> {
    let bytes = bincode::serialize(header)?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(device).map_err(|_| SecurityError::Crypto)?;
    mac.update(&bytes);
    Ok(mac.finalize().into_bytes().into())
}

fn write_vault(path: &Path, header: &Header, device: &[u8; 32]) -> Result<()> {
    let hmac = hmac_of(header, device)?;
    let file = VaultFile { header: header.clone(), hmac };
    let bytes = bincode::serialize(&file)?;
    let tmp = path.with_extension("tmp");
    let mut f = OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    drop(f);
    fs::rename(tmp, path)?;
    Ok(())
}

fn read_vault(path: &Path, device: &[u8; 32]) -> Result<Header> {
    let mut f = File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let file: VaultFile = bincode::deserialize(&buf)?;
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

const DEVICE_FILE: &str = ".device";

impl DeviceBind {
    pub fn ensure(dir: &Path) -> Result<[u8; 32]> {
        let path = dir.join(DEVICE_FILE);
        if path.exists() {
            let mut f = File::open(&path)?;
            let mut s = String::new();
            f.read_to_string(&mut s)?;
            let bytes = hex_decode(s.trim()).ok_or(SecurityError::Keystore)?;
            if bytes.len() != 32 { return Err(SecurityError::Keystore); }
            let mut out = [0u8; 32]; out.copy_from_slice(&bytes);
            Ok(out)
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

    pub fn destroy(dir: &Path) -> Result<()> {
        let path = dir.join(DEVICE_FILE);
        if path.exists() {
            let _ = overwrite_file(&path);
            let _ = fs::remove_file(&path);
        }
        Ok(())
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

pub fn harden_process() {
    #[cfg(target_os = "linux")]
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0); }
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX,
        };
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    }
}

pub fn mlock_region(ptr: *const u8, len: usize) -> bool {
    #[cfg(unix)]
    unsafe { libc::mlock(ptr as _, len) == 0 }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Memory::VirtualLock;
        VirtualLock(ptr as _, len) != 0
    }
}