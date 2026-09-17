# core/relay — выделенный релей `gipny-relay` (отдельный Cargo workspace)

Сервер с постоянным адресом i2p. Хранит зашифрованную почту и prekey-пакеты **для любого** клиента, в SQLite. Нужен тем, кому требуется ящик, который принимает почту, пока приложение закрыто. Встроенный релей приложения — это `libcore/src/relay_server.rs`.

## Что здесь

| Файл | За что отвечает |
|---|---|
| `src/main.rs` | `main` (переменные окружения, GC раз в час, цикл accept с пересборкой SAM-сессии), `load_or_create_identity` (`dest.key`/`dest.pub`), `open_session`, `handle_client` (challenge, `Auth`/`AuthV2`, push ожидающих писем), `client_loop` (Publish, GetBundle, Send, Ack, Ping), `send_frame`/`recv_frame` |
| `src/proto.rs` | Копия протокола: `ClientToRelay`, `RelayToClient`, `auth_v2_message`, `destination_hash`, `ERR_NEEDS_AUTH_V2`, golden-тесты байтов |
| `src/storage.rs` | SQLite (`relay.db`, WAL): `MESSAGE_TTL_MS` (14 дней), `BUNDLE_TTL_MS` (30 дней), `MAX_PER_RECIPIENT`, `PENDING_LIMIT` |
| `gipny-relay.service`, `gipny-i2pd.service` | systemd-юниты для сервера |
| `Cargo.toml` | Свой `[workspace]`: bincode 2, rusqlite 0.40 `bundled`, sha2 |

**Настройка — только переменными окружения:** `GIPNY_RELAY_DATA` (по умолчанию `./relay-data`) и `GIPNY_SAM_PORT` (7656).

## Куда вносить правки

| Задача | Где |
|---|---|
| Новый кадр протокола | `src/proto.rs` (в конец enum + golden-тест) **и** `libcore/src/relay.rs` |
| Правила входа | `src/main.rs`: `handle_client`, `client_loop` (`owner`) |
| Сроки хранения и лимиты | `src/storage.rs` |
| Схема базы | `src/storage.rs`: `open` |
| Параметры SAM-сессии | `src/main.rs`: `open_session` |
| Юниты systemd | `gipny-relay.service`, `gipny-i2pd.service` |
| Архив в релизе | джоба `relay (linux …)` в `.github/workflows/release.yml` |
| Публичный тестовый релей | `.github/workflows/relay-testnet.yml` |

## Что менять вместе

- **`src/proto.rs` ↔ `libcore/src/relay.rs`.** Порядок вариантов и раскладка байтов должны совпадать. Здесь bincode 2 в режиме `legacy()`, в клиентах bincode 1; совпадение держат golden-тесты с обеих сторон.
- **Логика в `main.rs` ↔ `libcore/src/relay_server.rs`.** Встроенный релей — порт этого сервера. Исправление в одном обычно нужно и в другом.

## Инварианты и грабли

- **Код с libcore не общий.** `rusqlite` здесь с `bundled`, в основном workspace с SQLCipher, и два `libsqlite3-sys` конфликтуют по `links`. Поэтому это отдельный workspace, и `core/relay/Cargo.lock` свой.
- **`dest.key` в каталоге данных — это адрес релея.** Потерять его значит сменить адрес для всех клиентов, а украсть — выдать себя за релей.
- **Собирать и публиковать можно только после `AuthV2`.** Простой `Auth` даёт только право положить письмо (для клиентов 0.4.2).
- **В отличие от встроенного релея, ошибки хранилища закрывают соединение**, кадра `Error` при этом нет. Исключение — отказ по `AuthV2`.

## Как проверить

```
env -u CC -u CXX cargo test --manifest-path core/relay/Cargo.toml
```

По живой i2p этот релей проверяет джоба `e2e (relay + two bots)` в `e2e-i2pd.yml`; в логе релея должно быть `auth ok … (v2)`. Бинарь для релиза собирает `release.yml`.
