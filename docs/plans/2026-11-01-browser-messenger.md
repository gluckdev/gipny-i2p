# Браузерный мессенджер: реализация и статус интеграции

Дата: **2026-11-01**. Рабочая ветка — `feat/browser-messenger`, PR — [#144 (DRAFT)](https://github.com/gluckdev/gipny-i2p/pull/144).
Базовый коммит: `318b2d5` (слияние PR #143 router spike в `main`).
Все сборки и исполнение тестов выполняются **исключительно в GitHub Actions**. Локальные сборки не запускаются и не инструктируются.

Результат: существующий интерфейс Gipny работает с ядром в Web Worker, создаёт собственную личность и зашифрованный профиль в OPFS, содержит путь открытия клиентской I2P destination через in-process emissary и обмена сообщениями. Сквозная доставка ещё проходит CI-приёмку.

---

## Архитектура и реализованные компоненты

1. **Общее криптографическое ядро и транспорт (WASM):**
   - Переиспользование исходников `gipny-libcore` (X3DH, Double Ratchet, ContactCard, Relay framing, wire format) в портативной WASM-сборке `browser-router`.
   - Прямой in-process client API emissary и типизированные потоки памяти (`ClientSession`). SAM/I2CP TCP/UDP listener'ы полностью отсутствуют; сетевой стек не открывает локальных портов.
   - Pinned `third_party/i2pd` зафиксирован на ревизии `62c013003233df3cc8ab6563563848ff3c316330`, сборщик браузерных пакетов генерирует актуальные ссылки на git-дерево и новый manifest.

2. **Криптографический актор и изоляция памяти (`BrowserEngine`):**
   - Все приватные ключи (`Identity`, client identity, SPK, OTKs, ratchet session states) хранятся исключительно внутри Rust Web Worker (`BrowserEngine`).
   - API не экспортирует приватный ключевой материал; доверенный JS Worker имеет доступ к WASM memory. JS оперирует только публичными метаданными (hex-ключи, карточки) и открытым текстом сообщений, необходимым для UI.
   - Транзакционная модель хранилища: операции модификации состояния выполняются в рамках `begin()`, `seal()`, `commit()` / `rollback()`.
   - Границы браузерного режима: **отсутствуют** нативные гарантии `mlock`/`mprotect`, `PROT_NONE`, `MADV_DONTDUMP`, SQLCipher, аппаратный keychain, duress-пароли и деструктивные счётчики попыток (при попытке создания профиля с duress или limit attempts возвращается явная ошибка).

3. **Хранилище OPFS и WebLocks (`ProfileStorage`):**
   - Защита от параллельного доступа через эксклюзивные Web Locks (`gipny/browser/profile-lock/v1:<hash>`) и синхронные файловые дескрипторы OPFS SyncAccessHandle.
   - Двухслотовое хранилище (`vault-a.bin`, `vault-b.bin`) с чередованием поколений. Запись и flush следующего слота завершаются строго до сетевой отправки или фиксации криптографического шага.
   - KDF пароля: Argon2id с параметрами 64 МиБ (65536 KiB), 3 итерации (t=3), 1 поток (p=1).
   - Поддержка passkey: W3C WebAuthn Level 3 PRF (hmac-secret) с выводом ключа через HKDF-SHA256.
   - Лимиты хранилища: максимальный размер профиля — 16 МиБ (`MAX_RECORD`), вложения inline — до 1 МиБ на сообщение (суммарно до 16 файлов), текст сообщения — до 131072 единиц UTF-16 (длина строки JS).
   - Защита DEK при смене пароля: новый wrapper DEK зеркалируется в оба слота только после успешного завершения flush первого слота (`replaceRecovery = true`). Ранее скопированные резервные копии сохраняют старый пароль.

4. **Адаптер Worker и жизненный цикл (`BrowserBackend` / `BrowserCoreClient`):**
   - Принудительное завершение и сброс ключей при блокировке, `pagehide` или засыпании вкладки (`freeze`). При перезагрузке состояние возобновляется через ввод пароля / passkey.
   - Очередь исходящих (outbox): сохранение состояния ratchet в OPFS выполняется строго перед физической передачей пакета релею и перед подтверждением приёма.
   - Устойчивость доставки: точный шифротекст сохраняется для повторных попыток (включая перезапуск вкладки); явный сброс сессии контакта (`reset_contact_session`) повторно шифрует сохранённую полезную нагрузку (удаляя истёкшие записи без plaintext).

5. **Релей, карточки и сетевые ограничения:**
   - Личный встроенный релей (`BrowserRelay`) и экспорт контактов через ContactCard (`gipny:v3:`).
   - DHT в браузере отсутствует; адреса релеев передаются непосредственно через карточки контактов и входящие конверты.
   - Отсутствуют: нативная консоль управления, чанковая передача больших файлов, транзитная маршрутизация I2P и автоматические обновления бинарников (обновления осуществляются через Service Worker PWA).

6. **PWA и offline-оболочка:**
   - Service Worker (`browser-sw.js`) кэширует статические ресурсы интерфейса (HTML, CSS, JS, WASM, иконки, звуки).
   - Конфигурация сети (`browser-config.json`), снапшоты netDb и пользовательские данные **явно исключены** из кэша PWA. При отсутствии сети зашифрованная история доступна offline.

7. **Граница форвардера:**
   - WSS-форвардер выполняет байтовую ретрансляцию NTCP2-трафика на основе строгого allowlist проверенных RouterInfo (RI), валидации TLS, Host, Origin и capability-токена текущего запуска.
   - Публичный сервер, рабочий домен и боевые TLS-сертификаты в репозитории не предоставляются.

---

## Матрица статуса (Done / In Progress / External Blocked)

Проверенный код — `fa60b02`, [run 41329839707](https://github.com/gluckdev/gipny-i2p/actions/runs/41329839707):
полный двухбраузерный CI acceptance, восстановление очереди и offline-история PASS.
Прошли 46 WASM, 89 session и 14 routing-path тестов, обе холодные загрузки,
платформенные проверки и CodeQL. Последующие правки документации фиксируют
этот проверенный SHA. Физические телефоны и публичный хостинг не проверены.

| Компонент / Задача | Статус | Проверяемый факт / Ссылка |
|---|---|---|
| **WASM-рантайм и Router Spike** | **DONE** | PR #143 объединён в `main` коммитом `318b2d5`. Upstream emissary зафиксирован с проверенным патчем `client-only-ntcp2.patch`. |
| **Портативное криптоядро (libcore in WASM)** | **DONE** | Bincode, Double Ratchet, X3DH, Ed25519, X25519, ContactCard скомпилированы под `wasm32-unknown-unknown`. |
| **In-process ClientSession** | **DONE** | `browser-router/src/network.rs`: прямые typed memory streams без открытия SAM/I2CP сокетов. |
| **BrowserEngine Actor & Transactions** | **DONE** | `browser-router/src/engine.rs`: изоляция приватных ключей в Rust, транзакции `begin`/`commit`/`rollback`. |
| **Шифрованный OPFS Vault & WebLocks** | **DONE** | `ui/src/browser-core/storage.ts`: dual-slot flush, WebLocks, Argon2id 64 MiB t3, passkey PRF wrapping. |
| **Worker UI Adapter & Outbox** | **DONE** | `ui/src/browser-core/backend.ts`: персистентность ratchet до отправки, exact ciphertext retry. |
| **PWA Cache & Offline Shell** | **DONE** | `ui/vite.browser.config.ts`: генерация `browser-sw.js` без кэширования сетевого конфига/снапшотов. |
| **Пин нативного i2pd** | **DONE** | Зафиксирован `62c013003233df3cc8ab6563563848ff3c316330`, проверка через `.github/scripts/assert-i2pd-pin.sh`. |
| **CI: Security fast & UI typecheck** | **DONE** | На `fa60b02`: security fast и UI typecheck PASS. |
| **CI: WASM Release compilation** | **DONE** | `wasm-pack build --target web --release --locked` компилируется без ошибок. |
| **CI: Real NTCP2 Handshake** | **DONE** | Реальное рукопожатие NTCP2 в Chrome headless PASS. |
| **CI: EngineTests** | **DONE** | `fa60b02`: 9/9 engine tests, всего 46/46 WASM tests PASS. |
| **CI: Transport recovery** | **DONE** | `fa60b02`: 3 явные регрессии LeaseSet/ключей, все 89 session и 14 routing-path тестов PASS. |
| **CI: Cold Bootstrap** | **DONE** | На `fa60b02` desktop и constrained PASS (38,31 / 145,07 с до готовности). Длительность зависит от публичной сети; результаты одного прогона не задают SLA. |
| **CI: 2-Browser Live Integration** | **DONE** | `fa60b02`: все 7 assertions PASS, включая доставку очереди после восстановления без дубликатов и offline-историю при выключенном HTTP-сервере. [Подробные результаты](../browser-messenger.md#validation-and-artifacts). |
| **Физические мобильные устройства** | **EXTERNAL BLOCKED** | Проверка на физических телефонах iOS Safari и Android Chrome не проводилась (десктопная эмуляция не заменяет устройство). |
| **Публичный хостинг форвардера и netDb** | **EXTERNAL BLOCKED** | Требуются серверная инфраструктура, домен, TLS-сертификаты и безопасный подписанный источник снапшотов netDb. |

> [!NOTE]
> Предыдущие показатели (27 тестов и 2 успешных холодных старта) относятся исключительно к **истории проверки концепции роутера (router spike)** на коммите `6b37dc2` / `318b2d5` и не означают готовности полного мессенджера.

---

## Критерии готовности (Acceptance Gates)

До выполнения следующих условий браузерный мессенджер **не объявляется готовым продуктом или релизом**:

1. **WASM & Engine Tests Gate — PASS на `fa60b02`:** 9/9 engine и 46/46 WASM-тестов; новые изменения кода требуют проверки своего коммита.
2. **Cold Bootstrap Gate — PASS на `fa60b02`:** оба профиля. Более ранние сбои остаются в истории и сохраняют ограничение стабильности живой сети.
3. **Two-Browser Live Acceptance Gate — PASS на `fa60b02`:** `run_messenger_ci.py` подтвердил обмен, восстановление профиля/очереди и offline-историю между двумя независимыми Chrome через реальную сеть I2P.
4. **Physical Mobile Device Gate — EXTERNAL BLOCKED:** Требуется проверка работоспособности, криптографической производительности и жизненного цикла вкладки на физических iOS Safari и Android Chrome.
5. **Infrastructure & Hosting Gate — EXTERNAL BLOCKED:** Требуются сервер, домен/TLS и развёртывание защищённого WSS-форвардера с RouterInfo allowlist и доверенным источником снапшотов netDb. Инструкция подготовлена; публичное размещение не выполнено.
