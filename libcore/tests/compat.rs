//! Cross-version compatibility fixtures (issue #31).
//!
//! Moving the crypto/serde stack to new majors may change nothing about what is
//! on the wire: an X3DH handshake and a ratchet message produced before must
//! decrypt after, attachment chunks likewise, and fixed seeds must derive to
//! the same keys and signatures. The owner-required device binding v2 rejects
//! the old portable vault explicitly and preserves its files unchanged.
//!
//! The files under `tests/fixtures/compat-0.4.0/` were produced by
//! `generate_fixtures` (run with `--ignored`) on the pre-migration stack:
//! rand 0.8, sha2 0.10, hkdf/hmac 0.12, chacha20poly1305 0.10, argon2 0.5,
//! ed25519/x25519-dalek 2, rusqlite 0.32 (bundled SQLCipher). They are
//! committed and must not be regenerated casually — regenerated on a newer
//! stack they would prove nothing.

use std::path::{Path, PathBuf};

use gipny_libcore::crypto::{
    x3dh_initiate, x3dh_respond, AttachmentCipher, Identity, IdentityCard, PreKeyBundle,
    PreKeyPair, RatchetHeader, X3dhInitial,
};
use gipny_libcore::device_binding::BindingError;
use gipny_libcore::security::SecurityError;
use gipny_libcore::Vault;

const AD: &[u8] = b"compat-ad";
const SIGNED: &[u8] = b"compat message";
const CHUNK_KEY: [u8; 32] = [0x42; 32];
const CHUNK_AAD: &[u8] = b"chunk-aad";
const CHUNK_PT: &[u8] = b"the quick brown fox";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/compat-0.4.0")
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(name)).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

fn bob() -> Identity { Identity::from_bytes([0x11; 32], [0x22; 32]) }
fn spk() -> PreKeyPair { PreKeyPair::from_secret([0x33; 32]) }
fn opk() -> PreKeyPair { PreKeyPair::from_secret([0x44; 32]) }

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// Produces the wire fixture set. The legacy vault must remain the original
/// pre-migration fixture; never regenerate it using the new storage format.
/// Run only in GitHub Actions on the appropriate old stack:
/// `cargo test -p gipny-libcore --test compat -- --ignored generate_fixtures`
#[test]
#[ignore]
fn generate_fixtures() {
    let dir = fixtures();
    std::fs::create_dir_all(&dir).unwrap();

    // Fixed seeds → identity card, a signature, a prekey's public half.
    let bob = bob();
    std::fs::write(dir.join("bob.card"), bincode::serialize(&bob.card()).unwrap()).unwrap();
    std::fs::write(dir.join("bob.sig"), bob.sign(SIGNED)).unwrap();
    std::fs::write(dir.join("spk.pub"), spk().public()).unwrap();

    // Alice (random) opens a session with Bob (fixed) and sends one more
    // message on the ratchet. Bob's side of both is replayed by the test.
    let alice = Identity::generate();
    let bundle = PreKeyBundle::new(&bob, &spk(), Some((7, &opk())));
    let (mut a_state, initial) = x3dh_initiate(&alice, &bundle, b"first contact", AD).unwrap();
    std::fs::write(dir.join("x3dh.initial"), bincode::serialize(&initial).unwrap()).unwrap();
    let msg2 = a_state.encrypt(b"second message", AD).unwrap();
    std::fs::write(dir.join("msg2.bin"), bincode::serialize(&msg2).unwrap()).unwrap();

    // An attachment chunk under a fixed key: the nonce comes from the index,
    // so the ciphertext is a function of (key, index, aad, plaintext).
    let chunk = AttachmentCipher::from_key(CHUNK_KEY).encrypt_chunk(3, CHUNK_AAD, CHUNK_PT).unwrap();
    std::fs::write(dir.join("chunk.bin"), chunk).unwrap();

}

#[test]
fn fixed_seeds_derive_the_same_keys_and_signature() {
    let bob = bob();
    assert_eq!(bincode::serialize(&bob.card()).unwrap(), read("bob.card"), "identity card from fixed seeds");
    assert_eq!(&bob.sign(SIGNED)[..], &read("bob.sig")[..], "ed25519 signature is deterministic");
    assert_eq!(&spk().public()[..], &read("spk.pub")[..], "x25519 public key from fixed secret");
    let card: IdentityCard = bincode::deserialize(&read("bob.card")).unwrap();
    let sig: [u8; 64] = read("bob.sig")[..].try_into().unwrap();
    assert!(card.verify(SIGNED, &sig));
}

#[test]
fn x3dh_handshake_and_ratchet_message_from_0_4_0_still_open() {
    let initial: X3dhInitial = bincode::deserialize(&read("x3dh.initial")).unwrap();
    let (mut b_state, pt) = x3dh_respond(&bob(), &spk(), Some(&opk()), &initial, AD).unwrap();
    assert_eq!(pt, b"first contact");
    let (header, ct): (RatchetHeader, Vec<u8>) = bincode::deserialize(&read("msg2.bin")).unwrap();
    assert_eq!(b_state.decrypt(&header, &ct, AD).unwrap(), b"second message");
}

#[test]
fn attachment_chunk_from_0_4_0_still_decrypts_and_re_encrypts_identically() {
    let cipher = AttachmentCipher::from_key(CHUNK_KEY);
    assert_eq!(cipher.decrypt_chunk(3, CHUNK_AAD, &read("chunk.bin")).unwrap(), CHUNK_PT);
    assert_eq!(cipher.encrypt_chunk(3, CHUNK_AAD, CHUNK_PT).unwrap(), read("chunk.bin"));
}

#[test]
fn portable_vault_from_0_4_0_is_rejected_without_modifying_data() {
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(&fixtures().join("vault"), tmp.path());
    let before: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().map(|entry| {
        let path = entry.unwrap().path();
        let bytes = std::fs::read(&path).unwrap();
        (path, bytes)
    }).collect();
    assert!(matches!(Vault::open(tmp.path()), Err(SecurityError::DeviceBinding(BindingError::LegacyUnbound))));
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), before.len());
    for (path, bytes) in before { assert_eq!(std::fs::read(path).unwrap(), bytes); }
}
