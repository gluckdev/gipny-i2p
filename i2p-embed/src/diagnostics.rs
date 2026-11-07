//! Compile-time diagnostics policy shared by the app and embedded router.
//! No runtime environment variable or persisted setting can enable a stable
//! release build. Only the compile-time `GIPNY_RELEASE_LOGS=full` does, and
//! release.yml sets it for prerelease versions (`0.5.0-rc1`) alone.

/// The developer log compiled into an optimised prerelease build for testers.
/// build-provenance.py refuses it in a stable version.
pub const RELEASE_LOGS: bool = release_logs_requested(option_env!("GIPNY_RELEASE_LOGS"));

pub const AVAILABLE: bool = cfg!(any(debug_assertions, test)) || RELEASE_LOGS;

const fn release_logs_requested(value: Option<&str>) -> bool {
    match value {
        Some(value) => matches!(value.as_bytes(), b"full"),
        None => false,
    }
}


/// Developer log level. Stable release builds pin to [`LogLevel::Null`];
/// developer and prerelease builds ([`AVAILABLE`]) pin to
/// [`LogLevel::Full`], the debug period: every action is
/// logged, each line stamped by the stderr capture (`HH:MM:SS.mmm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Log everything: every action, with a mandatory timestamp.
    Full,
    /// Logging off.
    Null,
}

impl LogLevel {
    pub const FULL: Self = Self::Full;
    pub const NULL: Self = Self::Null;

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Null => "null",
        }
    }

}

impl Default for LogLevel {
    fn default() -> Self {
        if AVAILABLE { Self::Full } else { Self::Null }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for LogLevel {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" | "trace" | "all" => Ok(Self::Full),
            "null" | "off" | "none" => Ok(Self::Null),
            _ => Err(format!("unknown log level: {value}")),
        }
    }
}

/// Immutable build policy: Full in developer/prerelease, Null in stable.
pub const fn current_level() -> LogLevel {
    if AVAILABLE { LogLevel::Full } else { LogLevel::Null }
}

pub const fn is_enabled() -> bool { AVAILABLE }

/// Legacy callers may supply settings, but runtime input cannot change policy.
pub fn capture_enabled(environment: Option<&str>, persisted: Option<&str>) -> bool {
    capture_for(AVAILABLE, environment, persisted)
}

fn capture_for(available: bool, _environment: Option<&str>, _persisted: Option<&str>) -> bool {
    available
}

pub fn router_log_path(requested: Option<&str>) -> Option<&str> {
    if AVAILABLE { requested } else { None }
}

/// Normalize all log-level arguments before i2pd sees them, including the
/// split `--loglevel debug` spelling. Never pass duplicate Boost options.
pub fn router_options(options: &[String], environment: Option<&str>) -> Vec<String> {
    router_options_for(AVAILABLE, options, environment)
}

fn router_options_for(available: bool, options: &[String], _environment: Option<&str>) -> Vec<String> {
    let mut out = Vec::with_capacity(options.len() + 1);
    let mut args = options.iter().peekable();
    while let Some(arg) = args.next() {
        if arg.starts_with("--loglevel=") {
        } else if arg == "--loglevel" {
            if args.peek().is_some_and(|value| !value.starts_with('-')) {
                args.next();
            }
        } else {
            out.push(arg.clone());
        }
    }
    let level = if available { "debug" } else { "warn" };
    out.push(format!("--loglevel={level}"));
    out
}

/// Low-level app/router traces are absent from stable release code, including
/// the formatting arguments: `AVAILABLE` is a constant `false` there and the
/// branch is removed at compile time. A `#[cfg]` here would be evaluated in the
/// calling crate and could not see `GIPNY_RELEASE_LOGS`.
#[macro_export]
macro_rules! debug_trace {
    ($($argument:tt)*) => {{
        if $crate::diagnostics::AVAILABLE && $crate::diagnostics::is_enabled() {
            eprintln!($($argument)*);
        }
    }};
}

/// Error-level logs of the action stream: emitted only while the developer
/// level allows output (the debug period, `Full`).
#[macro_export]
macro_rules! debug_error {
    ($($argument:tt)*) => {{
        if $crate::diagnostics::AVAILABLE && $crate::diagnostics::is_enabled() {
            eprintln!($($argument)*);
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn release_capture_cannot_be_enabled_by_legacy_settings_or_environment() {
        for environment in [None, Some("1"), Some("debug"), Some("true"), Some("0"), Some("invalid")] {
            for saved in [None, Some("debug"), Some("off"), Some("legacy")] {
                assert!(!capture_for(false, environment, saved));
            }
        }
    }

    #[test]
    fn prerelease_capture_cannot_be_disabled_by_runtime_settings() {
        for environment in [None,Some("0"),Some("off"),Some("false"),Some("unknown")] {
            for persisted in [None,Some("off"),Some("debug")] {
                assert!(capture_for(true,environment,persisted));
            }
        }
    }

    #[test]
    fn router_release_forces_warning_level_and_normalizes_duplicate_flags() {
        let options = vec!["--datadir=example".into(), "--loglevel=debug".into(), "--loglevel".into(), "info".into(), "--notransit".into()];
        let release = router_options_for(false, &options, Some("debug"));
        assert_eq!(release, ["--datadir=example", "--notransit", "--loglevel=warn"]);
        assert_eq!(router_options_for(true, &options, Some("debug")).last().unwrap(), "--loglevel=debug");
        assert_eq!(router_options_for(true, &[], None), ["--loglevel=debug"]);
        assert_eq!(router_options_for(true, &[], Some("invalid")), ["--loglevel=debug"]);
    }

    #[test]
    fn prerelease_log_switch_accepts_only_full() {
        assert!(release_logs_requested(Some("full")));
        for value in [None, Some(""), Some("null"), Some("FULL"), Some("1"), Some("full ")] {
            assert!(!release_logs_requested(value));
        }
    }

    #[test]
    fn developer_trace_evaluates_arguments_in_tests() {
        let observed = std::cell::Cell::new(false);
        debug_trace!("diagnostics test: {}", { observed.set(true); "enabled" });
        assert!(observed.get());
    }

    #[test]
    fn developer_default_level_is_full_and_release_pins_null() {
        assert_eq!(LogLevel::default(), LogLevel::Full);
        assert_eq!(LogLevel::FULL, LogLevel::Full);
        assert_eq!(LogLevel::NULL, LogLevel::Null);
        assert_eq!(LogLevel::Full.as_str(), "full");
        assert_eq!(LogLevel::Null.as_str(), "null");
        assert_eq!(LogLevel::from_str("FULL").unwrap(), LogLevel::Full);
        assert_eq!(LogLevel::from_str("full").unwrap(), LogLevel::Full);
        assert_eq!(LogLevel::from_str("null").unwrap(), LogLevel::Null);
        assert!(LogLevel::from_str("bogus").is_err());
    }

    #[test]
    fn current_policy_is_fixed_by_the_build() {
        assert_eq!(current_level(),LogLevel::Full);
        assert!(is_enabled());
    }
}
