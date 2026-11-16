//! Profile operations that must retain ownership until their builders stop.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use gipny_libcore::security::{UnlockOutcome, Vault};

use super::{drop_prewarm, Prewarm};
use crate::{diagnostics::scrub_debug_log, err, profile_dir, AppCtx};

pub(crate) struct ProfileOperation<'a>(&'a AtomicBool);

impl Drop for ProfileOperation<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl AppCtx {
    /// Never queue a destructive action behind a minutes-long boot. The UI
    /// must retry against the resulting profile state instead of a stale view.
    pub(crate) fn begin_profile_operation(&self) -> Result<ProfileOperation<'_>, String> {
        self.profile_operation
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| ProfileOperation(&self.profile_operation))
            .map_err(|_| "another profile operation is in progress".into())
    }

    /// Called only with the prewarm slot locked. A foreground command may
    /// claim its guard immediately after this check, but then waits for that
    /// same slot before authenticating/deleting/locking; it owns the next turn.
    pub(super) fn prewarm_may_start(&self) -> bool {
        // A read-only gate: even a short try_lock here could cause a
        // simultaneous foreground try_lock on another thread to report busy.
        !self.profile_operation.load(Ordering::Acquire)
    }
}

pub(crate) async fn cancel_pending_prewarm(ctx: &AppCtx) {
    let mut slot = ctx.prewarm.lock().await;
    if let Some(prepared) = slot.take() {
        drop_prewarm(prepared).await;
    }
}

async fn cancel_matching_prewarm(slot: &mut Option<Prewarm>, profile: &str) {
    if slot
        .as_ref()
        .is_some_and(|prepared| prepared.profile() == profile)
    {
        // Keep the slot locked until both the outer and child builders have
        // stopped; only then may deletion continue or a new prewarm start.
        drop_prewarm(slot.take().expect("matching prewarm exists")).await;
    }
}

pub(crate) async fn delete_profile_data(ctx: &AppCtx, profile: &str) -> Result<(), String> {
    let _operation = ctx.begin_profile_operation()?;
    let dir = profile_dir(ctx, profile)?;
    if ctx.profile.lock().await.as_deref() == Some(profile) {
        return Err("profile is active, lock first".into());
    }
    let mut slot = ctx.prewarm.lock().await;
    cancel_matching_prewarm(&mut slot, profile).await;
    if dir.exists() {
        gipny_libcore::security::secure_wipe_dir(&dir).map_err(err)?;
        let _ = std::fs::remove_dir(&dir);
    }
    Ok(())
}

/// Both normal unlock and resuming interrupted profile creation can wipe the
/// vault. Their callers hold the profile-operation guard across the whole boot.
pub(crate) async fn unlock_profile_vault(
    ctx: &AppCtx,
    profile: &str,
    dir: &Path,
    vault: &Vault,
    pass: &str,
) -> Result<UnlockOutcome, String> {
    unlock_profile_with(ctx, profile, dir, || vault.unlock(pass).map_err(err)).await
}

async fn unlock_profile_with(
    ctx: &AppCtx,
    profile: &str,
    dir: &Path,
    authenticate: impl FnOnce() -> Result<UnlockOutcome, String>,
) -> Result<UnlockOutcome, String> {
    let mut slot = ctx.prewarm.lock().await;
    let outcome = authenticate();
    // Vault::unlock performs duress/attempt-limit erasure synchronously. Join
    // our builders before reporting its result, also on a partial wipe that
    // removed the vault but failed later. The process-global router remains
    // alive; this does not claim to stop its public cache writes.
    if matches!(&outcome, Ok(UnlockOutcome::Wiped)) || !Vault::exists(dir) {
        cancel_matching_prewarm(&mut slot, profile).await;
        scrub_debug_log(&ctx.base_dir);
    }
    outcome
}

#[cfg(test)]
mod tests;
