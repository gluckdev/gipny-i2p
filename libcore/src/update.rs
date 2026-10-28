//! Auto-update: check GitHub Releases for a newer `gluckdev/gipny-i2p` tag,
//! download the asset for this platform, and install it without disrupting
//! the session that is running — the new build takes effect the *next* time
//! this process starts, never this one.
//!
//! Reached over our own i2p stream to an outproxy
//! (`libcore::i2p_http::OUTPROXY`), never straight over clearnet: this
//! is an i2p-first app, and a direct HTTPS call to GitHub at every launch
//! would tell GitHub (and anyone watching that link) which real IP runs
//! gipny. The outproxy sees the GitHub host requested by CONNECT, but not the
//! HTTPS content: TLS still ends at GitHub. The asset's SHA256 (from the
//! same release's `SHA256SUMS.txt`) is a mandatory integrity check, not an
//! independent release signature. Missing or invalid checksums stop the
//! update; HTTPS and the release publisher remain the source of trust.

use std::path::{Path, PathBuf};
use std::sync::Arc;


use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::net::TorNode;

/// owner/repo on GitHub whose releases are checked.
pub const GITHUB_REPO: &str = "gluckdev/gipny-i2p";

/// Refuse anything absurdly large rather than trust a response's declared
/// size unconditionally.
const MAX_ARTIFACT_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_RELEASE_BYTES: usize = 4 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const CHECKSUM_MANIFEST: &str = "SHA256SUMS.txt";

/// First check this long after start; then every `UPDATE_CHECK_INTERVAL_SECS`.
/// Shared by the app and `gipny-agent` so both poll on the same cadence.
pub const UPDATE_CHECK_INITIAL_SECS: u64 = 30;
pub const UPDATE_CHECK_INTERVAL_SECS: u64 = 6 * 3600;

/// Where a Windows install stages the downloaded installer to run at the next
/// launch (see `apply_staged_windows_installer`).
const PENDING_DIR: &str = "pending_update";
const PENDING_INSTALLER: &str = "installer.exe";

pub type Result<T> = std::result::Result<T, UpdateError>;

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("{0}")] I2pHttp(#[from] crate::i2p_http::HttpError),
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("bad json: {0}")] Json(String),
    #[error("downloaded file does not match the release's SHA256SUMS.txt")] BadSha256,
    #[error("release has no SHA256SUMS.txt")] MissingChecksumManifest,
    #[error("release has multiple SHA256SUMS.txt assets")] AmbiguousChecksumManifest,
    #[error("SHA256SUMS.txt has no checksum for {0}")] MissingChecksum(String),
    #[error("invalid SHA256SUMS.txt line {0}")] InvalidChecksumManifest(usize),
    #[error("SHA256SUMS.txt has multiple entries for {0}")] AmbiguousChecksum(String),
    #[error("download size {actual} differs from release size {expected}")] BadSize { expected: u64, actual: u64 },
    #[error("artifact too large")] TooLarge,
    #[error("unsupported: {0}")] Unsupported(String),
    #[error("update checking is unavailable this run (no local HTTP proxy)")] NotConfigured,
}

/// Which binary is asking — decides which release asset it looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    /// The desktop app (`gipny-i2p_<ver>_...`).
    App,
    /// `gipny-agent` (`gipny-agent_<ver>_...`).
    Agent,
}

/// What happened to a downloaded, verified update once handed to
/// [`Updater::install`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Replaced in place already — safe because nothing but the file on disk
    /// changed; this process keeps running the code it already loaded.
    InstalledNow,
    /// Staged; applied at the very start of the next launch
    /// ([`apply_staged_windows_installer`]), because this platform cannot
    /// overwrite a running executable.
    StagedForNextLaunch,
    /// Downloaded, but this build has no way to install it automatically
    /// (macOS, .deb, Android). The message says where the file is.
    Unsupported(String),
}

#[derive(Debug, Clone)]
pub struct ReleaseAsset {
    pub name: String,
    pub download_url: String,
    pub size: u64,
}

/// The latest release, unfiltered — every asset on the page.
#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub version: String,
    pub notes: String,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub asset: ReleaseAsset,
    /// Required, validated entry from this release's `SHA256SUMS.txt`.
    pub sha256: Sha256Digest,
}

/// A validated SHA256 from a release manifest. Download APIs require this
/// value so callers cannot accidentally request an unchecked executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    fn parse(hex: &[u8]) -> Option<Self> {
        if hex.len() != 64 || !hex.iter().all(u8::is_ascii_hexdigit) { return None; }
        let nibble = |b: u8| match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => unreachable!("validated hex"),
        };
        let mut bytes = [0; 32];
        for (i, pair) in hex.chunks_exact(2).enumerate() {
            bytes[i] = (nibble(pair[0]) << 4) | nibble(pair[1]);
        }
        Some(Self(bytes))
    }
}

impl std::fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 { write!(f, "{byte:02x}")?; }
        Ok(())
    }
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    // GitHub sends this as JSON `null`, not just omits it, for a release with
    // no description — `#[serde(default)]` alone only covers an absent key.
    #[serde(default)]
    body: Option<String>,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize, Clone)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

pub struct Updater {
    /// Always there: our own i2p stream to the outproxy. An `Option` so the
    /// "not configured" outcomes keep one place to come from.
    client: Option<Transport>,
    component: Component,
}

/// rustls has no default crypto backend until one is installed process-wide
/// (no aws-lc here); `ring` is the one this crate
/// depends on (see `Cargo.toml`). Installing it twice is a (harmless) error,
/// so this runs at most once even if more than one `Updater` is created (the
/// app and an in-process bot, say).
fn ensure_crypto_provider() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// How the updater reaches GitHub: our own i2p stream to the outproxy, then
/// TLS to GitHub inside it (`i2p_http`). No proxy, no port.
struct Transport {
    node: Arc<TorNode>,
    tls: Arc<rustls::ClientConfig>,
}

impl Transport {
    async fn text(&self, url: &str, accept: Option<&str>, max_bytes: usize) -> Result<String> {
        Ok(crate::i2p_http::get(&self.node, &self.tls, url, accept).await?.text(max_bytes).await?)
    }

    async fn body(&self, url: &str) -> Result<crate::i2p_http::Body> {
        Ok(crate::i2p_http::get(&self.node, &self.tls, url, None).await?)
    }
}

/// TLS that trusts Mozilla's root list and nothing else.
///
/// Not the platform verifier: `rustls-platform-verifier` on Android calls
/// back into Java and aborts the whole process if it was never
/// handed a JNI environment — `Expect rustls-platform-verifier to be
/// initialized`, SIGABRT on a tokio worker, the app gone mid-sentence. It is
/// reached the moment the update check opens its first connection, which is why
/// the phone died on a timer rather than on anything the user did.
///
/// Pinning the bundled roots is also the better answer than initialising the
/// platform verifier. This client talks to exactly one host, GitHub, through an
/// i2p outproxy, and the device trust store is not ours: a CA added by an
/// employer's MDM, by a user who tapped through a warning, or by malware would
/// otherwise be trusted to sign the release we are about to install.
fn webpki_tls() -> rustls::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// What a check found, with the four "nothing to do" cases kept apart so the
/// interface can say which one it is.
#[derive(Debug, Clone)]
pub enum CheckOutcome {
    Update(UpdateInfo),
    /// No local HTTP proxy this run: there is no way to ask at all.
    NotConfigured,
    UpToDate { latest: String },
    /// A newer version was already offered and the person chose «Позже».
    /// Not the same as being current, and must not be reported as it.
    Dismissed { version: String },
    /// A newer version exists, but this install (macOS, a dev run) has no
    /// file it could be updated from.
    UnsupportedInstall { latest: String },
    /// A newer version exists and this platform is supported, but that
    /// release does not carry the file — a build that did not finish, or a
    /// name that changed.
    NoAsset { latest: String, wanted: String },
}

impl Updater {
    pub fn new(node: Arc<TorNode>, component: Component) -> Self {
        ensure_crypto_provider();
        let client = Some(Transport { node, tls: Arc::new(webpki_tls()) });
        Self { client, component }
    }

    pub fn is_configured(&self) -> bool {
        self.client.is_some()
    }

    /// The latest release, every asset it has — for the app's Android-APK
    /// sideload picker, which is not about *this* platform at all.
    pub async fn latest_release(&self) -> Result<ReleaseInfo> {
        let client = self.client.as_ref().ok_or(UpdateError::NotConfigured)?;
        let text = client.text(
            &format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest"),
            Some("application/vnd.github+json"),
            MAX_RELEASE_BYTES,
        ).await?;
        let release: GhRelease = serde_json::from_str(&text).map_err(|e| UpdateError::Json(e.to_string()))?;
        let version = release.tag_name.strip_prefix('v').unwrap_or(&release.tag_name).to_string();
        Ok(ReleaseInfo {
            version,
            notes: release.body.unwrap_or_default(),
            assets: release.assets.into_iter()
                .map(|a| ReleaseAsset { name: a.name, download_url: a.browser_download_url, size: a.size })
                .collect(),
        })
    }

    /// `Some` when the latest release is newer than `current_version` and has
    /// an asset for this component on this platform/arch. The background loop
    /// uses this; anything a person pressed a button for wants
    /// [`Self::check_detailed`], which can say *why* there is nothing.
    pub async fn check(&self, current_version: &str) -> Result<Option<UpdateInfo>> {
        Ok(match self.check_detailed(current_version).await? {
            CheckOutcome::Update(info) => Some(info),
            _ => None,
        })
    }

    /// The same check, with its reasons kept apart.
    ///
    /// Four different things used to come back as "no update": auto-update is
    /// not available at all, you are on the newest version, this kind of
    /// install cannot be updated from inside the app, and the release has no
    /// file for this platform. The interface showed one sentence — «Установлена
    /// последняя версия» — for all four, which is how a 0.4.5 deb spent three
    /// releases telling its owner it was current.
    pub async fn check_detailed(&self, current_version: &str) -> Result<CheckOutcome> {
        if !self.is_configured() { return Ok(CheckOutcome::NotConfigured); }
        let release = self.latest_release().await?;
        if !version_newer(&release.version, current_version) {
            return Ok(CheckOutcome::UpToDate { latest: release.version });
        }
        let Some((prefix, suffix)) = target_suffix(self.component) else {
            return Ok(CheckOutcome::UnsupportedInstall { latest: release.version });
        };
        let Some(asset) = release.assets.iter().find(|a| a.name.starts_with(prefix) && a.name.ends_with(suffix)).cloned() else {
            return Ok(CheckOutcome::NoAsset { latest: release.version, wanted: format!("{prefix}<версия>{suffix}") });
        };
        let sha256 = self.asset_sha256(&release, &asset.name).await?;
        Ok(CheckOutcome::Update(UpdateInfo { version: release.version, notes: release.notes, asset, sha256 }))
    }

    /// Resolve a required checksum from the same release as the selected
    /// asset. Used by the APK picker as well as automatic update checks.
    pub async fn asset_sha256(&self, release: &ReleaseInfo, asset_name: &str) -> Result<Sha256Digest> {
        let client = self.client.as_ref().ok_or(UpdateError::NotConfigured)?;
        find_sha256(&release.assets, asset_name, |url| async move {
            client.text(&url, None, MAX_MANIFEST_BYTES).await
        }).await
    }

    /// Downloads `asset` into `dest_dir` (created if missing), requiring its
    /// checksum from the same release. Streamed, so
    /// `on_progress(downloaded, total)` can drive a progress notice.
    pub async fn download_asset<F>(&self, asset: &ReleaseAsset, expected_sha256: &Sha256Digest, dest_dir: &Path, on_progress: F) -> Result<PathBuf>
    where F: FnMut(u64, u64) + Send,
    {
        let dest = dest_dir.join(&asset.name);
        self.download_asset_to(asset, expected_sha256, &dest, on_progress).await?;
        Ok(dest)
    }

    /// As [`Self::download_asset`], but to an exact path instead of a
    /// directory — for the Android APK sideload picker, which already has a
    /// destination from the user's own save dialog.
    pub async fn download_asset_to<F>(&self, asset: &ReleaseAsset, expected_sha256: &Sha256Digest, dest: &Path, on_progress: F) -> Result<()>
    where F: FnMut(u64, u64) + Send,
    {
        if asset.size > MAX_ARTIFACT_BYTES { return Err(UpdateError::TooLarge); }
        let client = self.client.as_ref().ok_or(UpdateError::NotConfigured)?;
        let resp = client.body(&asset.download_url).await?;
        save_verified_asset(resp, asset, expected_sha256, dest, on_progress).await
    }

    /// Convenience over [`Self::download_asset`] for the update this
    /// component is actually installing.
    pub async fn download<F>(&self, info: &UpdateInfo, dest_dir: &Path, on_progress: F) -> Result<PathBuf>
    where F: FnMut(u64, u64) + Send,
    {
        self.download_asset(&info.asset, &info.sha256, dest_dir, on_progress).await
    }

    /// Installs a verified download. `data_dir` is only used to stage a
    /// Windows installer for the next launch; Linux installs are in-place and
    /// immediate. `agent_exe` is the agent's own binary path, resolved once by
    /// the caller before its first install this run — required for
    /// `Component::Agent`, ignored for `Component::App` (which finds its own
    /// path, preferring `$APPIMAGE` over `current_exe()`; see
    /// `install_appimage_now`). Not `self.component`-derived internally
    /// because a second install within one long-running agent process would
    /// otherwise resolve `current_exe()` again — by then a path to a deleted,
    /// already-replaced file.
    pub fn install(&self, downloaded: &Path, data_dir: &Path, agent_exe: Option<&Path>) -> Result<InstallOutcome> {
        match self.component {
            Component::App => {
                if cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some() {
                    install_appimage_now(downloaded)?;
                    Ok(InstallOutcome::InstalledNow)
                } else if is_deb_install() {
                    install_deb_now(downloaded)?;
                    Ok(InstallOutcome::StagedForNextLaunch)
                } else if cfg!(target_os = "windows") {
                    stage_windows_installer(downloaded, data_dir)?;
                    Ok(InstallOutcome::StagedForNextLaunch)
                } else {
                    Ok(InstallOutcome::Unsupported(format!("downloaded to {}", downloaded.display())))
                }
            }
            Component::Agent => {
                if cfg!(target_os = "linux") {
                    let exe = agent_exe.ok_or_else(|| UpdateError::Unsupported("no resolved agent binary path given".into()))?;
                    install_agent_binary_now(downloaded, exe)?;
                    Ok(InstallOutcome::InstalledNow)
                } else {
                    Ok(InstallOutcome::Unsupported(format!("downloaded to {}", downloaded.display())))
                }
            }
        }
    }
}

/// Are we running from a distribution package rather than an AppImage?
///
/// The binary then lives under /usr and belongs to dpkg, so replacing files by
/// hand would leave the package database lying about what is installed. The
/// update is handed to the package manager instead (`install_deb_now`).
#[cfg(target_os = "linux")]
pub fn is_deb_install() -> bool {
    if std::env::var_os("APPIMAGE").is_some() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    exe.starts_with("/usr/") && Path::new("/usr/lib/gipny-i2p").exists()
}
#[cfg(not(target_os = "linux"))]
pub fn is_deb_install() -> bool {
    false
}

/// Install a downloaded .deb through the package manager, asking for the
/// administrator password with the desktop's own dialog (`pkexec`).
///
/// dpkg needs root, and a messenger has no business holding root itself — so
/// this is the one update path that asks a question. It returns as soon as the
/// package is in; like every other platform, the new version runs at the next
/// launch.
#[cfg(target_os = "linux")]
fn install_deb_now(downloaded: &Path) -> Result<()> {
    // A .deb must keep its name: apt refuses a path that does not look like a
    // package, and the downloaded file is already named by the release.
    let path = downloaded.to_string_lossy().to_string();
    let out = std::process::Command::new("pkexec")
        .arg("apt-get")
        .arg("install")
        .arg("-y")
        .arg("--reinstall")
        .arg("--allow-downgrades")
        .arg(&path)
        .output()
        .map_err(|e| UpdateError::Unsupported(format!("pkexec: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    // 126/127 are polkit's "refused"/"no agent": that is a person saying no,
    // not a broken update.
    let code = out.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(UpdateError::Unsupported(match code {
        126 => "установка отменена".into(),
        127 => "нет графического запроса пароля (polkit) — поставьте пакет вручную".into(),
        _ => format!("apt-get: {}", stderr.trim()),
    }))
}
#[cfg(not(target_os = "linux"))]
fn install_deb_now(_: &Path) -> Result<()> {
    Err(UpdateError::Unsupported("deb install only on linux".into()))
}

/// Which release assets belong to this component's platform: assets are named
/// `<prefix><version><suffix>` (e.g. `gipny-i2p_0.4.2_amd64.AppImage`), so
/// matching on prefix+suffix does not need to know the version. `None` means
/// this platform has no applicable asset at all (macOS, .deb, a dev run) —
/// auto-update quietly does nothing rather than guessing.
fn target_suffix(component: Component) -> Option<(&'static str, &'static str)> {
    let arch = std::env::consts::ARCH;
    match component {
        Component::App if cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some() => {
            Some(("gipny-i2p_", if arch == "aarch64" { "_aarch64.AppImage" } else { "_amd64.AppImage" }))
        }
        Component::App if cfg!(target_os = "windows") => Some(("gipny-i2p_", "_x64-setup.exe")),
        // Installed from a package: update with a package, through the package
        // manager — see `install_deb_now`.
        Component::App if is_deb_install() => {
            Some(("gipny-i2p_", if arch == "aarch64" { "_arm64.deb" } else { "_amd64.deb" }))
        }
        // Android can check and download over i2p, but installing an APK is
        // the system installer's business, not ours — see `install`.
        Component::App if cfg!(target_os = "android") => {
            Some(("gipny-i2p_", if arch == "aarch64" { "_android-arm64.apk" } else { "_android-armv7.apk" }))
        }
        Component::Agent if cfg!(target_os = "linux") => {
            Some(("gipny-agent_", if arch == "aarch64" { "_linux-arm64.tar.gz" } else { "_linux-amd64.tar.gz" }))
        }
        _ => None,
    }
}

/// Resolve the sole manifest before fetching it. Fetch/HTTP errors propagate;
/// an unavailable integrity check must never become an unchecked update.
async fn find_sha256<F, Fut>(assets: &[ReleaseAsset], asset_name: &str, fetch: F) -> Result<Sha256Digest>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    let mut manifests = assets.iter().filter(|a| a.name == CHECKSUM_MANIFEST);
    let sums = manifests.next().ok_or(UpdateError::MissingChecksumManifest)?;
    if manifests.next().is_some() { return Err(UpdateError::AmbiguousChecksumManifest); }
    if sums.size > MAX_MANIFEST_BYTES as u64 { return Err(UpdateError::TooLarge); }
    let text = fetch(sums.download_url.clone()).await?;
    parse_sha256_manifest(&text, asset_name)
}

/// Accept GNU sha256sum text/binary mode and CRLF. Every nonempty row must
/// have one valid 64-hex digest and one filename. Duplicate names, even with
/// equal digests, are ambiguous. Release filenames need no GNU escaping.
fn parse_sha256_manifest(text: &str, asset_name: &str) -> Result<Sha256Digest> {
    if text.len() > MAX_MANIFEST_BYTES { return Err(UpdateError::TooLarge); }
    let mut found = None;
    let mut names = std::collections::HashSet::new();
    for (line_index, line) in text.lines().enumerate() {
        if line.trim().is_empty() { continue; }
        let bytes = line.as_bytes();
        let bad_line = || UpdateError::InvalidChecksumManifest(line_index + 1);
        if bytes.len() < 67 || bytes[64] != b' ' || !matches!(bytes[65], b' ' | b'*') {
            return Err(bad_line());
        }
        let digest = Sha256Digest::parse(&bytes[..64]).ok_or_else(bad_line)?;
        // The validated ASCII prefix makes this a UTF-8 boundary.
        let name = &line[66..];
        if name.contains(['\0', '\r', '\n']) { return Err(bad_line()); }
        if !names.insert(name) { return Err(UpdateError::AmbiguousChecksum(name.into())); }
        if name == asset_name { found = Some(digest); }
    }
    found.ok_or_else(|| UpdateError::MissingChecksum(asset_name.into()))
}

/// Write only to a uniquely owned temporary file in the destination directory.
/// The guard cleans up on read/write errors and cancellation. Persist replaces
/// the destination only after both metadata size and checksum are verified.
async fn save_verified_asset<F>(
    mut resp: crate::i2p_http::Body, asset: &ReleaseAsset, expected: &Sha256Digest,
    dest: &Path, mut on_progress: F,
) -> Result<()>
where F: FnMut(u64, u64) + Send,
{
    if asset.size > MAX_ARTIFACT_BYTES { return Err(UpdateError::TooLarge); }
    let parent = dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut partial = tempfile::Builder::new().prefix(".gipny-update-").tempfile_in(parent)?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    use std::io::Write;
    while let Some(chunk) = resp.chunk().await? {
        total += chunk.len() as u64;
        if total > MAX_ARTIFACT_BYTES { return Err(UpdateError::TooLarge); }
        if total > asset.size { return Err(UpdateError::BadSize { expected: asset.size, actual: total }); }
        hasher.update(&chunk);
        partial.write_all(&chunk)?;
        on_progress(total, asset.size);
    }
    if total != asset.size { return Err(UpdateError::BadSize { expected: asset.size, actual: total }); }
    let got: [u8; 32] = hasher.finalize().into();
    if got != expected.0 { return Err(UpdateError::BadSha256); }
    partial.as_file().sync_all()?;
    partial.persist(dest).map_err(|e| e.error)?;
    Ok(())
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

/// Replaces the running AppImage's file in place. Safe on Linux even though
/// this process is currently executing from that path: the kernel keeps the
/// already-mapped old file alive for this process, and the rename is what the
/// *next* launch sees. No relaunch.
#[cfg(target_os = "linux")]
fn install_appimage_now(downloaded: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let current = std::env::var("APPIMAGE").map(PathBuf::from).or_else(|_| std::env::current_exe())?;
    let parent = current.parent().ok_or_else(|| UpdateError::Unsupported("no parent dir".into()))?;
    let tmp = parent.join(format!(".{}.new", current.file_name().unwrap_or_default().to_string_lossy()));
    std::fs::copy(downloaded, &tmp)?;
    let mut perms = std::fs::metadata(&tmp)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&tmp, perms)?;
    std::fs::rename(&tmp, &current)?;
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn install_appimage_now(_: &Path) -> Result<()> {
    Err(UpdateError::Unsupported("AppImage install only on linux".into()))
}

/// Extracts `gipny-agent` from the downloaded release tarball and replaces
/// `current`'s file in place — the same safe rename as the AppImage case.
/// Leaves the bundled `i2pd` in the tarball alone. Takes `current` from the
/// caller rather than resolving `current_exe()` itself: after a first replace
/// in a long-running process, `current_exe()` resolves to a deleted, already
/// gone path (the kernel-held old inode, not the new file at that path).
#[cfg(target_os = "linux")]
fn install_agent_binary_now(downloaded_tar_gz: &Path, current: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = current.with_extension("new");
    {
        let file = std::fs::File::open(downloaded_tar_gz)?;
        let gz = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(gz);
        let mut found = false;
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.into_owned();
            if path.file_name().and_then(|n| n.to_str()) == Some("gipny-agent") {
                let mut out = std::fs::File::create(&tmp)?;
                std::io::copy(&mut entry, &mut out)?;
                found = true;
                break;
            }
        }
        if !found {
            return Err(UpdateError::Unsupported("no gipny-agent binary in the downloaded archive".into()));
        }
    }
    let mut perms = std::fs::metadata(&tmp)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&tmp, perms)?;
    std::fs::rename(&tmp, current)?;
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn install_agent_binary_now(_: &Path, _: &Path) -> Result<()> {
    Err(UpdateError::Unsupported("agent tarball install only on linux".into()))
}

/// Stages the downloaded NSIS installer under `data_dir` for
/// [`apply_staged_windows_installer`] to run at the next launch. Windows
/// cannot overwrite this process's own running exe, so nothing happens now.
fn stage_windows_installer(downloaded: &Path, data_dir: &Path) -> Result<()> {
    let dir = data_dir.join(PENDING_DIR);
    std::fs::create_dir_all(&dir)?;
    std::fs::copy(downloaded, dir.join(PENDING_INSTALLER))?;
    Ok(())
}

/// Call as early as possible in a profile's own startup — `lib.rs`'s `boot()`
/// calls it before the vault unlock and before the ~1-3 min router wait, not
/// after either. If the previous launch staged an installer, this runs it and
/// exits; the app closes and reopens once, instead of the user sitting
/// through unlock and router start only to have it close anyway.
///
/// `/S /UPDATE /R`: Tauri's NSIS template checks `/R` in silent mode (there is
/// no UI toggle to ask) and restarts the app itself once install finishes;
/// `/UPDATE` skips re-creating shortcuts and touching app data. No manual
/// wait-then-relaunch needed. Compiles; not run against a real installer yet
/// — needs a Windows CI build to confirm `/R` actually reopens gipny.
#[cfg(target_os = "windows")]
pub fn apply_staged_windows_installer(data_dir: &Path) {
    let dir = data_dir.join(PENDING_DIR);
    let installer = dir.join(PENDING_INSTALLER);
    if !installer.exists() {
        return;
    }
    eprintln!("[update] installing the staged update before starting...");
    let _ = std::process::Command::new(&installer)
        .args(["/S", "/UPDATE", "/R", "/NCRC"])
        .spawn();
    let _ = std::fs::remove_dir_all(&dir);
    std::process::exit(0);
}
#[cfg(not(target_os = "windows"))]
pub fn apply_staged_windows_installer(_data_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn artifact(size: u64) -> ReleaseAsset {
        ReleaseAsset { name: "app.apk".into(), download_url: "https://fixture.invalid/app.apk".into(), size }
    }

    fn manifest() -> ReleaseAsset {
        ReleaseAsset { name: CHECKSUM_MANIFEST.into(), download_url: "https://fixture.invalid/SHA256SUMS.txt".into(), size: 100 }
    }

    fn abc_digest() -> Sha256Digest {
        Sha256Digest::parse(ABC_SHA256.as_bytes()).unwrap()
    }

    // Real hyper HTTP decoding over an in-memory socket: no listening port,
    // router, TLS substitution or production transport fallback is involved.
    async fn http_fixture(wire: Vec<u8>, stall: bool) -> (crate::i2p_http::Body, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (client, mut server) = tokio::io::duplex(1024);
        let writer = tokio::spawn(async move {
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let Ok(byte) = server.read_u8().await else { return };
                request.push(byte);
            }
            if server.write_all(&wire).await.is_ok() && stall {
                std::future::pending::<()>().await;
            }
        });
        let (mut sender, connection) = hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(client)).await.unwrap();
        tokio::spawn(async move { let _ = connection.await; });
        let request = hyper::Request::get("/").header(hyper::header::HOST, "fixture.invalid")
            .body(http_body_util::Empty::<bytes::Bytes>::new()).unwrap();
        let response = sender.send_request(request).await.unwrap();
        (crate::i2p_http::Body::new(response.into_body()), writer)
    }

    async fn body(wire: &[u8]) -> crate::i2p_http::Body {
        http_fixture(wire.to_vec(), false).await.0
    }

    fn assert_only_destination(dir: &Path, dest: &Path, contents: &[u8]) {
        assert_eq!(std::fs::read(dest).unwrap(), contents);
        assert_eq!(std::fs::read_dir(dir).unwrap().count(), 1, "no staging file should survive");
    }

    #[test]
    fn manifest_accepts_release_text_binary_uppercase_crlf_and_exact_names() {
        for separator in ["  ", " *"] {
            let text = format!("{}{}app.apk\r\n\r\n", ABC_SHA256.to_uppercase(), separator);
            assert_eq!(parse_sha256_manifest(&text, "app.apk").unwrap(), abc_digest());
        }
        let text = format!("{ABC_SHA256}  app with spaces.apk\n{ABC_SHA256}  other.apk\n");
        assert_eq!(parse_sha256_manifest(&text, "app with spaces.apk").unwrap(), abc_digest());
        assert!(matches!(parse_sha256_manifest(&text, "app"), Err(UpdateError::MissingChecksum(_))));
        assert_eq!(abc_digest().to_string(), ABC_SHA256);
    }

    #[test]
    fn missing_malformed_or_ambiguous_checksums_are_errors() {
        assert!(matches!(parse_sha256_manifest("", "app.apk"), Err(UpdateError::MissingChecksum(_))));
        let bad_rows = [
            format!("{}  app.apk", &ABC_SHA256[..63]),
            format!("{ABC_SHA256}0  app.apk"),
            format!("{}g  app.apk", &ABC_SHA256[..63]),
            format!("{ABC_SHA256} app.apk"),
            format!("{ABC_SHA256}\t app.apk"),
            format!("{ABC_SHA256}  "),
            format!("{ABC_SHA256}  app.apk\ninvalid unrelated row"),
        ];
        for text in bad_rows {
            assert!(matches!(parse_sha256_manifest(&text, "app.apk"), Err(UpdateError::InvalidChecksumManifest(_))), "{text:?}");
        }
        for second_hash in [ABC_SHA256.to_string(), "0".repeat(64)] {
            let text = format!("{ABC_SHA256}  app.apk\n{second_hash} *app.apk\n");
            assert!(matches!(parse_sha256_manifest(&text, "app.apk"), Err(UpdateError::AmbiguousChecksum(_))));
        }
        let oversized = " ".repeat(MAX_MANIFEST_BYTES + 1);
        assert!(matches!(parse_sha256_manifest(&oversized, "app.apk"), Err(UpdateError::TooLarge)));
    }

    #[tokio::test]
    async fn manifest_resolution_fails_before_fetch_when_missing_ambiguous_or_oversized() {
        let mut oversized = manifest();
        oversized.size = MAX_MANIFEST_BYTES as u64 + 1;
        let inputs = [vec![], vec![manifest(), manifest()], vec![oversized]];
        for (index, assets) in inputs.into_iter().enumerate() {
            let fetched = std::cell::Cell::new(false);
            let result = find_sha256(&assets, "app.apk", |_| {
                fetched.set(true);
                std::future::ready(Err(UpdateError::NotConfigured))
            }).await;
            assert!(!fetched.get());
            assert!(match index {
                0 => matches!(result, Err(UpdateError::MissingChecksumManifest)),
                1 => matches!(result, Err(UpdateError::AmbiguousChecksumManifest)),
                _ => matches!(result, Err(UpdateError::TooLarge)),
            });
        }
    }

    #[tokio::test]
    async fn manifest_fetch_failure_and_missing_entry_are_never_optional() {
        let assets = [manifest()];
        let failed = find_sha256(&assets, "app.apk", |url| async move {
            assert_eq!(url, "https://fixture.invalid/SHA256SUMS.txt");
            Err(UpdateError::I2pHttp(crate::i2p_http::HttpError::Status(404)))
        }).await;
        assert!(matches!(failed, Err(UpdateError::I2pHttp(crate::i2p_http::HttpError::Status(404)))));
        let missing = find_sha256(&assets, "app.apk", |_| async {
            Ok(format!("{ABC_SHA256}  other.apk\n"))
        }).await;
        assert!(matches!(missing, Err(UpdateError::MissingChecksum(_))));
        let valid = find_sha256(&assets, "app.apk", |_| async {
            Ok(format!("{ABC_SHA256}  app.apk\n"))
        }).await.unwrap();
        assert_eq!(valid, abc_digest());
    }

    #[tokio::test]
    async fn bounded_http_text_rejects_chunked_overflow_and_invalid_utf8() {
        let response = body(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n2\r\ncd\r\n0\r\n\r\n").await;
        assert!(matches!(response.text(3).await, Err(crate::i2p_http::HttpError::TooLarge(3))));
        let response = body(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n\xff").await;
        assert!(matches!(response.text(8).await, Err(crate::i2p_http::HttpError::Http(_))));
    }

    #[tokio::test]
    async fn verified_chunked_download_replaces_destination_only_after_integrity_check() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("app.apk");
        std::fs::write(&dest, b"previous update").unwrap();
        let response = body(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\na\r\n2\r\nbc\r\n0\r\n\r\n").await;
        let mut progress = Vec::new();
        save_verified_asset(response, &artifact(3), &abc_digest(), &dest, |done, total| {
            progress.push((done, total));
        }).await.unwrap();
        assert_eq!(progress.last(), Some(&(3, 3)));
        assert_only_destination(dir.path(), &dest, b"abc");
    }

    #[tokio::test]
    async fn checksum_mismatch_preserves_existing_destination_and_removes_partial() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("app.apk");
        std::fs::write(&dest, b"previous update").unwrap();
        let response = body(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabd").await;
        let error = save_verified_asset(response, &artifact(3), &abc_digest(), &dest, |_, _| {}).await.unwrap_err();
        assert!(matches!(error, UpdateError::BadSha256));
        assert_only_destination(dir.path(), &dest, b"previous update");
    }

    #[tokio::test]
    async fn complete_but_wrong_size_and_truncated_http_preserve_destination() {
        let cases: &[(u64, &[u8])] = &[
            (4, b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc"),
            (2, b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc"),
            (3, b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nab"),
        ];
        for (index, (size, wire)) in cases.iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let dest = dir.path().join("app.apk");
            std::fs::write(&dest, b"previous update").unwrap();
            let response = body(wire).await;
            let result = save_verified_asset(response, &artifact(*size), &abc_digest(), &dest, |_, _| {}).await;
            assert!(if index < 2 { matches!(result, Err(UpdateError::BadSize { .. })) }
                else { matches!(result, Err(UpdateError::I2pHttp(_))) });
            assert_only_destination(dir.path(), &dest, b"previous update");
        }
    }

    #[tokio::test]
    async fn oversized_release_asset_is_rejected_before_transport_or_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("absent").join("app.apk");
        let updater = Updater { client: None, component: Component::App };
        let result = updater.download_asset_to(&artifact(MAX_ARTIFACT_BYTES + 1), &abc_digest(), &dest, |_, _| {}).await;
        assert!(matches!(result, Err(UpdateError::TooLarge)));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn failed_destination_replace_cleans_its_owned_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("occupied");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("keep"), b"keep").unwrap();
        let response = body(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc").await;
        assert!(matches!(save_verified_asset(response, &artifact(3), &abc_digest(), &dest, |_, _| {}).await, Err(UpdateError::Io(_))));
        assert_eq!(std::fs::read(dest.join("keep")).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn cancelled_download_cleans_partial_and_preserves_destination() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("app.apk");
        std::fs::write(&dest, b"previous update").unwrap();
        let (response, writer) = http_fixture(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nabc".to_vec(), true).await;
        let path = dest.clone();
        let (progress, started) = tokio::sync::oneshot::channel();
        let mut progress = Some(progress);
        let download = tokio::spawn(async move {
            save_verified_asset(response, &artifact(6), &abc_digest(), &path, move |_, _| {
                if let Some(progress) = progress.take() { let _ = progress.send(()); }
            }).await
        });
        started.await.unwrap();
        download.abort();
        assert!(download.await.unwrap_err().is_cancelled());
        writer.abort();
        assert_only_destination(dir.path(), &dest, b"previous update");
    }

    #[test]
    fn a_v_prefixed_tag_compares_correctly() {
        // GitHub tags are v0.4.2; the old parser assumed a bare 0.4.2 and
        // would have silently dropped the "v0" component.
        assert!(version_newer("0.4.2", "0.4.1"));
        assert!(!version_newer("0.4.1", "0.4.2"));
        assert!(!version_newer("0.4.1", "0.4.1"));
    }

    #[test]
    fn app_target_needs_the_appimage_env_var_on_linux() {
        // cargo test does not run inside an AppImage, so a plain dev/CI
        // process on Linux resolves to None here, same as any platform this
        // has no asset for at all — not a guess at "probably AppImage".
        if cfg!(target_os = "linux") {
            assert!(std::env::var_os("APPIMAGE").is_none(), "test process should not look like an AppImage");
            assert!(target_suffix(Component::App).is_none());
        }
    }

    #[test]
    fn agent_target_is_always_known_on_linux() {
        if cfg!(target_os = "linux") {
            let expect_arm64 = std::env::consts::ARCH == "aarch64";
            assert_eq!(
                target_suffix(Component::Agent),
                Some(("gipny-agent_", if expect_arm64 { "_linux-arm64.tar.gz" } else { "_linux-amd64.tar.gz" })),
            );
        }
    }
}
