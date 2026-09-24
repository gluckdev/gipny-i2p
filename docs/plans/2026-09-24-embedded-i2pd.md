# i2pd внутри процесса: никаких локальных портов

## Context

Сейчас приложение, агент и встроенный релей говорят с i2pd по SAMv3 через TCP на
`127.0.0.1` (`libcore/src/router.rs` запускает роутер с `--sam.address=127.0.0.1
--sam.port=…`; на Android i2pd встроен через JNI, но общение то же, через SAM). Плюс
HTTP-прокси роутера для проверки обновлений. Оба порта недоступны снаружи машины, но
доступны **любому процессу на ней**, а на Android — любому приложению с доступом в сеть.

В SAM у i2pd нет аутентификации: `STREAM ACCEPT` / `STREAM CONNECT` находят сессию по
ID (`libi2pd_client/SAM.cpp`, `FindSession(m_ID)`). До 2026-09-24 наши ID были
`префикс-pid-счётчик`, то есть угадывались за секунды: чужая программа могла принимать
соединения на наш релей и открывать соединения от нашего адреса. Письма это не
раскрывает (сквозное шифрование), но доставку можно перехватывать и глушить.

Сделано как временная мера: ID сессий стали секретными (`c862bc3` на `feat/dht-seed`,
128 случайных бит; `libcore/src/net.rs::sam_session_id`, `core/relay/src/proto.rs`).
Остаётся: чужая программа может завести *свою* сессию на нашем роутере (бесплатный
анонимный туннель за наш трафик) и пользоваться HTTP-прокси с аутпрокси.

**Решение владельца (2026-09-24): никаких портов вообще.** Ни SAM, ни HTTP-прокси.
Вариант «SAM через Unix-сокет» (issue #101, шаг 1) отменён в пользу встраивания: сокета
для SAM в i2pd из коробки нет (есть только у SOCKS-апстрима и I2PControl), это патч i2pd
плюс переделка yosemite, а Windows всё равно остаётся на TCP.

## Что уже известно

- **У i2pd есть API для встраивания** — `third_party/i2pd/libi2pd/api.h`:
  - `InitI2P(argc, argv, appName)`, `StartI2P(logStream)`, `StopI2P()`, `TerminateI2P()`;
  - `CreateLocalDestination(const PrivateKeys&, bool isPublic, const Mapping* params)` —
    постоянная личность; вторая перегрузка — транзиентная;
  - `CreateStream(dest, IdentHash remote)`, `AcceptStream(dest, Acceptor)`,
    `RequestLeaseSet(dest, remote)`, `DestroyStream`, `DestroyLocalDestination`.
- **Образец существует:** крейты [`i2pd-sys`](https://docs.rs/i2pd-sys/latest/i2pd_sys/)
  (C-прослойка ~25 функций над libi2pd + bindgen) и
  [`tachyon-i2p`](https://docs.rs/tachyon-i2p/latest/tachyon_i2p/) (async-обёртка:
  `Destination::accept()/connect()`, `I2pStream: AsyncRead + AsyncWrite`, без SAM).
  Брать зависимостью не будем: 0.0.x (август 2026), только Linux x86_64, криптография
  через aws-lc вместо нашего OpenSSL, своя сборка i2pd мимо нашего пина. Смотреть как
  образец устройства прослойки и моста колбэков в tokio.
- **Android уже собирает libi2pd библиотекой** (`android-router/jni/Android.mk`, JNI
  `gipny_i2pd_jni.cpp`, `nativeStartSam` игнорирует `samListen`, адрес/порт SAM идут из
  i2pd.conf). Десктоп компилирует i2pd из исходников в CI (release.yml, job `router`;
  Windows — mingw-w64 boost; macOS — Homebrew boost/openssl статически) и запускает
  бинарь дочерним процессом.
- Одна копия роутера на процесс (глобальное состояние libi2pd). У нас и так один роутер
  на приложение; e2e-харнесс гоняет двух ботов в одном процессе — им достаточно двух
  destination на одном роутере.
- Минусы, принятые осознанно: C++ через FFI; падение роутера роняет приложение (сейчас
  роутер — отдельный процесс, `router.rs` умеет его перезапускать); мост колбэков
  Boost.Asio → tokio.

## Что ещё надо выяснить до кода

Сбор фактов был начат и остановлен владельцем; повторить:

1. Сборка i2pd на каждой платформе в CI (release.yml: `router`, `android-router`,
   `desktop`; i2pd-build.yml; e2e-i2pd.yml; build.yml): команда, флаги make, откуда
   boost/openssl/zlib и статически ли, что получается (бинарь или `libi2pd.a`).
2. Все места, где Rust использует yosemite/SAM: `libcore/src/net.rs` (`TorNode`: сессия,
   `connect_detached(_with_options)`, accept-задача, пересоздание сессии, `hops`),
   `libcore/src/relay_server.rs` (`EphemeralRelay`: `generate_destination`, публикуемая
   сессия, `inbound_len/outbound_len`, пересборка на смене длины туннелей),
   `libcore/src/router.rs` (запуск дочернего роутера, `Previous::Serving`, проверка
   живости, перезапуск), `core/src/*.rs`, `agent/src/*.rs`, `tests/e2e-harness`
   (`GIPNY_SAM_PORT`, общий роутер), `core/relay` (отдельный серверный релей).
3. `libi2pd/Streaming.h`: `AsyncReceive`, `AsyncSend`/`Send`, `Close`, сигнатуры
   колбэков, на каком `io_context`/потоке они вызываются. `Destination.h`:
   `CreateStream` с колбэком, `AcceptStreams`/`SetAcceptor`, `IsReady`, `GetIdentHash`,
   `PrivateKeys::FromBase64`, параметры `inbound.length`/`outbound.length` через Mapping.
   `api.cpp`: что делает `InitI2P`/`StartI2P`, нужен ли конфиг-файл, как задаётся
   datadir, не поднимает ли он клиентские туннели и HTTP-прокси; хватает ли `libi2pd`
   без `libi2pd_client` для `api.h`.
4. Проверка обновлений: где используется `http_proxy_port` (update checker) — перевести
   на поток к аутпрокси через ту же встроенную destination.

## План

Отдельная ветка (не `feat/dht-seed`), один PR на шаг, каждый проверяется e2e.

1. **Крейт `i2p-embed`** (корень workspace):
   - `shim/shim.cpp` + `shim.h` — C-интерфейс над `api.h`: init/start/stop с datadir и
     параметрами (bandwidth, share, transit tunnels, yggdrasil — то, что сейчас идёт
     аргументами в `router.rs`), создание destination из base64-ключей и транзиентной,
     длины туннелей, адрес в base64/b32, accept с колбэком, connect с колбэком,
     receive/send/close потока с колбэками, счётчики для «измеренного канала».
   - `build.rs`: компиляция libi2pd из `third_party/i2pd` (`cc`), линковка boost,
     OpenSSL, zlib — по тем же источникам, что в CI сейчас. На Android линковать уже
     собираемый `libi2pd.a`.
   - Безопасная обёртка: `Router` (один на процесс), `Destination`, `I2pStream:
     AsyncRead + AsyncWrite` (колбэки → каналы/`Waker`).
   - Локально разрешено только компилировать ради проверки (см. память
     `builds-only-on-github`); выпуск — только CI.
2. **Трейт транспорта в libcore** поверх того, что сейчас даёт yosemite (connect,
   accept, пересоздание, длина туннелей), с реализацией на `i2p-embed`. SAM остаётся
   второй реализацией на время перехода — для `GIPNY_SAM_PORT` в e2e и для
   отдельного серверного релея.
3. **Перевести `TorNode`, `EphemeralRelay`, транспорт сети релеев** на трейт;
   `router.rs` — запуск встроенного роутера вместо дочернего процесса.
4. **Проверка обновлений без HTTP-прокси:** поток к аутпрокси через встроенную
   destination, HTTP поверх. Закрывает #102.
5. **Android:** `GipnyService` запускает встроенный роутер тем же путём, что десктоп;
   JNI `nativeStartSam` уходит.
6. **Агент:** флаг `--sam` остаётся только как escape hatch для внешнего роутера или
   уходит — решить с владельцем.
7. **e2e:** харнесс на встроенном роутере (один на процесс, destination на бота);
   отдельный серверный релей (`core/relay`) — отдельный вопрос: он живёт на сервере
   рядом с системным i2pd, ему SAM на loopback сервера допустим? Спросить владельца,
   раз «никаких портов».
8. Удалить yosemite и `router.rs`-запуск дочернего процесса, когда всё выше прошло e2e.

## Проверка

- `cargo check`/`cargo test` локально (с разрешённой локальной компиляцией i2pd).
- CI: build.yml на всех платформах (Linux, Windows, macOS, Android APK + эмулятор).
- e2e-i2pd: все пять сценариев, включая `e2e-dht` и агента.
- Ручная проверка на устройстве владельца: ни одного слушающего сокета у процесса
  (`ss -ltnp` / `lsof -i` на десктопе, `/proc/net/tcp` на Android).
