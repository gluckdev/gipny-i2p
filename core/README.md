# core — приложение gipny-i2p на Tauri (десктоп и Android)

Крейт `gipny`. Rust-часть приложения: `Core` (логика клиента поверх `libcore`), команды для интерфейса из `ui/`, запуск профиля. Рядом лежат `relay/` (выделенный релей, свой README) и `gen/android/` (Android-обёртка, свой README).

## Что здесь

| Путь | За что отвечает |
|---|---|
| `src/main.rs` | Точка входа, вызывает `gipny_lib::run()` |
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
- **Сеть релеев:** `spawn_dht_loop` (обслуживание раз в 45 мин, сбор писем раз в 10 мин), `collect_from_dht` (письма и intro → `handle_incoming_envelope`, отметки в `dht_seen`, удаление из сети), `look_up_address`/`maybe_look_up_address` (новый адрес контакта), `publish_bundle_to_dht`. Всё, что не зависит от `Core`, — в `libcore/src/dht_client.rs`.
- **Контакты:** `add_contact_via`, `accept_contact_request`, `decline_contact_request`, `delete_contact`, `request_resync`.
- **Группы:** `create_group`, `send_to_group`, `ensure_group_from_wire`.
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
- **Контакту в состоянии `RequestState::Incoming` ничего не отправляется**, даже аки.
- **Сборка `gipny` без файла в `resources/` падает** на глобе `resources/i2pd*` (tauri-build не принимает глоб без совпадений). В релизе он находит снимок netDb, в `build.yml` кладётся пустая заглушка.

## Как проверить

```
env -u CC -u CXX cargo check -p gipny
env -u CC -u CXX cargo test -p gipny --lib
cd ui && npx tsc --noEmit
```

Интерфейс без бэкенда: `cd ui && npm run dev`, затем http://127.0.0.1:5173/dev/preview.html. Установщики и APK собирает только GitHub (`build.yml`, `release.yml`).
