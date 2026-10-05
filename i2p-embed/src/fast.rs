//! The measuring profile: as few hops as the router will build, and no
//! background traffic to measure them through.
//!
//! Off unless `GIPNY_FAST` is set. Everything here reads the environment and
//! nothing else, so a host that never heard of it behaves exactly as before —
//! which is the point: the number a measurement gives is only worth something
//! against the number the shipping build gives.
//!
//! ## What the profile costs
//!
//! Hops are the network's only cover traffic. A tunnel of length `n` puts `n`
//! strangers between the two ends, and the peer who watches the wire learns
//! about the message from a router that is not us. At zero hops there is no
//! stranger: the two routers talk to each other directly, so each learns the
//! other's IP, and anyone watching either end sees the whole conversation.
//! Nobody in between knows anything — which is exactly the problem. Fast mode
//! is for measuring a link, and nothing else.
//!
//! Noise is separate, and cheaper to give up: exploratory tunnels and transit
//! are work the router does that the conversation between our two nodes does
//! not need, so turning them off costs the measurement nothing.

/// The most hops i2p builds by default, and the top of every clamp below.
pub const MAX_HOPS: u8 = 3;

/// Whether the measuring profile is on: `GIPNY_FAST` is set to something
/// other than `0`, `no`, `off` or the empty string.
///
/// A set-and-off value is honoured rather than treated as on, because the
/// variable reaches a child process by inheritance and a service unit that
/// sets it to `0` to mean "off" should get off.
pub fn enabled() -> bool {
    match std::env::var("GIPNY_FAST") {
        Ok(v) => !matches!(v.trim().to_ascii_lowercase().as_str(), "" | "0" | "no" | "off" | "false"),
        Err(_) => false,
    }
}

/// How many hops the profile asks for, or `None` to leave the default alone.
///
/// `GIPNY_FAST_HOPS` says so explicitly and wins over [`enabled`], so a run
/// that wants zero hops does not also have to believe in the rest of the
/// profile.
///
/// Read as a `u16` rather than a `u8` on purpose: the number is clamped, so
/// `300` is a slip in arithmetic and should arrive as the longest tunnel i2pd
/// builds rather than be thrown away as unreadable. Anything that is not a
/// number at all is ignored, which leaves the default in place.
pub fn hops() -> Option<u8> {
    parse_hops(&std::env::var("GIPNY_FAST_HOPS").ok()?)
}

fn parse_hops(raw: &str) -> Option<u8> {
    Some(raw.trim().parse::<u16>().ok()?.min(u16::from(MAX_HOPS)) as u8)
}

/// The hop count the profile settles on: `GIPNY_FAST_HOPS` when it names one,
/// otherwise [`MAX_HOPS`] — the shortest tunnel that is still a tunnel, and
/// the fastest answer that does not hand both routers' addresses to each other.
pub fn effective_hops() -> u8 {
    hops().unwrap_or(MAX_HOPS)
}

/// The i2pd options the profile adds, for [`Router::start`].
///
/// Only what the conversation between our own two nodes does not need:
/// exploratory tunnels (i2pd builds them to measure the network and to keep
/// spare gateways; 3 in and 3 out at two hops is steady traffic), transit for
/// strangers, and the RouterInfo republication the router runs on a timer.
///
/// The RouterInfo one is `trust.hidden`, and it is the only entry here the
/// shim has to be told about: `api::InitI2P` never reads that option, it is
/// the daemon's to apply, so without the shim change it would be a flag that
/// reads as if it worked. See `gipny_router_init`.
///
/// What is left cannot be turned off and is not noise: the transports'
/// keepalives, and the LeaseSet republication, without which the other node
/// could not find us at all.
pub const ROUTER_OPTIONS: &[&str] = &[
    // Exploratory tunnels, in and out. i2pd's own default is 3 of each at
    // 2 hops; with them gone nothing is built that the two nodes do not use.
    "--exploratory.inbound.quantity=0",
    "--exploratory.outbound.quantity=0",
    // Do not carry other people's tunnels, and never start doing so.
    "--notransit",
    "--share=0",
    "--limits.transittunnels=0",
    // Do not republish our RouterInfo on the router's 39-minute timer. A netDb
    // entry is found by hash lookup, so hiding the router costs a peer nothing
    // it was going to get; what it stops is us announcing where we are to
    // strangers, once a quarter of an hour.
    "--trust.hidden=true",
];

/// What a flag is called to i2pd: everything after the leading dashes, up to
/// the first `=`.
///
/// The name is what boost's parser counts, so it is what a duplicate is. Note
/// that it compares whole names: `--share` and `--shareddest.x` are two
/// options, and treating one as the other would silence the wrong thing.
fn option_name(option: &str) -> &str {
    let body = option.trim_start_matches('-');
    body.split('=').next().unwrap_or(body)
}

/// Apply the profile to `options`, for [`Router::start`].
///
/// The profile's own options go in place of any the caller already set rather
/// than after them: i2pd parses its command line with
/// boost::program_options, and that parser throws `multiple_occurrences` — and
/// `ParseCmdline` turns that into `ThrowFatal` and `exit` — the moment one
/// non-composing option appears twice. Two `--share` lines is not "the last one
/// wins", it is a router that dies before it logs anything. So the caller's are
/// dropped and only ours survive.
///
/// A profile that is off returns `options` untouched, so a host does not have
/// to know whether it is on.
pub fn merge(options: Vec<String>) -> Vec<String> {
    if !enabled() {
        return options;
    }
    let overridden: Vec<&str> = ROUTER_OPTIONS.iter().map(|o| option_name(o)).collect();
    let mut merged: Vec<String> = options
        .into_iter()
        .filter(|o| !overridden.contains(&option_name(o)))
        .collect();
    merged.extend(ROUTER_OPTIONS.iter().map(|o| o.to_string()));
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `GIPNY_FAST` and `GIPNY_FAST_HOPS` are process-global, so these run
    /// under one lock and are the only tests in the crate that touch them.
    #[test]
    fn profile_is_off_unless_asked_for() {
        // The env is whatever the test runner inherited; the contract is
        // checked on the parsed value, not on a mutation nobody else sees.
        let raw = std::env::var("GIPNY_FAST").ok().map(|v| v.trim().to_ascii_lowercase());
        let expected = !matches!(raw.as_deref(), None | Some("" | "0" | "no" | "off" | "false"));
        assert_eq!(enabled(), expected);
    }

    #[test]
    fn hop_numbers_out_of_range_are_clamped_and_words_are_ignored() {
        for (raw, want) in [("0", 0), ("1", 1), ("3", 3), ("9", MAX_HOPS), ("300", MAX_HOPS), (" 2 ", 2)] {
            assert_eq!(parse_hops(raw), Some(want), "{raw:?}");
        }
        // Not a number: no opinion, so the default stands.
        for raw in ["two", "", "-1", "2.5", "0x2"] {
            assert_eq!(parse_hops(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn an_explicit_number_outranks_the_switch() {
        // GIPNY_FAST_HOPS says so even with the profile off: somebody who typed
        // it wants it, and the switch only decides what else comes with it.
        assert_eq!(hops(), std::env::var("GIPNY_FAST_HOPS").ok().as_deref().and_then(parse_hops));
        assert_eq!(effective_hops(), hops().unwrap_or(MAX_HOPS));
    }

    #[test]
    fn option_names_are_what_boost_counts() {
        assert_eq!(option_name("--share=50"), "share");
        assert_eq!(option_name("--notransit"), "notransit");
        assert_eq!(option_name("--limits.transittunnels=0"), "limits.transittunnels");
        // A prefix is a different option, and treating it as the same one is
        // how `--share` ends up silencing `--shareddest.inbound.quantity`.
        assert_ne!(option_name("--shareddest.inbound.quantity=3"), option_name("--share"));
    }

    /// A repeated option kills the router before it logs anything, so the merge
    /// has to hold whatever the environment is doing to these tests.
    #[test]
    fn merge_never_leaves_a_repeated_option() {
        let base: Vec<String> = [
            "--datadir=/tmp/r",
            "--bandwidth=O",
            "--share=50",
            "--limits.transittunnels=256",
            "--exploratory.inbound.quantity=3",
            "--sam.enabled=false",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let merged = merge(base.clone());
        let mut names: Vec<&str> = merged.iter().map(|o| option_name(o)).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "an option was given twice: {names:?}");
        if enabled() {
            // The profile's value is the one left, and what it says nothing
            // about is untouched.
            let value = |name: &str| {
                merged
                    .iter()
                    .find(|o| option_name(o) == name)
                    .and_then(|o| o.split_once('='))
                    .map(|(_, v)| v)
            };
            assert_eq!(value("share"), Some("0"));
            assert_eq!(value("limits.transittunnels"), Some("0"));
            assert_eq!(value("datadir"), Some("/tmp/r"));
            assert_eq!(value("bandwidth"), Some("O"), "an option the profile says nothing about");
            assert_eq!(value("sam.enabled"), Some("false"));
        } else {
            assert_eq!(merged, base, "an off profile must change nothing");
        }
    }
}