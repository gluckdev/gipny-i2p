use super::*;

extern "C" {
    fn gipny_ssdp_location_matches(
        location: *const c_char,
        size: c_int,
        source: *const c_char,
    ) -> c_int;
}

#[test]
fn ssdp_description_must_belong_to_the_reply_source() {
    for (url, allowed) in [
        ("http://192.168.1.1/root.xml", true),
        ("http://192.168.1.1:5000/root.xml", true),
        ("http://192.168.1.10/root.xml", false),
        ("http://192.168.1.1.evil.test/root.xml", false),
        ("http://192.168.1.1@192.168.1.2/root.xml", false),
        ("http://router.local/root.xml", false),
        ("http://192.168.1.2/root.xml", false),
        ("https://192.168.1.1/root.xml", false),
    ] {
        let result = unsafe {
            gipny_ssdp_location_matches(
                c(url).as_ptr(),
                url.len() as c_int,
                c("192.168.1.1").as_ptr(),
            )
        };
        assert_eq!(result != 0, allowed, "{url}");
    }
}

#[test]
fn only_numeric_lan_description_and_same_device_control_urls_are_allowed() {
    assert_eq!(endpoint("http://192.168.1.1:5000/root.xml"), Some(HOME));
    for url in [
        "http://localhost/x",
        "http://127.0.0.1/x",
        "http://8.8.8.8/x",
        "http://[::1]/x",
        "https://192.168.1.1/x",
        "http://192.168.1.1@8.8.8.8/x",
        "http://192.168.1.1:0/x",
        "http://192.168.1.1/x\r\nHeader: evil",
    ] {
        assert_eq!(endpoint(url), None, "{url}");
    }
    assert_ne!(
        endpoint("http://192.168.1.1/a"),
        endpoint("http://192.168.1.2/b")
    );
    assert!(public(WAN));
    for ip in [
        "10.0.0.1",
        "100.64.0.1",
        "192.0.2.1",
        "127.0.0.1",
        "0.0.0.0",
        "224.0.0.1",
    ] {
        assert!(!public(ip.parse().unwrap()), "{ip}");
    }
}

#[test]
fn root_udn_is_read_from_the_same_bytes_and_bounded() {
    let xml = b"<root><device><UDN> uuid:abc-1 </UDN><deviceList><device><UDN>uuid:inner</UDN></device></deviceList></device></root>";
    assert_eq!(root_udn(xml).as_deref(), Some("uuid:abc-1"));
    assert_eq!(root_udn(b"<root></root>"), None);
    assert_eq!(root_udn(b"<UDN>uuid:x"), None);
    assert_eq!(root_udn(b"<UDN>uuid:\x01x</UDN>"), None);
    assert_eq!(
        root_udn(format!("<UDN>{}</UDN>", "a".repeat(129)).as_bytes()),
        None
    );
}
