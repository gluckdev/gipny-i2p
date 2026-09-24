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
/// Most parts in flight to one recipient, across files, resends included: what
/// can wait on their relay at once — ~8 MiB, a third of the built-in relay's
/// per-recipient limit, so nobody else's mail is pushed out. [`Flow`] keeps
/// the window under it, as large as the path carries.
pub const WINDOW: u32 = 32;
/// Largest file sent in parts.
pub const MAX_FILE_BYTES: u64 = 2 << 30;
/// A receiver acknowledges at least this often, in parts: every one. The
/// sender's window is timed by the acks, and one every few left a small
/// window (after a loss) waiting out the timeout for an ack that was due
/// only at the fourth part. An ack is a ~1 KiB letter against a 192 KiB part.
pub const ACK_EVERY: u32 = 1;
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
        let seen_to = (0..self.total).rev().find(|&i| self.has(i)).map_or(0, |i| i + 1);
        WireFileAck { file_id, received_up_to: self.up_to(), missing: self.missing(), seen_to }
    }
}

/// The sender's view of one file to one recipient; what the database keeps.
/// How much goes at once and when a part counts as lost is [`Flow`]'s, kept
/// per recipient in memory.
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
    /// Take in an acknowledgement. A hole it names is lost only once its
    /// part is past the timeout: over several lanes parts overtake each
    /// other, and a hole is most often a part still on its way.
    pub fn on_ack(&mut self, ack: &WireFileAck, flow: &mut Flow, now: i64) {
        let held = |i: u32| i < ack.received_up_to || (i < ack.seen_to && !ack.missing.contains(&i));
        flow.on_held(ack.file_id, held, now);
        self.acked = self.acked.max(ack.received_up_to.min(self.next));
        let mut lost = false;
        for &m in &ack.missing {
            if m >= self.acked && m < self.next && !self.resend.contains(&m) && flow.overdue(ack.file_id, m, now) {
                flow.unsent(ack.file_id, m);
                self.resend.push(m);
                lost = true;
            }
        }
        if lost {
            flow.on_loss(now);
        }
        self.resend.retain(|&m| m >= self.acked);
        self.resend.sort_unstable();
    }

    /// What to send now: lost parts first, then new ones, as many as the
    /// recipient's window has room for. Each is counted in flight from now.
    pub fn due(&mut self, file_id: [u8; 16], total: u32, flow: &mut Flow, now: i64) -> Vec<u32> {
        // Outstanding from before this run: in flight from now on, but for
        // what is to go again anyway.
        let resend = self.resend.clone();
        flow.adopt(file_id, (self.acked..self.next).filter(|i| !resend.contains(i)), now);
        // The tail: nothing after it came, so no ack names it a hole.
        let mut lost = false;
        for i in flow.overdue_parts(file_id, now) {
            // Lost: no longer in flight, it goes again below.
            flow.unsent(file_id, i);
            if i >= self.acked && i < self.next && !self.resend.contains(&i) {
                self.resend.push(i);
                lost = true;
            }
        }
        if lost {
            flow.on_loss(now);
        }
        self.resend.sort_unstable();
        let room = flow.room() as usize;
        let take = room.min(self.resend.len());
        let mut out: Vec<u32> = self.resend.drain(..take).collect();
        let resent = out.len();
        let limit = self.acked.saturating_add(MAX_SPAN).min(total);
        while out.len() < room && self.next < limit {
            out.push(self.next);
            self.next += 1;
        }
        for (k, &i) in out.iter().enumerate() {
            flow.on_sent(file_id, i, now, k < resent);
        }
        out
    }

    /// Nothing heard for long, though the recipient is alive: send what is
    /// outstanding again.
    pub fn rewind(&mut self, file_id: [u8; 16], flow: &mut Flow) {
        self.resend = (self.acked..self.next).collect();
        flow.forget(file_id);
    }

    pub fn done(&self, total: u32) -> bool {
        self.acked >= total
    }
}

/// Furthest a sender runs ahead of the recipient's first missing part.
const MAX_SPAN: u32 = 4 * WINDOW;
/// Parts in flight to a recipient at the start: grown as acks come on time.
const FLOW_START: f64 = 4.0;
/// Bounds of the timeout after which a part in flight counts as lost.
const RTO_MIN_MS: i64 = 10_000;
const RTO_MAX_MS: i64 = 120_000;
/// The timeout before a single round trip has been measured.
const RTO_FIRST_MS: i64 = 30_000;

/// How parts go to one recipient, across all files: a window like TCP's.
/// It opens by one part per part acknowledged until the first loss, then by
/// one per window; a loss halves it, once per round trip. The round trip is
/// measured on parts sent once (not on resends, whose ack could answer
/// either copy), and a part in flight longer than `srtt + 4·rttvar` is lost.
/// Every recipient's path is its own: a slow phone on three hops and a relay
/// on a server each get the window their path carries.
#[derive(Clone, Debug)]
pub struct Flow {
    cwnd: f64,
    ssthresh: f64,
    srtt: Option<f64>,
    rttvar: f64,
    last_cut: i64,
    /// Parts in flight: when each went, and whether it was a resend.
    sent: std::collections::HashMap<([u8; 16], u32), (i64, bool)>,
    /// Files seen since this started (see [`Sending::due`]).
    known: std::collections::HashSet<[u8; 16]>,
}

impl Default for Flow {
    fn default() -> Self {
        Self {
            cwnd: FLOW_START,
            ssthresh: WINDOW as f64,
            srtt: None,
            rttvar: 0.0,
            last_cut: i64::MIN / 2,
            sent: Default::default(),
            known: Default::default(),
        }
    }
}

impl Flow {
    /// Parts that may be in flight now.
    pub fn window(&self) -> u32 {
        (self.cwnd.floor() as u32).clamp(1, WINDOW)
    }

    pub fn in_flight(&self) -> u32 {
        self.sent.len() as u32
    }

    fn room(&self) -> u32 {
        self.window().saturating_sub(self.in_flight())
    }

    /// After this long in flight a part counts as lost.
    pub fn rto_ms(&self) -> i64 {
        match self.srtt {
            None => RTO_FIRST_MS,
            Some(s) => ((s + 4.0 * self.rttvar) as i64).clamp(RTO_MIN_MS, RTO_MAX_MS),
        }
    }

    pub fn srtt_ms(&self) -> Option<i64> {
        self.srtt.map(|s| s as i64)
    }

    fn adopt(&mut self, file_id: [u8; 16], outstanding: impl Iterator<Item = u32>, now: i64) {
        if self.known.insert(file_id) {
            for i in outstanding {
                self.sent.entry((file_id, i)).or_insert((now, true));
            }
        }
    }

    fn on_sent(&mut self, file_id: [u8; 16], index: u32, now: i64, resend: bool) {
        self.known.insert(file_id);
        self.sent.insert((file_id, index), (now, resend));
    }

    /// A part that was counted in flight did not go after all.
    pub fn unsent(&mut self, file_id: [u8; 16], index: u32) {
        self.sent.remove(&(file_id, index));
    }

    fn on_held(&mut self, file_id: [u8; 16], held: impl Fn(u32) -> bool, now: i64) {
        let arrived: Vec<(u32, i64, bool)> = self.sent.iter()
            .filter(|((f, i), _)| *f == file_id && held(*i))
            .map(|((_, i), (t, r))| (*i, *t, *r))
            .collect();
        for (i, at, resend) in arrived {
            self.sent.remove(&(file_id, i));
            if !resend {
                self.sample((now - at).max(0) as f64);
            }
            if self.cwnd < self.ssthresh {
                self.cwnd += 1.0;
            } else {
                self.cwnd += 1.0 / self.cwnd;
            }
            self.cwnd = self.cwnd.min(WINDOW as f64);
        }
    }

    fn sample(&mut self, rtt: f64) {
        match self.srtt {
            None => {
                self.srtt = Some(rtt);
                self.rttvar = rtt / 2.0;
            }
            Some(s) => {
                self.rttvar = 0.75 * self.rttvar + 0.25 * (s - rtt).abs();
                self.srtt = Some(0.875 * s + 0.125 * rtt);
            }
        }
    }

    fn overdue(&self, file_id: [u8; 16], index: u32, now: i64) -> bool {
        match self.sent.get(&(file_id, index)) {
            Some((at, _)) => now - at >= self.rto_ms(),
            // Not in flight as far as this run knows: already given up on.
            None => true,
        }
    }

    fn overdue_parts(&self, file_id: [u8; 16], now: i64) -> Vec<u32> {
        let rto = self.rto_ms();
        let mut v: Vec<u32> = self.sent.iter()
            .filter(|((f, _), (at, _))| *f == file_id && now - at >= rto)
            .map(|((_, i), _)| *i)
            .collect();
        v.sort_unstable();
        v
    }

    fn on_loss(&mut self, now: i64) {
        // Once per round trip: the parts of one burst lost together are one
        // sign the path is full, not several.
        let rtt = self.srtt.map(|s| s as i64).unwrap_or(RTO_FIRST_MS);
        if now - self.last_cut < rtt {
            return;
        }
        self.last_cut = now;
        self.ssthresh = (self.cwnd / 2.0).max(2.0);
        self.cwnd = self.ssthresh;
    }

    /// A file done, cancelled or rewound: its parts are no longer in flight.
    pub fn forget(&mut self, file_id: [u8; 16]) {
        self.sent.retain(|(f, _), _| *f != file_id);
        self.known.remove(&file_id);
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

    fn ack(file: [u8; 16], up_to: u32, missing: Vec<u32>, seen_to: u32) -> WireFileAck {
        WireFileAck { file_id: file, received_up_to: up_to, missing, seen_to }
    }

    const F: [u8; 16] = [1; 16];

    #[test]
    fn an_ack_says_where_the_held_parts_end() {
        let mut r = Received::new(10);
        assert_eq!(r.ack(F).seen_to, 0);
        r.mark(0);
        r.mark(6);
        assert_eq!(r.ack(F).seen_to, 7);
    }

    #[test]
    fn the_window_starts_small_and_opens_as_acks_come() {
        let (mut s, mut flow) = (Sending::default(), Flow::default());
        let first = s.due(F, 1000, &mut flow, 0);
        assert_eq!(first, vec![0, 1, 2, 3], "four to start with");
        assert!(s.due(F, 1000, &mut flow, 100).is_empty(), "nothing past the window");
        s.on_ack(&ack(F, 4, vec![], 4), &mut flow, 4_000);
        assert_eq!(flow.window(), 8, "one more per part acknowledged");
        assert_eq!(s.due(F, 1000, &mut flow, 4_000).len(), 8);
        assert_eq!(flow.srtt_ms(), Some(4_000));
    }

    #[test]
    fn a_hole_is_a_part_on_its_way_until_its_timeout() {
        let (mut s, mut flow) = (Sending::default(), Flow::default());
        s.due(F, 1000, &mut flow, 0);
        // 1 overtaken by 2 and 3 on other lanes: named, but only just sent.
        s.on_ack(&ack(F, 1, vec![1], 4), &mut flow, 2_000);
        assert!(s.resend.is_empty(), "not resent while it may still come");
        let w = flow.window();
        // Past the timeout it is lost: resent first, and the window halves.
        let late = 2_000 + flow.rto_ms();
        s.on_ack(&ack(F, 1, vec![1], 4), &mut flow, late);
        assert_eq!(s.resend, vec![1]);
        assert_eq!(flow.window(), w / 2);
        assert_eq!(s.due(F, 1000, &mut flow, late).first(), Some(&1));
    }

    #[test]
    fn the_last_parts_time_out_though_no_ack_names_them() {
        let (mut s, mut flow) = (Sending::default(), Flow::default());
        let total = 3;
        assert_eq!(s.due(F, total, &mut flow, 0), vec![0, 1, 2]);
        s.on_ack(&ack(F, 2, vec![], 2), &mut flow, 1_000);
        assert!(s.due(F, total, &mut flow, 1_500).is_empty());
        let late = 1_000 + flow.rto_ms() + 1;
        let again = s.due(F, total, &mut flow, late);
        assert_eq!(again, vec![2]);
        s.on_ack(&ack(F, 3, vec![], 3), &mut flow, 60_000);
        assert!(s.done(total));
        assert_eq!(flow.in_flight(), 0);
    }

    #[test]
    fn one_window_for_all_files_to_a_recipient() {
        let (mut a, mut b, mut flow) = (Sending::default(), Sending::default(), Flow::default());
        let g = [2; 16];
        let x = a.due(F, 100, &mut flow, 0).len();
        let y = b.due(g, 100, &mut flow, 0).len();
        assert_eq!(x + y, 4);
        assert_eq!(y, 0, "the second waits for room");
        a.on_ack(&ack(F, 2, vec![], 2), &mut flow, 3_000);
        assert!(!b.due(g, 100, &mut flow, 3_000).is_empty());
    }

    #[test]
    fn a_resent_part_does_not_measure_the_round_trip() {
        let (mut s, mut flow) = (Sending::default(), Flow::default());
        s.due(F, 1, &mut flow, 0);
        let late = flow.rto_ms();
        assert_eq!(s.due(F, 1, &mut flow, late), vec![0]);
        // Its ack could answer either copy.
        s.on_ack(&ack(F, 1, vec![], 1), &mut flow, late + 500);
        assert_eq!(flow.srtt_ms(), None);
    }

    #[test]
    fn the_timeout_follows_the_measured_round_trip() {
        let mut flow = Flow::default();
        assert_eq!(flow.rto_ms(), RTO_FIRST_MS);
        flow.sample(5_000.0);
        assert_eq!(flow.rto_ms(), 5_000 + 4 * 2_500);
        // Steady round trips: the margin shrinks, to the floor at most.
        for _ in 0..20 {
            flow.sample(5_000.0);
        }
        assert_eq!(flow.rto_ms(), RTO_MIN_MS);
        // A fast path still waits the floor, a slow one no more than the ceiling.
        let mut fast = Flow::default();
        fast.sample(100.0);
        assert_eq!(fast.rto_ms(), RTO_MIN_MS);
        let mut slow = Flow::default();
        slow.sample(200_000.0);
        assert_eq!(slow.rto_ms(), RTO_MAX_MS);
    }

    #[test]
    fn several_losses_in_one_round_trip_halve_the_window_once() {
        let mut flow = Flow::default();
        flow.cwnd = 16.0;
        flow.sample(5_000.0);
        flow.on_loss(100_000);
        flow.on_loss(101_000);
        assert_eq!(flow.window(), 8);
        flow.on_loss(100_000 + 6_000);
        assert_eq!(flow.window(), 4);
    }

    #[test]
    fn the_window_never_passes_its_cap() {
        let (mut s, mut flow) = (Sending::default(), Flow::default());
        let mut now = 0;
        while s.acked < 500 {
            let d = s.due(F, 500, &mut flow, now);
            assert!(flow.in_flight() <= WINDOW);
            now += 1_000;
            let top = d.iter().max().map_or(s.acked, |t| t + 1);
            s.on_ack(&ack(F, top, vec![], top), &mut flow, now);
        }
        assert_eq!(flow.window(), WINDOW);
    }

    #[test]
    fn after_a_restart_what_was_out_is_in_flight_again() {
        // The database says 0..10 went and 0..4 arrived; this run knows nothing.
        let mut s = Sending { next: 10, acked: 4, resend: vec![] };
        let mut flow = Flow::default();
        assert!(s.due(F, 100, &mut flow, 0).is_empty(), "six out already fill the window of four");
        assert_eq!(flow.in_flight(), 6);
        let again = s.due(F, 100, &mut flow, RTO_FIRST_MS);
        assert_eq!(again, vec![4, 5], "they time out like any other, and the window halves");
        assert_eq!(flow.in_flight(), 2);
    }

    #[test]
    fn rewind_sends_what_is_out_again() {
        let (mut s, mut flow) = (Sending::default(), Flow::default());
        s.due(F, 100, &mut flow, 0);
        s.rewind(F, &mut flow);
        assert_eq!(flow.in_flight(), 0);
        assert_eq!(s.due(F, 100, &mut flow, 1), vec![0, 1, 2, 3]);
    }
}
