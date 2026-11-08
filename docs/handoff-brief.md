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
в предрелизных Android-сборках; [журналы](release-diagnostics.md). Экраны входа на
телефоне (≤ 600px) стали плотнее, чтобы создание профиля помещалось в один экран
(650 px из 740 в `dev/preview.html`, блок «под принуждением» на виду); `dev/mock.ts`
получил сцены `select`/`unlock`, превью проверяет высоту карточки. Корень
приложения растягивается как flex-элемент (`.app-root`): на Chromium 83
`height: 100%` давал высоту контента, и под коротким экраном (загрузка профиля)
была светлая полоса. Сканер QR: карточка — плотный код, с 1080p-потока Honor его
не читал; теперь у камеры просится 2560×1920, добавлен проход по центральному
квадрату в полном разрешении, декодирование ушло в Worker (`ui/src/qr-worker.ts`),
QR в карточке показывается до 340 px вместо 200. Экран загрузки говорит «сеть не
отвечает», если за 30 с ни один узел не ответил (`gipny_router_peers` в shim →
`Router::peers` → `network-none`): на Honor с Wi-Fi без интернета он молчал все
10 минут до таймаута. Регистрация спрашивает имя один раз: имя профиля на
устройстве выводится из него (`ui/src/profile-name.ts`: транслитерация,
`[A-Za-z0-9_-]{1,32}`, уникальность только по локальному списку). Живая переписка
браузер ↔ телефон проверена локально (`browser-router/tests/run_browser_phone.py`,
Honor: запрос и сообщение за 32 с, ответ за 2 с). Дальше по
плану: i2pd с его туннелями в WASM для браузерного роутера.

Своя экранная клавиатура (2026-11-08, ветка `feat/own-keyboard` поверх PR #152):
на телефонах системная клавиатура не показывается — у всех текстовых полей
`inputmode="none"` (ставит `h()`), набирает панель приложения (`ui/src/osk.ts`:
раскладки ru/en/de, цифры и символы, эмодзи, shift/caps, повтор backspace,
клавиша раскладки; высота панели идёт в `--kbd-offset`, как у системной).
На Android окно дополнительно не взаимодействует с IME (`FLAG_ALT_FOCUSABLE_IM`
в `MainActivity.kt`): Huawei WebView 11 привязывал системную клавиатуру вопреки
`inputmode="none"`. Тесты `ui/tests/keyboard.test.mjs`, превью `dev/preview.html?scene=chat&osk=1`
(проверяет, что поле ввода над клавишами). План, срезы (vi Telex, km NiDA,
настройка auto/on/off, язык собеседника) и открытые вопросы —
[plans/2026-11-08-own-keyboard.md](plans/2026-11-08-own-keyboard.md).

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
[diagnostics](release-diagnostics.md). The sign-in screens are denser on phones (≤ 600px) so profile creation fits one
screen (650 px of 740 in `dev/preview.html`, duress block still in view);
`dev/mock.ts` gained `select`/`unlock` scenes and the preview checks the card
height. The app root stretches as a flex item (`.app-root`): Chromium 83
resolved its `height: 100%` to the content height, leaving a light band under
short screens such as the loading one. QR scanning: a card is a dense code and
the Honor's 1080p stream never read it; the camera is now asked for 2560×1920, a
centre-square pass at full resolution was added, decoding moved to a worker
(`ui/src/qr-worker.ts`), and the card's QR is shown at up to 340 px instead of
200. The loading screen now says that nothing on the network answers when no
router has replied for 30 s (`gipny_router_peers` in the shim → `Router::peers`
→ `network-none`); on the Honor's internet-less Wi-Fi it stayed silent for the
full ten-minute timeout. Registration asks for one name: the on-device profile
name is derived from it (`ui/src/profile-name.ts`: transliteration,
`[A-Za-z0-9_-]{1,32}`, unique on this device only). Live browser ↔ phone
messaging was verified locally (`browser-router/tests/run_browser_phone.py`,
Honor: request and message in 32 s, reply in 2 s). Next in plan: i2pd with its
tunnels in WASM for the browser router.

The app's own on-screen keyboard (2026-11-08, branch `feat/own-keyboard` on top
of PR #152): on phones the system keyboard never appears — every text field gets
`inputmode="none"` from `h()` and the app's panel types instead (`ui/src/osk.ts`:
ru/en/de layouts, digits and symbols, emoji, shift/caps, repeating backspace, a
layout key; its height feeds `--kbd-offset` as a system keyboard did). On Android
the window also declares itself IME-free (`FLAG_ALT_FOCUSABLE_IM` in
`MainActivity.kt`): Huawei WebView 11 bound the system keyboard despite
`inputmode="none"`. Tests in
`ui/tests/keyboard.test.mjs`; preview `dev/preview.html?scene=chat&osk=1` checks
that the field stays above the keys. Plan, slices (vi Telex, km NiDA, an
auto/on/off setting, the contact's language) and open questions:
[plans/2026-11-08-own-keyboard.md](plans/2026-11-08-own-keyboard.md).

[Исторический журнал / Historical log](archive/handoff-brief-through-2026-11-03.md)
