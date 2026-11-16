use emissary_core::{
    crypto::{sha256::Sha256, SigningPrivateKey, StaticPrivateKey},
    primitives::{Date, RouterAddress, RouterInfo},
    runtime::Runtime,
    Config,
};
use gipny_browser_router::BrowserRuntime;
use rand::RngExt;
use std::io::Write;

pub fn digest(bytes: &[u8]) -> String {
    Sha256::new()
        .update(bytes)
        .finalize_new()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(bytes).unwrap();
    gzip.finish().unwrap()
}

pub fn archive(records: &[Vec<u8>]) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for (index, bytes) in records.iter().enumerate() {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(
            &mut header,
            format!("rX/routerInfo-{index}.dat"),
            bytes.as_slice(),
        )
        .unwrap();
    }
    gzip(&tar.into_inner().unwrap())
}

pub fn signed_info(ip: &str, network: u8) -> Vec<u8> {
    signed_info_at(ip, network, 0)
}

pub fn signed_info_at(ip: &str, network: u8, age_ms: u64) -> Vec<u8> {
    let key = BrowserRuntime::rng().random();
    let secret = StaticPrivateKey::from_bytes(key);
    let signing = SigningPrivateKey::random(BrowserRuntime::rng());
    let address = RouterAddress::new_published_ntcp2(
        key,
        [0; 16],
        None,
        true,
        ip.parse().unwrap(),
        format!("{ip}:12345").parse().unwrap(),
    );
    let mut router = RouterInfo::new::<BrowserRuntime>(
        &Config {
            net_id: Some(network),
            ..Config::default()
        },
        Some(address),
        None,
        None,
        None,
        &secret,
        &signing,
        false,
    );
    router.published = Date::new((*router.published.date()).saturating_sub(age_ms));
    router.serialize(&signing)
}
