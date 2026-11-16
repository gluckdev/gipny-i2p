# Передача сессии

Обновлено: 2026-11-16. Проект: `/home/artamonov/projects/gipny-i2pd`.
Интеграционная ветка `release/0.5.0-alpha.2`, создана от `feat/own-keyboard`.
Собственные изменения сохранены тематическими коммитами; все 16 локальных и
remote refs объединены в HEAD `54ddcb3` (до последующих performance-правок).

## Текущий запрос владельца

- Разрешены тесты, коммиты, push, GitHub Actions и релиз `0.5.0-alpha.2`.
- До новых сборок свести все локальные и удалённые ветки в одну — выполнено.
  Main и три Dependabot PR объединены; сохранены более новые зависимости.
  Остальные ветки уже были ancestors. Коммитирован router pin `3835d09…`.
- Исследовать увеличение параллельных соединений по первичным форумам I2P,
  оценивать полезную скорость и потери, hops не менять.
- Последнее уточнение: добиться максимальной полезной пропускной способности
  для будущих голосовых звонков и трансляций экрана без ослабления анонимности.
  Режимы кофеин/кокаин удаляются полностью. Alpha2 ждёт performance-проверок.
- Готовится последовательный live sweep 1..8 потоков с 3-hop Destination,
  padding и прежним WINDOW=16: файл + одновременные короткие echo probes.
  Измерять RTT под нагрузкой, уникальные байты, повторы; не выдавать RTT jitter
  за RTP jitter или скорость файла за доказательство пригодности к звонкам.
- `GIPNY_FILE_LANES=1..8` — ограниченный измерительный override; без него
  остаётся прежняя политика до 4 потоков. Исправлен сброс cursor на каждом
  batch. Измеренного выигрыша пока нет; GitHub benchmark запущен, результаты
  ещё ожидаются (ссылка ниже).
- Автоматическая релизная рассылка отключена: sender вызывает отсутствующий
  CLI агента. Исправление и явные получатели требуют отдельной реализации.

### Подготовлен baseline для GitHub

Отправлен baseline `b1b1795807f3b14b70d46bcfa2ae3261e928a3f7` в
`origin/release/0.5.0-alpha.2`. Все актуальные local/remote refs проверены как
ancestors перед push; main пока не изменён.

- Throughput run: https://github.com/gluckdev/gipny-i2p/actions/runs/43875210051
  (e2e-i2pd dispatch `throughput=true`, pinned source, 1..8, 32 MiB).
- Native matrix baseline: https://github.com/gluckdev/gipny-i2p/actions/runs/43874857745
  (автоматический i2p-embed push run на предыдущем `6c0313e`).
- Прямой dispatch нового workflow дал 404 до появления в main; существующий
  e2e workflow вызывает его как reusable. Обычные delivery jobs в этом режиме
  skipped: throughput run не заменяет normal e2e gate перед релизом.
- На момент записи измеренные результаты ещё не получены. Native startup fixes
  закоммичены локально (`a6756d8`, `6200b44`), browser/UI проходят финальный
  review; alpha2 не опубликована.

Изменения: UI modes удалены (`6be4761`), padding/lanes/metrics (`2306baf`),
native protected throughput benchmark (`940a932`), alpha2 metadata (`a46a01b`),
отмена owned prewarm builders (`f4747ed`). Новые startup improvements идут
отдельным шагом, не выдавать их за проверенные live benchmark.

Проверено после изменений: core 30, libcore 210, DHT lib 27; дополнительно
8-stream collect regression, performance harness 12, workflow helper 16,
prewarm cancellation 3, release guard 9, provenance regressions 10.
Rust проверки здесь с I2P_EMBED_STUB=1, shipping native CI ещё требуется.
UI: обе сборки, полный npm test и layout 316/316; site 26/26 + build.

Исторический browser baseline: реальные application tunnels 2/2. Первичный вывод о том, что
это нельзя изменить без upstream, исправлен после проверки происхождения:
ClientHandle::create добавлен НАШИМ `client-only-ntcp2.patch` и наследует default.
Явные 3/3 реализованы в собственном adapter; чужие defaults не изменены.
Policy unit test, WASM build и frontend tests прошли; live browser CI на новой
политике ещё требуется. Реализованы parallel UI/config/WASM, verified local
netDb прежде сетевого snapshot и отменяемый public-bootstrap prewarm.

### Последнее уточнение: браузер на Honor и realme

Honor/Chromium сообщает WebAssembly disabled; realme/Chrome — отсутствие
сетевого сервиса на хостинге. Нужны центральные полупрозрачные уведомления с
полным текстом и без перехвата нажатий. Пользователь подчёркивает: браузер
должен запускать собственный I2P-router, как уже работало раньше.

История это подтверждает: CI `fa60b02`, run `41329839707` — два независимых
браузерных router, 7 acceptance checks; 2026-11-07 browser ↔ native Honor
также прошёл (WORK_NOTES.md). Router, identity и tunnels принадлежат браузеру;
локальный ws://127.0.0.1:18767/stream был только транспортом NTCP2-байтов.
Не заменять это серверным Core. Старые localhost endpoints нельзя переносить
в публичную конфигурацию мобильных пользователей.

Проверенный Pages URL: https://gluckdev.github.io/gipny-i2p/app/browser.html.
Во всех 8 успешных WASM Pages builds 7–8 ноября не было публичной network
configuration; текущий browser-config.json отвечает 404. Точный URL владельца
ещё запрошен, поэтому нельзя утверждать, что найденная конфигурация объясняет
именно его телефон. Новая инфраструктура не создавалась.

Window-проверка WebAssembly перенесена в фактический Worker; Chromium smoke
проверяет разные realms, недоступный API, CompileError и отсутствие unhandled
rejections. ToastLayer центрирует полный текст; обычные notices пропускают
pointer events, слишком длинный текст прокручивается внутри ограниченной
карточки. Layout316/316 и keyboard/modal/click-through/1000+chars checks прошли.
Эти изменения ещё не опубликованы и не проверены на самих телефонах.
Детали: docs/startup-performance.md, docs/browser-network-deployment.md.

Общий candidate `b232875` отправлен в GitHub, но clean native frontend gate
выявил TS2614: ambient wasm.d.ts не содержал двух новых exports. Прогоны
остановлены до публикации; installer checks2/2 успели пройти. Исправлены types
и две test-only границы: unit Worker не требует generated pkg, неиспользуемый
preparation re-export удалён. Полный npm run build + npm test прошёл из git
archive без browser-router/pkg; runtime/crypto не подменялись. Нужен повтор
всех финальных gates на следующем SHA. Run IDs хранятся также во временном
/tmp/gipny-alpha2-candidate-ci.json; старые runs не считать финальными.

## Как продолжить

1. Прочитать этот файл и [TASKS.md](TASKS.md), проверить `git status --short`.
   Дерево и подмодули не чистить и не откатывать.
2. Завершены Tauri entrypoint (416 строк), Session (463), DB (547), Core (409)
   и Rust UPnP manager (272). Не повторять эти переносы.
3. Текущий приоритет — восстановление mobile browser path, общие CI gates и
   alpha2; подробности последнего уточнения выше. Очередь декомпозиции крупных
   **собственных** файлов сохраняется. Следующие Rust-кандидаты: libcore/relay_server (1758),
   e2e-harness/main (1444), browser-router/engine (1389), libcore/update (1303),
   libcore/files (1244), core/relay/main (1014). UI/site очередь — TASKS.md.
4. DHT/backup исправления остаются отдельной очередью, не смешивать их с
   переносами. Для будущего seen fix нельзя просто переместить mark после
   handle: нужны общая атомарность, ratchet и concurrent/crash-тесты.

## Указания владельца

- Работать самостоятельно; Flint не вызывать.
- Локальные сборки/тесты разрешены. CI нужен, когда всё готово к релизу.
- Уточнение владельца: большие чужие файлы из upstream, vendored/generated-код
  не трогать. Разбиваем только собственный код; проверять происхождение кандидатов.
- Большие собственные исходники разнести по доменам, с понятными стандартными
  именами файлов, без дублирования реализаций.
- Сохранять API/IPC, wire-format, cfg, события, порядок сообщений,
  транзакционные границы, ratchet commit, X3DH, ACK и replay-защиту.
- Соединения увеличивать понемногу при необходимости и измерять прирост после
  каждого увеличения. Длину I2P-туннелей ради benchmark не менять.
- Отдельный запрос на commit/push/CI/release получен 2026-11-16 (см. выше).
  AI в соавторы не добавлять.
- Прежняя остановка для закрытия OpenCode отменена новым запросом продолжить.

## Выполнено в продолжении

### Декомпозиция Session, DB, Core, UPnP — последний шаг

- `libcore/src/session.rs`: 3015 → 463. Подмодули identity, contacts, messages,
  inbox, outbox, history, relay, dht, files, delivery; delivery/wire tests отдельно.
  Поля состояния, start/shutdown и общая политика в корне. Общий
  `message_wire.rs` остался единственным codec, публичные пути сохранены.
- `libcore/src/db.rs`: 2773 → 547. В корне типы/connection/helpers; schema,
  contacts, sessions, prekeys, messages, outbox, attachments, settings, groups,
  pins, dedup, DHT и backup выделены по доменам. File persistence объединён
  с существующим file_transfer.rs; outbox tests — db/outbox/tests.rs.
- `core/src/core.rs`: 2595 → 409. Выделены lifecycle, identity, contacts,
  messages, groups, history, hosted_relay, network, events. Оставшиеся методы
  прежних доменов добавлены в соответствующие файлы. Публичные типы re-export.
- `i2p-embed/src/upnp.rs`: 1949 → 272. Discovery, ffi, protocol, mapping,
  ownership, session и тесты по доменам. Это собственный Rust manager,
  происхождение проверено по README и e9a4721. Upstream C++/miniupnpc не менялись.
  Собственный upnp_fixture.rs сохранён; его доступ к helpers обеспечен
  test-only imports и pub(super), без изменения тестовой логики.

Проверено сравнение снимков после одинакового rustfmt (кроме видимости/импортов):
Session — 116 fn, 1 struct, 4 enum, все constants; Core — 166 fn, 7 struct,
8 enum; DB — 298 fn, 22 struct, 6 enum, 854 строковых/SQL literals; UPnP —
128 fn/FFI declarations, 22 struct, 4 enum, 2 trait, 4 type, constants.
Отдельные read-only обзоры Session, Core и DB не выявили нарушений API/cfg/tx.
Были поправлены только оборванные/неверно прикреплённые doc-комментарии.

Снимки и проверки: `/tmp/gipny-session-before-split-20261116.rs`,
`/tmp/gipny-verify-session.py`, `/tmp/gipny-core-before-20261116`,
`/tmp/gipny-verify-core-20261116.py`, `/tmp/gipny-db-before-20261116`,
`/tmp/gipny-verify-db.py`, `/tmp/gipny-upnp-before-20261116.rs`,
`/tmp/gipny-verify-upnp.py`. Это временные verification scripts.

Known DHT/backup дефекты не исправлялись, общее дублирование Core/Session
не устранено простым переносом. Аудиты дополнены картой новых путей;
старые line references в отчётах относятся к состоянию до декомпозиции.

### Декомпозиция Tauri entrypoint

Из `core/src/lib.rs` вынесены:

- `lifecycle.rs` — boot/prewarm, boot events, router settings/hints (621 строк).
- `dto.rs` — сериализуемые DTO и преобразования/attach_extras (270).
- `commands/messages.rs` — отправка, история, поиск, chat preferences (395).
- `commands/files.rs` — attachment/privacy, media history, clipboard и прежние
  attachment tests (454). `PASTE_DIR` остался одним static.
- `commands/updates.rs` — проверка/установка обновлений и APK (139).
- `commands/backup.rs` — тонкие export/import IPC (39).
- `diagnostics.rs` — capture/read/wipe логов (160).
- `platform.rs` — base dir, Windows AUMID, tray lifecycle, notifications (175).
- QR-команда добавлена в существующий `commands/identity.rs`.

В корне остались AppCtx, Tauri wiring и небольшие общие helpers. Публичные пути
`Prewarm` и `resolve_base_dir` сохранены через re-export. DTO и attachment
helpers импортируются из доменных модулей, исходные определения удалены.
Поведение, в том числе известные backup-ошибки, не исправлялось этим переносом.

После каждой логической группы выполнялись host check и 30 library tests.
Итоговое сравнение с началом сессии после одинакового rustfmt: 113 определений
функций, 16 struct и 1 enum совпадают после исключения видимости/форматирования.
`run()` отличается только квалифицированными путями регистрации обработчиков.
Все 117 IPC-имён и порядок совпадают с исходным снимком и HEAD, дублей нет.
Отдельный статический обзор не выявил изменений cfg/DTO/events/IPC.

Временные материалы проверки: `/tmp/gipny-verify-move.py`,
`/tmp/gipny-lib-before-20261116.rs` и форматированный снимок рядом.
Это ad-hoc проверка переноса, не сохранённые regression tests.

### DHT: статические результаты

Новые отчёты со сценариями, file:line и предлагаемыми тестами:

- [Lifecycle](docs/audits/dht-lifecycle-2026-11-16.md).
- [Ресурсы/протокол](docs/audits/dht-resources-2026-11-16.md).

Два независимых прохода и повторная проверка ведущим агентом конкретного кода
подтвердили следующие пути:

- durable seen пишется раньше inbox/ratchet commit: crash/abort оставляет
  необработанную DHT-копию пропускаемой;
- последующие collect обходят только сегодня/вчера, не весь 7-дневный TTL;
- Get пагинация и общий collect не имеют суммарного бюджета; bootstrap пингует
  всех кандидатов одновременно независимо от alpha;
- storage считает только payload bytes, пустые values под разными ключами
  обходят общую квоту и в MemStorage, и в SQLite;
- Core address lookup detached; оба пути могут применить запоздалый relay;
- Core republish_bundle не обновляет DHT после расходования OPK, SessionManager
  обновляет; коллизия одного OPK у двух offline отправителей требует отдельного
  протокольного решения;
- недоверенный Deleted(u32) может переполнить агрегированный счётчик;
- alias API допускает backdated ownership и раскрытие записи хранящему узлу
  (secret выводится из открытого key). Активное использование alias из
  Core/SessionManager не найдено; это отдельная граница воздействия.

Это статические доказательства, не live воспроизведение. Исправлений, новых
DHT regression tests и fault injection в этом шаге **нет**. Общая безопасность
DHT не подтверждена. Задачи добавлены в TASKS.md, не считать их закрытыми.

## Сохранённые результаты предыдущей работы

- Из Core ранее выделены `core/src/core/{files,relay,dht,updates,agent,outbox,inbox}.rs`.
  Имена `relay_loop`/`agent_loop` через `#[path]` избегают коллизий с libcore imports.
- Backup codec/export/restore и v2/v3-тесты находятся в `core/src/backup.rs`.
- Предыдущие 56 IPC-команд уже были в `commands/{profiles,vault,contacts,identity,
  network,agent,groups,pins}.rs`. Их тела в прежней сессии сравнивались с HEAD.
- В file batch-логах Core/SessionManager есть фактический `lanes.len()` с
  пояснением `incl main`. Controller не менялся: PARTS_PER_LANE=4,
  FILE_LANES=4, WINDOW=16; рост по ACK/window, не по измеренному throughput.
  Live benchmark и доказанный прирост отсутствуют. Lanes общие по relay,
  Flow — по contact; это важно для будущих измерений.

## Незакрытые вопросы backup и Android

Backup: export всё ещё читает `read(&a.path).unwrap_or_default()` (relative path
и замаскированные ошибки); restore может оставить частичный профиль без общего
rollback; attachment paths абсолютные. Требуется валидация до записи, проверка
legacy contact remap/security defaults/symlink-границ и тесты отказов. Staging
rename нельзя внедрять без проверки device binding, SQLite handles и поведения
публикации существующей цели на Windows/Unix.

Android: app manifest/MainActivity изменены раньше (убраны автоматический
battery-opt request и Kotlin biometric bridge), но merged manifest не проверен.
Есть StagedShare/drainStagedShares без подтверждённого consumer; не удалять
вслепую. Проверить URI grants, очередь, notification permission retry,
библиотечные biometric/boot permissions. Старые отчёты Flint/WhiteRabbit не
считать доказательством корректности или неиспользуемости кода.

## Рабочее дерево

Много предсуществующих изменений UI/browser-runtime, Android, зависимостей,
DHT, relay, libcore, сайта и документации. Они сохранены. Удалённые UI-файлы
обычно соответствуют новым путям в `ui/src/app/`, `ui/src/features/`,
`ui/src/browser-core/{authorization,diagnostics,files,messaging,networking,profiles}`.
Не восстанавливать старые копии без проверки. Новые Rust/UI-модули и отчёты
ещё не в индексе; обычный git diff не показывает содержимое untracked файлов.
`third_party/i2pd` и `third_party/i2pd-android` грязные; не чистить автоматически.

## Последние локальные проверки

```bash
env -u CC -u CXX -u AR cargo check -p gipny --tests
env -u CC -u CXX -u AR cargo test -p gipny-libcore -p gipny -p i2p-embed --lib -- --test-threads=2
env -u CC -u CXX -u AR cargo test -p i2p-embed --lib upnp::fixture:: -- --test-threads=2
```

Итог: **30 gipny + 209 libcore + 32 i2p-embed успешных теста**. Общий запуск
в песочнице прошёл 30+209+29, три native UPnP HTTP fixture получили запрет TCP
bind (Operation not permitted). Только эти три теста повторены вне песочницы
с разрешением и прошли. cfg(gipny_upnp) действительно включён и miniupnpc
статически связан; это не stub build. После этого менялись лишь комментарии
и документация, смысловые определения повторно сравнивались.

Scoped rustfmt --check и whitespace checks затронутых областей успешны.
Предсуществующее предупреждение unused `recv` в libcore/relay_server сохранено.
CC/CXX/AR очищались только для host-команд: CC указывал на отсутствующий Android
compiler. Глобальную конфигурацию не меняли. Проверки других платформ,
frontend, полного workspace и live I2P не выполнялись.
Все изменения на диске, новые файлы не в индексе; commit/push/CI/release нет.
