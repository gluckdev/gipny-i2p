//! Bounded IPv4 IGD port mapping for the embedded router. No daemon services,
//! proxy listeners or application ports are exposed by this module.
//!
//! What the LAN learns from it is kept to what a mapping needs. The HTTP
//! User-Agent is miniupnpc's own with a bare OS name. A mapping is described
//! as a generic word plus a tag derived per gateway from a local secret, so
//! two networks cannot match their tables against each other. The external
//! address goes to the router only once a mapping on that gateway is confirmed.
//! Mappings survive window focus and network notifications: the gateway is
//! re-verified, not torn down, and only rules that are provably ours on that
//! same gateway are ever deleted.
mod discovery;
mod ffi;
mod mapping;
mod ownership;
mod protocol;
mod session;

use std::ffi::c_int;
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use discovery::discover;
use ffi::{
    gipny_router_network_hook, gipny_router_stop_hook, gipny_router_upnp_abort,
    gipny_router_upnp_external, gipny_router_upnp_ports,
};
use ownership::{Registry, Secret};
use protocol::{c, Gateway};
use session::{Env, Ports, Session};

/// Network notifications come in bursts (focus is offline+online; Android
/// repeats itself): act once the host has been quiet this long.
const SETTLE: Duration = Duration::from_secs(2);
/// Exit waits at most about this long for the worker (plus one in-flight read).
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(3);
/// No new exchange is started this close to the end of the budget.
const SHUTDOWN_MARGIN: Duration = Duration::from_millis(500);
struct Native;
impl Env for Native {
    type Gateway = Gateway;
    fn ports(&mut self) -> Ports {
        let (mut tcp, mut udp) = (0, 0);
        match unsafe { gipny_router_upnp_ports(&mut tcp, &mut udp) } {
            1 => Ports::Active { tcp, udp },
            0 => Ports::Disabled,
            _ => Ports::Stopped,
        }
    }
    fn discover(&mut self, secret: &Secret, abort: &dyn Fn() -> bool) -> Option<Gateway> {
        discover(secret, abort)
    }
    fn publish(&mut self, external: Ipv4Addr) {
        unsafe { gipny_router_upnp_external(c(&external.to_string()).as_ptr()) };
    }
}

#[derive(Default)]
struct Flags {
    /// When exit asked the worker to stop.
    stop: Option<Instant>,
    offline: bool,
    revision: u64,
}
#[derive(Debug, PartialEq, Eq)]
enum Woke {
    Stop,
    Timeout,
    Network,
}
#[derive(Default)]
struct Control {
    flags: Mutex<Flags>,
    changed: Condvar,
}
impl Control {
    fn stopping(&self) -> bool {
        self.flags.lock().map_or(true, |f| f.stop.is_some())
    }
    fn stopped_at(&self) -> Option<Instant> {
        self.flags.lock().ok().and_then(|f| f.stop)
    }
    fn request_stop(&self) {
        if let Ok(mut flags) = self.flags.lock() {
            flags.stop.get_or_insert_with(Instant::now);
        }
        self.changed.notify_all();
    }
    /// What the host says about the network. Online while already online is
    /// no news for the gateway and does not wake the worker.
    fn report(&self, online: bool) {
        if let Ok(mut flags) = self.flags.lock() {
            if online && !flags.offline {
                return;
            }
            flags.offline = !online;
            flags.revision = flags.revision.wrapping_add(1);
        }
        self.changed.notify_all();
    }
    fn wait(&self, seen: u64, timeout: Duration) -> Woke {
        let Ok(flags) = self.flags.lock() else {
            return Woke::Stop;
        };
        let Ok((flags, _)) = self
            .changed
            .wait_timeout_while(flags, timeout, |f| f.stop.is_none() && f.revision == seen)
        else {
            return Woke::Stop;
        };
        if flags.stop.is_some() {
            Woke::Stop
        } else if flags.revision != seen {
            Woke::Network
        } else {
            Woke::Timeout
        }
    }
    /// Wait until the host has been quiet for `quiet` (or has kept talking for
    /// a while), then report where it ended up. `None`: stop instead.
    fn settle(&self, quiet: Duration) -> Option<(bool, u64)> {
        let started = Instant::now();
        let mut flags = self.flags.lock().ok()?;
        loop {
            if flags.stop.is_some() {
                return None;
            }
            let revision = flags.revision;
            let (next, timeout) = self
                .changed
                .wait_timeout_while(flags, quiet, |f| f.stop.is_none() && f.revision == revision)
                .ok()?;
            flags = next;
            if flags.stop.is_some() {
                return None;
            }
            if timeout.timed_out() || started.elapsed() >= quiet * 5 {
                return Some((flags.offline, flags.revision));
            }
        }
    }
}
#[derive(Default)]
struct Manager {
    control: Arc<Control>,
    thread: Mutex<Option<JoinHandle<()>>>,
}
fn manager() -> &'static Manager {
    static INSTANCE: OnceLock<Manager> = OnceLock::new();
    INSTANCE.get_or_init(Manager::default)
}

extern "C" fn network_hook(online: c_int) {
    // JNI invokes the C shim directly; route that signal to the same manager.
    let _ = std::panic::catch_unwind(|| manager().control.report(online != 0));
}
extern "C" fn stop_hook() {
    // Never let a poisoned Rust lock/unwind cross the C exit callback.
    let _ = std::panic::catch_unwind(|| {
        let m = manager();
        m.control.request_stop();
        // An exchange already in flight gives up at its next received chunk.
        unsafe { gipny_router_upnp_abort() };
        if let Ok(mut thread) = m.thread.lock() {
            if let Some(thread) = thread.take() {
                // Joined before StopI2P either way: the worker reads transports.
                let started = Instant::now();
                while !thread.is_finished() && started.elapsed() < SHUTDOWN_BUDGET {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if !thread.is_finished() {
                    unsafe { gipny_router_upnp_abort() };
                }
                let _ = thread.join();
            }
        }
    });
}
pub(crate) fn start(directory: Option<&Path>) {
    let (mut tcp, mut udp) = (0, 0);
    // Off in settings (upnp.enabled=false) or hidden: no worker, no files and
    // not a single packet to the LAN.
    if unsafe { gipny_router_upnp_ports(&mut tcp, &mut udp) } != 1 {
        return;
    }
    let Some(directory) = directory else {
        return;
    };
    let registry = match Registry::open(directory) {
        Ok(value) => value,
        Err(error) => {
            crate::debug_error!("[upnp] ownership state unavailable: {error}");
            return;
        }
    };
    let m = manager();
    let Ok(mut handle) = m.thread.lock() else {
        return;
    };
    if handle.is_some() {
        return;
    }
    if let Ok(mut flags) = m.control.flags.lock() {
        flags.stop = None;
    }
    unsafe { gipny_router_network_hook(network_hook) };
    let control = m.control.clone();
    match std::thread::Builder::new()
        .name("gipny-upnp".into())
        .spawn(move || run(control, registry))
    {
        Ok(thread) => {
            *handle = Some(thread);
            unsafe { gipny_router_stop_hook(stop_hook) };
        }
        Err(error) => crate::debug_error!("[upnp] worker start failed: {error}"),
    }
}

fn run(control: Arc<Control>, registry: Registry) {
    let mut session: Session<Gateway> = Session::new(registry, Instant::now());
    let mut seen = match control.flags.lock() {
        Ok(flags) => {
            session.offline = flags.offline;
            flags.revision
        }
        Err(_) => return,
    };
    let mut env = Native;
    let mut wait = Duration::ZERO;
    loop {
        match control.wait(seen, wait) {
            Woke::Stop => break,
            Woke::Timeout => {}
            Woke::Network => match control.settle(SETTLE) {
                None => break,
                Some((offline, revision)) => {
                    seen = revision;
                    session.network(offline, Instant::now());
                }
            },
        }
        let abort = || control.stopping();
        wait = session.cycle(&mut env, Instant::now(), &abort);
    }
    let deadline = control.stopped_at().unwrap_or_else(Instant::now) + SHUTDOWN_BUDGET;
    session.shutdown(&|| Instant::now() + SHUTDOWN_MARGIN >= deadline);
}

#[cfg(test)]
mod tests;

// The existing real-HTTP fixture imports its test surface from this module.
#[cfg(test)]
use mapping::{cleanup_except, maintain, Igd, Outcome, Rule};
#[cfg(test)]
use ownership::{gateway_key, read_record};
#[cfg(test)]
use protocol::text;
#[cfg(test)]
use session::{RECHECK, RENEW};
#[cfg(test)]
use std::io::{Read, Write};
#[cfg(test)]
use tests::Scratch;

#[cfg(test)]
#[path = "upnp_fixture.rs"]
mod fixture;
