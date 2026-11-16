use super::*;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

struct BuilderResources {
    dropped: Arc<AtomicUsize>,
    vault_existed_at_drop: Arc<AtomicBool>,
    dir: std::path::PathBuf,
}

impl Drop for BuilderResources {
    fn drop(&mut self) {
        self.vault_existed_at_drop
            .store(Vault::exists(&self.dir), Ordering::SeqCst);
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

fn profile_fixture(ctx: &AppCtx, profile: &str) -> std::path::PathBuf {
    let dir = profile_dir(ctx, profile).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    // These tests exercise lifecycle ownership, not vault cryptography/device
    // enrollment (covered in libcore). Only its existence check needs a file.
    std::fs::write(Vault::path_of(&dir), b"lifecycle fixture").unwrap();
    dir
}

async fn install_build(ctx: &AppCtx, profile: &str) -> (Arc<AtomicUsize>, Arc<AtomicBool>) {
    let dropped = Arc::new(AtomicUsize::new(0));
    let existed = Arc::new(AtomicBool::new(false));
    let resource = BuilderResources {
        dropped: dropped.clone(),
        vault_existed_at_drop: existed.clone(),
        dir: profile_dir(ctx, profile).unwrap(),
    };
    let (started, running) = tokio::sync::oneshot::channel();
    let builders = super::super::prewarm::builders::spawn(
        || Ok::<_, String>(()),
        |_| async move {
            let _resource = resource;
            started.send(()).unwrap();
            std::future::pending().await
        },
        |_| std::future::pending(),
    );
    running.await.unwrap();
    *ctx.prewarm.lock().await = Some(Prewarm::Building {
        profile: profile.to_owned(),
        settings: Default::default(),
        generation: ctx.invalidate_prewarm(),
        builders,
    });
    (dropped, existed)
}

#[tokio::test]
async fn delete_joins_the_matching_builder_before_erasing_the_directory() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = AppCtx::new(temp.path().to_owned());
    let dir = profile_fixture(&ctx, "alice");
    let (dropped, existed) = install_build(&ctx, "alice").await;
    delete_profile_data(&ctx, "alice").await.unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(
        existed.load(Ordering::SeqCst),
        "builder must stop before erase"
    );
    assert!(ctx.prewarm.lock().await.is_none());
    assert!(!dir.exists());
}

#[tokio::test]
async fn deleting_another_profile_preserves_the_prepared_owner() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = AppCtx::new(temp.path().to_owned());
    profile_fixture(&ctx, "alice");
    let bob = profile_fixture(&ctx, "bob");
    let (dropped, _) = install_build(&ctx, "alice").await;
    delete_profile_data(&ctx, "bob").await.unwrap();
    assert!(!bob.exists());
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(
        ctx.prewarm.lock().await.as_ref().unwrap().profile(),
        "alice"
    );
    delete_profile_data(&ctx, "alice").await.unwrap();
}

#[tokio::test]
async fn deletion_cannot_cross_the_boot_ownership_handoff() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = AppCtx::new(temp.path().to_owned());
    let dir = profile_fixture(&ctx, "alice");
    let opening = ctx.begin_profile_operation().unwrap();
    // A boot already took prewarm from the slot but has not published
    // ctx.profile yet. Checking only the active profile used to miss this.
    assert!(ctx.profile.lock().await.is_none());
    assert!(delete_profile_data(&ctx, "alice")
        .await
        .unwrap_err()
        .contains("in progress"));
    assert!(Vault::exists(&dir));
    drop(opening);
    delete_profile_data(&ctx, "alice").await.unwrap();
    assert!(!dir.exists());
}

#[tokio::test]
async fn foreground_unlock_wins_while_background_replacement_is_joining() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = Arc::new(AppCtx::new(temp.path().to_owned()));
    profile_fixture(&ctx, "alice");
    let (dropped, _) = install_build(&ctx, "alice").await;
    let (joining, background_joining) = tokio::sync::oneshot::channel();
    let (continue_cleanup, cleanup_allowed) = tokio::sync::oneshot::channel();
    let background = tokio::spawn({
        let ctx = ctx.clone();
        async move {
            let mut slot = ctx.prewarm.lock().await;
            assert!(ctx.prewarm_may_start());
            let old = slot.take().unwrap();
            joining.send(()).unwrap();
            cleanup_allowed.await.unwrap();
            drop_prewarm(old).await;
            // This is the second gate in prewarm_network, after the await.
            assert!(!ctx.prewarm_may_start());
        }
    });
    background_joining.await.unwrap();
    // Foreground must succeed immediately, although the background still
    // owns the slot. It must not receive a false busy error from prewarm.
    let foreground = ctx
        .begin_profile_operation()
        .expect("unlock takes priority");
    assert!(ctx.prewarm.try_lock().is_err());
    continue_cleanup.send(()).unwrap();
    let slot = ctx.prewarm.lock().await;
    assert!(slot.is_none());
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    background.await.unwrap();
    drop(slot);
    drop(foreground);
    assert!(ctx.prewarm_may_start());
}

#[tokio::test]
async fn active_profile_deletion_remains_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = AppCtx::new(temp.path().to_owned());
    let dir = profile_fixture(&ctx, "alice");
    *ctx.profile.lock().await = Some("alice".into());
    assert_eq!(
        delete_profile_data(&ctx, "alice").await.unwrap_err(),
        "profile is active, lock first"
    );
    assert!(Vault::exists(&dir));
}

#[tokio::test]
async fn wiped_unlock_joins_prepared_owner_and_scrubs_global_logs_before_return() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = AppCtx::new(temp.path().to_owned());
    let dir = profile_fixture(&ctx, "alice");
    let (dropped, _) = install_build(&ctx, "alice").await;
    std::fs::write(temp.path().join("debug.log"), b"private log").unwrap();
    std::fs::write(temp.path().join("debug.prev.log"), b"previous log").unwrap();
    let _operation = ctx.begin_profile_operation().unwrap();
    let outcome = unlock_profile_with(&ctx, "alice", &dir, || {
        // Match libcore's contract: erasure happens inside authentication,
        // before returning Wiped for either duress or the attempt limit.
        gipny_libcore::security::secure_wipe_dir(&dir).unwrap();
        Ok(UnlockOutcome::Wiped)
    })
    .await
    .unwrap();
    assert!(matches!(outcome, UnlockOutcome::Wiped));
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(ctx.prewarm.lock().await.is_none());
    assert!(!Vault::exists(&dir));
    assert!(!temp.path().join("debug.log").exists());
    assert!(!temp.path().join("debug.prev.log").exists());
}

#[tokio::test]
async fn partial_wipe_error_still_joins_the_prepared_owner() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = AppCtx::new(temp.path().to_owned());
    let dir = profile_fixture(&ctx, "alice");
    let (dropped, _) = install_build(&ctx, "alice").await;
    let error = unlock_profile_with(&ctx, "alice", &dir, || {
        std::fs::remove_file(Vault::path_of(&dir)).unwrap();
        Err("wipe failed later".into())
    })
    .await
    .err()
    .expect("partial wipe error must be preserved");
    assert_eq!(error, "wipe failed later");
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(ctx.prewarm.lock().await.is_none());
}

#[tokio::test]
async fn ordinary_failed_password_keeps_the_prepared_owner() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = AppCtx::new(temp.path().to_owned());
    let dir = profile_fixture(&ctx, "alice");
    let (dropped, _) = install_build(&ctx, "alice").await;
    let error = unlock_profile_with(&ctx, "alice", &dir, || Err("wrong password".into()))
        .await
        .err()
        .expect("incorrect password must still fail");
    assert_eq!(error, "wrong password");
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert!(ctx.prewarm.lock().await.is_some());
    delete_profile_data(&ctx, "alice").await.unwrap();
}

#[tokio::test]
async fn obsolete_generation_cannot_report_or_retain_a_current_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = AppCtx::new(temp.path().to_owned());
    profile_fixture(&ctx, "alice");
    let (dropped, _) = install_build(&ctx, "alice").await;
    assert_eq!(
        super::super::prewarm::prewarm_state(&ctx).await.unwrap(),
        "building"
    );
    // Simulate an invalidation delivered before a queued status poll. The old
    // task must be joined even though its profile/settings still match.
    ctx.invalidate_prewarm();
    assert_eq!(
        super::super::prewarm::prewarm_state(&ctx).await.unwrap(),
        "off"
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(ctx.prewarm.lock().await.is_none());
}
