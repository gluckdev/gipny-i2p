//! Network database snapshot seeding for the relay.
//! Copied from libcore/src/router.rs to avoid workspace dependency conflicts.

use std::path::{Path, PathBuf};
use std::io;

/// Fewer known routers than this and the bundled snapshot is laid out: i2pd
/// itself wants 90 before it stops calling its network empty.
const SEED_BELOW_ROUTERS: usize = 90;

/// On a first start (or after the network database was lost), lay out the
/// snapshot of the i2p network database that ships with the app
/// (`i2pd-netdb-seed.tar.gz`, made by release.yml), so the router starts
/// knowing hundreds of routers instead of reseeding over HTTPS first — most of
/// a cold start, and blockable. Existing entries are kept; stale ones the
/// router drops itself, and with too few left it reseeds as it always did.
/// Returns how many RouterInfos were written.
pub fn seed_netdb_from(seed: &Path, router_dir: &Path) -> io::Result<usize> {
    if !seed.is_file() {
        return Ok(0);
    }
    seed_netdb_reader(std::fs::File::open(seed)?, router_dir)
}

/// As [`seed_netdb_from`], from a gzipped tar of the snapshot.
pub fn seed_netdb_reader(seed: impl io::Read, router_dir: &Path) -> io::Result<usize> {
    let netdb = router_dir.join("netDb");
    if count_router_infos(&netdb) >= SEED_BELOW_ROUTERS {
        return Ok(0);
    }
    std::fs::create_dir_all(&netdb)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(seed));
    let mut written = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        // Only r?/routerInfo-*.dat, and only inside netDb: the archive is ours,
        // but a path that climbs out is refused all the same.
        let name_ok = path.file_name().and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("routerInfo-") && n.ends_with(".dat"));
        let plain = path.components().all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir));
        if !entry.header().entry_type().is_file() || !name_ok || !plain {
            continue;
        }
        let dest = netdb.join(&path);
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry.unpack(&dest)?;
        written += 1;
    }
    Ok(written)
}

fn count_router_infos(netdb: &Path) -> usize {
    let Ok(dirs) = std::fs::read_dir(netdb) else { return 0 };
    dirs.flatten()
        .filter_map(|d| std::fs::read_dir(d.path()).ok())
        .flat_map(|files| files.flatten())
        .filter(|f| f.file_name().to_str().is_some_and(|n| n.starts_with("routerInfo-")))
        .count()
}

/// The bundled network database snapshot: `GIPNY_I2P_SEED` (the app sets it
/// from its resource dir), or next to this executable — where the agent's
/// archive puts it.
pub fn bundled_seed() -> Option<PathBuf> {
    const NAME: &str = "i2pd-netdb-seed.tar.gz";
    if let Some(p) = std::env::var_os("GIPNY_I2P_SEED").map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for sub in ["", "resources", "../lib", "../Resources"] {
                candidates.push(if sub.is_empty() { dir.join(NAME) } else { dir.join(sub).join(NAME) });
            }
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

#[cfg(test)]
mod seed_tests {
    use super::*;

    fn snapshot(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let seed = dir.join("i2pd-netdb-seed.tar.gz");
        let file = std::fs::File::create(&seed).unwrap();
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(file, flate2::Compression::fast()));
        for (path, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            let name = h.as_old_mut().name.as_mut();
            name[..path.len()].copy_from_slice(path.as_bytes());
            h.set_cksum();
            tar.append(&h, *data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        seed
    }

    #[test]
    fn lays_out_router_infos_only_and_only_into_an_empty_netdb() {
        let dir = tempfile::tempdir().unwrap();
        let seed = snapshot(dir.path(), &[
            ("rA/routerInfo-A1.dat", b"a"),
            ("rB/routerInfo-B1.dat", b"b"),
            ("rB/notes.txt", b"no"),
            ("../routerInfo-escape.dat", b"no"),
        ]);
        let router = dir.path().join("router");
        assert_eq!(seed_netdb_from(&seed, &router).unwrap(), 2);
        assert!(router.join("netDb/rA/routerInfo-A1.dat").is_file());
        assert!(!router.join("netDb/rB/notes.txt").exists());
        assert!(!dir.path().join("routerInfo-escape.dat").exists());

        // A netDb that already knows enough routers is left alone.
        for i in 0..SEED_BELOW_ROUTERS {
            let d = router.join("netDb/rC");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(format!("routerInfo-C{i}.dat")), b"c").unwrap();
        }
        std::fs::remove_file(router.join("netDb/rA/routerInfo-A1.dat")).unwrap();
        assert_eq!(seed_netdb_from(&seed, &router).unwrap(), 0);
    }
}