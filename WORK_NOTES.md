# Work notes — gipny-i2pd

Repo `gluckdev/gipny-i2p`. Local handoff file, not committed. The committed
handoff is `docs/handoff-brief.md` (top section, 2026-11-02).

## Goal (owner, 2026-11-02)
«доделывай и доводи до релиза 0.5», then «но сначала релиз с логами».
Asked and answered: **v0.5.0-rc1 prerelease with Full developer log first,
then stable 0.5.0 with Null.**

## Done
- `74eb1c5` (pushed to main) — prerelease log mechanism + CI fix:
  - `GIPNY_RELEASE_LOGS=full` at compile time → `i2p-embed::diagnostics::AVAILABLE`;
    macros check the const (cfg in macro_rules! would be evaluated in the caller).
  - `release.yml`: `version` job → `release_logs` (full iff version has `-`),
    passed to desktop/relay/agent/android-router/android and to `openwrt.yml`
    (new input `release-logs`).
  - `build-provenance.py`: refuses Full in a stable version, records
    `developer_log` per manifest, refuses mixed manifests; tests added.
  - Updater: `installed_version` accepts `X.Y.Z-pre`; rc1 gets 0.5.0 as an update.
  - Fixed red main: `check-release-diagnostics.sh` compiled diagnostics.rs as
    crate root (`$crate::diagnostics` unresolved); test race on global level.
  - Agent no longer traces console command text (policy violation from 430ac88).
  - Docs: release-diagnostics.md, build-release.md, workflows/README.md, handoff-brief.md.
  - Local checks: probe script (3 modes, 7 tests), libcore update tests 24/24,
    `cargo check --release -p gipny -p gipny-agent`, provenance/guard tests.
- `4da6ae5` (pushed 2026-11-02 ~20:45 UTC, release run 41520454011) — version bump to 0.5.0-rc1 in
  tauri.conf.json, ui/package(-lock).json, core/agent Cargo.toml, Cargo.lock,
  browser backend.ts; README rows; `docs/releases/0.5.0-rc1.md`.

- Release run 41520454011 (`4da6ae5`) FAILED: OpenWrt ELF gate
  (`check-openwrt-elf.py`) demanded trace strings be absent, but rc1 has them
  by design; plus macos-15 cancelled for capacity. Fixed in `06f4be0`: gate
  takes `--developer-log` (from `openwrt-build.sh` when GIPNY_RELEASE_LOGS=full)
  and then requires all traces present. Local full agent build confirmed both
  strings are in the binary. Dispatched release.yml publish=true → run
  41536627786 on `06f4be0`.
- macOS arm64 (macos-15) jobs cancelled ("not acquired by Runner", GitHub
  capacity) on every run since 2026-11-02 17:44 UTC; last success 08:48 UTC.

- Run 41536627786 attempt 1: everything green (macos-15 got a runner, OpenWrt
  fixed) except `agent (linux amd64)` smoke — Secret Service fixture race
  (secret-tool hit D-Bus before the unlocked daemon owned the name → locked
  activated daemon → prompt without display). `gh run rerun --failed` started.
- `b79433f` (LOCAL, NOT PUSHED): fixture waits until the owner PID of
  org.freedesktop.secrets is the unlocked daemon. **Push only after rc1 is
  published** — release-guard refuses to publish once main advances past the
  built SHA (06f4be0).

- **v0.5.0-rc1 PUBLISHED** 2026-11-02 23:47 UTC, prerelease (latest stays
  v0.4.19), run 41536627786 attempt 2, target `06f4be0`, 24 assets, all 15
  manifests `developer_log: true`.
- Pushed `b79433f` (fixture fix) + `0d549c6` (handoff brief). e2e-i2pd
  dispatched on `0d549c6` → run 41548551724 (validates fixture with real
  secret-tool; e2e does not run on push, nightly 03:17 UTC).

## Next
1. CI on `74eb1c5`: all substantive jobs green (rust workspace fixed). Only
   macOS arm64 jobs cancelled — GitHub "capacity constraints… not acquired by
   Runner" — and the live browser constrained-cold bootstrap (network deadline).
2. e2e run 41548551724 on `0d549c6`: all 4 live scenarios green, fixture OK with real secret-tool.
   Owner tests rc1 and sends logs.
3. Before stable 0.5.0: decide whether operational errors silenced by
   430ac88 (`debug_error!` for update/rebuild failures) should go back to
   stderr in stable builds — docs still promise them. Then bump to `0.5.0`.

## Leave alone
- Dirty submodules: `third_party/i2pd` = applied memory patch
  (`scripts/apply-i2pd-memory-patches.sh`), `third_party/i2pd-android` = fetched
  deps. Never commit them.
- Merged branches `feat/browser-messenger`, `feat/browser-router-runtime` are
  ancestors of main; delete only on the owner's cleanup request.

## 2026-11-07 — Honor KOZ-AL00: белый экран rc3 → план из трёх пунктов

Диагноз (adb): Honor KOZ-AL00, Android 10 / Magic UI 4.0, без Google —
WebView = `com.huawei.webview` 11.0.7.300 = **Chromium 83**. Бандл rc3 собран
с `build.target: es2022`; минификатор выдал `||=` (Chrome 85) → SyntaxError
всего модуля → `#app` пуст → белый экран. wry пишет консоль в logcat только в
debug (`Logger` гейтится `BuildConfig.DEBUG`), поэтому logcat молчал.
Скриншот через adb невозможен (`FLAG_SECURE`). Второй телефон = свежая
Google WebView, поэтому там работало.

### 1. Совместимость с Chromium 83 — ветка `fix/android-webview-chromium-83`, PR #152 (ПРОВЕРЕНО на Honor 2026-11-07)
- `ui/vite.config.ts`: `WEBVIEW_FLOOR = ['chrome83']` для `build.target` и
  `cssTarget` (Vite 8 = rolldown/oxc + lightningcss: `||=` понижается, `inset`
  раскладывается в top/right/bottom/left).
- `ui/src/compat.ts` (первый импорт `main.ts`): шимы `replaceChildren` (86),
  `Object.hasOwn` (93), `crypto.randomUUID` (92) — иначе `main.ts:21` падает и
  в catch-ветке тоже.
- `styles.css`: `100vh` перед `100dvh` (dvh = Chrome 108). Не чинится (косметика
  на 83): flex `gap` (84), `:has()` (105), `@container` (105), `aspect-ratio` (88).
- `ui/tests/webview-compat.test.mjs` (acorn): парсит `dist/**/*.js`, валит сборку
  на синтаксисе новее 83 и на вызовах API без шима; в `npm test` после build.
- `MainActivity.kt`: `WebView.setWebContentsDebuggingEnabled(true)` при
  `BuildConfig.DEBUG || VERSION_NAME.contains('-')` → chrome://inspect в предрелизе.
- Docs: README (RU/EN) требование WebView, release-diagnostics (RU/EN),
  ui/README, core/gen/android/README, handoff brief.
- Проверка: локальная сборка APK (разрешена владельцем 2026-11-07 «для тестов
  honor можешь локально собирать»): stub-роутер как в build.yml
  (`I2P_EMBED_STUB=1`, `ORG_GRADLE_PROJECT_skipRouter=true`), debug-подпись →
  перед установкой удалить rc3 (другая подпись). Потом PR → rc4 из CI.

### 2. Масштаб экранов входа на телефоне — создание профиля в один экран (ПРОВЕРЕНО на Honor: 702 px при viewport 360×800)
Honor: 720×1600 @ 320 dpi = **360×800 CSS px** (≈776 видимых: статус-бар,
нав-бар скрыт). Измерено в `dev/preview.html?scene=auth&lang=ru&only=phone 360`
(iframe 360×740, RU): `.auth-card` **990 px** + `.auth` padding 16 = 1006.
Строки: language 37+24, эмблема 68+32, заголовок 36+8, подзаголовок 38 (2 строки)+22,
поля 96/96/59/59 (+14 каждое; 96 = label+input+hint в 2 строки), divider 20+28,
duress 80+14, chk 17+14, попытки 59+14, err 16+12, кнопки 46+18, panel 26×2.
Бюджет сокращения ≈ −330: language → в строку с эмблемой/иконка (−40), эмблема
68→40 и отступы 32→16 (−44), заголовок 36→28 (−8), подзаголовок в 1 строку,
отступ 22→10 (−31), hint → placeholder/title (−95), `.field` 14→8 (−42), input
padding 8→6 (−28), divider 28→16 (−12), panel 26→16 (−20), err/row отступы (−14)
→ ≈ 690 px: без прокрутки на 740/776.
Результат (dev/preview, 360×740, RU): создание **650 px**, выбор профиля 525,
пароль 395, загрузка 681 — всё помещается. Сделано: `.salon-head` (циферблат 40px
+ select без подписи в одну строку), hints скрыты, `.field` 8px, input 7px,
panel 14/18/16, кнопки 44px; инлайн-стили `.chk`/`.row` → классы `auth-chk`/
`auth-actions`; dev/mock.ts сцены `select`/`unlock`; preview.html проверяет
высоту карточки входа.
- Плотность, а не `zoom`: на `max-width: 600px` и `max-height: ~820px` уменьшить
  эмблему (dial 68→40, margins), заголовок 25→21, подзаголовок в одну строку,
  `.field` margin 14→8, label 12.5→11.5, input padding 8→6, hint → `placeholder`/
  `title`, panel padding 26→14, language picker в строку с заголовком.
- Решение владельца 2026-11-07: «блок под принуждением оставь видимым, компактно».
- То же для `auth-unlock`, `profile-select`, `auth-booting` («страниц»).
- Проверка: `vite dev` при 360×800 (Chrome device emulation) + на Honor.

### 3. i2pd и его туннели в WASM (браузерный роутер)
Состояние: `browser-router/` = emissary-core (Rust) client-only, патч
`client-only-ntcp2.patch`; exploratory-пул 3in/3out ×2 хопа построен в Chrome CI
(докум. `docs/plans/2026-11-01-browser-router-wasm.md`); клиентский pool,
leaseset, streaming, доставка браузер↔браузер — не подтверждены.
Исследование 2026-11-01 отклонило i2pd→Emscripten: pthreads/COOP/COEP (GitHub
Pages не ставит заголовки), asio/TCP над WebSocket, UDP/SSU2, OpenSSL.
Владелец 2026-11-07 просит «адаптацию i2pd и используемых им туннелей для WASM».
- Спайк A (неделя): libi2pd single-thread под Emscripten (`-sASYNCIFY` или
  `-pthread` + `coi-serviceworker`), asio → shim над WebSocket к форвардеру,
  OpenSSL wasm, без listener/SSU2/транзита; метрики: размер wasm, память,
  время до первого exploratory-туннеля; сравнить с emissary (3,6 МБ raw,
  ~5 МБ linear memory, 81–197 с до готовности).
- Спайк B (параллельно): довести emissary до «туннелей как у i2pd»: клиентский
  pool с дефолтами i2pd (длина/кол-во/варианс), публикация leaseset через
  outbound (без inbound-адреса), streaming, garlic → сквозная доставка двух
  браузеров и браузер↔натив.
- Решение владельца после спайков: один роутер (i2pd/Emscripten) для всех
  платформ или emissary в браузере с паритетом туннелей. Критерии: «секунды до
  первого туннеля на телефоне», память < ~64 МБ, хостинг на GitHub Pages без
  COOP/COEP.
- Документы: обновить plan 2026-11-01, handoff brief, browser-router/README;
  tests-for-every-feature: e2e доставка двух браузеров в CI.

### Проверка на Honor 2026-11-07 (PR #152, run 42367743849)
Локальный NDK не скачался (dl.google.com ~30–100 КБ/с), поэтому взят debug-APK
из CI (`gipny-android-debug-apk-no-router`, 78 МиБ, unstripped). rc3 удалён
(`adb uninstall`), поставлен APK, запуск: `#app` заполнен, «New profile»,
карточка 702 px, scrollHeight 718 при viewport 360×800 → без прокрутки.
DevTools через `adb forward tcp:9222 localabstract:webview_devtools_remote_<pid>`
+ `scratchpad/inspect-webview.mjs` (Runtime.evaluate + Page.captureScreenshot —
работает несмотря на FLAG_SECURE). Консоль: только старые предупреждения
(frame-ancestors в meta CSP, interactive-widget). Косметика Chromium 83:
квадратный `outline` вокруг `.salon-dial` (outline не скругляется до 94) —
заменить на `::after`-кольцо. Кнопка «Create profile» синяя — так и в
dev-превью на main (не моё изменение).

### 4. Регистрация: одно имя вместо двух полей — СДЕЛАНО в ветке (`ui/src/profile-name.ts`, тест в localization.test.mjs)
«На этапе регистрации один раз спрашиваем логин, не надо два окошка имени».
Сейчас `AuthCreate` просит `profile name` (имя профиля на устройстве, латиница/цифры,
≤32, имя каталога) и `display name` (видят собеседники, ≤64). Оставить одно поле
«имя»: display name вводит пользователь, а имя профиля выводить из него
автоматически (транслитерация/slug; уникальность — только в пределах устройства,
по локальному списку профилей, при коллизии — суффикс; никакой сетевой/глобальной
уникальности — владелец 2026-11-07), не показывая второе окно. Проверить: экспорт/импорт
профиля, выбор профиля в списке (показывать display name), ограничения
`^[A-Za-z0-9_-]{1,32}$` на стороне ядра (`vault_create`), браузерный режим.

### 5. Редизайн: сквозной дизайн приложения в стиле консоли IDE разработчика (владелец, 2026-11-07, в планы)
«Экран регистрации сильно наворочили, надо сделать редизайн, под получение в
результате сквозной дизайн приложения. С уклоном на то, как выглядит консоль IDE
у разработчика, такая стилизация».
- Сейчас экраны входа — три разных стиля: `profile-salon` (салон: дерево,
  циферблат, золото) у создания/выбора профиля, `login-cyber` у пароля,
  `boot-cyber` у загрузки; основное окно — «коммерческий» мессенджер (синий accent).
- Цель: одна дизайн-система на все экраны (вход, список, чат, настройки, консоль
  агента, браузерная версия) с визуальным языком консоли IDE: моноширинный шрифт,
  тёмная панель с тонкими рамками, строки-«промпты», статусная строка внизу,
  акцентные цвета как у подсветки синтаксиса, минимум декора (без дерева/золота).
- Регистрация упрощается вместе с пунктом 4 (одно имя) — экран как один
  «терминальный» блок: имя, пароль×2, duress-блок (видимый, компактно), кнопка.
- Порядок: токены (`:root` цвета/шрифты/радиусы) → экраны входа → список/чат →
  модалки/настройки → сайт/скриншоты; сверять с dev/preview на 360×740 и
  десктопе; старые темы (`salon`, `cyber`) удалить, не держать параллельно.
- Проверка: dev/preview сцены, фит карточек входа, i18n RU/EN, Chromium 83
  (гейт), скриншоты в README/сайте обновить.

### 6. Белая полоса: экран не развёрнут полностью (владелец, 2026-11-07) — НАЙДЕНО и ИСПРАВЛЕНО
Причина (DevTools на Honor): `#app` 800, но `App.el` (`.stack`, inline `height:100%`)
= 718 = высота контента: Chromium 83 не резолвит процентную высоту flex-элемента
здесь; `.auth` 718, ниже — `body` (светлый). Живая инъекция `#app > .stack { flex: 1 }`
дала 800/800. Коммит 45fe0a8: класс `app-root` + `.app-root { flex: 1 1 auto; min-height: 0 }`.
Главный экран на Honor: `.main` (grid) 365 px из 800 даже с растянутым корнем,
`grid-template-rows: minmax(0,1fr)` не помогло (`calc()`-высота «неопределённая»
для треков на 83); работает flex-колонка на телефоне (`.main{display:flex}`,
панели `flex:1`) — проверено инъекцией (sidebar 365 → 800), коммит следом
за 45fe0a8; dev/preview list/chat/console на 360×740 без изменений.
Старые гипотезы ниже — для истории:
«На разных этапах работы (например, в момент подключения к профилю) экран
развёрнут не полностью и видна белая полоса здоровая». Белое = фон `body`
светлой темы там, где контент приложения не дотягивает до края.
Гипотезы (проверить на Honor через DevTools: `innerHeight`,
`visualViewport.height/offsetTop`, `--kbd-offset`, высота `#app`/`.auth`):
- `--kbd-offset` остаётся > 0 после закрытия клавиатуры (main.ts считает
  `innerHeight - vv.height - vv.offsetTop`; на Chromium 83 `interactive-widget=
  resizes-content` не поддерживается, resize visualViewport может не прийти/прийти
  в другом порядке) → `height: calc(100vh - offset)` режет экран снизу;
- `100vh` на WebView c edge-to-edge и скрытым nav-bar ≠ реальная высота окна
  (нет `dvh` на 83); `#app { min-height: 100vh }` + `html, body { height: auto }`;
- экран загрузки `boot-cyber`/`auth` не растягивается на всю высоту контейнера
  (flex: 1 / min-height) и просвечивает `body`.
Исправление: сбрасывать offset в 0, когда клавиатура закрыта (порог, `focusout`),
растягивать корень через `position: fixed; inset: 0` или `min-height: 100%` от
`html` с `height: 100%`, фон `body` = фон приложения (чтобы просвет никогда не
был белым), затем перепроверить на Honor и в dev/preview.

### 7. Языки и перевод сообщений (владелец, 2026-11-07, в планы)
«Полный перевод ещё на немецкий, вьетнамский и, конечно, камбоджийские языки;
подключаемый модуль перевода сообщений между собеседниками (между указанными
языками), что будет догружаться по необходимости, если тяжёлый».
- Полная локализация UI + сайт + in-app docs: de, vi, km (кхмерский; Камбоджа —
  кхмерский официальный). Словари `ui/src/locales/*.ts` типизированы — добавить
  ключи целиком, тесты `localization.test.mjs` на полноту; шрифт с кхмерской
  письменностью (системный на Android/Windows есть; в WebView/Linux проверить
  fallback), переносы строк для кхмерского (нет пробелов между словами — `lang=km`,
  `overflow-wrap: anywhere`, `Intl.Segmenter` недоступен на Chromium 83 → без него).
- Модуль перевода сообщений: между заданными парами языков (ru/en/de/vi/km),
  на устройстве (без облака: инварианты приватности), подключаемый и догружаемый
  по требованию, если модель тяжёлая (десятки МБ): загрузка по I2P/с GitHub с
  проверкой SHA256 как у обновлений (`i2p_http.rs`, манифест), хранение в app
  data, включение в настройках чата «переводить входящие/исходящие», показывать
  оригинал по тапу. Кандидаты: компактные NMT-модели (Bergamot/Marian-класс,
  OPUS-MT) через ONNX/ggml в Rust; для km/vi проверить наличие моделей и качество.
  Браузерная версия — WASM-вариант того же модуля, догружаемый.
- Связать с планом речи/транскрипции (память: languages-voice-translation-plan).

### Honor, APK с роутером (превью release run 42370159062, a4589ec) — 2026-11-07
- Установлен `gipny-i2p_0.5.0-rc3_android-arm64.apk` (подписанный, libi2pd.so,
  снимок netDb). Экран создания отрисован (702 px / 800).
- Создание профиля через DevTools: vault создан, роутер стартовал (220 узлов из
  снимка), адрес выдан, но туннели не строились 10 минут → `destination
  readiness timed out` → возврат на экран создания («профиль не смог создаться»
  у владельца — то же самое). Причина — сеть: Wi-Fi телефона без интернета
  (`ping 1.1.1.1` 100 % потерь, DNS пустой, everValidated=false), LTE (cellcard)
  тоже не грузил страницы. После того как владелец подключил телефон к рабочему
  Wi-Fi (everValidated=true): туннели, `destination ready`, встроенный релей
  готов, relay-client connected — главный экран за ~30 с.
- Вывод: ошибок приложения на Honor больше нет; «не создаётся профиль» = нет
  интернета, а экран загрузки 10 минут молчит об этом.

### 8. Экран загрузки: сразу говорить «нет интернета» — СДЕЛАНО в ветке (shim `gipny_router_peers` → `Router::peers` → `network-none/ok` → строка `.boot-network`; C++ часть компилируется только в CI i2p-embed.yml)
Роутер 10 минут строит туннели без связи и только потом «readiness timed out».
Сделать: проверка сети до/во время загрузки (Android: ConnectivityManager
validated-сеть через GipnyService → событие в ядро; ядро: ни одного успешного
транспортного соединения за 30–60 с → `boot_status` со стадией «нет сети /
сеть без интернета», кнопка «повторить»); не ждать 10 минут молча; в
журнале писать число попыток/соединений транспорта. Тест: e2e с отключённой сетью.

### 9. Клавиатура: язык свой и язык собеседника (владелец, 2026-11-07, важная доработка)
Факт: своей клавиатуры в приложении нет — везде системная (Android IME, на
десктопе — раскладки ОС); в коде только `inputmode=numeric` у TOTP и
определение русской раскладки для выбора языка UI (`i18n.ts`).
Варианты:
- Минимум: подсказка языка полю ввода (`lang` на `<textarea>` композера по языку
  чата/собеседника; Chromium передаёт его IME как hintLocales) — Gboard/Celia
  переключат раскладку, если язык включён у пользователя; гарантий нет.
- Полный: собственная клавиатура в приложении (раскладки ru/en/de/vi/km + своя
  для языка собеседника, переключатель в композере). Плюс приватность: системные
  клавиатуры с облачным предсказанием видят текст; минус: объём работы, ввод
  кхмерского/вьетнамского (диакритика, составные символы) сложен; на десктопе
  не нужна.
- РЕШЕНИЕ владельца 2026-11-07: «клавиатура — уязвимость, надо только свою
  отображать». Системную клавиатуру не показывать вовсе (на полях ввода
  `inputmode="none"`, Chromium 66+/WebView), рисовать собственную в приложении:
  раскладки ru/en/de/vi/km (+ язык собеседника из настроек чата), переключатель,
  цифры/символы, эмодзи, голосовой ввод — нет (см. план речи). Десктоп —
  физическая клавиатура остаётся. Браузерная версия — та же экранная клавиатура
  на тач-устройствах.
- Связать с пунктом 7 (языки/перевод) и 5 (единый дизайн).

### 10. QR не читается с камеры Honor (владелец, 2026-11-07) — ИСПРАВЛЕНО в ветке
Камера работала (задняя, 1080×1920, автофокус), не хватало плотности: карточка
~177 модулей, после уменьшения до 1280 — 2,4 px/модуль. На телефоне через
DevTools (jsQR инъекцией): полный проход 1280 — 0/6 кадров, центральный квадрат
в полном разрешении — 2/6 и первым. Стоимость прохода 150–900 мс → вынесено в
Worker. Камера с запросом 2560×1920 даёт 1920×2560. QR в карточке 200 → до 340 px.
Проверка в реальной сборке — следующий превью-прогон (CI).

### Локальная сборка APK и браузерный тест (2026-11-07, вечер)
- Владелец: «гоняй больше билдов на этом ПК». Сеть ~150–300 КБ/с; dl.google.com
  и ghcr-образы с NDK не вариант. NDK r29 нашёлся в `~/rusnode/add/android_build/`
  (другой проект), SDK (cmdline-tools, platforms;36, build-tools;36) докачан в
  `~/android_build/sdk`, JDK 17 из apt. Скрипты: scratchpad `build-apk-local.sh`
  (deps → OpenSSL → ndk-build libi2pd.so → `cargo tauri android build --debug`),
  `honor-install-verify.sh`, DevTools-пробы. Debug-подпись: ставится поверх
  только таких же локальных сборок; релизный rc пришлось снести (профиль «Honor»
  владельца ушёл; он создал «Gluck» заново через одно поле имени).
- ВНИМАНИЕ: локальный libi2pd.so собран из i2pd `2ad5c1a5` (старый чекаут
  сабмодуля), а закоммиченный пин — `dc8bdea1` (cb0ad3c). Сабмодуль переведён на
  пин + патч памяти; APK с роутером на пине ещё не пересобран.
- Вердикт «нет сети» проверен на Honor; первый вариант (GetNumPeers) не срабатывал —
  считает и попытки соединения; исправлено: только пиры с сессией, выборка на
  потоке транспортов (41a45f5).
- Браузер ↔ Honor: форвардер и wasm-pack собраны локально, снимок netDb из
  артефакта netdb-seed через `prepare-netdb` (нужен native-peer → пин i2pd),
  сценарий `scratchpad/browser_phone.py` (CDP + DevTools телефона).
- CI browser-router на c6be87d упал: `wasm` — гейт webview-compat без dist/
  (исправлено ae9020f: пропуск без Tauri-сборки); `ntcp2` — live-флейк
  bootstrap + messenger-harness ожидал 4 поля формы (исправлено на 3).

### Браузер ↔ Honor — ПРОШЛО 2026-11-07 (`browser-router/tests/run_browser_phone.py`)
Форвардер + страница локально, headless Chrome, телефон через DevTools: туннели
браузера 64 с; запрос контакта + «hello from the browser» на телефоне через
32 с; квитанция доставки в браузер ~2,5 мин (медленно — отдельно посмотреть
путь квитанций в браузер); «hello from the Honor» в браузере через 2 с, квитанция
2 с. DevTools-сокет Huawei WebView рвётся после ~2 мин простоя — в сценарии
переподключение. На телефоне остался контакт «browser» в открытом профиле.

### CI на 678488e (конец дня 2026-11-07)
build, i2p-embed, security fast, codeql, OpenWrt, сайт — зелёные (сайт починен:
проверка ждала Chrome 3 с и падала до первого кейса на всех пушах ветки).
browser-router: `wasm` и форвардер зелёные; живая джоба NTCP2 прошла bootstrap и
первые сообщения двух браузеров, упала позже на «deadline: unblocked exact
ciphertext retry» (ранее сегодня — на bootstrap). К правкам ветки не относится
(они не касаются browser-router, кроме формы в harness); перезапустить позже.
QR новым путём (воркер) на Honor не замерен: кабель выпал во время скана.

## 2026-11-08 — PR #152 ждёт слова владельца; своя клавиатура; воркеры

- PR #152 (`fix/android-webview-chromium-83`, 678488e): все содержательные чеки
  зелёные; два красных — такие же на main: `updater through the outproxy (live)`
  = GitHub `/releases/latest` 404, пока есть только пререлизы (до стабильного
  0.5.0); живая браузерная джоба — WSS-проход упал на «unblocked exact ciphertext
  retry» (Боб принял письмо 16:57:49, его квитанция не дошла: `Relay did not
  answer`/`CANT_REACH_PEER`), WebRTC-проход прошёл. Перезапуск --failed запрошен
  23:00 UTC. Тело PR дополнено (п. 8 run_browser_phone.py, статус проверок).
  Мерж — только по слову владельца (память one-branch-policy).
- Схемы `core/gen/schemas/*.json` (+7,5 тыс. строк от локальной сборки Tauri)
  откатаны, не коммитить. Сабмодули как были (не коммитить).
- Локальная сборка APK с роутером: скрипт сессии
  `scratchpad/build-apk-local.sh` (ndk-build на пине dc8bdea1 + патч памяти —
  obj уже актуальны, 31 gipny_* символ, в т.ч. `gipny_router_peers`; ui build;
  `cargo tauri android build --debug --apk --target aarch64`, GIPNY_RELEASE_LOGS=full,
  без netDb-снимка — артефакт netdb-seed в релизных прогонах не хранится).
  Результат `scratchpad/apk/gipny-router-arm64-debug.apk` (95 МБ, universal debug),
  ~2 мин инкрементально. Урок: `nm | grep -q` под pipefail падает по SIGPIPE.
- Honor подключён (AADE9X2A04W03398), рядом realme RMX2063 (94a8938c) — всегда
  `adb -s`. USB Honor рвётся на больших передачах (install/push 95 МБ → offline,
  `failed to read copy response`); переведён на Wi-Fi: `adb tcpip 5555`,
  `adb connect 10.0.0.238:5555` (ПК 10.0.0.198). Установка по Wi-Fi запущена.
  Пакет — `app.gipny.i2p` (на Honor стоит локальная сборка 0.5.0-rc3 от 2026-11-07
  23:28 местного; поверх неё локальная подпись ставится `-r`).
- Своя клавиатура (решение владельца 2026-11-07) — ветка `feat/own-keyboard`
  поверх PR #152: e6be062 ядро (osk*.ts, inputmode=none через h(), ru/en/de,
  символы, эмодзи, тесты 7/7, гейт Chromium 83 зелёный), e23f3f1 документация +
  превью `?osk=1` (чат и вход на 360×740 и 412×892: клавиатура 199 px, поле над
  клавишами), далее настройка auto/on/off (воркер T4). План:
  docs/plans/2026-11-08-own-keyboard.md. Клавиатура ещё НЕ в установленном APK.
- Воркеры (владелец: «разделись на агентов, призови другие ИИ из опенкода,
  codex, agy»; «gipny-wt — мусор» → рабочие деревья ТОЛЬКО в scratchpad сессии,
  `git worktree add --detach`, после снятия диффа удалить): T1 Codex vi Telex,
  T2 agy km NiDA, T3 Codex немецкая локализация (пилот N языков), T4 opencode
  glm настройка клавиатуры (ПРИНЯТ, закоммичен), T5 opencode mimo разбор флейка
  браузерной джобы (отчёт), T6 agy ревью клавиатуры (отчёт). Брифы и логи —
  `scratchpad/briefs`, `scratchpad/logs`. agy в headless-режиме требует
  `--settings '{"permissions":{"allow":["command(node)",…]}}'`, иначе «no output
  produced»; `--mode auto`/`--dangerously-skip-permissions` блокирует классификатор
  Claude Code; `--mode plan`/`accept-edits` проходят.
- Vite dev на 5173 поднят для превью (`node_modules/.bin/vite`, не `npx vite` —
  тот тянет чужую версию); headless Chrome с CDP на 9333 (`scratchpad/cdp.mjs`:
  nav включает Emulation.setFocusEmulationEnabled, иначе focus() в кадре не работает).

### Итог вечера 2026-11-08 (по состоянию на ~00:30 UTC 09.11)
- `feat/own-keyboard` (поверх PR #152): e6be062 ядро → e23f3f1 доки/превью →
  5f9035a настройка auto/on/off (opencode glm) → c91fb1d vi Telex (Codex) →
  5410466 km NiDA (Codex + 4 клавиши с двумя кодовыми точками дописаны вручную) →
  fe8e8b4 немецкий + LANGS (Codex, 695 ключей) → ee2763b правки по ревью Codex →
  97feeb3 доки → 295f47d фокус назад в поле при тапе по клавише (найдено на Honor).
  Полный `npm run build && npm test`: 33 + 14 + 38 + гейт — зелёные. Ещё не
  запушено (ждёт проверки 295f47d на Honor).
- Honor: локальный APK с роутером (ветка PR #152) поставлен по Wi-Fi: профиль
  «osk-test» создан одним полем, роутер готов за 100 с, главный экран — п. 6/7/8
  PR #152 подтверждены в установленной сборке. Затем APK с клавиатурой:
  при настоящем касании поля пароля системная клавиатура НЕ появилась
  (innerHeight 800 неизменна), панель 199 px, буквы/пробел/backspace печатают
  (`adb shell input tap`, dpr 2; CDP Input.dispatchTouchEvent на этой WebView
  виснет). Функциональные клавиши (shift, ?123, EN, эмодзи) не срабатывали: тап
  уводил фокус на кнопку → панель пересоздавалась со сбросом. Исправление 295f47d
  (фокус назад в поле в onClick), новая сборка `scratchpad/apk-osk2` ставится.
  Пароль тестового профиля osk-test: `OskTest-2026!` (ввести клавиатурой так и не
  удалось из-за shift/символов — проверить после новой сборки). QR с камеры по
  новому пути НЕ замерен (нужен владелец с двумя телефонами).
- Воркеры: все отчёты в `scratchpad/wt/*/REPORT.md` и `scratchpad/reports`.
  T5 (анализ флейка) применён в PR #152: `browser-router/tests/run_messenger_ci.py`
  — два ожидания с бюджетами из констант (RESEND_WAIT 600 с, RECEIPT_WAIT 480 с),
  закоммичено в fix/android-webview-chromium-83 и запушено. T6-ревью (Codex):
  открытые пункты — вставка из буфера, «приватная подсветка» клавиш для пароля,
  RTL, iOS/WebKit, Android-резерв (onCreateInputConnection), тесты в реальном
  браузере. agy headless без `--dangerously-skip-permissions` не работает
  (классификатор Claude Code его запрещает) — задачи T2/T6 переданы Codex.
- CI: перезапуск browser-router на 678488e снова красный, но по другому шагу
  (cold bootstrap exploratory deadline + WebRTC «file-only request received after
  acceptance»; WSS прошёл) → сетевая изменчивость; больше не перезапускать.
- Владелец: «тестируй сразу на двух телефонах, плюс приложение браузерное».
  realme RMX2063 (94a8938c, Android 11, Google WebView 157, Chrome 157): стоит
  стабильная 0.4.19 с релизной подписью → локальный debug-APK поверх неё НЕ ставить
  (снесёт профиль). Запущен превью release.yml на feat/own-keyboard (run
  42430512247, publish=false) — подписанный APK для realme. Браузерная версия:
  `vite preview --config vite.browser.config.ts --host --port 4173` (browser-dist)
  + `adb reverse tcp:4173 tcp:4173` на телефоне → http://localhost:4173/browser.html
  (secure context; dev-сервер 5173 отдаёт 403 на wasm через /@fs). На realme в
  Chrome клавиатура прошла полностью (CDP Input.dispatchTouchEvent там работает);
  на Honor единственный браузер — Cromite (org.cromite.cromite) с выключенным
  WebAssembly → браузерная версия не стартует (настройка браузера). PR #153
  (base fix/android-webview-chromium-83) открыт с таблицей проверок.
- 2026-11-09 ~01:30 UTC. Превью 42430512247 (0.5.0-rc3, versionCode 5000,
  подпись «gipny release» совпала) поставлено на realme поверх 0.4.19 без потери
  данных (профиль «Fgg» на месте). Проверка касаниями `adb shell input tap`
  (realme dpr 3, Honor dpr 2; `finger-test.sh`): realme — всё работает, IME нет.
  Honor — ОТКРЫТИЕ: Huawei WebView 11 привязывал HeliBoard к полю вопреки
  inputmode=none; окно IME не видно на CDP-скриншоте (он снимает только WebView),
  innerHeight при adjust=pan не меняется, поэтому ранний вывод «IME нет» был
  ошибкой; признак — `dumpsys window windows | grep InputMethod` с
  mAnimationIsEntrance/Surface, и пустой журнал DOM-событий при набирающихся
  буквах. Исправление: FLAG_ALT_FOCUSABLE_IM + SOFT_INPUT_STATE_ALWAYS_HIDDEN в
  MainActivity.kt (коммит в feat/own-keyboard, запушен). После него на Honor
  касания доходят до панели, набрано «oSk-2026!😀». Побочный эффект: на Android
  настройка «Никогда (системная клавиатура)» не действует без моста (срез 2).
- Промах скрипта: finger-test.sh тапнул первую кнопку списка профилей (это
  «gluck» владельца на Honor) и в конце нажал Enter → одна неверная попытка
  разблокировки профиля gluck. Скрипт исправлен (без лишнего тапа и без Enter).
  Сказать владельцу.
- Realme сейчас: превью 0.5.0-rc3 без оконного заслона (там WebView 157 держит
  inputmode=none сам). Новый превью с заслоном — по желанию владельца.

## 2026-11-08 — «на айфоне не стартует wasm клиент в safari»

Диагноз (воспроизведено БЕЗ Safari, headless Chrome 157 по CDP на собранном
`ui/browser-dist` и на скачанном бандле с Pages): у публичного клиента
`gluckdev.github.io/gipny-i2p/app/` нет `browser-config.json` (404) — repo
variable `GIPNY_BROWSER_CONFIG_JSON` не существует, публичный мост/TURN/снапшот
не развёрнуты. Воркер стартует «without network config», `startNetwork` падает
с «Network service is not configured…», экран загрузки стоит на 11 % с
«[FAIL] Starting local relay — Not yet — we will keep trying in the background»
(ложь: без конфига ретраев нет), автопереход через 120 с не срабатывает
(`armBootTimeout` блокируется любым failed-шагом, включая фоновые relay/dht).
То есть роутер не стартует ни в одном браузере; iPhone — просто единственное
устройство, где владелец открывает веб-клиент.

Safari-специфика: статический разбор бандла — нужен Safari ≥ 15.4
(structuredClone/hasOwn/randomUUID/TLA), WASM-фичи все Safari 15+.
Локальный WebKitGTK 2.52 (python GI + xvfb, скрипт
`scratchpad/webkit-probe.py`): UI стартует, JSC выполняет WASM (Argon2id 283 мс),
но `createSyncAccessHandle` → NotSupportedError «Backend does not support this
operation» — ограничение GTK-порта, не Safari. Для реальной проверки OPFS-потока
в WebKit нужен macOS-раннер или телефон.

Ветка `fix/browser-boot-without-bridge` (worktree `../gipny-i2pd-wt-browser-boot`
от origin/main): A — правдивый экран загрузки + вход без моста + ошибка создания
в форме; B — Safari-порог `vite.browser.config.ts` + гейт `browser-compat.test.mjs`
+ понятные ошибки (Lockdown/WASM off, приватный режим, старый Safari);
C — docs/browser-messenger.md, handoff, README (статус хостинга + чеклист моста);
D — проверка Playwright WebKit на Linux. Переводы новых строк — Codex (Orca).

Владелец спросил про отдельный роутер «например на Java» — ответ в чате сессии:
Java в Safari не запустить; отдельный роутер = сервер-мост (тонкий клиент),
что противоречит решению 2026-11-01 (роутер в браузере, узлы — только транспорт).

SECURITY: repo VARIABLE `DHT_SEED_AGE_KEY` содержит приватный age-ключ
(relay-testnet.yml ждёт SECRET) — перенести в secret и ротировать.
