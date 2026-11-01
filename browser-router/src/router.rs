//! Execute the actual emissary Router with a client-only browser runtime.
use crate::{
    configure_forwarder_authenticated, metrics_snapshot, ntcp2::verified_peer, tasks::TaskScope,
    BrowserRuntime,
};
use emissary_core::{
    events::Event,
    primitives::{RouterAddress, RouterInfo},
    router::RouterBuilder,
    runtime::Runtime,
    Config, MetricsConfig, Ntcp2Config,
};
use futures::{
    future::{select, Either},
    FutureExt, StreamExt,
};
use rand::RngExt;
use std::{num::NonZeroUsize, time::Duration};
use wasm_bindgen::{prelude::*, JsCast};

fn client_config(routers: Vec<Vec<u8>>) -> Config {
    Config {
        ntcp2: Some(Ntcp2Config {
            disable_pq: true,
            ipv4: true,
            ipv4_host: None,
            ipv6: false,
            ipv6_host: None,
            iv: BrowserRuntime::rng().random(),
            key: BrowserRuntime::rng().random(),
            ml_kem: None,
            port: 0,
            publish_ipv4: false,
            publish_ipv6: false,
            max_connections: NonZeroUsize::new(24),
        }),
        routers,
        net_id: Some(2),
        refresh_interval: Some(1),
        metrics: Some(MetricsConfig { port: 0 }),
        // Default exploratory pool: 3 inbound + 3 outbound, 2 hops each.
        // No transit, SAM/I2CP, UDP/SSU2, local IPs or insecure hop selection.
        ..Config::default()
    }
}

#[derive(Debug)]
pub struct ExploratoryReceipt {
    pub router_id: String,
    pub seed_routers: usize,
    pub connections: usize,
    pub inbound: u64,
    pub outbound: u64,
    pub roundtrips: u64,
    pub ready_after_ms: u128,
    pub elapsed_ms: u128,
    pub held_seconds: u64,
    pub wasm_memory_bytes: u64,
}

fn verify_no_listener(
    info: &emissary_core::router::ProtocolAddressInfo,
    bytes: &[u8],
) -> Result<(), String> {
    if info.ntcp2_port.is_some()
        || info.ssu2_port.is_some()
        || info.i2cp.is_some()
        || info.sam_tcp.is_some()
    {
        return Err("client router unexpectedly exposed a listening protocol".into());
    }
    let router =
        RouterInfo::parse::<BrowserRuntime>(bytes).map_err(|e| format!("local RI: {e:?}"))?;
    if !router.addresses.iter().all(|address| {
        matches!(
            address,
            RouterAddress::Ntcp2 {
                socket_address: None,
                iv: None,
                ..
            }
        )
    }) {
        return Err("client RouterInfo contains a listening address".into());
    }
    Ok(())
}

pub async fn run_exploratory(
    routers: Vec<Vec<u8>>,
    timeout_seconds: u64,
    hold_seconds: u64,
) -> Result<ExploratoryReceipt, String> {
    if routers.len() < 16
        || routers.len() > 2048
        || !(30..=600).contains(&timeout_seconds)
        || !(15..=120).contains(&hold_seconds)
        || hold_seconds >= timeout_seconds
    {
        return Err("invalid router seed count or probe duration".into());
    }
    // Every bootstrap record is verified again inside WASM before starting core.
    for bytes in &routers {
        verified_peer(bytes).map_err(|e| format!("bootstrap RI: {e:?}"))?;
    }
    let seed_routers = routers.len();
    let _tasks = TaskScope::enter().map_err(str::to_owned)?;
    let started = web_time::Instant::now();
    let (mut router, mut events, local_info) =
        RouterBuilder::<BrowserRuntime>::new(client_config(routers))
            .build()
            .await
            .map_err(|e| format!("RouterBuilder: {e:?}"))?;
    verify_no_listener(router.protocol_address_info(), &local_info)?;
    let router_id = router.router_id().to_string();
    let mut stable = None;
    let mut first_ready = None;
    let mut connections = 0;
    loop {
        match select(&mut router, BrowserRuntime::timer(Duration::from_secs(1))).await {
            Either::Left(_) => {
                return Err(format!(
                    "router stopped unexpectedly: {:?}",
                    metrics_snapshot()
                ))
            }
            Either::Right(_) => {}
        }
        while let Some(Some(event)) = events.next().now_or_never() {
            if let Event::RouterStatus { transport, .. } = event {
                connections = transport.num_connected_routers;
            }
        }
        let snapshot = metrics_snapshot();
        let inbound = snapshot.gauge("tunnel_inbound_count");
        let outbound = snapshot.gauge("tunnel_outbound_count");
        let roundtrips = snapshot.counter("tunnel_test_success_count");
        if inbound > 0 && outbound > 0 && roundtrips > 0 {
            let (since, initial_roundtrips) =
                *stable.get_or_insert_with(|| (web_time::Instant::now(), roundtrips));
            first_ready.get_or_insert(started.elapsed().as_millis());
            if since.elapsed() >= Duration::from_secs(hold_seconds)
                && roundtrips > initial_roundtrips
            {
                // Stop the root future, then TaskScope cancels all its children.
                for _ in 0..3 {
                    router.shutdown();
                }
                return Ok(ExploratoryReceipt {
                    router_id,
                    seed_routers,
                    connections,
                    inbound,
                    outbound,
                    roundtrips,
                    ready_after_ms: first_ready.unwrap(),
                    elapsed_ms: started.elapsed().as_millis(),
                    held_seconds: hold_seconds,
                    wasm_memory_bytes: wasm_memory_bytes(),
                });
            }
        } else {
            stable = None;
        }
        if started.elapsed() >= Duration::from_secs(timeout_seconds) {
            return Err(format!(
                "exploratory deadline: connections={connections} metrics={snapshot:?}"
            ));
        }
        #[cfg(target_arch = "wasm32")]
        if started.elapsed().as_secs() % 10 == 0 {
            web_sys::console::log_1(
                &format!(
                    "ROUTER_PROGRESS elapsed_ms={} connections={connections} metrics={snapshot:?}",
                    started.elapsed().as_millis()
                )
                .into(),
            );
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn wasm_memory_bytes() -> u64 {
    let memory: js_sys::WebAssembly::Memory = wasm_bindgen::memory().unchecked_into();
    js_sys::Reflect::get(&memory.buffer(), &JsValue::from_str("byteLength"))
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0) as u64
}
#[cfg(not(target_arch = "wasm32"))]
fn wasm_memory_bytes() -> u64 {
    0
}

/// Run the full browser router until an inbound/outbound exploratory path has
/// passed real tunnel roundtrips and remained available for thirty seconds.
#[wasm_bindgen]
pub async fn exploratory_probe(
    router_infos: js_sys::Array,
    endpoint: String,
    capability: String,
) -> Result<String, JsValue> {
    if router_infos.length() > 2048 {
        return Err(JsValue::from_str("too many bootstrap records"));
    }
    let mut routers = Vec::new();
    for value in router_infos.iter() {
        let bytes = value
            .dyn_into::<js_sys::Uint8Array>()
            .map_err(|_| JsValue::from_str("bootstrap records must be Uint8Array"))?;
        if bytes.length() > 16384 {
            return Err(JsValue::from_str("oversize bootstrap record"));
        }
        routers.push(bytes.to_vec());
    }
    configure_forwarder_authenticated(&endpoint, &capability).map_err(JsValue::from_str)?;
    let receipt = run_exploratory(routers, 300, 30)
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    Ok(format!("{receipt:?}"))
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;
    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    async fn client_router_starts_without_a_listener_or_published_address() {
        let _tasks = TaskScope::enter().unwrap();
        let (router, _, bytes) = RouterBuilder::<BrowserRuntime>::new(client_config(Vec::new()))
            .build()
            .await
            .unwrap();
        verify_no_listener(router.protocol_address_info(), &bytes).unwrap();
    }

    #[wasm_bindgen_test]
    async fn browser_runtime_rejects_inbound_publication() {
        let mut config = client_config(Vec::new());
        config.ntcp2.as_mut().unwrap().publish_ipv4 = true;
        assert!(RouterBuilder::<BrowserRuntime>::new(config)
            .build()
            .await
            .is_err());
    }
}
