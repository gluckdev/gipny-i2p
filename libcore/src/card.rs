//! The textual contact card, `gipny:v1:…` / `gipny:v2:…`.
//!
//! Mirrors `encodeCard`/`decodeCard` in `ui/src/api.ts`, which stay the
//! reference for the app; this exists for headless binaries that take a card
//! on the command line and print their own. Any change to the format lands in
//! both places.
//!
//! * v1: `gipny:v1:<onion>:<sign_pk hex>:<dh_pk hex>[:<name>]`
//! * v2: `gipny:v2:<onion>:<sign_pk hex>:<dh_pk hex>:<relay>[:<name>]`
//!
//! `<name>` is percent-encoded the way `encodeURIComponent` does it, so it can
//! never contain a bare `:`.

use std::fmt;

use crate::crypto::IdentityCard;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContactCard {
    pub onion: String,
    pub sign_pk: [u8; 32],
    pub dh_pk: [u8; 32],
    /// Relay the holder collects from. `None` on a v1 card.
    pub relay: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CardError {
    #[error("not a gipny contact card")]
    NotACard,
    #[error("malformed contact card")]
    Malformed,
    #[error("card field is not a valid i2p address: {0}")]
    BadAddress(String),
}

impl ContactCard {
    pub fn identity(&self) -> IdentityCard {
        IdentityCard { sign_pk: self.sign_pk, dh_pk: self.dh_pk }
    }

    pub fn parse(input: &str) -> Result<Self, CardError> {
        let raw = input.trim();
        if let Some(rest) = raw.strip_prefix("gipny:v2:") {
            let mut it = rest.splitn(5, ':');
            let onion = it.next().ok_or(CardError::Malformed)?;
            let sign = it.next().ok_or(CardError::Malformed)?;
            let dh = it.next().ok_or(CardError::Malformed)?;
            let relay = it.next().ok_or(CardError::Malformed)?;
            let name = it.next();
            if onion.is_empty() || relay.is_empty() {
                return Err(CardError::Malformed);
            }
            if !is_valid_i2p_address(relay) {
                return Err(CardError::BadAddress(relay.to_string()));
            }
            return Ok(Self {
                onion: onion.to_string(),
                sign_pk: hex32(sign)?,
                dh_pk: hex32(dh)?,
                relay: Some(relay.to_string()),
                name: name.filter(|n| !n.is_empty()).map(percent_decode),
            });
        }
        if let Some(rest) = raw.strip_prefix("gipny:v1:") {
            let mut it = rest.splitn(4, ':');
            let onion = it.next().ok_or(CardError::Malformed)?;
            let sign = it.next().ok_or(CardError::Malformed)?;
            let dh = it.next().ok_or(CardError::Malformed)?;
            let name = it.next();
            if onion.is_empty() {
                return Err(CardError::Malformed);
            }
            return Ok(Self {
                onion: onion.to_string(),
                sign_pk: hex32(sign)?,
                dh_pk: hex32(dh)?,
                relay: None,
                name: name.filter(|n| !n.is_empty()).map(percent_decode),
            });
        }
        Err(CardError::NotACard)
    }

    /// v2 when there is a relay to carry, v1 otherwise — the same rule the app
    /// applies, so cards from either source look alike.
    pub fn encode(&self) -> String {
        let sign = hex(&self.sign_pk);
        let dh = hex(&self.dh_pk);
        let mut s = match &self.relay {
            Some(r) if !r.trim().is_empty() => {
                format!("gipny:v2:{}:{}:{}:{}", self.onion, sign, dh, r.trim())
            }
            _ => format!("gipny:v1:{}:{}:{}", self.onion, sign, dh),
        };
        if let Some(n) = self.name.as_deref().filter(|n| !n.is_empty()) {
            s.push(':');
            s.push_str(&percent_encode(n));
        }
        s
    }
}

impl fmt::Display for ContactCard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

/// A `.b32.i2p` hostname (52 base32 chars) or a full base64 destination
/// (516+ chars of i2p's alphabet, where `-` and `~` stand in for `+` and `/`).
pub fn is_valid_i2p_address(addr: &str) -> bool {
    let a = addr.trim();
    if let Some(host) = a.strip_suffix(".b32.i2p").or_else(|| a.strip_suffix(".B32.I2P")) {
        return host.len() == 52
            && host.chars().all(|c| matches!(c, 'a'..='z' | 'A'..='Z' | '2'..='7'));
    }
    let body = a.trim_end_matches('=');
    if a.len() - body.len() > 2 || body.len() < 516 {
        return false;
    }
    body.chars().all(|c| c.is_ascii_alphanumeric() || c == '~' || c == '-')
}

fn hex32(s: &str) -> Result<[u8; 32], CardError> {
    if s.len() != 64 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CardError::Malformed);
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| CardError::Malformed)?;
    }
    Ok(out)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `encodeURIComponent`: everything but `A-Z a-z 0-9 - _ . ! ~ * ' ( )` is
/// percent-encoded as UTF-8 bytes.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')');
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes.get(i + 1)), hex_val(bytes.get(i + 2))) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: Option<&u8>) -> Option<u8> {
    match b? {
        c @ b'0'..=b'9' => Some(c - b'0'),
        c @ b'a'..=b'f' => Some(c - b'a' + 10),
        c @ b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B32: &str = "zqeubwvvtm5f3cto5s3r6ftj36j4x4nsvvh3znux7uruhxjgk5za.b32.i2p";

    fn key_hex(prefix: &str, last: &str) -> String {
        let mut s = prefix.to_string();
        while s.len() < 64 - last.len() { s.push('0'); }
        s.push_str(last);
        assert_eq!(s.len(), 64);
        s
    }

    fn dest() -> String {
        // 516 chars of the i2p base64 alphabet plus the `AAAA` the real ones end in.
        let mut s: String = std::iter::repeat("Ab-~").take(129).collect();
        s.push_str("==");
        s
    }

    #[test]
    fn parses_v2_with_encoded_name() {
        let d = dest();
        let (sign, dh) = (key_hex("56b3e2253770a160", "aa"), key_hex("8003b0d82a1d64c7", "bb"));
        let s = format!("gipny:v2:{B32}:{sign}:{dh}:{d}:my%20laptop%3A1");
        let c = ContactCard::parse(&s).unwrap();
        assert_eq!(c.onion, B32);
        assert_eq!(c.relay.as_deref(), Some(d.as_str()));
        assert_eq!(c.name.as_deref(), Some("my laptop:1"));
        assert_eq!(c.sign_pk[0], 0x56);
        assert_eq!(c.dh_pk[31], 0xbb);
        assert_eq!(c.encode(), s);
    }

    #[test]
    fn parses_v1_without_name_and_roundtrips() {
        let (sign, dh) = (key_hex("56b3e2253770a160", "aa"), key_hex("8003b0d82a1d64c7", "bb"));
        let s = format!("gipny:v1:{B32}:{sign}:{dh}");
        let c = ContactCard::parse(&s).unwrap();
        assert_eq!(c.relay, None);
        assert_eq!(c.name, None);
        assert_eq!(c.encode(), s);
    }

    #[test]
    fn name_roundtrips_through_percent_encoding() {
        let c = ContactCard {
            onion: B32.into(), sign_pk: [1; 32], dh_pk: [2; 32],
            relay: Some(dest()), name: Some("сервер #1 (prod)".into()),
        };
        let back = ContactCard::parse(&c.encode()).unwrap();
        assert_eq!(back, c);
        assert!(!c.encode().contains("сервер"), "name must be percent-encoded on the wire");
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(ContactCard::parse("hello").unwrap_err(), CardError::NotACard);
        assert_eq!(ContactCard::parse("gipny:v1:x:abc:def").unwrap_err(), CardError::Malformed);
        let (sign, dh) = (key_hex("56b3e2253770a160", "aa"), key_hex("8003b0d82a1d64c7", "bb"));
        let bad_relay = format!("gipny:v2:{B32}:{sign}:{dh}:not-an-address");
        assert!(matches!(ContactCard::parse(&bad_relay).unwrap_err(), CardError::BadAddress(_)));
    }

    #[test]
    fn address_validation_matches_the_app() {
        assert!(is_valid_i2p_address(B32));
        assert!(is_valid_i2p_address(&dest()));
        assert!(!is_valid_i2p_address("x.i2p"));
        assert!(!is_valid_i2p_address(&"a".repeat(300)));
        assert!(!is_valid_i2p_address("short.b32.i2p"));
    }
}
