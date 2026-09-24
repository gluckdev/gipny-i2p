//! Attachment privacy: take out of a file what it says about its author and
//! device before it is sent.
//!
//! What is cleaned, and how:
//!
//! * **JPEG** — every APPn segment goes except the three a decoder needs to
//!   draw the picture right (JFIF, the ICC profile, Adobe's colour-transform
//!   marker), and so do comments. That removes EXIF, XMP, IPTC, the thumbnails
//!   (EXIF's and JFXX's — a thumbnail survives a crop), MPF sub-images and
//!   C2PA manifests. EXIF's Orientation is read first and written back as a
//!   minimal EXIF block of its own, because without it every phone photo taken
//!   upright arrives lying on its side. Anything after the end-of-image marker
//!   is cut: "motion photos" keep a video there. Pixels are not re-encoded.
//! * **PNG** — critical chunks and the ancillary chunks that affect rendering
//!   stay; text chunks, EXIF, the timestamp and unknown ancillary chunks go,
//!   and so does anything after IEND.
//! * **WebP** — the EXIF and XMP chunks go and the VP8X header stops claiming
//!   they are there; the RIFF size is recomputed.
//! * **PDF** — parsed (object streams included) and rewritten without the
//!   Info dictionary, without any /Metadata stream wherever it hangs, without
//!   /PieceInfo and the file /ID. Rewriting also drops superseded revisions.
//!
//! What cannot be cleaned is not sent: HEIC/AVIF, TIFF and camera raw, and
//! video carry the same GPS and device data, and a switch that says "metadata
//! is removed" must not quietly let them through. The same goes for a file
//! that claims one of the formats above and cannot be parsed as it.
//!
//! File names: only names that look machine-made (`IMG_20260916_165301.jpg`,
//! `Screenshot …`, a pasted `…-clipboard.png`) are replaced — they carry the
//! time of capture. A name a person chose is theirs to send.
//!
//! This runs on the chat send paths only. Files uploaded through the agent
//! console bypass it: a script has to arrive byte for byte under its own name.

use sha2::{Digest, Sha256};

const HINT: &str = "выключите «очищать метаданные вложений» в настройках или отправьте другой файл";

/// Cleans `data` according to what it is, and returns the name to send it
/// under. `Err` carries a message for the user; the file must then not be sent.
pub fn sanitize_attachment_data(filename: &str, data: &[u8]) -> Result<(String, Vec<u8>), String> {
    let ext = extension(filename);
    let (clean, canonical_ext, noun) = match sniff(data) {
        Some(Format::Jpeg) => (clean_jpeg(data).map_err(|e| malformed("JPEG", &e))?, "jpg", "photo"),
        Some(Format::Png) => (clean_png(data).map_err(|e| malformed("PNG", &e))?, "png", "photo"),
        Some(Format::Webp) => (clean_webp(data).map_err(|e| malformed("WebP", &e))?, "webp", "photo"),
        Some(Format::Pdf) => (clean_pdf(data).map_err(|e| malformed("PDF", &e))?, "pdf", "document"),
        Some(Format::Unsupported(what)) => return Err(unsupported(what)),
        None if CLEANABLE_EXT.contains(&ext.as_str()) => {
            return Err(malformed(&ext.to_uppercase(), "содержимое не соответствует расширению"));
        }
        None if UNSUPPORTED_EXT.contains(&ext.as_str()) => return Err(unsupported(&ext.to_uppercase())),
        None => return Ok((strip_paste_prefix(filename).to_string(), data.to_vec())),
    };
    Ok((neutral_name(filename, &clean, canonical_ext, noun), clean))
}

/// Whether [`sanitize_attachment_data`] would pass this file through
/// unchanged (only its name tidied), judged from its first bytes and name:
/// then a large one can be sent from disk without reading it into memory.
pub fn passes_through(filename: &str, head: &[u8]) -> bool {
    let ext = extension(filename);
    sniff(head).is_none() && !CLEANABLE_EXT.contains(&ext.as_str()) && !UNSUPPORTED_EXT.contains(&ext.as_str())
}

/// The name a file passed through keeps.
pub fn passed_name(filename: &str) -> String {
    strip_paste_prefix(filename).to_string()
}

fn unsupported(what: &str) -> String {
    format!("приватность вложений: из формата {what} метаданные удалить нельзя — конвертируйте в JPEG или PNG, либо {HINT}")
}

fn malformed(format: &str, detail: &str) -> String {
    format!("приватность вложений: не удалось разобрать {format} ({detail}) — {HINT}")
}

// ----- what is it ------------------------------------------------------------

enum Format {
    Jpeg,
    Png,
    Webp,
    Pdf,
    Unsupported(&'static str),
}

const CLEANABLE_EXT: &[&str] = &["jpg", "jpeg", "jpe", "jfif", "png", "webp", "pdf"];
const UNSUPPORTED_EXT: &[&str] = &[
    "heic", "heif", "avif", "tif", "tiff", "dng", "cr2", "cr3", "nef", "arw", "raf", "orf", "rw2",
    "mp4", "m4v", "mov", "3gp", "mkv", "webm", "avi",
];
const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// By content, not by name: a HEIC renamed to `.jpg` is still a HEIC.
fn sniff(d: &[u8]) -> Option<Format> {
    if d.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Format::Jpeg);
    }
    if d.starts_with(&PNG_SIG) {
        return Some(Format::Png);
    }
    if d.len() >= 12 && &d[..4] == b"RIFF" {
        return match &d[8..12] {
            b"WEBP" => Some(Format::Webp),
            b"AVI " => Some(Format::Unsupported("AVI")),
            _ => None,
        };
    }
    if d.len() >= 12 && &d[4..8] == b"ftyp" {
        return Some(Format::Unsupported("HEIC/AVIF/MP4/MOV"));
    }
    if d.starts_with(b"II*\0") || d.starts_with(b"MM\0*") {
        return Some(Format::Unsupported("TIFF/RAW"));
    }
    if d.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(Format::Unsupported("MKV/WebM"));
    }
    // The header may be preceded by a little junk; readers look for it in the
    // first kilobyte.
    let head = &d[..d.len().min(1024)];
    if head.windows(5).any(|w| w == b"%PDF-") {
        return Some(Format::Pdf);
    }
    None
}

fn extension(name: &str) -> String {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default()
}

// ----- names -----------------------------------------------------------------

/// `save_paste_temp` stores pasted files as `<unix millis>-<name>`; the prefix
/// is ours and is a timestamp, so it never goes out.
fn strip_paste_prefix(name: &str) -> &str {
    match name.split_once('-') {
        Some((head, rest)) if head.len() >= 10 && head.bytes().all(|b| b.is_ascii_digit()) && !rest.is_empty() => rest,
        _ => name,
    }
}

fn neutral_name(original: &str, clean: &[u8], ext: &str, noun: &str) -> String {
    let name = strip_paste_prefix(original);
    let stem = std::path::Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if !looks_machine_made(stem) {
        return name.to_string();
    }
    let digest = Sha256::digest(clean);
    format!("{noun}_{:02x}{:02x}{:02x}.{ext}", digest[0], digest[1], digest[2])
}

/// Names cameras, screenshot tools and messengers produce. They are worth
/// replacing because they encode when the picture was taken, and sometimes by
/// what. Deliberately a short list of well-known shapes: a false positive here
/// renames a file somebody named on purpose.
fn looks_machine_made(stem: &str) -> bool {
    let s = stem.to_lowercase();
    let digits_after = |prefix: &str| {
        s.strip_prefix(prefix)
            .map(|rest| rest.trim_start_matches(['_', '-', ' ']))
            .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_digit()))
    };
    if ["img", "dsc", "dscn", "dscf", "pxl", "vid", "mvimg", "photo", "image", "signal", "viber_image"]
        .iter()
        .any(|p| digits_after(p))
    {
        return true;
    }
    if ["screenshot", "screen shot", "снимок экрана", "скриншот", "whatsapp image", "clipboard"]
        .iter()
        .any(|p| s.starts_with(p))
    {
        return true;
    }
    // 20260916_165301, 1726500000000 and the like: nothing but a timestamp.
    let digits = s.chars().filter(char::is_ascii_digit).count();
    digits >= 8 && s.chars().all(|c| c.is_ascii_digit() || matches!(c, '_' | '-' | ' ' | '.'))
}

// ----- JPEG ------------------------------------------------------------------

fn clean_jpeg(d: &[u8]) -> Result<Vec<u8>, String> {
    if d.len() < 4 || d[0] != 0xFF || d[1] != 0xD8 {
        return Err("нет начала изображения".into());
    }
    let mut body: Vec<u8> = Vec::with_capacity(d.len());
    let mut orientation: Option<u16> = None;
    // Where a leading JFIF header ends in `body`: the orientation block goes
    // right after it, or right after SOI when there is none.
    let mut jfif_end = 0usize;
    let mut i = 2usize;
    let mut ended = false;

    while i < d.len() {
        if d[i] != 0xFF {
            return Err("ожидался маркер сегмента".into());
        }
        while i < d.len() && d[i] == 0xFF {
            i += 1; // fill bytes
        }
        let Some(&marker) = d.get(i) else { break };
        i += 1;
        match marker {
            0xD9 => {
                body.extend_from_slice(&[0xFF, 0xD9]);
                ended = true;
                break; // whatever follows is not the picture
            }
            0x01 | 0xD0..=0xD7 => {
                body.extend_from_slice(&[0xFF, marker]);
                continue;
            }
            0x00 | 0xD8 => return Err("неожиданный маркер".into()),
            _ => {}
        }
        let len = match d.get(i..i + 2) {
            Some(b) => u16::from_be_bytes([b[0], b[1]]) as usize,
            None => return Err("обрезан заголовок сегмента".into()),
        };
        if len < 2 || i + len > d.len() {
            return Err("сегмент выходит за конец файла".into());
        }
        let payload = &d[i + 2..i + len];
        let keep = match marker {
            0xE0 => payload.starts_with(b"JFIF\0"), // JFXX is a thumbnail
            0xE1 => {
                if orientation.is_none() {
                    if let Some(tiff) = payload.strip_prefix(b"Exif\0\0") {
                        orientation = exif_orientation(tiff);
                    }
                }
                false
            }
            0xE2 => payload.starts_with(b"ICC_PROFILE\0"),
            0xEE => payload.starts_with(b"Adobe"), // without it CMYK/YCCK decode with wrong colours
            0xE3..=0xEF | 0xFE => false,
            _ => true,
        };
        if keep {
            let first = body.is_empty();
            body.extend_from_slice(&[0xFF, marker]);
            body.extend_from_slice(&d[i..i + len]);
            if first && marker == 0xE0 {
                jfif_end = body.len();
            }
        }
        i += len;

        if marker == 0xDA {
            // Entropy-coded data runs to the next real marker. 0xFF00 is a
            // stuffed 0xFF, 0xFFD0–D7 are restart markers, 0xFFFF is fill.
            let start = i;
            loop {
                match d.get(i) {
                    None => return Err("нет конца изображения".into()),
                    Some(0xFF) => match d.get(i + 1) {
                        None => return Err("нет конца изображения".into()),
                        Some(0x00) | Some(0xD0..=0xD7) => i += 2,
                        Some(0xFF) => i += 1,
                        Some(_) => break,
                    },
                    Some(_) => i += 1,
                }
            }
            body.extend_from_slice(&d[start..i]);
        }
    }
    if !ended {
        return Err("нет конца изображения".into());
    }

    let mut out = Vec::with_capacity(body.len() + 40);
    out.extend_from_slice(&[0xFF, 0xD8]);
    out.extend_from_slice(&body[..jfif_end]);
    if let Some(o) = orientation.filter(|o| (2..=8).contains(o)) {
        out.extend_from_slice(&orientation_segment(o));
    }
    out.extend_from_slice(&body[jfif_end..]);
    Ok(out)
}

/// Orientation (tag 0x0112) from IFD0 of a TIFF block, either byte order. A
/// block too broken to read yields `None`: it is being dropped anyway.
fn exif_orientation(tiff: &[u8]) -> Option<u16> {
    let big = match tiff.get(..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b = tiff.get(o..o.checked_add(2)?)?;
        Some(if big { u16::from_be_bytes([b[0], b[1]]) } else { u16::from_le_bytes([b[0], b[1]]) })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b = tiff.get(o..o.checked_add(4)?)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if big { u32::from_be_bytes(a) } else { u32::from_le_bytes(a) })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    for k in 0..count {
        let entry = ifd.checked_add(2 + k * 12)?;
        if u16_at(entry)? == 0x0112 {
            // SHORT, one value, stored inline.
            return (u16_at(entry + 2)? == 3 && u32_at(entry + 4)? == 1).then(|| u16_at(entry + 8)).flatten();
        }
    }
    None
}

/// An APP1/EXIF segment holding the Orientation tag and nothing else.
fn orientation_segment(orientation: u16) -> Vec<u8> {
    let mut s = Vec::with_capacity(36);
    s.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x22]); // length 34: itself + 32
    s.extend_from_slice(b"Exif\0\0");
    s.extend_from_slice(b"MM\0\x2A\0\0\0\x08"); // big-endian TIFF, IFD0 at 8
    s.extend_from_slice(&[0x00, 0x01]); // one entry
    s.extend_from_slice(&[0x01, 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01]); // Orientation, SHORT, ×1
    s.extend_from_slice(&orientation.to_be_bytes());
    s.extend_from_slice(&[0x00, 0x00]); // value field padding
    s.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // no next IFD
    s
}

// ----- PNG -------------------------------------------------------------------

/// Ancillary chunks that change how the image is drawn or animated.
const PNG_KEEP: &[&[u8; 4]] = &[
    b"gAMA", b"cHRM", b"sRGB", b"iCCP", b"sBIT", b"bKGD", b"hIST", b"tRNS", b"pHYs", b"sPLT",
    b"acTL", b"fcTL", b"fdAT", b"cICP", b"mDCv", b"cLLi",
];

fn clean_png(d: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(d.len());
    out.extend_from_slice(&PNG_SIG);
    let mut i = PNG_SIG.len();
    let mut first = true;
    loop {
        let header = d.get(i..i + 8).ok_or("обрезан заголовок чанка")?;
        let len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let kind: [u8; 4] = [header[4], header[5], header[6], header[7]];
        let end = i.checked_add(12).and_then(|n| n.checked_add(len)).filter(|&n| n <= d.len())
            .ok_or("чанк выходит за конец файла")?;
        if first && &kind != b"IHDR" {
            return Err("первый чанк не IHDR".into());
        }
        first = false;
        // Bit 5 of the first byte clear = critical chunk: the image needs it.
        let critical = kind[0] & 0x20 == 0;
        if critical || PNG_KEEP.contains(&&kind) {
            out.extend_from_slice(&d[i..end]);
        }
        i = end;
        if &kind == b"IEND" {
            return Ok(out); // anything after IEND is not the image
        }
    }
}

// ----- WebP ------------------------------------------------------------------

fn clean_webp(d: &[u8]) -> Result<Vec<u8>, String> {
    const EXIF_FLAG: u8 = 0x08;
    const XMP_FLAG: u8 = 0x04;
    let riff = u32::from_le_bytes([d[4], d[5], d[6], d[7]]) as usize;
    let end = riff.checked_add(8).filter(|&n| n <= d.len() && riff >= 4).ok_or("размер RIFF больше файла")?;
    let mut out = Vec::with_capacity(end);
    out.extend_from_slice(b"RIFF\0\0\0\0WEBP");
    let mut i = 12;
    while i < end {
        let header = d.get(i..i + 8).filter(|_| i + 8 <= end).ok_or("обрезан заголовок чанка")?;
        let kind = [header[0], header[1], header[2], header[3]];
        let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let data_end = (i + 8).checked_add(len).filter(|&n| n <= end).ok_or("чанк выходит за конец файла")?;
        if matches!(&kind, b"VP8 " | b"VP8L" | b"VP8X" | b"ALPH" | b"ANIM" | b"ANMF" | b"ICCP") {
            out.extend_from_slice(&d[i..i + 8]);
            let at = out.len();
            out.extend_from_slice(&d[i + 8..data_end]);
            if &kind == b"VP8X" && len >= 1 {
                out[at] &= !(EXIF_FLAG | XMP_FLAG); // they are no longer in the file
            }
            if len % 2 == 1 {
                out.push(0);
            }
        }
        i = data_end + (len % 2); // chunks are padded to an even size
    }
    let size = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&size.to_le_bytes());
    Ok(out)
}

// ----- PDF -------------------------------------------------------------------

fn clean_pdf(d: &[u8]) -> Result<Vec<u8>, String> {
    use lopdf::{Document, Object, ObjectId};

    /// Removes the metadata keys from a dictionary and from everything nested
    /// in it, collecting the indirect objects they pointed at.
    fn strip(obj: &mut Object, dead: &mut Vec<ObjectId>) {
        let dict = match obj {
            Object::Dictionary(dict) => dict,
            Object::Stream(stream) => &mut stream.dict,
            Object::Array(items) => {
                items.iter_mut().for_each(|o| strip(o, dead));
                return;
            }
            _ => return,
        };
        for key in [b"Metadata".as_slice(), b"PieceInfo", b"LastModified"] {
            if let Some(Object::Reference(id)) = dict.remove(key) {
                dead.push(id);
            }
        }
        dict.iter_mut().for_each(|(_, o)| strip(o, dead));
    }

    let mut doc = Document::load_mem(d).map_err(|e| e.to_string())?;
    if doc.is_encrypted() {
        return Err("файл зашифрован".into());
    }
    let mut dead = Vec::new();
    if let Some(Object::Reference(id)) = doc.trailer.remove(b"Info") {
        dead.push(id);
    }
    // Producers derive the file identifier from the path and the time.
    doc.trailer.remove(b"ID");
    doc.objects.values_mut().for_each(|o| strip(o, &mut dead));
    for id in dead {
        doc.objects.remove(&id);
    }
    doc.prune_objects();
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_passes_through_can_go_from_disk() {
        assert!(passes_through("backup.tar.gz", b"\x1f\x8b\x08\x00"));
        assert!(passes_through("notes.bin", &[0, 1, 2, 3]));
        assert!(!passes_through("photo.bin", b"\xff\xd8\xff\xe0"), "a JPEG by content, whatever the name");
        assert!(!passes_through("scan.pdf", b"%PDF-1.7"));
        assert!(!passes_through("fake.jpg", b"not really"), "a cleanable extension goes through the filter");
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    // --- names ---

    #[test]
    fn only_machine_made_names_are_replaced() {
        let png = sample_png();
        for name in [
            "IMG_20260916_165301.png", "IMG-20260916-WA0007.png", "DSC01234.png", "PXL_20260916_1653.png",
            "Screenshot 2026-09-16 at 16.53.01.png", "Снимок экрана от 2026-09-16.png",
            "photo_2026-09-16_16-53-01.png", "20260916_165301.png", "1726500000000-clipboard.png",
        ] {
            let (out, _) = sanitize_attachment_data(name, &png).unwrap();
            assert!(out.starts_with("photo_") && out.ends_with(".png") && out.len() == "photo_abcdef.png".len(), "{name} → {out}");
        }
        for name in ["Отчёт_Q3.png", "rack-B-row-3.png", "diagram.png", "image of the rack.png", "photo booth layout.png"] {
            assert_eq!(sanitize_attachment_data(name, &png).unwrap().0, name);
        }
        // The paste prefix is ours and is a timestamp: it goes, the name stays.
        assert_eq!(sanitize_attachment_data("1726500000000-wiring.png", &png).unwrap().0, "wiring.png");
        assert_eq!(sanitize_attachment_data("1726500000000-notes.txt", b"hello").unwrap().0, "notes.txt");
    }

    // --- JPEG ---

    fn seg(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, marker];
        v.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(payload);
        v
    }

    /// EXIF with Make="Canon" and Orientation, in either byte order.
    fn exif(orientation: u16, big: bool) -> Vec<u8> {
        let w16 = |v: u16| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let w32 = |v: u32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let mut t = Vec::new();
        t.extend_from_slice(if big { b"MM" } else { b"II" });
        t.extend_from_slice(&w16(42));
        t.extend_from_slice(&w32(8));
        t.extend_from_slice(&w16(2));
        // Make: ASCII ×6 at offset 38 (8 + 2 + 24 + 4)
        t.extend_from_slice(&w16(0x010F)); t.extend_from_slice(&w16(2)); t.extend_from_slice(&w32(6)); t.extend_from_slice(&w32(38));
        t.extend_from_slice(&w16(0x0112)); t.extend_from_slice(&w16(3)); t.extend_from_slice(&w32(1));
        t.extend_from_slice(&w16(orientation)); t.extend_from_slice(&[0, 0]);
        t.extend_from_slice(&w32(0));
        t.extend_from_slice(b"Canon\0");
        [b"Exif\0\0".as_slice(), &t].concat()
    }

    const SCAN: &[u8] = &[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56, 0xFF, 0xFF, 0x00, 0x78];

    fn sample_jpeg(orientation: u16, big: bool) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        j.extend(seg(0xE0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0"));
        j.extend(seg(0xE0, b"JFXX\0\x10thumbnail-of-the-uncropped-picture"));
        j.extend(seg(0xE1, &exif(orientation, big)));
        j.extend(seg(0xE1, b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>Jane Roe</x:xmpmeta>"));
        j.extend(seg(0xE2, b"ICC_PROFILE\0\x01\x01profile-bytes"));
        j.extend(seg(0xE2, b"MPF\0second-image-index"));
        j.extend(seg(0xEB, b"JP\x00\x00c2pa-manifest: signed by Jane Roe"));
        j.extend(seg(0xED, b"Photoshop 3.0\08BIM-iptc-byline"));
        j.extend(seg(0xEE, b"Adobe\0\x64\0\0\0\0\x01"));
        j.extend(seg(0xFE, b"shot on my phone"));
        j.extend(seg(0xDB, &[0u8; 65]));
        j.extend(seg(0xC0, &[8, 0, 1, 0, 1, 1, 1, 0x11, 0]));
        j.extend(seg(0xC4, &[0u8; 20]));
        j.extend(seg(0xDA, &[1, 1, 0, 0, 0x3F, 0]));
        j.extend_from_slice(SCAN);
        j.extend_from_slice(&[0xFF, 0xD9]);
        j.extend_from_slice(b"\0\0\0\x18ftypmp42-a-whole-video-after-the-picture");
        j
    }

    /// (marker, payload) of every segment up to the scan.
    fn segments(j: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut out = Vec::new();
        let mut i = 2;
        while j[i + 1] != 0xDA {
            let len = u16::from_be_bytes([j[i + 2], j[i + 3]]) as usize;
            out.push((j[i + 1], j[i + 4..i + 2 + len].to_vec()));
            i += 2 + len;
        }
        out
    }

    #[test]
    fn jpeg_loses_its_metadata_and_keeps_what_draws_it() {
        for big in [true, false] {
            let (_, out) = sanitize_attachment_data("holiday.jpg", &sample_jpeg(6, big)).unwrap();
            let segs = segments(&out);
            let markers: Vec<u8> = segs.iter().map(|(m, _)| *m).collect();
            assert_eq!(markers, [0xE0, 0xE1, 0xE2, 0xEE, 0xDB, 0xC0, 0xC4], "byte order big={big}");
            assert!(segs[0].1.starts_with(b"JFIF\0"));
            assert!(segs[2].1.starts_with(b"ICC_PROFILE\0"));
            assert!(segs[3].1.starts_with(b"Adobe"));
            // The one EXIF block left says which way is up and nothing else.
            assert_eq!(segs[1].1.len(), 32);
            assert_eq!(exif_orientation(&segs[1].1[6..]), Some(6));
            for leak in [b"Canon".as_slice(), b"Jane Roe", b"thumbnail", b"MPF", b"c2pa", b"iptc", b"shot on", b"ftyp"] {
                assert!(!contains(&out, leak), "{:?} survived", String::from_utf8_lossy(leak));
            }
            // The scan is untouched and the file ends at the end of the image.
            assert!(contains(&out, SCAN));
            assert!(out.ends_with(&[0x78, 0xFF, 0xD9]));
        }
    }

    #[test]
    fn upright_jpeg_gets_no_exif_at_all() {
        let (_, out) = sanitize_attachment_data("a.jpg", &sample_jpeg(1, true)).unwrap();
        assert!(!segments(&out).iter().any(|(m, _)| *m == 0xE1));
    }

    #[test]
    fn broken_jpeg_is_refused_not_passed_through() {
        let whole = sample_jpeg(6, true);
        let eoi = whole.windows(2).rposition(|w| w == [0xFF, 0xD9]).unwrap();
        for cut in [eoi, eoi - 4, 40, 7] {
            let err = sanitize_attachment_data("a.jpg", &whole[..cut]).unwrap_err();
            assert!(err.contains("JPEG"), "{err}");
        }
    }

    // --- PNG ---

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = (data.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(data);
        v.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]); // the CRC travels with the chunk untouched
        v
    }

    fn sample_png() -> Vec<u8> {
        let mut p = PNG_SIG.to_vec();
        p.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]));
        p.extend(chunk(b"pHYs", &[0, 0, 0x0B, 0x13, 0, 0, 0x0B, 0x13, 1]));
        p.extend(chunk(b"tEXt", b"Author\0Jane Roe"));
        p.extend(chunk(b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0<xmp>Jane Roe</xmp>"));
        p.extend(chunk(b"eXIf", b"MM\0*camera-serial-0042"));
        p.extend(chunk(b"tIME", &[0x07, 0xEA, 9, 16, 16, 53, 1]));
        p.extend(chunk(b"caBX", b"c2pa-manifest"));
        p.extend(chunk(b"IDAT", &[0x78, 0x9C, 0x63, 0x60, 0x60, 0x60, 0, 0, 0, 4, 0, 1]));
        p.extend(chunk(b"IEND", &[]));
        p.extend_from_slice(b"appended-after-the-image");
        p
    }

    #[test]
    fn png_keeps_rendering_chunks_only() {
        let (_, out) = sanitize_attachment_data("diagram.png", &sample_png()).unwrap();
        let mut kinds = Vec::new();
        let mut i = 8;
        while i < out.len() {
            let len = u32::from_be_bytes([out[i], out[i + 1], out[i + 2], out[i + 3]]) as usize;
            kinds.push(String::from_utf8_lossy(&out[i + 4..i + 8]).into_owned());
            i += 12 + len;
        }
        assert_eq!(kinds, ["IHDR", "pHYs", "IDAT", "IEND"]);
        assert_eq!(i, out.len(), "nothing after IEND");
        for leak in [b"Jane Roe".as_slice(), b"camera-serial", b"c2pa", b"appended"] {
            assert!(!contains(&out, leak));
        }
    }

    #[test]
    fn broken_png_is_refused() {
        let whole = sample_png();
        let iend = whole.windows(4).position(|w| w == b"IEND").unwrap();
        assert!(sanitize_attachment_data("a.png", &whole[..iend - 4]).unwrap_err().contains("PNG"));
        assert!(sanitize_attachment_data("a.png", &whole[..30]).unwrap_err().contains("PNG"));
        let mut no_ihdr = PNG_SIG.to_vec();
        no_ihdr.extend(chunk(b"IDAT", &[1, 2, 3]));
        no_ihdr.extend(chunk(b"IEND", &[]));
        assert!(sanitize_attachment_data("a.png", &no_ihdr).unwrap_err().contains("IHDR"));
    }

    // --- WebP ---

    fn riff_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = kind.to_vec();
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        if data.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn sample_webp() -> Vec<u8> {
        let mut body = b"WEBP".to_vec();
        body.extend(riff_chunk(b"VP8X", &[0x20 | 0x08 | 0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        body.extend(riff_chunk(b"ICCP", b"icc"));
        body.extend(riff_chunk(b"VP8 ", b"pixel"));
        body.extend(riff_chunk(b"EXIF", b"Exif\0\0camera-serial-0042"));
        body.extend(riff_chunk(b"XMP ", b"<xmp>Jane Roe</xmp>"));
        let mut w = b"RIFF".to_vec();
        w.extend_from_slice(&(body.len() as u32).to_le_bytes());
        w.extend(body);
        w
    }

    #[test]
    fn webp_drops_exif_and_xmp_and_stops_advertising_them() {
        let (_, out) = sanitize_attachment_data("sticker.webp", &sample_webp()).unwrap();
        assert_eq!(u32::from_le_bytes([out[4], out[5], out[6], out[7]]) as usize, out.len() - 8);
        assert_eq!(&out[12..16], b"VP8X");
        assert_eq!(out[20], 0x20, "ICC flag stays, EXIF and XMP flags are cleared");
        assert!(contains(&out, b"ICCP") && contains(&out, b"pixel"));
        assert!(!contains(&out, b"EXIF") && !contains(&out, b"XMP ") && !contains(&out, b"Jane Roe"));
        assert_eq!(out.len() % 2, 0);
    }

    #[test]
    fn broken_webp_is_refused() {
        let mut w = sample_webp();
        let n = w.len();
        assert!(sanitize_attachment_data("a.webp", &w[..n - 6]).unwrap_err().contains("WebP"));
        w[4..8].copy_from_slice(&0xFFFF_FFu32.to_le_bytes());
        assert!(sanitize_attachment_data("a.webp", &w).unwrap_err().contains("WebP"));
    }

    // --- PDF ---

    fn sample_pdf() -> Vec<u8> {
        use lopdf::{dictionary, Document, Object, Stream};
        let mut doc = Document::with_version("1.5");
        let info = doc.add_object(dictionary! {
            "Author" => Object::string_literal("Jane Roe"),
            "Creator" => Object::string_literal("LibreOffice on jane-laptop"),
        });
        let xmp = doc.add_object(Stream::new(
            dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
            b"<x:xmpmeta><dc:creator>Jane Roe</dc:creator></x:xmpmeta>".to_vec(),
        ));
        let page_xmp = doc.add_object(Stream::new(
            dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
            b"<x:xmpmeta>edited on jane-laptop</x:xmpmeta>".to_vec(),
        ));
        let pages = doc.new_object_id();
        let content = doc.add_object(Stream::new(dictionary! {}, b"BT /F1 12 Tf (body text stays) Tj ET".to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => content, "Metadata" => page_xmp,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
        });
        doc.objects.insert(pages, Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1,
        }));
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages, "Metadata" => xmp });
        doc.trailer.set("Root", catalog);
        doc.trailer.set("Info", info);
        doc.trailer.set("ID", vec![Object::string_literal("path-and-time-hash"), Object::string_literal("path-and-time-hash")]);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn pdf_loses_info_and_every_metadata_stream() {
        let original = sample_pdf();
        assert!(contains(&original, b"Jane Roe") && contains(&original, b"jane-laptop"));
        let (name, out) = sanitize_attachment_data("Договор.pdf", &original).unwrap();
        assert_eq!(name, "Договор.pdf");
        for leak in [b"Jane Roe".as_slice(), b"jane-laptop", b"path-and-time-hash", b"xmpmeta"] {
            assert!(!contains(&out, leak), "{:?} survived", String::from_utf8_lossy(leak));
        }
        assert!(contains(&out, b"body text stays"));
        let doc = lopdf::Document::load_mem(&out).unwrap();
        assert!(doc.trailer.get(b"Info").is_err());
        assert_eq!(doc.get_pages().len(), 1);
    }

    #[test]
    fn unreadable_pdf_is_refused() {
        let err = sanitize_attachment_data("a.pdf", b"%PDF-1.7\nthis is not a pdf at all").unwrap_err();
        assert!(err.contains("PDF") && err.contains("настройках"), "{err}");
    }

    // --- everything else ---

    #[test]
    fn formats_that_cannot_be_cleaned_are_refused_whatever_they_are_called() {
        let heic = b"\0\0\0\x18ftypheic\0\0\0\0mif1heic-picture-with-gps";
        for name in ["IMG_0042.HEIC", "renamed-to-look-harmless.jpg", "clip.mov"] {
            let err = sanitize_attachment_data(name, heic).unwrap_err();
            assert!(err.contains("нельзя") && err.contains("JPEG"), "{name}: {err}");
        }
        assert!(sanitize_attachment_data("scan.tiff", b"II*\0rest-of-a-tiff").is_err());
        // By extension alone, when the content says nothing.
        assert!(sanitize_attachment_data("clip.mp4", b"????").is_err());
        // Claims to be a JPEG and is not one.
        assert!(sanitize_attachment_data("photo.jpg", b"GIF89a-not-a-jpeg").unwrap_err().contains("расширению"));
    }

    #[test]
    fn everything_else_goes_through_untouched() {
        for (name, data) in [("notes.txt", b"plain text".as_slice()), ("deploy.sh", b"#!/bin/sh\n"), ("backup.tar.gz", &[0x1F, 0x8B, 8, 0])] {
            let (n, d) = sanitize_attachment_data(name, data).unwrap();
            assert_eq!((n.as_str(), d.as_slice()), (name, data));
        }
    }

    #[test]
    fn content_wins_over_the_extension() {
        // A PNG called .jpg is cleaned as the PNG it is.
        let (_, out) = sanitize_attachment_data("diagram.jpg", &sample_png()).unwrap();
        assert!(out.starts_with(&PNG_SIG) && !contains(&out, b"Jane Roe"));
    }
}
