#![cfg(target_arch = "wasm32")]
use base64::Engine;
use gipny_browser_router::clean_attachment;
use wasm_bindgen_test::*;
wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn shared_image_cleaner_removes_appended_metadata_before_sending() {
    let mut image = base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jL1kAAAAASUVORK5CYII=")
        .unwrap();
    let original = image.clone();
    image.extend_from_slice(b"GPS and private author metadata");
    let result: serde_json::Value =
        serde_json::from_str(&clean_attachment("IMG_20261101.png", &image).unwrap()).unwrap();
    let data: Vec<u8> = serde_json::from_value(result["data"].clone()).unwrap();
    assert_eq!(data, original);
    assert!(result["name"].as_str().unwrap().starts_with("photo_"));
}

#[wasm_bindgen_test]
fn privacy_mode_rejects_unsupported_or_malformed_formats() {
    assert!(clean_attachment("photo.heic", b"anything").is_err());
    assert!(clean_attachment("notes.pdf", b"%PDF-1.7").is_err());
    let mut invalid = b"\x89PNG\r\n\x1a\n".to_vec();
    invalid.extend_from_slice(&u32::MAX.to_be_bytes());
    invalid.extend_from_slice(b"IHDR");
    assert!(clean_attachment("image.png", &invalid).is_err());
}
