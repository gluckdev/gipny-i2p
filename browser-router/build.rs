fn main() {
    println!("cargo:rerun-if-changed=native/fixture.cpp");
    #[cfg(feature = "native-peer")]
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("wasm32") {
        // Read the committed gitlink, then verify the actual fixture checkout.
        // A snapshot must describe the i2pd that collected its RouterInfos.
        let git = |directory: &str, args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(directory)
                .args(args)
                .output()
                .expect("git is required for native fixture provenance");
            assert!(
                output.status.success(),
                "cannot read native fixture provenance"
            );
            String::from_utf8(output.stdout).expect("git revision encoding")
        };
        let tree = git("..", &["ls-tree", "HEAD", "third_party/i2pd"]);
        let fields: Vec<_> = tree.split_whitespace().collect();
        assert!(fields.len() == 4 && fields[0] == "160000" && fields[1] == "commit");
        let revision = fields[2];
        let checked_out = git("../third_party/i2pd", &["rev-parse", "HEAD"]);
        assert_eq!(
            revision,
            checked_out.trim(),
            "i2pd does not match the committed pin"
        );
        println!("cargo:rustc-env=GIPNY_BROWSER_I2PD_REVISION={revision}");
        println!("cargo:rerun-if-changed=../.git/HEAD");
        println!("cargo:rerun-if-changed=../.git/index");
        cc::Build::new()
            .cpp(true)
            .std("c++20")
            .warnings(false)
            .include("../third_party/i2pd/libi2pd")
            .file("native/fixture.cpp")
            .compile("gipny_browser_native_fixture");
    }
}
