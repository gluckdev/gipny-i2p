//! Turn signed RouterInfos into a short-lived forwarder allowlist and browser
//! input. Public netDb records only: no router.keys or local private keys.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use emissary_core::crypto::{base64_encode, sha256::Sha256};
    use gipny_browser_router::{bootstrap::is_public_ipv4, ntcp2::verified_peer};
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
    assert!(
        args.len() == 4 || args.len() == 5,
        "prepare-netdb <native datadir> <candidates.tsv> <max public peers> [snapshot.tar.gz]"
    );
    let root = Path::new(&args[1]);
    let limit: usize = args[3].parse().expect("max peers");
    assert!((1..=2048).contains(&limit));
    let snapshot_mode = args.len() == 5;
    let mut paths = if snapshot_mode {
        Vec::new()
    } else {
        vec![root.join("router.info")]
    };
    files(&root.join("netDb"), &mut paths);
    paths.sort();
    let mut rows = Vec::new();
    let mut records = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut public = 0;
    let mut rejected = std::collections::BTreeMap::<String, usize>::new();
    for path in paths {
        let Ok(bytes) = fs::read(&path) else { continue };
        let peer = match verified_peer(&bytes) {
            Ok(peer) => peer,
            Err(error) => {
                *rejected.entry(format!("{error:?}")).or_default() += 1;
                continue;
            }
        };
        let fixture = path == root.join("router.info");
        if fixture && !peer.address.ip().is_loopback() {
            panic!("fixture must publish loopback");
        }
        if !fixture && !is_public_ipv4(peer.address.ip()) {
            continue;
        }
        if !fixture && public >= limit {
            continue;
        }
        let hash = base64_encode(peer.router.identity.hash());
        if !seen.insert(hash.clone()) {
            continue;
        }
        if !fixture {
            public += 1;
            records.push((hash.clone(), bytes.clone()));
        }
        rows.push(format!(
            "{}\t{}\t{}\t{}\t{}",
            peer.address,
            base64_encode(&bytes),
            hash,
            if fixture { "fixture" } else { "netdb" },
            peer.router.published.date() / 1000
        ));
    }
    if (!snapshot_mode && !rows.iter().any(|row| row.contains("\tfixture\t"))) || public == 0 {
        eprintln!("NETDB_NOT_READY public={public} rejected={rejected:?}");
        std::process::exit(2);
    }
    fs::write(&args[2], rows.join("\n") + "\n").expect("candidates TSV");
    if snapshot_mode {
        assert!(public >= 16, "snapshot needs at least 16 public peers");
        let gzip = flate2::GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), flate2::Compression::fast());
        let mut archive = tar::Builder::new(gzip);
        for (hash, bytes) in &records {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_cksum();
            archive
                .append_data(
                    &mut header,
                    format!("r{}/routerInfo-{hash}.dat", &hash[..1]),
                    bytes.as_slice(),
                )
                .expect("snapshot entry");
        }
        let bytes = archive
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gzip");
        assert!(bytes.len() <= gipny_browser_router::bootstrap::MAX_DOWNLOAD_BYTES);
        let digest = Sha256::new()
            .update(&bytes)
            .finalize_new()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        fs::write(&args[4], &bytes).expect("snapshot tar.gz");
        let manifest = serde_json::json!({
            "sha256": digest, "compressed_bytes": bytes.len(), "records": records.len(),
            "created_unix": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
            "i2pd_revision": env!("GIPNY_BROWSER_I2PD_REVISION"),
        });
        fs::write(
            format!("{}.json", args[4]),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .expect("snapshot manifest");
        println!("SNAPSHOT_PACKED {manifest}");
    }
    println!(
        "prepared {} verified RouterInfos ({public} public candidates)",
        rows.len()
    );
}

#[cfg(target_arch = "wasm32")]
fn main() {}
