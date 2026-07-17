# gipny

i2p-routed end-to-end encrypted desktop & mobile messenger. No phone, no email, no username. Identity is a pair of ed25519/x25519 keys plus an i2p destination — nothing else.

> **Fork note:** this is the **i2p** fork of gipny. The transport was migrated from Tor (Arti) to i2p (go‑i2p via SAMv3). Crypto, vault, relay protocol and UI are unchanged. See **[MIGRATION-i2p.md](MIGRATION-i2p.md)** for the full change log and current status.

- **Transport:** i2p via the bundled pure‑Go **go‑i2p** router (SAMv3), spoken from Rust with the `yosemite` crate. The router ships inside the app — nothing to install.
- **Crypto:** X3DH initial handshake + Double Ratchet (same primitives as Signal), XChaCha20-Poly1305 AEAD, Ed25519 + X25519.
- **Vault at rest:** SQLCipher (AES-256) with Argon2id KDF. Duress passphrase (wipe or decoy), attempt-limit auto-wipe, process hardening (no core dumps, mlock).
- **Metadata minimization:** sealed sender (`from = [0u8; 32]` on the wire — relay never sees the sender), fixed-bucket payload padding (sizes don't leak text vs. attachment vs. media).
- **Delivery:** single central blind relay over i2p — never P2P direct, but offline delivery works (relay holds encrypted blobs until peer is online).
- **Bot SDK:** first-class Rust crate for building bots that send text, files (multi-attachment), and inline buttons — all over the same encrypted channel.

### How the transport works

gipny bundles a small `gipny-i2p-router` binary (a wrapper around go‑i2p's embedded SAM bridge). On launch the app spawns it, waits for the SAMv3 API on `127.0.0.1:7656`, and routes everything through i2p automatically — you don't install anything.

The chain is: `you → i2p tunnels → gipny relay → recipient destination`. First launch is slower than subsequent ones (i2p reseeds and builds tunnels). A system‑wide VPN before launching adds one more layer between your real IP and the i2p entry.

> The former outer‑SOCKS5‑proxy option was removed: with i2p your traffic to the local router is loopback and the router does its own peer routing.

---

## Install prebuilt binaries

If you just want to use gipny, grab a release artifact for your platform from the **[Releases page](../../releases)** — no toolchain or compilation required.

### Linux

```bash
# AppImage (no install, just run)
chmod +x gipny-*.AppImage
./gipny-*.AppImage

# .deb (Debian / Ubuntu / Parrot / Mint / Kali)
sudo apt install ./gipny_*_amd64.deb

# .tar.gz (any glibc-based distro)
tar -xzf gipny-*-linux-amd64.tar.gz
cd gipny-*-linux-amd64/
./gipny
```

### Windows

Two artifacts per release:
- `gipny-*-windows-x64-setup.exe` — NSIS installer (recommended).
- `gipny-*-windows-x64.zip` — portable zip, unpack and run `gipny.exe`.

The installer puts gipny under `C:\Program Files\gipny\` and creates a Start menu shortcut. No admin rights needed for the portable zip.

### Android

`gipny-*-android-arm64.apk` (for typical phones) or `gipny-*-android-x86_64.apk` (for emulators / Chromebooks).

Sideload via `adb install -r gipny-*-android-arm64.apk`, or copy the APK to the phone and open it from a file manager — Android will offer to install. APKs are unsigned debug builds; you'll need to allow "Install unknown apps" for the source you used.

Once installed: open the app, create a profile, you're on Tor. First Tor bootstrap on cellular takes 30 s to 2 min; subsequent launches are faster.

---

## Project layout

```
core/        Tauri 2 desktop & android client (Rust + TS UI)
libcore/     shared crypto, session, transport, db
bot-sdk/     bot framework
ui/          TypeScript UI (vanilla, no framework)
docker/      Linux + Windows cross-build scripts
build.sh     one-shot release builder (lin + win + android)
```

The `core/relay/` directory is the relay server crate. It's deliberately outside the workspace and only needed if you want to run your own relay. Most builders can ignore it.

---

## Building from source

Targets: **Linux** (AppImage / .deb / .tar.gz), **Windows** (NSIS installer / portable zip), **Android** (APK arm64 + x86\_64). macOS is not currently supported.

### Linux native dev build

System dependencies (Debian/Ubuntu names; equivalents on other distros):

```bash
sudo apt install -y build-essential pkg-config curl ca-certificates git \
    libssl-dev libgtk-3-dev libwebkit2gtk-4.1-dev \
    libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev \
    libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
    gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly \
    gstreamer1.0-libav gstreamer1.0-pulseaudio gstreamer1.0-alsa \
    libasound2-dev nodejs npm
```

Install Rust (stable) via [rustup](https://rustup.rs/).

Then:

```bash
cd ui && npm install && npm run build && cd ..
cd core && cargo run                       # dev run
```

The Tauri window opens once the binary is built.

### Reproducible Linux / Windows release artifacts via Docker

Requires Docker. Produces clean binaries that don't depend on the host glibc / WebKit2GTK version.

```bash
./build.sh                    # all three: linux + windows + android (host SDK)
./build.sh --linux-only       # AppImage + .deb + .tar.gz
./build.sh --windows-only     # NSIS installer + portable zip
./build.sh --android-only     # APKs (arm64 + x86_64) — uses host JDK17 + Android SDK/NDK
./build.sh --no-android       # skip android (lin + win)
./build.sh --wipe             # clean release-artifacts/ first
```

Outputs land in `release-artifacts/`.

### Android — what you need on the host

The Android build runs natively against your local toolchain (not Docker, because the Tauri Android plugin pulls plugin sources from `~/.cargo/registry`).

Required:
- Android SDK (compile-SDK 36, min-SDK 24)
- Android NDK
- JDK 17 (newer JDK breaks Gradle 8.11 + AGP 8.11)

Environment variables: `ANDROID_HOME`, `NDK_HOME`, `JAVA_HOME`. `build.sh` autodetects `JAVA_HOME` on Void and Debian; for other distros set it manually.

The output APK is unsigned debug — sideload via `adb install -r` or from a file manager. The Play Store is not a target.

### Windows — note on cross-compile

The Windows build uses **mingw64** (`x86_64-pc-windows-gnu`), not MSVC, because `openssl-sys` fails under MSVC's perl-path discovery. Docker handles WebView2 SDK and NSIS plugin caching automatically.

---

## Bot SDK quickstart

```rust
use gipny_bot::{Bot, BotTarget};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Bot::builder()
        .data_dir("./bot-data")
        .display_name("stats-bot")
        .on_command("export", |ctx, _arg| async move {
            let today = generate_today_report().await?;
            let yesterday = generate_yesterday_report().await?;
            ctx.send_attachments_with_buttons(
                "stats ready",
                vec![
                    ("today.csv".into(),     today),
                    ("yesterday.csv".into(), yesterday),
                ],
                vec![vec![("refresh".into(), "export".into())]],
            ).await?;
            Ok(())
        })
        .on_callback(|ctx, data| async move {
            // same context as commands — full send API available
            Ok(())
        })
        .build()?
        .run().await
}
```

Full reference: [bot-sdk/docs.md](bot-sdk/docs.md).

---

## Security model in 60 seconds

| Layer            | What protects you                                                        | What still leaks                                                  |
| ---------------- | ------------------------------------------------------------------------ | ----------------------------------------------------------------- |
| Identity         | ed25519 + x25519 — no phone, email, or username; can't be doxed by ID    | If you reveal your onion address publicly, that's on you          |
| Network          | Tor v3 onion services + optional SOCKS5 outer proxy                      | Without outer proxy, ISP sees you're on Tor                       |
| Server (relay)   | sealed sender — relay never sees who's sending; only `{recipient, blob}` | Single canonical relay = SPOF for delivery (not for crypto)       |
| Content          | X3DH + Double Ratchet + XChaCha20-Poly1305 — full forward secrecy        | Compromise of device with unlocked vault reveals current session  |
| At rest          | SQLCipher (AES-256) + Argon2id KDF + duress passphrase                   | Cold-boot RAM extraction is theoretical but possible              |
| Metadata sizes   | Fixed-bucket padding (256 B → 16 MiB)                                    | Bucket choice still bins the size class                           |

### Honest caveats

- Small user base means less peer review than Signal.
- No independent crypto audit yet.
- One canonical relay; multi-relay is on the roadmap.
- No multi-device: one identity = one active client. Use export/import to migrate, then close the old client.
- Tor itself has known timing-correlation attacks at the NSA / global-passive-adversary level. Outer SOCKS5 proxy narrows that surface, doesn't close it.

### Comparison snapshot

|                                | gipny | Signal | Telegram | Element | Jabber | Tox |
| ------------------------------ | :---: | :----: | :------: | :-----: | :----: | :-: |
| E2E by default                 |   ✓   |   ✓    |    ✗     |    ±    |   ±    |  ✓  |
| Forward secrecy                |   ✓   |   ✓    |    ±     |    ✓    |   ±    |  ✓  |
| No phone number                |   ✓   |   ✗    |    ✗     |    ✓    |   ✓    |  ✓  |
| Traffic through Tor by default |   ✓   |   ✗    |    ✗     |    ✗    |   ±    |  ✗  |
| Sealed sender                  |   ✓   |   ✓    |    ✗     |    ✗    |   ✗    |  —  |
| Size-padding metadata          |   ✓   |   ±    |    ✗     |    ✗    |   ✗    |  ✗  |
| Offline delivery               |   ✓   |   ✓    |    ✓     |    ✓    |   ±    |  ✗  |
| Duress / panic passphrase      |   ✓   |   ✗    |    ✗     |    ✗    |   ✗    |  ✗  |

The in-app **Security & Anonymity** screen has the full breakdown with comparisons and how-to.

---

## Profile / data layout

```
Linux:    $XDG_DATA_HOME/gipny/profiles/<name>/      (default: ~/.local/share/gipny/...)
Windows:  %APPDATA%/gipny/profiles/<name>/
Android:  /data/user/0/app.gipny/gipny/profiles/<name>/
```

Each profile holds the SQLCipher database, Arti onion state, and `attachments/` blobs. Wipe a profile by deleting its directory.
