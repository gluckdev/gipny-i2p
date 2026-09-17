# dht — `gipny-dht`: сеть релеев, где узлы хранят запечатанную почту друг для друга

Протокол, криптография и хранилище. **В приложение и агента пока не подключено** (это фаза 4). Почему сеть и что она гарантирует — `docs/relay-independence.md`, раздел «Decision (2026-09-17)».

## Что здесь

| Файл | За что отвечает | Главное |
|---|---|---|
| `src/crypto.rs` | Ключи и запечатывание | `pair_secret` (X25519 статических ключей пары), `card_secret`, `mail_key`/`intro_key`/`addr_key`/`bundle_key`, `day_of`, `seal`/`open` (XChaCha20-Poly1305 + паддинг до степени двойки), `seal_to`/`open_from` (эфемерный X25519 к получателю), `MAX_PLAINTEXT` |
| `src/items.rs` | Что кладётся в сеть | `mail`/`open_mail`, `intro`/`open_intro`, `address_record`/`open_address_record`, `bundle_record`/`open_bundle_record`, `mail_keys_to_poll`, `intro_keys_to_poll`, `delete_hash`, `PreparedItem`, TTL `MAIL_TTL_MS`/`ADDRESS_TTL_MS`/`BUNDLE_TTL_MS` |
| `src/proto.rs` | Сообщения между узлами | `NodeInfo` (+ `node_id`), `StoredItem`, `DhtRequest`, `DhtEnvelope`, `DhtResponse`, `pow_ok`/`solve_pow`, `PROTOCOL_VERSION`, `MAX_VALUE_BYTES` |
| `src/store.rs` | Хранилище узла | трейт `Storage`, `MemStorage`, `StoreLimits`, `admit` |
| `src/node.rs` | Узел | трейты `Transport`/`Connection`, `DhtNode` (`handle` — ответ другим; `bootstrap`, `lookup`, `put`, `get`, `delete`, `republish`, `maintain`, `add_candidates`, `known_peers`), `NodeConfig` |
| `tests/sim.rs` | Сеть в памяти | переживание смены узлов, поиск адреса после перезапуска, отказы узлов |

## Куда вносить правки

| Задача | Где |
|---|---|
| Как вычисляются ключи хранения | `crypto.rs` (`*_key`) |
| Формат письма или записи | `items.rs` |
| Новый запрос между узлами | `proto.rs` (`DhtRequest`/`DhtResponse`, в конец) + `node.rs::handle` |
| Сложность proof-of-work, k, α, таймауты | `node.rs`: `NodeConfig` |
| Квоты и сроки хранения на узле | `store.rs`: `StoreLimits` |
| Поиск узлов, репликация | `node.rs`: `lookup`, `put`, `republish` |
| Хранилище на диске (SQLite приложения или агента) | реализовать `Storage` в интегрирующем крейте, не здесь |
| Транспорт по i2p | реализовать `Transport`/`Connection` в интегрирующем крейте |

## Инварианты и грабли

- **Без `workspace = true` и без rusqlite.** От крейта по пути будет зависеть `core/relay` со своим workspace и своим SQLite.
- **Id узла выводится из адреса релея** и не должен связываться с личностью пользователя.
- **Хранящий узел видит только случайные байты под случайным ключом:**
  - любое новое содержимое запечатывается через `seal`;
  - новый вид записи получает свою метку (`label`);
  - первое письмо запечатывается только через `seal_to`, потому что называет отправителя.
- **Подпись записи лежит внутри шифротекста,** узел её не проверяет. Читатель берёт новейшую валидную запись.
- **Удалить может только тот, кто вскрыл письмо** (токен внутри). Хранить сам хэш токена как токен нельзя: тест `only_the_token_deletes`.
- **`solve_pow` блокирующий,** запускать через `spawn_blocking` (так делает `store_at`).
- **Сообщения ходят внутри протокола релея как непрозрачные байты** (`node::encode_envelope` и соседние функции).

## Как проверить

```
env -u CC -u CXX cargo test -p gipny-dht
```

Главная уверенность — `tests/sim.rs`. Если убрать `republish`, тест `mail_outlives_every_node_it_was_first_stored_on` должен упасть. По живой i2p сеть пока не проверяется.
