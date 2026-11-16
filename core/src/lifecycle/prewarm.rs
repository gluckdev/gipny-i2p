//! Profile-owned client and relay preparation before unlocking.

use super::{
    boot_progress, boot_status, read_router_hint, resolve_bundled_router, router_settings_match,
};
use crate::{core, profile_dir, AppCtx};
use gipny_libcore::{net::I2pNode, security::Vault};
use std::sync::Arc;
use tauri::{AppHandle, State};

pub(super) mod builders;

#[cfg(test)]
#[path = "prewarm_tests.rs"]
mod tests;

/// A node started before any profile was opened, held for the profile it was
/// started for. Exactly one of these exists at a time: its destination and
/// its relay's are dropped when another profile is picked.
pub enum Prewarm {
    Building {
        profile: String,
        settings: gipny_libcore::router::RouterSettings,
        generation: u64,
        builders: builders::Builders<
            Arc<I2pNode>,
            Result<gipny_libcore::EphemeralRelay, gipny_libcore::net::NetError>,
            String,
        >,
    },
    Ready {
        profile: String,
        settings: gipny_libcore::router::RouterSettings,
        generation: u64,
        node: Arc<I2pNode>,
        /// The built-in relay's tunnels, building as soon as the router is up
        /// (serving nobody until the core claims it after unlock).
        relay: core::PrebuiltRelay,
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
    let dir = profile_dir(&ctx, &profile)?;
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
    // that arrived before deletion must never recreate the deleted directory.
    if !Vault::exists(&dir) {
        return Err("no such profile".into());
    }
    let settings = read_router_hint(&dir);
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
    let router_progress = progress.clone();
    let builders = builders::spawn(
        move || {
            I2pNode::prepare_router(&dir, settings, &router_progress).map_err(|e| format!("{e:?}"))
        },
        move |router| async move {
            I2pNode::start_on_router_with_progress(router, progress)
                .await
                .map(Arc::new)
                .map_err(|e| format!("{e:?}"))
        },
        |router| async move {
            // The channel opens only after the single router bootstrap. The
            // relay stays unclaimed and owns no profile identity/prekeys.
            let _router = router.ok_or(gipny_libcore::net::NetError::Closed)?;
            gipny_libcore::EphemeralRelay::start_unclaimed().await
        },
    );
    *slot = Some(Prewarm::Building {
        profile,
        settings,
        generation,
        builders,
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
    if let Some(Prewarm::Building { builders, .. }) = slot.as_ref() {
        if !builders.client_finished() {
            return Ok("building");
        }
        let Some(Prewarm::Building {
            profile,
            settings,
            generation,
            builders,
        }) = slot.take()
        else {
            unreachable!()
        };
        match builders.finish(close_prepared_relay).await {
            Some((node, relay)) => {
                *slot = Some(Prewarm::Ready {
                    profile,
                    settings,
                    generation,
                    node,
                    relay,
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

/// Stop and join the relay builder before dropping the client destination.
/// Dropping a running JoinHandle alone would detach its relay construction.
async fn drop_prepared<Node, Relay, Cleanup: std::future::Future<Output = ()>>(
    node: Node,
    relay: tokio::task::JoinHandle<Relay>,
    cleanup: impl FnOnce(Relay) -> Cleanup,
) {
    relay.abort();
    if let Ok(output) = relay.await {
        cleanup(output).await;
    }
    drop(node);
}

async fn close_prepared_relay(
    output: Result<gipny_libcore::EphemeralRelay, gipny_libcore::net::NetError>,
) {
    if let Ok(relay) = output {
        relay.shutdown().await;
    }
}

/// A historical "ready" state is insufficient after sleep or a network
/// change. Reject the whole prepared pair unless the live destination still
/// has both directions and its lease/outbound readiness.
async fn reuse_ready_destination<Node, Relay, Cleanup: std::future::Future<Output = ()>>(
    prepared: (Node, tokio::task::JoinHandle<Relay>),
    directions: (bool, bool),
    is_ready: bool,
    cleanup: impl FnOnce(Relay) -> Cleanup,
) -> Option<(Node, tokio::task::JoinHandle<Relay>)> {
    if destination_ready(directions, is_ready) {
        Some(prepared)
    } else {
        drop_prepared(prepared.0, prepared.1, cleanup).await;
        None
    }
}

/// Drop only this prewarm's destinations and await its owned builders. The
/// embedded router is process-global and remains alive with its original data
/// directory/settings until the application exits.
pub(super) async fn drop_prewarm(p: Prewarm) {
    match p {
        Prewarm::Building { builders, .. } => {
            builders.cancel(close_prepared_relay).await;
        }
        Prewarm::Ready { node, relay, .. } => {
            drop_prepared(node, relay, close_prepared_relay).await;
        }
    }
    // No timer is needed after the owned tasks and synchronous destination
    // destructors have completed. The process-global router keeps running.
}

/// The prewarmed node, if it fits this profile. Anything else is torn down
/// here, so the caller can safely start its own.
pub(super) async fn take_prewarmed(
    ctx: &State<'_, AppCtx>,
    app: &AppHandle,
    profile: &str,
    settings: gipny_libcore::router::RouterSettings,
) -> Option<(Arc<I2pNode>, core::PrebuiltRelay)> {
    let taken = ctx.prewarm.lock().await.take()?;
    if taken.profile() != profile
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
        Prewarm::Ready { node, relay, .. } => (node, relay),
        Prewarm::Building { builders, .. } => {
            // Still building: wait for it rather than start a second router.
            // Its progress is already reaching the same boot screen.
            match builders.finish(close_prepared_relay).await {
                Some(ready) => ready,
                _ => return None,
            }
        }
    };
    let (directions, is_ready) = current_readiness(&ready.0).await;
    let Some(ready) =
        reuse_ready_destination(ready, directions, is_ready, close_prepared_relay).await
    else {
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
