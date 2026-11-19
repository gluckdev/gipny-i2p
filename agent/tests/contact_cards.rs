//! CLI-only card validation must not start a router, open a provider or write
//! agent data. Compiled/executed only by GitHub CI.
use std::{path::PathBuf, process::Command};
use gipny_libcore::ContactCard;

struct Directory(PathBuf);
impl Drop for Directory { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

#[test]
fn validation_accepts_personal_cards_and_rejects_invalid_encodings_without_writing_data() {
    let root = Directory(std::env::temp_dir().join(format!("gipny-card-cli-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())));
    std::fs::create_dir(&root.0).unwrap();
    let data = root.0.join("must-not-exist");
    let address = "ibaueq2eivdeoscjjjfuytkoj5ifcustkrkvmv2ylfnfwxc5lzpq.b32.i2p";
    let mut card = ContactCard { i2p_address: address.into(), sign_pk: [1; 32], dh_pk: [2; 32],
        relay: Some(address.into()), name: None };
    let encoded = card.encode().unwrap();
    let invoke = |card: &str| {
        Command::new(env!("CARGO_BIN_EXE_gipny-agent"))
            .args(["--check-master-card", card, "--data"]).arg(&data)
            // Validation precedes Windows update bootstrap as well.
            .env("GIPNY_AGENT_READY_NONCE", "invalid-unused-ticket")
            .env_remove("GIPNY_AGENT_READY_PATH").output().unwrap()
    };
    let good = invoke(&encoded);
    assert!(good.status.success(), "{}", String::from_utf8_lossy(&good.stderr));
    assert!(good.stdout.is_empty() && good.stderr.is_empty());
    card.relay = None;
    let personal = invoke(&card.encode().unwrap());
    assert!(personal.status.success() && personal.stdout.is_empty() && personal.stderr.is_empty());
    for bad in [encoded.clone() + "=", "not a contact card".into()] {
        let output = invoke(&bad);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains(&bad));
        assert!(!data.exists());
    }
    assert!(!data.exists());
}
