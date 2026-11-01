fn main() {
    println!("cargo:rerun-if-changed=native/fixture.cpp");
    #[cfg(feature = "native-peer")]
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("wasm32") {
        cc::Build::new()
            .cpp(true)
            .std("c++20")
            .warnings(false)
            .include("../third_party/i2pd/libi2pd")
            .file("native/fixture.cpp")
            .compile("gipny_browser_native_fixture");
    }
}
