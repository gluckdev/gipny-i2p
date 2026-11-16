//! Durable received-part bitmap and bounded selective acknowledgements.

use crate::session::WireFileAck;

/// Holes named in one acknowledgement at most.
const MAX_MISSING: usize = 64;

/// Which parts of a file the receiver holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Received {
    bits: Vec<u8>,
    total: u32,
    count: u32,
}

impl Received {
    pub fn completed(total:u32) -> Self {
        Self { bits:vec![u8::MAX;total.div_ceil(8) as usize],total,count:total }
    }
    pub fn new(total: u32) -> Self {
        Self { bits: vec![0; total.div_ceil(8) as usize], total, count: 0 }
    }

    /// From the bitmap kept in the database.
    pub fn from_bits(bits: Vec<u8>, total: u32) -> Self {
        let mut r = Self { bits, total, count: 0 };
        r.bits.resize(total.div_ceil(8) as usize, 0);
        r.count = (0..total).filter(|&i| r.has(i)).count() as u32;
        r
    }

    pub fn bits(&self) -> &[u8] {
        &self.bits
    }

    pub fn has(&self, index: u32) -> bool {
        index < self.total && self.bits[(index / 8) as usize] & (1 << (index % 8)) != 0
    }

    /// Mark a part as held; false if it already was (a resend).
    pub fn mark(&mut self, index: u32) -> bool {
        if index >= self.total || self.has(index) {
            return false;
        }
        self.bits[(index / 8) as usize] |= 1 << (index % 8);
        self.count += 1;
        true
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn complete(&self) -> bool {
        self.count == self.total
    }

    /// The first part not held: everything below it is here.
    pub fn up_to(&self) -> u32 {
        (0..self.total).find(|&i| !self.has(i)).unwrap_or(self.total)
    }

    /// Parts known lost: not held, with a later one held.
    pub fn missing(&self) -> Vec<u32> {
        let Some(last) = (0..self.total).rev().find(|&i| self.has(i)) else { return Vec::new() };
        (self.up_to()..last).filter(|&i| !self.has(i)).take(MAX_MISSING).collect()
    }

    pub fn ack(&self, file_id: [u8; 16]) -> WireFileAck {
        let seen_to = (0..self.total).rev().find(|&i| self.has(i)).map_or(0, |i| i + 1);
        WireFileAck { file_id, received_up_to: self.up_to(), missing: self.missing(), seen_to, resumed: false, echo_index: 0, echo_sent_at: 0 }
    }

    /// The ack a recipient sends as it starts: what it holds, and that
    /// anything else once in flight to it is gone.
    pub fn resume_ack(&self, file_id: [u8; 16]) -> WireFileAck {
        WireFileAck { resumed: true, ..self.ack(file_id) }
    }
}

