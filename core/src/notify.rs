#[cfg(target_os = "linux")]
const NOTIFY_WAV: &[u8] = include_bytes!("../../ui/public/notify.wav");

include!(concat!(env!("OUT_DIR"), "/sounds.rs"));

#[cfg(target_os = "linux")]
use std::sync::OnceLock;

#[cfg(target_os = "linux")]
struct Backends {
    players: Vec<&'static str>,
    notifier: Option<&'static str>,
}

#[cfg(target_os = "linux")]
static BACKENDS: OnceLock<Backends> = OnceLock::new();

#[cfg(target_os = "linux")]
fn find_in_path(cmd: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|p| {
        let full = p.join(cmd);
        std::fs::metadata(&full).map(|m| m.is_file()).unwrap_or(false)
    })
}

#[cfg(target_os = "linux")]
fn backends() -> &'static Backends {
    BACKENDS.get_or_init(|| {
        let candidates = ["paplay", "pw-play", "canberra-gtk-play", "aplay"];
        let players: Vec<&'static str> = candidates.iter().copied().filter(|c| find_in_path(c)).collect();
        let notifier = if find_in_path("notify-send") { Some("notify-send") } else { None };
        Backends { players, notifier }
    })
}

#[cfg(target_os = "linux")]
fn sound_bytes(name: &str) -> &'static [u8] {
    for (n, b) in EMBEDDED_SOUNDS {
        if *n == name { return b; }
    }
    NOTIFY_WAV
}

#[cfg(target_os = "linux")]
fn safe_key(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(32).collect()
}

#[cfg(target_os = "linux")]
fn extract_sound(name: &str) -> Option<std::path::PathBuf> {
    let key = safe_key(name);
    let key = if key.is_empty() { "default".to_string() } else { key };
    let bytes = sound_bytes(&key);
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let uid = unsafe { libc::getuid() };
    let path = dir.join(format!("gipny-sound-{}-{}-{}.wav", uid, env!("CARGO_PKG_VERSION"), key));
    if !path.exists() {
        std::fs::write(&path, bytes).ok()?;
    }
    Some(path)
}

#[cfg(target_os = "linux")]
fn spawn_player(player: &str, wav: &std::path::Path) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(player);
    match player {
        "canberra-gtk-play" => { cmd.arg("-f").arg(wav); }
        "aplay" => { cmd.arg("-q").arg(wav); }
        _ => { cmd.arg(wav); }
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.spawn().map(|_| ())
}

#[cfg(target_os = "linux")]
fn rodio_fallback(name: &str) {
    let bytes = sound_bytes(name);
    std::thread::spawn(move || {
        let Ok(stream) = rodio::DeviceSinkBuilder::open_default_sink() else { return };
        let sink = rodio::Player::connect_new(stream.mixer());
        let Ok(decoder) = rodio::Decoder::new(std::io::Cursor::new(bytes)) else { return };
        sink.set_volume(0.4);
        sink.append(decoder);
        sink.sleep_until_end();
    });
}

pub fn play_sound(name: Option<String>) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let name = name.as_deref().unwrap_or("default");
        let b = backends();
        let mut last_err: Option<String> = None;
        if let Some(wav) = extract_sound(name) {
            for player in &b.players {
                match spawn_player(player, &wav) {
                    Ok(()) => return Ok(()),
                    Err(e) => last_err = Some(format!("{}: {}", player, e)),
                }
            }
        }
        rodio_fallback(name);
        if b.players.is_empty() {
            return Err("no audio backend found (install pulseaudio-utils, pipewire, alsa-utils or libcanberra)".into());
        }
        if let Some(e) = last_err {
            return Err(format!("all players failed; fallback to rodio. last: {}", e));
        }
        return Ok(());
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = name;
        Ok(())
    }
}

pub fn notify_os(title: &str, body: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use std::process::{Command, Stdio};
        let b = backends();
        let Some(notifier) = b.notifier else {
            return Err("notify-send not found (install libnotify)".into());
        };
        let mut cmd = Command::new(notifier);
        cmd.arg("-a").arg("gipny")
            .arg("-c").arg("im.received")
            .arg("--")
            .arg(title)
            .arg(body)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd.spawn().map_err(|e| format!("notify-send spawn: {}", e))?;
        return Ok(());
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (title, body);
        Ok(())
    }
}

pub fn probe_report() -> String {
    let embedded: Vec<&str> = EMBEDDED_SOUNDS.iter().map(|(n, _)| *n).collect();
    let embedded_str = if embedded.is_empty() { "default".to_string() } else { embedded.join(",") };
    #[cfg(target_os = "linux")]
    {
        let b = backends();
        let players = if b.players.is_empty() { "none".to_string() } else { b.players.join(",") };
        let notifier = b.notifier.unwrap_or("none");
        return format!("players={} notifier={} sounds={}", players, notifier, embedded_str);
    }
    #[cfg(not(target_os = "linux"))]
    {
        format!("sounds={}", embedded_str)
    }
}
