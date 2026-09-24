//! The i2pd router inside this process ([`i2p_embed`]), instead of a child
//! process reached over SAM on a loopback port.
//!
//! One router per process: libi2pd keeps global state. It runs out of one
//! data directory for the life of the process, whichever profile opened it
//! first — the router's state is the network's, not a person's (addresses are
//! new every start), so profiles can share it. Its settings are the first
//! profile's too, until the app restarts.
//!
//! A build with the `embedded-i2p` feature uses nothing else: no SAM, no
//! router process, no local port. SAM remains only in builds without it
//! (platforms not moved yet: Windows, macOS, Android).

use std::path::Path;
use std::sync::{Arc, OnceLock};

use crate::net::NetError;
use crate::router::{RouterSettings, Yggdrasil};

static ROUTER: OnceLock<Arc<i2p_embed::Router>> = OnceLock::new();

/// Whether this run uses the in-process router: always, in a build that has it.
pub fn enabled() -> bool {
    true
}

/// The router, started on first use under `data_dir/i2p/router`.
pub fn router(data_dir: &Path, settings: RouterSettings) -> Result<Arc<i2p_embed::Router>, NetError> {
    if let Some(r) = ROUTER.get() {
        return Ok(r.clone());
    }
    let router_dir = data_dir.join("i2p").join("router");
    std::fs::create_dir_all(&router_dir).map_err(|e| NetError::I2p(format!("router data dir: {e}")))?;
    let seeded = match (crate::router::compiled_in_seed(), crate::router::bundled_seed()) {
        (Some(bytes), _) => Some(crate::router::seed_netdb_reader(bytes, &router_dir)),
        (None, Some(seed)) => Some(crate::router::seed_netdb_from(&seed, &router_dir)),
        (None, None) => None,
    };
    if let Some(seeded) = seeded {
        match seeded {
            Ok(0) => {}
            Ok(n) => eprintln!("[i2p] laid out {n} known routers from the bundled snapshot"),
            Err(e) => eprintln!("[i2p] bundled network snapshot not used: {e}"),
        }
    }
    // What router.rs passes the child, less SAM and the HTTP proxy: nothing
    // here listens on any port but the router's own transports.
    let mut options = vec![
        format!("--datadir={}", router_dir.display()),
        "--sam.enabled=false".into(),
        "--http.enabled=false".into(),
        "--httpproxy.enabled=false".into(),
        "--socksproxy.enabled=false".into(),
        "--upnp.enabled=false".into(),
        // Reseed bundles are checked against the certificates i2p-embed lays
        // out; i2pd's default takes them unverified.
        "--reseed.verify=true".into(),
        format!("--bandwidth={}", settings.transit.bandwidth()),
        format!("--share={}", settings.transit.share_percent()),
        format!("--limits.transittunnels={}", settings.transit.transit_tunnels()),
    ];
    if matches!(settings.yggdrasil, Yggdrasil::On) {
        options.push("--meshnets.yggdrasil".into());
    }
    let log = router_dir.join("i2pd.log");
    let started = i2p_embed::Router::start(&options, log.to_str())
        .map_err(|e| NetError::I2p(format!("in-process router: {e}")))?;
    let started = Arc::new(started);
    // Another thread may have won the race; theirs is the one kept.
    Ok(ROUTER.get_or_init(|| started).clone())
}

/// The router, if this process started one.
pub fn running() -> Option<Arc<i2p_embed::Router>> {
    ROUTER.get().cloned()
}

/// Options for a destination with `hops`-long tunnels both ways.
pub fn destination_options(publish: bool, hops: u8) -> i2p_embed::DestinationOptions {
    let hops = hops.clamp(crate::net::MIN_HOPS, crate::net::DEFAULT_HOPS);
    i2p_embed::DestinationOptions { publish, inbound_length: hops, outbound_length: hops, extra: Vec::new() }
}
