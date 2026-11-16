//! Unique probe accounting and explicitly defined latency statistics.

use serde::Serialize;
use std::time::{Duration, Instant};

pub(super) const MAX_PROBES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Pending,
    Received,
    TimedOut,
    Interrupted,
    SendError,
}

#[derive(Serialize)]
pub(super) struct Sample {
    id: usize,
    sent_offset_ms: u64,
    queued: bool,
    enqueue_completed: bool,
    enqueue_error: bool,
    outcome: Outcome,
    rtt_ms: Option<u64>,
    late_rtt_ms: Option<u64>,
    #[serde(skip)]
    sent_at: Instant,
}

#[derive(Default, Serialize)]
pub(super) struct Percentiles {
    p50: Option<u64>,
    p95: Option<u64>,
    p99: Option<u64>,
}

#[derive(Serialize)]
pub(super) struct Summary<'a> {
    attempted: usize,
    sent: usize,
    received: usize,
    lost: usize,
    timed_out: usize,
    interrupted: usize,
    send_errors: usize,
    enqueue_outcome_unknown: usize,
    late: usize,
    duplicates: usize,
    unexpected_echoes: usize,
    skipped_in_flight_limit: usize,
    total_budget: usize,
    budget_exhausted: bool,
    stopped_on_timeout: bool,
    rtt_ms: Percentiles,
    /// Mean absolute difference between consecutive accepted RTTs, in arrival order.
    jitter_ms: Option<f64>,
    samples: &'a [Sample],
}

pub(super) struct Probes {
    origin: Instant,
    timeout: Duration,
    limit: usize,
    samples: Vec<Sample>,
    arrival_rtts: Vec<u64>,
    duplicates: usize,
    unexpected_echoes: usize,
    skipped: usize,
    budget_exhausted: bool,
    stopped_on_timeout: bool,
}

impl Probes {
    pub fn new(origin: Instant, timeout: Duration, limit: usize) -> Self {
        Self {
            origin,
            timeout,
            limit,
            samples: vec![],
            arrival_rtts: vec![],
            duplicates: 0,
            unexpected_echoes: 0,
            skipped: 0,
            budget_exhausted: false,
            stopped_on_timeout: false,
        }
    }

    pub fn expire(&mut self, now: Instant) {
        for sample in &mut self.samples {
            if sample.outcome == Outcome::Pending
                && now.duration_since(sample.sent_at) >= self.timeout
            {
                sample.outcome = Outcome::TimedOut;
                self.stopped_on_timeout = true;
            }
        }
    }

    pub fn pending(&self) -> usize {
        self.samples
            .iter()
            .filter(|s| s.outcome == Outcome::Pending)
            .count()
    }

    pub fn begin(&mut self, now: Instant) -> Option<usize> {
        self.expire(now);
        if self.stopped_on_timeout {
            return None;
        }
        if self.samples.len() >= MAX_PROBES {
            self.budget_exhausted = true;
            return None;
        }
        if self.pending() >= self.limit {
            self.skipped += 1;
            return None;
        }
        let id = self.samples.len() + 1;
        self.samples.push(Sample {
            id,
            sent_offset_ms: millis(now.duration_since(self.origin)),
            queued: false,
            enqueue_completed: false,
            enqueue_error: false,
            outcome: Outcome::Pending,
            rtt_ms: None,
            late_rtt_ms: None,
            sent_at: now,
        });
        Some(id)
    }

    pub fn sent(&mut self, id: usize, queued: bool) {
        let sample = &mut self.samples[id - 1];
        sample.enqueue_completed = true;
        sample.queued |= queued;
        if !queued {
            sample.enqueue_error = true;
            if sample.outcome == Outcome::Pending {
                sample.outcome = Outcome::SendError;
            }
        }
    }

    pub fn receive(&mut self, id: usize, now: Instant) {
        self.expire(now);
        let Some(sample) = id.checked_sub(1).and_then(|n| self.samples.get_mut(n)) else {
            self.unexpected_echoes += 1;
            return;
        };
        let rtt = millis(now.duration_since(sample.sent_at));
        match sample.outcome {
            Outcome::Pending => {
                // The event and the enqueue worker's completion use different
                // queues. An echo can be consumed before that completion.
                sample.queued = true;
                sample.outcome = Outcome::Received;
                sample.rtt_ms = Some(rtt);
                self.arrival_rtts.push(rtt);
            }
            Outcome::TimedOut | Outcome::Interrupted | Outcome::SendError
                if sample.late_rtt_ms.is_none() =>
            {
                sample.late_rtt_ms = Some(rtt);
            }
            _ => self.duplicates += 1,
        }
    }

    pub fn finish(&mut self, now: Instant) {
        self.expire(now);
        for sample in &mut self.samples {
            if sample.outcome == Outcome::Pending {
                sample.outcome = Outcome::Interrupted;
            }
        }
    }

    pub fn summary(&self) -> Summary<'_> {
        let count = |outcome| self.samples.iter().filter(|s| s.outcome == outcome).count();
        let sent = self.samples.iter().filter(|s| s.queued).count();
        let received = count(Outcome::Received);
        let mut sorted = self.arrival_rtts.clone();
        sorted.sort_unstable();
        let jitter_ms = (self.arrival_rtts.len() > 1).then(|| {
            self.arrival_rtts
                .windows(2)
                .map(|p| p[0].abs_diff(p[1]) as f64)
                .sum::<f64>()
                / (self.arrival_rtts.len() - 1) as f64
        });
        Summary {
            attempted: self.samples.len(),
            sent,
            received,
            lost: sent.saturating_sub(received),
            timed_out: count(Outcome::TimedOut),
            interrupted: count(Outcome::Interrupted),
            send_errors: self.samples.iter().filter(|s| s.enqueue_error).count(),
            enqueue_outcome_unknown: self
                .samples
                .iter()
                .filter(|s| !s.enqueue_completed && !s.queued)
                .count(),
            late: self
                .samples
                .iter()
                .filter(|s| s.late_rtt_ms.is_some())
                .count(),
            duplicates: self.duplicates,
            unexpected_echoes: self.unexpected_echoes,
            skipped_in_flight_limit: self.skipped,
            total_budget: MAX_PROBES,
            budget_exhausted: self.budget_exhausted,
            stopped_on_timeout: self.stopped_on_timeout,
            rtt_ms: Percentiles {
                p50: percentile(&sorted, 50),
                p95: percentile(&sorted, 95),
                p99: percentile(&sorted, 99),
            },
            jitter_ms,
            samples: &self.samples,
        }
    }

    pub fn successful(&self) -> bool {
        !self.samples.is_empty()
            && self.samples.iter().all(|s| s.outcome == Outcome::Received)
            && self.unexpected_echoes == 0
            && !self.samples.iter().any(|s| s.enqueue_error)
            && !self.budget_exhausted
    }
}

pub(super) fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

/// Nearest rank: the first sample at or above p% of the ordered population.
fn percentile(sorted: &[u64], percent: usize) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    sorted
        .get((sorted.len() * percent).div_ceil(100).saturating_sub(1))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_and_jitter_use_unique_on_time_echoes_only() {
        let t = Instant::now();
        let mut p = Probes::new(t, Duration::from_secs(10), 4);
        for (i, rtt) in [100, 200, 150, 400].into_iter().enumerate() {
            let sent = t + Duration::from_secs(i as u64);
            let id = p.begin(sent).unwrap();
            p.sent(id, true);
            p.receive(id, sent + Duration::from_millis(rtt));
        }
        p.receive(4, t + Duration::from_millis(3500));
        let s = p.summary();
        assert_eq!((s.sent, s.received, s.duplicates), (4, 4, 1));
        assert_eq!(
            (s.rtt_ms.p50, s.rtt_ms.p95, s.rtt_ms.p99),
            (Some(150), Some(400), Some(400))
        );
        assert_eq!(s.jitter_ms, Some(400.0 / 3.0));
    }

    #[test]
    fn full_window_stops_production_after_timeout_and_keeps_late_echoes_visible() {
        let t = Instant::now();
        let mut p = Probes::new(t, Duration::from_secs(1), 1);
        let id = p.begin(t).unwrap();
        p.sent(id, true);
        assert_eq!(p.begin(t + Duration::from_millis(500)), None);
        p.receive(id, t + Duration::from_secs(1)); // Exactly the deadline is late.
        p.receive(id, t + Duration::from_secs(2));
        assert_eq!(p.begin(t + Duration::from_secs(2)), None);
        p.receive(999, t + Duration::from_secs(2));
        let s = p.summary();
        assert_eq!(
            (
                s.sent,
                s.received,
                s.lost,
                s.timed_out,
                s.late,
                s.send_errors
            ),
            (1, 0, 1, 1, 1, 0)
        );
        assert_eq!(
            (s.skipped_in_flight_limit, s.duplicates, s.unexpected_echoes),
            (1, 1, 1)
        );
        assert_eq!(s.rtt_ms.p99, None);
        assert_eq!(s.jitter_ms, None);
        assert!(!p.successful());
        assert!(s.stopped_on_timeout);
    }

    #[test]
    fn enqueue_result_racing_with_echo_preserves_both_receipt_and_failure() {
        let t = Instant::now();
        let mut p = Probes::new(t, Duration::from_secs(10), 1);
        let id = p.begin(t).unwrap();
        p.receive(id, t + Duration::from_millis(20));
        p.sent(id, true);
        assert_eq!(p.summary().received, 1);
        assert_eq!(p.summary().duplicates, 0);
        let id = p.begin(t + Duration::from_secs(1)).unwrap();
        p.sent(id, false);
        assert_eq!(p.summary().send_errors, 1);
        assert!(!p.successful());
    }

    #[test]
    fn successful_fast_echoes_cannot_exceed_the_total_probe_budget() {
        let t = Instant::now();
        let mut p = Probes::new(t, Duration::from_secs(10), 1);
        for i in 0..MAX_PROBES {
            let now = t + Duration::from_secs(i as u64);
            let id = p.begin(now).unwrap();
            p.sent(id, true);
            p.receive(id, now + Duration::from_millis(1));
        }
        assert_eq!(p.begin(t + Duration::from_secs(MAX_PROBES as u64)), None);
        assert_eq!(p.summary().attempted, MAX_PROBES);
        assert!(p.summary().budget_exhausted);
        assert!(!p.successful());
    }

    #[test]
    fn run_end_does_not_silently_drop_pending_samples() {
        let t = Instant::now();
        let mut p = Probes::new(t, Duration::from_secs(30), 1);
        let id = p.begin(t).unwrap();
        p.sent(id, true);
        p.finish(t + Duration::from_secs(1));
        assert_eq!(p.summary().interrupted, 1);
        assert_eq!(p.summary().lost, 1);
        assert!(!p.successful());
        assert!(serde_json::to_string(&p.summary())
            .unwrap()
            .contains("\"p50\":null"));
    }

    #[test]
    fn interrupted_enqueue_has_an_unknown_outcome_instead_of_assuming_packet_loss() {
        let t = Instant::now();
        let mut p = Probes::new(t, Duration::from_secs(30), 1);
        p.begin(t).unwrap();
        p.finish(t + Duration::from_secs(1));
        assert_eq!(p.summary().interrupted, 1);
        assert_eq!(p.summary().enqueue_outcome_unknown, 1);
        assert_eq!(p.summary().lost, 0);
        assert!(!p.successful());
    }
}
