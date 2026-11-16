//! The UI reads the same application policy used by our typed client adapter.
//! Exploratory pools keep their upstream policy and are not application hops.
use crate::BrowserRuntime;
use emissary_core::ClientHandle;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn application_tunnel_hops() -> Vec<u8> {
    vec![
        ClientHandle::<BrowserRuntime>::INBOUND_HOPS as u8,
        ClientHandle::<BrowserRuntime>::OUTBOUND_HOPS as u8,
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn application_policy_is_three_hops_in_both_directions() {
        assert_eq!(super::application_tunnel_hops(), [3, 3]);
    }
}
