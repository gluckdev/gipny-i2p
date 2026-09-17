//! Auto-update: check GitHub Releases for a newer `gluckdev/gipny-i2p` tag,
//! download the asset for this platform, and install it without disrupting
//! the session that is running — the new build takes effect the *next* time
//! this process starts, never this one.
//!
//! Reached through the local i2pd HTTP proxy with an outproxy
//! (`libcore::router::DEFAULT_OUTPROXY`), never straight over clearnet: this
//! is an i2p-first app, and a direct HTTPS call to GitHub at every launch
//! would tell GitHub (and anyone watching that link) which real IP runs
//! gipny. The outproxy only ever sees an encrypted CONNECT to
//! `api.github.com` / `objects.githubusercontent.com`, never content — TLS is
//! still end to end to GitHub, which is also why the asset's SHA256 (from the
//! same release's `SHA256SUMS.txt`) is only a defense-in-depth check here, not
//! the source of trust the old signed-manifest design
//! (superseded; see git history) needed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::net::TorNode;

/// owner/repo on GitHub whose releases are checked.
pub const GITHUB_REPO: &str = "gluckdev/gipny-i2p";

/// Refuse anything absurdly large rather than trust a response's declared
/// size unconditionally.
const MAX_ARTIFACT_BYTES: u64 = 1024 * 1024 * 1024;

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
    #[error("http: {0}")] Http(#[from] reqwest::Error),
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("bad json: {0}")] Json(String),
    #[error("downloaded file does not match the release's SHA256SUMS.txt")] BadSha256,
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
    /// From the release's `SHA256SUMS.txt`, when that asset was found there.
    pub sha256: Option<String>,
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
    /// `None` when this router has no local HTTP proxy to use (Android, or
    /// attached to a router we don't own) — every method then reports
    /// unavailable rather than trying to dial nothing.
    client: Option<reqwest::Client>,
    component: Component,
}

/// `reqwest`'s `rustls-no-provider` feature means TLS has no default crypto
/// backend until one is installed process-wide; `ring` is the one this crate
/// depends on (see `Cargo.toml`). Installing it twice is a (harmless) error,
/// so this runs at most once even if more than one `Updater` is created (the
/// app and an in-process bot, say).
fn ensure_crypto_provider() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

impl Updater {
    pub fn new(node: Arc<TorNode>, component: Component) -> Self {
        ensure_crypto_provider();
        let client = node.http_proxy_port().and_then(|port| {
            reqwest::Client::builder()
                .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).ok()?)
                .timeout(Duration::from_secs(1800))
                .user_agent("gipny-i2p-updater")
                .build()
                .ok()
        });
        Self { client, component }
    }

    pub fn is_configured(&self) -> bool {
        self.client.is_some()
    }

    /// The latest release, every asset it has — for the app's Android-APK
    /// sideload picker, which is not about *this* platform at all.
    pub async fn latest_release(&self) -> Result<ReleaseInfo> {
        let client = self.client.as_ref().ok_or(UpdateError::NotConfigured)?;
        let text = client
            .get(format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest"))
            .header("Accept", "application/vnd.github+json")
            .send().await?
            .error_for_status()?
            .text().await?;
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
    /// an asset for this component on this platform/arch.
    pub async fn check(&self, current_version: &str) -> Result<Option<UpdateInfo>> {
        if !self.is_configured() { return Ok(None); }
        let release = self.latest_release().await?;
        if !version_newer(&release.version, current_version) { return Ok(None); }
        let Some((prefix, suffix)) = target_suffix(self.component) else { return Ok(None) };
        let Some(asset) = release.assets.iter().find(|a| a.name.starts_with(prefix) && a.name.ends_with(suffix)).cloned() else {
            return Ok(None);
        };
        let sha256 = find_sha256(self.client.as_ref().unwrap(), &release.assets, &asset.name).await;
        Ok(Some(UpdateInfo { version: release.version, notes: release.notes, asset, sha256 }))
    }

    /// Downloads `asset` into `dest_dir` (created if missing), verifying its
    /// SHA256 against `expected_sha256` when one is known. Streamed, so
    /// `on_progress(downloaded, total)` can drive a progress notice.
    pub async fn download_asset<F>(&self, asset: &ReleaseAsset, expected_sha256: Option<&str>, dest_dir: &Path, on_progress: F) -> Result<PathBuf>
    where F: FnMut(u64, u64) + Send,
    {
        std::fs::create_dir_all(dest_dir)?;
        let dest = dest_dir.join(&asset.name);
        self.download_asset_to(asset, expected_sha256, &dest, on_progress).await?;
        Ok(dest)
    }

    /// As [`Self::download_asset`], but to an exact path instead of a
    /// directory — for the Android APK sideload picker, which already has a
    /// destination from the user's own save dialog.
    pub async fn download_asset_to<F>(&self, asset: &ReleaseAsset, expected_sha256: Option<&str>, dest: &Path, mut on_progress: F) -> Result<()>
    where F: FnMut(u64, u64) + Send,
    {
        if asset.size > MAX_ARTIFACT_BYTES { return Err(UpdateError::TooLarge); }
        let client = self.client.as_ref().ok_or(UpdateError::NotConfigured)?;
        if let Some(parent) = dest.parent() { std::fs::create_dir_all(parent)?; }
        let partial = dest.with_extension(format!(
            "{}.partial", dest.extension().and_then(|s| s.to_str()).unwrap_or("")
        ));

        let mut resp = client.get(&asset.download_url).send().await?.error_for_status()?;
        let mut file = std::fs::File::create(&partial)?;
        let mut hasher = Sha256::new();
        let mut total: u64 = 0;
        use std::io::Write;
        while let Some(chunk) = resp.chunk().await? {
            hasher.update(&chunk);
            file.write_all(&chunk)?;
            total += chunk.len() as u64;
            on_progress(total, asset.size);
            if total > MAX_ARTIFACT_BYTES {
                drop(file);
                let _ = std::fs::remove_file(&partial);
                return Err(UpdateError::TooLarge);
            }
        }
        drop(file);

        if let Some(expected) = expected_sha256 {
            let got = hex_encode(&hasher.finalize());
            if got != expected {
                let _ = std::fs::remove_file(&partial);
                return Err(UpdateError::BadSha256);
            }
        }
        let _ = std::fs::remove_file(dest);
        std::fs::rename(&partial, dest)?;
        Ok(())
    }

    /// Convenience over [`Self::download_asset`] for the update this
    /// component is actually installing.
    pub async fn download<F>(&self, info: &UpdateInfo, dest_dir: &Path, on_progress: F) -> Result<PathBuf>
    where F: FnMut(u64, u64) + Send,
    {
        self.download_asset(&info.asset, info.sha256.as_deref(), dest_dir, on_progress).await
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

/// `SHA256SUMS.txt` is `sha256sum -- *` output: `<hex>  <filename>` per line
/// (sometimes `*filename` in binary mode). Best-effort — a miss just means no
/// extra check on top of TLS, not a failure.
async fn find_sha256(client: &reqwest::Client, assets: &[ReleaseAsset], asset_name: &str) -> Option<String> {
    let sums = assets.iter().find(|a| a.name == "SHA256SUMS.txt")?;
    let text = client.get(&sums.download_url).send().await.ok()?.error_for_status().ok()?.text().await.ok()?;
    text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset_name).then(|| hash.to_string())
    })
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

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes { s.push_str(&format!("{:02x}", b)); }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

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
