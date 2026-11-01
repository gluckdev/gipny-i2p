//! Turn signed RouterInfos into a short-lived forwarder allowlist and browser
//! input. Public netDb records only: no router.keys or local private keys.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use emissary_core::crypto::base64_encode;
    use gipny_browser_router::ntcp2::verified_peer;
    use std::{fs, path::Path};

    fn files(path: &Path, output: &mut Vec<std::path::PathBuf>) {
        if path.is_dir() {
            for entry in std::fs::read_dir(path).expect("netDb directory").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files(&path, output);
                } else if path
                    .file_name()
                    .and_then(|p| p.to_str())
                    .is_some_and(|name| name.starts_with("routerInfo-") && name.ends_with(".dat"))
                {
                    output.push(path);
                }
            }
        }
    }
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(
        args.len(),
        4,
        "prepare-netdb <native datadir> <candidates.tsv> <max public peers>"
    );
    let root = Path::new(&args[1]);
    let limit: usize = args[3].parse().expect("max peers");
    let mut paths = vec![root.join("router.info")];
    files(&root.join("netDb"), &mut paths);
    let mut rows = Vec::new();
    let mut public = 0;
    for path in paths {
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(peer) = verified_peer(&bytes) else {
            continue;
        };
        let fixture = path == root.join("router.info");
        if fixture && !peer.address.ip().is_loopback() {
            panic!("fixture must publish loopback");
        }
        if !fixture && (peer.address.ip().is_loopback() || peer.address.ip().is_unspecified()) {
            continue;
        }
        if !fixture && public >= limit {
            continue;
        }
        if !fixture {
            public += 1;
        }
        rows.push(format!(
            "{}\t{}\t{}\t{}\t{}",
            peer.address,
            base64_encode(&bytes),
            base64_encode(peer.router.identity.hash()),
            if fixture { "fixture" } else { "netdb" },
            peer.router.published.date() / 1000
        ));
    }
    assert!(
        rows.iter().any(|row| row.contains("\tfixture\t")),
        "no signed local fixture RouterInfo"
    );
    assert!(public > 0, "no verified public NTCP2 peers in fresh netDb");
    fs::write(&args[2], rows.join("\n") + "\n").expect("candidates TSV");
    println!(
        "prepared {} verified RouterInfos ({public} public candidates)",
        rows.len()
    );
}

#[cfg(target_arch = "wasm32")]
fn main() {}
