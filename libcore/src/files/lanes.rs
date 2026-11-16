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

fn configured_limit() -> Option<usize> {
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
