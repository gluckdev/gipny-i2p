//! Compiles libi2pd (from the third_party/i2pd submodule) and our C shim into
//! one static library linked into whatever uses this crate.
//!
//! Needs boost (headers, and program_options), OpenSSL and zlib — the same as
//! building i2pd itself. Where they are not on the default search paths:
//!   I2P_EMBED_INCLUDE_DIRS  extra include dirs, separated by the platform's
//!                           path separator
//!   I2P_EMBED_LIB_DIRS      extra library dirs, likewise
//!   I2P_EMBED_STATIC=1      link boost, OpenSSL and zlib statically
//!   I2P_EMBED_I2PD_SRC      another i2pd checkout (default: the submodule)
//!   I2P_EMBED_SKIP_NATIVE=1 compile nothing native (a `cargo check` of the
//!                           Rust side on a machine without boost)

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=I2P_EMBED_SKIP_NATIVE");
    if env::var_os("I2P_EMBED_SKIP_NATIVE").is_some_and(|v| v == "1") {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let i2pd = env::var_os("I2P_EMBED_I2PD_SRC")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../third_party/i2pd"));
    let lib = i2pd.join("libi2pd");
    assert!(
        lib.join("api.h").is_file(),
        "i2pd sources not found at {} — git submodule update --init third_party/i2pd",
        i2pd.display()
    );
    println!("cargo:rerun-if-changed=shim");
    println!("cargo:rerun-if-changed={}", lib.display());
    for var in ["I2P_EMBED_INCLUDE_DIRS", "I2P_EMBED_LIB_DIRS", "I2P_EMBED_STATIC", "I2P_EMBED_I2PD_SRC"] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++20")
        .warnings(false)
        .include(&lib)
        .include(manifest.join("shim"))
        .file(manifest.join("shim/shim.cpp"));
    for entry in std::fs::read_dir(&lib).expect("read libi2pd") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "cpp") {
            build.file(path);
        }
    }
    if let Some(dirs) = env::var_os("I2P_EMBED_INCLUDE_DIRS") {
        for d in env::split_paths(&dirs) {
            build.include(d);
        }
    }
    if target_os == "windows" {
        build.define("WIN32_LEAN_AND_MEAN", None).define("_WIN32_WINNT", "0x0A00");
    }
    build.compile("gipny_i2pd");

    if let Some(dirs) = env::var_os("I2P_EMBED_LIB_DIRS") {
        for d in env::split_paths(&dirs) {
            println!("cargo:rustc-link-search=native={}", d.display());
        }
    }
    let kind = if env::var_os("I2P_EMBED_STATIC").is_some_and(|v| v == "1") { "static=" } else { "" };
    for name in ["boost_program_options", "ssl", "crypto", "z"] {
        println!("cargo:rustc-link-lib={kind}{name}");
    }
    match target_os.as_str() {
        "windows" => {
            for name in ["ws2_32", "mswsock", "iphlpapi", "crypt32", "bcrypt"] {
                println!("cargo:rustc-link-lib={name}");
            }
        }
        "linux" => {
            println!("cargo:rustc-link-lib=pthread");
            println!("cargo:rustc-link-lib=atomic");
        }
        _ => {}
    }
}
