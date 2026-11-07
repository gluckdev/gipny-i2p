# Передача контекста / Project handoff

Дата / Date: 2026-11-06. Версия / Version: `0.5.0-rc3` (ревью и исправления
поверх рабочей копии Codex), базовый / baseline HEAD
`a6e41caa07ffff94a834f72a4c13acd73f188224`.

## Русский

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

После rc3 (2026-11-07): на Honor KOZ-AL00 (Huawei WebView 11 = Chromium 83,
без Google) rc3 открывался пустым экраном — бандл UI был собран под ES2022.
Исправление: `WEBVIEW_FLOOR` (Chromium 83) в `ui/vite.config.ts`, шимы
`ui/src/compat.ts`, гейт `ui/tests/webview-compat.test.mjs`, `chrome://inspect`
в предрелизных Android-сборках; [журналы](release-diagnostics.md). Дальше по
плану: экраны входа в один экран телефона (360×800) и i2pd с его туннелями в
WASM для браузерного роутера.

Сборки приложения и исполняемые тесты платформ — только GitHub Actions. Старые
данные можно менять без миграций; новые сообщения, ACK и состояние криптографии
обязаны сохраняться корректно. Runtime — Rust/C с C++ upstream i2pd; Python
допустим для CI и служебных инструментов.

Вход в актуальные материалы: [состояние](current-state.md),
[документация](README.md), [браузер](browser-messenger.md),
[файлы](file-transfer.md), [UPnP](upnp.md), [настройки/upstreams](upstreams-and-settings.md),
[зеркала](site-mirrors.md), [планы](roadmap.md), [rc3](releases/0.5.0-rc3.md).

## English

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

After rc3 (2026-11-07): on an Honor KOZ-AL00 (Huawei WebView 11 = Chromium 83,
no Google services) rc3 opened as a blank page because the UI bundle targeted
ES2022. Fixed by `WEBVIEW_FLOOR` (Chromium 83) in `ui/vite.config.ts`, shims in
`ui/src/compat.ts`, the `ui/tests/webview-compat.test.mjs` gate and
`chrome://inspect` in prerelease Android builds; see
[diagnostics](release-diagnostics.md). Next in plan: sign-in screens that fit
one phone screen (360×800), and i2pd with its tunnels in WASM for the browser
router.

[Исторический журнал / Historical log](archive/handoff-brief-through-2026-11-03.md)
