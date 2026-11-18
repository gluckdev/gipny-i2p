//! Application streams sharing the existing I2P destination and tunnel pool.
//! The measurement override changes concurrency, never hops or padding.

use std::sync::OnceLock;

pub const MAX_LANES: usize = 8;
const DEFAULT_LANES: usize = 4;
const PARTS_PER_LANE: u32 = 4;

fn measured_limit(raw: Option<&str>) -> Option<usize> {
    raw?.trim()
        .parse()
        .ok()
        .filter(|n| (1..=MAX_LANES).contains(n))
}

pub(super) fn configured_limit() -> Option<usize> {
    static LIMIT: OnceLock<Option<usize>> = OnceLock::new();
    *LIMIT.get_or_init(|| measured_limit(std::env::var("GIPNY_FILE_LANES").ok().as_deref()))
}

/// Includes the main stream. Read once per process to keep an experiment fixed.
/// Unset/invalid overrides retain the existing four-stream policy.
pub fn limit() -> usize {
    configured_limit().unwrap_or(DEFAULT_LANES)
}

fn desired_for(window: u32, measured: Option<usize>) -> usize {
    match measured {
        // A measured sweep needs to reach 5..8 streams without changing the
        // 16-part flight budget. Never allocate more streams than flight slots.
        Some(limit) => (window as usize).clamp(1, limit),
        None => (window.div_ceil(PARTS_PER_LANE) as usize).clamp(1, DEFAULT_LANES),
    }
}

pub fn desired(window: u32) -> usize {
    desired_for(window, configured_limit())
}

/// Per-contact probe. Compare newly acknowledged parts per millisecond over
/// full flights, and only compare samples taken with the requested lanes open.
#[derive(Clone, Debug)]
pub(super) struct Adaptive {
    target: usize,
    active: usize,
    start: Option<i64>,
    parts: usize,
    baseline: Option<f64>,
    cooldown_until: i64,
}

impl Default for Adaptive {
    fn default() -> Self {
        Self {
            target: 1,
            active: 0,
            start: None,
            parts: 0,
            baseline: None,
            cooldown_until: 0,
        }
    }
}

impl Adaptive {
    pub(super) fn desired(&self, window: u32) -> usize {
        match configured_limit() {
            Some(limit) => desired_for(window, Some(limit)),
            None => self.target.min(desired_for(window, None)),
        }
    }

    pub(super) fn using(&mut self, active: usize) {
        if self.active != active {
            self.active = active;
            self.start = None;
        }
    }

    pub(super) fn loss(&mut self, now: i64) {
        self.target = (self.active.min(self.target) / 2).max(1);
        self.baseline = None;
        self.start = None;
        self.cooldown_until = now.saturating_add(60_000);
    }

    pub(super) fn ack(&mut self, now: i64, parts: usize, loaded: bool) {
        if parts == 0 {
            return;
        }
        if !loaded || self.active != self.target || now < self.cooldown_until {
            self.start = None;
            return;
        }
        let Some(start) = self.start else {
            self.start = Some(now);
            self.parts = 0;
            return;
        };
        let elapsed = now.saturating_sub(start);
        if !(0..=180_000).contains(&elapsed) {
            self.start = None;
            self.baseline = None;
            return;
        }
        self.parts += parts;
        if self.parts < 16 || elapsed < 8_000 {
            return;
        }
        let rate = self.parts as f64 / elapsed as f64;
        self.start = None;
        if let Some(previous) = self.baseline.take() {
            if rate < previous * 1.10 {
                self.target = self.target.saturating_sub(1).max(1);
                self.cooldown_until = now.saturating_add(60_000);
            }
        } else if self.target < DEFAULT_LANES {
            self.baseline = Some(rate);
            self.target += 1;
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Cursor(usize);

impl Cursor {
    /// Retain position across send batches, including one-part ACK-driven ones.
    pub(super) fn next(&mut self, count: usize) -> usize {
        assert!(count > 0, "the main file stream must exist");
        let selected = self.0 % count;
        self.0 = (selected + 1) % count;
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measurement_configuration_is_bounded_and_does_not_change_default() {
        for raw in [
            None,
            Some(""),
            Some("0"),
            Some("9"),
            Some("-1"),
            Some("NaN"),
        ] {
            assert_eq!(measured_limit(raw), None);
        }
        for limit in 1..=MAX_LANES {
            assert_eq!(measured_limit(Some(&limit.to_string())), Some(limit));
            assert_eq!(desired_for(16, Some(limit)), limit);
        }
        assert_eq!(
            (1..=16).map(|w| desired_for(w, None)).collect::<Vec<_>>(),
            vec![1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4]
        );
        assert_eq!(desired_for(4, Some(8)), 4);
    }

    #[test]
    fn probe_uses_connected_lanes_and_keeps_only_a_measured_gain() {
        let mut lanes = Adaptive::default();
        lanes.using(1);
        lanes.ack(0, 1, true);
        lanes.ack(8_000, 16, true);
        assert_eq!(lanes.target, 2);
        lanes.ack(9_000, 16, true);
        lanes.ack(18_000, 16, true);
        assert_eq!(lanes.target, 2);
        lanes.using(2);
        lanes.ack(19_000, 1, true);
        lanes.ack(27_000, 24, true);
        assert_eq!(lanes.target, 2);
        lanes.ack(28_000, 1, true);
        lanes.ack(36_000, 24, true);
        assert_eq!(lanes.target, 3);
        lanes.using(3);
        lanes.ack(37_000, 1, true);
        lanes.ack(45_000, 24, true);
        assert_eq!(lanes.target, 2);
        lanes.using(2);
        lanes.ack(46_000, 32, true);
        lanes.ack(54_000, 32, true);
        assert_eq!(lanes.target, 2);
        lanes.loss(55_000);
        assert_eq!(lanes.target, 1);
    }

    #[test]
    fn cursor_survives_batches_and_live_lane_count_changes() {
        let mut cursor = Cursor::default();
        let mut counts = [0; 8];
        // Each iteration represents a separate one-part ACK-driven pump.
        for _ in 0..24 {
            counts[cursor.next(8)] += 1;
        }
        assert_eq!(counts, [3; 8]);
        assert_eq!(cursor.next(8), 0);
        assert_eq!(cursor.next(8), 1);
        assert_eq!(cursor.next(1), 0);
        assert_eq!(
            (0..6).map(|_| cursor.next(3)).collect::<Vec<_>>(),
            vec![0, 1, 2, 0, 1, 2]
        );
    }
}
