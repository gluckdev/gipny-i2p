# Режим агента: консоль в чате + отдельный бинарь gipny-agent

## Context

Мессенджер — админский. Фишка релиза 0.4.0: любой клиент можно переключить в **режим
агента**, назначив мастера. У мастера в чате с этим контактом появляется переключатель
**чат / консоль**; в консоли фон чёрный, всё набранное уходит как команда и выполняется
на той стороне, вывод приходит обратно. Выключить режим может и мастер (кнопка в консоли),
и сам агент (настройки/баннер). Отдельно — **headless-бинарь `gipny-agent`** без UI:
только команды от одного мастера, заданного при установке, ставится однострочником на
Linux, macOS и Windows.

Ответы владельца, зафиксированные в дизайне: команда помечается на уровне протокола
(мастер в режиме «консоль» → сообщение-команда), а не «любой текст = команда»; все три
платформы для бинаря; служба от root, если ставили через `sudo`; команды, пришедшие,
пока режим был выключен, **выполняются** после включения.

Транспорт не меняется: команды и вывод — обычные сообщения Double Ratchet через релей,
с одним новым полем. Аутентификация мастера — криптографическая, по сессии ratchet с
контактом (`identity_sign`), а не по имени.

### Что уже есть и переиспользуется

- **Расширение сообщения без миграции**: дополнительные данные сообщения живут в
  `settings` как `buttons_<id>` / `sound_<id>` (`libcore/src/session.rs:585-591`,
  `build_payload_from_db` :1543-1548, триггер очистки `tr_buttons_msg_del`
  `libcore/src/db.rs:439-443`, `attach_buttons` `core/src/lib.rs:254`). Флаг консоли
  ляжет туда же: `console_<id>`.
- **Версионирование `WirePayload`**: новое поле только в конец; `encode_payload`
  выбирает самый короткий вариант, `decode_payload` пробует от нового к старому
  (`session.rs:385-403`); bincode 1 терпит хвостовые байты → старые клиенты новое поле
  просто не видят.
- **Знакомство**: `X3dhInit` от неизвестного отправителя **автоматически создаёт
  контакт** (`session.rs:1029-1036`, `core/src/core.rs:1145`), имя берётся из
  `sender_name` (`apply_peer_name`). Неподтверждённые сообщения пересылаются до 8 раз с
  бэкоффом (`db.rs:870`, `session.rs:1392`). Поэтому агент пишет мастеру **первым**, и
  у мастера сам появляется контакт с консолью — вставлять карточку руками не нужно.
- **Headless-клиент**: `tests/e2e-harness/src/main.rs:56-108` `start_bot` —
  `TorNode::start` + `Db::open_plain` (без пароля) + `SessionManager::start` + цикл
  `SessionEvent`. Это шаблон для `gipny-agent` (bot-sdk не берём: у него ретраи
  обработчика с dead-letter, что для shell-команд означало бы повторный запуск, и
  перехват `/`-префикса).
- **Упаковка**: джоба `relay` в `.github/workflows/release.yml:261-302`, юнит
  `core/relay/gipny-relay.service`, строгая таблица `ROWS` в
  `.github/scripts/release-notes.sh:25-37`.
- **UI-паттерны**: баннер `.relay-banner` (`ui/src/app.ts:213-228`, сигналы
  `state.ts:690-714`, CSS `styles.css:1416-1429`); секция настроек с радио
  (`ui/src/settings.ts:170-260`); `.msg-body` уже `white-space: pre-wrap`
  (`styles.css:781`); выбор контакта — `<select>` как в `group.ts:18-32`.

---

## 1. Протокол и общая логика (libcore)

**`libcore/src/session.rs`** — поле в конец `WirePayload` (:68-94):

```rust
#[serde(default)] pub console: Option<WireConsole>,

pub struct WireConsole {
    pub kind: u8,              // CONSOLE_COMMAND=0, OUTPUT=1, GRANT=2, REVOKE=3, OFF=4
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub truncated: bool,
}
```
`u8`, не enum: незнакомый kind игнорируется, а не ломает декодирование. Механика:
текущий `WirePayload` становится `WireV7` (+ `From` в обе стороны, как V6);
`encode_payload` — первая ветка `p.console.is_some() → WirePayload`, затем
`notify_sound → WireV7`, остальное сдвигается; `decode_payload` — `WirePayload`, потом
`WireV7`, дальше как было. Тест: V6-байты декодируются в `console: None`; payload с
`console` после `encode→decode` равен исходному; старый `WireV7` декодирует новый
payload (хвост игнорируется).

Тела управляющих сообщений — короткие ASCII-маркеры (`[agent on]`, `[agent off]`,
`[agent stop]`): старый клиент без поля покажет их как текст, новый рисует по `kind`.

**`libcore/src/agent.rs`** (новый модуль, `pub mod agent` в lib.rs):
- `pub struct ExecOptions { timeout: Duration /*120 с*/, max_output: usize /*60 000 байт — влезает в корзину 64 КиБ PADDING_BUCKETS*/ }`
- `pub async fn run_command(cmd: &str, o: &ExecOptions) -> ExecResult { output: String, exit_code: Option<i32>, duration_ms: u64, truncated: bool }` — `tokio::process::Command`: unix `sh -c`, Windows
  `powershell -NoProfile -NonInteractive -Command` с `[Console]::OutputEncoding=UTF8`
  первой строкой (иначе кириллица из `cmd` приходит в cp866); stdout+stderr в одну ленту
  (`Stdio::piped`, читать параллельно); `kill_on_drop(true)` + `tokio::time::timeout`
  → при таймауте kill и пометка `[timeout 120s]`; обрезка: голова 40 000 + хвост
  20 000 байт с маркером `[… N байт пропущено …]`, резать по границе UTF-8.
- `pub fn parse_control(body: &str) -> Option<Control>` — только буквальный первый токен
  `/agent`: `Control::Get(path)`. Всё остальное, включая `/usr/bin/x`, — команда.
- `pub fn read_file_for_send(path, max: 8 MiB) -> Result<(String /*name*/, Vec<u8>)>`
  для `/agent get <путь>` — ответ уходит вложением через существующие
  `send_message(..., attachments, ...)`.
- `pub const SETTING_AGENT_MASTER: &str = "agent_master"` (32 байта sign_pk мастера;
  наличие ключа = режим включён).
- Аудит: каждая выполненная команда — `eprintln!("[agent] exec from <sign_pk hex8>: <cmd>")`
  и результат `[agent] exit=N dur=Nms`. У standalone это journald/лог задачи.

**`libcore/src/card.rs`** (новый): Rust-зеркало `ui/src/api.ts:400-463` —
`ContactCard { onion, sign_pk, dh_pk, relay: Option<String>, name: Option<String> }`,
`parse(&str)` (v1 и v2, те же регулярки/проверки, 64-hex ключи, `isValidI2pAddress`),
`encode()` (v2 если есть релей). Нужно агенту: разобрать карточку мастера при установке и
напечатать свою. Тесты: round-trip + две строки-фикстуры, снятые с TS-реализации.

**`libcore/src/db.rs`**:
- `ensure_column(contacts, "agent_granted", "INTEGER NOT NULL DEFAULT 0")` рядом с
  :457-463; в `select_contact!` (:215), `Contact` (:59-75), `map`; `set_contact_agent_granted(id, bool) -> bool`.
- Триггер `tr_console_msg_del` (новый, рядом с :439): удаляет `console_<id>` и
  `console_pending_<id>`.
- `list_setting_keys_with_prefix(prefix) -> Vec<String>` — для очереди отложенных команд.

**Оба конвейера** — `SessionManager` и `Core` (правки симметричные, иначе бот и приложение
разъедутся):
- `send_console(contact_id, body, console: WireConsole, attachments) -> i64`: как
  `send_message` (`session.rs:563-594`, `core.rs:355-380`) + `set_setting(console_<id>,
  bincode(console))`, kick.
- `build_payload_from_db` (`session.rs:1533`, `core.rs:1808`): читать `console_<id>` →
  `p.console`.
- `persist_incoming` (`session.rs:1153`, `core.rs:1262`): консольные сообщения идут по
  обычному пути сохранения (`insert_message_with_origin` + ack), после вставки
  `set_setting(console_<mid>)`, затем побочные действия по `kind` (раздел 2). Убедиться,
  что путь сохранения не отбрасывает пустое/маркерное тело.

## 2. Режим агента в приложении (`core/src/core.rs`, `core/src/lib.rs`)

**Состояние**: `settings[agent_master] = sign_pk`. `Core` получает `agent_tx:
mpsc::UnboundedSender<i64>` (id входящего сообщения-команды) и воркер, запущенный в
`Core::start` по образцу `spawn_purge_loop` (:1826): выполняет **строго по одной** в
порядке очереди — `run_command` → `send_console(master, output, Output{exit,dur,trunc})`
→ `delete_setting(console_pending_<id>)`. `/agent get` → `send_console` с вложением.

**Приём** (`persist_incoming`, после сохранения строки):
- `COMMAND`: `set_setting(console_pending_<mid>)`; если режим включён **и**
  `contact.identity_sign == master` → `agent_tx.send(mid)`. Иначе строка остаётся
  отложенной (выбор владельца: выполнить после включения).
- `OFF` от мастера при включённом режиме → `disable_agent_mode(reason=remote)`.
- `GRANT` / `REVOKE` → `set_contact_agent_granted(cid, true/false)`, `ContactUpdated`.
- `OUTPUT` → просто сохранено; UI перерисует.
- В `CoreEvent::IncomingMessage` добавить `console_kind: Option<u8>` — UI по нему
  решает про звук/уведомление.

**Включение** `set_agent_mode(Some(cid))`: контакт существует, не заблокирован, не
группа → `set_setting(agent_master)`; `send_console(cid, "[agent on]", GRANT)`;
собрать все `console_pending_*` этого контакта по возрастанию id → в `agent_tx`;
`CoreEvent::AgentModeChanged{master: Some{contact_id, name}}`.
**Выключение** (локально или по OFF): `delete_setting`; `send_console(master,
"[agent off]", REVOKE)`; событие с `None`. Удаление/блокировка контакта-мастера
(`delete_contact`, `update_contact` с trust=Blocked) → автоматическое выключение.
На старте `Core::start` ничего не шлёт — состояние читается UI через `get_agent_mode`.

**Мастерская сторона**: `send_console_command(cid, body) -> i64` — COMMAND (сначала
`parse_control` не нужен — сторона агента разберёт); `send_agent_off(cid)` — OFF.

**Tauri-команды** (`lib.rs:108-142`, стиль `set_relay_address` :596-599):
`get_agent_mode -> Option<AgentMasterDto{contact_id, name, sign_pk}>`,
`set_agent_mode(contact_id: Option<i64>)`, `send_console_command(contact_id, body)`,
`send_agent_off(contact_id)`. `MessageDto` (:238-253) + `console: Option<ConsoleDto{kind,
exit_code, duration_ms, truncated}>` через `attach_console` по образцу `attach_buttons`;
`ContactDto` + `agent_granted`.

## 3. UI (`ui/src/*.ts`, `ui/src/styles.css`)

- **`api.ts`**: типы `Message.console`, `Contact.agent_granted`, `CoreEvent`
  `AgentModeChanged` и `console_kind` в `IncomingMessage`; обёртки четырёх команд.
- **`state.ts`**: сигнал `agentMode: Signal<AgentMaster|null>` (загрузка при старте,
  обновление по событию :688+); для `console_kind != null` не играть звук и не слать
  OS-уведомление, unread считать как обычно; `ContactUpdated` уже перезагружает контакты.
- **`app.ts`**: второй баннер по образцу `.relay-banner` (:213-228) — «РЕЖИМ АГЕНТА ·
  мастер: <имя> · [выключить]», виден при `agentMode != null`; кнопка →
  `set_agent_mode(null)`.
- **`settings.ts`**: секция «режим агента» после релея (:119-142): `<select>` из
  `store.contacts` без групп и заблокированных (как `group.ts:18-32`) + кнопка
  «включить»/«выключить», пояснение: «мастер сможет выполнять на этом устройстве любые
  команды от вашего имени».
- **`chat.ts`**: режим `console` внутри `Chat` (класс `console` на `.chat`), а не
  отдельный view — переиспользует лог, скролл, композер:
  - переключатель «чат | консоль» в `.chat-header` (:152-160) только если
    `contact.agent_granted`; рядом кнопка «выключить агента» → `send_agent_off`;
    выбор помнить в `store` на время сессии (по умолчанию «чат»);
  - фильтр `renderLog`: в консоли — только сообщения с `console`; в чате — без
    COMMAND/OUTPUT **если** у контакта есть консоль (иначе показывать их inline мono
    блоками с `$`, чтобы на стороне агента было видно, что выполнялось); GRANT/REVOKE/OFF
    везде — системная строка через `.divider-text` («вам открыта консоль», «консоль
    закрыта»);
  - рендер: COMMAND → `$ <body>`, OUTPUT → тело + приглушённый трейлер `exit N · 1.2s
    [обрезано]`; композер с приглашением `$`, Enter → `send_console_command`,
    Shift+Enter — перенос; вложения из OUTPUT (ответ на `/agent get`) — существующий
    `loadAttachmentsFor`.
  - CSS: `.chat.console` переопределяет локальные токены — фон `#000`, текст `#d8d8d8`,
    команды акцентным зелёным, без пузырей и аватаров; работает поверх обеих тем.
- Android: ничего дополнительного — ядро в том же процессе, `sh` есть, прав root нет;
  тумблер доступен, в пояснении отметить «на Android — в песочнице приложения».

## 4. Бинарь `agent/` (пакет `gipny-agent`, member workspace)

Зависимости: `gipny-libcore`, tokio, anyhow, libc (для `gethostname`); без clap —
три флага разбираются вручную.

- CLI: `gipny-agent --master <карточка v2> [--data-dir DIR] [--name NAME]`. Карточка
  сохраняется в `<data>/master.card` (0600), потом можно запускать без флага. v1-карточка
  (без релея) — ошибка с подсказкой: агент собирает с релея мастера, других настроек нет.
  Данные: `GIPNY_AGENT_DATA` или `--data-dir`; `GIPNY_SAM_PORT` — как в e2e,
  подключиться к чужому роутеру.
- Старт (по `tests/e2e-harness/src/main.rs:56-108`): `TorNode::start(data,
  Default::default())` → `Db::open_plain(<data>/agent.db)` → `SessionManager::start` →
  `set_relay_onion(card.relay)`, `set_display_name(name | hostname)` →
  `add_contact_via(&IdentityCard{sign_pk, dh_pk}, &card.onion, "master", Some(relay))`
  (идемпотентно: `INSERT OR IGNORE`) → записать свою v2-карточку в `<data>/card.txt` и
  напечатать → `send_console(master, "[agent on]", GRANT)` (доставится, как только
  мастер онлайн; у мастера контакт создаётся сам).
- Цикл событий: `IncomingPayload{contact_id, payload}` → отбросить всё, где
  `contact.identity_sign != master` или `payload.group.is_some()` или `console.is_none()`;
  COMMAND → в очередь (`mpsc`, один воркер, по порядку) → `run_command` → `send_console(
  OUTPUT)`; OFF → `send_console(REVOKE)`, дождаться kick, `std::process::exit(0)` — с
  `Restart=on-failure` служба остаётся остановленной; `/agent get` → вложение.
- Никакого «режима чата» и настроек — только это.

## 5. Установщики (в корне репозитория, тянутся raw с main; копия кладётся в архив)

**`install-agent.sh`** (Linux + macOS), `set -euo pipefail`, идемпотентен (повторный
запуск = обновление на месте):
```
curl -fsSL https://raw.githubusercontent.com/gluckdev/gipny-i2p/main/install-agent.sh | sudo bash -s -- 'gipny:v2:…'
```
`uname -s/-m` → `linux|macos` × `amd64|arm64|x86_64`; последний тег через
`https://api.github.com/repos/gluckdev/gipny-i2p/releases/latest` (`sed`, без jq);
`releases/download/<tag>/gipny-agent_<ver>_<os>-<arch>.tar.gz` + `SHA256SUMS.txt`,
проверка `sha256sum -c` / `shasum -a 256`; root (`EUID=0`) → `/usr/local/bin`,
данные `/var/lib/gipny-agent` (Linux) или `/Library/Application Support/gipny-agent`
(macOS), служба **от root**, `User=` переопределяется `GIPNY_AGENT_USER`; без root →
`~/.local/bin`, `~/.local/share/gipny-agent`, `systemctl --user` + `loginctl
enable-linger`, на macOS `~/Library/LaunchAgents`. Linux: юнит `agent/gipny-agent.service`
(по образцу relay, без `ProtectSystem`/`ProtectHome` — агент должен видеть систему,
оставить `PrivateTmp=false`, `Restart=on-failure`). macOS: `agent/app.gipny.agent.plist`,
`launchctl bootstrap system|gui/$UID`. После старта ждёт `<data>/card.txt` (до 3 мин) и
печатает: «Агент запущен. В приложении мастера появится контакт "<name>" с консолью».

**`install-agent.ps1`** (Windows, от администратора):
`irm https://raw.githubusercontent.com/gluckdev/gipny-i2p/main/install-agent.ps1 | iex` с
`$env:GIPNY_MASTER='gipny:v2:…'` (или параметр `-Master`); zip в `%ProgramFiles%\gipny-agent`,
данные `%ProgramData%\gipny-agent`, `Get-FileHash` против `SHA256SUMS.txt`; автозапуск —
задача планировщика `schtasks /Create /TN gipny-agent /SC ONSTART /RU SYSTEM /RL HIGHEST`
(бинарь не реализует SCM, поэтому не служба) + немедленный `/Run`.

## 6. CI и релиз

- **`.github/workflows/release.yml`**: джоба `agent` (`needs: router`) с матрицей пяти
  ног — `linux/amd64` (ubuntu-22.04, `router-linux-amd64`), `linux/arm64`
  (ubuntu-22.04-arm), `macos/arm64` (macos-15), `macos/x86_64` (macos-15-intel),
  `windows/amd64` (windows-latest, `i2pd.exe`); шаги как у `relay` (:274-302), но
  `cargo build --release -p gipny-agent` в основном workspace; macOS — `codesign -s -`;
  Windows — zip. Имена: `gipny-agent_<ver>_{linux-amd64,linux-arm64,macos-arm64,
  macos-x86_64}.tar.gz`, `gipny-agent_<ver>_windows-amd64.zip`; upload как
  `gipny-agent-<os>-<arch>` (паттерн `gipny-*` подхватит publish). `publish.needs` +
  `agent`.
- **`.github/scripts/release-notes.sh`**: пять строк `ROWS` «Агент · …» после релея;
  в «Установка» — абзац с однострочниками.
- **`.github/workflows/build.yml`**: `-p gipny-agent` в строку :60; в `macos` —
  `cargo build --release -p gipny-agent`.
- **E2E** `tests/e2e-harness` + `.github/workflows/e2e-i2pd.yml`: режим
  `E2E_AGENT_BIN=<путь>` — вместо bot-b запускается **настоящий** `gipny-agent` с
  `--master <v2-карточка A>` (кодек из `libcore::card`, релей A из in-process режима) и
  унаследованным `GIPNY_SAM_PORT`; A ждёт `IncomingPayload` с GRANT (контакт создаётся
  сам), шлёт N COMMAND `echo hello-<i>`, ждёт N OUTPUT с `hello-<i>` и `exit_code==0`,
  затем OFF и проверяет, что процесс завершился с 0. Третья джоба «e2e (agent binary)»
  копирует «relays inside the bots» (:375-427) + `cargo build -p gipny-agent`.
- **Документы**: `docs/releases/0.4.0.md` — пункт «Режим агента» первым; README —
  раздел «Режим агента и gipny-agent» (однострочники, что именно даётся мастеру, как
  снять).

## 7. Безопасность (встроено в дизайн, не дополнительное трение)

- Мастер = `identity_sign` контакта; сообщение приписывается контакту только после
  расшифровки его ratchet-сессией; `sender_name` — лишь подпись.
- Выполняются только COMMAND, только из личного чата, только от мастера, только при
  включённом режиме (отложенные — после включения, по решению владельца).
- Replay/подмена отсекаются ratchet (`ratchet_rejects_tampering_and_replay`); дубликат
  по `origin_msg_id` не создаёт вторую строку.
- Режим выключен по умолчанию, включается явно, виден баннером; автоснятие при удалении
  или блокировке мастера; мастер может снять удалённо.
- Таймаут 120 с с kill, потолок вывода, строго последовательное выполнение; каждый запуск
  в логе. У бинаря: `master.card` 0600, данные 0700, root только через `sudo`.

## 8. Порядок работ

1. libcore: `WireConsole` + V7-сдвиг + тесты; `card.rs` + тесты; `agent.rs` + тесты
   (unix: exit code, таймаут `sleep 5` при 1 с, обрезка, кириллица); DB-колонка, триггер,
   префиксный список ключей.
2. `SessionManager`: `send_console`, `build_payload_from_db`, `persist_incoming`.
3. `Core` (зеркально) + воркер + `set/get_agent_mode` + события + Tauri-команды + DTO.
4. UI: api/state/app-баннер/настройки/консоль в `chat.ts`/CSS.
5. Крейт `agent/` + юнит + plist + оба установщика.
6. CI: build.yml, e2e-режим и джоба, release.yml, release-notes.sh, docs.

## Verification (только GitHub Actions)

- `build.yml`: workspace + `gipny-agent` собираются на Linux и macOS; тесты libcore
  зелёные (wire V7, card, agent runner).
- `e2e-i2pd` (dispatch через браузер): три джобы зелёные, новая — «e2e (agent binary)»
  печатает N/N выполненных команд и время ответа.
- Пробный `release` (dispatch): пять артефактов агента, страница релиза со строками
  «Агент · …» и однострочниками, `SHA256SUMS.txt` их включает.
- Ручная проверка: установить `gipny-agent` на VPS однострочником с карточкой своего
  приложения → в приложении появляется контакт с переключателем → консоль: `uptime`,
  `ls -la /`, `/agent get /etc/os-release`, «выключить агента» → служба остановлена.
  Второе приложение (ноутбук): Настройки → режим агента → мастер; с телефона/десктопа
  команды выполняются; выключить локально с баннера и удалённо кнопкой.
