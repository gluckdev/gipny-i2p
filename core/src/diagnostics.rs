//! Build-gated diagnostic capture, access, and wiping.

use tauri::State;

use crate::AppCtx;

/// Logging is a build capability, not a per-profile or runtime preference.
pub(crate) fn log_enabled(_base_dir: &std::path::Path) -> bool {
    gipny_libcore::diagnostics::AVAILABLE
}

/// Redirect this process's stderr into a file, one timestamped line at a time.
///
/// Unix only: it is a `dup2`, and Windows has no equivalent that survives the
/// way Tauri starts up. stderr goes into a pipe rather than straight into the
/// file so every line can be stamped — without that, a log of "relay failed"
/// and "router ready" says nothing about *when*, which is most of what one
/// needs from it.
#[cfg(unix)]
pub(crate) fn install_log_capture(base_dir: &std::path::Path) {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::io::FromRawFd;

    let log_path = base_dir.join("debug.log");
    // Keep the previous run's log: a crash is only readable afterwards, and
    // truncating on start threw away exactly the interesting one.
    let _ = std::fs::rename(&log_path, base_dir.join("debug.prev.log"));
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path)
    else {
        return;
    };

    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return;
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);
    if unsafe { libc::dup2(write_fd, libc::STDERR_FILENO) } < 0 {
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return;
    }
    unsafe { libc::close(write_fd) };

    std::thread::spawn(move || {
        let reader = BufReader::new(unsafe { std::fs::File::from_raw_fd(read_fd) });
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let _ = writeln!(file, "{} {line}", log_stamp());
            let _ = file.flush();
        }
    });
    eprintln!("log capture installed: {}", log_path.display());
    eprintln!(
        "gipny {} on {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    );
}

/// `HH:MM:SS.mmm` in local time — enough to line our log up against i2pd's,
/// which stamps the same way, without dragging in a date library.
#[cfg(unix)]
fn log_stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let ms = now.subsec_millis();
    let local = secs + local_utc_offset_secs();
    let tod = local.rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60,
        ms
    )
}

/// The offset `localtime_r` reports for now, in seconds.
#[cfg(unix)]
fn local_utc_offset_secs() -> i64 {
    unsafe {
        let mut t: libc::time_t = 0;
        libc::time(&mut t);
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
}

/// Best-effort overwrite-and-remove of the app-global debug.log (if any). Called
/// on a duress/attempt-limit wipe so no plaintext log survives outside the
/// per-profile dir that `secure_wipe_dir` scrubs.
pub(crate) fn scrub_debug_log(base_dir: &std::path::Path) {
    scrub_one_log(&base_dir.join("debug.prev.log"));
    scrub_one_log(&base_dir.join("debug.log"));
}

fn scrub_one_log(p: &std::path::Path) {
    let p = p.to_path_buf();
    if let Ok(meta) = std::fs::metadata(&p) {
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&p) {
            use std::io::Write;
            let mut f = f;
            let _ = f.write_all(&vec![0u8; meta.len() as usize]);
            let _ = f.flush();
        }
    }
    let _ = std::fs::remove_file(&p);
}

#[tauri::command]
pub(crate) fn read_debug_log(ctx: State<'_, AppCtx>) -> Result<String, String> {
    read_log_file(&ctx.base_dir.join("debug.log"))
}

/// The log of the run before this one — where a crash left its last words.
#[tauri::command]
pub(crate) fn read_previous_log(ctx: State<'_, AppCtx>) -> Result<String, String> {
    read_log_file(&ctx.base_dir.join("debug.prev.log"))
}

fn read_log_file(p: &std::path::Path) -> Result<String, String> {
    if !gipny_libcore::diagnostics::AVAILABLE {
        return Err("diagnostic logging is unavailable in release builds".into());
    }
    let raw = std::fs::read_to_string(p).unwrap_or_else(|_| String::from("(журнал пуст)"));
    let lines: Vec<&str> = raw.lines().collect();
    let take = lines.len().saturating_sub(2000);
    Ok(lines[take..].join("\n"))
}

/// Whether the log is being written, and where it is.
#[tauri::command]
pub(crate) fn log_settings(ctx: State<'_, AppCtx>) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "available": gipny_libcore::diagnostics::AVAILABLE,
        "enabled": log_enabled(&ctx.base_dir),
        "path": if gipny_libcore::diagnostics::AVAILABLE {
            ctx.base_dir.join("debug.log").display().to_string()
        } else { String::new() },
    }))
}

/// Wipe what has been written so far, for handing the app to someone else.
#[tauri::command]
pub(crate) fn clear_debug_log(ctx: State<'_, AppCtx>) -> Result<(), String> {
    scrub_debug_log(&ctx.base_dir);
    Ok(())
}
