//! What a node holds for others.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use crate::crypto::{sha256, DhtKey};
use crate::items::delete_hash;
use crate::proto::{StoredItem, MAX_VALUE_BYTES};

#[derive(Clone, Debug)]
pub struct StoreLimits {
    pub max_total_bytes: usize,
    pub max_bytes_per_key: usize,
    pub max_items_per_key: usize,
    /// Longest lifetime a node grants, whatever the item asks for.
    pub max_ttl_ms: u64,
}

impl Default for StoreLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 64 * 1024 * 1024,
            max_bytes_per_key: 16 * 1024 * 1024,
            max_items_per_key: 64,
            max_ttl_ms: crate::items::MAIL_TTL_MS,
        }
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("item too large")]
    TooLarge,
    #[error("already expired")]
    Expired,
    #[error("store failed: {0}")]
    Backend(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub items: usize,
    pub bytes: usize,
}

/// Storage for items held on behalf of others. The app and the agent keep
/// them in their database so they outlive a restart; tests keep them in
/// memory.
pub trait Storage: Send + Sync + 'static {
    /// Store an item, evicting the oldest under the same key and then the
    /// oldest overall to stay within limits. Storing an identical value again
    /// is a no-op.
    fn put(&self, item: StoredItem, now_ms: u64) -> Result<(), StoreError>;
    /// Live items under `key`, oldest first.
    fn get(&self, key: &DhtKey, now_ms: u64) -> Vec<StoredItem>;
    /// Remove the items under `key` whose delete hash `token` matches.
    fn delete(&self, key: &DhtKey, token: &[u8; 32]) -> u32;
    fn gc(&self, now_ms: u64);
    /// Every live item, for handing on to the nodes now closest to it.
    fn all(&self, now_ms: u64) -> Vec<StoredItem>;
    fn stats(&self) -> StoreStats;
}

struct Entry {
    item: StoredItem,
    seq: u64,
}

#[derive(Default)]
struct Inner {
    by_key: HashMap<DhtKey, Vec<u64>>,
    entries: BTreeMap<u64, Entry>,
    bytes: usize,
    next: u64,
}

impl Inner {
    fn remove(&mut self, seq: u64) {
        if let Some(e) = self.entries.remove(&seq) {
            self.bytes -= e.item.value.len();
            if let Some(list) = self.by_key.get_mut(&e.item.key) {
                list.retain(|s| *s != seq);
                if list.is_empty() {
                    self.by_key.remove(&e.item.key);
                }
            }
        }
    }
}

pub struct MemStorage {
    limits: StoreLimits,
    inner: Mutex<Inner>,
}

impl MemStorage {
    pub fn new(limits: StoreLimits) -> Self {
        Self { limits, inner: Mutex::default() }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Validate and clamp an item before any storage takes it; shared by every
/// backend so they agree on what a node accepts.
pub fn admit(mut item: StoredItem, limits: &StoreLimits, now_ms: u64) -> Result<StoredItem, StoreError> {
    if item.value.len() > MAX_VALUE_BYTES || item.value.len() > limits.max_bytes_per_key {
        return Err(StoreError::TooLarge);
    }
    if item.expires_at_ms <= now_ms {
        return Err(StoreError::Expired);
    }
    item.expires_at_ms = item.expires_at_ms.min(now_ms + limits.max_ttl_ms);
    Ok(item)
}

impl Storage for MemStorage {
    fn put(&self, item: StoredItem, now_ms: u64) -> Result<(), StoreError> {
        let item = admit(item, &self.limits, now_ms)?;
        let mut g = self.lock();
        let value_hash = sha256(&[&item.value]);
        let existing: Vec<u64> = g.by_key.get(&item.key).cloned().unwrap_or_default();
        if existing.iter().any(|s| g.entries.get(s).is_some_and(|e| sha256(&[&e.item.value]) == value_hash)) {
            return Ok(());
        }
        let len = item.value.len();
        // Oldest under this key first, so one key cannot crowd out the rest.
        let mut under_key: Vec<u64> = existing;
        let mut key_bytes: usize = under_key.iter().filter_map(|s| g.entries.get(s)).map(|e| e.item.value.len()).sum();
        while !under_key.is_empty()
            && (under_key.len() >= self.limits.max_items_per_key || key_bytes + len > self.limits.max_bytes_per_key)
        {
            let oldest = under_key.remove(0);
            key_bytes -= g.entries.get(&oldest).map_or(0, |e| e.item.value.len());
            g.remove(oldest);
        }
        while g.bytes + len > self.limits.max_total_bytes {
            let Some((&oldest, _)) = g.entries.iter().next() else { break };
            g.remove(oldest);
        }
        let seq = g.next;
        g.next += 1;
        g.bytes += len;
        g.by_key.entry(item.key).or_default().push(seq);
        g.entries.insert(seq, Entry { item, seq });
        Ok(())
    }

    fn get(&self, key: &DhtKey, now_ms: u64) -> Vec<StoredItem> {
        let g = self.lock();
        g.by_key.get(key).into_iter().flatten()
            .filter_map(|s| g.entries.get(s))
            .filter(|e| e.item.expires_at_ms > now_ms)
            .map(|e| e.item.clone())
            .collect()
    }

    fn delete(&self, key: &DhtKey, token: &[u8; 32]) -> u32 {
        let wanted = delete_hash(token);
        let mut g = self.lock();
        let matching: Vec<u64> = g.by_key.get(key).into_iter().flatten()
            .filter(|s| g.entries.get(s).is_some_and(|e| e.item.delete_hash == Some(wanted)))
            .copied()
            .collect();
        for s in &matching {
            g.remove(*s);
        }
        matching.len() as u32
    }

    fn gc(&self, now_ms: u64) {
        let mut g = self.lock();
        let expired: Vec<u64> = g.entries.values().filter(|e| e.item.expires_at_ms <= now_ms).map(|e| e.seq).collect();
        for s in expired {
            g.remove(s);
        }
    }

    fn all(&self, now_ms: u64) -> Vec<StoredItem> {
        self.lock().entries.values().filter(|e| e.item.expires_at_ms > now_ms).map(|e| e.item.clone()).collect()
    }

    fn stats(&self) -> StoreStats {
        let g = self.lock();
        StoreStats { items: g.entries.len(), bytes: g.bytes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: u8, value: &[u8], expires: u64) -> StoredItem {
        StoredItem { key: [key; 32], value: value.to_vec(), delete_hash: None, expires_at_ms: expires }
    }

    fn limits() -> StoreLimits {
        StoreLimits { max_total_bytes: 100, max_bytes_per_key: 60, max_items_per_key: 3, max_ttl_ms: 1000 }
    }

    #[test]
    fn identical_values_are_stored_once() {
        let s = MemStorage::new(limits());
        s.put(item(1, b"abc", 500), 0).unwrap();
        s.put(item(1, b"abc", 500), 0).unwrap();
        assert_eq!(s.get(&[1; 32], 0).len(), 1);
    }

    #[test]
    fn a_full_key_drops_its_own_oldest_and_a_full_store_the_oldest_overall() {
        let s = MemStorage::new(limits());
        for v in [b"a1", b"a2", b"a3", b"a4"] {
            s.put(item(1, v, 500), 0).unwrap();
        }
        let values: Vec<Vec<u8>> = s.get(&[1; 32], 0).into_iter().map(|i| i.value).collect();
        assert_eq!(values, vec![b"a2".to_vec(), b"a3".to_vec(), b"a4".to_vec()]);

        s.put(item(2, &[0; 50], 500), 0).unwrap();
        s.put(item(3, &[1; 50], 500), 0).unwrap();
        assert!(s.stats().bytes <= 100);
        assert!(s.get(&[1; 32], 0).is_empty(), "key 1 was the oldest");
        assert_eq!(s.get(&[3; 32], 0).len(), 1);
    }

    #[test]
    fn lifetimes_are_clamped_and_expired_items_vanish() {
        let s = MemStorage::new(limits());
        s.put(item(1, b"x", 1_000_000), 10).unwrap();
        assert_eq!(s.get(&[1; 32], 10)[0].expires_at_ms, 1010);
        assert!(s.get(&[1; 32], 1010).is_empty());
        s.gc(1010);
        assert_eq!(s.stats(), StoreStats::default());
        assert_eq!(s.put(item(1, b"x", 5), 10), Err(StoreError::Expired));
        assert_eq!(s.put(item(1, &[0; 61], 50), 10), Err(StoreError::TooLarge));
    }

    #[test]
    fn only_the_token_deletes() {
        let s = MemStorage::new(limits());
        let token = [7; 32];
        let mut it = item(1, b"mail", 500);
        it.delete_hash = Some(delete_hash(&token));
        s.put(it, 0).unwrap();
        s.put(item(1, b"record", 500), 0).unwrap();
        assert_eq!(s.delete(&[1; 32], &[8; 32]), 0);
        assert_eq!(s.delete(&[1; 32], &delete_hash(&token)), 0, "the hash itself is not the token");
        assert_eq!(s.delete(&[1; 32], &token), 1);
        assert_eq!(s.get(&[1; 32], 0).len(), 1);
    }
}
