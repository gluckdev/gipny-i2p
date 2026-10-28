//! What a process listens on, from `/proc` (Linux only): the "no local
//! ports" promise checked, not taken on trust. The router's own transports
//! (NTCP2 on every address, SSU2 over UDP) face the network and are expected;
//! anything on loopback — a SAM bridge, an HTTP proxy, a control port — is a
//! door another process on the machine could use, and fails the run.

use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const OBSERVER_FLAG: &str = "--inspect-process-listeners";
const REPORT_MAGIC: &str = "GIPNY-LISTENERS-1";
const MAX_REPORT: usize = 64 * 1024;
const OBSERVER_TIMEOUT: Duration = Duration::from_secs(10);

/// One listening TCP socket: its local address and port.
#[derive(Debug, PartialEq, Eq)]
pub struct Listener {
    pub addr: String,
    pub port: u16,
    pub loopback: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcessIdentity { pid: u32, start_ticks: u64 }

fn invalid(message: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message) }

fn positive_number(text: &str) -> io::Result<u64> {
    let value = text.parse::<u64>().map_err(|_| invalid("expected a positive decimal number"))?;
    if value == 0 || text != value.to_string() { return Err(invalid("noncanonical positive decimal number")); }
    Ok(value)
}

fn numeric_pid(text: &str) -> io::Result<u32> {
    u32::try_from(positive_number(text)?).map_err(|_| invalid("PID exceeds u32"))
}

fn process_identity(pid: u32) -> io::Result<ProcessIdentity> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let (prefix, fields) = text.rsplit_once(") ").ok_or_else(|| invalid("invalid /proc stat framing"))?;
    if prefix.split_once(" (").map(|(number, _)| number) != Some(pid.to_string().as_str()) {
        return Err(invalid("/proc stat PID mismatch"));
    }
    // The tail begins at field 3; starttime is field 22. comm may contain ')'.
    let start_ticks = positive_number(fields.split_whitespace().nth(19)
        .ok_or_else(|| invalid("missing /proc starttime"))?)?;
    Ok(ProcessIdentity { pid, start_ticks })
}

fn snapshot(identity: ProcessIdentity) -> io::Result<Vec<Listener>> {
    if process_identity(identity.pid)? != identity { return Err(invalid("process identity changed before observation")); }
    let listeners = tcp_listeners(&identity.pid.to_string())?;
    if process_identity(identity.pid)? != identity { return Err(invalid("process identity changed during observation")); }
    Ok(listeners)
}

fn encode_report(identity: ProcessIdentity, listeners: &[Listener]) -> io::Result<Vec<u8>> {
    let mut report = format!("{REPORT_MAGIC}\t{}\t{}\n", identity.pid, identity.start_ticks);
    for listener in listeners {
        report.push_str(&format!("{}\t{}\n", listener.addr, listener.port));
        if report.len() > MAX_REPORT { return Err(invalid("listener report exceeds limit")); }
    }
    Ok(report.into_bytes())
}

fn parse_report(identity: ProcessIdentity, bytes: &[u8]) -> io::Result<Vec<Listener>> {
    if bytes.len() > MAX_REPORT { return Err(invalid("observer report exceeds limit")); }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("observer report is not UTF-8"))?;
    let header = format!("{REPORT_MAGIC}\t{}\t{}\n", identity.pid, identity.start_ticks);
    let rows = text.strip_prefix(&header).ok_or_else(|| invalid("observer identity/header mismatch"))?;
    if !text.ends_with('\n') { return Err(invalid("truncated observer report")); }
    let mut seen = std::collections::HashSet::new();
    let mut listeners = Vec::new();
    for row in rows.lines() {
        let (address, port) = row.split_once('\t').ok_or_else(|| invalid("invalid listener row"))?;
        let ip: std::net::IpAddr = address.parse().map_err(|_| invalid("invalid listener IP"))?;
        let port = u16::try_from(positive_number(port)?).map_err(|_| invalid("invalid listener port"))?;
        if address != ip.to_string() || !seen.insert((ip, port)) { return Err(invalid("noncanonical or duplicate listener")); }
        let loopback = match ip {
            std::net::IpAddr::V4(ip) => ip.is_loopback(),
            std::net::IpAddr::V6(ip) => ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback()),
        };
        listeners.push(Listener { addr: address.into(), port, loopback });
    }
    Ok(listeners)
}

fn observer_arguments(args: &[std::ffi::OsString]) -> io::Result<Option<ProcessIdentity>> {
    if args.is_empty() { return Ok(None); }
    if args.len() != 3 || args[0] != OBSERVER_FLAG { return Err(invalid("expected --inspect-process-listeners PID START_TICKS")); }
    let pid = numeric_pid(args[1].to_str().ok_or_else(|| invalid("non-UTF-8 PID"))?)?;
    let start_ticks = positive_number(args[2].to_str().ok_or_else(|| invalid("non-UTF-8 starttime"))?)?;
    Ok(Some(ProcessIdentity { pid, start_ticks }))
}

/// Early test-only executable mode: no router, runtime, profiles, env commands
/// or arbitrary paths. A privileged observer reads only fixed /proc metadata.
pub fn run_observer_mode() -> io::Result<bool> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Some(identity) = observer_arguments(&args)? else { return Ok(false); };
    if !cfg!(target_os = "linux") { return Err(invalid("process observation requires Linux")); }
    let report = encode_report(identity, &snapshot(identity)?)?;
    io::stdout().lock().write_all(&report)?;
    Ok(true)
}

fn read_bounded(reader: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit { return Err(invalid("observer pipe exceeds limit")); }
    Ok(bytes)
}

fn pipe_reader(reader: impl Read + Send + 'static, limit: usize) -> std::sync::mpsc::Receiver<io::Result<Vec<u8>>> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || { let _ = sender.send(read_bounded(reader, limit)); });
    receiver
}

fn receive_pipe(receiver: std::sync::mpsc::Receiver<io::Result<Vec<u8>>>, deadline: Instant) -> io::Result<Vec<u8>> {
    receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())).map_err(|error| match error {
        std::sync::mpsc::RecvTimeoutError::Timeout => io::Error::new(io::ErrorKind::TimedOut, "observer pipe did not close"),
        std::sync::mpsc::RecvTimeoutError::Disconnected => invalid("observer pipe reader stopped without a result"),
    })?
}

fn privileged_observer(identity: ProcessIdentity, binary: &Path) -> io::Result<Vec<Listener>> {
    // Never a shell, arbitrary command from env, or production-agent switch.
    let mut child = Command::new("sudo").args(["-n", "--"]).arg(binary)
        .arg(OBSERVER_FLAG).arg(identity.pid.to_string()).arg(identity.start_ticks.to_string())
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let stdout = child.stdout.take().expect("piped observer stdout");
    let stderr = child.stderr.take().expect("piped observer stderr");
    let stdout = pipe_reader(stdout, MAX_REPORT);
    let stderr = pipe_reader(stderr, 4096);
    let deadline = Instant::now() + OBSERVER_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => break Err(io::Error::new(io::ErrorKind::TimedOut, "process observer timed out")),
            Err(error) => break Err(error),
        }
    };
    if status.is_err() { let _ = child.kill(); let _ = child.wait(); }
    // sudo may keep a monitor/PTY process with an inherited pipe. Do not join
    // a reader indefinitely after killing our own sudo handle. Detached readers
    // are bounded and disappear when this failed harness process exits.
    let stdout = receive_pipe(stdout, deadline);
    let stderr = receive_pipe(stderr, deadline);
    let status = status?;
    let (stdout, stderr) = (stdout?, stderr?);
    if !status.success() {
        return Err(io::Error::other(format!("privileged metadata observer {status}: {}", String::from_utf8_lossy(&stderr))));
    }
    if !stderr.is_empty() { return Err(invalid("unexpected observer stderr")); }
    parse_report(identity, &stdout)
}

fn permission_fallback<T>(direct: impl FnOnce() -> io::Result<T>, observer: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    match direct() {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => observer(),
        result => result,
    }
}

pub fn observed_tcp_listeners(pid: &str) -> io::Result<Vec<Listener>> {
    if pid == "self" { return tcp_listeners(pid); }
    let identity = process_identity(numeric_pid(pid)?)?;
    permission_fallback(|| snapshot(identity), || privileged_observer(identity, &std::env::current_exe()?))
}

pub fn reject_loopback(listeners: &[Listener]) -> io::Result<()> {
    let local: Vec<_> = listeners.iter().filter(|listener| listener.loopback)
        .map(|listener| format!("{}:{}", listener.addr, listener.port)).collect();
    if !local.is_empty() { return Err(invalid(&format!("listens on loopback: {}", local.join(", ")))); }
    Ok(())
}

/// TCP sockets in LISTEN held by process `pid` (`"self"` for this one).
pub fn tcp_listeners(pid: &str) -> std::io::Result<Vec<Listener>> {
    if pid != "self" { numeric_pid(pid)?; }
    let inodes = socket_inodes(pid)?;
    let mut out = Vec::new();
    for table in ["tcp", "tcp6"] {
        let text = match std::fs::read_to_string(format!("/proc/{pid}/net/{table}")) {
            Ok(text) => text,
            Err(error) if table == "tcp6" && error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for line in text.lines().skip(1) {
            if let Some((l, inode)) = parse_listen_line(line) {
                if inodes.contains(&inode) {
                    out.push(l);
                }
            }
        }
    }
    Ok(out)
}

fn socket_inodes(pid: &str) -> std::io::Result<std::collections::HashSet<u64>> {
    let mut set = std::collections::HashSet::new();
    for fd in std::fs::read_dir(format!("/proc/{pid}/fd"))? {
        let target = match std::fs::read_link(fd?.path()) {
            Ok(target) => target,
            // A socket may close between readdir and readlink. Denial and all
            // other failures must not silently become an empty listener set.
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let t = target.to_string_lossy();
        if let Some(n) = t.strip_prefix("socket:[").and_then(|r| r.strip_suffix(']')) {
            if let Ok(n) = n.parse() {
                set.insert(n);
            }
        }
    }
    Ok(set)
}

/// A `/proc/net/tcp{,6}` row in state LISTEN (`0A`), with its inode.
fn parse_listen_line(line: &str) -> Option<(Listener, u64)> {
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.len() < 10 || f[3] != "0A" {
        return None;
    }
    let (hex_addr, hex_port) = f[1].split_once(':')?;
    let port = u16::from_str_radix(hex_port, 16).ok()?;
    let inode = f[9].parse().ok()?;
    let (addr, loopback) = decode_addr(hex_addr)?;
    Some((Listener { addr, port, loopback }, inode))
}

/// The kernel prints each 32-bit word of the address in host order
/// (little-endian on every runner we use).
fn decode_addr(hex: &str) -> Option<(String, bool)> {
    let words: Vec<[u8; 4]> = (0..hex.len() / 8)
        .map(|i| u32::from_str_radix(&hex[i * 8..i * 8 + 8], 16).map(|w| w.to_le_bytes()))
        .collect::<Result<_, _>>()
        .ok()?;
    match words.len() {
        1 => {
            let ip = std::net::Ipv4Addr::from(words[0]);
            Some((ip.to_string(), ip.is_loopback()))
        }
        4 => {
            let bytes: Vec<u8> = words.concat();
            let ip = std::net::Ipv6Addr::from(<[u8; 16]>::try_from(bytes).ok()?);
            let loopback = ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback());
            Some((ip.to_string(), loopback))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_arguments_and_reports_are_strict_and_identity_bound() {
        use std::ffi::OsString;
        let identity = ProcessIdentity { pid: 123, start_ticks: 456 };
        let args = [OBSERVER_FLAG, "123", "456"].map(OsString::from);
        assert_eq!(observer_arguments(&args).unwrap(), Some(identity));
        for args in [
            vec![OBSERVER_FLAG, "../self", "456"], vec![OBSERVER_FLAG, "0", "456"],
            vec![OBSERVER_FLAG, "0123", "456"], vec![OBSERVER_FLAG, "123", "+456"],
            vec![OBSERVER_FLAG, "123", "456", "extra"], vec!["--arbitrary-command"],
        ] {
            assert!(observer_arguments(&args.into_iter().map(OsString::from).collect::<Vec<_>>()).is_err());
        }
        let listeners = vec![
            Listener { addr: "0.0.0.0".into(), port: 1234, loopback: false },
            Listener { addr: "127.0.0.1".into(), port: 7656, loopback: true },
            Listener { addr: "::ffff:127.0.0.1".into(), port: 4444, loopback: true },
        ];
        let report = encode_report(identity, &listeners).unwrap();
        assert_eq!(parse_report(identity, &report).unwrap(), listeners);
        assert!(reject_loopback(&listeners).is_err());
        assert!(reject_loopback(&listeners[..1]).is_ok());
        assert!(parse_report(ProcessIdentity { pid: 124, ..identity }, &report).is_err());
        assert!(parse_report(ProcessIdentity { start_ticks: 457, ..identity }, &report).is_err());
        let header = format!("{REPORT_MAGIC}\t123\t456\n");
        for row in ["127.0.0.1\t0\n", "127.0.0.1\t65536\n", "127.0.0.1\t+80\n",
            "127.0.0.1\t80\textra\n", "localhost\t80\n", "127.0.0.1\t80", "\n",
            "127.0.0.1\t80\n127.0.0.1\t80\n", "::FFFF:127.0.0.1\t80\n"] {
            assert!(parse_report(identity, format!("{header}{row}").as_bytes()).is_err(), "accepted {row:?}");
        }
        assert!(parse_report(identity, &[b'x'; MAX_REPORT + 1]).is_err());
        assert!(read_bounded(&b"12345"[..], 4).is_err());
        assert_eq!(read_bounded(&b"1234"[..], 4).unwrap(), b"1234");
        let (_sender, receiver) = std::sync::mpsc::channel();
        assert_eq!(receive_pipe(receiver, Instant::now()).unwrap_err().kind(), io::ErrorKind::TimedOut);
        let (sender, receiver) = std::sync::mpsc::channel();
        drop(sender);
        assert_eq!(receive_pipe(receiver, Instant::now()).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn only_permission_denied_uses_observer_and_its_errors_remain_failures() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let denied = || Err::<Vec<Listener>, _>(io::Error::from(io::ErrorKind::PermissionDenied));
        let observed = permission_fallback(denied, || { calls.set(calls.get() + 1); Ok(Vec::new()) }).unwrap();
        assert!(observed.is_empty());
        assert_eq!(calls.get(), 1);
        for kind in [io::ErrorKind::NotFound, io::ErrorKind::InvalidData, io::ErrorKind::Interrupted] {
            let result = permission_fallback(|| Err::<(), _>(io::Error::from(kind)), || {
                calls.set(calls.get() + 1); Ok(())
            });
            assert_eq!(result.unwrap_err().kind(), kind);
        }
        assert_eq!(calls.get(), 1, "unexpected failures must never fall back or become PASS");
        assert_eq!(permission_fallback(denied, denied).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    // Executed in a separate libtest process by the test below, never by the
    // privileged metadata observer. A closed stdin also ends the fixture.
    #[cfg(target_os = "linux")]
    #[test]
    fn hardened_child_listener_fixture() {
        if std::env::var_os("GIPNY_TEST_HARDENED_LISTENER").is_none() { return; }
        gipny_libcore::security::harden_process().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let listeners = tcp_listeners("self").unwrap();
        assert!(listeners.iter().any(|listener| listener.loopback && listener.port == port));
        println!("GIPNY-HARDENED-LISTENER {} {port}", std::process::id());
        io::stdout().flush().unwrap();
        let _ = io::stdin().read(&mut [0]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn privileged_observer_detects_real_loopback_on_nondumpable_child() {
        use std::io::BufRead;
        // Required in the dedicated GHA step: missing setup is a failure,
        // never a silent skip or parser-only replacement for the real probe.
        let binary = std::env::var_os("E2E_OBSERVER_BIN").expect("CI must set E2E_OBSERVER_BIN to the built harness");
        let binary = std::fs::canonicalize(binary).expect("built observer executable must exist");
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
        }
        let mut child = ChildGuard(Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "ports::tests::hardened_child_listener_fixture", "--nocapture"])
            .env("GIPNY_TEST_HARDENED_LISTENER", "1")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap());
        let stdout = child.0.stdout.take().unwrap();
        let expected_pid = child.0.id();
        let (sender, ready) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = (|| -> io::Result<u16> {
                let mut output = io::BufReader::new(stdout.take(4096));
                loop {
                    let mut line = String::new();
                    if output.read_line(&mut line)? == 0 { return Err(invalid("fixture exited without its ready marker")); }
                    // libtest may prefix the test name on the same line.
                    if let Some((_, marker)) = line.trim_end().split_once("GIPNY-HARDENED-LISTENER ") {
                        let (pid, port) = marker.split_once(' ').ok_or_else(|| invalid("invalid fixture marker"))?;
                        if numeric_pid(pid)? != expected_pid { return Err(invalid("fixture PID mismatch")); }
                        return u16::try_from(positive_number(port)?).map_err(|_| invalid("invalid fixture port"));
                    }
                }
            })();
            let _ = sender.send(result);
        });
        let port = ready.recv_timeout(OBSERVER_TIMEOUT).expect("hardened fixture readiness timed out")
            .expect("hardened fixture readiness failed");
        assert!(child.0.try_wait().unwrap().is_none());
        let identity = process_identity(child.0.id()).unwrap();
        assert_eq!(snapshot(identity).unwrap_err().kind(), io::ErrorKind::PermissionDenied,
            "unprivileged cross-process observation must reproduce the hardened-agent denial");
        let listeners = privileged_observer(identity, &binary).expect("real sudo metadata observer must succeed");
        assert!(listeners.iter().any(|listener| listener.addr == "127.0.0.1" && listener.port == port && listener.loopback),
            "the privileged observer must identify the actual fixture socket");
        assert!(reject_loopback(&listeners).is_err(), "real loopback must fail the same policy used by agent e2e");
        assert!(privileged_observer(ProcessIdentity { start_ticks: identity.start_ticks + 1, ..identity }, &binary).is_err(),
            "a stale/reused PID identity must fail rather than inspect a replacement process");
        assert!(child.0.try_wait().unwrap().is_none(), "observation must not stop the child");
    }

    #[test]
    fn a_loopback_listener_is_told_from_one_facing_the_network() {
        // 127.0.0.1:7656 (SAM) listening, inode 4242.
        let sam = "   0: 0100007F:1DE8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0000000000000000 100 0 0 10 0";
        let (l, inode) = parse_listen_line(sam).unwrap();
        assert_eq!((l.addr.as_str(), l.port, l.loopback, inode), ("127.0.0.1", 7656, true, 4242));
        // 0.0.0.0:12345 — a transport.
        let any = "   1: 00000000:3039 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 99 1 0 100 0 0 10 0";
        assert!(!parse_listen_line(any).unwrap().0.loopback);
        // An established connection is not a listener.
        let est = "   2: 0100007F:1DE8 0100007F:A000 01 00000000:00000000 00:00000000 00000000  1000        0 7 1 0 100 0 0 10 0";
        assert!(parse_listen_line(est).is_none());
    }

    #[test]
    fn ipv6_loopback_and_mapped_loopback_count() {
        let v6 = "   0: 00000000000000000000000001000000:1DE8 00000000000000000000000000000000:0000 0A 0 0 0 1000 0 5 1";
        let (l, _) = parse_listen_line(v6).unwrap();
        assert_eq!((l.addr.as_str(), l.loopback), ("::1", true));
        let mapped = "   0: 0000000000000000FFFF00000100007F:1DE8 00000000000000000000000000000000:0000 0A 0 0 0 1000 0 6 1";
        assert!(parse_listen_line(mapped).unwrap().0.loopback);
        let any6 = "   0: 00000000000000000000000000000000:3039 00000000000000000000000000000000:0000 0A 0 0 0 1000 0 8 1";
        assert!(!parse_listen_line(any6).unwrap().0.loopback);
    }

    #[test]
    fn this_process_is_readable() {
        // Whatever it holds, reading it works on Linux.
        if cfg!(target_os = "linux") {
            tcp_listeners("self").unwrap();
        }
    }
}
