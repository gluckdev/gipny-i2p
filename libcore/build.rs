//! GIPNY_NETDB_SEED: a network database snapshot (release.yml's
//! i2pd-netdb-seed.tar.gz) to compile into the binary, for builds that ship
//! no file beside it (Android). Unset, the snapshot is looked for at run time.

fn main() {
    println!("cargo:rustc-check-cfg=cfg(gipny_netdb_seed)");
    println!("cargo:rerun-if-env-changed=GIPNY_NETDB_SEED");
    let Some(seed) = std::env::var_os("GIPNY_NETDB_SEED").filter(|v| !v.is_empty()) else { return };
    let seed = std::path::PathBuf::from(seed);
    let seed = seed.canonicalize().unwrap_or_else(|e| panic!("GIPNY_NETDB_SEED={}: {e}", seed.display()));
    println!("cargo:rerun-if-changed={}", seed.display());
    println!("cargo:rustc-env=GIPNY_NETDB_SEED_PATH={}", seed.display());
    println!("cargo:rustc-cfg=gipny_netdb_seed");
}
