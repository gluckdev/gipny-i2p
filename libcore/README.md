# libcore — общее ядро gipny: криптография, сессии, база, релей, роутер, агент, автообновление

Крейт `gipny-libcore` используют приложение (`core/`), агент (`agent/`), боты (`bot-sdk/`) и e2e-харнесс. Он собирается в том числе в APK под Android.

## Что здесь

| Файл | За что отвечает | Главное |
|---|---|---|
| `crypto.rs` | Ключи личности, X3DH, Double Ratchet, шифрование вложений | `Identity`, `IdentityCard`, `PreKeyBundle`, `x3dh_initiate`/`x3dh_respond`, `RatchetState`, `AttachmentCipher`, `fill_random` |
| `security.rs` | Хранилище ключей (vault), KDF, duress-пароль, бэкапы, защита памяти | `Vault`, `MasterKey`, `UnlockOutcome`, `DuressMode`, `backup_seal`/`backup_open`, `harden_process` |
| `db.rs` | SQLCipher-база: контакты, сообщения, группы, сессии, prekeys, настройки, очереди отправки | `Db::open` / `open_plain`, `migrate` + `ensure_column`, `Contact`, `TrustLevel`, `RequestState`, `list_unacked_outgoing`, `RETRY_TTL_MS` |
| `session.rs` | Мессенджер без UI для агента и ботов (`SessionManager`); здесь же формат сообщения. С 0.4.11 — тот же путь через сеть релеев, что у `Core`: `Route::{Relay,Dht}`, `dht_handler`/`join_dht`, `collect_from_dht` (вступительные письма первыми; неоткрывшееся остаётся в сети), `set_local_relay` (свой релей — по трубе); релеи собеседников дозваниваются заранее (`warm_peer_relays`/`warm_relay_of`, повтор 5 с → 2 мин) и по тому же соединению запрашивается бандл (`bundle_cache`); X3DH без ожидания, скрещённые инициализации — `ours_stands` | `SessionManager`, `SessionEvent`, `WirePayload` и снимки `WireV0..WireV9`, `encode_payload`/`decode_payload`, `pad_payload`; файлы частями: поля `files`/`file_chunk`/`file_ack`/`file_cancel`, `store_outgoing`, `accept_offers`, `on_file_chunk`, `send_file_parts` (по `FILE_LANES` потокам), `spawn_collect_lanes` (приём частей несколькими соединениями со своим сторонним релеем, `collect_lane_wanted`/`collect_lane_holds`), события `FileProgress`/`FileReceived`/`FileFailed` |
| `files.rs` | Файлы частями: нарезка, запись частей на диск каждая в своё место, сборка с проверкой sha256, учёт полученного и окно отправителя | `INLINE_MAX` (128 КБ), `CHUNK_SIZE` (192 КБ), `WINDOW`, `seal_from`, `read_part`/`write_part`, `open_to`, `read_attachment` (оба формата), `Received` (сводный ack с дырами и `seen_to`), `Sending`, `Flow` (окно на собеседника, как у TCP: время пути, таймаут, рост по ack и половина при потере; рост стоп, когда круг дольше самого быстрого — очередь) |
| `relay.rs` | Клиент протокола релея и сам протокол | `ClientToRelay`/`RelayToClient`, `EnvelopeBlob`, `connect`/`connect_peer`/`connect_local` (свой релей в процессе, по трубе), `AuthV2` + `auth_v2_message` + `destination_hash`, `ERR_NOT_SERVED`, `ERR_NEEDS_AUTH_V2` |
| `relay_server.rs` | Встроенный релей в процессе: опубликованная destination на роутере процесса | `EphemeralRelay::start(limits, dht)`, `start_unclaimed()` + `claim` (туннели строятся до разблокировки профиля), `connect_local` (вход для владельца по трубе, без i2p), `set_hops` (пересборка destination), `MemStore`, `MemStoreLimits` (`personal`), `handle_client` (несколько соединений одного ключа — `Lanes`, почта по кругу, неподтверждённое закрытого — остальным, `redeal`); первый кадр `Dht` — анонимный запрос к узлу сети (`serve_dht`, `DhtHandler`) |
| `dht_client.rs` | Узел сети релеев (`gipny-dht`) в приложении и агенте | `new_node`, `handler` (для релея), `join` (релей поднялся), `maintain` (раз в 45 мин), `DbStorage` (таблицы `dht_items`, `dht_peers`), `I2pTransport`, `status`, сиды из `GIPNY_DHT_SEEDS` (только при сборке); `save_peers` не затирает таблицу, если никто не ответил |
| `embedded.rs` | Роутер i2pd внутри процесса (`i2p-embed`), один на процесс, под `<data_dir>/i2p/router` | `router` (запуск: datadir, SAM/http/httpproxy/socks/upnp выключены, `--reseed.verify=true`, bandwidth/share/transittunnels из настроек, Yggdrasil; снимок netDb до старта), `running`, `destination_options`; `GIPNY_I2P_LOGLEVEL` — уровень `i2pd.log` для диагностики |
| `net.rs` | Наша destination на роутере процесса, только исходящие | `I2pNode` (он же `TorNode`), `start`/`start_with_progress`, `connect_relay`/`connect_service`, `set_hops`, `recreate` (новая destination на тех же ключах) |
| `router.rs` | Настройки роутера, прогресс запуска, снимок netDb | `RouterSettings`, `TransitProfile`, `Yggdrasil`; `BootProgress`/`note` — прогресс запуска наружу (его показывает экран открытия профиля); `seed_netdb_from`/`seed_netdb_reader` (раскладка снимка только в пустой netDb), `compiled_in_seed` (`GIPNY_NETDB_SEED` при сборке, Android), `bundled_seed` (`GIPNY_I2P_SEED` или рядом с исполняемым файлом) |
| `i2p_http.rs` | HTTPS GET в clearnet через i2p без локального прокси: свой поток к outproxy, `CONNECT`, TLS (rustls/ring, корни webpki), HTTP/1.1 (hyper) | `get`, `Body`, `OUTPROXY` (b32 `exit.stormycloud.i2p`) |
| `update.rs` | Автообновление через GitHub Releases по i2p-outproxy (транспорт — `i2p_http.rs`) | `Updater` (`check`/`download`/`install`), `Component`, `InstallOutcome`, `target_suffix`, `apply_staged_windows_installer`, `is_deb_install`/`install_deb_now` (пакет ставит менеджер пакетов через `pkexec`, поэтому там установка по кнопке, а не молча) |
| `agent.rs` | Режим агента: выполнение команд мастера | `run_command`, `handle_console_request`, `parse_control`, `save_uploads`, `BODY_GRANT`/`BODY_REVOKE`/`BODY_OFF` |
| `card.rs` | Текстовая карточка `gipny:v1:` / `gipny:v2:` для headless-бинарей | `ContactCard`, `is_valid_i2p_address` |
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
| Флаги i2pd, транзит, Yggdrasil | `embedded.rs` (`router`), значения — `router.rs` (`TransitProfile`) |
| Снимок netDb для первого запуска | `router.rs` (`seed_netdb_*`, `bundled_seed`, `compiled_in_seed`), `build.rs` (`GIPNY_NETDB_SEED`) |
| Своя destination, дозвон, восстановление | `net.rs` |
| Outproxy обновлений, TLS | `i2p_http.rs` (`OUTPROXY`) |
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
- **Письмо не больше 4 МБ** (`MAX_PAYLOAD_BYTES`): следующая корзина паддинга, 16 МБ, вместе с заголовком ratchet уже не проходит `MAX_FRAME` релея. Всё больше 128 КБ идёт частями (`files.rs`), превышение — `MessageFailed`, а не «отправлено».
- **Части — только через релей получателя**, не через сеть релеев (там 64 записи / 16 МБ на ключ). Повтор частей — только по дыре в `file_ack` или, если получатель подавал признаки жизни, по таймеру раз в 30 мин; иначе офлайн-получатель стоил бы гигабайт дублей на релее. Частей в полёте — сколько даёт окно собеседника (`files::Flow`, от 4 до `WINDOW`), считая повторы, на все файлы к нему; дыра в ack повторяется, только когда её часть старше таймаута: по нескольким полосам части обгоняют друг друга, и дыра чаще всего — часть в пути. Полос столько, сколько нужно окну (`lanes_for`, по 8 частей).
- **Вложение на диске бывает двух видов**: целиком одним блоком (`chunk_size` `None`) или частями, каждая в своём слоте. Читать только через `files::read_attachment` (или `open_to` потоком).
- **TLS для автообновления — `rustls` с провайдером `ring`, не `aws-lc-rs`.** `aws-lc-sys` требует cmake и C-тулчейн на каждую цель, что ломает кросс-сборку Android.
- **Один роутер на процесс** (`embedded.rs`): libi2pd держит глобальное состояние. Настройки роутера — от первого открытого профиля до перезапуска приложения. Падение роутера роняет процесс: перезапускать нечего.
- **Ни одного локального порта.** SAM, HTTP-прокси, SOCKS, веб-консоль, UPnP выключены флагами в `embedded.rs::router`; обновления ходят через `i2p_http.rs`, а не через прокси.
- **Встроенный релей личный** (`MemStoreLimits::personal`): чужую почту он не принимает, а его адрес живёт только в памяти.
- **Relay выдаёт `Incoming` с `from = [0; 32]` (sealed sender).** Получатель перебирает сессии всех контактов.
- **Собирать и публиковать через релей можно только после `AuthV2`.** Простой `Auth` оставлен для 0.4.2 и даёт только право положить письмо.
- **`Db::open` — SQLCipher с ключом из vault.** `open_plain` используют только агент и боты.

## Как проверить

```
env -u CC -u CXX cargo test -p gipny-libcore
```

`env -u CC -u CXX` обязателен: в оболочке экспортирован компилятор Android NDK, и OpenSSL иначе собирается не под хост. Тесты компилируют libi2pd (`i2p-embed`), им нужны заголовки boost, OpenSSL и zlib; `I2P_EMBED_SKIP_NATIVE=1` — только для `cargo check` без них. Строки `sqlcipher … error decrypting` в выводе тестов — это ожидаемый тест неверного пароля. Сборки бинарей и e2e по живой i2p — только на GitHub (`build.yml`, `i2p-embed.yml`, `e2e-i2pd.yml`).
