//! Compiles libi2pd (from the third_party/i2pd submodule) and our C shim into
//! one static library linked into whatever uses this crate.
//!
//! Needs boost (headers, and program_options), OpenSSL and zlib — the same as
//! building i2pd itself. Where they are not on the default search paths:
//!   I2P_EMBED_INCLUDE_DIRS  extra include dirs, separated by the platform's
//!                           path separator
//!   I2P_EMBED_LIB_DIRS      extra library dirs, likewise
//!   I2P_EMBED_STATIC=1      link boost, OpenSSL and zlib statically
//!   I2P_EMBED_STATIC_LIBS   or only these, comma separated (e.g.
//!                           "boost_program_options")
//!   I2P_EMBED_LIBS          the libraries' names, if not boost_program_options,
//!                           ssl, crypto, z (vcpkg's on Windows)
//!   I2P_EMBED_I2PD_SRC      another i2pd checkout (default: the submodule)
//!   I2P_EMBED_CERTS_DIR     the reseed/family certificates to compile in
//!                           (default: the checkout's contrib/certificates;
//!                           CI passes upstream's current set, fetched by
//!                           scripts/fresh-i2p-certs.sh)
//!   I2P_EMBED_PREBUILT_DIR  link the libi2pd.so there (Android, where
//!                           ndk-build compiles libi2pd with the shim) and
//!                           compile nothing
//!   I2P_EMBED_STUB=1        no router: the shim's functions over nothing
//!                           (shim/stub.c), for builds that must link and
//!                           start but never run one (debug APKs in build.yml)
//!   I2P_EMBED_SKIP_NATIVE=1 compile nothing native (a `cargo check` of the
//!                           Rust side on a machine without boost)

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let i2pd = env::var_os("I2P_EMBED_I2PD_SRC")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../third_party/i2pd"));
    println!("cargo:rerun-if-env-changed=I2P_EMBED_CERTS_DIR");
    let certs = env::var_os("I2P_EMBED_CERTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| i2pd.join("contrib/certificates"));
    certificates(&certs);

    println!("cargo:rerun-if-env-changed=I2P_EMBED_SKIP_NATIVE");
    if env::var_os("I2P_EMBED_SKIP_NATIVE").is_some_and(|v| v == "1") {
        return;
    }
    // No router at all: the shim's interface over nothing, so a build that
    // only has to link and start (build.yml's debug APKs) needs no boost.
    println!("cargo:rerun-if-env-changed=I2P_EMBED_STUB");
    if env::var_os("I2P_EMBED_STUB").is_some_and(|v| v == "1") {
        println!("cargo:rerun-if-changed=shim/stub.c");
        cc::Build::new().include(manifest.join("shim")).file(manifest.join("shim/stub.c")).compile("gipny_i2pd_stub");
        return;
    }
    // Android: libi2pd and the shim are one shared library built by ndk-build
    // (android-router/jni, with the NDK's own boost and OpenSSL), packaged
    // beside ours; link against it instead of compiling anything here.
    println!("cargo:rerun-if-env-changed=I2P_EMBED_PREBUILT_DIR");
    if let Some(dir) = env::var_os("I2P_EMBED_PREBUILT_DIR").filter(|v| !v.is_empty()) {
        let dir = PathBuf::from(dir);
        assert!(dir.join("libi2pd.so").is_file(), "no libi2pd.so in I2P_EMBED_PREBUILT_DIR={}", dir.display());
        println!("cargo:rerun-if-changed={}", dir.join("libi2pd.so").display());
        println!("cargo:rustc-link-search=native={}", dir.display());
        println!("cargo:rustc-link-lib=dylib=i2pd");
        return;
    }
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
        // As upstream's CMake build: no <windows.h> min/max macros over std::.
        build
            .define("WIN32_LEAN_AND_MEAN", None)
            .define("NOMINMAX", None)
            .define("_WIN32_WINNT", "0x0A00")
            .define("WINVER", "0x0A00");
    }
    if target_os == "macos" {
        // FS.cpp and friends key the macOS paths off it, as upstream's builds.
        build.define("MAC_OSX", None);
    }
    // libi2pd's globals (tunnels, netdb, transports) are destroyed by exit()
    // in an order that is the linker's: on macOS ~Tunnels ran after a mutex
    // it needs was gone ("mutex lock failed", SIGABRT after every clean run).
    // The router is stopped before that (the shim's atexit handler), so let
    // the process take the rest with it. Clang only; gcc keeps them and has
    // not shown the problem.
    build.flag_if_supported("-fno-c++-static-destructors");
    if env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|e| e == "msvc") {
        // Libraries are named below, not by boost's #pragma autolink (whose
        // names vcpkg's builds do not match); /bigobj for i2pd's larger units.
        build.define("BOOST_ALL_NO_LIB", None).flag("/bigobj").flag("/utf-8");
    }
    build.compile("gipny_i2pd");

    if let Some(dirs) = env::var_os("I2P_EMBED_LIB_DIRS") {
        for d in env::split_paths(&dirs) {
            println!("cargo:rustc-link-search=native={}", d.display());
        }
    }
    println!("cargo:rerun-if-env-changed=I2P_EMBED_STATIC_LIBS");
    let all_static = env::var_os("I2P_EMBED_STATIC").is_some_and(|v| v == "1");
    let some_static: Vec<String> = env::var("I2P_EMBED_STATIC_LIBS")
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    // I2P_EMBED_LIBS overrides the names (vcpkg on Windows: e.g.
    // "boost_program_options-vc143-mt,libssl,libcrypto,zlib").
    println!("cargo:rerun-if-env-changed=I2P_EMBED_LIBS");
    let libs: Vec<String> = env::var("I2P_EMBED_LIBS")
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_else(|_| ["boost_program_options", "ssl", "crypto", "z"].map(String::from).to_vec());
    for name in &libs {
        let kind = if all_static || some_static.iter().any(|s| s == name) { "static=" } else { "" };
        println!("cargo:rustc-link-lib={kind}{name}");
    }
    match target_os.as_str() {
        "windows" => {
            // Winsock and interfaces for the router; the rest for a static
            // OpenSSL and FS.cpp's known-folder lookup.
            for name in ["ws2_32", "mswsock", "iphlpapi", "crypt32", "bcrypt", "advapi32", "user32", "shell32", "ole32"] {
                println!("cargo:rustc-link-lib={name}");
            }
        }
        "macos" => {
            // libc++ comes with cc; OpenSSL's rand and keychain bits.
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=Security");
        }
        "linux" => {
            println!("cargo:rustc-link-lib=pthread");
            println!("cargo:rustc-link-lib=atomic");
        }
        _ => {}
    }
}

/// i2pd's reseed and family certificates, compiled into the binary: the
/// router verifies reseed bundles against them, and nothing but this binary
/// is shipped. `CERTIFICATES` in $OUT_DIR/certificates.rs, as
/// (path under certificates/, contents).
fn certificates(dir: &std::path::Path) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files = Vec::new();
    for sub in ["reseed", "family"] {
        let Ok(entries) = std::fs::read_dir(dir.join(sub)) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "crt") {
                let name = format!("{sub}/{}", path.file_name().unwrap().to_string_lossy());
                files.push((name, path));
            }
        }
    }
    files.sort();
    assert!(
        files.iter().any(|(n, _)| n.starts_with("reseed/")),
        "no reseed certificates under {} — a router could never bootstrap",
        dir.display()
    );
    let mut out = String::from("pub static CERTIFICATES: &[(&str, &[u8])] = &[\n");
    for (name, path) in &files {
        out.push_str(&format!("    ({name:?}, include_bytes!({:?})),\n", path.canonicalize().unwrap()));
    }
    out.push_str("];\n");
    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("certificates.rs");
    std::fs::write(dest, out).expect("write certificates.rs");
}
