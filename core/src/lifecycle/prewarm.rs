//! Profile-owned client and relay preparation before unlocking.

use super::{
    boot_progress, boot_status, read_router_hint, resolve_bundled_router, router_settings_match,
};
use crate::{profile_dir, AppCtx};
use gipny_libcore::{net::I2pNode, security::Vault};
use std::sync::Arc;
use tauri::{AppHandle, State};

/// The outgoing destination can be prepared before unlock. The stable personal
/// terminal can only be constructed once its private key is available in the vault.
pub struct Preparation(pub(super) tokio::task::JoinHandle<Result<Arc<I2pNode>, String>>);
impl Drop for Preparation {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl Preparation {
    async fn finish(mut self) -> Option<Arc<I2pNode>> {
        (&mut self.0).await.ok()?.ok()
    }
    async fn cancel(mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }
}
pub enum Prewarm {
    Building {
        profile: String,
        settings: gipny_libcore::router::RouterSettings,
        generation: u64,
        builder: Preparation,
    },
    Ready {
        profile: String,
        settings: gipny_libcore::router::RouterSettings,
        generation: u64,
        node: Arc<I2pNode>,
    },
}

impl Prewarm {
    pub(super) fn profile(&self) -> &str {
        match self {
            Self::Building { profile, .. } | Self::Ready { profile, .. } => profile,
        }
    }

    fn settings(&self) -> gipny_libcore::router::RouterSettings {
        match self {
            Self::Building { settings, .. } | Self::Ready { settings, .. } => *settings,
        }
    }

    fn generation(&self) -> u64 {
        match self {
            Self::Building { generation, .. } | Self::Ready { generation, .. } => *generation,
        }
    }
}

impl AppCtx {
    pub(super) fn invalidate_prewarm(&self) -> u64 {
        self.prewarm_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1
    }

    fn prewarm_is_current(&self, generation: u64) -> bool {
        self.prewarm_generation
            .load(std::sync::atomic::Ordering::Acquire)
            == generation
    }
}

/// Start building i2p tunnels now, for the profile about to be opened.
///
/// The interface calls this as soon as it knows which profile that is — on the
/// unlock screen, and again after a logout — so the router, its tunnels and
/// our destination are up by the time the password is typed. If the guess was
/// wrong (another profile is picked), the node is dropped and a new one built.
#[tauri::command]
pub(crate) async fn prewarm_network(
    profile: String,
    ctx: State<'_, AppCtx>,
    app: AppHandle,
) -> Result<(), String> {
    let dir = if profile.trim().is_empty() {
        ctx.base_dir.clone()
    } else {
        profile_dir(&ctx, &profile)?
    };
    let mut slot = ctx.prewarm.lock().await;
    // Background preparation never holds the foreground-operation guard:
    // unlock can claim it immediately, then wait for this slot's cleanup.
    // Once boot has taken its resources, this gate suppresses stale UI work.
    if !ctx.prewarm_may_start() {
        return Ok(());
    }
    // A profile is already open and owns its destinations.
    if ctx.core.lock().await.is_some() {
        return Ok(());
    }
    // Recheck while holding the same locks as deletion/wipe. A prewarm request
    // for a named profile that arrived before deletion must never recreate the
    // deleted directory; an empty profile name prewarms in base_dir for first-run creation.
    if !profile.trim().is_empty() && !Vault::exists(&dir) {
        return Err("no such profile".into());
    }
    let settings = if profile.trim().is_empty() {
        gipny_libcore::router::RouterSettings::default()
    } else {
        read_router_hint(&dir)
    };
    if let Some(p) = slot.as_ref() {
        if p.profile() == profile
            && router_settings_match(p.settings(), settings)
            && ctx.prewarm_is_current(p.generation())
        {
            return Ok(());
        }
    }
    // Strictly take-then-drop: the old node's destinations go before new ones
    // are built on the same router.
    let generation = ctx.invalidate_prewarm();
    if let Some(old) = slot.take() {
        drop_prewarm(old).await;
    }
    // Unlock/delete/lock may have claimed the foreground operation while the
    // previous preparation was joining. Yield the empty slot to that action.
    if !ctx.prewarm_may_start() {
        return Ok(());
    }
    resolve_bundled_router(&app);
    let progress = Some(boot_progress(&app));
    let builder = Preparation(tokio::spawn(async move {
        let router =
            I2pNode::prepare_router(&dir, settings, &progress).map_err(|e| format!("{e:?}"))?;
        I2pNode::start_on_router_with_progress(router, progress)
            .await
            .map(Arc::new)
            .map_err(|e| format!("{e:?}"))
    }));
    *slot = Some(Prewarm::Building {
        profile,
        settings,
        generation,
        builder,
    });
    Ok(())
}

/// What the unlock screen shows while it waits: `building`, `ready`, `off`.
#[tauri::command]
pub(crate) async fn prewarm_status(ctx: State<'_, AppCtx>) -> Result<&'static str, String> {
    prewarm_state(&ctx).await
}

pub(super) async fn prewarm_state(ctx: &AppCtx) -> Result<&'static str, String> {
    let mut slot = ctx.prewarm.lock().await;
    if slot
        .as_ref()
        .is_some_and(|prepared| !ctx.prewarm_is_current(prepared.generation()))
    {
        drop_prewarm(slot.take().expect("stale preparation exists")).await;
        return Ok("off");
    }
    // A finished build is only noticed when someone looks; do that here so the
    // state this reports is the real one.
    if let Some(Prewarm::Building { builder, .. }) = slot.as_ref() {
        if !builder.0.is_finished() {
            return Ok("building");
        }
        let Some(Prewarm::Building {
            profile,
            settings,
            generation,
            builder,
        }) = slot.take()
        else {
            unreachable!()
        };
        match builder.finish().await {
            Some(node) => {
                *slot = Some(Prewarm::Ready {
                    profile,
                    settings,
                    generation,
                    node,
                });
            }
            // A failed prewarm is not an error the person has to act on: the
            // ordinary boot path will try again, out loud, after the password.
            _ => return Ok("off"),
        }
    }
    Ok(match slot.as_ref() {
        Some(Prewarm::Ready { node, .. }) => {
            let (directions, is_ready) = current_readiness(node).await;
            if destination_ready(directions, is_ready) {
                "ready"
            } else {
                "building"
            }
        }
        Some(Prewarm::Building { .. }) => "building",
        None => "off",
    })
}

async fn current_readiness(node: &I2pNode) -> ((bool, bool), bool) {
    let destination = node.destination().await;
    (
        destination.tunnel_directions_ready(),
        destination.is_ready(),
    )
}

fn destination_ready(directions: (bool, bool), is_ready: bool) -> bool {
    directions.0 && directions.1 && is_ready
}

pub(super) async fn drop_prewarm(p: Prewarm) {
    if let Prewarm::Building { builder, .. } = p {
        builder.cancel().await;
    }
}

/// The prewarmed node, if it fits this profile. Anything else is torn down
/// here, so the caller can safely start its own.
pub(super) async fn take_prewarmed(
    ctx: &State<'_, AppCtx>,
    app: &AppHandle,
    profile: &str,
    settings: gipny_libcore::router::RouterSettings,
) -> Option<Arc<I2pNode>> {
    let taken = ctx.prewarm.lock().await.take()?;
    if (!taken.profile().trim().is_empty() && taken.profile() != profile)
        || !router_settings_match(taken.settings(), settings)
        || !ctx.prewarm_is_current(taken.generation())
    {
        boot_status(
            &app,
            "router",
            "active",
            if taken.profile() != profile {
                "the destinations were prepared for another profile; rebuilding them"
            } else if !ctx.prewarm_is_current(taken.generation()) {
                "discarding destinations from an obsolete preparation"
            } else {
                "discarding destinations prepared with another settings hint; router settings require an application restart"
            },
        );
        drop_prewarm(taken).await;
        return None;
    }
    let ready = match taken {
        Prewarm::Ready { node, .. } => node,
        Prewarm::Building { builder, .. } => {
            // Still building: wait for it rather than start a second router.
            // Its progress is already reaching the same boot screen.
            match builder.finish().await {
                Some(ready) => ready,
                _ => return None,
            }
        }
    };
    let (directions, is_ready) = current_readiness(&ready).await;
    if !destination_ready(directions, is_ready) {
        boot_status(
            app,
            "tunnels",
            "active",
            "prepared destination is no longer ready; rebuilding destinations",
        );
        return None;
    };
    // The screen's own listener may have attached after these stages went by.
    // Replay success only after checking the current destination, not its age.
    boot_status(
        app,
        "router",
        "done",
        "router ready (started before unlocking)",
    );
    boot_status(app, "session", "done", "address ready");
    boot_status(app, "inbound", "done", "prepared inbound tunnels are ready");
    boot_status(
        app,
        "outbound",
        "done",
        "prepared outbound tunnels are ready",
    );
    boot_status(
        app,
        "tunnels",
        "done",
        "prepared destination readiness rechecked",
    );
    Some(ready)
}

#[cfg(test)]
#[path = "prewarm_tests.rs"]
mod tests;
