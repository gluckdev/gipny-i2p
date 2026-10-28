//! Standalone rustc probe: exercise the production policy without linking i2pd.
#[path = "../src/diagnostics.rs"]
mod diagnostics;

fn main() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let observed = AtomicUsize::new(0);
    debug_trace!("GIPNY_PRIVATE_TRACE_SENTINEL_20261028 {}", observed.fetch_add(1, Ordering::SeqCst));
    let available = cfg!(debug_assertions);
    assert_eq!(diagnostics::AVAILABLE, available);
    assert_eq!(observed.load(Ordering::SeqCst), usize::from(available));
    for persisted in [None, Some("debug"), Some("off")] {
        assert_eq!(diagnostics::capture_enabled(Some("1"), persisted), available);
    }
    assert!(!diagnostics::capture_enabled(Some("0"), Some("debug")));
    assert_eq!(diagnostics::router_log_path(Some("i2pd.log")).is_some(), available);
    let options = vec!["--loglevel=debug".into(), "--loglevel".into(), "info".into()];
    assert_eq!(diagnostics::router_options(&options, Some("debug")),
        [if available { "--loglevel=debug" } else { "--loglevel=warn" }]);
}
