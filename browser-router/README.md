# Эксперимент: I2P runtime в браузере

Этот отдельный Rust workspace реализует `emissary_core::runtime::Runtime` для
`wasm32-unknown-unknown`. Upstream закреплён на публичном commit
`5e696a90276c316ebc5ec3a1d0815b02f17516ef`; его исходники не изменяются.
Эксперимент следует [плану](../docs/plans/2026-11-01-browser-router-wasm.md).

В runtime есть браузерные часы (`web-time`), криптографический RNG через
`getrandom/wasm_js`, gzip на Rust, задачи `spawn_local`, `JoinSet` и таймеры.
JavaScript-объекты остаются в локальных задачах; `TcpStream` и таймеры отдают
роутеру только Rust-каналы, удовлетворяющие исходным требованиям `Send`/`Sync`.
Небезопасных реализаций `Send`/`Sync` нет. Метрики пока пустые.

`BrowserTcpStream` передаёт байты двоичными WebSocket-сообщениями. Вызов
`configure_forwarder("wss://entry.example/stream")` задаёт адрес посредника;
`connect(IP:port)` добавляет один query-параметр `target=IP:port`. Для настоящего
форвардера используется `configure_forwarder_authenticated(endpoint, capability)`:
capability передаётся в WebSocket subprotocol `gipny.ntcp2.v1.<token>`, не в URL.
Ответных
служебных текстовых сообщений нет. Посредник должен завершить подключение TCP
до открытия WebSocket, сохранять порядок байтов и размер сообщений до 64 КиБ.
`flush` ждёт опустошения браузерного `bufferedAmount`; это не подтверждение
доставки TCP/I2P. Очереди ограничены восемью сообщениями плюс служебный слот
канала; переполнение, текстовые и слишком большие сообщения закрывают поток
с ошибкой. Короткие чтения и границы WS-сообщений прозрачны для вызывающего кода.
`wss` обязателен, кроме `ws` на loopback.

Браузер не может слушать TCP или привязывать UDP, поэтому соответствующие
операции сразу возвращают `None`. Настоящий роутер нужно запускать как
клиентский, без SSU2 и локальных SAM/I2CP-серверов.

Проверки выполняются в GitHub Actions:
[Browser router WASM](../.github/workflows/browser-router.yml). Workflow
собирает upstream core и release WASM с `--locked`, затем запускает девять
тестов WASM в headless Chrome: часы/RNG/задачи/таймеры, gzip и ограничение
распаковки, недоступные сетевые операции, URL, короткие чтения, пустые кадры,
EOF, ошибочные кадры/переполнение/обрыв, отказ подключения и backpressure
при большой записи. Python-сервер
из `tests/echo_server.py` — fixture с заранее заданными ответами. Дополнительно
workflow исполняет пять Python-тестов границы настоящего форвардера и отдельную
джобу NTCP2 (ниже).
Локальных сборок и запусков тестов для этого эксперимента нет.

Подтверждено на `d1eccd7`: [GitHub CI 41269666409](https://github.com/gluckdev/gipny-i2p/actions/runs/41269666409)
собрал upstream core и release WASM, **9/9 Chrome tests PASS**. Повторный
push-run `41269662893` также зелёный. Первый прогон поймал гонку чистого
закрытия с `flush`; исправление проверяет, что при закрытии нет неотправленных
команд и браузерный буфер пуст.

Артефакт CI содержит raw WASM, размер, SHA256, commit исходников и лицензию
emissary. На `d1eccd7` ранний `runtime_probe()` занимал **649 825 байт** и
удерживал только часы/RNG/gzip. На `b912f01` release-модуль с экспортом
`ntcp2_probe()` занимает **1 451 904 байта**; это всё ещё не полный I2P-роутер.

## NTCP2-probe и форвардер

`ntcp2::probe` проверяет подпись и свежесть RouterInfo (36 часов, netId=2),
выбирает опубликованный классический IPv4 NTCP2-адрес и выполняет
`SessionRequest` → `SessionCreated` → `SessionConfirmed` в браузере. Критерий
успеха — **аутентифицированный data-phase ответ** с допустимыми блоками,
а не открытие TCP/WebSocket. Termination от пэра считается отказом. Затем
probe посылает нормальный NTCP2 Termination и закрывает соединение.
Noise/AES/ChaChaPoly/SipHash берутся из публичного API emissary; framing
ограничен этим экспериментом и классическим X25519. Источники:
[NTCP2 specification](https://i2p.net/en/docs/specs/ntcp2/) и
[закреплённый initiator.rs](https://github.com/eepnet/emissary/blob/5e696a90276c316ebc5ec3a1d0815b02f17516ef/emissary-core/src/transport/ntcp2/session/initiator.rs).
JS API — `ntcp2_probe(router_info_bytes, endpoint, capability)`.

`forwarder/server.py` передаёт зашифрованные байты WS ↔ TCP. Он проверяет
точные Host/Origin, единственный capability subprotocol и единственный
`target`, а TCP открывает до upgrade. Цели — только IPv4-литералы из
allowlist `prepare-netdb`, который проверяет подписи RouterInfo. DNS и
произвольных URL-целей нет. Private/reserved адреса запрещены; отдельный
`--allow-fixture-loopback` разрешает только локальный CI fixture.
Allowlist истекает через 10 минут или раньше вместе со свежестью RI;
соединение ограничено 60 секундами/8 МиБ, всего 8 соединений, очередь WS — 8.
TCP `drain`, WS send и чтения имеют таймауты. Для non-loopback listener
обязателен TLS; перед публичной эксплуатацией потребуется отдельная модель
выдачи/ротации capability и обновления netDb.

Настройки форвардера: `--listen IP:port`, один или несколько `--host` и
`--origin`, `--capability-file`, `--candidates` (TSV от `prepare-netdb`),
при TLS — `--tls-cert`/`--tls-key`. Capability — случайные 32 байта в
unpadded base64url; его нет в retained artifacts. Python-зависимость закреплена
в workflow: `websockets==15.0.1`.

Джоба `NTCP2 browser to native and public I2P` собирает fixture `native-peer`
из собственного `i2p-embed` и pinned C++ i2pd, выполняет verified reseed и
проверяет полученный netDb. Тестовый `native/fixture.cpp` разрешает loopback
перед запуском транспортных потоков; он собирается только с feature
`native-peer`. Затем три WASM-теста в Chrome проверяют handshake
с native fixture, handshake хотя бы с одним из 12 публичных кандидатов и
отказ на повреждённой подписи/неверном capability. Публичные пэры проверяются
последовательно, по 20 секунд на попытку. Журналы и публичные RouterInfo
сохраняются в `browser-ntcp2-evidence`; router.keys и capability не сохраняются.
Подтверждено на `b912f01`: [GitHub CI 41272794574](https://github.com/gluckdev/gipny-i2p/actions/runs/41272794574)
прошёл целиком — **9/9 runtime, 5/5 forwarder, 3/3 NTCP2 tests**. Проверены
13 подписанных RouterInfo (fixture + 12 публичных кандидатов). Native i2pd
ответил за 191 мс (687 аутентифицированных байт, 3 блока); публичный пэр —
с первой попытки за 491 мс (1224 байта, 3 блока). Это время одной попытки
handshake после загрузки WASM и проверки RI, не время bootstrap приложения.
Журнал и RouterInfo — артефакт `browser-ntcp2-evidence` (`13069873728`),
WASM — `browser-router-wasm` (`13070052907`). Код протокола исполнялся в Chrome;
Python только пересылал байты. Натуральный тестовый пэр собран из i2pd
`6b214f74d2c1b9d892cda29119f0942097cbfd85`.

Полный `Router<BrowserRuntime>` пока не запускается: upstream требует настоящий
TCP listener и использует его наличие как признак доступности транспорта для
исходящих соединений. Это отдельная доработка client-only транспорта;
probe проверяет протокол, не подменяя listener фиктивным сокетом. Следующие
этапы — client-only Router, exploratory-туннель, bootstrap/память на телефоне,
OPFS/ключи/passkey и PWA lifecycle.
