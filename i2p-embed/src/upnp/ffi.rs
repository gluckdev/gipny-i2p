//! C ABI declarations for miniupnpc and the embedded-router hooks.

use std::ffi::{c_char, c_int, c_uint, c_void};

#[repr(C)]
pub(super) struct Device {
    pub(super) next: *mut Device,
    pub(super) url: *mut c_char,
    pub(super) st: *mut c_char,
    pub(super) usn: *mut c_char,
    pub(super) scope: c_uint,
}
#[repr(C)]
pub(super) struct Urls {
    pub(super) control: *mut c_char,
    pub(super) description: *mut c_char,
    pub(super) cif: *mut c_char,
    pub(super) ipv6: *mut c_char,
    pub(super) root: *mut c_char,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Service {
    pub(super) control: [c_char; 128],
    pub(super) event: [c_char; 128],
    pub(super) description: [c_char; 128],
    pub(super) kind: [c_char; 128],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Data {
    pub(super) element: [c_char; 128],
    pub(super) base: [c_char; 128],
    pub(super) presentation: [c_char; 128],
    pub(super) level: c_int,
    pub(super) cif: Service,
    pub(super) first: Service,
    pub(super) second: Service,
    pub(super) ipv6: Service,
    pub(super) temporary: Service,
}

extern "C" {
    pub(super) fn upnpDiscoverDevices(
        kinds: *const *const c_char,
        delay: c_int,
        interface: *const c_char,
        daemon: *const c_char,
        port: c_int,
        ipv6: c_int,
        ttl: u8,
        error: *mut c_int,
        all: c_int,
    ) -> *mut Device;
    pub(super) fn freeUPNPDevlist(list: *mut Device);
    pub(super) fn miniwget_getaddr(
        url: *const c_char,
        size: *mut c_int,
        local: *mut c_char,
        local_len: c_int,
        scope: c_uint,
        status: *mut c_int,
    ) -> *mut c_void;
    pub(super) fn parserootdesc(buffer: *const c_char, size: c_int, data: *mut Data);
    pub(super) fn FreeUPNPUrls(urls: *mut Urls);
    pub(super) fn GetUPNPUrls(
        urls: *mut Urls,
        data: *mut Data,
        description: *const c_char,
        scope: c_uint,
    );
    pub(super) fn UPNP_GetStatusInfo(
        url: *const c_char,
        kind: *const c_char,
        status: *mut c_char,
        uptime: *mut c_uint,
        error: *mut c_char,
    ) -> c_int;
    pub(super) fn UPNP_GetExternalIPAddress(
        url: *const c_char,
        kind: *const c_char,
        address: *mut c_char,
    ) -> c_int;
    pub(super) fn UPNP_GetSpecificPortMappingEntry(
        url: *const c_char,
        kind: *const c_char,
        port: *const c_char,
        proto: *const c_char,
        remote: *const c_char,
        client: *mut c_char,
        internal: *mut c_char,
        description: *mut c_char,
        enabled: *mut c_char,
        lease: *mut c_char,
    ) -> c_int;
    pub(super) fn UPNP_AddPortMapping(
        url: *const c_char,
        kind: *const c_char,
        external: *const c_char,
        internal: *const c_char,
        client: *const c_char,
        description: *const c_char,
        proto: *const c_char,
        remote: *const c_char,
        lease: *const c_char,
    ) -> c_int;
    pub(super) fn UPNP_DeletePortMapping(
        url: *const c_char,
        kind: *const c_char,
        port: *const c_char,
        proto: *const c_char,
        remote: *const c_char,
    ) -> c_int;
    pub(super) fn gipny_router_upnp_ports(tcp: *mut u16, udp: *mut u16) -> c_int;
    pub(super) fn gipny_router_upnp_external(ip: *const c_char);
    pub(super) fn gipny_router_upnp_random(token: *mut u8) -> c_int;
    pub(super) fn gipny_router_upnp_hmac(
        key: *const u8,
        key_len: usize,
        data: *const u8,
        data_len: usize,
        out: *mut u8,
    ) -> c_int;
    pub(super) fn gipny_router_upnp_abort();
    pub(super) fn gipny_router_stop_hook(hook: extern "C" fn());
    pub(super) fn gipny_router_network_hook(hook: extern "C" fn(c_int));
}
