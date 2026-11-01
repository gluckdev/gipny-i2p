//! Test-only i2pd peer. Built from the repository's pinned C++ sources in CI.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let directory = std::env::args().nth(1).expect("native-peer <datadir>");
    std::fs::create_dir_all(&directory).expect("datadir");
    let log = std::path::Path::new(&directory).join("i2pd.log");
    let _router = i2p_embed::Router::start(
        &[
            format!("--datadir={directory}"),
            "--notransit".into(),
            "--host=127.0.0.1".into(),
            "--nat=true".into(),
            "--ntcp2.port=19720".into(),
            "--ntcp2.published=true".into(),
            "--ssu2.enabled=false".into(),
            "--ipv6=false".into(),
            "--reseed.verify=true".into(),
            "--persist.netdbinterval=15".into(),
        ],
        log.to_str(),
    )
    .expect("native i2pd peer");
    println!("READY");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}
