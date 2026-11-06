# libcore — общее ядро / Shared core

## Русский

`gipny-libcore` используют desktop/Android приложение, агент, боты и e2e-harness.
Браузер повторно использует портативные криптографию, карточки и wire-форматы;
его хранилище и асинхронная доставка реализованы в WASM/Worker отдельно.

### Карта исходников

| Модуль | Назначение |
|---|---|
| `crypto.rs`, `card.rs` | Личность, X3DH, Double Ratchet, шифрование вложений, карточки контактов |
| `message_wire.rs`, `relay_wire.rs`, `dm_reference.rs` | Общие форматы сообщений, релея и ссылок на исходное личное сообщение |
| `security.rs`, `memory.rs`, `device_binding.rs` | Vault, KDF, защита ключей в памяти и привязка к OS credential store |
| `db.rs`, `db/message_commit.rs` | SQLCipher, контакты, история, очереди; совместная публикация сообщения, файлов и управляющих metadata |
| `db/dm.rs`, `db/deletion_notice.rs` | Tombstones, удаление и отдельное подтверждение пользователя «ОК» |
| `files.rs`, `db/file_transfer.rs` | Подготовка зашифрованных файлов, части, окно отправки и устойчивые ACK |
| `session.rs` | Доставка для агента и ботов, повторы, группы, контакты и файлы |
| `relay.rs`, `relay_server.rs`, `dht_client.rs` | Клиент релея, встроенный релей и участие в сети доставки |
| `embedded.rs`, `net.rs`, `router.rs` | i2pd внутри процесса, I2P destinations, транспорт и настройки роутера |
| `i2p_http.rs`, `update.rs` | HTTPS через I2P outproxy и проверяемая подготовка обновлений |

### Границы компонентов

`Core` в `core/src/core.rs` и `SessionManager` — две реализации доставки.
Изменение одной требует проверки второй и browser backend. Форматы релея
также представлены в отдельном workspace `core/relay`; кодировка карточек
имеет TypeScript-реализацию. Старые данные не требуют миграций по решению
владельца, но все текущие клиенты должны понимать один протокол.

Нативная БД — SQLCipher. GUI получает ключ из vault; production agent/bot paths
используют OS-bound ключ. `open_plain` нужен тестовым фикстурам, а не аварийному
переключению production-профиля на незашифрованное хранение. Защита от копии
профиля, раскрытия памяти и захвата ОС имеет разные границы:
[хранилище](../docs/device-bound-storage.md), [память](../docs/memory-security.md).

При сборке для WASM native DB/session/router модули заменяются портативными
форматами. Нет SQLCipher, Tokio, libi2pd, нативного DHT, updater и OS API.
`memory_browser.rs` использует `Arc<Zeroizing<_>>` без `mlock`/`mprotect`.
Подробнее: [браузерный мессенджер](../docs/browser-messenger.md).

### Доставка и файлы

Сообщение, его описания файлов, peer state и управляющие metadata сохраняются
одной DB-транзакцией. `StagedAttachments` владеет подготовленными зашифрованными
файлами до commit и удаляет их при обычной ошибке. Аварийное завершение процесса
до commit может оставить неиспользуемый шифротекст. Это граница публикации
прикладных данных; полный цикл криптографии и доставки проверяется отдельно.

Запрос контакта сохраняет текст и file offers, включая сообщение только с файлом.
Приём multipart-байтов и ACK сообщения начинаются после принятия. До 16 файлов
в сообщении, 64 незавершённых incoming offers на профиль и 16 от контакта.
Маленькие файлы делят inline-бюджет 1 МиБ; остальные идут частями по 192 КиБ,
с пределом отдельного файла 2 ГиБ. Изображение прогресса и окончательный ACK
опираются на сохранённое состояние и проверку SHA-256, а не на доставку текста.

Части идут через релей получателя. Окно `Flow` ограничивает незавершённые части
на контакт; несколько потоков переиспользуют I2P-соединения. Прямой IP fallback
не включается. Групповая очередь сохраняется до E2E ACK каждого получателя.
[Передача файлов](../docs/file-transfer.md), [группы](../docs/group-delivery.md),
[удаление сообщений](../docs/message-deletion.md).

### Роутер, журналы и проверки

Один i2pd работает на процесс: его состояние и настройки сохраняются до выхода.
SAM, локальные HTTP/SOCKS-прокси и веб-консоль выключены. NTCP2/SSU2 имеют
сетевые порты; [UPnP](../docs/upnp.md) управляет допустимыми mappings с проверкой
владения. Stable-сборка не включает подробный capture; prerelease/developer/test
используют фиксированную diagnostic policy. Границы платформ описаны в
[журналах](../docs/release-diagnostics.md).

Обновление требует корректный SHA-256; checksum с того же сервера не заменяет
независимую подпись издателя. См. [проверку обновлений](../docs/update-verification.md).
Сборки и исполняемые тесты запускаются только в GitHub Actions:
`security-fast.yml`, `build.yml`, `i2p-embed.yml`, `e2e-i2pd.yml`,
`browser-router.yml`. Синтаксическая проверка, stub, native fixture и доставка
через живую I2P подтверждают разные свойства. Новые изменения этого прохода
ещё требуют CI и проверки телефон ↔ ПК; [текущее состояние](../docs/current-state.md).

## English

`gipny-libcore` serves the desktop/Android app, agent, bots and e2e harness.
The browser reuses portable crypto, contact cards and wire formats; WASM/Worker
storage and asynchronous delivery have separate implementations.

| Module | Responsibility |
|---|---|
| `crypto.rs`, `card.rs` | Identity, X3DH, Double Ratchet, attachment encryption and contact cards |
| `message_wire.rs`, `relay_wire.rs`, `dm_reference.rs` | Shared message, relay and original-DM reference formats |
| `security.rs`, `memory.rs`, `device_binding.rs` | Vault/KDF, memory protection and OS credential custody |
| `db.rs`, `db/message_commit.rs` | SQLCipher and atomic message/file/control metadata publication |
| `db/dm.rs`, `db/deletion_notice.rs` | Tombstones, deletion and explicit human OK receipts |
| `files.rs`, `db/file_transfer.rs` | Encrypted file staging, parts, sending windows and durable ACKs |
| `session.rs` | Agent/bot delivery, retry, groups, contacts and files |
| `relay.rs`, `relay_server.rs`, `dht_client.rs` | Relay client, built-in relay and delivery network |
| `embedded.rs`, `net.rs`, `router.rs` | In-process i2pd, I2P destinations and router configuration |
| `i2p_http.rs`, `update.rs` | HTTPS over an I2P outproxy and verified update preparation |

`Core` and `SessionManager` contain separate delivery implementations. Check
both and the browser when changing behavior. Relay formats also appear in the
separate `core/relay` workspace; contact cards have a TypeScript implementation.
Old-data migrations are not required, but current clients must interoperate.

Native GUI storage uses a vault SQLCipher key; production agent/bot paths use
OS-bound keys. Plain DBs are test fixtures, never a production fallback. Profile
copying, memory disclosure and a compromised OS have distinct limits; see
[device storage](../docs/device-bound-storage.md) and [memory](../docs/memory-security.md).
WASM excludes native SQLCipher/Tokio/i2pd/DHT/updater/OS facilities and uses portable
formats instead. Its `Arc<Zeroizing<_>>` memory wrapper lacks `mlock`/`mprotect`.
See the [browser guide](../docs/browser-messenger.md).

Message publication includes file offers/peer state and control metadata in one
DB transaction. `StagedAttachments` owns prepared ciphertext until commit and
cleans it after ordinary failures. Process death before commit can leave encrypted
orphans. This application-data boundary does not establish validation of the
entire crypto/delivery cycle.

Contact requests retain text and file-only offers, but multipart bytes and content
ACKs wait for acceptance. Limits are 16 files per message, 64 unfinished incoming
offers per profile and 16 per contact. Small files share a 1 MiB inline letter
budget; larger/excess files use 192 KiB parts, up to 2 GiB each. Progress and final
receipts follow durable state and SHA-256 verification, not text delivery.
Parts use recipient relays, bounded per-contact windows and multiple I2P streams,
without a direct-IP fallback. Group recipient queues remain until each E2E ACK.
See [files](../docs/file-transfer.md), [groups](../docs/group-delivery.md) and
[deletion](../docs/message-deletion.md).

The router is process-global. Local SAM/HTTP/SOCKS/web-console listeners are off;
NTCP2/SSU2 retain actual transport ports, with managed [UPnP](../docs/upnp.md).
Stable builds omit detailed capture; prerelease/developer/test diagnostics use a
fixed build policy. Platform limits are in [logging](../docs/release-diagnostics.md).
Updates require a valid SHA-256; a same-host checksum is not independent publisher
signing. See [update verification](../docs/update-verification.md).

Application builds and executable checks belong in GitHub Actions: security-fast,
build, i2p-embed, e2e-i2pd and browser-router workflows. Syntax checking, stubs,
native fixtures and live I2P delivery prove different properties. New changes
still need CI and phone/PC validation; see [current state](../docs/current-state.md).
