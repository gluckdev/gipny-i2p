# .github/workflows — CI, релизы, e2e по живой i2p и тестовый релей

Всё собирается только здесь, на раннерах GitHub: локальных сборок у проекта нет. Этот файл описывает и соседей: `.github/scripts/`, `.github/codeql/`, `.github/dependabot.yml`.

## Что здесь

| Файл | Когда запускается | Джобы | Что доказывает / производит |
|---|---|---|---|
| `build.yml` | push в `main`, любой PR, вручную | `rust workspace`, `macos (app + libcore tests)`, `rust tests (crypto/vault/kdf)`, `ui typecheck`, `android APK`, `android emulator smoke` | Наш код компилируется и тесты проходят. Роутер не собирается: вместо `core/resources/i2pd` кладётся заглушка. APK собираются с `ORG_GRADLE_PROJECT_skipRouter`, без i2pd — они проверяют сборку и упаковку, а не связь. Тайминги KDF попадают в summary джобы. |
| `cleanup.yml` | воскресной ночью и вручную | `delete stale artifacts and caches` | Хранилище Actions не растёт бесконечно: удаляет артефакты старше 7 дней и кеши, к которым не обращались 21 день. К 0.4.6 накопилось 1132 артефакта на 29,6 ГБ, а кеши упёрлись в потолок 10 ГБ и начали вытеснять друг друга — вместе с кешем роутера, ради которого всё и затевалось. |
| `codeql.yml` | push/PR в `main`, раз в неделю (пн) | `analyze (actions / javascript-typescript / rust)` | Статический анализ, `build-mode: none`. Настройки — `.github/codeql/codeql-config.yml` (тесты не сканируются: в фикстурах ключи зашиты намеренно). |
| `e2e-i2pd.yml` | каждую ночь, вручную, PR с изменениями в `third_party/**` или в самом файле | `router (i2pd, linux/amd64)` → `e2e (relay + two bots)`, `e2e (relays inside the bots)`, `e2e (agent binary)` → `record the proven revision` | Единственная проверка того, что сообщения реально доходят по i2p. Роутер собирается из **головы ветки** сабмодуля, а не из пина. Три сценария: два `gipny-relay` + два бота; релеи внутри ботов (`E2E_IN_PROCESS_RELAYS`); настоящий `gipny-agent` (`E2E_AGENT_BIN`). Успех — строка `[e2e] SUCCESS` в логе харнесса. Каждая джоба ~10–15 мин, лимит 30. |
| `i2pd-build.yml` | вручную, раз в неделю (пн) | linux x86_64 (static, static musl), linux arm64 musl, macos, windows, android по трём ABI | «Широкая сеть»: апстрим i2pd всё ещё собирается везде, включая платформы, которых нет в релизе. Строит **пин** сабмодулей. Android — до 120 мин. |
| `release.yml` | push тега `v*`, вручную | `version matches the tag` → `router (i2pd, …)`, `router (i2pd, android …)` → `desktop (…)`, `relay (linux …)`, `agent (linux …)`, `android` → `release page` | Все артефакты релиза и страница релиза. На теге публикует; при ручном запуске делает только превью (артефакт `release-page-preview`). Последний релиз шёл ~20 мин. |
| `relay-testnet.yml` | вручную, каждые 6 ч | `relay` | **Временный** публичный `gipny-relay` на раннере GitHub (~5,5 ч на запуск). Идентичность, очередь и состояние роутера переносятся между запусками через `actions/cache` (`relay-testnet-state-*`), поэтому адрес постоянный. Адрес — в summary и в артефакте `relay-destination`. |

**Скрипты и настройки рядом:**

- `.github/scripts/release-notes.sh <tag> <assets-dir> <owner/repo> [<git-ref>]` пишет текст страницы релиза. Сверху кладёт `docs/releases/<version>.md`, если файл есть, затем таблицу загрузок и changelog от предыдущего тега. Падает, если какого-то платформенного файла нет или среди файлов есть лишний.
- `.github/scripts/build-i2pd-macos.sh` собирает роутер для macOS одинаково в `i2pd-build.yml` и `release.yml`.
- `.github/codeql/codeql-config.yml`: `paths-ignore: '**/tests/**'`.
- `.github/dependabot.yml`: еженедельные PR для cargo (корень и `core/relay`), npm (`ui`), github-actions и сабмодулей.

## Куда вносить правки

| Задача | Файл |
|---|---|
| Выпустить версию | Поднять версию в `core/Cargo.toml`, `agent/Cargo.toml`, `core/tauri.conf.json`, `ui/package.json` (и `ui/package-lock.json`); написать `docs/releases/<версия>.md`; закоммитить в `main`; `git tag -a vX.Y.Z` и `git push origin vX.Y.Z` |
| Текст страницы релиза | `docs/releases/<версия>.md` (свободный текст); общий шаблон, таблица загрузок, changelog — `.github/scripts/release-notes.sh` |
| Добавить или переименовать артефакт релиза | Джоба в `release.yml` + таблица платформ в `release-notes.sh` (иначе публикация упадёт) + имена ассетов, которые ищет автообновление (`libcore/src/update.rs`, `target_suffix`) |
| Пререлиз | Тег с дефисом (`v0.5.0-rc1`): `release.yml` ставит `prerelease` и не делает его «Latest» |
| Изменить сценарий e2e | Джобы в `e2e-i2pd.yml` + сам харнесс `tests/e2e-harness/` (переменные `E2E_*`) |
| Поменять, как двигается пин i2pd | Джоба `bump` («record the proven revision») в `e2e-i2pd.yml` |
| Сборка роутера под платформу | `i2pd-build.yml` (проверка) и джобы `router` / `android-router` в `release.yml` (то, что поставляется); macOS — `.github/scripts/build-i2pd-macos.sh` |
| Проверки на каждый push | `build.yml` |
| Правила dependabot | `.github/dependabot.yml` (пример: `bincode` в основном workspace не поднимается выше 1.x, в `core/relay` — до 3.x) |
| Тестовый релей | `relay-testnet.yml` |
| Что сканирует CodeQL | `codeql.yml` (языки), `.github/codeql/codeql-config.yml` (пути) |
| Подпись Android | Секреты `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`; шаг `configure Android signing` в `release.yml` |

## Время сборки

Релиз собирает пять роутеров i2pd из исходников, две библиотеки для Android,
пять десктопных сборок, APK, релей и агента. Что сделано, чтобы это не длилось
час:

- **Роутер кешируется по ревизии сабмодуля** (`actions/cache`, ключ
  `i2pd-<платформа>-<ревизия>`), как это давно сделано для Android. Пин двигается
  только когда e2e доказал доставку, поэтому обычный релиз собирает роутер из
  кеша за секунды вместо 4–11 минут на каждую платформу. На Windows при попадании
  в кеш пропускается и установка MSYS2 с boost.
- **Rust и Gradle кешируются в сборке APK** (`Swatinem/rust-cache`, `~/.gradle`),
  а `tauri-cli` ставится готовым бинарём через `cargo binstall` вместо
  компиляции из исходников — и в `release.yml`, и в `build.yml`.
- **APK собирается по одной джобе на ABI** (`arm64-v8a`, `armeabi-v7a`). Вместе
  они были самой длинной частью релиза (~15 минут на две подряд), а общего у них
  ничего нет: свой тулчейн, свой роутер, свой APK. `ORG_GRADLE_PROJECT_routerAbis`
  теперь содержит только ABI этой джобы, иначе Gradle стажировал бы и чужой.
- **Артефакты живут 5 дней** (debug-APK из `build.yml` — 3): это передача файлов
  между джобами, а всё, что нужно людям, лежит на странице релиза. По умолчанию
  GitHub хранит их 90 дней.
- **Что осталось последовательным:** `desktop`, `relay` и `agent` ждут *всю*
  матрицу `router`, а не свою платформу — GitHub не умеет зависеть от одной ветки
  матрицы. При попадании в кеш это секунды; заметно только когда пин роутера
  только что сдвинули. Если станет мешать — разносить матрицу на отдельные джобы.
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
2. **Порядок релиза: сначала ручной прогон `release.yml` с `main`, потом тег.** Ручной
   прогон собирает всё и останавливается, не публикуя, — и заодно кладёт кеши в ту
   единственную область, откуда прогон по тегу сможет их взять. Без этого шага тег
   всегда стартует с холодного кеша.

Проверять так: `gh cache list` — суммарный объём должен оставаться ниже 10 ГБ, а ключи
`i2pd-*` должны в нём присутствовать. Если снова не влезает, следующие рычаги:
`shared-key` для джоб, собирающих один и тот же workspace; слияние трёх почти одинаковых
`v0-rust-e2e-*`; либо вынести бинарь роутера из `actions/cache` на скользящий
pre-release ассет, который не вытесняет ничто.

## Что менять вместе, инварианты и грабли

- **Версия и тег.** Джоба `version matches the tag` сравнивает тег с версиями в `core/tauri.conf.json`, `ui/package.json` и `core/Cargo.toml` и падает первой, до многочасовых сборок. Поднимайте все три (плюс `agent/Cargo.toml`) в одном коммите.
- **Пин i2pd двигается только по доставке.** Все workflow, кроме `router` в `e2e-i2pd.yml`, собирают закреплённый коммит сабмодуля. `bump` коммитит новый пин, только если `e2e (relay + two bots)` доставил сообщения, и **пушит в ветку, на которой запущен workflow**. Ручной запуск на своей ветке может добавить в неё коммит пина. Push в эту ветку, пока идёт `bump`, отклонится — сделайте `git pull --rebase` и пушьте снова.
- **`gh` CLI на этой машине не может запускать workflow** (`HTTP 403` на `gh workflow run`, `gh run cancel`, `gh release edit`, `gh pr create`). Ручной запуск — через страницу Actions в браузере: *workflow → Run workflow → ветка → Run workflow*. PR и merge работают через GitHub MCP. Чтение (`gh run list/view`, `gh pr checks`, `gh api …/actions/jobs/<id>/logs`) работает.
- **Сборка релиза для Android требует секретов подписи.** Без них шаг `configure Android signing` падает на `test -n`.
- **`release-notes.sh` строгий:** новый файл в `dist/` без строки в таблице, как и строка без файла, валят публикацию.
- **Кэш `relay-testnet` читается из PR-workflow** (особенность скоупа кэша): этот адрес нельзя считать доверенной инфраструктурой. Две копии одного адреса одновременно — конфликт LeaseSet, поэтому стоит `concurrency: relay-testnet`.
- **`i2pd-build.yml` собирает `TORRENTS=no`:** в апстриме появился BitTorrent-клиент, тянущий `boost_json`. `e2e-i2pd.yml` и `relay-testnet.yml` тоже собирают без него.
- **Боевые APK с роутером — только из `release.yml`.** APK из `build.yml` без `libi2pd.so` не подключаются к сети по построению.

## Как проверить

- Статус: `gh run list --limit 10`, `gh run view <id> --json jobs`, `gh pr checks <номер>`.
- Лог джобы, в том числе ещё идущей: `gh api repos/gluckdev/gipny-i2p/actions/jobs/<job-id>/logs`.
- Доставку по живой i2p проверяет ручной запуск `e2e-i2pd` на нужной ветке (через страницу Actions). Смотреть на `[e2e] SUCCESS` в логе.
- Страницу релиза без публикации даёт ручной запуск `release` (артефакт `release-page-preview`).
- Синтаксис YAML локально: `python3 -c "import yaml,sys; yaml.safe_load(open(sys.argv[1]))" .github/workflows/<файл>.yml`.
- **Скрипт `android emulator smoke` — в файле `.github/scripts/android-smoke.sh`, а не в `script:`.** `reactivecircus/android-emulator-runner` выполняет каждую строку `script` отдельной `sh -c`, и переменные между строками теряются (так `adb install` получал пустой путь). Поэтому в `script:` — одна команда, а джобе нужен `actions/checkout`.
