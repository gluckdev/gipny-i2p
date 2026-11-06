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
    let mut result = clean_attachment("IMG_20261101.png", &image).unwrap();
    assert!(result.name().starts_with("photo_"));
    assert_eq!(result.take_data().to_vec(), original);
    assert!(
        result.take_data().to_vec().is_empty(),
        "bytes cross to JS once"
    );
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
