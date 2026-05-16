use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use image::{ImageFormat, Rgba, RgbaImage};

fn main() {
    generate_icons();
    generate_sounds_table();
    ensure_ui_built();
    tauri_build::build();
}

fn generate_sounds_table() {
    let dir = Path::new("../ui/public/sounds");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let out_path = Path::new(&out_dir).join("sounds.rs");
    println!("cargo:rerun-if-changed=../ui/public/sounds");

    let mut entries: Vec<(String, std::path::PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("wav") { continue; }
            let stem = match p.file_stem().and_then(|s| s.to_str()) { Some(s) => s, None => continue };
            if stem.is_empty() || stem.len() > 32 { continue; }
            if !stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') { continue; }
            println!("cargo:rerun-if-changed=../ui/public/sounds/{}.wav", stem);
            entries.push((stem.to_string(), p.canonicalize().unwrap_or(p)));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut src = String::from("pub const EMBEDDED_SOUNDS: &[(&str, &[u8])] = &[\n");
    for (name, path) in &entries {
        let abs = path.display().to_string().replace('\\', "/");
        src.push_str(&format!("    (\"{}\", include_bytes!(\"{}\")),\n", name, abs));
    }
    src.push_str("];\n");
    std::fs::write(&out_path, src).expect("write sounds.rs");
}

fn generate_icons() {
    let dir = Path::new("icons");
    if dir.join("icon.ico").exists() { return; }
    std::fs::create_dir_all(dir).expect("mkdir icons");
    let base = draw(512);
    resize_and_save(&base, 32, dir.join("32x32.png"));
    resize_and_save(&base, 128, dir.join("128x128.png"));
    resize_and_save(&base, 256, dir.join("icon.png"));
    let ico = nearest_resize(&base, 256);
    ico.save_with_format(dir.join("icon.ico"), ImageFormat::Ico).expect("ico");
}

fn resize_and_save(src: &RgbaImage, size: u32, path: std::path::PathBuf) {
    let out = nearest_resize(src, size);
    out.save_with_format(path, ImageFormat::Png).expect("png");
}

fn nearest_resize(src: &RgbaImage, size: u32) -> RgbaImage {
    let mut out = RgbaImage::new(size, size);
    let sw = src.width() as f32;
    let sh = src.height() as f32;
    for y in 0..size {
        for x in 0..size {
            let sx = ((x as f32 + 0.5) / size as f32 * sw) as u32;
            let sy = ((y as f32 + 0.5) / size as f32 * sh) as u32;
            out.put_pixel(x, y, *src.get_pixel(sx.min(src.width() - 1), sy.min(src.height() - 1)));
        }
    }
    out
}

fn draw(size: u32) -> RgbaImage {
    let mut img = RgbaImage::new(size, size);
    let bg = Rgba([8, 14, 8, 255]);
    let border = Rgba([45, 110, 45, 255]);
    let phosphor = Rgba([92, 255, 92, 255]);
    let amber = Rgba([255, 176, 0, 255]);

    for p in img.pixels_mut() { *p = bg; }

    let corner = size / 10;
    let thick = (size / 48).max(2);
    for y in 0..size {
        for x in 0..size {
            let in_corner = (x < corner && y < corner && (corner - x) + (corner - y) > corner)
                || (x >= size - corner && y < corner && (x + 1 - (size - corner)) + (corner - y) > corner)
                || (x < corner && y >= size - corner && (corner - x) + (y + 1 - (size - corner)) > corner)
                || (x >= size - corner && y >= size - corner && (x + 1 - (size - corner)) + (y + 1 - (size - corner)) > corner);
            if in_corner { img.put_pixel(x, y, Rgba([0, 0, 0, 0])); continue; }
            let on_border = x < thick || y < thick || x >= size - thick || y >= size - thick;
            if on_border { img.put_pixel(x, y, border); }
        }
    }

    let gt: [u8; 7] = [0b10000, 0b01000, 0b00100, 0b00010, 0b00100, 0b01000, 0b10000];
    let cell = size / 14;
    let gh = cell * 7;
    let gw = cell * 5;
    let y0 = (size - gh) / 2;
    let gap = cell;
    let total_w = gw + gap + cell * 4;
    let x0 = (size - total_w) / 2;
    for (dy, row) in gt.iter().enumerate() {
        for dx in 0..5u32 {
            if (row >> (4 - dx)) & 1 == 1 {
                fill_block(&mut img, x0 + dx * cell, y0 + (dy as u32) * cell, cell, phosphor);
            }
        }
    }
    let ux = x0 + gw + gap;
    let uy = y0 + gh - cell;
    for dx in 0..4u32 {
        fill_block(&mut img, ux + dx * cell, uy, cell, amber);
    }
    img
}

fn fill_block(img: &mut RgbaImage, x: u32, y: u32, size: u32, c: Rgba<u8>) {
    for dy in 0..size {
        for dx in 0..size {
            let px = x + dx;
            let py = y + dy;
            if px < img.width() && py < img.height() {
                img.put_pixel(px, py, c);
            }
        }
    }
}

fn ensure_ui_built() {
    let ui = Path::new("../ui");
    if !ui.exists() {
        println!("cargo:warning=ui/ not found; skipping frontend build");
        return;
    }
    if !ui.join("node_modules").exists() {
        println!("cargo:warning=installing ui dependencies...");
        let status = Command::new(npm())
            .arg("install")
            .current_dir(ui)
            .status()
            .expect("run npm install");
        if !status.success() { panic!("npm install failed"); }
    }
    let dist = ui.join("dist").join("index.html");
    let rebuild = !dist.exists() || stale(&dist, ui);
    if rebuild {
        println!("cargo:warning=building ui...");
        let status = Command::new(npm())
            .args(["run", "build"])
            .current_dir(ui)
            .status()
            .expect("run npm build");
        if !status.success() { panic!("npm run build failed"); }
    }
    println!("cargo:rerun-if-changed=../ui/src");
    println!("cargo:rerun-if-changed=../ui/index.html");
    println!("cargo:rerun-if-changed=../ui/package.json");
}

fn stale(dist: &Path, ui: &Path) -> bool {
    let Ok(dm) = std::fs::metadata(dist).and_then(|m| m.modified()) else { return true; };
    newer(&ui.join("src"), dm) || newer(&ui.join("index.html"), dm) || newer(&ui.join("package.json"), dm)
}

fn newer(p: &Path, t: SystemTime) -> bool {
    if p.is_file() {
        std::fs::metadata(p).and_then(|m| m.modified()).map(|m| m > t).unwrap_or(false)
    } else if p.is_dir() {
        std::fs::read_dir(p).map(|es| es.filter_map(|e| e.ok()).any(|e| newer(&e.path(), t))).unwrap_or(false)
    } else { false }
}

fn npm() -> &'static str { if cfg!(windows) { "npm.cmd" } else { "npm" } }