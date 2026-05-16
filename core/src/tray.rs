#[cfg(not(any(target_os = "android", target_os = "ios")))]
const FONT_5X7: &[(char, [u8; 7])] = &[
    ('0', [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110]),
    ('1', [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
    ('2', [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111]),
    ('3', [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110]),
    ('4', [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010]),
    ('5', [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110]),
    ('6', [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110]),
    ('7', [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000]),
    ('8', [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110]),
    ('9', [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100]),
    ('+', [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000]),
];

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn glyph_for(ch: char) -> Option<&'static [u8; 7]> {
    FONT_5X7.iter().find(|(c, _)| *c == ch).map(|(_, g)| g)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn put_pixel(buf: &mut [u8], w: u32, h: u32, x: i32, y: i32, rgba: [u8; 4]) {
    if x < 0 || y < 0 || (x as u32) >= w || (y as u32) >= h { return; }
    let idx = ((y as u32 * w + x as u32) * 4) as usize;
    buf[idx..idx + 4].copy_from_slice(&rgba);
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn draw_filled_circle(buf: &mut [u8], w: u32, h: u32, cx: i32, cy: i32, r: i32, color: [u8; 4]) {
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                put_pixel(buf, w, h, cx + dx, cy + dy, color);
            }
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn draw_text(buf: &mut [u8], w: u32, h: u32, cx: i32, cy: i32, text: &str, scale: i32, color: [u8; 4]) {
    let glyph_w = 5 * scale;
    let glyph_h = 7 * scale;
    let gap = scale;
    let n = text.chars().count() as i32;
    if n == 0 { return; }
    let total_w = n * glyph_w + (n - 1) * gap;
    let mut x0 = cx - total_w / 2;
    let y0 = cy - glyph_h / 2;
    for ch in text.chars() {
        if let Some(glyph) = glyph_for(ch) {
            for row in 0..7i32 {
                for col in 0..5i32 {
                    if glyph[row as usize] & (1 << (4 - col)) != 0 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                put_pixel(buf, w, h, x0 + col * scale + sx, y0 + row * scale + sy, color);
                            }
                        }
                    }
                }
            }
        }
        x0 += glyph_w + gap;
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn render_badge(base: &tauri::image::Image<'_>, count: u32) -> tauri::image::Image<'static> {
    let w = base.width();
    let h = base.height();
    let mut buf = base.rgba().to_vec();
    if count == 0 {
        return tauri::image::Image::new_owned(buf, w, h);
    }
    let label = if count > 99 { "9+".to_string() } else { count.to_string() };
    let badge_diam = ((w.min(h) as i32) * 5 / 8).max(14);
    let r = badge_diam / 2;
    let cx = w as i32 - r - 1;
    let cy = h as i32 - r - 1;
    draw_filled_circle(&mut buf, w, h, cx, cy, r, [220, 40, 40, 255]);
    draw_filled_circle(&mut buf, w, h, cx, cy, r - 1, [255, 60, 60, 255]);
    let scale = ((badge_diam - 4) / 9).max(1);
    draw_text(&mut buf, w, h, cx, cy, &label, scale, [255, 255, 255, 255]);
    tauri::image::Image::new_owned(buf, w, h)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn apply(app: &tauri::AppHandle, count: u32) -> Result<(), String> {
    let tray = app.tray_by_id("main").ok_or_else(|| "no tray".to_string())?;
    let base = app.default_window_icon().cloned().ok_or_else(|| "no base icon".to_string())?;
    let icon = render_badge(&base, count);
    tray.set_icon(Some(icon)).map_err(|e| e.to_string())?;
    let tooltip = if count == 0 { "gipny".to_string() } else { format!("gipny · {} unread", count) };
    tray.set_tooltip(Some(tooltip)).map_err(|e| e.to_string())?;
    let title = if count == 0 { String::new() } else if count > 99 { "9+".to_string() } else { count.to_string() };
    tray.set_title(Some(title)).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn apply(_app: &tauri::AppHandle, _count: u32) -> Result<(), String> { Ok(()) }
