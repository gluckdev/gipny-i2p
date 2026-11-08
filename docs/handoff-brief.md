# Передача контекста / Project handoff

Дата / Date: 2026-11-08 (запись о браузерном клиенте; бриф rc3 от 2026-11-06
ниже без изменений / browser-client entry; the rc3 brief of 2026-11-06 follows
unchanged). Версия / Version: `0.5.0-rc3` (ревью и исправления поверх рабочей
копии Codex), базовый / baseline HEAD `a6e41caa07ffff94a834f72a4c13acd73f188224`.

## Русский

### 2026-11-08: публичный браузерный клиент не выходит в сеть

- **Сообщение:** владелец с iPhone (Safari): «WASM-клиент не запускается».
- **Диагноз:** воспроизведено без Safari — headless Chrome на собранном клиенте.
  У https://gluckdev.github.io/gipny-i2p/app/browser.html нет `browser-config.json`
  (HTTP 404): `pages.yml` пишет его из repository variable `GIPNY_BROWSER_CONFIG_JSON`
  через `scripts/prepare-browser-config.py`, переменной нет (`gh variable list`),
  публичный мост, TURN, снапшот и домен не развёрнуты. Поэтому WASM-роутер не
  стартует ни в одном браузере и ни на одном устройстве: профиль создаётся, старые
  чаты читаются, отправить и принять нельзя. UI выдавал это за сбой роутера: экран
  загрузки стоял на 11 % с «[ FAIL ] Поднимаю свой релей» (`boot.step.relay`) и строкой
  `boot.retrying_in_background` («повторим в фоне» — ложно, без конфигурации
  повторов нет), а автоматический вход в чаты через 120 с не срабатывал, потому
  что его блокировал неудавшийся фоновый шаг. Статический анализ бандла блокеров
  движка для текущего Safari не нашёл (нужны structuredClone, Object.hasOwn,
  crypto.randomUUID, top-level await и синхронные OPFS access handles).
- **Ветка `fix/browser-boot-without-bridge`:** экран загрузки сообщает, что хостинг
  не предоставляет сетевой сервис, и открывает сохранённые чаты offline; явный
  нижний порог Safari/iOS 16.4 в браузерной сборке и тест-шлюз
  совместимости; понятные сообщения для Lockdown Mode (WebAssembly отключён) и
  приватного режима (нет приватного хранилища); документация —
  [браузер](browser-messenger.md#hosting-status), README, [состояние](current-state.md).
- **Только владелец:** развернуть мост (хост с публичным IP и доменом, TLS,
  форвардер, снапшот netDb, TURN для WebRTC), задать `GIPNY_BROWSER_CONFIG_JSON`
  и передеплоить Pages; чек-лист — в [browser-messenger.md](browser-messenger.md#hosting-status).
- **Открыто:** версия iOS и точный экран у владельца (загрузка на 11 %, пустая
  страница или сообщение о совместимости). Движок Safari проверен: задеплоенный
  бандл с Pages в Playwright WebKit 27.2 создаёт профиль за 297 мс и упирается в
  тот же экран «[FAIL] Starting local relay»; тот же прогон теперь в CI
  (`ui/tests/webkit-smoke.mjs`, job `wasm`). Системный WebKitGTK 2.52 не создаёт
  OPFS sync access handles и для этого не годится.

Gipny — мессенджер поверх I2P. Desktop и Android используют общий UI и Rust Core
с in-process i2pd; агент и боты — `SessionManager` общей библиотеки. Браузер
использует тот же UI, Rust WASM в Worker, client-only emissary и зашифрованный OPFS.
Отдельный локальный `gipny-web` удалён. Yggdrasil удалён: только I2P, ускорение и
маскировка — на уровне i2pd.

GitHub Pages раздаёт приложение и обновления; сообщения, профиль и ключи
обрабатываются на устройстве. Самостоятельность веб-клиента не отменяет доверия
к загружаемому коду, потребности в I2P/мосте и свежих bootstrap-данных (RouterInfo
не старше 36 часов). Браузер сохраняет свой живой netDb между запусками и
стартует с него без снапшота; первый запуск по-прежнему требует bootstrap.

### Что сделано в rc3

- **Приём:** durable inbox — ratchet, расход OPK и письмо фиксируются одной
  синхронной транзакцией до ACK релею; сбои хранилища не подтверждаются и
  повторяются (`libcore/src/db/inbox.rs`). X3DH init проверяется до любых
  изменений. Core и SessionManager.
- **Полномочия и устойчивость:** состав группы меняет только участник, ключи
  закреплены (`db/group_wire.rs`); длина ключа — ошибка, не panic; обязательная
  блокировка каталога для desktop и агента (`libcore/src/data_lock.rs`); агент
  перепроверяет мастера перед запуском; встроенный и standalone релей ограничивают
  соединения и время до первого кадра.
- **Уведомления о действиях собеседника:** WireDm 5/6/7 (копирование, сохранение,
  сочетание клавиш снимка) и квитанции 8/9/10; хранятся у автора без содержимого,
  удаляются с сообщением (`db/peer_events.rs`, `browser-core/deletion.ts`).
- **UPnP:** переключатель (по умолчанию вкл.), нейтральные описание/User-Agent,
  метка на шлюз, IP после подтверждённого проброса, без пересоздания на фокусе.
- **Мост WSS/WebRTC:** политика портов NTCP2-целей, keepalive, TCP после DataChannel,
  байтовые квоты Window/Worker, `/rtc` только при безопасной конфигурации.
- **Браузерные файлы:** без повторной отправки шифротекста, ограниченная история
  повторов, окно на контакт, без автоответов непринятым, компактный снимок профиля.
- **UI:** отметки удаления по инициатору, исправленные зависание чата, тексты и
  черновики; убраны мёртвые API/CSS/строки.

### Ограничения результата

Локально пройдены unit-тесты всех Rust-корней и UI/сайта. CI, живая доставка
телефон ↔ ПК ↔ браузер, реальные NAT/TURN/IGD и crash-injection на устройстве —
ещё нет. Открыто: Windows-захват журнала (CR-017), привязка `sign_pk` в X3DH init,
модель владения DHT-именами.

Сборки приложения и исполняемые тесты платформ — только GitHub Actions. Старые
данные можно менять без миграций; новые сообщения, ACK и состояние криптографии
обязаны сохраняться корректно. Runtime — Rust/C с C++ upstream i2pd; Python
допустим для CI и служебных инструментов.

Вход в актуальные материалы: [состояние](current-state.md),
[документация](README.md), [браузер](browser-messenger.md),
[файлы](file-transfer.md), [UPnP](upnp.md), [настройки/upstreams](upstreams-and-settings.md),
[зеркала](site-mirrors.md), [планы](roadmap.md), [rc3](releases/0.5.0-rc3.md).

## English

### 2026-11-08: the public browser client does not go online

The owner reported from an iPhone (Safari) that the WASM client does not start.
Reproduced without Safari in headless Chrome against the built client: the hosted
page https://gluckdev.github.io/gipny-i2p/app/browser.html has no `browser-config.json`
(HTTP 404). `pages.yml` writes it from the repository variable `GIPNY_BROWSER_CONFIG_JSON`
through `scripts/prepare-browser-config.py`; the variable does not exist (`gh variable
list`), and no public bridge, TURN, snapshot or domain is deployed. So the WASM router
never starts in any browser on any device: a profile is created, old chats are
readable, nothing is sent or received. The UI presented this as a router failure: the
boot screen stuck at 11 % with "[ FAIL ] Starting local relay" and
`boot.retrying_in_background` ("we will keep trying in the background", false: nothing
retries without configuration), and the 120 s automatic entry into the chats never
fired because the failed background step blocked it. Static analysis of the bundle
found no engine blocker for current Safari (it needs structuredClone, Object.hasOwn,
crypto.randomUUID, top-level await and synchronous OPFS access handles).

Branch `fix/browser-boot-without-bridge`: the boot screen states that this hosting
provides no network service and opens saved chats offline; an explicit Safari/iOS
16.4 floor in the browser build with a compatibility gate test; clear
messages for Lockdown Mode (WebAssembly disabled) and private browsing (no private
storage); docs ([browser](browser-messenger.md#hosting-status), READMEs,
[current state](current-state.md)). Only the owner can deploy the bridge (a host
with a public IP and domain, TLS, the forwarder, a netDb snapshot, TURN for WebRTC),
set `GIPNY_BROWSER_CONFIG_JSON` and redeploy Pages; the checklist is in
[browser-messenger.md](browser-messenger.md#hosting-status). Open: the iOS version
and the exact screen the owner saw (boot at 11 %, a blank page or a compatibility
message). The Safari engine is covered: the deployed Pages bundle in Playwright
WebKit 27.2 creates the profile in 297 ms and hits the same "[FAIL] Starting local
relay" screen; the same run is now in CI (`ui/tests/webkit-smoke.mjs`, `wasm` job).
The system WebKitGTK 2.52 cannot create OPFS sync access handles and is no substitute.

Gipny runs on I2P. Desktop/Android share UI and Rust Core with in-process i2pd;
agents/bots use the library's SessionManager. The browser shares UI and uses Rust
WASM in a Worker, client-only emissary and encrypted OPFS. The local gipny-web
gateway and Yggdrasil are removed: I2P only, with speed and camouflage pursued
inside i2pd.

rc3 adds a durable inbox (ratchet, OPK use and the letter commit atomically and
synced before the relay ACK; storage failures retry), authenticate-before-mutate
X3DH, member-only group changes with pinned keys, panic-free key parsing, a
mandatory data-directory lock for desktop and agent, agent permission rechecks,
bounded relay admission, author-side copy/save/screenshot-shortcut notices
(WireDm 5-10, no content, deleted with the message), a switchable and neutral
per-gateway UPnP, a hardened WSS/WebRTC bridge, safer browser large-file transfer
and UI fixes with dead API/CSS/strings removed.

Local unit tests pass for every Rust root and the UI/site. CI, live
phone/PC/browser delivery, physical NAT/TURN/IGD and on-device crash injection
are pending. Open: Windows log capture, binding `sign_pk` in X3DH inits, DHT name
ownership. The browser now keeps its live netDb between starts. Application builds belong in GitHub
Actions. Start with [current state](current-state.md), [documentation](README.md),
[browser](browser-messenger.md), [files](file-transfer.md), [UPnP](upnp.md),
[upstreams](upstreams-and-settings.md), [mirrors](site-mirrors.md),
[roadmap](roadmap.md) and the [rc3 notes](releases/0.5.0-rc3.md).

[Исторический журнал / Historical log](archive/handoff-brief-through-2026-11-03.md)
