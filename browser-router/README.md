# Эксперимент: I2P runtime в браузере

Этот отдельный Rust workspace реализует `emissary_core::runtime::Runtime` для
`wasm32-unknown-unknown`. Upstream закреплён на публичном commit
`5e696a90276c316ebc5ec3a1d0815b02f17516ef` в `emissary-source.json`;
`patches/client-only-ntcp2.patch` добавляет исходящий режим без listener.
`scripts/prepare_emissary.py` получает эти исходники, проверяет точный SHA и
соответствие локального diff патчу, сохраняет `emissary-provenance.json`.
Исходники в `upstream/` генерируются этим скриптом перед Cargo-командами;
неожиданные локальные изменения скрипт отклоняет. Сборки/тесты — только в CI.
Эксперимент следует [плану](../docs/plans/2026-11-01-browser-router-wasm.md).

В runtime есть браузерные часы (`web-time`), криптографический RNG через
`getrandom/wasm_js`, gzip на Rust, задачи `spawn_local`, `JoinSet` и таймеры.
JavaScript-объекты остаются в локальных задачах; `TcpStream` и таймеры отдают
роутеру только Rust-каналы, удовлетворяющие исходным требованиям `Send`/`Sync`.
Небезопасных реализаций `Send`/`Sync` нет. Метрики хранят значения по статическим
именам без peer labels и сервера экспорта. Пустой JoinSet остаётся открытым
и пробуждает владельца после добавления задачи, как у штатных runtime emissary.

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
операции сразу возвращают `None`. Роутер запускается как
клиентский, без SSU2 и локальных SAM/I2CP-серверов.

Проверки выполняются в GitHub Actions:
[Browser router WASM](../.github/workflows/browser-router.yml). Workflow
подготавливает закреплённый core с проверяемым патчем, собирает core и release
WASM с `--locked`, затем запускает 3 unit-теста, 10 runtime-тестов и
5 тестов загрузчика снапшота WASM
в headless Chrome: часы/RNG/задачи/таймеры, gzip и ограничение
распаковки, недоступные сетевые операции, URL, короткие чтения, пустые кадры,
EOF, ошибочные кадры/переполнение/обрыв, отказ подключения и backpressure
при большой записи. Python-сервер
из `tests/echo_server.py` — fixture с заранее заданными ответами. Дополнительно
workflow исполняет 6 Python-тестов границы настоящего форвардера и отдельную
джобу NTCP2 и полного роутера (ниже).
Локальных сборок и запусков тестов для этого эксперимента нет.

Подтверждено на `d1eccd7`: [GitHub CI 41269666409](https://github.com/gluckdev/gipny-i2p/actions/runs/41269666409)
собрал upstream core и release WASM, **9/9 Chrome tests PASS**. Повторный
push-run `41269662893` также зелёный. Первый прогон поймал гонку чистого
закрытия с `flush`; исправление проверяет, что при закрытии нет неотправленных
команд и браузерный буфер пуст.

Артефакт CI содержит raw WASM, размер, SHA256, commit исходников и лицензию
emissary. На `d1eccd7` ранний `runtime_probe()` занимал **649 825 байт** и
удерживал только часы/RNG/gzip. На `b912f01` release-модуль с экспортом
`ntcp2_probe()` занимал **1 451 904 байта**. На `d18630c` модуль с полным
клиентским роутером и экспортом `exploratory_probe()` — **3 403 781 байт**.

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

Джоба `Browser NTCP2 and exploratory tunnels` собирает fixture `native-peer`
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

## Полный клиентский роутер и exploratory

`router::run_exploratory` запускает настоящий `Router<BrowserRuntime>` с
дефолтным exploratory pool: 3 входящих и 3 исходящих туннеля, по 2 хопа,
без transit, SAM/I2CP, SSU2 или insecure hop selection. Каждый seed RI
повторно проверяется в WASM. JS API — `exploratory_probe(arrayOfUint8Arrays,
endpoint, capability)`; текущий эксперимент ограничен 300 секундами.

Патч вводит `Runtime::TCP_LISTENER_SUPPORTED` (по умолчанию true). В браузере
он false: NTCP2 открывает исходящие соединения, публикует только ключ без
входящего адреса, а `ProtocolAddressInfo` не объявляет порт. Запрос публикации
входящего адреса возвращает ошибку. Трейт больше не использует наличие
listener для выбора исходящего IP-семейства. Патч содержит также явный путь
workspace для вложенной сборки; криптографические алгоритмы не меняются.

Критерий live CI: минимум один входящий и один исходящий туннель, успешный
реальный tunnel roundtrip, 30 секунд непрерывной доступности и ещё один
успешный roundtrip после этого интервала. Сохраняются значения метрик, время
и размер WASM memory после проверки (это не измеренный пик). Весь набор дочерних задач
отменяется при завершении/таймауте. Одновременно поддерживается один такой
запуск на browser thread.

Для этого CI использует до 512 проверенных seed RI и профиль форвардера
24 соединения / 600 секунд / 32 МиБ, idle timeout 120 секунд. Общие пределы
настройки — 32 соединения / 600 секунд / 64 МиБ; дефолты handshake-probe
остались 8 / 60 / 8 МиБ. Raw stream и HTTP-граница проверяются отдельно.

Подтверждено на `d18630c`:
[GitHub CI 41277188521](https://github.com/gluckdev/gipny-i2p/actions/runs/41277188521)
прошёл целиком — **3 unit + 10 runtime + 6 forwarder + 3 NTCP2 + 1 exploratory
tests PASS**. Chrome запустил настоящий роутер с 61 публичным seed RI из
verified reseed/netDb. Результат `EXPLORATORY_PASS`:

| Измерение | Результат |
|---|---|
| Соединения с пэрами на момент успеха | 10 |
| Входящие / исходящие exploratory-туннели | 3 / 3, по 2 хопа |
| Успешные tunnel roundtrips | 7 |
| Непрерывная доступность входящего и исходящего пути | 40 секунд; новый roundtrip после границы 30 секунд |
| Первое выполнение критерия готовности | 81 298 мс |
| Завершение проверки | 121 313 мс |
| Линейная WASM memory на момент успеха | 5 111 808 байт (4,875 МиБ) |
| Raw release WASM | 3 403 781 байт |

Отсчёт времени начинается после первоначальной проверки seed RI в WASM,
перед `RouterBuilder::build`; загрузка страницы/модуля и получение netDb
в него не входят. Память — размер линейной памяти в конце прогона, без JS
heap и без измерения пика. Это один прогон на Chrome раннера GitHub, не
мобильный замер и не подтверждение целевого bootstrap за секунды.

Артефакты run: `browser-exploratory-evidence` (`13071837628`),
`browser-ntcp2-evidence` (`13071847528`) и `browser-router-wasm`
(`13070529979`, ZIP 987 085 байт вместе с метаданными и лицензией).
В WASM-артефакте есть provenance исходников: emissary
`5e696a90276c316ebc5ec3a1d0815b02f17516ef`, SHA256 применённого патча и
точного diff — `4cb5f8fe8971d001905b7b6d029dd0b1a21c5e1b7b0cb7ab69c7835d0fe49b88`.
Native handshake повторно прошёл за 237 мс, публичный — за 575 мс с первой
попытки; оба получили аутентифицированный data-phase ответ.

Продолжение с загрузкой netDb и измерением полного bootstrap — ниже.
Дальнейшие этапы — запуск на телефоне, OPFS/ключи/passkey и PWA lifecycle.

## Загрузка снапшота и полный bootstrap

`bootstrap_probe(snapshot_url, sha256, endpoint, capability)` получает
`tar.gz` netDb по HTTPS (loopback HTTP разрешён для CI), проверяет SHA256,
распаковывает архив в памяти и проверяет каждый RI перед запуском полного
роутера. Digest выбирает конкретный снапшот; подлинность самих RI
подтверждается их I2P-подписями, независимо от digest/сервера снапшота.
Лимиты: 4 МиБ скачивания, 16 МиБ распаковки, 4096 tar entries,
2048 пригодных публичных IPv4 classic NTCP2 RI по 16 КиБ. Ссылки,
родительские пути и неожиданные файлы отклоняются, дубликаты удаляются.
Fetch не посылает credentials/referrer, не принимает redirect, прерывается
через 45 секунд или при превышении лимита. Записи проверяются партиями
с возвратом управления браузеру. На диск/OPFS архив не извлекается.

`tests/run_bootstrap_ci.py` получает свежий netDb через отдельный pinned i2pd
с verified reseed, упаковывает проверенные публичные RI и отдаёт архив по
HTTP. Страница `tests/bootstrap-page/` загружает JS/WASM и архив с пустым
профилем/без кеша, затем требует тот же exploratory gate с roundtrip после
30 секунд удержания. Для этого сценария seed RI передаются только скачанным
архивом, без встраивания в тестовый WASM.
Два профиля — обычный Chrome и desktop Chrome с CPU ×4, viewport 390×844,
HTTP 200 КиБ/с download / 100 КиБ/с upload и latency 80 мс. Это эмуляция,
не аппаратный замер Android/iPhone.

JSON receipt разделяет загрузку/инициализацию модуля, скачивание, digest,
распаковку, проверку RI и построение туннелей. `navigation_to_ready_ms`
начинается с навигации страницы и включает подготовку до первого успешного
критерия туннелей. Максимальная линейная WASM memory — ёмкость памяти
модуля; JS heap измеряется CDP с периодом около секунды и хранится как
максимум наблюдавшихся значений. Это не RSS процесса или всего устройства.
Публичные snapshot/manifest/receipt и модуль сохраняются отдельными
артефактами; capability, private keys и Chrome profile туда не входят.

Предыдущий повтор на документационном `953014f`, run `41278342212`,
прошёл runtime/NTCP2, но завершил exploratory по таймауту (74 seed RI,
3 построенных туннеля и ни одного успешного roundtrip). Положительный run
`41277188521` сохранён выше; устойчивость live bootstrap ещё требует проверки.
CodeQL на `953014f` прошёл (`41278342141`).

На `6b37dc2` [CI 41281997598](https://github.com/gluckdev/gipny-i2p/actions/runs/41281997598)
прошёл целиком: **3 unit + 10 runtime + 5 snapshot + 6 forwarder + 3 NTCP2
tests**, затем **оба холодных live-сценария**. Chrome — `156.0.8078.47`.
CodeQL на том же SHA прошёл (`41281997545`).
Снапшот содержит 303 проверенных RI: 168 043 байта gzip, 542 208 байт tar;
SHA256 `c3e13e03fb84b8cc53934c356814238e867b4a708762cfcde1b9e91b1fad08d9`.

| Измерение | Обычный Chrome | CPU ×4, ограниченная загрузка |
|---|---:|---:|
| Навигация → модуль готов | 67,8 мс | 10 369,6 мс |
| Скачивание снапшота | 4 мс | 906 мс |
| SHA256 / распаковка | 2 / 5 мс | 10 / 16 мс |
| Tar + проверка 303 RI, включая уступку event loop | 87 мс | 255 мс |
| Навигация → готовые входящий и исходящий пути с roundtrip | 197 328 мс | 107 202 мс |
| Навигация → конец проверки | 229 338 мс | 141 239 мс |
| Соединения / inbound / outbound на момент успеха | 12 / 2 / 3 | 17 / 2 / 3 |
| Успешные roundtrip / удержание | 5 / 32 с | 5 / 34 с |
| Максимальная ёмкость линейной WASM memory | 5 898 240 байт | 6 619 136 байт |
| Максимум JS heap из секундных CDP samples | 2 116 520 байт | 2 338 692 байт |
| Максимальный интервал 100-мс heartbeat | 155,9 мс | 544,8 мс |

Главная задержка в обоих прогонах — построение путей в живой I2P. Два запуска
не дают основания сравнивать CPU по времени сетевой готовности или заявлять
bootstrap за секунды. Физический телефон и мобильный браузер этим не проверены.
Подгружаемый после `wasm-bindgen` модуль — **2 012 452 байта WASM + 29 579 байт
JS**; raw release WASM до обработки — **3 603 680 байт**. HTTP в этом CI
отдавал файлы без gzip/brotli; ZIP артефакта не равен размеру HTTP-загрузки.

Артефакты: `browser-bootstrap-evidence` (`13074456534`, public snapshot,
manifest и JSON receipts), `browser-bootstrap-module` (`13074327101`,
ZIP 776 196 байт с модулем/страницей/лицензией/provenance),
`browser-router-wasm` (`13073487160`) и `browser-ntcp2-evidence` (`13074067958`).
Исходный emissary и SHA256 патча не менялись относительно exploratory
checkpoint выше. Все сборки и исполнение тестов — только GitHub Actions.
