//! What a process listens on, from `/proc` (Linux only): the "no local
//! ports" promise checked, not taken on trust. The router's own transports
//! (NTCP2 on every address, SSU2 over UDP) face the network and are expected;
//! anything on loopback — a SAM bridge, an HTTP proxy, a control port — is a
//! door another process on the machine could use, and fails the run.

/// One listening TCP socket: its local address and port.
#[derive(Debug, PartialEq, Eq)]
pub struct Listener {
    pub addr: String,
    pub port: u16,
    pub loopback: bool,
}

/// TCP sockets in LISTEN held by process `pid` (`"self"` for this one).
pub fn tcp_listeners(pid: &str) -> std::io::Result<Vec<Listener>> {
    let inodes = socket_inodes(pid)?;
    let mut out = Vec::new();
    for table in ["tcp", "tcp6"] {
        let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/net/{table}")) else { continue };
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
        let Ok(target) = std::fs::read_link(fd?.path()) else { continue };
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
