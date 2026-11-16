//! Content-free, process-local file-transfer measurements.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct FileMetricsSnapshot {
    /// Bytes newly accepted by durable file storage, excluding duplicate parts.
    pub unique_bytes_received: u64,
    pub duplicate_parts_received: u64,
    /// Successful application enqueues, not network delivery acknowledgements.
    pub queued_parts: u64,
    pub queued_retransmissions: u64,
    /// Largest set of open send streams available to a successful part enqueue.
    /// Includes the main stream; this is not a count of streams proven busy.
    pub peak_send_lanes: usize,
    /// Successfully connected receive streams, including the main stream.
    pub peak_receive_lanes: usize,
}

#[derive(Default)]
pub(crate) struct FileMetrics {
    unique_bytes_received: AtomicU64,
    duplicate_parts_received: AtomicU64,
    queued_parts: AtomicU64,
    queued_retransmissions: AtomicU64,
    peak_send_lanes: AtomicUsize,
    peak_receive_lanes: AtomicUsize,
}

impl FileMetrics {
    pub(crate) fn received(&self, fresh: bool, bytes: usize) {
        self.receiving_lanes(1);
        if fresh {
            self.unique_bytes_received.fetch_add(bytes as u64, Relaxed);
        } else {
            self.duplicate_parts_received.fetch_add(1, Relaxed);
        }
    }

    pub(crate) fn queued(&self, retransmission: bool, lanes: usize) {
        self.queued_parts.fetch_add(1, Relaxed);
        if retransmission {
            self.queued_retransmissions.fetch_add(1, Relaxed);
        }
        self.peak_send_lanes.fetch_max(lanes, Relaxed);
    }

    pub(crate) fn receiving_lanes(&self, count: usize) {
        self.peak_receive_lanes.fetch_max(count, Relaxed);
    }

    pub(crate) fn snapshot(&self) -> FileMetricsSnapshot {
        // These independent diagnostic counters are not a transaction. Take a
        // final sample after completion when comparing bytes with the file hash.
        FileMetricsSnapshot {
            unique_bytes_received: self.unique_bytes_received.load(Relaxed),
            duplicate_parts_received: self.duplicate_parts_received.load(Relaxed),
            queued_parts: self.queued_parts.load(Relaxed),
            queued_retransmissions: self.queued_retransmissions.load(Relaxed),
            peak_send_lanes: self.peak_send_lanes.load(Relaxed),
            peak_receive_lanes: self.peak_receive_lanes.load(Relaxed),
        }
    }
}
