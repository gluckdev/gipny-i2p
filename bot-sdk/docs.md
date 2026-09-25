# gipny-bot

Библиотека для написания ботов для мессенджера gipny. Бот — обычный аккаунт
gipny без интерфейса: подключается к релею, получает сообщения и отвечает. То же
сквозное шифрование, что у обычных клиентов.

Чем бот отличается от агента (`gipny-agent`): бот говорит **с кем угодно** и
делает **только то, что вы написали в обработчиках**, а агент подчиняется
**одному мастеру** и выполняет **произвольные команды оболочки**. Сравнение — в
корневом `README.md`, раздел «Агент и бот — в чём разница».

## Быстрый старт

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
        .relay("<i2p destination релея>")
        .on_command("start", |ctx, _args| async move {
            ctx.reply("Привет!").await?;
            Ok(())
        })
        .build()?
        .run()
        .await
}
```

Запуск:
```bash
cargo run
```

Бот запускает роутер i2p внутри своего процесса (libi2pd вкомпилирован через
`libcore`, отдельный `i2pd` не нужен, портов нет; данные — в
`<data_dir>/i2p/router`). При первом
запуске роутер делает reseed и строит туннели — это 1–3 минуты, — после чего бот
печатает свою карточку:
```
[bot] Identity card (share with users):
  sign_pk: abc123...
  dh_pk:   def456...
```

Пользователи добавляют бота в контакты по этим ключам.

**Релей боту нужно указать.** Встроенного релея в процессе, как у приложения и у
агента, в SDK нет: `relay(...)` задаёт адрес внешнего релея, с которого бот
забирает почту. Поднять свой можно из архива `gipny-relay` со страницы релиза
(или `cargo build --release -p gipny-relay`). Пример личного релея внутри
процесса есть в `tests/e2e-harness` (`start_in_process_relays`) — если он вам
нужен, повторите этот приём у себя.

## API

### Builder

```rust
Bot::builder()
    .data_dir(path)          // обязательно: где хранить ключи, базу и состояние роутера
    .display_name("name")    // необязательно: имя, которое видят пользователи
    .relay("base64dest...")  // адрес релея (i2p destination)
    .vault_passphrase("…")   // необязательно: пароль для шифрования базы бота
    .on_message(|ctx, msg| async { ... })
    .on_command("name", |ctx, args| async { ... })
    .on_callback(|ctx, data| async { ... })
    .build()?
    .run()
    .await
```

### Обработчики

**`on_message(fn)`** — любое сообщение, которое не является командой.

```rust
.on_message(|ctx, msg| async move {
    ctx.reply(format!("эхо: {}", msg.body)).await?;
    Ok(())
})
```

Поля `msg: IncomingMessage`:
- `body: String`;
- `sender_sign_pk: Vec<u8>` — открытый ключ пользователя;
- `sent_at: i64` — unix-время в миллисекундах;
- `attachments: Vec<WireAttachment>`;
- `message_id: i64` — локальный идентификатор в базе бота.

**`on_command(name, fn)`** — сообщения, начинающиеся с `/name`. Аргументы — всё,
что идёт после команды.

```rust
.on_command("notify", |ctx, args| async move {
    ctx.reply(format!("отправляю: {}", args)).await?;
    Ok(())
})
```

Для `/notify привет мир` обработчик получит `args = "привет мир"`.

**`on_callback(fn)`** — пользователь нажал кнопку.

```rust
.on_callback(|ctx, data| async move {
    ctx.reply(format!("вы нажали: {}", data)).await?;
    Ok(())
})
```

`data` — строка `callback_data`, привязанная к кнопке.

### Контекст

Каждый обработчик получает `Context`:

```rust
ctx.contact_id          // i64 — id пользователя в базе бота
ctx.origin_msg_id       // Option<u64> — для коллбеков: id сообщения с кнопкой
ctx.session             // Arc<SessionManager> — низкоуровневый API
```

Методы:

```rust
ctx.reply("текст")                                                // отправить текст
ctx.reply_with_buttons("текст", buttons)                          // текст + инлайн-клавиатура
ctx.send_attachment("подпись", "file.pdf", bytes)                 // текст + один файл
ctx.send_attachment_with_buttons("подпись", "file.pdf", bytes, buttons)
ctx.send_attachments("подпись", vec![(name, bytes), ...])         // текст + N файлов
ctx.send_attachments_with_buttons("подпись", files, buttons)
ctx.edit(message_id, "новый текст")                               // правка своего сообщения
ctx.edit_with_buttons(message_id, "новый текст", buttons)         // правка вместе с кнопками
```

### Файлы в коллбеках

У обработчика `on_callback` тот же `Context`, что у `on_message`, — доступен весь
API отправки. Несколько файлов по одному нажатию:

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

Ограничение на размер — 16 МиБ на сообщение (предел корзины паддинга).
Несколько файлов в одном сообщении делят этот предел между собой.

### Инлайн-клавиатуры

Кнопки — это `Vec<Vec<(текст, callback_data)>>`: внешний вектор — ряды.

```rust
ctx.reply_with_buttons("Выберите:", vec![
    vec![
        ("Да".into(), "yes".into()),
        ("Нет".into(), "no".into()),
    ],
    vec![
        ("Может быть".into(), "maybe".into()),
    ],
]).await?;
```

Когда пользователь нажимает кнопку:
1. срабатывает `on_callback` с `data = "yes"` (и так далее);
2. `ctx.origin_msg_id` — id сообщения с кнопкой на стороне отправителя;
3. бот может вызвать `ctx.edit(origin, "новый текст")` и обновить сообщение на
   месте.

### Пример: вложенное меню

```rust
.on_command("start", |ctx, _| async move {
    ctx.reply_with_buttons("Меню:", vec![
        vec![("Статистика".into(), "stats".into())],
        vec![("Настройки".into(), "settings".into())],
    ]).await?;
    Ok(())
})
.on_callback(|ctx, data| async move {
    let origin = ctx.origin_msg_id.unwrap_or(0);
    match data.as_str() {
        "stats" => {
            ctx.edit_with_buttons(origin, "Статистика: 42 пользователя",
                vec![vec![("< Назад".into(), "back".into())]]).await?;
        }
        "back" => {
            ctx.edit_with_buttons(origin, "Меню:", vec![
                vec![("Статистика".into(), "stats".into())],
                vec![("Настройки".into(), "settings".into())],
            ]).await?;
        }
        _ => {}
    }
    Ok(())
})
```

## Развёртывание

### Локальная проверка
```bash
cargo run
```

### На сервере (systemd на VPS)

Сборка релизного бинаря:
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

Личность бота лежит в `/var/lib/my-bot/bot.db`. Делайте резервную копию: потеря
базы означает новую личность, и пользователям придётся добавлять бота заново.

## Что лежит в каталоге данных

```
data-dir/
├── bot.db           — SQLite: контакты, сессии, prekey-и, сообщения
├── i2p/             — состояние роутера i2p (сам сетевой адрес эфемерный:
│                      создаётся заново каждую сессию и не сохраняется)
└── attachments/     — зашифрованные блобы вложений
```

## Как это устроено

- Бот забирает почту с релея, адрес которого задан через `relay(...)`.
- Сквозное шифрование Double Ratchet, как в обычных чатах: содержимое релею не
  видно.
- Пока бот выключен, сообщения ждут на релее и доставляются при подключении —
  если релей внешний и работает постоянно.
- Ограничения частоты запросов нет: если нужно, ведите учёт по `contact_id`
  в своём обработчике.
- Обработчики запускаются в задачах tokio: фоновую работу порождать можно
  свободно, но не блокируйте обработчик ожиданием того, что придёт следующим
  сообщением.

## Ограничения

- Групп у ботов пока нет: только личные чаты.
- Богатых типов сообщений нет: текст, вложения, кнопки.
- Размер `callback_data` стоит держать разумным (меньше 1 КБ).
- Правки работают только для сообщений, отправленных самим ботом; сообщения
  пользователя править нельзя.
- Запросов в контакты у ботов нет: бот принимает всех, кто ему написал. Если
  нужен отбор, ведите свой список разрешённых `sender_sign_pk`.
