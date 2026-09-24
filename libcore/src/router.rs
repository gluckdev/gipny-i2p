//! The i2p router's settings, startup progress, and the network database
//! snapshot it starts from.
//!
//! The router itself is libi2pd compiled into this process
//! ([`crate::embedded`]): no child process, no SAM, no local port.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How much of the line to give to other people's tunnels.
///
/// This is an anonymity setting, not a generosity setting. Transit traffic is
/// cover traffic that strangers generate and pay for: a router carrying nothing
/// but its own messages hands an observer a clean signal, while one relaying for
/// others is indistinguishable from one merely passing something along. Carrying
/// none is the cheapest way to lose anonymity, and carrying some is the cheapest
/// way to buy it.
///
/// The cost is bandwidth, and on a metered or battery-powered device that cost
/// is real, so this is a curve rather than a switch. i2pd's own defaults — 100%
/// share, 25000 transit tunnels — sit at the server end of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TransitProfile {
    /// Minimal but never zero: metered connections and battery. Still blends.
    Frugal,
    /// The default. A desktop on mains power, or a phone charging on wi-fi.
    #[default]
    Balanced,
    /// A machine that is always on and not paying per gigabyte.
    Generous,
}

impl TransitProfile {
    /// i2pd's `bandwidth`: a letter (L=32, O=256, P=2048 KB/s) or a number.
    pub fn bandwidth(self) -> &'static str {
        match self {
            Self::Frugal => "L",
            Self::Balanced => "O",
            Self::Generous => "P",
        }
    }

    /// Percentage of that line offered to transit.
    pub fn share_percent(self) -> u8 {
        match self {
            Self::Frugal => 15,
            Self::Balanced => 50,
            Self::Generous => 80,
        }
    }

    /// Cap on simultaneous transit tunnels. Enough to blend into; far below
    /// i2pd's 25000, which assumes a server.
    pub fn transit_tunnels(self) -> u32 {
        match self {
            Self::Frugal => 32,
            Self::Balanced => 256,
            Self::Generous => 2048,
        }
    }

    /// Stable name for storage and for the UI.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Frugal => "frugal",
            Self::Balanced => "balanced",
            Self::Generous => "generous",
        }
    }

    /// Anything unrecognised falls back to the default rather than erroring: a
    /// stored value from a newer build must not stop the router from starting.
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "frugal" => Self::Frugal,
            "generous" => Self::Generous,
            _ => Self::Balanced,
        }
    }
}

/// Where startup progress goes while the caller is blocked.
///
/// libcore knows nothing about interfaces: it calls this with a stable stage
/// id and one line for a person to read, and whoever started the node decides
/// what to do with them. Without it, the minutes spent waiting for SAM are
/// visible only as `eprintln!` in a log nobody has open — which is exactly how
/// unlocking a profile came to look like a freeze.
pub type BootProgress = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Report one step, and keep the log line it replaced.
pub(crate) fn note(progress: &Option<BootProgress>, stage: &str, detail: impl AsRef<str>) {
    let detail = detail.as_ref();
    eprintln!("[i2p] {detail}");
    if let Some(p) = progress {
        p(stage, detail);
    }
}

/// When to carry i2p over a Yggdrasil mesh as well as plain IP.
///
/// Yggdrasil is a different underlay, so it is a way in when the normal way is
/// blocked — i2p peers and reseed servers are both blockable. It needs a
/// Yggdrasil node already running on this machine; i2pd looks up the local mesh
/// address and does not provide one. With none present the router carries on
/// over plain IP, so turning this on is harmless even when nothing is there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Yggdrasil {
    /// Never. Plain IP only.
    Off,
    /// Not unless asked: the same as `Off` for the in-process router, kept
    /// so stored settings keep their meaning.
    #[default]
    Auto,
    /// Always announce over the mesh.
    On,
}

impl Yggdrasil {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::On => "on",
        }
    }

    /// Unknown values fall back to the default rather than erroring: a setting
    /// written by a newer build must not stop the router from starting.
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "off" => Self::Off,
            "on" => Self::On,
            _ => Self::Auto,
        }
    }
}

/// Router knobs the user can change.
///
/// Both are persisted per profile and only read when the router starts: i2pd
/// takes these from its command line, and its runtime setters are not wired to
/// anything reachable from outside the process. Changing either therefore needs
/// the router restarted, which drops built tunnels — so the UI says so rather
/// than pretending the change is instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RouterSettings {
    pub transit: TransitProfile,
    pub yggdrasil: Yggdrasil,
}

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
pub(crate) fn seed_netdb_from(seed: &Path, router_dir: &Path) -> std::io::Result<usize> {
    if !seed.is_file() {
        return Ok(0);
    }
    seed_netdb_reader(std::fs::File::open(seed)?, router_dir)
}

/// The snapshot compiled into this binary (`GIPNY_NETDB_SEED` at build time;
/// the Android build, which has no file beside it that Rust could read).
pub(crate) fn compiled_in_seed() -> Option<&'static [u8]> {
    #[cfg(gipny_netdb_seed)]
    return Some(include_bytes!(env!("GIPNY_NETDB_SEED_PATH")));
    #[cfg(not(gipny_netdb_seed))]
    None
}

/// As [`seed_netdb_from`], from a gzipped tar of the snapshot.
pub(crate) fn seed_netdb_reader(seed: impl std::io::Read, router_dir: &Path) -> std::io::Result<usize> {
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
pub(crate) fn bundled_seed() -> Option<PathBuf> {
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
            // set_path refuses "..", as a well-behaved writer would; write the
            // raw name so the reader's own check is what is tested.
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
