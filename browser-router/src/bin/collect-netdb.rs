//! CI snapshot producer, using the same pinned i2pd/certificates as releases.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use std::{
        path::Path,
        time::{Duration, Instant},
    };
    let directory = std::env::args().nth(1).expect("collect-netdb <datadir>");
    std::fs::create_dir_all(&directory).expect("datadir");
    let router = i2p_embed::Router::start(
        &[
            format!("--datadir={directory}"),
            "--notransit".into(),
            "--reseed.verify=true".into(),
            "--persist.netdbinterval=15".into(),
        ],
        None,
    )
    .expect("verified snapshot router");
    let started = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(10));
        let count = router.netdb_routers().unwrap_or(0);
        println!(
            "SNAPSHOT_PROGRESS seconds={} routers={count}",
            started.elapsed().as_secs()
        );
        if count >= 2500 || started.elapsed() >= Duration::from_secs(360) {
            break;
        }
    }
    drop(router); // Flush public RouterInfos before the packer reads netDb.
    assert!(Path::new(&directory).join("netDb").is_dir(), "netDb output");
}

#[cfg(target_arch = "wasm32")]
fn main() {}
