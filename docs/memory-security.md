# Защита ключей в памяти

Состояние на 28 октября 2026: реализация в неопубликованной ветке
`feat/today-integration-20261028` поверх `b32b135`. Требование владельца —
усилить защиту памяти во всех компонентах, включая headless/OpenWrt.
Описанные ниже изменения требуют проверки в GitHub CI; локальная компиляция
после этого требования не выполнялась. Это ограниченная защита ключевого
материала, а не обещание шифрования всей памяти приложения.

## Что хранится запечатанным

`libcore/src/memory.rs` выделяет отдельную страницу ОС через `mmap` или
`VirtualAlloc`. Размер берётся у ОС: фиксированного предположения о 4 KiB нет.
Страница содержит случайный 32-байтовый wrapping key и рабочую область
2048 байт. Обычная куча содержит шифртекст XChaCha20-Poly1305, nonce, tag и AAD.
Каждое новое запечатывание получает свежий nonce и случайную часть AAD;
AAD также содержит версию, назначение и размер контейнера.

Так хранятся:

- `MasterKey` разблокированного vault;
- секреты подписи и DH в `Identity`, секреты `PreKeyPair`;
- DH-secret, root/chain keys и каждый skipped message key в `RatchetState`;
- ключ внутри `AttachmentCipher`;
- ограниченные по размеру приватные I2P-ключи в `SealedBlob`, используемые
  destination и встроенным релеем;
- производный парный DHT-secret между сетевыми ожиданиями.

`Identity` кэширует только публичную карточку. `Clone` защищённых контейнеров
копирует шифртекст и ссылку на protector, а не открытый ключ.

## Доступ и отказ

При выделении страницы проверяются `mlock`/`VirtualLock`. На Linux и Android
также обязателен успешный `MADV_DONTDUMP`. Между операциями страница имеет
`PROT_NONE`/`PAGE_NOACCESS`. При доступе mutex открывает страницу, AEAD проверяет
tag и расшифровывает ключ прямо в рабочую область. `SecretGuard`/`BlobGuard`
дают синхронную ссылку; они `!Send` и `!Sync`. Guard нельзя переносить через
`await` в задаче, требующей `Send`. На том же потоке вложенное открытие
возвращает `NestedAccess`, чтобы избежать взаимной блокировки.

При уничтожении guard рабочая область стирается через `zeroize`, после чего
страница снова закрывается. Ошибка закрытия приводит к аварийному завершению,
чтобы процесс не продолжал работу с доступным ключом. Ошибки выделения,
блокировки, открытия и аутентификации возвращаются вызывающему коду; пути
`off`/`best_effort` и незаметного хранения открытого ключа нет. Совместимые
конструкторы без `Result` завершают работу при невозможности защиты; production
загрузка личности использует `try_*` API и передаёт ошибку вызывающему коду.

Одна страница совместно используется живыми контейнерами. Глобальная ссылка
слабая (`Weak`); последний владелец уничтожает wrapping key, стирает страницу,
снимает lock и освобождает отображение. Число skipped keys не увеличивает
locked working set. Само число таких ключей ограничено 1000 на ratchet,
как и допустимый разрыв; превышение возвращает ошибку, без молчаливого удаления.
Сохранённое состояние сверх нового лимита также отклоняется явно.

Это **не гарантия стирания при нажатии «заблокировать»**: фоновые задачи и
другие открытые профили могут удерживать владельцев. Для такой гарантии нужен
отдельно проверенный cancellation/drain всех задач и жизненного цикла ключей.

## Границы хранения и вычисления

Ratchet запечатывается по отдельным 32-байтовым полям. Его serde-адаптеры
сохраняют прежний bincode layout в зашифрованной базе: они последовательно
открывают только сериализуемую запись. `encrypt` и `decrypt` работают с копией
запечатанного состояния и фиксируют её только после успеха, включая новое
запечатывание. Неудачная операция не продвигает цепь.

На границе SQLCipher открытый ключ/сериализованное состояние всё ещё нужны
библиотеке. Сериализованные ratchet bytes, hex/PRAGMA для ключа базы,
временные KEK и расшифрованный master vault обёрнуты в `Zeroizing`. Для KDF
используется заранее выделенный `Zeroizing<Vec<argon2::Block>>`: стандартный
allocating helper Argon2 0.6 освобождал workspace без стирания. Во время
вычисления этот большой workspace остаётся обычной памятью; он не целиком
`mlock` и не имеет гарантии отсутствия в swap. Features `zeroize` включены
также у используемых ChaCha20-Poly1305, HMAC, SHA-2 и Argon2.
Параметр памяти из внешнего backup ограничен 8192–262144 KiB до запуска KDF
и выделения workspace; новый экспорт использует 65536 KiB. Это также закрывает
возможность заставить импорт выделить гигабайты по недоверенному заголовку.

Экспорт ключа вложения для metadata возвращает краткоживущую стираемую копию.
Metadata, plaintext сообщений, содержимое файлов, пароли ввода, JSON IPC,
копии внутри криптографических библиотек, стек и регистры этим контейнером
целиком не покрываются. `Zeroizing` сокращает время жизни известных буферов;
он не доказывает отсутствие всех копий, созданных компилятором или библиотекой.

## Возможности ОС и процесс

| Платформа | Страница | Idle access | Отдельное исключение страницы из core | Процесс |
|---|---|---|---|---|
| Linux / OpenWrt | `mmap` + `mlock` | `PROT_NONE` | `MADV_DONTDUMP` | `RLIMIT_CORE=0`, `PR_SET_DUMPABLE=0` |
| Android | `mmap` + `mlock` | `PROT_NONE` | `MADV_DONTDUMP` | `RLIMIT_CORE=0`, `PR_SET_DUMPABLE=0` |
| macOS / прочие Unix | `mmap` + `mlock` | `PROT_NONE` | этой реализацией не заявлено | `RLIMIT_CORE=0` |
| Windows | `VirtualAlloc` + `VirtualLock` | `PAGE_NOACCESS` | этой реализацией не заявлено | `SetErrorMode` подавляет диалоги, не все дампы |

`memory::capabilities()` проверяет строгий backend на текущей ОС и возвращает
его фактический размер страницы. Успех на одной машине не подтверждает работу
на другом Android/vendor kernel или всех OpenWrt-устройствах. Недостаточный
`RLIMIT_MEMLOCK`/working set означает явный отказ; приложение не увеличивает
лимиты и не вызывает `mlockall`.

Общий `i2p_embed::process_security::harden_process` вызывают GUI (включая
Android), агент, bot SDK, standalone relay и e2e harness. На Unix ошибки
`setrlimit` передаются вызывающему коду; на Linux/Android так же проверяется
`prctl`. Эти настройки относятся к соответствующему процессу. Для WebView и
native libi2pd требуется отдельная проверка их копий и процессов.

Защита снижает риск раскрытия простаивающих ключей при случайном чтении кучи,
swap и обычных дампах на поддерживаемых ОС. Она не защищает от root/kernel,
DMA, произвольного исполнения кода в процессе или привилегированного отладчика.
Внутренние долгоживущие C++-объекты libi2pd, JS/GC и SQLCipher не становятся
зашифрованными автоматически. После `fork` OS memory locks не наследуются;
поведение дочерних процессов также нельзя выводить из защиты родителя.

## WebView: время жизни открытого содержимого

Ключи E2E, vault и ratchet не передаются в JavaScript. Открытый текст для
отображения всё ещё проходит через IPC и DOM. Новая политика в `ui/src/state.ts`
хранит `Message[]` и pinned bodies только активного диалога: смена чата убирает
остальные entries, а фоновое событие обновляет unread/контактные metadata без
новой загрузки тела истории. Окно активной истории ограничено 2000 строками;
пагинация и переход к далёкому результату поиска открывают ограниченное окно.

Epoch активного чата отклоняет ответы list/pinned/pagination, завершившиеся
после переключения или начала lock. Tombstone overlay не даёт ответу,
снятому до удаления, вернуть body или pinned record. Блокировка закрывает
текстовое представление до ожидания native vault IPC, очищает Store audio
references и останавливает принадлежащие ему sources. Это не cancellation/drain
фоновых задач Rust и не доказательство уничтожения их ключей.

`ChatView.destroy()` очищает input/reply/edit, pending attachments, историю и
черновик консоли, signatures, attachment/image caches и принадлежащие ему
DOM nodes. Новое удаление также очищает активную композицию, previews и модальные
копии. Изображение хранится как ограниченный текущим представлением object URL;
при удалении или уходе URL отзывается. Старые асинхронные attachment/image/file
picker ответы не заполняют уничтоженное или удалённое представление.
App закрывает свои modals, очищает их inputs/media/DOM и image document listeners.
Смена языка сохраняет только черновик текущего представления и не заводит кеш
всех посещённых чатов.

Это сокращение известных ссылок и времени жизни копий. Immutable JS strings,
base64 IPC, `Blob`, браузерные caches, layout/GPU buffers и GC не имеют гарантии
немедленного `zeroize`. OS notifications, буфер обмена, сохранённые пользователем
файлы и сторонние copies находятся за этой границей. JS-шифрование кеша тем же
доступным процессу ключом не выдаётся за защиту всей RAM.

DOM regressions проверяют switch/lock с задержанными
list/pinned responses, фоновых событий без body fetch, stale response после
tombstone, destroy с незавершённой загрузкой вложений, image URL cleanup и
deletion-before-image-response. На checkpoint
`d907307e5b03a4c6301890d21689835f224a09a7` **2026-10-28** TypeScript/Vite и
13/13 UI tests, включая эти cases и locale draft invariants, прошли в
[GitHub Actions](https://github.com/gluckdev/gipny-i2p/actions/runs/40757759828/job/123061162378).
Это результат UI-границы данного SHA, не подтверждение всех memory/native
tests или более поздних изменений рабочего дерева.
После этого checkpoint дополнительно исправлен lifecycle `ContextMenu`:
закрытие снимает принадлежащие меню document handlers и отменяет delayed
registration; long-press удалённого DOM не открывает меню заново. Новый
отдельный regression source ожидает следующего GitHub CI.

## Native libi2pd и JNI: принадлежащие нам копии

Rust FFI создаёт приватные NUL-terminated буферы заранее с нужной ёмкостью
в `Zeroizing<Vec<u8>>`, без промежуточного долгоживущего `CString`. Shim
очищает принадлежащие ему строки через `OPENSSL_cleanse` до `free`; Base64
приватных ключей очищается после копирования на границе. Private destination
cache в Rust хранится как `SealedBlob`, guard заканчивается до network await.

Публичный patch `i2p-embed/patches/0001-clean-owned-private-material.patch`
применяется к pinned source во всех native CI paths. Он добавляет очистку
`PrivateKeys`, `B33OfflineKeys` и временных Base64 vectors, `BN_clear_free`
для private BIGNUM и очистку RedDSA private array. Нативная сборка проверяет
marker patch и отказывает при его отсутствии. Проверка контекста через
`git apply --check` прошла на исходном pinned дереве; новой локальной C++
сборки не выполнялось. Это очистка известных owned buffers; долгоживущие
private objects C++ роутера ещё не получили полного idle encryption.

Android bridge использует настоящий Tauri JVM и ClassLoader, затем GlobalRef
класса и AttachGuard фонового потока. Инициализация provider завершается
до vault create/unlock/import; UI thread не блокируется. Keystore wrapping
key не экспортируется, краткий binding secret возвращается в Java byte array,
копируется в `Zeroizing` Rust buffer, Java array очищается до закрытия JNI frame.
Это не обещание очистки внутренних копий Android crypto service.

Отдельный actual-code отзыв Claude получен 2026-10-28. Подтверждённые замечания
привели к `Zeroizing<Vec<u8>>` в возвращаемых типах backup plaintext и
`RatchetState::to_bytes`, а также к сериализации vault authentication/mutations
одним operation mutex. Он закрывает конкурентное чтение stale attempt counter
в одном объекте Vault; межпроцессная блокировка этим не заявлена. Предположение
отзыва о shared signer проверено по pinned `PrivateKeys::operator=`: signer
сбрасывается и пересоздаётся, а не заимствует private storage исходного объекта.
Замечание об отсутствии backup KDF bounds относилось к более раннему snapshot;
лимит был добавлен до получения отзыва. Ответ модели не заменяет GitHub CI.

## Проверки перед выпуском

Написаны проверки свежих nonce, отказа при повреждённом tag и очистки scratch;
жизненного цикла последнего владельца; ограниченного blob; вложенного доступа;
недоступности idle-страницы в отдельном процессе; реального отказа `mlock` при
нулевом лимите; очистки при panic и уничтожения владельца при poisoned mutex;
Linux kernel flags `lo`/`dd` и недоступного отображения; общего process hardening
в отдельном процессе; совместимости
legacy ratchet bytes; rollback при отказе encryption; лимита skipped keys.
Существующие crypto tests проверяют X3DH, out-of-order/replay, persist/restore
и успешную расшифровку после повреждённого сообщения.
Отдельная backup-проверка запрещает вход в KDF при недопустимой стоимости,
чтобы регрессия не приводила к гигабайтному выделению памяти самого CI runner.

Команды предназначены для GitHub CI с настоящими исходниками libi2pd:

```sh
cargo test --locked -p gipny-libcore --lib
cargo test --locked -p gipny-libcore --test crypto
cargo test --locked -p gipny-libcore --doc
cargo test --locked -p i2p-embed --lib process_security::tests
```

Нужны также Linux/macOS/Windows runs и Android/OpenWrt target checks.
Compile-fail doctest подтверждает `!Send`; тест `PROT_NONE` намеренно завершает
изолированный дочерний процесс сигналом без core dump. Тест отказа `mlock`
снижает привилегии только дочернего процесса, если runner запущен от root.
До результатов CI эти проверки нельзя считать пройденными. Ни native/network
доказательство, ни успешную сборку платформ этот документ не подменяет.

## Первоисточники

- [Linux `mlock`](https://man7.org/linux/man-pages/man2/mlock.2.html),
  [`madvise`](https://man7.org/linux/man-pages/man2/madvise.2.html),
  [`core`](https://man7.org/linux/man-pages/man5/core.5.html).
- [Microsoft `VirtualLock`](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-virtuallock),
  [`VirtualProtect`](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-virtualprotect).
- [RustCrypto `zeroize`](https://docs.rs/zeroize/),
  [Argon2 0.6 `hash_password_into_with_memory`](https://docs.rs/argon2/0.6.0/argon2/struct.Argon2.html#method.hash_password_into_with_memory).
