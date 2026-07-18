//! Vault lifecycle + first-run recovery tests.
//!
//! Mirrors the decision logic of the app's `vault_create` command: a first run
//! killed between `Vault::create` and the end of boot leaves a vault with no
//! `display_name` in the DB — such a profile must be resumable with the same
//! passphrase, while a completed profile must stay a hard "already exists".
//! Timing lines are printed with a `[timing]` prefix; CI lifts them into the
//! job summary.

use std::time::Instant;

use gipny_libcore::db::Db;
use gipny_libcore::security::{ArgonParams, DuressMode, UnlockOutcome, Vault};

fn mk_of(outcome: UnlockOutcome) -> gipny_libcore::security::MasterKey {
    match outcome {
        UnlockOutcome::Primary(k) => k,
        _ => panic!("expected primary unlock"),
    }
}

#[test]
fn vault_roundtrip_and_wrong_pass() {
    let dir = tempfile::tempdir().unwrap();
    let params = ArgonParams { m: 8192, t: 1, p: 1 }; // cheap: logic-only test
    Vault::create_with_params(dir.path(), "correct horse", None, DuressMode::Wipe, 0, params)
        .unwrap();

    let vault = Vault::open(dir.path()).unwrap();
    assert!(matches!(vault.unlock("correct horse").unwrap(), UnlockOutcome::Primary(_)));
    assert!(vault.unlock("wrong pass").is_err());
    // and the right pass still works after a failed attempt
    assert!(matches!(vault.unlock("correct horse").unwrap(), UnlockOutcome::Primary(_)));
}

#[test]
fn duress_decoy_unlocks_separately() {
    let dir = tempfile::tempdir().unwrap();
    let params = ArgonParams { m: 8192, t: 1, p: 1 };
    Vault::create_with_params(dir.path(), "real pass", Some("duress pass"), DuressMode::Decoy, 0, params)
        .unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    assert!(matches!(vault.unlock("real pass").unwrap(), UnlockOutcome::Primary(_)));
    assert!(matches!(vault.unlock("duress pass").unwrap(), UnlockOutcome::Decoy(_)));
}

/// The app's first-run recovery decision, replicated end to end:
/// half-initialized profile (vault exists, no display_name) → resume;
/// completed profile → occupied; wrong passphrase → occupied.
#[test]
fn first_run_interrupted_is_resumable() {
    let dir = tempfile::tempdir().unwrap();
    let params = ArgonParams { m: 8192, t: 1, p: 1 };

    // Simulate a first run killed right after Vault::create: no DB writes yet.
    Vault::create_with_params(dir.path(), "pass-1", None, DuressMode::Wipe, 0, params).unwrap();

    // Retry with the same passphrase → must be treated as resumable.
    let vault = Vault::open(dir.path()).unwrap();
    let mk = mk_of(vault.unlock("pass-1").unwrap());
    let db = Db::open(&dir.path().join("data.db"), &mk).unwrap();
    assert!(db.get_setting("display_name").unwrap().is_none(), "fresh profile must be resumable");

    // Finish initialization (what boot + set display_name do).
    db.set_setting("display_name", b"alice").unwrap();
    drop(db);

    // Retry after completion → genuine duplicate.
    let vault = Vault::open(dir.path()).unwrap();
    let mk = mk_of(vault.unlock("pass-1").unwrap());
    let db = Db::open(&dir.path().join("data.db"), &mk).unwrap();
    assert!(db.get_setting("display_name").unwrap().is_some(), "completed profile must not be recreatable");
    drop(db);

    // Wrong passphrase on an existing vault → not resumable either.
    let vault = Vault::open(dir.path()).unwrap();
    assert!(vault.unlock("other-pass").is_err());
}

/// KDF cost measurement for both platform profiles. Run in --release for
/// meaningful numbers; CI publishes the `[timing]` lines to the job summary.
#[test]
fn kdf_timing_report() {
    for (label, params) in [
        ("desktop (256MiB, t=4)", ArgonParams { m: 262144, t: 4, p: 1 }),
        ("mobile  (64MiB,  t=3)", ArgonParams { m: 65536, t: 3, p: 1 }),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let t0 = Instant::now();
        Vault::create_with_params(dir.path(), "bench pass", None, DuressMode::Wipe, 0, params)
            .unwrap();
        let create_ms = t0.elapsed().as_millis();

        let vault = Vault::open(dir.path()).unwrap();
        let t1 = Instant::now();
        let _ = mk_of(vault.unlock("bench pass").unwrap());
        let unlock_ms = t1.elapsed().as_millis();

        println!("[timing] vault {label}: create {create_ms} ms, unlock {unlock_ms} ms");
        // Catch pathological regressions only — runners vary a lot.
        assert!(unlock_ms < 30_000, "unlock took {unlock_ms} ms — something is badly wrong");
    }
}
