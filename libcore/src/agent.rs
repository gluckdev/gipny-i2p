//! Agent mode: running a master's console commands on this machine.
//!
//! The transport is the ordinary message pipeline — a command is a message
//! carrying `WireConsole { kind: CONSOLE_COMMAND }`, its result one carrying
//! `CONSOLE_OUTPUT`. What lives here is the part both hosts of that pipeline
//! share: the app (`core/src/core.rs`) when the user has switched it into agent
//! mode, and the headless `gipny-agent` binary, which is nothing else.
//!
//! Who may drive the console is decided by the caller, and only ever by the
//! signing key of the contact a message was decrypted for — never by anything
//! the message says about itself.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::session::{WireConsole, CONSOLE_OUTPUT};

/// Settings key holding the master's 32-byte signing key. Present = agent mode
/// is on for that contact; absent = off.
pub const SETTING_AGENT_MASTER: &str = "agent_master";

/// Marker bodies for the control kinds. A client without the console field
/// shows these as plain text, which is the right fallback.
pub const BODY_GRANT: &str = "[agent on]";
pub const BODY_REVOKE: &str = "[agent off]";
pub const BODY_OFF: &str = "[agent stop]";

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// Fits the 64 KiB padding bucket with room for the frame around it, so a
/// full page of output costs the same on the wire as a short one.
pub const DEFAULT_MAX_OUTPUT: usize = 60_000;
/// Largest file `/agent get` will send back.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ExecOptions {
    pub timeout: Duration,
    /// Cap on the output text; beyond it the middle is dropped, keeping the
    /// start (what the command printed first) and the end (how it finished).
    pub max_output: usize,
    /// Where commands run, where uploads land by default and what relative
    /// paths mean. `None`: the home directory.
    pub cwd: Option<PathBuf>,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self { timeout: DEFAULT_TIMEOUT, max_output: DEFAULT_MAX_OUTPUT, cwd: None }
    }
}

impl ExecOptions {
    fn base_dir(&self) -> Option<PathBuf> {
        self.cwd.clone().filter(|p| p.is_dir()).or_else(home_dir)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecResult {
    pub output: String,
    /// `None` when the process did not exit normally (signal, timeout, or it
    /// could not be started).
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub truncated: bool,
    pub timed_out: bool,
}

/// Keeps the first `head` and last `tail` bytes of a stream and counts the
/// rest, so a runaway command costs bounded memory however long it runs.
struct Capture {
    head: Vec<u8>,
    head_cap: usize,
    tail: VecDeque<u8>,
    tail_cap: usize,
    dropped: usize,
}

impl Capture {
    fn new(max_output: usize) -> Self {
        let head_cap = max_output * 2 / 3;
        Self {
            head: Vec::new(),
            head_cap,
            tail: VecDeque::new(),
            tail_cap: max_output - head_cap,
            dropped: 0,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        for &b in chunk {
            if self.head.len() < self.head_cap {
                self.head.push(b);
            } else {
                if self.tail.len() == self.tail_cap {
                    self.tail.pop_front();
                    self.dropped += 1;
                }
                self.tail.push_back(b);
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.head.is_empty() && self.tail.is_empty()
    }

    fn render(&self) -> (String, bool) {
        let mut s = String::from_utf8_lossy(&self.head).into_owned();
        if self.dropped > 0 {
            s.push_str(&format!("\n[… {} bytes skipped …]\n", self.dropped));
        }
        let tail: Vec<u8> = self.tail.iter().copied().collect();
        s.push_str(&String::from_utf8_lossy(&tail));
        (s, self.dropped > 0)
    }
}

fn spawn_reader<R>(mut r: R, cap: Arc<Mutex<Capture>>) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            match r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => cap.lock().unwrap_or_else(|p| p.into_inner()).push(&buf[..n]),
            }
        }
    })
}

fn shell_command(cmd: &str) -> Command {
    #[cfg(unix)]
    {
        let mut c = Command::new("sh");
        // `exec 2>&1` inside the shell keeps stdout and stderr in the order the
        // command produced them, which two separate pipes cannot.
        c.arg("-c").arg(format!("exec 2>&1\n{cmd}"));
        c
    }
    #[cfg(windows)]
    {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command"]);
        // Without this, output comes back in the OEM code page and Cyrillic
        // text is mojibake by the time it is a Rust string.
        c.arg(format!(
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; $OutputEncoding = [System.Text.Encoding]::UTF8; & {{ {cmd} }} 2>&1 | Out-String -Width 4096"
        ));
        c
    }
}

/// Runs `cmd` through the platform shell and returns everything it printed.
///
/// Never returns `Err`: a command that could not be started is reported in
/// the output text like any other failure, so the caller always has something
/// to send back and never retries the shell.
pub async fn run_command(cmd: &str, o: &ExecOptions) -> ExecResult {
    let started = Instant::now();
    let mut command = shell_command(cmd);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    if let Some(dir) = o.base_dir() {
        command.current_dir(dir);
    }
    // Own process group, so a timeout kills what the shell started too and
    // not just the shell — otherwise `sleep 999` outlives the command that
    // ran it and keeps the output pipe open.
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ExecResult {
                output: format!("[cannot start shell: {e}]"),
                exit_code: None,
                duration_ms: started.elapsed().as_millis() as u64,
                truncated: false,
                timed_out: false,
            };
        }
    };
    let out_cap = Arc::new(Mutex::new(Capture::new(o.max_output)));
    let err_cap = Arc::new(Mutex::new(Capture::new(o.max_output)));
    let out_task = child.stdout.take().map(|s| spawn_reader(s, out_cap.clone()));
    let err_task = child.stderr.take().map(|s| spawn_reader(s, err_cap.clone()));

    let mut timed_out = false;
    let status = match tokio::time::timeout(o.timeout, child.wait()).await {
        Ok(Ok(s)) => Some(s),
        Ok(Err(_)) => None,
        Err(_) => {
            timed_out = true;
            kill_tree(&mut child);
            let _ = child.wait().await;
            None
        }
    };
    // The pipes close when every holder exits. A daemon the command left behind
    // keeps them open; the grace period is for the normal case, after which
    // whatever was captured is what gets reported.
    for mut t in [out_task, err_task].into_iter().flatten() {
        if tokio::time::timeout(Duration::from_millis(500), &mut t).await.is_err() {
            t.abort();
        }
    }
    let duration_ms = started.elapsed().as_millis() as u64;

    let (mut output, mut truncated) = out_cap.lock().unwrap_or_else(|p| p.into_inner()).render();
    {
        let err = err_cap.lock().unwrap_or_else(|p| p.into_inner());
        if !err.is_empty() {
            let (e, t) = err.render();
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&e);
            truncated |= t;
        }
    }
    let exit_code = status.and_then(|s| s.code());
    if timed_out {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&format!("[killed: {:.1}s timeout]", o.timeout.as_secs_f32()));
    } else if let Some(s) = status {
        if s.code().is_none() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&format!("[killed by signal: {s}]"));
        }
    }
    ExecResult { output, exit_code, duration_ms, truncated, timed_out }
}

fn kill_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
}

/// The one reserved word in a console: a body whose first token is exactly
/// `/agent`. Anything else — `/usr/bin/x` included — is a shell command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    /// `/agent get <path>` — send the file back as an attachment.
    Get(String),
    /// `/agent put [<dir or path>]` — save the attached file(s) there; no
    /// argument means the home directory.
    Put(Option<String>),
    /// `/agent`, `/agent help`.
    Help,
    Unknown(String),
}

pub fn parse_control(body: &str) -> Option<Control> {
    let t = body.trim();
    let rest = t.strip_prefix("/agent")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim();
    if rest.is_empty() || rest == "help" {
        return Some(Control::Help);
    }
    if let Some(p) = rest.strip_prefix("get") {
        if p.starts_with(char::is_whitespace) {
            let path = p.trim();
            if !path.is_empty() {
                return Some(Control::Get(path.to_string()));
            }
        }
    }
    if rest == "put" {
        return Some(Control::Put(None));
    }
    if let Some(p) = rest.strip_prefix("put") {
        if p.starts_with(char::is_whitespace) {
            return Some(Control::Put(Some(p.trim().to_string())));
        }
    }
    Some(Control::Unknown(rest.to_string()))
}

pub const HELP_TEXT: &str = "\
/agent get <path>         send a file back as an attachment (up to 8 MiB)
/agent put [<dir|path>]   save the attached file(s) there; default: home directory
attach + command          files are saved to the home directory, then the command runs
anything else             runs in the shell; output comes back with the exit code";

/// One console message from the master, answered. Attached files are saved
/// first (to the `/agent put` target, or the home directory), then the body
/// is run as a control word or a shell command. Never fails: a failure is a
/// reply with a non-zero exit code, so the caller always has something to send
/// back and never retries anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleReply {
    pub body: String,
    pub console: WireConsole,
    pub attachments: Vec<(String, Vec<u8>)>,
}

impl ConsoleReply {
    fn plain(body: String, exit_code: i32) -> Self {
        Self {
            body,
            console: WireConsole { kind: CONSOLE_OUTPUT, exit_code: Some(exit_code), duration_ms: Some(0), truncated: false },
            attachments: vec![],
        }
    }
}

pub async fn handle_console_request(
    body: &str,
    attachments: &[(String, Vec<u8>)],
    o: &ExecOptions,
) -> ConsoleReply {
    let control = parse_control(body);
    let base = o.base_dir();
    let mut lines = String::new();
    if !attachments.is_empty() {
        let target = match &control {
            Some(Control::Put(p)) => p.as_deref(),
            _ => None,
        };
        match save_uploads(attachments, target, base.as_deref()) {
            Ok(saved) => {
                for (p, n) in saved {
                    lines.push_str(&format!("saved {} ({n} bytes)\n", p.display()));
                }
            }
            Err(e) => return ConsoleReply::plain(format!("{lines}upload failed: {e}\n"), 1),
        }
    }
    match control {
        Some(Control::Put(_)) => {
            if attachments.is_empty() {
                ConsoleReply::plain(format!("{lines}/agent put needs an attached file\n"), 1)
            } else {
                ConsoleReply::plain(lines, 0)
            }
        }
        Some(Control::Get(path)) => match read_file_for_send(&path, base.as_deref()) {
            Ok((name, data)) => ConsoleReply {
                body: format!("{lines}{name}: {} bytes\n", data.len()),
                console: WireConsole { kind: CONSOLE_OUTPUT, exit_code: Some(0), duration_ms: Some(0), truncated: false },
                attachments: vec![(name, data)],
            },
            Err(e) => ConsoleReply::plain(format!("{lines}{e}\n"), 1),
        },
        Some(Control::Help) => ConsoleReply::plain(format!("{lines}{HELP_TEXT}\n"), 0),
        Some(Control::Unknown(w)) => {
            ConsoleReply::plain(format!("{lines}unknown /agent command: {w}\n{HELP_TEXT}\n"), 1)
        }
        None => {
            if body.trim().is_empty() {
                return ConsoleReply::plain(lines, 0);
            }
            let r = run_command(body, o).await;
            ConsoleReply {
                body: format!("{lines}{}", r.output),
                console: WireConsole {
                    kind: CONSOLE_OUTPUT,
                    exit_code: r.exit_code,
                    duration_ms: Some(r.duration_ms),
                    truncated: r.truncated,
                },
                attachments: vec![],
            }
        }
    }
}

/// `~` and relative paths are taken from `base` (the command directory).
fn resolve_path(p: &str, base: Option<&Path>) -> PathBuf {
    let p = p.trim();
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        if let Some(h) = home_dir() {
            return h.join(rest);
        }
    }
    if p == "~" {
        if let Some(h) = home_dir() {
            return h;
        }
    }
    let path = PathBuf::from(p);
    if path.is_relative() {
        if let Some(b) = base {
            return b.join(path);
        }
    }
    path
}

/// Writes uploaded files. A target that is an existing directory takes every
/// file under its own name; any other target is the exact destination of a
/// single file. Names are reduced to their last component so an attachment
/// called `../x` cannot climb out of the directory.
pub fn save_uploads(
    files: &[(String, Vec<u8>)],
    target: Option<&str>,
    base: Option<&Path>,
) -> Result<Vec<(PathBuf, usize)>, String> {
    let base = match target.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => resolve_path(t, base),
        None => base.map(Path::to_path_buf).ok_or_else(|| "no directory to save into".to_string())?,
    };
    let mut out = Vec::with_capacity(files.len());
    if base.is_dir() {
        for (name, data) in files {
            let leaf = Path::new(name).file_name().map(|n| n.to_os_string())
                .filter(|n| !n.is_empty())
                .ok_or_else(|| format!("bad attachment name: {name:?}"))?;
            let dest = base.join(leaf);
            std::fs::write(&dest, data).map_err(|e| format!("{}: {e}", dest.display()))?;
            out.push((dest, data.len()));
        }
    } else {
        if files.len() != 1 {
            return Err(format!("{}: not a directory, and {} files were attached", base.display(), files.len()));
        }
        let (_, data) = &files[0];
        std::fs::write(&base, data).map_err(|e| format!("{}: {e}", base.display()))?;
        out.push((base, data.len()));
    }
    Ok(out)
}

/// Reads a file for `/agent get`, refusing anything over [`MAX_FILE_BYTES`].
pub fn read_file_for_send(path: &str, base: Option<&Path>) -> Result<(String, Vec<u8>), String> {
    let resolved = resolve_path(path, base);
    let p = resolved.as_path();
    let meta = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if !meta.is_file() {
        return Err(format!("{}: not a regular file", p.display()));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "{}: {} bytes, larger than the {} MiB limit",
            p.display(), meta.len(), MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    let data = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    Ok((name, data))
}

pub fn hostname() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
        if rc == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            let s = String::from_utf8_lossy(&buf[..end]).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "agent".into())
}

/// Short hex prefix of a key for log lines.
pub fn hex8(b: &[u8]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_parsing_reserves_only_the_literal_word() {
        assert_eq!(parse_control("/agent get /etc/hosts"), Some(Control::Get("/etc/hosts".into())));
        assert_eq!(parse_control("  /agent   get   /tmp/a b.txt  "), Some(Control::Get("/tmp/a b.txt".into())));
        assert_eq!(parse_control("/agent"), Some(Control::Help));
        assert_eq!(parse_control("/agent help"), Some(Control::Help));
        assert_eq!(parse_control("/agent frobnicate"), Some(Control::Unknown("frobnicate".into())));
        assert_eq!(parse_control("/agent get"), Some(Control::Unknown("get".into())));
        assert_eq!(parse_control("/agent put"), Some(Control::Put(None)));
        assert_eq!(parse_control("/agent put /etc/nginx/"), Some(Control::Put(Some("/etc/nginx/".into()))));
        assert_eq!(parse_control("/usr/bin/uptime"), None);
        assert_eq!(parse_control("/agentx"), None);
        assert_eq!(parse_control("uptime"), None);
    }

    #[test]
    fn capture_keeps_head_and_tail() {
        let mut c = Capture::new(30);
        c.push(&[b'a'; 50]);
        c.push(&[b'z'; 10]);
        let (s, truncated) = c.render();
        assert!(truncated);
        assert!(s.starts_with(&"a".repeat(20)));
        assert!(s.ends_with("zzzzzzzzzz"));
        assert!(s.contains("[… 30 bytes skipped …]"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_a_command_and_reports_exit_code() {
        let r = run_command("printf 'hi\\n'; printf 'err\\n' >&2; exit 3", &ExecOptions::default()).await;
        assert_eq!(r.exit_code, Some(3));
        assert_eq!(r.output, "hi\nerr\n", "stderr interleaves with stdout in order");
        assert!(!r.truncated && !r.timed_out);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kills_on_timeout_and_keeps_partial_output() {
        let o = ExecOptions { timeout: Duration::from_millis(500), ..Default::default() };
        let t0 = Instant::now();
        let r = run_command("echo before; sleep 30; echo too-late", &o).await;
        assert!(r.timed_out);
        assert_eq!(r.exit_code, None);
        assert!(r.output.starts_with("before\n"), "{}", r.output);
        assert!(r.output.contains("[killed: 0.5s timeout]"), "{}", r.output);
        assert!(!r.output.contains("too-late"));
        // The sleep is in the shell's process group and dies with it, so the
        // pipe closes at once rather than after the reader grace period.
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    }

    #[tokio::test]
    async fn put_then_get_roundtrips_through_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().to_string_lossy().into_owned();
        let files = vec![("../escape.txt".to_string(), b"hello".to_vec()), ("b.bin".to_string(), vec![0, 1, 2])];
        let reply = handle_console_request(&format!("/agent put {target}"), &files, &ExecOptions::default()).await;
        assert_eq!(reply.console.exit_code, Some(0), "{}", reply.body);
        assert!(dir.path().join("escape.txt").is_file(), "name reduced to its last component");
        assert!(reply.body.contains("saved") && reply.body.contains("(5 bytes)"));

        let get = handle_console_request(&format!("/agent get {}", dir.path().join("b.bin").display()), &[], &ExecOptions::default()).await;
        assert_eq!(get.console.exit_code, Some(0));
        assert_eq!(get.attachments, vec![("b.bin".to_string(), vec![0, 1, 2])]);

        let exact = dir.path().join("renamed.txt");
        let reply = handle_console_request(&format!("/agent put {}", exact.display()), &files[..1], &ExecOptions::default()).await;
        assert_eq!(reply.console.exit_code, Some(0), "{}", reply.body);
        assert_eq!(std::fs::read(&exact).unwrap(), b"hello");

        let missing = handle_console_request("/agent get /definitely/not/here", &[], &ExecOptions::default()).await;
        assert_eq!(missing.console.exit_code, Some(1));
        assert!(missing.attachments.is_empty());
        let no_file = handle_console_request("/agent put", &[], &ExecOptions::default()).await;
        assert_eq!(no_file.console.exit_code, Some(1));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn attachment_plus_command_saves_then_runs() {
        let dir = tempfile::tempdir().unwrap();
        let o = ExecOptions { cwd: Some(dir.path().to_path_buf()), ..Default::default() };
        let files = vec![("script.sh".to_string(), b"echo from-script".to_vec())];
        let reply = handle_console_request("sh script.sh", &files, &o).await;
        assert_eq!(reply.console.exit_code, Some(0), "{}", reply.body);
        assert!(reply.body.contains("saved"), "{}", reply.body);
        assert!(reply.body.ends_with("from-script\n"), "{}", reply.body);
        // `/agent put` with no target lands in the same directory.
        let reply = handle_console_request("/agent put", &files, &o).await;
        assert_eq!(reply.console.exit_code, Some(0), "{}", reply.body);
        assert!(dir.path().join("script.sh").is_file());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn truncates_long_output() {
        let o = ExecOptions { max_output: 300, ..Default::default() };
        let r = run_command("yes | head -c 2000", &o).await;
        assert!(r.truncated);
        assert!(r.output.contains("bytes skipped"));
        assert!(r.output.len() < 400);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn utf8_survives() {
        let r = run_command("printf 'привет\\n'", &ExecOptions::default()).await;
        assert_eq!(r.output, "привет\n");
    }

    #[test]
    fn hostname_is_nonempty() {
        assert!(!hostname().is_empty());
    }
}
