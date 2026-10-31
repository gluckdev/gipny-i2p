# core — приложение gipny-i2p на Tauri (десктоп и Android)

Крейт `gipny`. Rust-часть приложения: `Core` (логика клиента поверх `libcore`), команды для интерфейса из `ui/`, запуск профиля. Рядом лежат `relay/` (выделенный релей, свой README) и `gen/android/` (Android-обёртка, свой README).

## gipny-web

Состояние на **2026-10-31**, уточнено **2026-11-01** после review T9b: экспериментальный локальный веб-режим, отдельный
бинарь за feature `web-gateway`. Решение владельца: браузер служит интерфейсом,
ключи, криптография, хранилище и встроенный I2P остаются в нативном процессе
на машине пользователя. Хостинг не должен получать переписку, даже шифротекст.
Это проверка реализуемости, не релиз и не доказанная замена desktop-приложения.
Shipping-сборки выполняются только в GitHub Actions; бинарь не входит в обычный
desktop bundle.

Запуск уже собранного бинаря из корня с готовым `ui/dist`:

```sh
gipny-web --static ./ui/dist
gipny-web --pages
gipny-web --port 8080 --no-open
gipny-web --pages https://example.org/app/ --allow-origin https://example.org
gipny-web --data-dir /path/to/separate/test-data --no-open
```

Без параметров адреса процесс слушает `127.0.0.1:0`: ОС назначает порт.
`--port N` закрепляет локальный порт; `--port` и `--listen` несовместимы.
Явный loopback `--listen` также сохраняет все local-защиты, каталог desktop и
обычные хопы клиента. Loopback-адреса, включая `::1`, нормализуются в `127.0.0.1`. В stderr
печатается приватная ссылка `http://127.0.0.1:<port>/#token=<token>`; процесс
открывает её через `xdg-open`, `open` или Windows `cmd /c start`. `--no-open`
оставляет только печать. `--pages [URL]` открывает по умолчанию
`https://gluckdev.github.io/gipny-i2p/app/#port=<port>&token=<token>`.
Fragment не отправляется HTTP-серверу. UI удаляет его из адресной строки,
открывает WebSocket только к loopback и сохраняет порт/токен в `sessionStorage`
вкладки только после успешного `web_info`. Перезагрузка использует эти данные, новая вкладка требует сопряжения.
Браузеры могут восстанавливать sessionStorage вместе с закрытой сессией:
перезапуск нативного процесса всё равно меняет токен.

Если данных сопряжения нет или соединение отклонено, экран до запуска профиля
предлагает ввести порт и токен из напечатанной ссылки. После трёх быстрых
разрывов автоматические переподключения прекращаются и UI возвращается к
сопряжению. Для своего статического хоста нужно явно доверить его Origin через
повторяемый `--allow-origin`; один `--pages URL` доверия не добавляет.
Origin должен содержать `http(s)://host[:port]` без пути, даже завершающего `/`,
query, fragment или userinfo; регистр нормализуется. Неверный аргумент — ошибка.
Отказ автоматического сопряжения виден в форме; отказ submit останавливает его
переподключения и не стирает поля. Уведомления браузера запрашиваются при первой
отправке по действию пользователя, не при bootstrap; подсказка появляется при `denied`.

Локальный режим использует `resolve_base_dir()` desktop-приложения:
Linux `$XDG_DATA_HOME/gipny-i2p` либо `~/.local/share/gipny-i2p`, Windows
`%APPDATA%/gipny-i2p`, macOS `~/Library/Application Support/gipny-i2p`.
`--data-dir` и `GIPNY_DATA_DIR` переопределяют каталог. `--static` и
`GIPNY_WEB_STATIC` задают каталог UI. `GIPNY_WEB_LISTEN` больше не включает
публичный listener: legacy-режим требует явного non-loopback `--listen`.
Локальный режим удаляет `GIPNY_SERVER_HOPS` из окружения и не применяет серверное
правило одного хопа; действуют обычные настройки клиента.

Vault-lock в libcore защищает только потоки одного процесса. В base dir
остаётся переиспользуемый `instance.lock`: guard держит открытый `File` с OS
advisory lock (`File::try_lock`), без PID и без удаления файла. Kernel снимает
lock при завершении процесса, включая `process::exit`, SIGKILL и аварию;
наличие файла само по себе не означает занятость и ручная очистка не нужна.
Старый sentinel при успешном захвате очищается. `gipny-web` отказывает, если
lock взять нельзя, и называет каталог. Desktop берёт его best-effort: при
конкуренции или другой ошибке пишет warning и продолжает прежний запуск;
desktop-vs-desktop остаётся за single-instance plugin. Поэтому lock не даёт
гарантии взаимного исключения web/desktop, если desktop запускается вторым.
Mobile entrypoint не изменён. Реальный lifecycle разных ОС ещё требует проверки.

Граница доступа в local mode:

- Новый токен на каждый запуск: 32 байта системного RNG (`rand`), base64url без
  padding (`base64`, уже используемые libcore). `/ws?token=...` сравнивает токен
  циклом без досрочного выхода по содержимому; неверный/отсутствующий → 401.
- Origin обязателен: loopback с текущим портом, `https://gluckdev.github.io`
  и явные `--allow-origin`. Другой/отсутствующий → 403. Весь Pages origin
  доверен, включая другие project sites того же владельца.
- Каждый маршрут, включая `/health` и static fallback, проверяет Host:
  `127.0.0.1:<port>` или `localhost:<port>`; другой → 403 (DNS rebinding).
- Ответы получают `Cache-Control: no-store`, `Referrer-Policy: no-referrer`,
  `X-Content-Type-Options: nosniff`. HTML получает CSP как в Tauri с loopback
  WebSocket вместо IPC. Локальный режим не включает permissive CORS.
  У статического Pages UI собственная meta CSP, разрешающая loopback порты;
  landing сохраняет отдельную политику `connect-src 'none'`.

Токен даёт доступ ко всему RPC, включая операции с vault: храните ссылку в тайне.
Loopback доступен локальным процессам, способным подделать Origin/Host;
проверки опираются на секретность bearer-токена и ограничения доступа ОС, а не
на границу между локальными пользователями. Автозапуск передаёт URL в argv
`xdg-open`/`open` и далее браузера; `/proc/*/cmdline` может раскрывать его другим
пользователям на всё время жизни браузера. Полный fragment попадает в историю
до `replaceState`; история, её синхронизация и расширения могут сохранить токен.
На многопользовательской машине рекомендуется `--no-open`: откройте голый URL
UI без fragment и введите порт/токен в форме. Не помещайте секретную ссылку в
общие терминальные логи. Ранний `harden_process()` уменьшает риск дампов и
доступа к памяти нативного процесса, но не закрывает argv/историю браузера и
не защищает от привилегированного процесса. Заражённый профиль
браузера или расширение видит открытый текст и токен. JS, раздаваемый Pages,
входит в цепочку поставки интерфейса и может читать данные после сопряжения;
эти проверки не защищают от подмены доверенного UI. Хостинг получает обычные
запросы статических файлов/IP, но приложение не отправляет ему переписку.
В браузере файлы выбираются input, вложения скачиваются через Blob, уведомления
используют Notification API либо показ сообщения внутри UI. Оба web RPC отправки
вложений используют те же `attachment_privacy_enabled`/`prepare_attachment`, что
и desktop path-команды: при включённой приватности метаданные очищаются, а
неподдерживаемый формат отклоняется; при выключенной сохраняются исходные байты.
Web-вложения передаются inline base64. Пределы axum WebSocket по умолчанию:
64 MiB на сообщение и 16 MiB на кадр. Это предел всего JSON, включая все файлы,
текст и base64 (примерно 4/3 исходного размера); превышение закрывает WebSocket.
Это не desktop-путь отправки больших файлов с диска.
Импорт/экспорт desktop-backup
и APK-save оставлены с подсказкой о необходимости desktop: существующий web RPC
не имеет этих методов. Полная совместимость RPC не входит в подтверждённый объём.

Только явный **non-loopback** `--listen <addr>` сохраняет **старый hosted gateway**: данные и ключи
на хосте gateway, hop count из `--hops`/`GIPNY_SERVER_HOPS` (по умолчанию 1),
`./data` по умолчанию, permissive CORS и отсутствие local token/Origin/Host
защиты. Он печатает громкое предупреждение и не является целевым режимом владельца.

Остаточные проверки и пределы автоматических тестов:
[план от 2026-10-31](../docs/plans/2026-10-31-web-local-mode.md).

## Что здесь

| Путь | За что отвечает |
|---|---|
| `src/main.rs` | Точка входа, вызывает `gipny_lib::run()` |
| `src/web_main.rs` | Локальный browser launcher; только non-loopback `--listen` сохраняет legacy gateway; feature `web-gateway` |
| `src/lib.rs` | Сборка Tauri (`run`), каталог данных (`resolve_base_dir`, `profile_dir`), `AppCtx`, **все `#[tauri::command]`** и список `invoke_handler!`, DTO для интерфейса (`ContactDto`, `MessageDto`…), запуск профиля `boot` (установка отложенного обновления Windows → разблокировка vault → база → i2p → `Core::start`) |
| `src/core.rs` | `Core`, клиент целиком (см. разбивку ниже), и `CoreEvent` — события в интерфейс |
| `src/sanitizer.rs` | Очистка метаданных вложений перед отправкой (EXIF и подобное) |
| `src/notify.rs` | Системные уведомления и звуки |
| `src/tray.rs` | Иконка в трее со счётчиком |
| `tauri.conf.json` | Имя, идентификатор `app.gipny.i2p`, версия, окна, бандлы, ресурс `resources/i2pd*` |
| `tauri.android.conf.json` | Переопределения для Android (идентификатор `app.gipny`, без ресурса роутера) |
| `capabilities/default.json` | Разрешения Tauri для интерфейса |
| `icons/` | Иконки; `icon.svg` — исходник, PNG генерирует `tools/gen-icons.sh` |
| `resources/i2pd-netdb-seed.tar.gz` | Снимок netDb для быстрого первого запуска, кладёт `release.yml`; в git не хранится. Роутер вкомпилирован (`i2p-embed`), бинаря i2pd здесь нет; `resolve_bundled_router` в `lib.rs` только передаёт путь к снимку в `GIPNY_I2P_SEED` |
| `build.rs` | Сборочный скрипт Tauri |

**`core.rs` по областям:**

- **Запуск:** `Core::start` поднимает циклы и встроенный релей. Прогресс открытия
  профиля идёт наружу событием `boot_status` (`boot_status`/`boot_progress` в
  `lib.rs`): этапы `vault | router | tunnels | session | core | relay | dht`, их
  показывает экран загрузки. Релей и вход в сеть зеркалятся туда из `CoreEvent`
  в пересылке событий, чтобы экран не зависел от того, подписался ли уже главный вид.
- **Свой релей:** `relay_mode`/`set_relay_mode`, `start_hosted_relay`/`run_hosted_relay`, `flush_relay_announcements` (адрес объявляется повторно, пока контакт не ответит — `note_heard_from`; контакт из списка не выбрасывается).
- **Соединения с релеями:** `spawn_relay_loop` (свой релей), `relay_for` (релей контакта, пул `peer_relays`), `warm_peer_relays`/`warm_relay_of` (дозвон заранее: при старте, при добавлении контакта, при новом адресе релея; неудача — повтор через 5 с с удвоением до 2 мин, `peer_relay_backoff`; если сессии с контактом ещё нет, по тому же соединению сразу запрашивается его бандл и хранится 2 мин в `bundle_cache`), `run_recv_loop`, `handle_relay_frame`.
- **Приём:** `handle_incoming_envelope` (X3dhInit / Ratchet) → `persist_incoming`. Контакт-запрос обрабатывает `persist_from_requester`, имя и адрес отправителя — `apply_contact_hints`.
- **Отправка:** `send_message` кладёт в базу, `spawn_send_loop` → `flush_all_pending` отправляет и повторяет, `send_payload_via_relay` шифрует, `ensure_session_for` открывает сессию (X3DH сразу, без ожидания; скрестившиеся инициализации решает `ours_stands` — остаётся сессия меньшего ключа подписи, проигравшая хранится, чтобы прочитать письма по ней; проигравший ничего разом не переотправляет — недошедшее уходит обычным повтором). Бандл для X3DH `ensure_session_for` сперва берёт из `bundle_cache`, если он свежее `BUNDLE_PREFETCH_TTL`. Куда именно уходит письмо, решает `route_for` → `Route::Relay` (релей контакта) или `Route::Dht` (сеть релеев), отдаёт `deliver`.
- **Файлы частями** (больше 128 КБ, `gipny_libcore::files`): `store_outgoing` запечатывает их частями и предлагает в письме (`register_offers`, `offers_for`), `send_file_parts` шлёт части из цикла отправки только на релей получателя, по `FILE_LANES` соединениям (`file_lanes`); приём — `accept_offers`, `on_file_chunk`, `on_file_ack`, отмена — `cancel_files`; для UI — `files_progress` и события `FileProgress`/`FileReceived`/`FileFailed`. Команда режима агента ждёт свои файлы, прежде чем выполниться.
- **Сеть релеев:** `spawn_dht_loop` (обслуживание раз в 45 мин, сбор писем раз в 10 мин), `collect_from_dht` (письма и intro → `handle_incoming_envelope`, отметки в `dht_seen`, удаление из сети), `look_up_address`/`maybe_look_up_address` (новый адрес контакта), `publish_bundle_to_dht`. Всё, что не зависит от `Core`, — в `libcore/src/dht_client.rs`.
- **Контакты:** `add_contact_via`, `accept_contact_request`, `decline_contact_request`, `delete_contact`, `request_resync`.
- **Группы:** `create_group`, `send_to_group`, `ensure_group_from_wire`; `attempt_group_delivery` сохраняет очередь до ACK, `acknowledge_delivery` проверяет получателя и полное завершение ([контракт](../docs/group-delivery.md), следующая версия).
- **Режим агента:** `agent_master`, `set_agent_mode`, `disable_agent_mode`.
- **Обновления:** `spawn_update_loop`, `check_and_emit_update`, `install_update`, `auto_update_enabled`.
- **Prekeys:** `republish_bundle`.

## Куда вносить правки

| Задача | Где |
|---|---|
| Новая функция для интерфейса | метод в `core.rs` → `#[tauri::command]` в `lib.rs` → добавить в `invoke_handler!` → `Api.*` в `ui/src/api.ts` → case в `ui/dev/mock.ts` |
| Новое событие в интерфейс | вариант `CoreEvent` в `core.rs` → union `CoreEvent` в `ui/src/api.ts` → обработчик в `ui/src/state.ts` |
| Новое поле контакта для интерфейса | `ContactDto` и его `From<Contact>` в `lib.rs` → `interface Contact` в `ui/src/api.ts` → фикстуры в `ui/dev/mock.ts` |
| Порядок запуска профиля | `boot` в `lib.rs` |
| Встроенный/внешний релей | `core.rs`: `set_relay_mode`, `run_hosted_relay`, `relay_onion` |
| Что и когда отправляется, повторы | `core.rs`: `flush_all_pending` (константы `RETRY_*` — вверху файла) |
| Обработка входящего сообщения | `core.rs`: `persist_incoming` |
| Сессии и X3DH | `core.rs`: `ensure_session_for`, `handle_incoming_envelope` |
| Запросы в контакты | `core.rs`: `add_contact_via`, `accept_request_now`, `persist_from_requester`, `MAX_INCOMING_REQUESTS` |
| Пометка «контакт недоступен» | `core.rs`: `note_reachability`, `CONTACT_UNREACHABLE_AFTER`; текст — `ui/src/chat.ts` |
| Автообновление в приложении | `core.rs`: `check_and_emit_update`, `install_update`; сама установка — `libcore/src/update.rs` |
| Настройка в базе | константа `SETTING_*` вверху `core.rs`, геттер и сеттер в `Core`, команды в `lib.rs` |
| Очистка вложений | `sanitizer.rs` |
| Уведомления, звуки, трей | `notify.rs`, `tray.rs` |
| Разрешения Tauri | `capabilities/default.json` |
| Версия приложения | `Cargo.toml` + `tauri.conf.json` (+ `ui/package.json`, `agent/Cargo.toml`) |

## Что менять вместе

- **`core.rs` ↔ `libcore/src/session.rs`** (агент и боты) — две копии логики доставки.
- **`CoreEvent`, DTO, команды ↔ `ui/src/api.ts` ↔ `ui/dev/mock.ts`.** Без мока превью интерфейса молча ломается.
- **Версии в `Cargo.toml`, `tauri.conf.json`, `ui/package.json`** сверяет с тегом джоба `version matches the tag` в `release.yml`.

## Инварианты и грабли

- **Адрес встроенного релея не пишется в настройки** (`relay_onion` берёт его из памяти). В 0.4.0 записанный мёртвый адрес ломал доставку.
- **`relay_for` никогда не ждёт дозвона.** Он возвращает `None` и дозванивается в фоне, иначе один недоступный контакт тормозит отправку всем.
- **Отложенный установщик Windows запускается в `boot` до разблокировки vault**, а не в `Core::start`. Иначе пользователь ждёт роутер, после чего приложение всё равно закрывается.
- **Сохранение APK проверяет SHA256 наравне с автообновлением** (следующая версия). `download_apk` получает сумму из того же `ReleaseInfo`, а ошибка manifest приводит к `UpdateFailed`. [Контракт обновлятора](../docs/update-verification.md).
- **Контакту в состоянии `RequestState::Incoming` ничего не отправляется**, даже аки.
- **Успех отправки группы сохраняет `pending_outbound`.** Полный снимок получателей публикуется через `Db::queue_group_recipients` после подготовки метаданных, до сетевого `await`. ACK удаляет только пару сообщения и аутентифицированного получателя; `MessageDelivered` возникает после последнего ожидания. Семидневная очистка снимает все оставшиеся ожидания просроченного сообщения, сохраняя историю без отметки доставки. Границы проверки и поведение при блокировке/удалении — в [group-delivery.md](../docs/group-delivery.md).
- **Сборка `gipny` без файла в `resources/` падает** на глобе `resources/i2pd*` (tauri-build не принимает глоб без совпадений). В релизе он находит снимок netDb, в `build.yml` кладётся пустая заглушка.
- **Релиз не создаёт `debug.log`/`debug.prev.log` и файл журнала роутера** (следующая версия). `GIPNY_DEBUG_LOG`, прежний `log.conf` и UI не включают захват. Настройки показывают недоступность capability; разработка и тесты сохраняют диагностику. Операционные ошибки остаются в stderr. [Политика, профиль и проверки](../docs/release-diagnostics.md).

## Как проверить

```
env -u CC -u CXX cargo check -p gipny
env -u CC -u CXX cargo test -p gipny --lib
cd ui && npx tsc --noEmit
```

Интерфейс без бэкенда: `cd ui && npm run dev`, затем http://127.0.0.1:5173/dev/preview.html. Установщики и APK собирает только GitHub (`build.yml`, `release.yml`).
