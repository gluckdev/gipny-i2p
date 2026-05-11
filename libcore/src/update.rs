use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::net::{NetError, TorNode};

pub const DEFAULT_UPDATE_ONION: &str = "zcchjuutlm3ukwgx2yc43k43muq2fw5sxwr2bfpznhrazkztetqzy4ad.onion";

const UPDATE_PORT: u16 = 80;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CHUNK_FRAME: usize = 2 * 1024 * 1024;

const UPDATE_VERIFY_KEY: [u8; 32] = [
    0x69, 0xd0, 0x0c, 0x18, 0xf5, 0x97, 0x38, 0x65,
    0xf3, 0xe0, 0xb0, 0xf3, 0x78, 0x31, 0x39, 0x8e,
    0x6f, 0x38, 0x8e, 0x97, 0xcf, 0xe4, 0x85, 0x4c,
    0xde, 0x6e, 0x07, 0xee, 0x06, 0xbf, 0xfe, 0xd5,
];

pub type Result<T> = std::result::Result<T, UpdateError>;

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("net")] Net(#[from] NetError),
    #[error("io")] Io(#[from] std::io::Error),
    #[error("codec")] Codec,
    #[error("bad signature")] BadSignature,
    #[error("bad sha256")] BadSha256,
    #[error("protocol: {0}")] Protocol(String),
    #[error("no artifact for target {0}")] NoArtifact(String),
    #[error("artifact too large")] TooLarge,
    #[error("server: {0}")] Server(String),
    #[error("unsupported: {0}")] Unsupported(String),
}

impl From<bincode::Error> for UpdateError { fn from(_: bincode::Error) -> Self { Self::Codec } }

#[derive(Serialize, Deserialize, Debug, Clone)]
enum Req {
    GetManifest,
    GetArtifact { path: String },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
enum Resp {
    Manifest { json: Vec<u8>, sig: Vec<u8> },
    Chunk { data: Vec<u8>, eof: bool },
    Error(String),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Artifact {
    pub file: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Manifest {
    pub version: String,
    pub notes: String,
    pub artifacts: BTreeMap<String, Artifact>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub target_key: String,
    pub artifact: Artifact,
}

pub struct Updater {
    node: Arc<TorNode>,
    onion: String,
}

impl Updater {
    pub fn new(node: Arc<TorNode>) -> Self {
        Self { node, onion: DEFAULT_UPDATE_ONION.to_string() }
    }

    pub fn with_onion(node: Arc<TorNode>, onion: impl Into<String>) -> Self {
        Self { node, onion: onion.into() }
    }

    pub async fn check(&self, current_version: &str) -> Result<Option<UpdateInfo>> {
        let m = self.fetch_manifest().await?;
        if !version_newer(&m.version, current_version) { return Ok(None); }
        let target_key = detect_target();
        let artifact = m.artifacts.get(&target_key)
            .ok_or_else(|| UpdateError::NoArtifact(target_key.clone()))?
            .clone();
        Ok(Some(UpdateInfo { version: m.version, notes: m.notes, target_key, artifact }))
    }

    pub async fn manifest(&self) -> Result<Manifest> {
        self.fetch_manifest().await
    }

    pub async fn download<F>(&self, info: &UpdateInfo, on_progress: F) -> Result<PathBuf>
    where F: FnMut(u64, u64) + Send,
    {
        let dl_dir = download_dir();
        std::fs::create_dir_all(&dl_dir)?;
        let fname = info.artifact.file.rsplit('/').next().unwrap_or("update.bin");
        let dest = dl_dir.join(fname);
        self.download_artifact_to(&info.artifact, &dest, on_progress).await?;
        Ok(dest)
    }

    pub async fn download_artifact_to<F>(&self, artifact: &Artifact, dest: &Path, mut on_progress: F) -> Result<()>
    where F: FnMut(u64, u64) + Send,
    {
        if artifact.size > MAX_ARTIFACT_BYTES { return Err(UpdateError::TooLarge); }
        let relay = self.node.connect_relay(&self.onion, UPDATE_PORT).await?;
        let mut stream = relay.into_inner();
        write_frame(&mut stream, &bincode::serialize(&Req::GetArtifact {
            path: artifact.file.clone(),
        })?).await?;

        if let Some(parent) = dest.parent() { std::fs::create_dir_all(parent)?; }
        let partial = dest.with_extension(
            format!("{}.partial",
                dest.extension().and_then(|s| s.to_str()).unwrap_or(""))
        );

        use std::io::Write;
        let mut file = std::fs::File::create(&partial)?;
        let mut hasher = Sha256::new();
        let mut total: u64 = 0;

        loop {
            let frame = read_frame(&mut stream, MAX_CHUNK_FRAME).await?;
            let resp: Resp = bincode::deserialize(&frame)?;
            match resp {
                Resp::Chunk { data, eof } => {
                    hasher.update(&data);
                    file.write_all(&data)?;
                    total += data.len() as u64;
                    on_progress(total, artifact.size);
                    if total > artifact.size + 1024 {
                        let _ = std::fs::remove_file(&partial);
                        return Err(UpdateError::TooLarge);
                    }
                    if eof { break; }
                }
                Resp::Error(e) => {
                    let _ = std::fs::remove_file(&partial);
                    return Err(UpdateError::Server(e));
                }
                _ => {
                    let _ = std::fs::remove_file(&partial);
                    return Err(UpdateError::Protocol("unexpected".into()));
                }
            }
        }
        drop(file);

        let got = hex_encode(&hasher.finalize());
        if got != artifact.sha256 {
            let _ = std::fs::remove_file(&partial);
            return Err(UpdateError::BadSha256);
        }
        let _ = std::fs::remove_file(dest);
        std::fs::rename(&partial, dest)?;
        Ok(())
    }

    pub fn install_and_respawn(&self, downloaded: &Path, target_key: &str) -> Result<()> {
        InstallStrategy::for_target(target_key).install(downloaded)
    }

    async fn fetch_manifest(&self) -> Result<Manifest> {
        let relay = self.node.connect_relay(&self.onion, UPDATE_PORT).await?;
        let mut stream = relay.into_inner();
        write_frame(&mut stream, &bincode::serialize(&Req::GetManifest)?).await?;
        let frame = read_frame(&mut stream, MAX_MANIFEST_BYTES).await?;
        let resp: Resp = bincode::deserialize(&frame)?;
        let (json, sig) = match resp {
            Resp::Manifest { json, sig } => (json, sig),
            Resp::Error(e) => return Err(UpdateError::Server(e)),
            _ => return Err(UpdateError::Protocol("expected manifest".into())),
        };
        verify_sig(&json, &sig)?;
        serde_json::from_slice(&json).map_err(|_| UpdateError::Codec)
    }
}

fn verify_sig(json: &[u8], sig: &[u8]) -> Result<()> {
    if sig.len() != 64 { return Err(UpdateError::BadSignature); }
    let mut arr = [0u8; 64];
    arr.copy_from_slice(sig);
    let signature = Signature::from_bytes(&arr);
    let vk = VerifyingKey::from_bytes(&UPDATE_VERIFY_KEY).map_err(|_| UpdateError::BadSignature)?;
    vk.verify(json, &signature).map_err(|_| UpdateError::BadSignature)
}

fn version_newer(candidate: &str, current: &str) -> bool {
    let c: Vec<u64> = candidate.split('.').filter_map(|s| s.parse().ok()).collect();
    let n: Vec<u64> = current.split('.').filter_map(|s| s.parse().ok()).collect();
    for i in 0..c.len().max(n.len()) {
        let a = c.get(i).copied().unwrap_or(0);
        let b = n.get(i).copied().unwrap_or(0);
        if a > b { return true; }
        if a < b { return false; }
    }
    false
}

pub fn detect_target() -> String {
    let arch = std::env::consts::ARCH;
    let (os, kind) = if cfg!(target_os = "linux") {
        let kind = if std::env::var("APPIMAGE").is_ok() { "appimage" }
            else if Path::new("/var/lib/dpkg/info/gipny.list").exists() { "deb" }
            else { "targz" };
        ("linux", kind)
    } else if cfg!(target_os = "windows") { ("windows", "exe") }
    else if cfg!(target_os = "macos") { ("macos", "dmg") }
    else { ("unknown", "unknown") };
    format!("{}-{}-{}", os, kind, arch)
}

fn download_dir() -> PathBuf {
    if let Ok(home) = env::var("HOME") {
        let d = PathBuf::from(&home).join("Downloads");
        if d.exists() { return d; }
        return PathBuf::from(home);
    }
    if let Ok(up) = env::var("USERPROFILE") {
        let d = PathBuf::from(&up).join("Downloads");
        if d.exists() { return d; }
        return PathBuf::from(up);
    }
    env::temp_dir()
}

enum InstallStrategy {
    AppImageReplace,
    WindowsInstaller,
    Manual,
}

impl InstallStrategy {
    fn for_target(t: &str) -> Self {
        if t.starts_with("linux-appimage") { Self::AppImageReplace }
        else if t.starts_with("windows-exe") { Self::WindowsInstaller }
        else { Self::Manual }
    }

    fn install(&self, src: &Path) -> Result<()> {
        match self {
            Self::AppImageReplace => install_appimage(src),
            Self::WindowsInstaller => install_windows(src),
            Self::Manual => Err(UpdateError::Unsupported(
                format!("automatic install not supported for this platform; file saved to {}", src.display())
            )),
        }
    }
}

#[cfg(target_os = "linux")]
fn install_appimage(src: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let current = env::var("APPIMAGE").map(PathBuf::from).or_else(|_| env::current_exe())?;
    let parent = current.parent().ok_or_else(|| UpdateError::Io(std::io::Error::new(std::io::ErrorKind::Other, "no parent")))?;
    let new_path = parent.join(format!(".{}.new", current.file_name().unwrap_or_default().to_string_lossy()));
    std::fs::copy(src, &new_path)?;
    let mut perms = std::fs::metadata(&new_path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&new_path, perms)?;
    std::fs::rename(&new_path, &current)?;
    std::process::Command::new(&current).spawn()?;
    std::process::exit(0);
}

#[cfg(not(target_os = "linux"))]
fn install_appimage(_: &Path) -> Result<()> {
    Err(UpdateError::Unsupported("AppImage only on linux".into()))
}

#[cfg(target_os = "windows")]
fn install_windows(src: &Path) -> Result<()> {
    std::process::Command::new(src)
        .args(["/SILENT", "/NORESTART", "/CLOSEAPPLICATIONS"])
        .spawn()?;
    std::process::exit(0);
}

#[cfg(not(target_os = "windows"))]
fn install_windows(_: &Path) -> Result<()> {
    Err(UpdateError::Unsupported("installer only on windows".into()))
}

async fn write_frame<S: AsyncWriteExt + Unpin>(stream: &mut S, data: &[u8]) -> Result<()> {
    stream.write_all(&(data.len() as u32).to_be_bytes()).await?;
    stream.write_all(data).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_frame<S: AsyncReadExt + Unpin>(stream: &mut S, max: usize) -> Result<Vec<u8>> {
    let mut lenb = [0u8; 4];
    stream.read_exact(&mut lenb).await?;
    let len = u32::from_be_bytes(lenb) as usize;
    if len > max { return Err(UpdateError::Protocol("frame too large".into())); }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes { s.push_str(&format!("{:02x}", b)); }
    s
}