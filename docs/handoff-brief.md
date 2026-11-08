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
- **Живой NTCP2 в CI:** job был красным на `main` и во всех PR с 2026-11-06:
  нативный пир после reseed держал 61 RouterInfo старше 36 ч, сессии к ним рвались,
  свежих не приходило, `run_ntcp2_ci.py` падал по дедлайну. Теперь перед шагом
  рукопожатий `collect-netdb` собирает свежий netDb, пир подсаживается на записи
  моложе 36 ч (`browser-router/tests/netdb_seed.py`, unit-тесты), снапшот
  холодного старта берётся оттуда же; reseed остался запасным путём. Принцип
  владельца «автообновляемые перед сборкой апстримы» записан в
  [workflows/README.md](../.github/workflows/README.md).

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

Своя экранная клавиатура, 8 локализаций, единый дизайн и автообновление
(2026-11-08, ветка `feat/own-keyboard` поверх PR #152):
- На телефонах системная клавиатура не показывается — у всех текстовых полей
  `inputmode="none"` (ставит `h()`), набирает панель приложения (`ui/src/osk.ts`:
  8 раскладок `ru`, `uk`, `en`, `de`, `vi` Telex, `km` NiDA, `hi` InScript, `zh`,
  цифры и символы, эмодзи, shift/caps, повтор backspace, клавиша раскладки;
  высота панели идёт в `--kbd-offset`). В личных и групповых чатах в круг
  переключения автоматически добавляются языки участников (`detectParticipantLayouts`,
  `setChatLayouts`), а `en` присутствует всегда. На Android окно дополнительно не
  взаимодействует с IME (`FLAG_ALT_FOCUSABLE_IM` в `MainActivity.kt`).
- Приложение (`ui/src/i18n.ts`, `ui/src/locales/*.ts`) и публичный сайт
  (`site/main.js`, `site/index.html`) поддерживают 8 языков (`ru`, `en`, `uk`,
  `de`, `km`, `vi`, `hi`, `zh`) и выбирают язык по умолчанию по единой логике из
  системных локалей (`navigator.languages`, `Intl`) и установленных в системе
  клавиатур (`navigator.keyboard.getLayoutMap()`).
- Весь интерфейс приведён к единому сквозному адаптивному дизайну IDE-консоли
  (светлая и тёмная темы в `ui/src/styles.css`, единая шапка `authHeader()` на
  экранах выбора, создания, разблокировки и загрузки профиля, 316 проверок
  Playwright в `ui/tests/layout.test.mjs`).
- Проверены и доведены ручное и автоматическое обновление в `SettingsModal`
  (`checkUpdate`, `installUpdate`, `restartApp`, `getAutoUpdate`/`setAutoUpdate`),
  постоянный баннер перезапуска подготовленного обновления (`.update-banner`),
  передача установщику Android (`UpdateInstallerOpened`) и превью-бэкенд
  `ui/dev/update-mock.ts`.
План и детали — [plans/2026-11-08-own-keyboard.md](plans/2026-11-08-own-keyboard.md).

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
Live NTCP2 in CI had been red on `main` and every PR since 2026-11-06: after reseed
the native peer held 61 RouterInfos older than 36 h, sessions to them reset, nothing
fresher arrived and `run_ntcp2_ci.py` hit its deadline. The job now collects a fresh
netDb with `collect-netdb` before the handshake step, seeds the peer only with
records younger than 36 h (`browser-router/tests/netdb_seed.py`, unit-tested) and
makes the cold-start snapshot from the same data; reseed remains the fallback. The
owner's "upstreams refreshed before every build" principle is recorded in
[workflows/README.md](../.github/workflows/README.md).
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

The app's own on-screen keyboard, 8 localizations, unified IDE-console design
and update flows (2026-11-08, branch `feat/own-keyboard` on top of PR #152):
- On phones the system keyboard never appears — every text field gets
  `inputmode="none"` from `h()` and the app's panel types instead (`ui/src/osk.ts`:
  8 layouts `ru`, `uk`, `en`, `de`, `vi` Telex, `km` NiDA, `hi` InScript, `zh`,
  digits and symbols, emoji, shift/caps, repeating backspace, a layout key; its
  height feeds `--kbd-offset`). Direct and group chats automatically include
  participant languages (`detectParticipantLayouts`, `setChatLayouts`), with `en`
  always present. On Android the window also declares itself IME-free
  (`FLAG_ALT_FOCUSABLE_IM` in `MainActivity.kt`).
- Both the app (`ui/src/i18n.ts`, `ui/src/locales/*.ts`) and the website
  (`site/main.js`, `site/index.html`) support 8 languages (`ru`, `en`, `uk`, `de`,
  `km`, `vi`, `hi`, `zh`) and select the default language from system locales and
  installed system keyboard layouts (`navigator.keyboard.getLayoutMap()`).
- All screens share one unified responsive light/dark IDE-console design
  (`ui/src/styles.css`, `authHeader()` in `ui/src/profile-salon.ts`, verified across
  316 combinations in `ui/tests/layout.test.mjs`).
- Manual and automatic update flows in `SettingsModal` and `App` (`checkUpdate`,
  `installUpdate`, `restartApp`, `getAutoUpdate`/`setAutoUpdate`, persistent
  `.update-banner`, Android `UpdateInstallerOpened`, and `ui/dev/update-mock.ts`)
  are complete and covered by tests.
See [plans/2026-11-08-own-keyboard.md](plans/2026-11-08-own-keyboard.md).

[Исторический журнал / Historical log](archive/handoff-brief-through-2026-11-03.md)
