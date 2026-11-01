> Требование владельца от 2026-10-28: все дальнейшие сборки только в GitHub Actions,
> из публичных исходников. GitLab относится к AiBlow. Полный контракт source archive,
> provenance, SDK и границ проверки: [docs/build-release.md](../../docs/build-release.md).

# .github/workflows — CI, релизы, e2e по живой i2p и тестовый релей

Всё собирается только здесь, на раннерах GitHub: локальных сборок у проекта нет. Этот файл описывает и соседей: `.github/scripts/`, `.github/codeql/`, `.github/dependabot.yml`.

## Что здесь

| Файл | Когда запускается | Джобы | Что доказывает / производит |
|---|---|---|---|
| `browser-router.yml` | push/PR с изменением `browser-router/**` или workflow; вручную | `wasm`, `NTCP2 browser to native and public I2P` | `b912f01`: run `41272794574` PASS — `--locked` WASM build, 9 Chrome runtime + 5 Python forwarder + 3 Chrome NTCP2 tests. Native i2pd собирается из pinned C++; публичный пэр берётся из свежего verified netDb. Артефакты: WASM/size/SHA256/source/license и public NTCP2 evidence; private keys/capability не сохраняются. Это живой handshake, полный Router и exploratory ещё не проверены. |
| `pages.yml` | PR/push main с изменением `site/**`, `ui/**` или workflow; вручную; успешный release main | `Validate and build static site` → main-only deploy | На 2026-10-31 landing сохраняет CSP `connect-src 'none'`; job собирает UI с `VITE_BASE=/gipny-i2p/app/`, запускает `npm test` и копирует результат в `_site/app/`. Проверка Chrome относится к landing, не к HTTPS → loopback, локальной сети или I2P. |
| `security-fast.yml` | PR/push `main` с изменением Rust/UI и вручную | `unit + callers` на Linux, Windows, macOS arm64/Intel; `Android Rust + JNI types` | На 2026-10-31 Linux также запускает `cargo test --locked -p gipny --features web-gateway --test web_local`: HTTP Host/headers, capability/Origin WebSocket, фиксированный порт и RPC до unlock на stub; затем отдельную `--lib web_server::tests::inline_attachments_follow_the_desktop_privacy_setting` регрессию sanitizer без Core. Быстрая компиляция callers и unit tests memory/device-binding/vault/card/deletion без C++ роутера (`I2P_EMBED_STUB=1`). Android job проверяет реальные JNI types для arm64. Shipping artifacts не создаёт; не заменяет full native/I2P, hardware Keystore и установку на устройстве. Kotlin instrumentation sources отдельно собирает `build.yml`, без заявления о прохождении TEE на эмуляторе. |
| `build.yml` | push в `main`, любой PR, вручную | `rust workspace`, `macos (app + libcore tests)`, `rust tests (crypto/vault/kdf)`, `ui typecheck` (включая endpoint/pairing unit tests), `android APK`, `android emulator smoke` | Наш код компилируется и тесты проходят. Роутер вкомпилирован в каждый бинарь (`i2p-embed`), поэтому десктопные и тестовые джобы собирают и libi2pd — против boost из дистрибутива (macOS — Homebrew). APK собираются с заглушкой прослойки (`I2P_EMBED_STUB=1`) и `ORG_GRADLE_PROJECT_skipRouter`: они проверяют сборку, упаковку и запуск, а не связь. Тайминги KDF попадают в summary джобы. |
| `cleanup.yml` | воскресной ночью и вручную | `delete stale artifacts and caches` | Хранилище Actions не растёт бесконечно: удаляет артефакты старше 7 дней и кеши, к которым не обращались 21 день. К 0.4.6 накопилось 1132 артефакта на 29,6 ГБ, а кеши упёрлись в потолок 10 ГБ и начали вытеснять друг друга — вместе с кешем роутера, ради которого всё и затевалось. |
| `codeql.yml` | push/PR в `main`, раз в неделю (пн) | `analyze (actions / javascript-typescript / rust)` | Статический анализ, `build-mode: none`. Настройки — `.github/codeql/codeql-config.yml` (тесты не сканируются: в фикстурах ключи зашиты намеренно). |
| `e2e-i2pd.yml` | каждую ночь, вручную, PR с изменениями router/workflow | `i2pd revision under test` → четыре live e2e jobs → `record the proven revision` | PR и обычный manual проверяют **committed pin**; schedule и явный `test_upstream=true` разрешают upstream probe. Один SHA передаётся всем четырём сценариям через `.github/scripts/i2pd-under-test.sh`: внешние релеи/боты, встроенные релеи, agent, DHT с уходом сторон и новыми адресами. Отдельного i2pd/SAM/local listener нет. Успех — `[e2e] SUCCESS`; подтверждение shipping pin отличается от успеха upstream probe. Bump допустим только upstream-режиму после успеха всех четырёх сценариев; pinned checks не меняют pin. |
| `i2p-embed.yml` | push/PR, меняющие `i2p-embed/**`, `libcore/**`, `tests/e2e-harness/**`, `android-router/**` и скрипты сборки роутера | `live` (Linux), `jammy` (boost 1.74, static), `macos` (arm64 и intel), `windows` (MSVC, vcpkg), `android` (arm64: `libi2pd.so` + линковка), `e2e` (релеи внутри ботов, одновременная первая запись), `updater` (обновлятор через аутпрокси) | Роутер внутри процесса собирается и работает на всех платформах релиза: live-тест поднимает две destination и гоняет эхо по живой i2p. e2e выкладывает `i2pd.log` артефактом. `updater` — живая проверка обновлений: список релизов и файл с GitHub через аутпрокси; аутпрокси чужой, так что его падение ничего больше не блокирует. На ветку — один прогон, последний (`concurrency` по имени ветки): push и pull_request одной ветки больше не гоняют всё дважды. |
| `release.yml` | push в `main`, меняющий `core/tauri.conf.json` (подъём версии); вручную | `versions agree` → `network database snapshot`, `router (i2pd, android …)` → `desktop (…)`, `relay (linux …)`, `agent (linux …)`, `android` → `release page` | Все артефакты релиза и страница релиза. На подъёме версии в `main` собирает, **сам ставит тег `vX.Y.Z`** на собранный коммит и публикует один раз. Ручной запуск по умолчанию — превью; явный `publish=true` только с `main` продолжает неопубликованную версию через ту же полную матрицу и строгие draft/asset guards. Холодная сборка ~30 мин, роутер Android с нуля — до часа. |
| `relay-testnet.yml` | push в `main`, меняющий `core/relay/**`, `libcore/**`, `third_party/i2pd`, `scripts/**` или сам workflow; по расписанию каждые 5 ч; вручную | `relay` | **Временный** публичный `gipny-relay --dht` на раннере GitHub (~5,5 ч на запуск): и тестовый релей, и **сид сети релеев**. Идентичность, очередь, узел сети и роутер переносятся между запусками через `actions/cache` (`dht-seed-state-*`) **только зашифрованными `age`** (секрет `DHT_SEED_AGE_KEY`; без него сервис работает, но ничего не сохраняет, и адрес меняется каждый запуск). Адрес — в summary и в артефакте `relay-destination`; его вписывают в переменную `GIPNY_DHT_SEEDS`. |

`i2p-embed.yml` также запускается при изменении корневых `Cargo.toml` и
`Cargo.lock`, чтобы обновление TLS-зависимости не пропустило native проверки.
Live updater собирается с `--locked`. Этот job имеет `continue-on-error`
из-за внешнего outproxy: при TLS-изменении проверяй именно его фактический
SUCCESS, а не только общий зелёный статус workflow.

`security-fast.yml` дополнительно выполняет реальный macOS local Keychain
enrollment/read/copy-isolation/delete под бесплатной ad-hoc подписью в двух
временных OS Keychain. На Linux отдельная регрессия создаёт настоящий
non-dumpable child с loopback listener и проверяет узкий metadata observer;
это не native I2P transport test. Fixtures не являются fallback приложения:
они не публикуют OS credentials и не запускают production agent от root.

Build/CodeQL badges README явно обозначают `main`; статус открытого PR смотрят
по его checks. Успех отдельного e2e «5/5» не заменяет полный build/release matrix.
Недоступный hosted runner или отменённая очередь не являются выполненным тестом.
`build.yml`, `e2e-i2pd.yml` и `codeql.yml` отменяют устаревший прогон той же ветки,
сохраняя ресурсы для последнего изменения. Release preview остаётся отдельным
ручным запуском с `publish=false` без публикации тега.

**Скрипты и настройки рядом:**

- `.github/scripts/release-notes.sh <tag> <assets-dir> <owner/repo> [<git-ref>]` пишет текст страницы релиза. Сверху кладёт `docs/releases/<version>.md`, если файл есть, затем таблицу загрузок и changelog от предыдущего тега. Падает, если какого-то платформенного файла нет или среди файлов есть лишний.
- `.github/scripts/release-guard.py` проверяет main-only eligibility, отличие отсутствующего тега от ошибки запроса, актуальность собранного SHA, точную привязку draft и полный remote asset set. `test-release-guard.py` исполняется в GitHub CI на Linux/Windows и перед release matrix; API mutations тестам не нужны. Восстановление неопубликованной версии — [контракт](../../docs/build-release.md#как-запускать-и-проверять).
- `.github/scripts/vcpkg-i2p-deps.sh` собирает boost и OpenSSL из pinned public vcpkg ports для Windows (MSVC) одинаково в `i2p-embed.yml` и `release.yml`; проверяет commit object/HEAD, binary registry отключён.
- `.github/scripts/boost-po-pic.sh` собирает `boost_program_options` 1.74 статически с `-fPIC` для Linux-десктопа в `release.yml` (и проверки в `i2p-embed.yml` → `jammy`): крейт приложения — ещё и cdylib, а статическая boost из Ubuntu 22.04 на x86_64 собрана без `-fPIC` и в разделяемый объект не линкуется. Ключ `rust-cache` в релизных джобах — образ раннера: build-скрипты, собранные на 24.04 (`relay-testnet.yml`), на 22.04 не запускаются (GLIBC_2.39).
- `.github/scripts/i2pd-under-test.sh <sha>` ставит сабмодуль на проверяемую ревизию для всех джоб `e2e-i2pd.yml`.
- `scripts/fresh-i2p-certs.sh` кладёт текущие сертификаты reseed из апстрима; каждая джоба, которая компилирует роутер (релиз, `build.yml`, `i2p-embed.yml`, e2e, `relay-testnet.yml`), вкомпилирует их (`I2P_EMBED_CERTS_DIR`); без сертификатов сборка `i2p-embed` падает, даже с заглушкой.
- `.github/codeql/codeql-config.yml`: `paths-ignore: '**/tests/**'`.
- `.github/dependabot.yml`: еженедельные PR для cargo (корень и `core/relay`), npm (`ui`), github-actions и сабмодулей. Правила `ignore`: `dtolnay/rust-toolchain` (версия Rust закреплена вручную, тег действия — это версия Rust), мажорные bincode в обоих cargo-каталогах (смена wire-формата, не обновление зависимости) и minor-версии `jni` 0.x (ломающее изменение API — поднимать вместе с портом кода).

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
  кешируются** — для Android сохраняются исходники `deps/boost`, сборка
  OpenSSL `deps/openssl/out/<abi>` и архивы `deps/*.tar.gz` из `fetch_deps.sh`.
  Ключ включает NDK, ABI, commit i2pd-android и хэш `fetch_deps.sh`/`build_openssl.sh`;
  Boost program_options и `libi2pd.so` каждый раз собираются из исходников;
  его manifest и SHA256 проверяются перед упаковкой APK.
- **Rust и Gradle кешируются в сборке APK** (`Swatinem/rust-cache`, `~/.gradle`),
  `tauri-cli 2.12.2` компилируется из публичного crate через `cargo install --locked`.
  Fallback на готовый CLI отсутствует. Rust закреплён на `1.93.1`.
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

- **Версия.** Джоба `versions agree` сверяет версии в `core/tauri.conf.json`, `ui/package.json`, `core/Cargo.toml` и `agent/Cargo.toml` и падает первой, до долгих сборок. Поднимайте их в одном коммите. Публикуется версия один раз: push с уже существующим тегом делает только превью, явный `publish=true` отклоняется. Для исправленного SHA ещё не опубликованной версии используйте `publish=true` с `main`, сохраняя номер и все проверки. Руками тег не ставить: триггера по тегу больше нет.
- **Пин i2pd двигается только после всех четырёх live сценариев.** PR и обычный manual компилируют committed pin без его изменения. Schedule или явный `test_upstream=true` проверяют upstream; лишь после всех четырёх PASS job `bump` создаёт коммит от проверенного application SHA и делает обычный non-force push. Если ветка успела измениться, push отказывает: proof не присваивается молча другой ревизии.
- **Права GitHub проверяются по текущему API-ответу.** Старые 403 относились к прежнему token/session. В сессии интеграции `gh api` успешно обновлял PR, запускал manual preview и отменял устаревшие runs. Не переносите старый отказ на новый токен и не обещайте доступ к secrets без отдельной проверки. Запуск доступен также через Actions → Run workflow; merge/publish выполняются в пределах задачи владельца.
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


Проверка UI включает `npm run build` и `npm test` (джоба
`ui typecheck`): оба языка, буквальные параметры, переключение
и сохранение черновика в DOM. Она не требует роутера или браузерного бинаря;
методика и пределы — [локализация](../../docs/localization.md).

## Проверка происхождения релиза

`release.yml` создаёт committed source archive всех рекурсивных сабмодулей,
`SOURCE-MANIFEST.json` и per-job build manifests. Финальная collection требует
совпадения workflow Git SHA, SHA256/размера каждого файла и полной матрицы assets;
создаёт `BUILD-MANIFEST.json` и `SHA256SUMS.txt`. Tauri не может молча изменить
reviewed Cargo/npm lockfiles. `I2P_EMBED_STUB`, `I2P_EMBED_SKIP_NATIVE` и release
assertions блокируются shipping gate. Debug stub APK из `build.yml` не являются
релизами. Проверки manifest на синтетических данных выполняет
`.github/scripts/test-build-provenance.py`; они не компилируют приложение.

Windows preview выявил exact-byte drift `Cargo.lock` после успешной сборки
приложения и агента. Теперь все три reviewed lockfiles имеют `text eol=lf`
в `.gitattributes`; Windows `core.autocrlf` не должен менять их байты.
`build-provenance.py locks` проверяет каждый файл сразу после checkout,
а `record` повторяет строгую проверку после build. Отсутствующий файл, CRLF
и содержательное изменение — ошибки; нормализация при приёмке не разрешена.
Диагностика различает CRLF и content drift, не объявляя причиной старого
прогона гипотезу без его checkout snapshot. Tauri получает Cargo `--locked`
после разделителя: `cargo tauri build --ci --bundles … -- --locked`.
Synthetic regressions и реальные checkout bytes проверяются в installer CI
на Linux и Windows; локальной сборкой приложения это не заменяют.

Windows source builds вызывают `.github/scripts/windows-source-tools.ps1`:
установленный Microsoft `vswhere` выбирает VC x64, затем штатный
`Launch-VsDevShell.ps1` с явными host/target amd64. Helper проверяет compiler
и linker, переносит только allowlist build-tool переменных и добавленные
PATH entries, сохраняет native Strawberry Perl. Полный process environment
не экспортируется. Это заменяет `ilammy/msvc-dev-cmd@v1`, чей опубликованный
action ещё использует deprecated Node 20; совместимость нового helper
подтверждается следующими Windows CI builds, не чтением PowerShell source.
