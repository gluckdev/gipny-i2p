//! One fragmented ciphertext per authenticated stream. Incomplete uploads
//! reserve their final allocation from a shared budget and never reach storage.
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const PART_BYTES: usize = 256 * 1024;
// Ordinary payloads allow 4 MiB before ratchet headers, padding and encoding.
pub const MAX_BYTES: usize = 4 * 1024 * 1024 + 4096;
pub const TIME_LIMIT: Duration = Duration::from_secs(120);
// An existing relay can hold any opaque legacy blob from a 16 MiB wire frame.
const MAX_PUSH: usize = 16 * 1024 * 1024 + 4;
const RAM_BUDGET: usize = 64 * 1024 * 1024;
const INVALID: &str = "invalid or expired fragmented deposit";

pub fn parts(blob: &[u8]) -> Result<impl ExactSizeIterator<Item = &[u8]>, &'static str> {
    split(blob, MAX_BYTES)
}

fn split(blob: &[u8], limit: usize) -> Result<impl ExactSizeIterator<Item = &[u8]>, &'static str> {
    if !(2..=limit).contains(&blob.len()) {
        return Err(INVALID);
    }
    let count = blob.len().div_ceil(PART_BYTES).max(2);
    Ok(blob.chunks(blob.len().div_ceil(count)))
}

/// A length prefix gives even an empty legacy item two nonempty fragments.
/// Build just the next wire fragment, without copying the whole backlog item.
pub fn push_parts(
    blob: &[u8],
) -> Result<impl ExactSizeIterator<Item = Vec<u8>> + '_, &'static str> {
    if blob.len() > MAX_PUSH - 4 {
        return Err(INVALID);
    }
    let prefix = (blob.len() as u32).to_be_bytes();
    let len = prefix.len() + blob.len();
    let count = len.div_ceil(PART_BYTES).max(2);
    let width = len.div_ceil(count);
    Ok((0..count).map(move |index| {
        let start = index * width;
        let end = (start + width).min(len);
        let mut part = Vec::with_capacity(end - start);
        if start < prefix.len() {
            part.extend_from_slice(&prefix[start..end.min(prefix.len())]);
        }
        if end > prefix.len() {
            part.extend_from_slice(&blob[start.saturating_sub(prefix.len())..end - prefix.len()]);
        }
        part
    }))
}

struct Upload {
    to: [u8; 32],
    total: u16,
    next: u16,
    width: usize,
    deadline: Instant,
    bytes: Vec<u8>,
    _permit: OwnedSemaphorePermit,
}

#[derive(Default)]
pub struct Assembly(Option<Upload>);

impl Assembly {
    pub fn deadline(&self) -> Option<Instant> {
        self.0.as_ref().map(|upload| upload.deadline)
    }

    /// The stream is closed on error. Only a complete validated set returns a blob.
    pub fn push(
        &mut self,
        to: [u8; 32],
        index: u16,
        total: u16,
        part: &[u8],
        now: Instant,
    ) -> Result<Option<Vec<u8>>, &'static str> {
        self.add(to, index, total, part, now, MAX_BYTES)
    }

    pub fn receive_push(
        &mut self,
        from: [u8; 32],
        index: u16,
        total: u16,
        part: &[u8],
        now: Instant,
    ) -> Result<Option<Vec<u8>>, &'static str> {
        let Some(mut tagged) = self.add(from, index, total, part, now, MAX_PUSH)? else {
            return Ok(None);
        };
        let len = u32::from_be_bytes(
            tagged
                .get(..4)
                .ok_or(INVALID)?
                .try_into()
                .map_err(|_| INVALID)?,
        ) as usize;
        if len != tagged.len() - 4 {
            return Err(INVALID);
        }
        tagged.drain(..4);
        Ok(Some(tagged))
    }

    fn add(
        &mut self,
        to: [u8; 32],
        index: u16,
        total: u16,
        part: &[u8],
        now: Instant,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, &'static str> {
        if total < 2
            || total as usize > limit.div_ceil(PART_BYTES)
            || index >= total
            || part.is_empty()
            || part.len() > PART_BYTES
        {
            return Err(INVALID);
        }
        if self.0.is_none() {
            if index != 0 || part.len() * (usize::from(total) - 1) >= limit {
                return Err(INVALID);
            }
            static BUDGET: OnceLock<Arc<Semaphore>> = OnceLock::new();
            let cap = (part.len() * usize::from(total)).min(limit);
            let permit = BUDGET
                .get_or_init(|| Arc::new(Semaphore::new(RAM_BUDGET)))
                .clone()
                .try_acquire_many_owned(cap as u32)
                .map_err(|_| "fragmented deposit memory limit")?;
            self.0 = Some(Upload {
                to,
                total,
                next: 0,
                width: part.len(),
                deadline: now + TIME_LIMIT,
                bytes: Vec::with_capacity(cap),
                _permit: permit,
            });
        }
        let upload = self.0.as_mut().ok_or(INVALID)?;
        let last = index + 1 == total;
        if now >= upload.deadline
            || to != upload.to
            || index != upload.next
            || total != upload.total
            || part.len() > upload.width
            || (!last && part.len() != upload.width)
            || upload.bytes.len() + part.len() > limit
        {
            return Err(INVALID);
        }
        upload.next += 1;
        upload.bytes.extend_from_slice(part);
        Ok(if last {
            self.0.take().map(|done| done.bytes)
        } else {
            None
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_to_back_parts_bind_the_peer_and_expire_as_a_unit() {
        let now = Instant::now();
        let peer = [1; 32];
        let begin = || {
            let mut assembly = Assembly::default();
            assert_eq!(assembly.push(peer, 0, 3, &[1, 2], now).unwrap(), None);
            assembly
        };
        assert!(Assembly::default().push(peer, 1, 2, &[1], now).is_err());
        assert!(begin().push(peer, 1, 3, &[3], now).is_err());
        assert!(begin().push([2; 32], 1, 3, &[3, 4], now).is_err());
        assert!(begin().push(peer, 2, 3, &[3, 4], now).is_err());
        assert!(begin().push(peer, 1, 2, &[3, 4], now).is_err());
        assert!(begin().push(peer, 1, 3, &[3, 4], now + TIME_LIMIT).is_err());
        assert!(Assembly::default().push(peer, 0, 18, &[1], now).is_err());
        assert!(parts(&[]).is_err());
        assert!(parts(&[1]).is_err());
        assert_eq!(parts(&[1, 2]).unwrap().len(), 2);
        let mut valid = begin();
        assert_eq!(valid.push(peer, 1, 3, &[3, 4], now).unwrap(), None);
        assert_eq!(
            valid.push(peer, 2, 3, &[5], now).unwrap(),
            Some(vec![1, 2, 3, 4, 5])
        );
        let mut push = Assembly::default();
        assert!(push
            .receive_push(peer, 0, 2, &[0, 0], now)
            .unwrap()
            .is_none());
        assert!(
            push.receive_push(peer, 1, 2, &[0, 1], now).is_err(),
            "invalid legacy length prefix"
        );
    }
}
