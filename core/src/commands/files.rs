//! Attachments, privacy filtering, media history, and clipboard adapters.

use tauri::{AppHandle, State};

use crate::core::PendingAttachment;
use crate::dto::{AttachmentDto, FileProgressDto, MediaItemDto, MessageFilesDto};
use crate::{base64_encode, core_of, err, parse_group_id, sanitizer, AppCtx};

const SETTING_ATTACHMENT_PRIVACY: &str = "attachment_privacy";

pub(crate) fn attachment_privacy_enabled(db: &gipny_libcore::db::Db) -> Result<bool, String> {
    Ok(!matches!(
        db.get_setting(SETTING_ATTACHMENT_PRIVACY)
            .map_err(err)?
            .as_deref(),
        Some(b"0"),
    ))
}

#[tauri::command]
pub(crate) async fn get_attachment_privacy(ctx: State<'_, AppCtx>) -> Result<bool, String> {
    let core = core_of(&ctx).await?;
    attachment_privacy_enabled(core.db())
}

#[tauri::command]
pub(crate) async fn set_attachment_privacy(
    enabled: bool,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .db()
        .set_setting(
            SETTING_ATTACHMENT_PRIVACY,
            if enabled { b"1" } else { b"0" },
        )
        .map_err(err)
}

/// Past 128 KiB a file goes in parts (libcore::files), so the old 12 MiB cap
/// (one letter) is gone. Most large files are sent from disk (up to
/// files::MAX_FILE_BYTES); this caps the ones read into memory to be
/// cleaned of metadata (photos, PDF).
const MAX_ATTACHMENT_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) fn prepare_attachment(
    name: String,
    data: Vec<u8>,
    sanitize: bool,
) -> Result<PendingAttachment, String> {
    if sanitize {
        let (name, data) = sanitizer::sanitize_attachment_data(&name, &data)?;
        Ok(PendingAttachment {
            name,
            data,
            from: None,
        })
    } else {
        // Console uploads are operational files: scripts, configs and command
        // arguments rely on the original name and exact bytes.
        Ok(PendingAttachment {
            name,
            data,
            from: None,
        })
    }
}

pub(crate) static PASTE_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// The directory temporary copies of attachments go to (see `setup`).
fn paste_dir() -> std::path::PathBuf {
    PASTE_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir().join("gipny-i2p-paste"))
}

pub(crate) fn read_one_attachment(p: &str, sanitize: bool) -> Result<PendingAttachment, String> {
    let path = std::path::PathBuf::from(p);
    let meta = std::fs::metadata(&path).map_err(err)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| "file".into());
    // Large, and nothing the privacy filter would change: sent from disk in
    // parts, never read whole (pasted copies stay the in-memory way: they are
    // removed right after). Up to what files in parts carry.
    if meta.len() > gipny_libcore::files::INLINE_MAX as u64 && !path.starts_with(paste_dir()) {
        use std::io::Read;
        let mut head = [0u8; 64];
        let n = std::fs::File::open(&path)
            .and_then(|mut f| f.read(&mut head))
            .map_err(err)?;
        if !sanitize || sanitizer::passes_through(&name, &head[..n]) {
            if meta.len() > gipny_libcore::files::MAX_FILE_BYTES {
                return Err(format!(
                    "файл слишком большой: {} ({} МБ, лимит {} МБ)",
                    p,
                    meta.len() / (1024 * 1024),
                    gipny_libcore::files::MAX_FILE_BYTES / (1024 * 1024)
                ));
            }
            let name = if sanitize {
                sanitizer::passed_name(&name)
            } else {
                name
            };
            return Ok(PendingAttachment {
                name,
                data: Vec::new(),
                from: Some(path),
            });
        }
    }
    if meta.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "файл слишком большой: {} ({} МБ, лимит {} МБ)",
            p,
            meta.len() / (1024 * 1024),
            MAX_ATTACHMENT_BYTES / (1024 * 1024)
        ));
    }
    let data = std::fs::read(&path).map_err(err)?;
    let attachment = prepare_attachment(name, data, sanitize)?;

    // Pasted images and drops arrive through a temp copy (`save_paste_temp`,
    // `paste_clipboard_image`). Remove it only after preparation succeeded: a
    // rejected format can then be retried after privacy mode is switched off.
    if path.starts_with(paste_dir()) {
        let _ = std::fs::remove_file(&path);
    }
    Ok(attachment)
}

#[cfg(test)]
mod attachment_path_tests {
    use super::{prepare_attachment, read_one_attachment};

    fn file(dir: &std::path::Path, name: &str, head: &[u8], size: usize) -> String {
        let mut data = head.to_vec();
        data.resize(size, 7);
        let p = dir.join(name);
        std::fs::write(&p, &data).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn a_large_file_the_filter_passes_goes_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let big = gipny_libcore::files::INLINE_MAX + 1;
        let p = file(dir.path(), "backup.tar.gz", b"\x1f\x8b", big);
        let a = read_one_attachment(&p, true).unwrap();
        assert!(a.from.is_some() && a.data.is_empty(), "streamed, not read");
        assert_eq!(a.name, "backup.tar.gz");
    }

    #[test]
    fn a_large_photo_is_read_to_be_cleaned_and_a_small_file_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let big = gipny_libcore::files::INLINE_MAX + 1;
        // A JPEG by content: the filter must see it whole (here it refuses the
        // made-up bytes, which proves it looked rather than streaming them).
        let p = file(dir.path(), "photo.jpg", b"\xff\xd8\xff\xe0", big);
        assert!(read_one_attachment(&p, true).is_err());
        // With the filter off the same file goes from disk.
        let a = read_one_attachment(&p, false).unwrap();
        assert!(a.from.is_some());
        // Small: in memory, as before.
        let p = file(dir.path(), "note.txt", b"hi", 10);
        let a = read_one_attachment(&p, true).unwrap();
        assert!(a.from.is_none() && a.data.len() == 10);
    }

    #[test]
    fn console_attachment_keeps_the_original_name_and_bytes() {
        let bytes = b"#!/bin/sh\necho hello\n".to_vec();
        let a = prepare_attachment("deploy.sh".into(), bytes.clone(), false).unwrap();
        assert_eq!(a.name, "deploy.sh");
        assert_eq!(a.data, bytes);
    }
}

#[tauri::command]
pub(crate) async fn list_attachments(
    message_id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<Vec<AttachmentDto>, String> {
    let list = core_of(&ctx)
        .await?
        .db()
        .list_attachments(message_id)
        .map_err(err)?;
    Ok(list
        .into_iter()
        .map(|a| AttachmentDto {
            id: a.id,
            message_id: a.message_id,
            name: a.name,
            size: a.size,
        })
        .collect())
}

/// Files of a message still moving in parts (not yet attachments, or not yet
/// at every recipient).
#[tauri::command]
pub(crate) async fn files_progress(
    message_id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<Vec<FileProgressDto>, String> {
    let rows = core_of(&ctx)
        .await?
        .files_progress(message_id)
        .map_err(err)?;
    Ok(rows
        .into_iter()
        .map(
            |(name, size, done, total, incoming, status)| FileProgressDto {
                name,
                size,
                done,
                total,
                incoming,
                status,
            },
        )
        .collect())
}

/// One bounded request for the visible history, also after reopening a chat.
#[tauri::command]
pub(crate) async fn message_files(
    message_ids: Vec<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<std::collections::HashMap<i64, MessageFilesDto>, String> {
    if message_ids.len() > 500 || message_ids.iter().any(|id| *id <= 0) {
        return Err("invalid file history window".into());
    }
    let core = core_of(&ctx).await?;
    let mut result = std::collections::HashMap::new();
    for id in message_ids {
        let attachments = core
            .db()
            .list_attachments(id)
            .map_err(err)?
            .into_iter()
            .map(|a| AttachmentDto {
                id: a.id,
                message_id: a.message_id,
                name: a.name,
                size: a.size,
            })
            .collect();
        let progress = core
            .files_progress(id)
            .map_err(err)?
            .into_iter()
            .map(
                |(name, size, done, total, incoming, status)| FileProgressDto {
                    name,
                    size,
                    done,
                    total,
                    incoming,
                    status,
                },
            )
            .collect();
        result.insert(
            id,
            MessageFilesDto {
                attachments,
                progress,
            },
        );
    }
    Ok(result)
}

/// Stop sending a message's files in parts.
#[tauri::command]
pub(crate) async fn cancel_files(message_id: i64, ctx: State<'_, AppCtx>) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .cancel_files(message_id)
        .await
        .map_err(err)
}

#[tauri::command]
pub(crate) async fn list_media_contact(
    contact_id: i64,
    limit: i64,
    ctx: State<'_, AppCtx>,
) -> Result<Vec<MediaItemDto>, String> {
    let rows = core_of(&ctx)
        .await?
        .db()
        .list_attachments_for_contact(contact_id, limit)
        .map_err(err)?;
    Ok(rows
        .into_iter()
        .map(|(a, ts)| MediaItemDto {
            id: a.id,
            message_id: a.message_id,
            name: a.name,
            size: a.size,
            sent_at: ts,
        })
        .collect())
}

#[tauri::command]
pub(crate) async fn list_media_group(
    group_id: String,
    limit: i64,
    ctx: State<'_, AppCtx>,
) -> Result<Vec<MediaItemDto>, String> {
    let gid = parse_group_id(&group_id)?;
    let rows = core_of(&ctx)
        .await?
        .db()
        .list_attachments_for_group(&gid, limit)
        .map_err(err)?;
    Ok(rows
        .into_iter()
        .map(|(a, ts)| MediaItemDto {
            id: a.id,
            message_id: a.message_id,
            name: a.name,
            size: a.size,
            sent_at: ts,
        })
        .collect())
}

#[tauri::command]
pub(crate) async fn load_attachment(
    attachment_id: i64,
    ctx: State<'_, AppCtx>,
) -> Result<String, String> {
    let core = core_of(&ctx).await?;
    let att = core
        .db()
        .get_attachment(attachment_id)
        .map_err(err)?
        .ok_or("not found")?;
    let bytes = core.read_attachment(&att).map_err(err)?;
    Ok(base64_encode(&bytes))
}

#[tauri::command]
pub(crate) async fn save_attachment(
    attachment_id: i64,
    dest_path: String,
    app: AppHandle,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    use std::io::Write;
    use std::str::FromStr;
    use tauri_plugin_fs::FsExt;
    let core = core_of(&ctx).await?;
    let att = core
        .db()
        .get_attachment(attachment_id)
        .map_err(err)?
        .ok_or("not found")?;
    // On Android the save dialog returns a content:// URI, which std::fs
    // cannot open; the fs plugin resolves it through the content resolver.
    // On the desktop it is an ordinary path either way.
    let path =
        tauri_plugin_fs::FilePath::from_str(&dest_path).map_err(|_| "bad path".to_string())?;
    let mut opts = tauri_plugin_fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    let file = app.fs().open(path, opts).map_err(err)?;
    // Part by part for a file sent in parts: never whole in memory.
    let mut out = std::io::BufWriter::new(file);
    core.write_attachment_to(&att, &mut out).map_err(err)?;
    out.flush().map_err(err)?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn save_paste_temp(name: String, data: Vec<u8>) -> Result<String, String> {
    let dir = paste_dir();
    std::fs::create_dir_all(&dir).map_err(err)?;
    let safe_name = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    let prefix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("{}-{}", prefix, safe_name));
    std::fs::write(&path, &data).map_err(err)?;
    Ok(path.to_string_lossy().to_string())
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
pub(crate) async fn paste_clipboard_image() -> Result<Option<String>, String> {
    let img = match tokio::task::spawn_blocking(|| {
        let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        cb.get_image().map_err(|e| match e {
            arboard::Error::ContentNotAvailable => "no-image".to_string(),
            other => other.to_string(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
    {
        Ok(img) => img,
        Err(s) if s == "no-image" => return Ok(None),
        Err(s) => return Err(s),
    };
    if img.width == 0 || img.height == 0 || img.bytes.is_empty() {
        return Ok(None);
    }
    let mut png_buf = Vec::with_capacity(img.bytes.len() / 4);
    {
        let mut enc = png::Encoder::new(&mut png_buf, img.width as u32, img.height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&img.bytes)
            .map_err(|e| e.to_string())?;
    }
    let dir = paste_dir();
    std::fs::create_dir_all(&dir).map_err(err)?;
    let prefix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("{}-clipboard.png", prefix));
    std::fs::write(&path, &png_buf).map_err(err)?;
    Ok(Some(path.to_string_lossy().to_string()))
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
pub(crate) async fn paste_clipboard_image() -> Result<Option<String>, String> {
    Ok(None)
}
