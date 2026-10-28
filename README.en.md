# gipny-i2p

[Русская документация](README.md) · [Downloads](https://github.com/gluckdev/gipny-i2p/releases) · [Security](SECURITY.md) · [Development documentation](docs/README.md)

gipny-i2p is a desktop and Android messenger with end-to-end encryption over I2P.
You exchange contact cards instead of registering a phone number, email address
or server account. Identity keys are created on your device.

This is an independent I2P fork of [ri-kont/gipny](https://github.com/ri-kont/gipny).
The original project used Tor/Arti. This fork replaces the transport with i2pd,
embeds the router in the application process, and develops relay discovery,
offline delivery and I2P reliability. The inherited encrypted vault, X3DH and
Double Ratchet remain central to its design. See the
[migration history](MIGRATION-i2p.md) for the reasons and implementation history.

## Getting started

1. Download the installer for your platform from the
   [releases page](https://github.com/gluckdev/gipny-i2p/releases).
2. Create a local profile and choose a passphrase. No registration server is involved.
3. Wait for the embedded router and relay to become ready. A first startup can take minutes.
4. Open **My card** and share your contact card or QR code through a channel you trust.
5. Choose **Add contact**, paste or scan the other person's card, and accept their request.
6. Compare key fingerprints in person or through another authenticated channel before marking a contact verified.

The built-in relay needs no separate server installation. Its address changes
when the app restarts; established contacts learn updated addresses through the
protocol. Give new contacts a fresh card after restarting.

## English and Russian

The working tree supports both English and Russian throughout profile setup,
chats, contacts, groups, QR scanning, security settings, backups, updates,
notifications and the agent console. This translation is unreleased until it is
included in a tagged build; an older download may still have a partial translation.

Choose **English** or **Русский** before unlocking, or use **Settings → Language**.
An explicit choice is kept locally on this device. Without a saved preference,
Russian, Belarusian and Ukrainian browser locales retain the existing Russian
default; other locales use English. Changing language does not restart the app
or lock the profile. Unsent messages, attached file references, replies and
inline edits stay in memory during the change.

Names, message contents, filenames, commands, command output and downloaded
release notes retain their original text. Known native attachment and installer
errors have English explanations; unknown technical diagnostics remain intact.
The operating system supplies its own permission and file-picker dialogs.

## Features

- Direct conversations and groups; contact requests, verification and blocking.
- Files and images, attachment gallery, clipboard images and chunked transfers
  with progress and cancellation.
- Replies, edits, forwarding, pinned messages and chats; encrypted local search.
- Typing and delivery indicators, notifications and per-chat muting.
- Disappearing messages, profile locking, encrypted backup and restore.
- Local avatars, contact folders, light/dark/system themes.
- Optional agent mode: a trusted master can run commands through a chat.
  This grants real access as your operating-system user; enable it deliberately.
- A separate headless [agent](agent/README.md) and Rust [bot SDK](bot-sdk/README.md).

## Transport and delivery

The native application embeds `libi2pd` through the `i2p-embed` crate, with one
router per process. It does not expose a local SAM or HTTP proxy port. The
router itself still participates in I2P using its normal network transports.

Messages are encrypted on the sender's device before reaching a relay. Each
client can run its own ephemeral relay. If the recipient's relay is unavailable,
the relay network provides an offline fallback using encrypted records and
Kademlia discovery. Delivery depends on nodes retaining those records until
the recipient returns; it is not guaranteed storage. The sender retries until
acknowledgement or expiry, for up to seven days.

The normal profile uses message-size padding and multi-hop tunnels. Faster
settings shorten tunnels and remove padding, changing privacy properties.
Latency improvements obtained this way are a tradeoff, not an anonymity guarantee.

The existing `gipny-web` uses a server-side gateway. It is different from the
planned static browser client whose cryptography, storage and messaging run on
the user's device. Publishing static files to GitHub Pages does not implement
a direct browser-to-I2P transport. Distributed temporary message shares are
also a research direction, not an existing delivery mode.

## Cryptography and local data

| Purpose | Current design |
|---|---|
| Identity authentication | Ed25519 |
| Key agreement | X25519 and X3DH |
| Session encryption | Double Ratchet, XChaCha20-Poly1305 |
| Key derivation | HKDF-SHA256; Argon2id for vault passphrases |
| Native client's vault | SQLCipher, including local search |
| Backup encryption | XChaCha20-Poly1305 |
| Network | I2P, with in-process i2pd |

These descriptions apply to the native client's vault. Headless agents and bots
have different storage properties; do not assume their databases are encrypted
like the desktop profile.

There has been no independent code or cryptography audit. I2P cannot guarantee
protection against a global traffic observer or a compromised endpoint. A
recipient can retain copies or screenshots even after a disappearing-message
timer or deletion request. Secure deletion cannot guarantee removal from SSD
remapping, backups or a compromised operating system.

Updates use an I2P outproxy and TLS to reach GitHub. A checksum published through
the same release channel is not an independent release signature. See
[security documentation](SECURITY.md) and the Russian README's threat-model
section for further limits.

## Backups and another device

Export a backup from **Settings → Backup**, using a separate passphrase of at
least eight characters. Import it from the profile selection screen on the new
device. Close gipny on the old device after importing: **one identity currently
supports one active device**. Concurrent use can make ratchet sessions diverge.
A lost backup passphrase cannot be recovered.

## Development

The UI is vanilla TypeScript with Vite and Tauri 2. The application and protocol
components are Rust; the embedded router includes C++. Native installation
artifacts are built by GitHub Actions. UI-only checks do not build the router:

```bash
cd ui
npm ci
npm run build
npm test
```

Use a Node version supported by the pinned Vite dependency (Node 20.19+ or
22.12+). The DOM regression tests need no native application, I2P router,
account or browser binary. They do not establish that native packaging, device
permissions or live I2P delivery work.

For visual inspection, run `npm run dev` from `ui/` and open
`/dev/preview.html?scene=chat&lang=en` or the same URL with `lang=ru`.
The preview offers phone, tablet and desktop widths, themes and mocked scenes.
It uses synthetic data, not your profile.

Read the [UI map](ui/README.md), [localization guide](docs/localization.md),
[crate map and native build guidance](README.md), and
[dated handoff notes](docs/handoff-brief.md) before changing the relevant component.
Historical documents retain their original context and are labelled accordingly.

Documentation must stay current with code changes. Update affected user
instructions, both locale dictionaries, security explanations, checks and the
project blob together. Restore broken links to the correct source instead of
removing them. Preserve upstream attribution and the GPL-3.0 license.
