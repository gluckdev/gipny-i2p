//! Compact v3 contact cards. v1/v2 remain import formats.
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
use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine};
use sha2::{Digest, Sha256};

use crate::crypto::IdentityCard;

const V3_PREFIX: &str = "gipny:v3:";
const CHECKSUM_DOMAIN: &[u8] = b"gipny/contact-card/v3\0";
const MAX_NAME_BYTES: usize = 96;
const MAX_V3_BYTES: usize = 230;
const MAX_V3_CHARS: usize = 316;

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
        if let Some(payload) = raw.strip_prefix(V3_PREFIX) { return parse_v3(payload); }
        if raw.len() > 16_384 { return Err(CardError::Malformed); }
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
                name: name.filter(|n| !n.is_empty()).map(percent_decode).transpose()?,
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
                name: name.filter(|n| !n.is_empty()).map(percent_decode).transpose()?,
            });
        }
        Err(CardError::NotACard)
    }

    /// Canonical v3: intact public keys and I2P destination hashes. The
    /// checksum detects copying errors, not identity substitution.
    pub fn encode(&self) -> Result<String, CardError> {
        let onion = address_hash(&self.onion)?;
        let relay = self.relay.as_deref().map(address_hash).transpose()?;
        let name = self.name.as_deref().filter(|name| !name.is_empty());
        if name.is_some_and(|name| name.len() > MAX_NAME_BYTES) { return Err(CardError::Malformed); }
        let mut body = Vec::with_capacity(MAX_V3_BYTES);
        body.push(u8::from(relay.is_some()) | (u8::from(name.is_some()) << 1));
        body.extend_from_slice(&self.sign_pk);
        body.extend_from_slice(&self.dh_pk);
        body.extend_from_slice(&onion);
        if let Some(relay) = relay { body.extend_from_slice(&relay); }
        if let Some(name) = name {
            body.push(name.len() as u8);
            body.extend_from_slice(name.as_bytes());
        }
        let checksum = checksum(&body);
        body.extend_from_slice(&checksum);
        Ok(format!("{V3_PREFIX}{}", URL_SAFE_NO_PAD.encode(body)))
    }
}

impl fmt::Display for ContactCard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode().map_err(|_| fmt::Error)?)
    }
}

fn checksum(body: &[u8]) -> [u8; 4] {
    let mut hash = Sha256::new();
    hash.update(CHECKSUM_DOMAIN);
    hash.update(body);
    let sum = hash.finalize();
    [sum[0], sum[1], sum[2], sum[3]]
}

fn parse_v3(payload: &str) -> Result<ContactCard, CardError> {
    if payload.len() + V3_PREFIX.len() > MAX_V3_CHARS { return Err(CardError::Malformed); }
    let bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| CardError::Malformed)?;
    if !(101..=MAX_V3_BYTES).contains(&bytes.len()) || URL_SAFE_NO_PAD.encode(&bytes) != payload {
        return Err(CardError::Malformed);
    }
    let (body, sum) = bytes.split_at(bytes.len() - 4);
    if checksum(body).as_slice() != sum || body[0] & !3 != 0 { return Err(CardError::Malformed); }
    let mut offset = 97;
    let relay = if body[0] & 1 != 0 {
        let hash: &[u8; 32] = body.get(offset..offset + 32).ok_or(CardError::Malformed)?.try_into().map_err(|_| CardError::Malformed)?;
        offset += 32;
        Some(b32_address(hash))
    } else { None };
    let name = if body[0] & 2 != 0 {
        let len = usize::from(*body.get(offset).ok_or(CardError::Malformed)?);
        offset += 1;
        if len == 0 || len > MAX_NAME_BYTES { return Err(CardError::Malformed); }
        let name = std::str::from_utf8(body.get(offset..offset + len).ok_or(CardError::Malformed)?).map_err(|_| CardError::Malformed)?;
        offset += len;
        Some(name.to_string())
    } else { None };
    if offset != body.len() { return Err(CardError::Malformed); }
    Ok(ContactCard { sign_pk: body[1..33].try_into().map_err(|_| CardError::Malformed)?,
        dh_pk: body[33..65].try_into().map_err(|_| CardError::Malformed)?,
        onion: b32_address(body[65..97].try_into().map_err(|_| CardError::Malformed)?), relay, name })
}

pub(crate) fn address_hash(address: &str) -> Result<[u8; 32], CardError> {
    let address = address.trim();
    let bad = || CardError::BadAddress(address.to_string());
    if address.len() > 4096 { return Err(bad()); }
    let lower = address.to_ascii_lowercase();
    if let Some(host) = lower.strip_suffix(".b32.i2p") {
        if host.len() != 52 { return Err(bad()); }
        let mut bytes = Vec::with_capacity(32);
        let (mut acc, mut bits) = (0u32, 0u32);
        for byte in host.bytes() {
            let digit = match byte { b'a'..=b'z' => byte - b'a', b'2'..=b'7' => byte - b'2' + 26, _ => return Err(bad()) };
            acc = (acc << 5) | u32::from(digit);
            bits += 5;
            if bits >= 8 { bits -= 8; bytes.push((acc >> bits) as u8); }
        }
        if bits != 4 || acc & 15 != 0 { return Err(bad()); }
        return bytes.try_into().map_err(|_| bad());
    }
    if address.len() > 4096 || !is_valid_i2p_address(address) { return Err(bad()); }
    let body = address.trim_end_matches('=');
    let normalized = body.replace('-', "+").replace('~', "/");
    let padding = (4 - normalized.len() % 4) % 4;
    if address.len() != body.len() && address.len() - body.len() != padding { return Err(bad()); }
    let raw = STANDARD.decode(format!("{normalized}{}", "=".repeat(padding))).map_err(|_| bad())?;
    // Full Destination: encryption key256 + signing key128 + certificate
    // type1/length2 + exactly that certificate payload. Reachability is not
    // implied; actual I2P certificate/algorithm handling remains in libi2pd.
    if raw.len() < 387 || 387 + usize::from(u16::from_be_bytes([raw[385], raw[386]])) != raw.len() {
        return Err(bad());
    }
    Ok(Sha256::digest(&raw).into())
}

fn b32_address(hash: &[u8; 32]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut text = String::with_capacity(60);
    let (mut acc, mut bits) = (0u32, 0u32);
    for byte in hash {
        acc = (acc << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 { bits -= 5; text.push(ALPHABET[((acc >> bits) & 31) as usize] as char); }
    }
    if bits != 0 { text.push(ALPHABET[((acc << (5 - bits)) & 31) as usize] as char); }
    text.push_str(".b32.i2p");
    text
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

fn percent_decode(s: &str) -> Result<String, CardError> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let (Some(h), Some(l)) = (hex_val(bytes.get(i + 1)), hex_val(bytes.get(i + 2))) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
            return Err(CardError::Malformed);
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).map_err(|_| CardError::Malformed)
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
        let mut raw: Vec<u8> = (0..384).map(|n| n as u8).collect();
        raw.extend_from_slice(&[0, 0, 0]); // Null certificate, no payload.
        STANDARD.encode(raw).replace('+', "-").replace('/', "~")
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
        let compact = c.encode().unwrap();
        assert!(compact.starts_with(V3_PREFIX));
        let restored = ContactCard::parse(&compact).unwrap();
        assert_eq!(restored.identity(), c.identity());
        assert_eq!(restored.relay, Some(b32_address(&address_hash(&d).unwrap())));
    }

    #[test]
    fn parses_v1_without_name_and_roundtrips() {
        let (sign, dh) = (key_hex("56b3e2253770a160", "aa"), key_hex("8003b0d82a1d64c7", "bb"));
        let s = format!("gipny:v1:{B32}:{sign}:{dh}");
        let c = ContactCard::parse(&s).unwrap();
        assert_eq!(c.relay, None);
        assert_eq!(c.name, None);
        assert!(c.encode().unwrap().starts_with(V3_PREFIX));
    }

    #[test]
    fn name_roundtrips_through_compact_utf8() {
        let c = ContactCard {
            onion: B32.into(), sign_pk: [1; 32], dh_pk: [2; 32],
            relay: Some(dest()), name: Some("сервер #1 (prod)".into()),
        };
        let encoded = c.encode().unwrap();
        let back = ContactCard::parse(&encoded).unwrap();
        assert_eq!(back.identity(), c.identity());
        assert_eq!(back.name, c.name);
        assert!(!encoded.contains("сервер"), "wire card contains ASCII symbols only");
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

    #[test]
    fn matches_shared_python_and_typescript_golden_vectors() {
        let fixtures: serde_json::Value = serde_json::from_str(include_str!("../../tests/fixtures/contact-cards-v3.json")).unwrap();
        for vector in fixtures["valid"].as_array().unwrap() {
            let input = &vector["input"];
            let card = ContactCard {
                onion: input["onion"].as_str().unwrap().into(),
                sign_pk: hex32(input["sign_pk"].as_str().unwrap()).unwrap(),
                dh_pk: hex32(input["dh_pk"].as_str().unwrap()).unwrap(),
                relay: input["relay"].as_str().map(str::to_string),
                name: input["name"].as_str().map(str::to_string),
            };
            let encoded = card.encode().unwrap();
            assert_eq!(encoded, vector["encoded"].as_str().unwrap(), "{}", vector["id"]);
            let parsed = ContactCard::parse(&encoded).unwrap();
            assert_eq!(parsed.identity(), card.identity());
            assert_eq!(parsed.onion, vector["decoded"]["onion"].as_str().unwrap());
            assert_eq!(parsed.relay.as_deref(), vector["decoded"]["relay"].as_str());
            assert_eq!(parsed.name.as_deref(), vector["decoded"]["name"].as_str());
            assert_eq!(parsed.encode().unwrap(), encoded);
            assert!(crate::relay::same_destination(&card.onion, &parsed.onion));
            if let (Some(original), Some(compact)) = (card.relay.as_deref(), parsed.relay.as_deref()) {
                assert!(crate::relay::same_destination(original, compact));
                assert_eq!(crate::relay::destination_hash(original), crate::relay::destination_hash(compact));
            }
        }
        for vector in fixtures["invalid"].as_array().unwrap() {
            assert!(ContactCard::parse(vector["encoded"].as_str().unwrap()).is_err(), "{}", vector["id"]);
        }
    }

    #[test]
    fn export_rejects_bad_addresses_and_names_without_lossy_fallback() {
        let mut card = ContactCard { onion: B32.into(), sign_pk: [1; 32], dh_pk: [2; 32],
            relay: Some(B32.into()), name: None };
        assert_eq!(card.encode().unwrap().len(), 187);
        card.name = Some("я".repeat(49));
        assert!(card.encode().is_err());
        card.name = None;
        card.relay = Some(String::new());
        assert!(card.encode().is_err());
        card.relay = None;
        card.onion = "a".repeat(52) + ".b32.i2p";
        assert!(card.encode().is_ok());
        card.onion = "a".repeat(51) + "b.b32.i2p"; // Nonzero base32 padding bits.
        assert!(card.encode().is_err());
        card.onion = dest();
        card.onion.pop();
        assert!(card.encode().is_err());
    }
}
