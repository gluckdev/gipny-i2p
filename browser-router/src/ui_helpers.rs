//! Public presentation data only. Private state stays behind BrowserEngine.
use gipny_libcore::crypto::PreKeyBundle;
use wasm_bindgen::prelude::*;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|value| format!("{value:02x}")).collect()
}

#[wasm_bindgen]
impl crate::BrowserEngine {
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

#[wasm_bindgen]
pub fn clean_attachment(name: &str, data: &[u8]) -> Result<String, JsValue> {
    if name.len() > 1024 || data.len() > 1024 * 1024 {
        return Err(JsValue::from_str("browser attachment size limit"));
    }
    let (name, data) = crate::sanitizer::sanitize_attachment_data(name, data)
        .map_err(|error| JsValue::from_str(&error))?;
    serde_json::to_string(&serde_json::json!({ "name": name, "data": data }))
        .map_err(|_| JsValue::from_str("attachment serialization failed"))
}
