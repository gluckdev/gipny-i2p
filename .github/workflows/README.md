# .github/workflows — CI, релизы, e2e по живой i2p и тестовый релей

Всё собирается только здесь, на раннерах GitHub: локальных сборок у проекта нет. Этот файл описывает и соседей: `.github/scripts/`, `.github/codeql/`, `.github/dependabot.yml`.

## Что здесь

| Файл | Когда запускается | Джобы | Что доказывает / производит |
|---|---|---|---|
| `build.yml` | push в `main`, любой PR, вручную | `rust workspace`, `macos (app + libcore tests)`, `rust tests (crypto/vault/kdf)`, `ui typecheck`, `android APK`, `android emulator smoke` | Наш код компилируется и тесты проходят. Роутер вкомпилирован в каждый бинарь (`i2p-embed`), поэтому десктопные и тестовые джобы собирают и libi2pd — против boost из дистрибутива (macOS — Homebrew). APK собираются с заглушкой прослойки (`I2P_EMBED_STUB=1`) и `ORG_GRADLE_PROJECT_skipRouter`: они проверяют сборку, упаковку и запуск, а не связь. Тайминги KDF попадают в summary джобы. |
| `cleanup.yml` | воскресной ночью и вручную | `delete stale artifacts and caches` | Хранилище Actions не растёт бесконечно: удаляет артефакты старше 7 дней и кеши, к которым не обращались 21 день. К 0.4.6 накопилось 1132 артефакта на 29,6 ГБ, а кеши упёрлись в потолок 10 ГБ и начали вытеснять друг друга — вместе с кешем роутера, ради которого всё и затевалось. |
| `codeql.yml` | push/PR в `main`, раз в неделю (пн) | `analyze (actions / javascript-typescript / rust)` | Статический анализ, `build-mode: none`. Настройки — `.github/codeql/codeql-config.yml` (тесты не сканируются: в фикстурах ключи зашиты намеренно). |
| `e2e-i2pd.yml` | каждую ночь, вручную, PR с изменениями в `third_party/**` или в самом файле | `i2pd revision under test` → `e2e (relay + two bots)`, `e2e (relays inside the bots)`, `e2e (agent binary)`, `e2e (relay network, each side away in turn)` → `record the proven revision` | Единственная проверка того, что сообщения реально доходят по i2p. Каждый бинарь вкомпилирует **голову ветки** сабмодуля, а не пин (`.github/scripts/i2pd-under-test.sh`); отдельного i2pd, SAM и портов нет. Четыре сценария: два `gipny-relay` + два бота; релеи внутри ботов (`E2E_IN_PROCESS_RELAYS`); настоящий `gipny-agent` (`E2E_AGENT_BIN`); сеть релеев со своим сидом `gipny-relay --dht` внутри джобы, стороны уходят по очереди (`E2E_DHT_OFFLINE`, `E2E_DHT_SEED_DEST`). Успех — строка `[e2e] SUCCESS` в логе харнесса. Джобы ~10–15 мин (лимит 45), сеть релеев ~16 мин (лимит 60). Пин i2pd двигает только первый сценарий. |
| `i2p-embed.yml` | push/PR, меняющие `i2p-embed/**`, `libcore/**`, `tests/e2e-harness/**`, `android-router/**` и скрипты сборки роутера | `live` (Linux), `jammy` (boost 1.74, static), `macos` (arm64 и intel), `windows` (MSVC, vcpkg), `android` (arm64: `libi2pd.so` + линковка), `e2e` (релеи внутри ботов, одновременная первая запись), `updater` (обновлятор через аутпрокси) | Роутер внутри процесса собирается и работает на всех платформах релиза: live-тест поднимает две destination и гоняет эхо по живой i2p. e2e выкладывает `i2pd.log` артефактом. `updater` — живая проверка обновлений: список релизов и файл с GitHub через аутпрокси; аутпрокси чужой, так что его падение ничего больше не блокирует. |
| `release.yml` | push в `main`, меняющий `core/tauri.conf.json` (подъём версии); вручную | `versions agree` → `network database snapshot`, `router (i2pd, android …)` → `desktop (…)`, `relay (linux …)`, `agent (linux …)`, `android` → `release page` | Все артефакты релиза и страница релиза. На подъёме версии в `main` собирает, **сам ставит тег `vX.Y.Z`** на собранный коммит и публикует (один раз на версию: если тег уже есть — только превью). При ручном запуске — только превью (артефакт `release-page-preview`). Холодная сборка ~30 мин, роутер Android с нуля — до часа. |
| `relay-testnet.yml` | вручную, каждые 6 ч | `relay` | **Временный** публичный `gipny-relay --dht` на раннере GitHub (~5,5 ч на запуск): и тестовый релей, и **сид сети релеев**. Идентичность, очередь, узел сети и роутер переносятся между запусками через `actions/cache` (`dht-seed-state-*`) **только зашифрованными `age`** (секрет `DHT_SEED_AGE_KEY`; без него сервис работает, но ничего не сохраняет, и адрес меняется каждый запуск). Адрес — в summary и в артефакте `relay-destination`; его вписывают в переменную `GIPNY_DHT_SEEDS`. |

**Скрипты и настройки рядом:**

- `.github/scripts/release-notes.sh <tag> <assets-dir> <owner/repo> [<git-ref>]` пишет текст страницы релиза. Сверху кладёт `docs/releases/<version>.md`, если файл есть, затем таблицу загрузок и changelog от предыдущего тега. Падает, если какого-то платформенного файла нет или среди файлов есть лишний.
- `.github/scripts/vcpkg-i2p-deps.sh` ставит boost и OpenSSL из vcpkg для Windows (MSVC) одинаково в `i2p-embed.yml` и `release.yml`.
- `.github/scripts/boost-po-pic.sh` собирает `boost_program_options` 1.74 статически с `-fPIC` для Linux-десктопа в `release.yml` (и проверки в `i2p-embed.yml` → `jammy`): крейт приложения — ещё и cdylib, а статическая boost из Ubuntu 22.04 на x86_64 собрана без `-fPIC` и в разделяемый объект не линкуется. Ключ `rust-cache` в релизных джобах — образ раннера: build-скрипты, собранные на 24.04 (`relay-testnet.yml`), на 22.04 не запускаются (GLIBC_2.39).
- `.github/scripts/i2pd-under-test.sh <sha>` ставит сабмодуль на проверяемую ревизию для всех джоб `e2e-i2pd.yml`.
- `scripts/fresh-i2p-certs.sh` кладёт текущие сертификаты reseed из апстрима; каждая джоба, которая компилирует роутер (релиз, `build.yml`, `i2p-embed.yml`, e2e, `relay-testnet.yml`), вкомпилирует их (`I2P_EMBED_CERTS_DIR`); без сертификатов сборка `i2p-embed` падает, даже с заглушкой.
- `.github/codeql/codeql-config.yml`: `paths-ignore: '**/tests/**'`.
- `.github/dependabot.yml`: еженедельные PR для cargo (корень и `core/relay`), npm (`ui`), github-actions и сабмодулей.

## Куда вносить правки

| Задача | Файл |
|---|---|
| Выпустить версию | Поднять версию в `core/Cargo.toml`, `agent/Cargo.toml`, `core/tauri.conf.json`, `ui/package.json` (и `ui/package-lock.json`), `Cargo.lock` (`gipny`, `gipny-agent`); строка в таблице версий `README.md`; написать `docs/releases/<версия>.md`; закоммитить в `main` и запушить — дальше `release.yml` всё делает сам, **тег руками не ставить**. Сиды сети релеев вшиваются из переменной репозитория `GIPNY_DHT_SEEDS` (весь `release.yml`, `env:`) |
| Текст страницы релиза | `docs/releases/<версия>.md` (свободный текст); общий шаблон, таблица загрузок, changelog — `.github/scripts/release-notes.sh` |
| Добавить или переименовать артефакт релиза | Джоба в `release.yml` + таблица платформ в `release-notes.sh` (иначе публикация упадёт) + имена ассетов, которые ищет автообновление (`libcore/src/update.rs`, `target_suffix`) |
| Пререлиз | Тег с дефисом (`v0.5.0-rc1`): `release.yml` ставит `prerelease` и не делает его «Latest» |
| Изменить сценарий e2e | Джобы в `e2e-i2pd.yml` + сам харнесс `tests/e2e-harness/` (переменные `E2E_*`) |
| Поменять, как двигается пин i2pd | Джоба `bump` («record the proven revision») в `e2e-i2pd.yml` |
| Сборка роутера под платформу | `i2p-embed/build.rs` (переменные `I2P_EMBED_*`), зависимости — шаги в `release.yml` и `i2p-embed.yml`; Android — `android-router/jni` и джоба `android-router` в `release.yml` |
| Проверки на каждый push | `build.yml` |
| Правила dependabot | `.github/dependabot.yml` (пример: `bincode` в основном workspace не поднимается выше 1.x, в `core/relay` — до 3.x) |
| Тестовый релей | `relay-testnet.yml` |
| Что сканирует CodeQL | `codeql.yml` (языки), `.github/codeql/codeql-config.yml` (пути) |
| Подпись Android | Секреты `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`; шаг `configure Android signing` в `release.yml` |

## Время сборки

Релиз собирает снимок netDb, две библиотеки для Android, пять десктопных
сборок, APK, релей и агента. Отдельных бинарей i2pd нет: каждый бинарь
вкомпилирует роутер. Что сделано, чтобы это не длилось час:

- **boost и OpenSSL для Windows (vcpkg) и Android (`android-deps-*`)
  кешируются** — это самая долгая часть холодной сборки; `libi2pd.so` для
  Android кешируется по пину i2pd, NDK и хэшу `android-router/jni/**` и
  `i2p-embed/shim/**`.
- **Rust и Gradle кешируются в сборке APK** (`Swatinem/rust-cache`, `~/.gradle`),
  а `tauri-cli` ставится готовым бинарём через `cargo binstall` вместо
  компиляции из исходников — и в `release.yml`, и в `build.yml`.
- **APK собирается по одной джобе на ABI** (`arm64-v8a`, `armeabi-v7a`).
  `ORG_GRADLE_PROJECT_routerAbis` содержит только ABI этой джобы.
- **Артефакты живут 5 дней** (debug-APK из `build.yml` — 3): это передача файлов
  между джобами, а всё, что нужно людям, лежит на странице релиза.
- Мерить так: `gh run view <id>` показывает время каждой джобы, а
  `gh api repos/<repo>/actions/runs/<id>/timing` — общее машинное время.

### Кеш достаётся только тому, кто запущен с `main`

Это правило GitHub, и оно не очевидно: прогон читает кеши **своей ссылки и ветки по
умолчанию**, а пишет всегда в свою. Значит:

- прогон **по тегу** сохраняет кеши под `refs/tags/vX.Y.Z`, и их после этого не может
  прочитать никто — ни следующий тег, ни `main`. В прогоне `35321193991` под мёртвым
  тегом так осело 2.4 ГБ, а все пять desktop-джоб честно написали «No cache found» и
  компилировали зависимости с нуля по 13+ минут;
- прогон **из PR** пишет почти-дубль кешей `main` по 1.1–1.5 ГБ на джобу. Лимит
  репозитория — 10 ГБ, вытеснение по LRU, и первыми вылетают маленькие кеши роутера
  `i2pd-*`, к которым обращается только релиз. Именно поэтому роутер пересобирался по
  3–9 минут при неподвижном пине.

Отсюда два следствия, которые надо помнить:

1. **`save-if: github.ref == 'refs/heads/main'`** на всех `Swatinem/rust-cache`. Читают
   по-прежнему все, пишет только `main`.
2. **Релиз собирается на `main`, а не по тегу** (с 2026-09-24). Раньше порядок был
   «сначала ручной прогон на `main`, потом тег», и шаг с ручным прогоном пропускали —
   тег стартовал с холодного кеша и собирал всё второй раз. Теперь подъём версии в
   `main` сам запускает сборку, её кеши остаются в `main`, а тег ставит джоба
   `release page`. Кеш boost/OpenSSL для Android (`android-deps-*`) отдельный от
   роутера и не выбрасывается при сдвиге пина i2pd или правке JNI.

Проверять так: `gh cache list` — суммарный объём должен оставаться ниже 10 ГБ, а ключи
`i2pd-*` должны в нём присутствовать. Если снова не влезает, следующие рычаги:
`shared-key` для джоб, собирающих один и тот же workspace; слияние трёх почти одинаковых
`v0-rust-e2e-*`; либо вынести бинарь роутера из `actions/cache` на скользящий
pre-release ассет, который не вытесняет ничто.

## Что менять вместе, инварианты и грабли

- **Версия.** Джоба `versions agree` сверяет версии в `core/tauri.conf.json`, `ui/package.json`, `core/Cargo.toml` и `agent/Cargo.toml` и падает первой, до долгих сборок. Поднимайте их в одном коммите. Публикуется версия один раз: если тег `vX.Y.Z` уже есть, прогон делает только превью. Руками тег не ставить: триггера по тегу больше нет, а при ручном теге страницу релиза никто не создаст.
- **Пин i2pd двигается только по доставке.** Все workflow, кроме `e2e-i2pd.yml`, собирают закреплённый коммит сабмодуля. `bump` коммитит новый пин, только если `e2e (relay + two bots)` доставил сообщения, и **пушит в ветку, на которой запущен workflow**. Ручной запуск на своей ветке может добавить в неё коммит пина. Push в эту ветку, пока идёт `bump`, отклонится — сделайте `git pull --rebase` и пушьте снова.
- **`gh` CLI на этой машине не может запускать workflow** (`HTTP 403` на `gh workflow run`, `gh run cancel`, `gh release edit`, `gh pr create`). Ручной запуск — через страницу Actions в браузере: *workflow → Run workflow → ветка → Run workflow*. PR и merge работают через GitHub MCP. Чтение (`gh run list/view`, `gh pr checks`, `gh api …/actions/jobs/<id>/logs`) работает.
- **Сборка релиза для Android требует секретов подписи.** Без них шаг `configure Android signing` падает на `test -n`.
- **`release-notes.sh` строгий:** новый файл в `dist/` без строки в таблице, как и строка без файла, валят публикацию.
- **Кэш `relay-testnet` читается из PR-workflow** (особенность скоупа кэша), поэтому состояние сида лежит там только зашифрованным `age`. Старые кэши `relay-testnet-state-*` держали ключ открытым — сид начинает с нового ключа. Две копии одного адреса одновременно — конфликт LeaseSet, поэтому стоит `concurrency: relay-testnet`.
- **`GIPNY_DHT_SEEDS` читается только при сборке** (`option_env!` в `libcore/src/dht_client.rs`): клиенты не берут сиды из окружения при запуске. e2e передаёт свой сид ботам через их таблицу узлов.
- **В роутер входит только `libi2pd`**: ни `libi2pd_client` (SAM, прокси, BitTorrent с `boost_json`), ни демон. Поэтому `TORRENTS=no` больше не нужен.
- **Боевые APK с роутером — только из `release.yml`.** APK из `build.yml` линкуют заглушку прослойки и не подключаются к сети по построению.
- **Секреты и переменные Actions токену `gh` на этой машине недоступны** (403): `DHT_SEED_AGE_KEY` и `GIPNY_DHT_SEEDS` вписывает владелец.

## Как проверить

- Статус: `gh run list --limit 10`, `gh run view <id> --json jobs`, `gh pr checks <номер>`.
- Лог джобы, в том числе ещё идущей: `gh api repos/gluckdev/gipny-i2p/actions/jobs/<job-id>/logs`.
- Доставку по живой i2p проверяет ручной запуск `e2e-i2pd` на нужной ветке (через страницу Actions). Смотреть на `[e2e] SUCCESS` в логе.
- Страницу релиза без публикации даёт ручной запуск `release` (артефакт `release-page-preview`).
- Синтаксис YAML локально: `python3 -c "import yaml,sys; yaml.safe_load(open(sys.argv[1]))" .github/workflows/<файл>.yml`.
- **Скрипт `android emulator smoke` — в файле `.github/scripts/android-smoke.sh`, а не в `script:`.** `reactivecircus/android-emulator-runner` выполняет каждую строку `script` отдельной `sh -c`, и переменные между строками теряются (так `adb install` получал пустой путь). Поэтому в `script:` — одна команда, а джобе нужен `actions/checkout`.
