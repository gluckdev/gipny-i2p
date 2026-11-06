# Передача файлов / File transfer

## Русский

### Размеры и хранение

В текущих исходниках нативные клиенты и браузер принимают файлы до **2 ГиБ**.
В браузере файл хранится отдельно от профиля, в зашифрованном OPFS-хранилище.
Прежние ограничения браузера — 1 МиБ на отправку вложений и 4 МиБ на всё
их хранилище — заменены потоковой передачей и квотой сайта.
Общий объём ограничивает квота, выделенная браузером этому сайту; приложение
оставляет **64 МиБ** для профиля, подтверждений и оболочки. Оценка квоты
приблизительна, поэтому ошибка фактической записи всё равно обрабатывается.
Размер зашифрованной записи профиля остаётся ограничен 16 МиБ: это история,
контакты, криптографическое состояние, очередь и сведения о файлах, без их байтов.

Перед принятием предложения резервируется длина зашифрованного файла.
Браузер одновременно принимает до 64 файлов, до 16 от одного контакта.
Предложение от ещё не принятого контакта не резервирует место: сначала нужно
принять контакт. Запрос `navigator.storage.persist()` отправляется при создании
профиля; решение остаётся за браузером. Очистка данных сайта удаляет также OPFS.

В одном сообщении допускается до 16 файлов. Нативные клиенты также ограничивают
незавершённые входящие предложения: 64 на профиль, 16 от контакта. Описание файла
сохраняется вместе с сообщением, включая сообщение без текста и запрос контакта;
multipart-байты принимаются после согласия. Веб-клиент следует тому же правилу.
Суммарный бюджет маленьких вложений внутри одного письма — 1 МиБ; остальные
нативный отправитель автоматически передаёт частями. Это не предел размера файла.

Очистка метаданных JPEG/PNG/WebP и предпросмотр в браузере ограничены
**32 МиБ**, поскольку пока используют обработку в памяти. Крупный файл можно
сохранить потоком без предпросмотра. Если включённая очистка метаданных не может
обработать файл, отправка выдаёт ошибку; защита не отключается автоматически.
Очистка PDF в WASM пока недоступна. Для неподдерживаемого формата пользователь может явно изменить настройку
очистки или предварительно преобразовать файл. Нативная очистка имеет отдельный
предел 512 МиБ; это не предел потоковой передачи обычного файла.

### Доставка и подтверждения

Общий формат `WireFileOffer` / `WireFileChunk` / `WireFileAck` используется
в Core, SessionManager и браузере. Обычная часть — 192 КиБ. Предложение содержит
размер, SHA-256 и случайный идентификатор файла. Получение текста или предложения
файла ещё не означает, что вложение полностью доставлено.

* Отправитель сохраняет резервирование частей до сетевого ожидания. Поздняя
  ошибка отправки не должна перезаписывать уже полученный ACK.
* Сообщение и описание его файлов публикуются вместе. В нативной БД эта
  транзакция включает очередь получателя файла, reply, buttons и console metadata;
  подготовленные зашифрованные файлы удаляются при обычной ошибке до commit.
  Перезапуск процесса в середине подготовки может оставить ненужный шифротекст.
* Получатель сохраняет зашифрованную часть перед подтверждением. Он сообщает
  непрерывный префикс и пропуски; порядок прихода частей может отличаться.
* Полный ACK выдаётся после проверки SHA-256 всего файла. Браузер выполняет
  эту проверку потоком вне очереди криптографических транзакций. Пока проверка
  идёт, браузер не отправляет квитанций по этому файлу: последняя часть не
  считается ни полученной, ни потерянной. Нативный отправитель может за это
  время повторить её по своему таймеру; повтор безвреден.
* Состояние частей сохраняется при закрытии профиля. После открытия получатель
  сообщает сохранённый прогресс; повтор последней части получает полный ACK
  без создания второго вложения.
* Ошибка квоты, отмена или несовпадение SHA-256 не отображаются как успех.
  Плашка файла сохраняет соответствующий статус. Зелёное переливание означает
  отправку; окончательный статус зависит от подтверждений файла.

Часть личного файла связывается с автором, исходным сообщением и его временем.
Повтор не создаёт новое вложение; удаление исходного сообщения запрещает
восстановление его файла. До принятия контакта подтверждение сообщения не
отправляется. При принятии клиент сообщает сохранённое состояние файла.

В браузере до четырёх сетевых операций выполняются параллельно; исправные
соединения к релею переиспользуются. Медленное соединение занимает только своё
место: освободившееся сразу получает следующее письмо, поэтому подтверждения и
новые части не ждут самой долгой отправки. Релей, молчащий 60 секунд после
открытия потока, считается мёртвым: поток закрывается, письмо уходит повтором по
новому, не дожидаясь транспортных таймаутов записи и чтения (60 + 120 секунд).
Файл в OPFS открывается один раз на серию частей и закрывается после 10 секунд
простоя; открытие, не завершившееся за 10 секунд, — временная ошибка: часть
уходит повтором, а остальные операции с файлом не ждут зависшего вызова.
Служебные подтверждения имеют приоритет.
Шифрование и фиксация состояния ratchet остаются последовательными.
Это не переключение на прямой IP-транспорт: части по-прежнему идут через I2P.

### Повторы, окна и очереди в браузере

* Шифротекст, который релей получателя подтвердил, больше никогда не
  отправляется: получатель не может расшифровать повтор, а нативный клиент в
  ответ сбрасывает сессию. До подтверждения тот же шифротекст повторяется
  байт в байт, в том числе после перезапуска.
* Потерянная часть шифруется заново как новое письмо. Часть считается
  потерянной, если её не подтвердили за таймаут (от 30 до 120 с по измеренному
  времени ответа, 60 с до первого замера) или если получатель после
  перезапуска прислал квитанцию `resumed`. Пропуск в квитанции до таймаута —
  обычно часть, которая ещё в пути. После открытия профиля таймеры частей в
  пути отсчитываются заново, без лавины повторов.
* Обычное сообщение и запрос удаления (виды 1 и 3) после подтверждения релеем
  хранят только текст письма. Если ответа нет, письмо шифруется заново через
  2 минуты, затем с удвоением интервала до 1 часа. Сообщение повторяется до
  7 дней, запрос удаления — до ответа вида 2 или 4.
* Окно — до 8 частей в очереди или в пути на каждого получателя. Медленный,
  недоступный или заблокированный контакт не задерживает остальных. В снимке
  профиля одновременно хранится не более 16 зашифрованных, ещё не
  доставленных частей; новые части для недоступного релея не шифруются.
* Передача завершается ошибкой, если релей получателя принимает части, но
  квитанций нет 24 часа, либо если квитанций нет 7 дней. Ошибка чтения части
  останавливает только этот файл.
* Квитанции, отмены и признаки набора текста не выбрасывают ошибку при полной
  очереди: для них зарезервированы последние 256 из 2048 мест, а
  незашифрованные квитанции одного файла объединяются. Запросов удаления в
  очереди не больше 16 на контакт; остальные отправляются по мере ответов.
* Непринятый и заблокированный контакты не получают автоматических писем:
  квитанций, отмен файлов, запросов удаления. Так же поступает нативный клиент.
  Если письмо незнакомца истекло до принятия, при принятии запрос удаления не
  отправляется. Встроенные вложения непринятого контакта ограничены:
  16 файлов и 4 МиБ на контакт, 16 МиБ на все такие контакты.
* Записи о повторах ограничены: 1024 последних конверта и, без срока,
  до 64 начальных писем X3DH на отправителя. Части файлов не занимают
  постоянную историю повторов.
* Прогресс частей хранится битовой картой (около 1,4 КиБ для 2 ГиБ).
  Шифротекст в очереди хранится в base64, а части в очереди — только ссылками.
  Открытый текст части в снимок профиля не попадает. Свыше 1024 записей
  завершённые передачи забываются; их идентификаторы (до 2048) хранятся
  отдельно, чтобы поздняя часть не восстановила файл.

### Защита файлов в браузере

Для каждого файла Rust создаёт отдельный случайный ключ. В метаданных профиля
хранится только обёрнутый ключ; в OPFS — случайное имя и зашифрованные части.
XChaCha20-Poly1305 связывает часть с идентификатором файла и номером части.
Каждая запись получает новый nonce, включая повтор после сбоя.
При удалении обновляются оба слота профиля, затем удаляется ненужный файл.
Это не обещание физического стирания флеш-памяти или внешних резервных копий.

Скачивание использует одноразовый локальный URL и Service Worker: расшифрованные
части передаются по запросу браузерного потока. Ответ не попадает в CacheStorage,
ключи не передаются Service Worker, сервер сайта не участвует в скачивании.
Блокировка профиля или удаление вложения останавливают последующее чтение.
Защиту обеспечивает случайный одноразовый адрес, действующий, пока открыто
запросившее окно. Привязать скачивание к окну браузер не позволяет: переход по
ссылке скачивания не сообщает Service Worker окно-источник. Ошибка после начала
скачивания показывается в приложении.

### Проверки

Локальные node-проверки `ui/tests/browser.test.mjs` покрывают отказ от повтора
сохранённого шифротекста, таймаут и `resumed`, окно на получателя, отказ по
молчанию, резерв очереди, ограничение истории повторов на синтетической
передаче из 12 000 частей, письма непринятым контактам, обрезку записей передач
и загрузку снимков rc2. Живая проверка браузер ↔ нативный клиент для этих
изменений ещё не выполнена. Изменения этого прохода ещё требуют выполнения GitHub CI. Добавлены проверки
границ 2 ГиБ, квоты, отмены, восстановления состояния, повреждения файла и потери
последнего ACK; WASM-проверки привязки шифротекста к профилю/файлу/индексу;
браузерный сценарий передачи 5 МиБ, смены пароля, повторного открытия и потокового
скачивания. Добавлены сбои SQL при публикации, file-only запрос контакта,
повтор после принятия и проверки связи части с сообщением. Наличие сценария
не означает его успешное выполнение. Реальная
передача 2 ГиБ и измерение скорости телефон ↔ ПК остаются отдельной проверкой.

Основания для поведения хранилища: [OPFS](https://developer.mozilla.org/en-US/docs/Web/API/File_System_API/Origin_private_file_system),
[оценка квоты](https://developer.mozilla.org/en-US/docs/Web/API/StorageManager/estimate).

## English

Native and browser source implementations accept files up to **2 GiB**. Browser
attachments now live in separate encrypted OPFS files, outside the **16 MiB**
profile metadata record. The old 1 MiB sending / 4 MiB attachment-store limits
are removed. Available origin quota determines total capacity, with **64 MiB**
reserved for profile transactions and the shell. Quota estimates are advisory;
actual write failures remain errors. Clearing site data also deletes OPFS.

The browser reserves ciphertext file length before accepting an offer and allows
64 incoming transfers, at most 16 per contact. Unaccepted contacts cannot reserve
large files. Persistence is requested at profile creation and remains subject to
the browser's decision. Browser metadata cleaning and previews still have a
**32 MiB** in-memory limit; large downloads stream without previews. PDF cleaning
is unavailable in WASM. Unsupported
or oversized privacy cleaning fails explicitly, never silently bypassing the
user's setting. Native in-memory cleaning has its own 512 MiB limit.

Messages allow 16 files. Native clients also bound unfinished incoming offers to
64 per profile and 16 per contact. A file offer is committed with its message,
including file-only contact requests; multipart bytes and content receipts wait
for acceptance. Browser behavior follows the same rule. Native inline attachments
share a 1 MiB letter budget; excess small files use the multipart path automatically.
This is separate from the 2 GiB file limit.

Core, SessionManager and the browser share file offers, chunks and receipts.
Normal chunks contain 192 KiB. Text/offer delivery is distinct from completed
file delivery. Parts are reserved before network waits and persisted before
acknowledgement; delayed send failures cannot overwrite newer ACK progress.
Completion requires a whole-file SHA-256 check. Progress survives reopening,
and duplicate final parts produce another final receipt without duplicate files.
Cancellation, storage failure and bad hashes remain visible failures. Green
animation indicates sending, not successful completion.

Native message publication includes file metadata/peer state, reply, buttons and
console metadata in one DB transaction. Preparation errors remove the attempt's
staged ciphertext; process death before commit can still leave encrypted orphans.
DM parts bind the author, original message and timestamp; duplicates do not create
new attachments, and deletion forbids resurrection. Accepting a contact resumes
the retained offer and sends the content acknowledgement.

The browser overlaps at most four network operations, reuses authenticated relay
streams and prioritizes controls while keeping ratchet commits sequential. A slow
stream holds only its own slot: a freed slot takes the next letter at once, so
receipts and further parts never wait for the slowest send. A relay silent for
60 s after the stream opened is treated as dead: the stream is closed and the
letter goes again on a fresh one instead of waiting out the transport write and
read timeouts (60 + 120 s). An OPFS file is opened once per run of parts and
closed after 10 s idle; an open that does not finish within 10 s is a retryable
error, so the part goes again and later operations on the file never wait
behind the hung call. File parts continue through I2P.

Browser retransmission and queue bounds:

* A ciphertext confirmed by the recipient's relay is never sent again. The
  receiver cannot decrypt a replay, and a native client resets the session in
  response. Until the deposit is confirmed, the same bytes are retried,
  including after a restart.
* A lost part is encrypted again as a new letter. A part is lost after its
  timeout (30–120 s from the measured round trip, 60 s before the first
  sample) or when a restarted receiver sends a `resumed` receipt. A hole
  reported earlier is normally a part still in transit. Reopening a profile
  restarts in-flight timers instead of resending everything at once.
* After deposit, ordinary messages and deletion requests (kinds 1/3) keep only
  their payload. Without an answer, they are encrypted again after 2 minutes,
  doubling to 1 hour: messages for up to 7 days, deletion requests until kind
  2/4 arrives.
* Each recipient has a window of 8 queued or in-flight parts, so a slow,
  offline or blocked contact does not stall others. The profile snapshot holds
  at most 16 encrypted, undelivered parts; parts for a failing relay are not
  encrypted. A transfer fails after 24 hours of stored parts without receipts,
  or after 7 days without any receipt. An unreadable part fails only its file.
* Receipts, cancellations and typing indicators never fail when the queue is
  full. They may use the last 256 of 2048 slots, and unencrypted receipts for
  one file are merged. At most 16 deletion requests per contact are queued.
* Pending and blocked contacts receive no automatic letters: no receipts,
  file cancellations or deletion requests, matching native clients. When a
  stranger's message expires before acceptance, accepting the contact does not
  send a deletion request. Inline attachments from pending contacts are capped
  at 16 files and 4 MiB per contact, 16 MiB in total.
* Replay records are bounded: the 1024 most recent envelopes, plus up to 64
  X3DH initial letters per sender kept for the profile's lifetime. File parts
  do not enter the lifetime history.
* Part progress is a bitmap (about 1.4 KiB for 2 GiB). Queued ciphertext is
  stored as base64, queued parts as references only, and part plaintext never
  enters the snapshot. Beyond 1024 records, completed transfers are forgotten
  and up to 2048 closed ids prevent a late part from restoring a file. Rust owns independent file keys; only wrapped tokens
enter profile metadata. Each OPFS part uses a fresh XChaCha20-Poly1305 nonce and
authenticated file/index context. Erasure rewrites both profile slots before
removing obsolete ciphertext, without promising physical media erasure.

Downloads use a one-use local URL and Service Worker backpressure. Plaintext is
streamed in bounded parts, never cached; keys remain in the WASM worker. Locking
or deleting the attachment prevents further reads. The website server does not
serve the attachment. The random single-use URL, valid while the requesting
window is open, is the protection: a download navigation does not identify its
window to the Service Worker. Errors after the download starts are reported in
the app.

Local node tests in `ui/tests/browser.test.mjs` cover no replay of stored
ciphertext, timeout/`resumed` retransmission, per-recipient windows, silent
transfer failure, the receipt reserve, bounded replay records over a synthetic
12,000-part transfer, pending-contact silence, transfer pruning and rc2
snapshot loading. A live browser ↔ native run of these changes is still
pending. New regression scenarios are prepared for **GitHub CI, not yet executed for this
change**: 2 GiB boundaries, quota/refusal, cancellation, reopen, hash failure,
lost final ACK, WASM context binding, and a 5 MiB browser transfer followed by
password rotation/reopen and streaming download. Actual 2 GiB delivery and
phone/PC speed measurements require separate validation. New scenarios also cover
SQL publication failure, file-only contact requests, acceptance and file/message
reference validation. Storage behavior follows
[OPFS](https://developer.mozilla.org/en-US/docs/Web/API/File_System_API/Origin_private_file_system)
and [StorageManager.estimate](https://developer.mozilla.org/en-US/docs/Web/API/StorageManager/estimate).
