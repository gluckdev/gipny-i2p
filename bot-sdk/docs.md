# gipny-bot

Library for writing bots for gipny messenger. Bots are regular gipny accounts without UI — they connect to the relay, receive messages, and respond. Same E2E encryption as regular clients.

## Quick start

`Cargo.toml`:
```toml
[dependencies]
gipny-bot = { path = "../../bot-sdk" }
tokio = { version = "1", features = ["full"] }
anyhow = "1"
```

`src/main.rs`:
```rust
use gipny_bot::Bot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Bot::builder()
        .data_dir("./bot-data")
        .display_name("my-bot")
        .on_command("start", |ctx, _args| async move {
            ctx.reply("Hello!").await?;
            Ok(())
        })
        .build()?
        .run()
        .await
}
```

Run:
```bash
cargo run
```

The bot spawns the bundled i2p router (`gipny-i2p-router`; resolved from
`GIPNY_I2P_BIN`, next to the executable, or `PATH`). On first start the router
reseeds and builds tunnels (~1-3 min), then the bot prints its identity card:
```
[bot] Identity card (share with users):
  sign_pk: abc123...
  dh_pk:   def456...
```

Users add the bot as a contact in gipny client using these keys.

## API

### Builder

```rust
Bot::builder()
    .data_dir(path)          // required: where to store keys/db/i2p router state
    .display_name("name")    // optional: shown to users
    .relay("base64dest...")  // optional: override default relay (i2p destination)
    .on_message(|ctx, msg| async { ... })
    .on_command("name", |ctx, args| async { ... })
    .on_callback(|ctx, data| async { ... })
    .build()?
    .run()
    .await
```

### Handlers

**`on_message(fn)`** — any non-command message.

```rust
.on_message(|ctx, msg| async move {
    ctx.reply(format!("echo: {}", msg.body)).await?;
    Ok(())
})
```

`msg: IncomingMessage` fields:
- `body: String`
- `sender_sign_pk: Vec<u8>` — bot user's pubkey
- `sent_at: i64` — unix ms
- `attachments: Vec<WireAttachment>`
- `message_id: i64` — local ID in bot's DB

**`on_command(name, fn)`** — messages starting with `/name`. Args = text after command.

```rust
.on_command("notify", |ctx, args| async move {
    ctx.reply(format!("sending: {}", args)).await?;
    Ok(())
})
```

Invoked for `/notify hello world` → `args = "hello world"`.

**`on_callback(fn)`** — user tapped a button.

```rust
.on_callback(|ctx, data| async move {
    ctx.reply(format!("you pressed: {}", data)).await?;
    Ok(())
})
```

`data` is the `callback_data` string attached to the button.

### Context

All handlers receive `Context`:

```rust
ctx.contact_id          // i64 - user's contact id in bot's db
ctx.origin_msg_id       // Option<u64> - for callbacks: the msg_id of button source
ctx.session             // Arc<SessionManager> - low-level API
```

Methods:

```rust
ctx.reply("text")                                                 // send text
ctx.reply_with_buttons("text", buttons)                           // text + inline keyboard
ctx.send_attachment("caption", "file.pdf", bytes)                 // text + 1 file
ctx.send_attachment_with_buttons("caption", "file.pdf", bytes, buttons)  // text + 1 file + buttons
ctx.send_attachments("caption", vec![(name, bytes), ...])         // text + N files
ctx.send_attachments_with_buttons("caption", files, buttons)      // text + N files + buttons
ctx.edit(message_id, "new text")                                  // edit existing message
ctx.edit_with_buttons(message_id, "new text", buttons)            // edit + update buttons
```

### Files in callbacks

`on_callback` handler has the same `Context` as `on_message` — full send API available.
Send multiple files from a single button press:

```rust
.on_callback(|ctx, data| async move {
    if data == "export" {
        let today = generate_today_report();
        let yesterday = generate_yesterday_report();
        ctx.send_attachments_with_buttons(
            "статистика готова",
            vec![
                ("today.csv".into(), today),
                ("yesterday.csv".into(), yesterday),
            ],
            vec![vec![("обновить".into(), "export".into())]],
        ).await?;
    }
    Ok(())
})
```

File size cap is 16 MiB per message (padding bucket limit). Multiple files in one message share that limit.

### Inline keyboards

Buttons are `Vec<Vec<(text, callback_data)>>` — outer vec = rows.

```rust
ctx.reply_with_buttons("Choose:", vec![
    vec![
        ("Yes".into(), "yes".into()),
        ("No".into(), "no".into()),
    ],
    vec![
        ("Maybe".into(), "maybe".into()),
    ],
]).await?;
```

When user taps a button:
1. `on_callback` fires with `data = "yes"` (etc.)
2. `ctx.origin_msg_id` = the message_id of the message with the button (on sender side)
3. Bot can `ctx.edit(origin, "new text")` to update the message in-place

### Example: nested menu

```rust
.on_command("start", |ctx, _| async move {
    ctx.reply_with_buttons("Menu:", vec![
        vec![("Stats".into(), "stats".into())],
        vec![("Settings".into(), "settings".into())],
    ]).await?;
    Ok(())
})
.on_callback(|ctx, data| async move {
    let origin = ctx.origin_msg_id.unwrap_or(0);
    match data.as_str() {
        "stats" => {
            ctx.edit_with_buttons(origin, "Stats: 42 users",
                vec![vec![("< Back".into(), "back".into())]]).await?;
        }
        "back" => {
            ctx.edit_with_buttons(origin, "Menu:", vec![
                vec![("Stats".into(), "stats".into())],
                vec![("Settings".into(), "settings".into())],
            ]).await?;
        }
        _ => {}
    }
    Ok(())
})
```

## Deployment

### Local test
```bash
cargo run
```

### Production (systemd on VPS)

Build release binary:
```bash
cargo build --release
scp target/release/my-bot root@vps:/usr/local/bin/
```

`/etc/systemd/system/my-bot.service`:
```ini
[Unit]
Description=my gipny bot
After=network-online.target

[Service]
Type=simple
User=mybot
WorkingDirectory=/var/lib/my-bot
ExecStart=/usr/local/bin/my-bot
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
useradd -r -s /sbin/nologin mybot
mkdir -p /var/lib/my-bot
chown mybot:mybot /var/lib/my-bot
systemctl enable --now my-bot
journalctl -u my-bot -f
```

Bot's identity is stored in `/var/lib/my-bot/bot.db`. Back this up — losing it = new identity = users must re-add the bot.

## Data layout

```
data-dir/
├── bot.db           — SQLite: contacts, sessions, prekeys, messages
├── i2p/             — i2p router state (the network address itself is
│                      ephemeral: regenerated every session, never stored)
└── attachments/     — encrypted attachment blobs
```

## Architecture notes

- Bot uses the same relay as regular gipny clients (default is hardcoded)
- E2E encryption with Double Ratchet, same as regular chats — relay never sees content
- Bot can be reached both when online and offline; messages queue on the relay and deliver when bot reconnects
- No built-in rate limiting — if you need it, implement per-contact_id tracking in your handler
- Handlers run in tokio tasks — spawn background work freely, but don't block a handler waiting on something that requires another message

## Limitations

- No groups support for bots yet (bots only handle DMs)
- No rich message types (only text + attachments + buttons)
- Callback data size should be kept reasonable (< 1KB)
- Edits only work on messages the bot itself sent; you can't edit user messages