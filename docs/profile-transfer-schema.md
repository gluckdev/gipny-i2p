# Переносимый профиль: формат и порядок реализации

Обновлено: 2026-11-16. **Перенос профиля ещё не готов.** Это общий контракт
native/WASM и список оставшихся adapters; он не заменяет
[порядок подтверждения и удаления](profile-transfer.md).

## Уже реализованные части схемы

`libcore/src/profile_transfer/portable/` содержит crypto-state codec,
manifest layout, а также bounded доменные схемы контактов (`contacts.rs`),
групп (`groups.rs`) и точных UTF-8 строк (`text.rs`) с перекрёстной проверкой
привязки ключей (cross-domain key pinning). Формат version 1 использует exact
little-endian fixed-integer bincode с запретом trailing bytes. Это новый формат;
прежние backup/vault containers не являются его входом.

- Crypto domain сохраняет application signing/DH secrets, current и historical
  signed prekeys, оставшиеся OTK, next-ID high-water mark, primary/lost/alternate
  ratchets и pending own initializations. Public keys проверяются по secrets;
  duplicate keys/IDs и reuse ID отвергаются. Ratchet decoding сохраняет общий
  skipped-key bound и отвергает лишние bytes.
- Contacts/Groups domains сохраняют trust/admission статус без возможности
  самовольного повышения доверия при декодировании, валидируют адреса I2P
  (отвергая clearnet/HTTP/относительные пути), гарантируют стабильность
  идентификаторов и отсутствие дублей. Проверка `Index` сопоставляет
  участников групп и сессии ratchet с проверенными контактами и собственной
  личностью, предотвращая подмену ключей.
- Secret types не имеют `Debug`/`Clone`, очищаются при уничтожении. Декодеры
  очищают частично заполненные secret buffers при ошибке. Это меры уменьшения
  времени жизни plaintext, не доказательство отсутствия всех compiler/OS copies.
- Manifest перечисляет девять доменов в фиксированном порядке, record offsets,
  длины и SHA-256. Layout проверяет contiguous coverage без overlap/holes и
  становится immutable. Проверка record digest **не означает** AEAD authentication,
  fsync или Prepared: эти обязательства принадлежат ещё не готовым adapters.
- Allocation bounds проверяются до доверия serialized lengths: metadata codec
  до 16 MiB, ratchet до 128 KiB, peers до 1024, signed и one-time keys по 256,
  alternates до 3 на peer. Manifest до 65 536 records по 256 KiB, суммарно до
  16 GiB. Это format ceilings, не обещание, что каждая платформа примет весь объём.
  IDs ограничены точными JS integers. Receiver capability/quota preflight будет
  проверяться дополнительно; данные нельзя обрезать для прохождения лимита.

## Mapping полного профиля

| Переносимый домен | Native источник | Browser источник | Обязательная семантика |
| --- | --- | --- | --- |
| Identity | DB settings `identity_sign`, `identity_dh`; Core.identity | Engine VaultPayload signing seed/DH secret | Сохранить application identity. Missing key — отказ, не generation fallback |
| Prekeys | prekeys + signed_prekey_id + allocator high-water | spk/otks/next_otk_id | Не возрождать spent OTK; current и still-accepted historical SPK; ID не переиспользовать |
| Ratchets/initializations | DB sessions и owned RAM maps | sessions/lost_inits/own_inits/alternates | Согласованный snapshot после drain; сохранить skipped keys и pending X3DH, не молча reset sessions |
| Contacts/groups | contacts, groups, members | metadata contacts/groups.members | Sign/DH/address/relay/trust/request state; stable natural keys, точный remap local IDs |
| Messages | messages, buttons/console metadata, pins/replies | StoredMessage и metadata | origin/author/chat/reply/pin/TTL/read/delivery; FTS/indexes пересоздать |
| Reliability | durable inbox, pending_outbound, retry state, in-flight workers | outbox payload/unconfirmed ciphertext/deposited/initial, seen/initials | Не decrypt consumed envelope второй раз; не терять committed plaintext inbox; pending initial ciphertext сохранить точно |
| Deletion/replay | dm tables, tombstones, counters, pending notices/deletes, blocked files | deletion/peer_reports/tombstones/initials/seen/closedFiles | Suppression и replay state сохраняются; поддержка в receiver нужна, одних сохранённых неиспользуемых bytes недостаточно |
| Attachments | key/path/chunk_size и sealed files | inline data или OPFS StoredBlob/key token | Stream authenticated logical bytes; fresh receiver local file keys; full digest/readback. Absolute paths и wrapped source tokens не копируются |
| File progress | file_in/out/peer/early + held bits | transfers/held/flight/earlyParts/closedFiles | File ID/peer/offer/hash и подтверждённые ranges сохранить; unauthenticated partial ciphertext не даёт ACK |
| Preferences/policy | типизированная часть settings | displayName/ui/muted/pins | Explicit mapping, без blind copy private settings/OS tokens/executable jobs. Unsupported required policy — отказ |

Пока implemented crypto codec не имеет adapters ни для Core, ни для BrowserEngine.
Остальные домены перечислены в manifest, но их typed codecs/import/export ещё
должны быть реализованы. **Нельзя отправить один crypto domain и назвать это
полным переносом.**

Native `vault.bin` хранит wrapped local MasterKey; application identity находится
в SQLCipher settings. Browser metadata version 2 и private EngineState находятся
в vault, а attachments отдельно в OPFS. Поэтому копирование одного vault/DB
файла не является cross-platform snapshot.

Нужны отдельные решения для current browser single SPK против native SPK history,
timestamped browser own-initial grace против native RAM marker, canonical durable
inbox/replay/outbox semantics и больших профилей, превышающих browser vault limit.
До реализации соответствующего adapter capability должен отвергать такое
состояние до Prepared, а не пропускать/сбрасывать его.

Local encryption keys, DeviceBind/OS handles/WebAuthn credentials/RP binding не
переносятся как рабочие credentials. Receiver создаёт своё binding и проверяет
открытие карантина до Prepared. Обычная I2P Destination также не тождественна
application identity: native Destination сейчас ephemeral, browser identity bytes
сохраняются. Правило новой Destination/announcement после активации должно быть
явным. Dedicated transfer-only rendezvous keys отдельно переживают source erase.

## Оставшиеся конечные этапы

1. Typed домены и cross-reference validation всей mapping выше; streamed
   attachments, complete state-vector fixtures native↔browser. Нет raw SQLCipher
   copy, opaque browser JSON вместо семантической схемы или legacy migration.
2. Единый owner/freeze barrier, drain всех mutations, durable journals и admission
   до любого ordinary open. Quarantine вне обычного namespace. Native sync/CAS и
   browser exclusive OPFS/Web Locks с реальными fault tests.
3. Strict idempotent eraser всех принадлежащих профилю ключей/файлов/scratch.
   Unknown/failed removal запрещает activation. Одного key revoke недостаточно.
   Source CommitDecided сохраняется до удаления, Erased — до сообщения activation.
4. Выбранная проверенная PAKE библиотека, mutual confirmation с bound transcript,
   AEAD records с уникальными nonces и durable verified bitmap. Local password
   check отдельно от remote attempts/duress. Никакого самодельного PAKE.
5. Persistent transfer-only I2P endpoints и decision recovery/polling, 2/2 hops и
   padding. Native/WASM adapters и backend APIs; source final OK/Cancel только
   после полного durable Prepared. У Prepared receiver нет local timeout/cancel.
6. Сквозные четыре направления native/browser, restart/drop/replay/quota/disk
   faults, независимый review реальных adapters, затем включение UI. Успешные
   model/codec tests не заменяют этих проверок и не разрешают beta publication.

Foundation проверяется отдельными state-machine/fault/BFS tests. Portable tests
проверяют actual delayed skipped-key decrypt после roundtrip, продолжение ratchet
в обе стороны, replay rejection, prekey history/high-water, hostile sequence
lengths, all fixed-key truncations и manifest gaps/overlap/trailing bytes. Это
проверки соответствующих ограниченных модулей, не security certification переноса.
