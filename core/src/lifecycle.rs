//! Profile startup, router prewarming, and persistent router settings.

use std::sync::Arc;

use gipny_libcore::net::I2pNode;
use gipny_libcore::security::{UnlockOutcome, Vault};
use tauri::{AppHandle, Emitter, State};

use crate::core::Core;
use crate::{core_of, err, profile_dir, AppCtx};

mod profile_operations;
pub(crate) use profile_operations::{
    cancel_pending_prewarm, delete_profile_data, unlock_profile_vault,
};

pub(crate) mod prewarm;
use prewarm::take_prewarmed;
pub use prewarm::Prewarm;

/// How much transit traffic the bundled router carries. See
/// [`gipny_libcore::router::TransitProfile`] — this is an anonymity setting.
const SETTING_ROUTER_TRANSIT: &str = "router_transit";
/// Whether the router may ask the home gateway to open its ports (UPnP).
const SETTING_ROUTER_UPNP: &str = "router_upnp";

/// One step of opening a profile, for the screen that would otherwise show a
/// disabled button for three minutes. `stage` is a stable id the interface
/// turns into its own wording; `detail` is the technical line (the same one
/// that goes to stderr), shown under «технические подробности».
#[derive(Clone, serde::Serialize)]
pub(crate) struct BootStatus {
    pub(crate) stage: &'static str,
    pub(crate) state: &'static str,
    pub(crate) detail: String,
}

fn boot_status(
    app: &AppHandle,
    stage: &'static str,
    state: &'static str,
    detail: impl Into<String>,
) {
    let _ = app.emit(
        "boot_status",
        BootStatus {
            stage,
            state,
            detail: detail.into(),
        },
    );
}

/// The callback libcore reports router and tunnel progress through. Stage ids come
/// from there (`router`, `router-reused`, `tunnels`, `tunnels-done`, `session`,
/// `session-done`); anything unknown still reaches the technical log.
fn boot_progress(app: &AppHandle) -> gipny_libcore::router::BootProgress {
    let app = app.clone();
    Arc::new(move |stage: &str, detail: &str| {
        let (stage, state) = match stage {
            "router" => ("router", "active"),
            "router-ready" | "router-reused" => ("router", "done"),
            "router-failed" => ("router", "failed"),
            "session" => ("session", "active"),
            "session-done" => ("session", "done"),
            "session-failed" => ("session", "failed"),
            "inbound" => ("inbound", "active"),
            "inbound-done" => ("inbound", "done"),
            "outbound" => ("outbound", "active"),
            "outbound-done" => ("outbound", "done"),
            "tunnels" => ("tunnels", "active"),
            "tunnels-done" => ("tunnels", "done"),
            "tunnels-failed" => ("tunnels", "failed"),
            // Not a step: a verdict on the network while the tunnel steps run.
            // "none" — nothing has answered for long enough that the device
            // looks offline; "ok" — something did, after such a verdict.
            "network-none" => ("network", "none"),
            "network-ok" => ("network", "ok"),
            // Anything else is a stage this screen has no row for. It used to
            // be reported as "router active", which meant a stage nobody had
            // heard of lit up a step that was doing something else entirely.
            // The row lookup misses on purpose, so the line still reaches the
            // technical log without claiming a step ran.
            _ => ("note", ""),
        };
        boot_status(&app, stage, state, detail);
    })
}

/// Point the router at the network database snapshot shipped as a Tauri
/// resource (the router itself is compiled in).
///
/// `resource_dir()` is authoritative: the deb and AppImage put resources under
/// `usr/lib/<product>/resources/`, which relative probing does not reach. On
/// Android the snapshot is compiled into the binary instead.
///
/// Called before *any* router start — the prewarm below runs before `boot`.
fn resolve_bundled_router(app: &AppHandle) {
    #[cfg(not(target_os = "android"))]
    if std::env::var_os("GIPNY_I2P_SEED").is_none() {
        use tauri::Manager;
        if let Ok(res) = app.path().resource_dir() {
            for cand in [
                res.join("i2pd-netdb-seed.tar.gz"),
                res.join("resources").join("i2pd-netdb-seed.tar.gz"),
            ] {
                if cand.exists() {
                    std::env::set_var("GIPNY_I2P_SEED", cand);
                    break;
                }
            }
        }
    }
    #[cfg(target_os = "android")]
    let _ = app;
}

/// Router knobs live in the encrypted database, which is unreadable until the
/// password is typed — and the router has to start before that. So the last
/// values used are left beside the profile in the clear.
///
/// This deliberately persists the transit/UPnP choices in cleartext,
/// independently of diagnostics (release has no router log). It contains no
/// keys or messages and is inside the profile directory for a duress wipe.
fn router_hint_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("router.hint")
}

fn read_router_hint(dir: &std::path::Path) -> gipny_libcore::router::RouterSettings {
    let raw = std::fs::read_to_string(router_hint_path(dir)).unwrap_or_default();
    let mut it = raw.split_whitespace();
    let transit = gipny_libcore::router::TransitProfile::parse(it.next().unwrap_or_default());
    // Named, so a hint of an older layout cannot be misread: absent means on.
    let upnp = it
        .find_map(|word| word.strip_prefix("upnp="))
        .unwrap_or_default();
    gipny_libcore::router::RouterSettings {
        transit,
        upnp: gipny_libcore::router::Upnp::parse(upnp),
    }
}

fn write_router_hint(dir: &std::path::Path, s: gipny_libcore::router::RouterSettings) {
    let _ = std::fs::write(
        router_hint_path(dir),
        format!("{} upnp={}\n", s.transit.as_str(), s.upnp.as_str()),
    );
}

/// Whether a node started for `have` can be handed to a profile that wants
/// `want`.
fn router_settings_match(
    have: gipny_libcore::router::RouterSettings,
    want: gipny_libcore::router::RouterSettings,
) -> bool {
    have == want
}

pub(crate) async fn boot(
    ctx: &State<'_, AppCtx>,
    app: AppHandle,
    vault: Arc<Vault>,
    pass: &str,
    profile: &str,
    dir: &std::path::Path,
) -> Result<Option<String>, String> {
    // As early as this profile's own startup gets: before the vault is even
    // unlocked, and well before the ~1-3 min router wait below. A staged
    // Windows update means the app closes and reopens once here, rather than
    // after the user has sat through both of those for nothing.
    match gipny_libcore::update::apply_staged_windows_installer(dir, env!("CARGO_PKG_VERSION")) {
        Ok(true) => std::process::exit(0),
        Ok(false) => {}
        Err(error) => {
            // Keep the old application usable and retain the staged installer
            // when CreateProcess, verification or a previous attempt failed.
            let _ = app.emit(
                "core_event",
                crate::core::CoreEvent::UpdateFailed {
                    reason: error.to_string(),
                },
            );
        }
    }
    boot_status(&app, "vault", "active", "unlocking the vault (argon2id)");
    // Argon2id with 256 MiB takes a second or three on a desktop and longer on
    // a tired laptop, at 100% of a core. Saying so beats a frozen button.
    let outcome = unlock_profile_vault(ctx, profile, dir, &vault, pass)
        .await
        .map_err(|e| {
            boot_status(&app, "vault", "failed", format!("{e}"));
            e
        })?;
    // The decoy key is freshly random and unrelated to the primary master key,
    // so it cannot open data.db — SQLCipher rejects it and the unlock screen
    // showed a "bad key" error, in front of whoever was applying the coercion.
    // That is the precise situation duress mode exists to avoid, so the decoy
    // gets a database of its own. It is created empty on first use and looks
    // like a profile nobody has written much in, which is the point.
    let (mk, db_name) = match outcome {
        UnlockOutcome::Primary(k) => (k, "data.db"),
        UnlockOutcome::Decoy(k) => (k, "decoy.db"),
        UnlockOutcome::Wiped => {
            return Err("wiped".into());
        }
    };
    // A failure here used to leave the vault step spinning forever: the key was
    // right — that part said so — and the only thing after it was a `?`, so
    // nothing ever said otherwise. The person saw argon2id apparently still
    // running, minutes after it had finished, with no error anywhere.
    let db = match gipny_libcore::db::Db::open(&dir.join(db_name), &mk) {
        Ok(db) => Arc::new(db),
        Err(e) => {
            let msg = err(e);
            boot_status(
                &app,
                "vault",
                "failed",
                format!("profile opened the key but not the database: {msg}"),
            );
            return Err(msg);
        }
    };
    boot_status(&app, "vault", "done", format!("profile opened ({db_name})"));
    resolve_bundled_router(&app);
    // Router knobs are per profile and read once, here: i2pd takes them on its
    // command line and exposes no way to change them afterwards.
    let settings = gipny_libcore::router::RouterSettings {
        transit: db
            .get_setting(SETTING_ROUTER_TRANSIT)
            .ok()
            .flatten()
            .and_then(|v| String::from_utf8(v).ok())
            .map(|v| gipny_libcore::router::TransitProfile::parse(&v))
            .unwrap_or_default(),
        upnp: db
            .get_setting(SETTING_ROUTER_UPNP)
            .ok()
            .flatten()
            .and_then(|v| String::from_utf8(v).ok())
            .map(|v| gipny_libcore::router::Upnp::parse(&v))
            .unwrap_or_default(),
    };
    // So the next launch can start the router before the password (the vault
    // this was just read from is unreadable at that point).
    write_router_hint(dir, settings);
    // Ephemeral per-session i2p address: the node regenerates its destination
    // every launch (identity is the vault keypair, and the relay routes by that
    // key, not by address — so nothing about the address needs persisting).
    let (node, prebuilt_relay) = match take_prewarmed(ctx, &app, profile, settings).await {
        Some((node, relay)) => (node, Some(relay)),
        // start_with_progress has already named the step that failed — the
        // router, the address, or the tunnels — so nothing is reported here.
        // It used to call every failure of this call "router failed", which put
        // a ten-minute tunnel failure under a step that was long since done.
        None => (
            Arc::new(
                I2pNode::start_with_progress(dir, settings, Some(boot_progress(&app)))
                    .await
                    .map_err(err)?,
            ),
            None,
        ),
    };
    let warning: Option<String> = None;
    boot_status(&app, "core", "active", "starting the messenger core");
    let (core, mut events) = Core::start(dir.to_path_buf(), db, node, prebuilt_relay)
        .await
        .map_err(|e| {
            boot_status(&app, "core", "failed", format!("{e:?}"));
            err(e)
        })?;
    boot_status(&app, "core", "done", "core running");
    let app2 = app.clone();
    tokio::spawn(async move {
        while let Some(e) = events.recv().await {
            // The unlock screen waits for the relay and the network, and those
            // only exist as core events. Mirror the two it needs so it does not
            // depend on the main view's listener being attached yet.
            match &e {
                crate::core::CoreEvent::RelayInfoChanged { info } => match &info.hosted {
                    crate::core::HostedRelayState::Ready { address } => {
                        boot_status(
                            &app2,
                            "relay",
                            "done",
                            format!(
                                "built-in relay ready at {}",
                                &address[..address.len().min(16)]
                            ),
                        );
                        // Joining the network is the next thing that happens,
                        // and it waits on nodes that answer. Without this the
                        // step jumps from empty to a verdict with nothing in
                        // between, which is the whole tell of a step that
                        // skipped.
                        boot_status(&app2, "dht", "active", "looking for the relay network");
                    }
                    crate::core::HostedRelayState::Failed { reason } => {
                        boot_status(&app2, "relay", "failed", reason.clone());
                    }
                    crate::core::HostedRelayState::Starting => {
                        boot_status(&app2, "relay", "active", "building the relay's tunnels");
                    }
                    // Not a failure and not a success: in external mode there is
                    // no built-in relay to raise. It said nothing at all, and a
                    // step left empty is back-filled to a green tick by the next
                    // stage that speaks, so a person was told a relay had been
                    // built when we never tried.
                    crate::core::HostedRelayState::Off => {
                        boot_status(
                            &app2,
                            "relay",
                            "skipped",
                            "not our relay to raise — an external one is configured",
                        );
                        // Nothing announces this node, so no join follows and
                        // no verdict ever arrives. Without a line of its own the
                        // step would stay empty on a screen where every other
                        // step has a state.
                        boot_status(
                            &app2,
                            "dht",
                            "skipped",
                            "no relay of ours to announce — the external one handles it",
                        );
                    }
                },
                crate::core::CoreEvent::DhtJoined { peers, reached } => {
                    // Zero peers is not a finished step: bootstrap returns
                    // promptly on an empty candidate list, so reporting "done"
                    // here told the person the relay network was up on a device
                    // that had answered nobody. It is a step that decided it
                    // could not run, and it says which of the two it was.
                    //
                    // The event is matched by reference, so the fields are
                    // references too, and both arms have to be the same type:
                    // one &str and one String does not unify.
                    let (state, detail) = if *reached {
                        ("done", format!("relay network: {peers} node(s) known"))
                    } else {
                        (
                            "skipped",
                            "no relay node answered yet — we look again on the next tick"
                                .to_string(),
                        )
                    };
                    boot_status(&app2, "dht", state, detail);
                }
                _ => {}
            }
            let _ = app2.emit("core_event", &e);
        }
    });
    *ctx.profile.lock().await = Some(profile.to_string());
    *ctx.vault.lock().await = Some(vault);
    *ctx.core.lock().await = Some(core);
    Ok(warning)
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct RouterSettingsDto {
    /// "frugal" | "balanced" | "generous"
    transit: String,
    /// "on" | "off"; absent from an older UI means on.
    #[serde(default = "upnp_default")]
    upnp: String,
}

fn upnp_default() -> String {
    gipny_libcore::router::Upnp::default().as_str().to_string()
}

#[tauri::command]
pub(crate) async fn get_router_settings(
    ctx: State<'_, AppCtx>,
) -> Result<RouterSettingsDto, String> {
    let db = core_of(&ctx).await?.db().clone();
    let transit = db
        .get_setting(SETTING_ROUTER_TRANSIT)
        .map_err(err)?
        .and_then(|v| String::from_utf8(v).ok())
        .unwrap_or_else(|| {
            gipny_libcore::router::TransitProfile::default()
                .as_str()
                .to_string()
        });
    let upnp = db
        .get_setting(SETTING_ROUTER_UPNP)
        .map_err(err)?
        .and_then(|v| String::from_utf8(v).ok())
        .map(|v| gipny_libcore::router::Upnp::parse(&v).as_str().to_string())
        .unwrap_or_else(upnp_default);
    Ok(RouterSettingsDto { transit, upnp })
}

/// Stored for the next application start. The process-global router reads
/// these once; switching or reopening a profile does not restart that router.
#[tauri::command]
pub(crate) async fn set_router_settings(
    settings: RouterSettingsDto,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    let db = core_of(&ctx).await?.db().clone();
    // Normalise through the parser so an unknown value cannot be stored.
    let transit = gipny_libcore::router::TransitProfile::parse(&settings.transit);
    db.set_setting(SETTING_ROUTER_TRANSIT, transit.as_str().as_bytes())
        .map_err(err)?;
    let upnp = gipny_libcore::router::Upnp::parse(&settings.upnp);
    db.set_setting(SETTING_ROUTER_UPNP, upnp.as_str().as_bytes())
        .map_err(err)?;
    // Keep the plaintext hint in step, or the next launch prewarms a router
    // with the old knobs and then throws it away once the vault says otherwise.
    if let Some(p) = ctx.profile.lock().await.clone() {
        if let Ok(dir) = profile_dir(&ctx, &p) {
            write_router_hint(
                &dir,
                gipny_libcore::router::RouterSettings { transit, upnp },
            );
        }
    }
    Ok(())
}
