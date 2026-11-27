//! Run the router until it knows enough of the network, then stop: the
//! network database snapshot release.yml ships (`i2pd-netdb-seed.tar.gz`) is
//! made from what this leaves in `<datadir>/netDb`.
//!
//!   cargo run --release -p i2p-embed --example netdb_snapshot -- <datadir> [routers] [max secs]
//!
//! No transit (a snapshot run is nobody's relay), no destinations.

use std::path::Path;
use std::time::{Duration, Instant};

fn routers(netdb: &Path) -> usize {
    let Ok(dirs) = std::fs::read_dir(netdb) else { return 0 };
    dirs.flatten()
        .filter_map(|d| std::fs::read_dir(d.path()).ok())
        .flat_map(|f| f.flatten())
        .filter(|f| f.file_name().to_str().is_some_and(|n| n.starts_with("routerInfo-")))
        .count()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("usage: netdb_snapshot <datadir> [routers] [max secs]");
    let want: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(2500);
    let max = Duration::from_secs(args.next().and_then(|s| s.parse().ok()).unwrap_or(360));
    std::fs::create_dir_all(&dir).expect("datadir");
    let log = Path::new(&dir).join("i2pd.log");
    let router = i2p_embed::Router::start(
        &[
            format!("--datadir={dir}"),
            "--notransit".into(),
            "--reseed.verify=true".into(),
            "--exploratory.inbound.length=2".into(),
            "--exploratory.outbound.length=2".into(),
        ],
        log.to_str(),
    )
    .expect("router");
    // Pre-warm 2-hop client inbound and outbound tunnels during the build
    // snapshot run so the bundled netDb has actively probed, tunnel-verified peers.
    let warm_dest =
        i2p_embed::Destination::new(&router, None, &i2p_embed::DestinationOptions::default()).ok();
    let netdb = Path::new(&dir).join("netDb");
    let t0 = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(10));
        let n = routers(&netdb);
        let tunnels_warm = warm_dest
            .as_ref()
            .is_none_or(|d| d.tunnel_directions_ready() == (true, true) && d.is_ready());
        println!(
            "after {} s: {n} routers known, 2-hop client tunnels warm: {tunnels_warm}",
            t0.elapsed().as_secs()
        );
        if (n >= want && tunnels_warm) || t0.elapsed() >= max {
            break;
        }
    }
    drop(warm_dest);
    // Stopping writes out what the router holds in memory.
    drop(router);
    println!("snapshot: {} routers", routers(&netdb));
}
