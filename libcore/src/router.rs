//! i2p router (SAMv3 bridge) lifecycle.
//!
//! Unlike the previous embedded Tor transport (Arti compiled into the binary),
//! i2p needs a running router that exposes a SAMv3 bridge on a local TCP port.
//! We bundle a small go-i2p based helper binary (`gipny-i2p-router`, a thin
//! wrapper around `go-i2p/go-sam-bridge`'s embedded-router library) and spawn it
//! as a child process; [`crate::net`] then speaks SAMv3 to it.
//!
//! On Android the router is started in-process by the Kotlin foreground service
//! (via JNI); there we only [`RouterHandle::attach`] to the already-listening
//! SAM port instead of spawning a child.

use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::net::{NetError, Result};

/// Default SAMv3 TCP port.
pub const DEFAULT_SAM_PORT: u16 = 7656;

/// How long to wait for the router to come up. First run reseeds and builds
/// tunnels, which can take a couple of minutes.
const START_TIMEOUT: Duration = Duration::from_secs(180);
/// Poll interval while waiting for SAM to answer.
const PROBE_INTERVAL: Duration = Duration::from_millis(500);
/// Per-probe connect/response timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Handle to a running i2p router.
///
/// Dropping the handle kills the child process we spawned (if any), tearing the
/// router down together with the profile — matching the previous Tor behaviour
/// where the transport lived and died with the unlocked vault.
pub struct RouterHandle {
    child: Option<Child>,
    sam_port: u16,
}

impl RouterHandle {
    /// Spawn our bundled i2p router and wait until its SAM bridge answers.
    ///
    /// `data_dir` is the profile directory; router state lives under
    /// `data_dir/i2p/router`. `bin` overrides the router binary path (otherwise
    /// it is resolved from `GIPNY_I2P_BIN`, next to the executable, or `PATH`).
    pub async fn start(data_dir: &Path, bin: Option<PathBuf>) -> Result<Self> {
        let bin = match bin {
            Some(b) => b,
            None => resolve_router_bin()?,
        };
        let router_dir = data_dir.join("i2p").join("router");
        std::fs::create_dir_all(&router_dir)
            .map_err(|e| NetError::I2p(format!("router data dir: {e}")))?;

        // Always run our own router on a private, free port so the profile is
        // self-contained and we never route through an untrusted foreign router.
        let sam_port = pick_free_port(DEFAULT_SAM_PORT);

        eprintln!(
            "[i2p] launching router {} (SAM 127.0.0.1:{sam_port}); first run may take 1-3 min...",
            bin.display()
        );
        let child = Command::new(&bin)
            .arg("--sam-listen")
            .arg(format!("127.0.0.1:{sam_port}"))
            .arg("--data")
            .arg(&router_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| NetError::I2p(format!("spawn router {}: {e}", bin.display())))?;

        let mut handle = Self { child: Some(child), sam_port };
        handle.await_ready().await?;
        eprintln!("[i2p] router ready (SAM up on {sam_port})");
        Ok(handle)
    }

    /// Attach to an already-running SAM bridge (Android: started in-process via
    /// JNI; or a developer-managed router). Does not own the process.
    pub async fn attach(sam_port: u16) -> Result<Self> {
        let mut handle = Self { child: None, sam_port };
        handle.await_ready().await?;
        Ok(handle)
    }

    /// SAM TCP port the router is listening on.
    pub fn sam_port(&self) -> u16 {
        self.sam_port
    }

    async fn await_ready(&mut self) -> Result<()> {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if probe_sam(self.sam_port).await {
                return Ok(());
            }
            // If the child we own has already died, surface it instead of spinning.
            if let Some(child) = self.child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(NetError::I2p(format!("router exited early: {status}")));
                }
            }
            if Instant::now() >= deadline {
                self.kill_child();
                return Err(NetError::I2p("router SAM did not come up in time".into()));
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

    /// Stop the router (kills the child process if we own it).
    pub async fn shutdown(&mut self) {
        self.kill_child();
    }
}

impl Drop for RouterHandle {
    fn drop(&mut self) {
        self.kill_child();
    }
}

/// Probe a SAMv3 bridge: TCP connect + `HELLO VERSION` handshake, expect `RESULT=OK`.
pub async fn probe_sam(port: u16) -> bool {
    let fut = async {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
        stream
            .write_all(b"HELLO VERSION MIN=3.0 MAX=3.3\n")
            .await
            .ok()?;
        let mut buf = [0u8; 256];
        let n = stream.read(&mut buf).await.ok()?;
        let reply = String::from_utf8_lossy(&buf[..n]);
        Some(reply.contains("RESULT=OK"))
    };
    matches!(tokio::time::timeout(PROBE_TIMEOUT, fut).await, Ok(Some(true)))
}

/// Return `preferred` if free, otherwise an OS-assigned free port.
fn pick_free_port(preferred: u16) -> u16 {
    if StdTcpListener::bind(("127.0.0.1", preferred)).is_ok() {
        return preferred;
    }
    StdTcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(preferred)
}

/// Resolve the bundled router binary: `GIPNY_I2P_BIN`, then next to the current
/// executable (and common bundle sub-dirs), then the bare name on `PATH`.
fn resolve_router_bin() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("GIPNY_I2P_BIN") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let name = if cfg!(windows) { "gipny-i2p-router.exe" } else { "gipny-i2p-router" };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for sub in ["", "resources", "../lib", "../Resources"] {
                let cand = if sub.is_empty() { dir.join(name) } else { dir.join(sub).join(name) };
                if cand.exists() {
                    return Ok(cand);
                }
            }
        }
    }
    // Fall back to PATH resolution by bare name.
    Ok(PathBuf::from(name))
}
