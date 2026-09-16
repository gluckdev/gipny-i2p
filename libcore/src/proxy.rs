//! sing-box proxy client lifecycle.
//!
//! Spawns `sing-box` as a child process configured to run the client tunnels
//! (OpenVPN, Hysteria 2, VLESS-Reality, etc.).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;

use crate::net::{NetError, Result};

const START_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_INTERVAL: Duration = Duration::from_millis(200);

/// Handle to a running sing-box proxy client instance.
pub struct ProxyHandle {
    child: Option<Child>,
    _config_path: PathBuf,
}

impl ProxyHandle {
    /// Spawn sing-box with the specified config file.
    ///
    /// If `bin` is `None`, resolves the binary automatically.
    /// If `probe_port` is provided, waits until a SOCKS5/HTTP socket on that port answers.
    pub async fn start(
        config_path: &Path,
        bin: Option<PathBuf>,
        probe_port: Option<u16>,
    ) -> Result<Self> {
        let bin = match bin {
            Some(b) => b,
            None => resolve_singbox_bin()?,
        };

        if !config_path.exists() {
            return Err(NetError::Proxy(format!(
                "config file does not exist: {}",
                config_path.display()
            )));
        }

        eprintln!(
            "[proxy] launching sing-box {} with config {}",
            bin.display(),
            config_path.display()
        );

        let mut cmd = Command::new(&bin);
        cmd.arg("run").arg("-c").arg(config_path);

        // Inherit or log stderr to capture startup logs, or Stdio::null()
        let child = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| NetError::Proxy(format!("spawn sing-box {}: {e}", bin.display())))?;

        let mut handle = Self {
            child: Some(child),
            _config_path: config_path.to_path_buf(),
        };

        if let Some(port) = probe_port {
            handle.await_ready(port).await?;
        }

        eprintln!("[proxy] sing-box is running");
        Ok(handle)
    }

    async fn await_ready(&mut self, port: u16) -> Result<()> {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                return Ok(());
            }
            if let Some(child) = self.child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(NetError::Proxy(format!("sing-box exited early: {status}")));
                }
            }
            if Instant::now() >= deadline {
                self.kill_child();
                return Err(NetError::Proxy("sing-box port did not answer in time".into()));
            }
            tokio::time::sleep(PROBE_INTERVAL).await;
        }
    }

    fn kill_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Stop sing-box.
    pub async fn shutdown(&mut self) {
        self.kill_child();
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        self.kill_child();
    }
}

/// Resolve the sing-box binary: `GIPNY_SINGBOX_BIN`, next to current exe, then `PATH`.
fn resolve_singbox_bin() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("GIPNY_SINGBOX_BIN") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let name = if cfg!(windows) { "sing-box.exe" } else { "sing-box" };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for sub in ["", "resources", "../lib", "../Resources"] {
                let cand = if sub.is_empty() {
                    dir.join(name)
                } else {
                    dir.join(sub).join(name)
                };
                if cand.exists() {
                    return Ok(cand);
                }
            }
        }
    }
    Ok(PathBuf::from(name))
}
