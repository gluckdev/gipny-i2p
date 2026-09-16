// Sanitizer module: strips metadata (EXIF, IPTC, XMP, device info) from images
// and documents, and anonymizes filenames to prevent device/user fingerprinting.

pub fn sanitize_attachment_data(filename: &str, data: &[u8]) -> (String, Vec<u8>) {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let clean_data = match ext.as_str() {
        "jpg" | "jpeg" => strip_jpeg_metadata(data),
        "png" => strip_png_metadata(data),
        "pdf" => strip_pdf_metadata(data),
        _ => data.to_vec(),
    };

    let clean_name = sanitize_filename(filename, &clean_data);
    (clean_name, clean_data)
}

/// Anonymizes filenames like "IMG_20260916_165301.jpg" or "Personal_Doc.pdf"
/// into neutral names "photo_<hash>.jpg" or "file_<hash>.ext".
pub fn sanitize_filename(original: &str, data: &[u8]) -> String {
    let p = std::path::Path::new(original);
    let ext = p.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let hash = if !data.is_empty() {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(data);
        let digest = hasher.finalize();
        format!("{:02x}{:02x}{:02x}", digest[0], digest[1], digest[2])
    } else {
        "data".to_string()
    };

    match ext.as_str() {
        "jpg" | "jpeg" => format!("photo_{}.jpg", hash),
        "png" => format!("image_{}.png", hash),
        "webp" => format!("image_{}.webp", hash),
        "gif" => format!("animation_{}.gif", hash),
        "ogg" | "opus" | "wav" | "m4a" => format!("audio_{}.{}", hash, ext),
        "mp4" | "mov" | "mkv" => format!("video_{}.{}", hash, ext),
        "pdf" => format!("document_{}.pdf", hash),
        "zip" | "tar" | "gz" | "7z" => format!("archive_{}.{}", hash, ext),
        "" => format!("file_{}", hash),
        _ => {
            let safe_ext: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).take(6).collect();
            if safe_ext.is_empty() {
                format!("file_{}", hash)
            } else {
                format!("doc_{}.{}", hash, safe_ext)
            }
        }
    }
}

/// Strips JPEG metadata: retains SOI, DQT, DHT, SOF, SOS and image scan data,
/// while dropping APP1 (EXIF / XMP), APP2 (FlashPix), APP13 (Photoshop IPTC),
/// and COM (Comments).
pub fn strip_jpeg_metadata(data: &[u8]) -> Vec<u8> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return data.to_vec(); // Not a valid JPEG
    }

    let mut out = Vec::with_capacity(data.len());
    out.push(0xFF);
    out.push(0xD8); // SOI

    let mut i = 2;
    while i < data.len() {
        if data[i] != 0xFF {
            out.extend_from_slice(&data[i..]);
            break;
        }

        while i < data.len() && data[i] == 0xFF {
            i += 1;
        }
        if i >= data.len() { break; }

        let marker = data[i];
        i += 1;

        if marker == 0xD8 || (marker >= 0xD0 && marker <= 0xD7) || marker == 0x01 {
            out.push(0xFF);
            out.push(marker);
            continue;
        }
        if marker == 0xDA {
            out.push(0xFF);
            out.push(marker);
            out.extend_from_slice(&data[i..]);
            break;
        }
        if marker == 0xD9 {
            out.push(0xFF);
            out.push(marker);
            break;
        }

        if i + 2 > data.len() {
            out.extend_from_slice(&data[i - 2..]);
            break;
        }
        let len = ((data[i] as usize) << 8) | (data[i + 1] as usize);
        if i + len > data.len() {
            out.extend_from_slice(&data[i - 2..]);
            break;
        }

        // APP1 = 0xE1 (EXIF/XMP), APP2 = 0xE2, APP13 = 0xED (IPTC), COM = 0xFE
        let is_metadata = marker == 0xE1 || marker == 0xE2 || marker == 0xED || marker == 0xFE;

        if !is_metadata {
            out.push(0xFF);
            out.push(marker);
            out.extend_from_slice(&data[i..i + len]);
        }
        i += len;
    }

    out
}

/// Strips PNG metadata chunks: drops eXIf, tEXt, zTXt, iTXt, tIME, pHYs.
pub fn strip_png_metadata(data: &[u8]) -> Vec<u8> {
    const PNG_SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    if data.len() < 8 || &data[0..8] != &PNG_SIG {
        return data.to_vec();
    }

    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&PNG_SIG);

    let mut i = 8;
    while i + 8 <= data.len() {
        let length = u32::from_be_bytes([data[i], data[i+1], data[i+2], data[i+3]]) as usize;
        let chunk_type = &data[i+4..i+8];
        let total_chunk_len = 12 + length;

        if i + total_chunk_len > data.len() {
            out.extend_from_slice(&data[i..]);
            break;
        }

        let is_meta = match chunk_type {
            b"eXIf" | b"tEXt" | b"zTXt" | b"iTXt" | b"tIME" | b"pHYs" => true,
            _ => false,
        };

        if !is_meta {
            out.extend_from_slice(&data[i..i + total_chunk_len]);
        }

        if chunk_type == b"IEND" {
            break;
        }

        i += total_chunk_len;
    }

    out
}

/// Strips simple PDF metadata
pub fn strip_pdf_metadata(data: &[u8]) -> Vec<u8> {
    if data.len() < 5 || &data[0..5] != b"%PDF-" {
        return data.to_vec();
    }

    let mut out = data.to_vec();
    let keys = [
        b"/Author".as_slice(),
        b"/Creator".as_slice(),
        b"/Producer".as_slice(),
        b"/CreationDate".as_slice(),
        b"/ModDate".as_slice(),
        b"/Title".as_slice(),
    ];

    for key in &keys {
        let mut idx = 0;
        while let Some(pos) = find_subsequence(&out[idx..], key) {
            let start = idx + pos + key.len();
            let mut j = start;
            while j < out.len() && (out[j] == b' ' || out[j] == b'\t' || out[j] == b'\r' || out[j] == b'\n') {
                j += 1;
            }
            if j < out.len() && out[j] == b'(' {
                let content_start = j + 1;
                let mut depth = 1;
                j += 1;
                while j < out.len() && depth > 0 {
                    if out[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    if out[j] == b'(' { depth += 1; }
                    else if out[j] == b')' { depth -= 1; }
                    j += 1;
                }
                let content_end = if depth == 0 { j - 1 } else { j };
                for b in &mut out[content_start..content_end] {
                    *b = b' ';
                }
            }
            idx = j;
        }
    }

    out
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}
