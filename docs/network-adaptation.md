# Адаптация сети и возобновление файлов

Дата проверки: 2026-11-16. Исходная точка: `release/0.5.0-beta.1`, `f73fd42`. Это исследование и порядок beta работ; новые контроллеры и adapters ещё не реализованы. Изменения upstream, туннельной политики и production file/relay code в рамках исследования не выполнялись. Alpha2 не публикуется; старые CI — baseline.

## Автонастройка передачи файлов в beta

Native начинает с одного используемого потока на контакт. При занятом окне
он сравнивает скорость подтверждения частей за интервалы от 8 секунд и 16
новых подтверждений. Добавляет один поток, удерживает его при приросте от 10%,
иначе возвращается назад и ждёт минуту перед следующей пробой. Таймаут,
который сокращает окно, сокращает и целевое число потоков. Обычный предел —
четыре, общий предел окна — 16 частей. `GIPNY_FILE_LANES=1..8` сохраняет
прежний режим измерений. Уже открытые соединения могут использовать другие контакты.

Браузер начинает с двух параллельных отправок частей. Подтверждения увеличивают
лимит до четырёх, просрочка сокращает его до одного или двух. Общая очередь
получателя ограничена восемью частями. Эти пределы относятся к вложениям.
Результаты старых измерений ниже получены до включения автоматики.

## Подготовка личного аудиозвонка

Проектирование на 18 ноября 2026 года. В форке Mattermost Calls клиент
получает микрофон через `getUserMedia`, подключает WebRTC с параметрами ICE
и читает статистику выбранного соединения через `getStats`. Сервер плагина
запускает RTC service или подключается к rtcd. Ветка main форка при сверке
отставала от upstream на 94 коммита. [Клиент форка](https://github.com/gluckdev/mattermost-plugin-calls/blob/cd6da06fc32678b8ab83a21a1fedbe084315798a/webapp/src/client.ts) ·
[Сервер форка](https://github.com/gluckdev/mattermost-plugin-calls/blob/cd6da06fc32678b8ab83a21a1fedbe084315798a/server/activate.go) ·
[Клиент upstream](https://github.com/mattermost/mattermost-plugin-calls/blob/main/webapp/src/client.ts).

Element Call выделяет сигнализацию MatrixRTC и пересылку медиа через LiveKit.
В обоих проектах медиа имеют собственное соединение и собственные измерения.
[Архитектура Element Call](https://github.com/element-hq/element-call#readme).

Для реализации поверх I2P приглашение должно иметь короткий срок жизни.
После принятия нужен двусторонний обмен пробами через выбранный медиаканал.
Измерять RTT, вариацию интервалов прихода и долю просроченных кадров в обоих
направлениях. До включения микрофона дать управляющим пакетам приоритет
перед файлами. Начинать с одного канала и низкого битрейта. Добавление
канала проверять по времени доставки кадров при нагрузке. Для варианта
с SFU такие измерения нужны на соединении WebRTC. Файловый ACK и время
приёма сервером отражают другой участок маршрута. Подтверждение работы
требует слышимого двустороннего аудио, в том числе при параллельной передаче файла.

## 1. Что действительно работает

| Уровень | Native сейчас | Browser сейчас | Граница утверждения |
| --- | --- | --- | --- |
| Между I2P routers | NTCP2/TCP и SSU2/UDP включены одновременно | NTCP2 реализован в браузерном router | Это I2NP transport, а не прямой TCP/UDP до собеседника |
| Доступ браузера к первому I2P peer | Не требуется bridge | WSS либо WebRTC DataChannel → собственный byte forwarder → TCP/NTCP2 peer | Router, Destination и криптографическая identity остаются в браузере |
| WebRTC | Не добавляется к native автоматически | Реализован, не только проект; ordered reliable DataChannel несёт NTCP2 ciphertext | Это не SSU2 и не UDP socket API браузера |
| SSU2 в браузере | — | `BrowserUdpSocket` явно unsupported | Понадобится отдельный datagram adapter и проверка совместимости, а не переименование WebRTC |
| UPnP | Уже есть собственный управляемый IPv4 IGD worker | Browser API не управляет IGD; работа bridge/TURN — отдельная серверная конфигурация | Нельзя объявлять новую поддержку UPnP без учёта существующего worker |
| QUIC / WebTransport | Не подключены как I2P transports приложения | Не реализованы как byte/datagram adapter | Отдельный эксперимент с совместимым сервером; готового fallback нет |
| ICMP | Не реализован как data transport | Raw ICMP API нет в используемом runtime | Не включать в список резервных путей |

I2P описывает NTCP2 и SSU2 как текущие router-to-router transports; они передают I2NP, а не произвольный application stream. I2P может выбирать транспорт независимо от наших потоков доставки. Устаревшие NTCP/SSU не следует предлагать как новый fallback. [I2P Transport Overview](https://www.i2p.net/en/docs/overview/transport/)

NTCP2 аутентифицирует router и защищает router-to-router обмен; SSU2 использует UDP, имеет механизмы NAT traversal и migration, но не является QUIC и не предоставляет приложениям QUIC streams. [NTCP2 specification](https://www.i2p.net/en/docs/specs/ntcp2/), [SSU2 specification](https://i2p.net/en/docs/specs/ssu2/)

Локальные основания: `libcore/src/embedded.rs:35–88`, `browser-router/src/router.rs:18–40`, `browser-router/src/socket.rs:65–103`, `browser-router/src/unsupported.rs`, `ui/src/browser-core/networking/rtc-transport.ts`, `browser-router/forwarder/src/lib.rs`. Здесь ссылки с диапазонами — обычные текстовые координаты для поиска, не обещание стабильных line numbers после дальнейших изменений.

Application pools сохраняют configured 3 inbound / 3 outbound hops и padding. Exploratory pools — другой уровень: native сейчас 2/2 hops, 2+2 quantity; нельзя превращать заявление об application policy в заявление обо всех внутренних пулах. Адаптивный file controller не должен менять ни те, ни другие параметры.

## 2. Browser WSS/WebRTC: доказательства и оставшиеся проверки

На SHA `2139725` [browser CI 43891713781](https://github.com/gluckdev/gipny-i2p/actions/runs/43891713781) прошёл full messenger scenario отдельно через WSS и WebRTC: по 11 acceptance assertions, verified 5 MiB attachment, ratchet messages/receipts, restart recovery и доставка после остановки application host. Оба receipt содержат configured application policy 3/3+padded. Это не независимое наблюдение фактических hops на wire. Публичные receipts находятся в artifacts указанного CI run; проверку нужно повторить на конечном beta SHA.

Оба receipt явно говорят `physical_phone:false`, `turn_relay_tested:false`. Поэтому CI подтверждает реализованный byte path и сценарий на fixture, но не доступность публичного TURN, работу на телефоне или в сети с заблокированным UDP. Сравнение полных сценариев WSS 680.31s / WebRTC 718.05s не является транспортным throughput benchmark: в них входят setup, UI, ожидания и разные моменты состояния I2P.

Фактические ограничения собственного adapter:

- Выбор `wss` / `webrtc` явный. `rtc-transport.ts:1–2` прямо фиксирует отсутствие automatic WSS fallback. Ошибка WebRTC не должна описываться пользователю как успешное переключение транспорта.
- Browser ICE policy по умолчанию `relay`; конфигурация требует TURN URL, username и credential. `all` требует публичный IPv4 candidate; LAN, mDNS, IPv6 и TCP candidates фильтруются, кроме явно ограниченного loopback CI режима. Это реальная граница IPv6-only совместимости.
- WSS остаётся signaling/lifetime каналом даже при WebRTC. DataChannel не обходит блокировку signaling endpoint.
- Forwarder `/rtc` требует корректного public IPv4/bind. Один PeerConnection использует свой UDP socket; deployment должен разрешать выбранный диапазон UDP. NAT rewriting/allowlist — часть существующего server adapter, не identity server.
- Передача ограничена очередями/кредитами, handshake deadline, лимитами session/bytes; закрытие Worker/profile закрывает принадлежащие ему RTC sessions. Числа текущих caps описаны в `browser-router/forwarder/README.md`; это ресурсные ограничения, не доказанный оптимум производительности.

WebSocket API соединяется с сервером, не предоставляет произвольный raw networking. WebRTC DataChannel использует SCTP/DTLS/ICE и может иметь разные reliability/ordering режимы; наш NTCP2 adapter выбирает упорядоченную надёжную доставку, поэтому произвольная замена её на unordered/unreliable сломает stream contract. [WebSockets standard](https://websockets.spec.whatwg.org/), [RFC 8831](https://www.rfc-editor.org/rfc/rfc8831.html)

WebRTC standard требует поддержку TURN-over-TCP и TURN-over-TLS/TCP для обхода блокировки UDP со стороны клиента; это не равно TURN TCP relay candidate до peer. У нас SDP сохраняет UDP candidates, что само по себе не исключает UDP relay candidate, достигнутый через TCP/TLS к TURN. Но такой путь надо реально проверить с `rtc_ice_policy:relay`, заблокированным прямым UDP и receipt выбранной candidate pair. Наличие стандартного browser API не доказывает корректную конфигурацию TURN. [RFC 8835, transport requirements](https://www.rfc-editor.org/rfc/rfc8835.html)

Нужные beta проверки: настоящий публичный bridge; TURN UDP/TCP/TLS отдельно; истёкшие credentials; UDP blocked; browser IPv6-only; WSS-only proxy; captive portal; недоступный signaling; отключённый WASM; unsupported RTCPeerConnection; lock/profile switch во время ICE. Не логировать ICE адреса/credentials в пользовательский отчёт. Для публичного TURN ограничивать relay peers адресами bridge и сроком credentials; текущие рекомендации есть в собственном forwarder README. Не считать world-readable browser config секретным хранилищем.

Будущий automatic WSS fallback возможен только как явно разрешённая политика поверх **того же browser-owned router**, с bounded attempt/backoff, актуальной generation и capability/config validation. Он не должен незаметно выбирать server-owned router или contact-to-contact ICE. Сначала требуется доказать корректное закрытие старого adapter и отсутствие позднего открытия соединений предыдущего профиля.

WebTransport — browser/server API, для которого нужен совместимый сервер; это не raw UDP к существующим I2P routers. QUIC обеспечивает multiplexed streams, но общий congestion budget соединения остаётся общим. Вывод для нашего проекта: инкапсуляция QUIC поверх ordered WSS/TCP не устраняет HOL нижнего TCP; standalone QUIC endpoint не превращается автоматически в совместимый I2P transport. [WebTransport specification](https://www.w3.org/TR/webtransport/), [RFC 9000](https://www.rfc-editor.org/rfc/rfc9000.html)

## 3. Native UPnP и смена сети

Собственная реализация уже существует: `i2p-embed/src/upnp.rs` и `upnp/{discovery,mapping,ownership,protocol,session}.rs`. Она обслуживает только router TCP/UDP ports, проверяет владение правилами перед renewal/delete, использует per-gateway tag, публикует внешний адрес после подтверждённого mapping и debounce сетевых уведомлений. Выключенный UPnP не должен запускать worker/создавать его state. Это не публикация приложения, SAM или proxy listener.

`libcore/src/router.rs` включает UPnP по умолчанию, если пользователь не выключил его. `libcore/src/embedded.rs` содержит process-global router: настройки первого старта действуют до перезапуска процесса. Смена профиля не является перезапуском global router и не должна без основания удалять/создавать mappings.

Предлагаемый beta scope: проверить уже существующий путь на реальном IGD, повторных network events, Wi-Fi→mobile→Wi-Fi, offline/cancel/shutdown, stale owned mapping и чужом mapping. Не обещать обход CGNAT/операторского firewall и не добавлять PCP/NAT-PMP/IPv6 pinholes в список реализованных возможностей. Готовый UPnP worker не означает входящую достижимость в любой сети. Общие настройки i2pd нужно отличать от собственных defaults приложения. [i2pd configuration](https://docs.i2pd.website/en/latest/user-guide/configuration/)

## 4. N+1: измеримый следующий шаг до адаптивного controller

Это предлагаемый дизайн, не текущий production behavior.

Сначала зафиксировать смысл N+1: **один существующий main stream под control и N bulk streams внутри общего cap C**, а не незаметно C+1 соединений. Например, при C=4 сравнить текущие 4 mixed streams и 1 control + 3 bulk. Не добавлять Destination или tunnel pool. При отсутствии extra stream начинать через main с прежними bounded budgets, затем перестать ставить на main новые file frames; уже поставленные frame нельзя магически убрать.

Подтверждённые очереди и HOL paths разобраны в `/tmp/gipny-alpha2-control-lane-review.md`: основной relay FIFO общий для коротких Send/Ping/ACK и части file frames; file round-robin включает main; sender последовательно await-ит enqueue, а receiver обрабатывает целый frame перед следующим. Relay receiver выбирает collect lane по числу frames/unacked, не по bytes/type. Поэтому sender reservation может помочь, но не обещает устранить receiver HOL; ciphertext не даёт relay безопасно классифицировать сообщения по смыслу без изменения протокола/утечки metadata.

Текущий `WINDOW=16`, part=192 KiB: максимум новых логических частей окна около 3 MiB plaintext, около 4 MiB после padding. Это **не hard cap всех физических bytes в очередях**: повторные отправки, старые queued copies, startup/restart adoption и общие FIFO учитываются отдельно. Открытые lanes в существующем `file_metrics` — не число одновременно busy streams.

Предлагаемый первый эксперимент: paired A/B/B/A, одинаковые cap=4, file size, probe policy, 3/3 hops, padding, tunnel counts и bandwidth. A — текущие mixed lanes; B — main reservation. Собирать verified unique-file goodput, RTT p50/p95/p99+jitter, deadline/lost/late probes, duplicate/retry bytes, queue age и queued bytes раздельно main/extra, duration полного write/flush, receiver processing age. Метаданные только агрегатные; не писать plaintext, адреса или contact identifiers. Для причинного вывода нужен repeat на нескольких независимых network intervals.

Нельзя объявлять причиной старого baseline именно HOL: source подтверждает возможный путь задержки, но baseline не содержал queue-age measurements. Повтор 6/4/6/4 оборвался после трёх точек; третья точка имела устаревший initial report, а не измеренный ноль bytes. Исправление `f73fd42` сохраняет checkpoint и единый setup+measurement deadline; до сравнения вариантов нужен новый валидный прогон с этим harness.

## 5. Controller: медленный внешний контур и явные ограничения ресурсов

В коде уже есть внутренний `Flow`: общий per-recipient congestion window, srtt/rttvar/RTO, backoff, обработка перестановки/повторной отправки (`libcore/src/files.rs:550+`). Внешний lane controller должен менять только число используемых/open streams и admission bytes, не умножать WINDOW на N и не соревноваться с внутренним congestion control.

Предлагаемые состояния: paused → probing → steady → backing_off; отдельно terminal cancelled. Increase разрешён на один lane, когда есть backlog, несколько окон стабильных измерений показывают прирост unique goodput, control latency/loss в бюджете, CPU/RSS/UI headroom сохранён. Decrease/backoff — при росте queue age, probe deadline, отсутствии marginal gain, памяти/CPU/thermal pressure или пользовательском ограничении. Нужны hysteresis, minimum residence, bounded retry с jitter и cooldown; конкретные thresholds определяются экспериментом, не выдаются за универсальные числа.

RTT estimator, pacing и реакция на persistent congestion полезны как принципы. Но application RTT через туннели, очереди relay, storage и ratchet не равен QUIC packet RTT, поэтому копировать RFC packet thresholds в file ACK controller неправильно. [RFC 9002](https://www.rfc-editor.org/rfc/rfc9002.html)

Измерять CPU/router+app+relay, RSS и event-loop delay отдельно. Доступные battery/thermal/metered hints использовать как локальные ограничения, отсутствие API хранить как unknown, не как «ресурсов достаточно». Пользовательские режимы: pause bulk, Wi-Fi/unmetered-only, max bytes/sec, max aggregate queued bytes и max lanes. Сохранять control/ACK budget; pause не означает навсегда удерживать полные buffers. Foreground transfer и deferrable background transfer имеют разные OS contracts.

Android рекомендует учитывать metered/unmetered network state, ограничения фоновой работы и стоимость radio wakeups. Deferrable work можно привязать к network/charging constraints; активная пользовательская передача требует подходящего API жизненного цикла. Нельзя обещать постоянную фоновую работу WebView/browser или нулевой расход батареи. [Read network state](https://developer.android.com/develop/connectivity/network-ops/reading-network-state), [Network power optimization](https://developer.android.com/develop/connectivity/network-ops/network-access-optimization), [Data transfer task options](https://developer.android.com/develop/background-work/background-tasks/data-transfer-options)

Ownership controller: задача принадлежит одной session/profile generation; lock/delete/duress/profile switch отменяет её и дожидается принадлежащих задач. Никакие delayed timers не должны повторно открыть old identity. Lane selection не меняет hops, padding, prekeys, isolation или UPnP ownership. Native global router остаётся отдельным владельцем process lifetime.

## 6. Что уже есть для resume и чего ещё нельзя обещать

Native собственный код уже поддерживает file manifest, encrypted part storage, bitmap, cumulative/selective ACK, duplicate suppression и resumed ACK (`libcore/src/files.rs`, `libcore/src/db/{dm,file_transfer}.rs`, `libcore/src/session/files.rs`). `write_part` проверяет index/size, пишет зашифрованный part и `sync_all` до отметки получения. `apply_file_chunk` проверяет contact/file/message binding, trust и tombstones; повторный chunk не перезаписывается. Полная готовность устанавливается только после SHA-256 проверки всего файла.

Не следует называть всё это одинаково power-loss durable: SQLite в обычном режиме WAL+NORMAL (`db.rs:401/416`), а `commit_received` отдельно использует FULL для атомарного сохранения ratchet/prekey/inbox до relay ACK (`db/inbox.rs:41`). Затем pending inbox позволяет повторить применение после сбоя; deletion inbox следует после обработки. Поэтому оценивать нужно **всю цепочку inbox → file fsync → bitmap transaction → file ACK → inbox_done**, а не только наличие fsync в одном helper. WAL+NORMAL переживает application crash, но обычная транзакция может откатиться при power loss. Это ограничение гарантий, не доказательство потерянного файла в текущей реализации. [SQLite synchronous semantics](https://sqlite.org/pragma.html#pragma_synchronous)

Sender persistence содержит `next/acked/resend`, а не полный durable receiver bitmap; `Flow` и часть in-flight bookkeeping живут в памяти. После restart receiver отправляет resumed ACK, sender reconciles outstanding. До получения актуального ACK возможна повторная передача уже имеющихся частей. Корректное требование: **после аутентифицированного resume snapshot не планировать снова подтверждённые immutable chunks**, а не «ни одного повторного сетевого byte при потере ACK». Transport retransmission и lost ACK делают последнее недоказуемым.

Проверен protocol constraint sparse ACK: раньше native обрезал missing до64,
но оставлял seen_to последней held частью, что давало ложное подтверждение
неназванных дыр. Исправление ограничивает seen_to перед65-м hole, как уже делал
browser; wire/window не меняются. Regression/property tests проверяют, что
каждый ACK-held index реально сохранён, включая sparse resume и sender retry.
Достижимость >64holes в обычном WINDOW16 production потоке этим не доказана;
увеличение окна и новые range protocols всё равно требуют отдельной проверки.

Предлагаемое продолжение: session-bound resume negotiation с file/version identity и bounded bitmap/range digest; дождаться её перед новым bulk после restart; sender durable confirmed-set; paused-state, который не теряет manifest/bitmap; проверка tombstones до replay. Последующее уточнение владельца: совместимость со старыми профилями не нужна. Поэтому новый beta формат можно отделить явно, без migrations/aliases прежних данных; реальные старые файлы на диске не удалять. Versioning и immutable identity нужны для корректности уже начатых beta transfers, не как обещание legacy compatibility.

## 7. Частичный ciphertext и ratchet

AEAD даёт аутентифицированное сообщение целиком либо failure. Произвольный prefix ciphertext без финального tag нельзя считать проверенным plaintext и подтверждать как полученный файл. Для granularity меньше существующего part нужны независимые authenticated records с однозначным AAD, bounded index/length и immutable object identity. Nonce нельзя повторять для разных plaintext под одним key. [RFC 5116](https://www.rfc-editor.org/rfc/rfc5116.html)

Сейчас `AttachmentCipher` использует XChaCha20-Poly1305 и nonce, производный от part index (`libcore/src/crypto.rs:619+`). Это требует неизменности file key+index→plaintext: изменённый файл/версия должны получать новый key/id. Существующий disk format не следует незаметно менять; новый record wire format должен аутентифицировать domain/version, file identity, index, length и manifest digest. Прежний disk AAD и привязки через DB/wire нужно рассматривать отдельно, а не приписывать им будущий AAD design.

Повторный logical chunk сейчас заново оборачивается в ratchet message; его ciphertext bytes могут отличаться. Поэтому нельзя докачать хвост «предыдущего ratchet ciphertext» новым сообщением без отдельного immutable object layer. Ratchet keys потребляются, skipped-key store ограничен; rollback ratchet ради resume или неограниченное сохранение старых keys противоречит его модели. [Signal Double Ratchet specification](https://signal.org/docs/specifications/doubleratchet/)

Собственные ограничения: `MAX_SKIP=1000`; decrypt копирует state и применяет результат только после успешной authentication. Новый controller не должен породить несериализованные concurrent ratchet mutations, бесконечные retries/skip gaps или повторное применение одного payload. Chunk ACK и relay/mailbox ACK — разные события и разные гарантии хранения.

## 8. Сравнение проектов: что можно перенять

| Источник | Подтверждённый механизм | Применение к нашему плану / граница |
| --- | --- | --- |
| [Signal firewall guidance](https://support.signal.org/hc/en-us/articles/360007320291-Firewall-and-Internet-settings) | TCP/UDP/WebSocket и отдельные требования calls/relay | Проверять реальные разрешённые network paths. Это не спецификация анонимного I2P fallback и не доказательство arbitrary-range resume |
| [Briar architecture](https://briarproject.org/how-it-works/) | Internet через Tor; nearby Bluetooth/Wi-Fi и removable media; локальное хранение/sync | Явно различать transport classes и trust/privacy. Nearby contact link не равен I2P multi-hop anonymity |
| [Jami developer update](https://forum.jami.net/t/dev-update-6-major-bug-fixes/2626) | ICE/UPnP/TURN; описан исправленный lifecycle bug UPnP после deactivation | Network helper должен иметь owner/cancel lifecycle; исторический bug не означает current regression |
| [Jami swarm file transfer](https://dl.jami.net/docs/developer/jami-concepts/swarm.html#file-transfer) | File metadata/hash, waiting download queue, запрос к доступным conversation devices, проверка файла | Полезна durable очередь и повторное обнаружение источника; документация не доказывает произвольную побайтовую докачку |
| [SimpleX XFTP specification](https://github.com/simplex-chat/simplexmq/blob/stable/protocol/xftp.md) | Отдельный file protocol с file description, encrypted chunks и проверками digest | Полезно отделение durable immutable objects от chat envelope. Не делать вывод, что XFTP уже решает arbitrary partial-byte resume в нашем ratchet |
| [QUIC RFC 9000](https://www.rfc-editor.org/rfc/rfc9000.html) | Multiplex streams, stream flow control, migration | Разделение streams не равно отдельному congestion path и не означает disk-durable application ACK |

У этих проектов разные доверенные серверы, discovery, metadata leakage и privacy goals. Ни один источник не обосновывает замену нашей 3/3 padded policy прямой связью с контактом ради скорости.

## 9. Последовательность beta work и критерии готовности

1. **Сначала валидное измерение.** Benchmark deadline/checkpoint fix уже сохранён в `f73fd42`: Python 20/20, performance Rust 15/15, harness all-targets check прошли. Новый CI пока не запускался. Последний старый прерванный point имеет неизвестный outcome, не zero goodput.
2. **Инструментировать и проверить main reservation.** Fixed cap A/B/B/A с aggregate queue metrics и теми же privacy settings. Не выбирать «оптимальные 6/8 lanes» по одному неполному sweep.
3. **Доказать resume contract.** Crash после каждого шага storage/ACK; lost/duplicate/out-of-order parts; corrupted/truncated ciphertext/tag; disk full; stale generation; удаление/tombstone; repeated startup; >64 sparse holes; skipped-key limits. Отдельно application crash и power-loss model. UI progress считает уникальные verified chunks, завершение — hash полного файла.
4. **Добавить opt-in bounded controller.** Сначала simulator/fake clock/resource inputs и deterministic hysteresis/cancel tests; затем constrained real network + low-resource device. Preserve aggregate window/caps до отдельного обоснования.
5. **Совместимость существующих browser paths.** TURN UDP/TCP/TLS и WSS failure matrix, реальный Android WebView/Chrome/другие поддерживаемые браузеры; runtime feature detection. Отчёт должен разделять fixture, desktop emulation и physical device.
6. **Только затем новые transport adapters.** WebTransport/QUIC/SSU2 browser datagrams — самостоятельный scoped прототип с архитектурой bridge, authentication, ownership, ресурсными caps и privacy review. ICMP fallback не планировать как уже доступную функцию.

Transfer-only profile не должен автоматически означать другой transport/privacy режим. Общий portable identity/storage формат описан в [дизайне переноса](profile-transfer.md); старые данные и миграции не поддерживаются; этот план требует только scoped transfer capability, отдельного queue ownership и отсутствия фонового chat/identity reuse из чужого профиля. До beta release нужны результаты этих gates, а не только успешная компиляция.

## English

Native routing currently supports NTCP2/TCP and SSU2/UDP. Browser-owned I2P uses NTCP2 over WSS or WebRTC to the byte forwarder; both paths passed baseline CI. This is not browser SSU2, QUIC or ICMP support. TURN and physical-phone validation remain separate gates. Automatic lane changes must depend on verified unique-byte goodput, latency, memory and CPU pressure, with hysteresis and bounded resources. Preserve application 3/3 hops, padding, router selection and identity separation. Durable authenticated chunks can be reused, but incomplete AEAD records or lost acknowledgements can still cause retransmission. See the proposed measurement, fault-injection and implementation sequence above; none of those future controllers is claimed as shipped.
