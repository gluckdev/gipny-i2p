//! Validated IGD descriptions and the HTTP/SOAP gateway adapter.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::net::Ipv4Addr;
use std::ptr;

use super::ffi::{
    miniwget_getaddr, parserootdesc, Data, FreeUPNPUrls, GetUPNPUrls, UPNP_AddPortMapping,
    UPNP_DeletePortMapping, UPNP_GetExternalIPAddress, UPNP_GetSpecificPortMappingEntry,
    UPNP_GetStatusInfo, Urls,
};
use super::mapping::{Igd, Mapping, Rule};
use super::ownership::{gateway_key, GatewayKey, Secret};

const SERVICES: [&str; 3] = [
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANIPConnection:2",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];

pub(super) fn text(bytes: &[c_char]) -> Option<String> {
    let end = bytes.iter().position(|b| *b == 0)?;
    String::from_utf8(bytes[..end].iter().map(|b| *b as u8).collect()).ok()
}
pub(super) fn c(value: &str) -> CString {
    CString::new(value).expect("validated UPnP string")
}
pub(super) fn lan(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local()
}
/// Only a globally routable address is worth handing to the router.
pub(super) fn public(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b))
        || (a == 198 && (b == 18 || b == 19)))
}
/// The numeric IPv4 host of a plain `http://` URL: no DNS, userinfo, IPv6
/// scope tricks or alternate schemes from an untrusted XML file.
fn host(value: &str) -> Option<Ipv4Addr> {
    if value.len() > 2048
        || value
            .bytes()
            .any(|b| b <= 32 || b >= 127 || b == b'\\' || b == b'#' || b == b'@')
    {
        return None;
    }
    let rest = value.strip_prefix("http://")?;
    let authority = rest.split('/').next()?;
    let (host, port) = authority.split_once(':').unwrap_or((authority, "80"));
    if port.parse::<u16>().ok()? == 0 {
        return None;
    }
    host.parse::<Ipv4Addr>().ok()
}
/// Numeric LAN URLs only: no loopback or off-device SOAP targets.
pub(super) fn endpoint(value: &str) -> Option<Ipv4Addr> {
    host(value).filter(|ip| lan(*ip))
}

struct Description {
    data: Data,
    local: Ipv4Addr,
    udn: String,
}
struct Body(*mut c_void);
impl Drop for Body {
    fn drop(&mut self) {
        unsafe { libc::free(self.0) };
    }
}
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
/// The first `<UDN>` in a root description is the root device's.
pub(super) fn root_udn(xml: &[u8]) -> Option<String> {
    let start = find(xml, b"<UDN>")? + 5;
    let length = find(&xml[start..], b"</UDN>")?;
    let value = std::str::from_utf8(&xml[start..start + length])
        .ok()?
        .trim();
    (!value.is_empty() && value.len() <= 128 && value.bytes().all(|b| b.is_ascii_graphic()))
        .then(|| value.to_owned())
}
/// One HTTP fetch and one parse of a root description. `checked` holds it to
/// numeric LAN URLs; only the loopback CI fixture passes `false`.
fn describe(url: &str, checked: bool) -> Option<Description> {
    if checked && endpoint(url).is_none() {
        return None;
    }
    host(url)?;
    let source = CString::new(url).ok()?;
    let (mut size, mut status) = (0 as c_int, 0 as c_int);
    let mut local = [0 as c_char; 64];
    let body = Body(unsafe {
        miniwget_getaddr(
            source.as_ptr(),
            &mut size,
            local.as_mut_ptr(),
            local.len() as c_int,
            0,
            &mut status,
        )
    });
    if body.0.is_null() || status != 200 || size <= 0 {
        return None;
    }
    let xml = unsafe { std::slice::from_raw_parts(body.0 as *const u8, size as usize) };
    // All fields are char arrays and an integer; zero is upstream's own start.
    let mut data: Data = unsafe { std::mem::zeroed() };
    unsafe { parserootdesc(body.0 as *const c_char, size, &mut data) };
    Some(Description {
        data,
        local: text(&local)?.parse().ok()?,
        udn: root_udn(xml).unwrap_or_default(),
    })
}

pub(super) struct Gateway {
    urls: Urls,
    pub(super) data: Data,
    url: String,
    /// Our address as the gateway sees us, when its description was fetched.
    local: Ipv4Addr,
    /// The internal client mappings point at.
    pub(super) client: Ipv4Addr,
    udn: String,
    pub(super) key: GatewayKey,
    checked: bool,
}
impl Drop for Gateway {
    fn drop(&mut self) {
        unsafe { FreeUPNPUrls(&mut self.urls) };
    }
}
impl Gateway {
    pub(super) fn open(
        url: &str,
        secret: &Secret,
        checked: bool,
        abort: &dyn Fn() -> bool,
    ) -> Option<Self> {
        let description = describe(url, checked)?;
        let device = host(url)?;
        let key = gateway_key(secret, &description.udn, device)?;
        let source = CString::new(url).ok()?;
        // The second WAN service from the same parse, not a second download.
        for alternate in [false, true] {
            if abort() {
                return None;
            }
            let mut data = description.data;
            if alternate {
                std::mem::swap(&mut data.first, &mut data.second);
                if text(&data.first.kind).map_or(true, |kind| kind.is_empty()) {
                    break;
                }
            }
            let mut gateway = Self {
                urls: unsafe { std::mem::zeroed() },
                data,
                url: url.to_owned(),
                local: description.local,
                client: description.local,
                udn: description.udn.clone(),
                key: key.clone(),
                checked,
            };
            unsafe { GetUPNPUrls(&mut gateway.urls, &mut gateway.data, source.as_ptr(), 0) };
            if gateway.usable(device) {
                return Some(gateway);
            }
        }
        None
    }
    fn usable(&self, device: Ipv4Addr) -> bool {
        if (self.checked && !lan(self.local)) || self.urls.control.is_null() {
            return false;
        }
        let Ok(control) = unsafe { CStr::from_ptr(self.urls.control) }.to_str() else {
            return false;
        };
        if host(control) != Some(device) || (self.checked && endpoint(control).is_none()) {
            return false;
        }
        if !text(&self.data.first.kind).is_some_and(|kind| SERVICES.contains(&kind.as_str())) {
            return false;
        }
        let (mut status, mut error, mut uptime) = ([0; 64], [0; 64], 0);
        (unsafe {
            UPNP_GetStatusInfo(
                self.urls.control,
                self.data.first.kind.as_ptr(),
                status.as_mut_ptr(),
                &mut uptime,
                error.as_mut_ptr(),
            )
        }) == 0
            && text(&status).as_deref() == Some("Connected")
    }
}
impl Igd for Gateway {
    fn key(&self) -> &GatewayKey {
        &self.key
    }
    fn client(&self) -> Ipv4Addr {
        self.client
    }
    fn query(&self, rule: &Rule) -> Result<Mapping, i32> {
        let (mut client, mut port, mut description, mut enabled, mut lease) =
            ([0; 16], [0; 6], [0; 80], [0; 4], [0; 16]);
        let result = unsafe {
            UPNP_GetSpecificPortMappingEntry(
                self.urls.control,
                self.data.first.kind.as_ptr(),
                c(&rule.port.to_string()).as_ptr(),
                c(rule.protocol).as_ptr(),
                ptr::null(),
                client.as_mut_ptr(),
                port.as_mut_ptr(),
                description.as_mut_ptr(),
                enabled.as_mut_ptr(),
                lease.as_mut_ptr(),
            )
        };
        if result != 0 {
            return Err(result);
        }
        Ok(Mapping {
            client: text(&client).ok_or(-1)?.parse().map_err(|_| -1)?,
            port: text(&port).ok_or(-1)?.parse().map_err(|_| -1)?,
            description: text(&description).ok_or(-1)?,
            enabled: text(&enabled).as_deref() == Some("1"),
            lease: text(&lease).and_then(|value| value.trim().parse().ok()),
        })
    }
    fn add(&self, rule: &Rule, lease: &str) -> i32 {
        let port = c(&rule.port.to_string());
        unsafe {
            UPNP_AddPortMapping(
                self.urls.control,
                self.data.first.kind.as_ptr(),
                port.as_ptr(),
                port.as_ptr(),
                c(&rule.client.to_string()).as_ptr(),
                c(&self.key.label).as_ptr(),
                c(rule.protocol).as_ptr(),
                ptr::null(),
                c(lease).as_ptr(),
            )
        }
    }
    fn delete(&self, rule: &Rule) -> i32 {
        unsafe {
            UPNP_DeletePortMapping(
                self.urls.control,
                self.data.first.kind.as_ptr(),
                c(&rule.port.to_string()).as_ptr(),
                c(rule.protocol).as_ptr(),
                ptr::null(),
            )
        }
    }
    fn external(&self) -> Option<Ipv4Addr> {
        let mut external = [0; 16];
        if unsafe {
            UPNP_GetExternalIPAddress(
                self.urls.control,
                self.data.first.kind.as_ptr(),
                external.as_mut_ptr(),
            )
        } != 0
        {
            return None;
        }
        text(&external)?.parse().ok()
    }
    fn same_device(&self) -> bool {
        describe(&self.url, self.checked).is_some_and(|now| {
            now.local == self.local
                && now.udn == self.udn
                && now.data.base == self.data.base
                && (now.data.first.control == self.data.first.control
                    || now.data.second.control == self.data.first.control)
        })
    }
}
