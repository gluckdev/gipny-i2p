# libcore — общее ядро gipny: криптография, сессии, база, релей, роутер, агент, автообновление

Крейт `gipny-libcore` используют приложение (`core/`), агент (`agent/`), боты (`bot-sdk/`) и e2e-харнесс. Он собирается в том числе в APK под Android.

## Что здесь

| Файл | За что отвечает | Главное |
|---|---|---|
| `crypto.rs` | Ключи личности, X3DH, Double Ratchet, шифрование вложений | `Identity`, `IdentityCard`, `PreKeyBundle`, `x3dh_initiate`/`x3dh_respond`, `RatchetState`, `AttachmentCipher`, `fill_random` |
| `security.rs` | Хранилище ключей (vault), KDF, duress-пароль, бэкапы, защита памяти | `Vault`, `MasterKey`, `UnlockOutcome`, `DuressMode`, `backup_seal`/`backup_open`, `harden_process` |
| `db.rs` | SQLCipher-база: контакты, сообщения, группы, сессии, prekeys, настройки, очереди отправки | `Db::open` / `open_plain`, `migrate` + `ensure_column`, `Contact`, `TrustLevel`, `RequestState`, `list_unacked_outgoing`, `RETRY_TTL_MS` |
| `session.rs` | Мессенджер без UI для агента и ботов (`SessionManager`); здесь же формат сообщения | `SessionManager`, `SessionEvent`, `WirePayload` и снимки `WireV0..WireV7`, `encode_payload`/`decode_payload`, `pad_payload` |
| `relay.rs` | Клиент протокола релея и сам протокол | `ClientToRelay`/`RelayToClient`, `EnvelopeBlob`, `connect`/`connect_peer`, `AuthV2` + `auth_v2_message` + `destination_hash`, `ERR_NOT_SERVED`, `ERR_NEEDS_AUTH_V2` |
| `relay_server.rs` | Встроенный релей в процессе | `EphemeralRelay::start`, `MemStore`, `MemStoreLimits` (`personal`), `handle_client`; первый кадр `Dht` — анонимный запрос к узлу сети (`serve_dht`, `DhtHandler`) |
| `dht_client.rs` | Узел сети релеев (`gipny-dht`) в приложении и агенте | `new_node`, `handler` (для релея), `join` (релей поднялся), `maintain` (раз в 45 мин), `DbStorage` (таблицы `dht_items`, `dht_peers`), `I2pTransport`, `status`, сиды из `GIPNY_DHT_SEEDS` |
| `net.rs` | SAM-сессия клиента через `yosemite` | `I2pNode` (он же `TorNode`), `connect_relay`/`connect_service`, `sam_port`, `http_proxy_port` |
| `router.rs` | Запуск и настройка процесса i2pd; `BootProgress`/`note` — прогресс запуска наружу (его показывает экран открытия профиля) | `RouterHandle` (`spawn`/`attach`), `RouterSettings`, `TransitProfile`, `Yggdrasil`, `DEFAULT_OUTPROXY`; `alive`/`restart` — роутер, умерший во время работы (приложение иначе вечно стучится в мёртвый SAM: проверяется в `net.rs::ensure_router` перед каждой пересборкой сессии), `previous_router` — роутер, оставшийся от прошлого запуска профиля (живой SAM переиспользуется, зависший останавливается: i2pd держит блокировку на `i2pd.pid`, второй экземпляр иначе сразу падает) |
| `update.rs` | Автообновление через GitHub Releases по i2p-outproxy | `Updater` (`check`/`download`/`install`), `Component`, `InstallOutcome`, `target_suffix`, `apply_staged_windows_installer` |
| `agent.rs` | Режим агента: выполнение команд мастера | `run_command`, `handle_console_request`, `parse_control`, `save_uploads`, `BODY_GRANT`/`BODY_REVOKE`/`BODY_OFF` |
| `card.rs` | Текстовая карточка `gipny:v1:` / `gipny:v2:` для headless-бинарей | `ContactCard`, `is_valid_i2p_address` |
| `proxy.rs` | Дочерний процесс sing-box | запуск и остановка клиента |
| `lib.rs` | Реэкспорты | — |
| `tests/` | `crypto.rs` (криптоядро), `compat.rs` + `fixtures/compat-0.4.0` (база и vault из 0.4.0 открываются), `first_run.rs` | — |

## Куда вносить правки

| Задача | Где |
|---|---|
| Новое поле в сообщении | `session.rs`: поле в конец `WirePayload` с `#[serde(default)]`, условие в `encode_payload`, `None` во всех `From<WireVn>` и литералах в `core/src/core.rs` и `session.rs` |
| Новый кадр протокола релея | `relay.rs`: вариант в **конец** `ClientToRelay`/`RelayToClient` + golden-тест + то же в `core/relay/src/proto.rs` |
| Правила входа на релей | `relay.rs` (`handshake`, `predates_auth_v2`), `relay_server.rs` (`handle_client`, `client_loop`) |
| Сколько и как долго встроенный релей держит почту | `relay_server.rs`: `MemStoreLimits` |
| Политика повторной отправки | `db.rs`: `RETRY_TTL_MS`, `list_unacked_outgoing`, `pending_outbound_for_recipient`; бэкофф — константы `RETRY_*` в `core.rs`/`session.rs` |
| Новая колонка или таблица | `db.rs`: `migrate`: колонка — через `ensure_column` (таблица должна быть в `MIGRATE_TABLES`), новая таблица — `CREATE TABLE IF NOT EXISTS` в общем батче; для контактов и сообщений ещё `select_contact!`/`message_cols!` и `map_contact`/`map_message` |
| Состояние запроса в контакты | `db.rs`: `RequestState`, `set_contact_request_state`, `list_incoming_requests` |
| Криптография сессий | `crypto.rs` + `tests/crypto.rs` |
| Пароль, KDF, duress, бэкап | `security.rs` |
| Флаги i2pd, транзит, outproxy, порты | `router.rs` (`spawn`) |
| SAM-сессия, дозвон, восстановление | `net.rs` |
| Имена ассетов автообновления, установка по платформам | `update.rs`: `target_suffix`, `install` |
| Частота проверки обновлений | `update.rs`: `UPDATE_CHECK_INITIAL_SECS`, `UPDATE_CHECK_INTERVAL_SECS` |
| Команды консоли агента | `agent.rs` (`handle_console_request`, `HELP_TEXT`) |
| Формат карточки | `card.rs` **и** `ui/src/api.ts` (`encodeCard`/`decodeCard`) |
| Поведение агента и ботов при приёме и отправке | `session.rs` (`flush_all_pending`, `handle_incoming_envelope`, `relay_for`) |

## Что менять вместе

- **`session.rs` ↔ `core/src/core.rs`.** Это две копии одной логики доставки. Исправление приёма или отправки почти всегда нужно в обоих файлах.
- **`relay.rs` ↔ `core/relay/src/proto.rs`.** Протокол описан дважды. Порядок вариантов — это номер на проводе, поэтому golden-тесты стоят в обоих крейтах.
- **`card.rs` ↔ `ui/src/api.ts`** — формат карточки.
- **`update.rs` ↔ `.github/workflows/release.yml`** — имена ассетов релиза.

## Инварианты и грабли

- **bincode остаётся 1.x.** Версионирование `WirePayload` держится на том, что bincode 1 игнорирует лишние байты в конце. Мажорное обновление — это смена протокола, dependabot его блокирует.
- **Новые поля и варианты — только в конец.** Старые клиенты декодируют по позиции.
- **TLS для автообновления — `rustls` с провайдером `ring`, не `aws-lc-rs`.** `aws-lc-sys` требует cmake и C-тулчейн на каждую цель, что ломает кросс-сборку Android.
- **Встроенный релей личный** (`MemStoreLimits::personal`): чужую почту он не принимает, а его адрес живёт только в памяти.
- **Relay выдаёт `Incoming` с `from = [0; 32]` (sealed sender).** Получатель перебирает сессии всех контактов.
- **Собирать и публиковать через релей можно только после `AuthV2`.** Простой `Auth` оставлен для 0.4.2 и даёт только право положить письмо.
- **`Db::open` — SQLCipher с ключом из vault.** `open_plain` используют только агент и боты.

## Как проверить

```
env -u CC -u CXX cargo test -p gipny-libcore
```

`env -u CC -u CXX` обязателен: в оболочке экспортирован компилятор Android NDK, и OpenSSL иначе собирается не под хост. Строки `sqlcipher … error decrypting` в выводе тестов — это ожидаемый тест неверного пароля. Сборки бинарей и e2e по живой i2p — только на GitHub (`build.yml`, `e2e-i2pd.yml`).
