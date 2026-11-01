//! Test-only i2pd peer. Built from the repository's pinned C++ sources in CI.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use std::ffi::CString;
    let directory = std::env::args().nth(1).expect("native-peer <datadir>");
    std::fs::create_dir_all(&directory).expect("datadir");
    i2p_embed::write_certificates(&std::path::Path::new(&directory).join("certificates"))
        .expect("verified reseed certificates");
    let arguments: Vec<CString> = [
        "gipny-browser-native-fixture".into(),
        format!("--datadir={directory}"),
        "--notransit".into(),
        "--host=127.0.0.1".into(),
        "--nat".into(),
        "--ntcp2.port=19720".into(),
        "--ntcp2.published=true".into(),
        "--ssu2.enabled=false".into(),
        "--reseed.verify=true".into(),
        "--persist.netdbinterval=15".into(),
    ]
    .into_iter()
    .map(|value: String| CString::new(value).unwrap())
    .collect();
    assert!(
        gipny_browser_router::native_fixture::start(&arguments),
        "native i2pd fixture init"
    );
    println!("READY");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}
