// Wire format shared by native and browser personal terminals.
use serde::{Deserialize, Serialize};

pub const MAX_TEXT: usize = 16 * 1024;
pub const MAX_PENDING: usize = 64;
pub const PART_SIZE: usize = 4096;
pub const MAX_ENVELOPE: usize = MAX_TEXT + 2048;
pub const MAX_TTL_MS: i64 = 90 * 86_400_000;
const INVALID: &str = "one-time message unavailable";

pub fn parts(envelope: &[u8]) -> impl ExactSizeIterator<Item = &[u8]> {
    let total = envelope.len().div_ceil(PART_SIZE).max(2);
    envelope.chunks(envelope.len().div_ceil(total).max(1))
}

#[derive(Default)]
pub struct PartBuffer {
    next: u16,
    total: u16,
    width: usize,
    envelope: Vec<u8>,
}

impl PartBuffer {
    pub fn push(
        &mut self,
        index: u16,
        total: u16,
        bytes: &[u8],
    ) -> Result<Option<Vec<u8>>, &'static str> {
        if !(2..=MAX_ENVELOPE.div_ceil(PART_SIZE) as u16).contains(&total)
            || index != self.next
            || index >= total
            || bytes.is_empty()
            || bytes.len() > PART_SIZE
            || self.envelope.len() + bytes.len() > MAX_ENVELOPE
        {
            return Err(INVALID);
        }
        if index == 0 {
            self.total = total;
            self.width = bytes.len();
        }
        let last = index + 1 == total;
        if total != self.total || bytes.len() > self.width || (!last && bytes.len() != self.width) {
            return Err(INVALID);
        }
        self.envelope.extend_from_slice(bytes);
        self.next += 1;
        Ok(last.then(|| std::mem::take(&mut self.envelope)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub to: [u8; 32],
    pub id: [u8; 16],
    pub nonce: [u8; 32],
    pub expires_at: i64,
}

impl Request {
    pub fn current(&self, now: i64) -> bool {
        self.expires_at > now && self.expires_at <= now.saturating_add(MAX_TTL_MS)
    }
}
