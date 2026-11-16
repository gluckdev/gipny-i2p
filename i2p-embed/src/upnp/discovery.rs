//! Bounded SSDP search for numeric LAN gateways.

use std::collections::HashSet;
use std::ffi::{c_int, CStr};
use std::ptr;

use super::ffi::{freeUPNPDevlist, upnpDiscoverDevices, Device};
use super::ownership::Secret;
use super::protocol::{c, endpoint, Gateway};

const DISCOVERY_DELAY_MS: c_int = 1500;
const CANDIDATES: usize = 8;
const SEARCH: [&str; 4] = [
    "urn:schemas-upnp-org:device:InternetGatewayDevice:1",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
    "upnp:rootdevice",
];
struct Devices(*mut Device);
impl Drop for Devices {
    fn drop(&mut self) {
        unsafe { freeUPNPDevlist(self.0) };
    }
}
pub(super) fn discover(secret: &Secret, abort: &dyn Fn() -> bool) -> Option<Gateway> {
    // Empty daemon path selects bounded local SSDP, not an optional minissdpd
    // service that could omit the packet origin or block without our deadline.
    let daemon = c("");
    let mut candidates = HashSet::new();
    // One search type per call, so a shutdown waits out at most one window.
    for kind in SEARCH {
        if abort() || candidates.len() >= CANDIDATES {
            break;
        }
        let kind = c(kind);
        let kinds = [kind.as_ptr(), ptr::null()];
        let mut error = 0;
        let devices = Devices(unsafe {
            upnpDiscoverDevices(
                kinds.as_ptr(),
                DISCOVERY_DELAY_MS,
                ptr::null(),
                daemon.as_ptr(),
                0,
                0,
                2,
                &mut error,
                0,
            )
        });
        let mut current = devices.0;
        for _ in 0..64 {
            if current.is_null() || abort() {
                break;
            }
            let item = unsafe { &*current };
            if !item.url.is_null() && candidates.len() < CANDIDATES {
                if let Ok(url) = unsafe { CStr::from_ptr(item.url) }.to_str() {
                    if endpoint(url).is_some() && candidates.insert(url.to_string()) {
                        if let Some(gateway) = Gateway::open(url, secret, true, abort) {
                            return Some(gateway);
                        }
                    }
                }
            }
            current = item.next;
        }
    }
    None
}
