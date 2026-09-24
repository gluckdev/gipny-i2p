//! Files too large for one letter: cut into parts, sent one letter each on
//! the ratchet session, put back together on the other side.
//! Plan: docs/plans/2026-09-24-chunked-files.md.
//!
//! On disk a file is kept the way the parts travel: each part sealed on its
//! own ([`AttachmentCipher`], the part's index in the nonce) in a slot of its
//! own, `index × (chunk_size + TAG)` from the start. The sender reads one part
//! at a time, never the whole file; the receiver writes each where it
//! belongs, in whatever order the parts come.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::crypto::AttachmentCipher;
use crate::session::WireFileAck;

/// Attachments up to this size still ride inside their letter.
pub const INLINE_MAX: usize = 128 * 1024;
/// Plaintext bytes per part: with the letter around it and the ratchet on
/// top, a part fits the 256 KiB padding bucket — through any relay and the
/// relay network alike.
pub const CHUNK_SIZE: u32 = 192 * 1024;
/// Parts of one file sent to one recipient and not yet acknowledged, resends
/// included: what can wait on their relay at once (well inside its per
/// recipient limit, so nobody else's mail is pushed out).
pub const WINDOW: u32 = 16;
/// Largest file sent in parts.
pub const MAX_FILE_BYTES: u64 = 2 << 30;
/// A receiver acknowledges at least this often, in parts.
pub const ACK_EVERY: u32 = 4;
/// Holes named in one acknowledgement at most.
const MAX_MISSING: usize = 64;
/// AEAD tag each sealed part carries.
const TAG: u64 = 16;

pub fn chunk_count(size: u64, chunk_size: u32) -> u32 {
    size.div_ceil(chunk_size as u64).max(1) as u32
}

/// Plaintext length of part `index`.
pub fn chunk_len(size: u64, chunk_size: u32, index: u32) -> usize {
    let start = index as u64 * chunk_size as u64;
    size.saturating_sub(start).min(chunk_size as u64) as usize
}

fn slot(chunk_size: u32, index: u32) -> u64 {
    index as u64 * (chunk_size as u64 + TAG)
}

/// Seal `src` into `dest` part by part. Returns its size and sha256.
pub fn seal_from(mut src: impl Read, dest: &Path, cipher: &AttachmentCipher, chunk_size: u32) -> io::Result<(u64, [u8; 32])> {
    let mut out = File::create(dest)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; chunk_size as usize];
    let (mut size, mut index) = (0u64, 0u32);
    loop {
        let n = read_full(&mut src, &mut buf)?;
        if n == 0 && index > 0 {
            break;
        }
        hash.update(&buf[..n]);
        let sealed = cipher.encrypt_chunk(index as u64, &[], &buf[..n]).map_err(io::Error::other)?;
        out.write_all(&sealed)?;
        size += n as u64;
        index += 1;
        if n < buf.len() {
            break;
        }
        if size > MAX_FILE_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "file larger than MAX_FILE_BYTES"));
        }
    }
    out.sync_all()?;
    Ok((size, hash.finalize().into()))
}

fn read_full(src: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match src.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// Part `index` of a sealed file, opened.
pub fn read_part(path: &Path, cipher: &AttachmentCipher, size: u64, chunk_size: u32, index: u32) -> io::Result<Vec<u8>> {
    let len = chunk_len(size, chunk_size, index) + TAG as usize;
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(slot(chunk_size, index)))?;
    let mut sealed = vec![0u8; len];
    f.read_exact(&mut sealed)?;
    cipher.decrypt_chunk(index as u64, &[], &sealed).map_err(io::Error::other)
}

/// Seal a received part into its slot. The file is created on first use and
/// may have holes until every part is in.
pub fn write_part(path: &Path, cipher: &AttachmentCipher, size: u64, chunk_size: u32, index: u32, data: &[u8]) -> io::Result<()> {
    if index >= chunk_count(size, chunk_size) || data.len() != chunk_len(size, chunk_size, index) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "part does not fit the offered file"));
    }
    let sealed = cipher.encrypt_chunk(index as u64, &[], data).map_err(io::Error::other)?;
    let mut f = OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
    f.seek(SeekFrom::Start(slot(chunk_size, index)))?;
    f.write_all(&sealed)?;
    Ok(())
}

/// Open every part in order into `out`, returning the sha256 of what was
/// written; the caller compares it with the offer's.
pub fn open_to(path: &Path, cipher: &AttachmentCipher, size: u64, chunk_size: u32, out: &mut impl Write) -> io::Result<[u8; 32]> {
    let mut hash = Sha256::new();
    for index in 0..chunk_count(size, chunk_size) {
        let part = read_part(path, cipher, size, chunk_size, index)?;
        hash.update(&part);
        out.write_all(&part)?;
    }
    Ok(hash.finalize().into())
}

/// An attachment on disk, whole in memory, whichever way it was sealed:
/// whole (`chunk_size` `None`, one chunk) or in parts.
pub fn read_attachment(path: &Path, key: [u8; 32], size: u64, chunk_size: Option<i64>) -> io::Result<Vec<u8>> {
    let cipher = AttachmentCipher::from_key(key);
    match chunk_size {
        None => {
            let sealed = std::fs::read(path)?;
            cipher.decrypt_chunk(0, &[], &sealed).map_err(io::Error::other)
        }
        Some(cs) => {
            let mut out = Vec::with_capacity(size as usize);
            open_to(path, &cipher, size, cs as u32, &mut out)?;
            Ok(out)
        }
    }
}

/// Which parts of a file the receiver holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Received {
    bits: Vec<u8>,
    total: u32,
    count: u32,
}

impl Received {
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
        WireFileAck { file_id, received_up_to: self.up_to(), missing: self.missing() }
    }
}

/// The sender's view of one file to one recipient.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sending {
    /// Parts `0..next` have gone out at least once.
    pub next: u32,
    /// Parts `0..acked` are known to be there.
    pub acked: u32,
    /// Known lost: to go again before anything new.
    pub resend: Vec<u32>,
}

impl Sending {
    /// Take in an acknowledgement.
    pub fn on_ack(&mut self, ack: &WireFileAck) {
        self.acked = self.acked.max(ack.received_up_to.min(self.next));
        for &m in &ack.missing {
            if m >= self.acked && m < self.next && !self.resend.contains(&m) {
                self.resend.push(m);
            }
        }
        self.resend.retain(|&m| m >= self.acked);
        self.resend.sort_unstable();
    }

    /// What to send now: lost parts first, then new ones, never more than
    /// [`WINDOW`] past what is acknowledged.
    pub fn due(&mut self, total: u32) -> Vec<u32> {
        let mut out: Vec<u32> = std::mem::take(&mut self.resend);
        let limit = self.acked.saturating_add(WINDOW).min(total);
        while self.next < limit && out.len() < WINDOW as usize {
            out.push(self.next);
            self.next += 1;
        }
        out
    }

    /// Nothing heard for long, though the recipient is alive: send what is
    /// outstanding again.
    pub fn rewind(&mut self) {
        self.resend = (self.acked..self.next).collect();
    }

    pub fn done(&self, total: u32) -> bool {
        self.acked >= total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 % 251) as u8).collect()
    }

    #[test]
    fn sizes() {
        assert_eq!(chunk_count(0, 10), 1);
        assert_eq!(chunk_count(10, 10), 1);
        assert_eq!(chunk_count(11, 10), 2);
        assert_eq!(chunk_len(25, 10, 2), 5);
        assert_eq!(chunk_len(25, 10, 3), 0);
    }

    #[test]
    fn seal_read_and_reassemble_out_of_order() {
        let dir = tempfile::tempdir().unwrap();
        let src = data(10 * 1000 + 123);
        let cs = 1000;
        let (a, b) = (AttachmentCipher::generate(), AttachmentCipher::generate());
        let sealed = dir.path().join("out");
        let (size, sha) = seal_from(&src[..], &sealed, &a, cs).unwrap();
        assert_eq!(size, src.len() as u64);
        assert_eq!(sha, <[u8; 32]>::from(Sha256::digest(&src)));

        // The receiver takes the parts in any order, under its own key.
        let recv = dir.path().join("in");
        let total = chunk_count(size, cs);
        let mut got = Received::new(total);
        for index in (0..total).rev() {
            let part = read_part(&sealed, &a, size, cs, index).unwrap();
            write_part(&recv, &b, size, cs, index, &part).unwrap();
            assert!(got.mark(index));
            assert!(!got.mark(index), "a resend is not new");
        }
        assert!(got.complete());
        let mut out = Vec::new();
        assert_eq!(open_to(&recv, &b, size, cs, &mut out).unwrap(), sha);
        assert_eq!(out, src);
    }

    #[test]
    fn attachments_read_back_either_way() {
        let dir = tempfile::tempdir().unwrap();
        let src = data(5000);
        let c = AttachmentCipher::generate();
        let whole = dir.path().join("whole");
        std::fs::write(&whole, c.encrypt_chunk(0, &[], &src).unwrap()).unwrap();
        assert_eq!(read_attachment(&whole, *c.key(), 5000, None).unwrap(), src);
        let parts = dir.path().join("parts");
        seal_from(&src[..], &parts, &c, 700).unwrap();
        assert_eq!(read_attachment(&parts, *c.key(), 5000, Some(700)).unwrap(), src);
    }

    #[test]
    fn a_part_that_does_not_fit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let c = AttachmentCipher::generate();
        let p = dir.path().join("in");
        assert!(write_part(&p, &c, 25, 10, 3, &[0; 5]).is_err(), "past the end");
        assert!(write_part(&p, &c, 25, 10, 2, &[0; 6]).is_err(), "wrong length");
        assert!(write_part(&p, &c, 25, 10, 2, &[0; 5]).is_ok());
    }

    #[test]
    fn empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = AttachmentCipher::generate();
        let p = dir.path().join("f");
        let (size, sha) = seal_from(&[][..], &p, &c, 10).unwrap();
        assert_eq!(size, 0);
        let mut out = Vec::new();
        assert_eq!(open_to(&p, &c, 0, 10, &mut out).unwrap(), sha);
        assert!(out.is_empty());
    }

    #[test]
    fn acks_name_the_holes() {
        let mut r = Received::new(10);
        for i in [0, 1, 2, 5, 7] {
            r.mark(i);
        }
        let a = r.ack([1; 16]);
        assert_eq!(a.received_up_to, 3);
        assert_eq!(a.missing, vec![3, 4, 6]);
        let back = Received::from_bits(r.bits().to_vec(), 10);
        assert_eq!(back, r);
    }

    #[test]
    fn the_window_and_resends() {
        let total = 40;
        let mut s = Sending::default();
        let first = s.due(total);
        assert_eq!(first, (0..WINDOW).collect::<Vec<_>>());
        assert!(s.due(total).is_empty(), "nothing past the window");

        // 0..10 arrived, 10 and 12 lost, 11 arrived.
        s.on_ack(&WireFileAck { file_id: [0; 16], received_up_to: 10, missing: vec![10, 12] });
        let next = s.due(total);
        assert_eq!(&next[..2], &[10, 12], "lost parts first");
        assert_eq!(next[2..], (16..26).collect::<Vec<_>>()[..], "then new ones up to acked + WINDOW");

        s.on_ack(&WireFileAck { file_id: [0; 16], received_up_to: 26, missing: vec![] });
        s.rewind();
        assert!(s.due(total).is_empty() || s.acked == 26);
        while !s.done(total) {
            let d = s.due(total);
            let top = d.iter().max().copied().unwrap_or(s.acked);
            s.on_ack(&WireFileAck { file_id: [0; 16], received_up_to: top + 1, missing: vec![] });
        }
        assert!(s.done(total));
    }
}
