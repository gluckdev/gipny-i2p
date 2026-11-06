//! Public presentation data only. Private state stays behind BrowserEngine.
use crate::BrowserEngine;
use bincode::Options;
use gipny_libcore::{crypto::PreKeyBundle, relay::EnvelopeBlob};
use sha2::{Digest, Sha256};
use wasm_bindgen::prelude::*;
use zeroize::Zeroize;

#[wasm_bindgen]
pub struct AttachmentHasher {
    hash: Sha256,
    bytes: u64,
}

#[wasm_bindgen]
impl AttachmentHasher {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            hash: Sha256::new(),
            bytes: 0,
        }
    }
    pub fn update(&mut self, data: &[u8]) -> Result<(), JsValue> {
        if data.len() > 192 * 1024 || self.bytes + data.len() as u64 > 2 * 1024 * 1024 * 1024 {
            return Err(JsValue::from_str("file hashing size limit"));
        }
        self.bytes += data.len() as u64;
        self.hash.update(data);
        Ok(())
    }
    pub fn finish(self) -> Vec<u8> {
        self.hash.finalize().to_vec()
    }
}

#[wasm_bindgen]
pub fn attachment_passes_through(name: &str, head: &[u8]) -> bool {
    name.len() <= 1024 && head.len() <= 1024 && crate::sanitizer::passes_through(name, head)
}

#[wasm_bindgen]
pub fn attachment_passed_name(name: &str) -> String {
    crate::sanitizer::passed_name(name)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|value| format!("{value:02x}")).collect()
}

#[wasm_bindgen]
impl BrowserEngine {
    pub fn bundle_info(&self) -> Result<String, JsValue> {
        let bundle: PreKeyBundle = bincode::deserialize(&self.my_bundle()?)
            .map_err(|_| JsValue::from_str("public bundle serialization"))?;
        Ok(serde_json::json!({
            "sign_pk": hex(&bundle.identity.sign_pk), "dh_pk": hex(&bundle.identity.dh_pk),
            "signed_prekey": hex(&bundle.signed_prekey), "signed_prekey_sig": hex(&bundle.signed_prekey_sig),
            "one_time_prekey": bundle.one_time_prekey.as_ref().map(|key| hex(key)),
            "one_time_id": bundle.one_time_id,
        }).to_string())
    }
}

#[wasm_bindgen]
pub fn contact_qr_svg(text: &str) -> Result<String, JsValue> {
    if text.is_empty() || text.len() > 4096 {
        return Err(JsValue::from_str("QR input size limit"));
    }
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L)
        .map_err(|_| JsValue::from_str("QR input is too large"))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(256, 256)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

/// Whether an envelope opens a new X3DH session. Only such letters need a
/// lifetime replay record: a ratchet letter's key is consumed on first use.
/// Uses the engine's envelope encoding (`EnvelopeBlob`, bincode fixint).
#[wasm_bindgen]
pub fn envelope_is_initial(envelope: &[u8]) -> bool {
    matches!(
        bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .reject_trailing_bytes()
            .with_limit(16 * 1024 * 1024)
            .deserialize::<EnvelopeBlob>(envelope),
        Ok(EnvelopeBlob::X3dhInit(_))
    )
}

/// A cleaned attachment. The bytes cross to JS once as a `Uint8Array`; JSON
/// would turn every byte into a number token (over 1 GB for 32 MiB).
#[wasm_bindgen]
pub struct CleanedAttachment {
    name: String,
    data: Vec<u8>,
}

#[wasm_bindgen]
impl CleanedAttachment {
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> String {
        self.name.clone()
    }

    /// Moves the bytes out and wipes the WASM copy; a second call is empty.
    pub fn take_data(&mut self) -> js_sys::Uint8Array {
        let out = js_sys::Uint8Array::from(self.data.as_slice());
        self.data.zeroize();
        self.data = Vec::new();
        out
    }
}

impl Drop for CleanedAttachment {
    fn drop(&mut self) {
        self.data.zeroize();
    }
}

#[wasm_bindgen]
pub fn clean_attachment(name: &str, data: &[u8]) -> Result<CleanedAttachment, JsValue> {
    if name.len() > 1024 || data.len() > 32 * 1024 * 1024 {
        return Err(JsValue::from_str("browser attachment size limit"));
    }
    let (name, data) = crate::sanitizer::sanitize_attachment_data(name, data)
        .map_err(|error| JsValue::from_str(&error))?;
    Ok(CleanedAttachment { name, data })
}
