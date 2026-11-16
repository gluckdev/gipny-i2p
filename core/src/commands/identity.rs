use tauri::State;

use crate::dto::BundleDto;
use crate::{core_of, err, hex, AppCtx};

#[tauri::command]
pub(crate) async fn my_card(ctx: State<'_, AppCtx>) -> Result<serde_json::Value, String> {
    let core = core_of(&ctx).await?;
    let card = core.my_card();
    Ok(serde_json::json!({
        "sign_pk": hex(&card.sign_pk),
        "dh_pk": hex(&card.dh_pk),
    }))
}

#[tauri::command]
pub(crate) async fn my_i2p_address(ctx: State<'_, AppCtx>) -> Result<String, String> {
    Ok(core_of(&ctx).await?.my_i2p_address().to_string())
}

#[tauri::command]
pub(crate) async fn my_b32(ctx: State<'_, AppCtx>) -> Result<String, String> {
    Ok(core_of(&ctx).await?.my_b32())
}

#[tauri::command]
pub(crate) async fn my_fingerprint(ctx: State<'_, AppCtx>) -> Result<String, String> {
    Ok(hex(&core_of(&ctx).await?.my_fingerprint()))
}

#[tauri::command]
pub(crate) async fn my_bundle(ctx: State<'_, AppCtx>) -> Result<BundleDto, String> {
    let b = core_of(&ctx).await?.my_bundle().map_err(err)?;
    Ok(BundleDto {
        sign_pk: hex(&b.identity.sign_pk),
        dh_pk: hex(&b.identity.dh_pk),
        signed_prekey: hex(&b.signed_prekey),
        signed_prekey_sig: hex(&b.signed_prekey_sig),
        one_time_prekey: b.one_time_prekey.as_ref().map(|k| hex(&k[..])),
        one_time_id: b.one_time_id,
    })
}

#[tauri::command]
pub(crate) async fn get_display_name(ctx: State<'_, AppCtx>) -> Result<String, String> {
    core_of(&ctx).await?.display_name().map_err(err)
}

#[tauri::command]
pub(crate) async fn set_display_name(name: String, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .db()
        .set_setting("display_name", name.as_bytes())
        .map_err(err)
}

/// A contact card as a QR picture, so two people can exchange cards by
/// pointing one phone at another instead of copying a card. Rendered
/// here (pure Rust) rather than in the interface: one small dependency instead
/// of a JavaScript one. Returns an SVG that scales to whatever box it is put in.
#[tauri::command]
pub(crate) fn qr_svg(text: String) -> Result<String, String> {
    // v3 is 187 ASCII characters without a name (at most316 with one).
    // Keep the existing larger bound for legacy v1/v2 cards.
    if text.is_empty() || text.len() > 2000 {
        return Err("nothing to encode".into());
    }
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L)
        .map_err(|e| format!("qr: {e}"))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .quiet_zone(true)
        .min_dimensions(240, 240)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}
