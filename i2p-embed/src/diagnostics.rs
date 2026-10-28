//! Compile-time diagnostics policy shared by the app and embedded router.
//! No environment variable or persisted setting can enable a release build.

pub const AVAILABLE: bool = cfg!(any(debug_assertions, test));

pub fn capture_enabled(environment: Option<&str>, persisted: Option<&str>) -> bool {
    capture_for(AVAILABLE, environment, persisted)
}

fn capture_for(available: bool, environment: Option<&str>, persisted: Option<&str>) -> bool {
    if !available { return false; }
    match environment.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("1" | "true" | "on" | "debug") => return true,
        Some("0" | "false" | "off") => return false,
        _ => {}
    }
    !persisted.is_some_and(|value| value.trim().eq_ignore_ascii_case("off"))
}

pub fn router_log_path(requested: Option<&str>) -> Option<&str> {
    if AVAILABLE { requested } else { None }
}

/// Normalize all log-level arguments before i2pd sees them, including the
/// split `--loglevel debug` spelling. Never pass duplicate Boost options.
pub fn router_options(options: &[String], environment: Option<&str>) -> Vec<String> {
    router_options_for(AVAILABLE, options, environment)
}

fn router_options_for(available: bool, options: &[String], environment: Option<&str>) -> Vec<String> {
    let mut out = Vec::with_capacity(options.len() + 1);
    let mut requested = None;
    let mut args = options.iter().peekable();
    while let Some(arg) = args.next() {
        if let Some(level) = arg.strip_prefix("--loglevel=") {
            requested = Some(level);
        } else if arg == "--loglevel" {
            if args.peek().is_some_and(|value| !value.starts_with('-')) {
                requested = args.next().map(String::as_str);
            }
        } else {
            out.push(arg.clone());
        }
    }
    let level = if available {
        environment.or(requested).filter(|level| matches!(*level, "critical" | "error" | "warn" | "info" | "debug")).unwrap_or("info")
    } else { "warn" };
    out.push(format!("--loglevel={level}"));
    out
}

/// Low-level app/router traces are absent from release code, including the
/// formatting arguments. Operational warnings/errors deliberately use stderr.
#[macro_export]
macro_rules! debug_trace {
    ($($argument:tt)*) => {{
        #[cfg(any(debug_assertions, test))]
        { eprintln!($($argument)*); }
        #[cfg(not(any(debug_assertions, test)))]
        { if false { let _ = format_args!($($argument)*); } }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_capture_cannot_be_enabled_by_legacy_settings_or_environment() {
        for environment in [None, Some("1"), Some("debug"), Some("true"), Some("0"), Some("invalid")] {
            for saved in [None, Some("debug"), Some("off"), Some("legacy")] {
                assert!(!capture_for(false, environment, saved));
            }
        }
    }

    #[test]
    fn developer_capture_defaults_on_and_honors_explicit_settings() {
        assert!(capture_for(true, None, None));
        assert!(capture_for(true, None, Some("debug")));
        assert!(!capture_for(true, None, Some(" OFF\n")));
        assert!(capture_for(true, Some("1"), Some("off")));
        assert!(!capture_for(true, Some("0"), Some("debug")));
        assert!(!capture_for(true, Some("false"), None));
        assert!(!capture_for(true, Some("unknown"), Some("off")));
    }

    #[test]
    fn router_release_forces_warning_level_and_normalizes_duplicate_flags() {
        let options = vec!["--datadir=example".into(), "--loglevel=debug".into(), "--loglevel".into(), "info".into(), "--notransit".into()];
        let release = router_options_for(false, &options, Some("debug"));
        assert_eq!(release, ["--datadir=example", "--notransit", "--loglevel=warn"]);
        assert_eq!(router_options_for(true, &options, Some("debug")).last().unwrap(), "--loglevel=debug");
        assert_eq!(router_options_for(true, &[], None), ["--loglevel=info"]);
        assert_eq!(router_options_for(true, &[], Some("invalid")), ["--loglevel=info"]);
    }

    #[test]
    fn developer_trace_evaluates_arguments_in_tests() {
        let observed = std::cell::Cell::new(false);
        debug_trace!("diagnostics test: {}", { observed.set(true); "enabled" });
        assert!(observed.get());
    }
}
