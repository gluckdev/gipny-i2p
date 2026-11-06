# Rust-мост WSS / WebRTC / Rust WSS–WebRTC bridge

## Русский

`gipny-forwarder` — отдельный Rust workspace. Он передаёт NTCP2-шифротекст
между браузером и разрешёнными I2P IPv4-пирами. Он не хранит профиль, не владеет
ключами мессенджера и не завершает NTCP2. Python в рабочем мосте удалён;
Python-скрипты остаются только инструментами CI.

Сборка/тесты выполняются в GitHub Actions (`Browser router WASM`, job `forwarder`).
Используется закреплённый `webrtc = 0.21.1`, lockfile и тот же проверенный emissary,
что у браузерного роутера. Бинарник — artifact `browser-forwarder-linux`.
Release собирается с `panic = "unwind"`: паника в разборе SDP/STUN/DTLS/SCTP
завершает только задачу своего сеанса, остальные сеансы продолжают работу.
Запуск уже собранного бинарника на сервере:

```sh
gipny-forwarder \
  --listen 127.0.0.1:18767 \
  --host bridge.example.org \
  --origin https://chat.example.org \
  --capability-file /run/gipny/forwarder-capability \
  --candidates /srv/gipny/candidates.tsv \
  --max-connections 16 --session-seconds 600 --idle-seconds 120 \
  --max-bytes 67108864 \
  --rtc-bind 10.0.0.5:0 --rtc-public-ip 203.0.114.7
```

HTTP listener разрешён только на loopback. TLS завершает локальный reverse proxy;
он сохраняет точные Host/Origin и WebSocket subprotocol, ограничивает соединения,
частоту запросов и размеры заголовков. Отключите логирование capability, SDP,
транспортных байтов и заголовков. Прокси обслуживает `/stream` и `/rtc`.

`/stream?target=IP:port` передаёт binary WSS ↔ TCP. `/rtc?target=IP:port`
принимает один JSON SDP offer и возвращает answer; после этого NTCP2 идёт через
DataChannel `gipny-ntcp2-v1`. Сигнальный WSS остаётся открытым для контроля жизни
сеанса: его закрытие завершает сеанс. Канал обязателен ordered/reliable, без
retransmission/time limits. Один запрос — один TCP, один PeerConnection и один
DataChannel.

Таймауты прокси. После answer по сигнальному WSS данные не идут, поэтому мост
шлёт WebSocket Ping каждые 20 с на `/rtc` и на `/stream`. Ping/Pong не считаются
активностью: `--idle-seconds` учитывает только NTCP2-байты. Значения по умолчанию
nginx (`proxy_read_timeout`/`proxy_send_timeout` 60 с) и idle-лимит Cloudflare
(около 100 с) совместимы. Если прокси режет соединение раньше 25 с или отвечает
на Ping сам, не передавая его мосту, сеансы WebRTC будут обрываться.

### Назначения

Capability передаётся в subprotocol `gipny.ntcp2.v1.<43-character token>`, не URL.
Токен, известный клиенту, не защищает от злоупотребления этим клиентом. Поэтому
цель ограничена политикой, а не только подписью: RouterInfo может подписать
и опубликовать кто угодно, указав в NTCP2 чужой `host:port`.

- Цель — канонический `IPv4:port`, публичный адрес из allowlist с непросроченной
  lease. Проверяются подпись, сеть, адрес, дата и identity каждой RI из TSV.
- Порт не ниже 1024 и не из `BLOCKED_NTCP2_PORTS` (`browser-router/src/ntcp2.rs`):
  порты, которые отвергает Java I2P `TransportUtil.isValidPort` (SSDP, NFS, SIP,
  IRC, X11, службы I2P 7650–7668 и i2pd 7070, Tor, принтеры), плюс SOCKS/HTTP-прокси,
  MySQL/PostgreSQL/MSSQL/Oracle, Redis, memcached, MongoDB, CouchDB, Elasticsearch,
  Docker/etcd/Kubernetes, RDP/VNC/WinRM, MQTT/AMQP, Bitcoin и Tor control.
  Политика применяется при разборе цели, загрузке TSV и проверке RouterInfo;
  её же используют `prepare-netdb`, bootstrap и WASM-клиент. Строка TSV с
  запрещённым портом пропускается и не срывает загрузку остальных.
- На один IP:port — до 4 подключений подряд, затем одно в 15 с (HTTP 429), сверх
  глобального burst 32 с пополнением 1/с. Это ограничивает нагрузку на одну цель,
  но мост по-прежнему открыто подключается к I2P-узлам.

TCP открывается только после проверки Host/Origin/token/target, лимитов и
резервирования permit. Для `/stream` — до upgrade. Для `/rtc` авторизация и
permit тоже до upgrade, а TCP — только после открытия DataChannel: медленные
ICE/TURN/DTLS не расходуют таймаут NTCP2-рукопожатия пира (около 10 с).

### WebRTC

`/rtc` работает только в конфигурации, которая не раскрывает внутренние адреса
(иначе 404):

- `--rtc-public-ip <IPv4>` — публичный адрес при 1:1 NAT / облачной VM; лучше
  вместе с `--rtc-bind <частный IP интерфейса>:0`, чтобы открыть один сокет;
- или `--rtc-bind <публичный IPv4>:0` на хосте с публичным интерфейсом.

Wildcard или частный `--rtc-bind` без `--rtc-public-ip` — ошибка запуска.
webrtc 0.21.1 сохраняет, но не применяет `SettingEngine::with_nat_1to1_ips`
(кандидаты строятся из адресов сокетов). Поэтому мост сам переписывает answer:
остаются только host UDP-кандидаты компонента 1, их адрес заменяется на
`--rtc-public-ip` (порт сохраняется), прочие удаляются. Адреса VPC, docker0 и
других интерфейсов в answer не попадают. mDNS отключён: нет multicast-сокета
на сеанс и разрешения `.local`. `--allow-fixture-ice-private` (только CI)
разрешает wildcard bind и частные кандидаты.

UDP-порты. В webrtc 0.21.1 нет UDP mux; каждый PeerConnection открывает свой
сокет без SO_REUSEADDR. Используйте порт 0: каждый сеанс получает эфемерный порт
ядра (`sysctl net.ipv4.ip_local_port_range`, обычно 32768–60999). Firewall или
security group должен пропускать входящий UDP на этот диапазон; чтобы сузить его,
запускайте мост на отдельной VM или в своём network namespace/контейнере с
меньшим `ip_local_port_range` (не меньше `--max-connections`). Фиксированный порт
допускает один RTC-сеанс и поэтому разрешён только с `--max-connections 1`.

ICE браузера. `relay` (по умолчанию) требует TURN. `all` требует хотя бы один
публичный кандидат браузера: за NAT это STUN в `rtc_ice_servers` либо публичный
адрес. LAN/mDNS/IPv6-кандидаты отбрасываются, оффер без публичного кандидата
отвергается; peer-reflexive обнаружение не используется как обход.

TURN. Учётные данные в публичном `browser-config.json` фактически долговечны и
доступны всем, поэтому TURN-сервер должен ретранслировать только к мосту.
Для coturn: `allowed-peer-ip=<IP моста>` вместе с запретом всего остального
(`denied-peer-ip=0.0.0.0-255.255.255.255`,
`denied-peer-ip=::-ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff`; адрес, указанный
и в allowed, и в denied, coturn считает разрешённым), как минимум `denied-peer-ip`
для 0/8, 10/8, 100.64/10, 127/8, 169.254/16, 172.16/12, 192.168/16 и IPv6
loopback/ULA/link-local, а также `no-multicast-peers` и `no-tcp-relay`. coturn
задаёт диапазоны как `начало-конец` (`denied-peer-ip=10.0.0.0-10.255.255.255`),
а не CIDR. Если конфигурация обновляется часто,
используйте TURN REST (`use-auth-secret` + `static-auth-secret`) с коротким сроком.
Без этого TURN — открытый relay от имени вашего сервера.

### Квоты и ёмкость

До 32 сеансов на процесс, глобальный burst 32 с пополнением 1/с, WSS frame
64 КиБ, DataChannel frame 16 КиБ, SCTP send/receive buffers по 128 КиБ, сеанс до
600 с, idle до 300 с, суммарно до 64 МиБ в обе стороны. По умолчанию 16 сеансов,
600/120 с и 64 МиБ. Это ограниченный начальный профиль, не результат бенчмарка.

Стоимость сеанса (оценка, не замер): один TCP к I2P-пиру; для WebRTC также
PeerConnection с DTLS/SCTP, UDP-сокетом и задачей драйвера. Буферы — до 128 КиБ
SCTP в каждую сторону и до 2 × 64 КиБ WSS, то есть порядка сотен КиБ на сеанс.
Каждый NTCP2-сеанс браузерного роутера — отдельный сеанс моста и отдельный permit;
один пользователь держит их несколько одновременно (столько, сколько нужно его
туннелям). Поэтому `--max-connections` 16–32 обслуживает лишь несколько
одновременных пользователей; для большего числа нужны дополнительные экземпляры
или хосты. По истечении session/idle/байтового лимита браузер переподключается.

### Allowlist

Allowlist загружается на старте и повторно проверяется каждые пять минут.
TSV содержит не больше 2049 строк: до 2048 публичных (`prepare-netdb`) и одна
fixture-строка. Каждая lease истекает не позднее 10 минут после проверки или
36 часов после публикации RI. При ошибке reload прежние сроки не продлеваются.
Оператор обновляет TSV атомарной заменой; форвардер сам не выполняет reseed.
Снимок браузера, SHA256 и allowlist должны быть согласованы. Старый TSV не даёт
постоянно работающий сервис. Не включайте `--allow-fixture-*` на публичном
сервисе. Публичное развёртывание этой правкой не выполнялось; NAT, запрет UDP и
TURN/TLS требуют отдельной проверки на CI/устройствах.

## English

`gipny-forwarder` is a standalone Rust workspace forwarding NTCP2 ciphertext
between browsers and allowlisted IPv4 I2P peers. It has no messenger/profile keys
and does not terminate NTCP2. Python is tooling only. Build and test in GitHub
Actions (`forwarder` job); the artifact is `browser-forwarder-linux`. Dependencies
include pinned webrtc 0.21.1 and the browser router's verified emissary source.
Release builds use `panic = "unwind"`, so a panic in SDP/STUN/DTLS/SCTP parsing
ends only its own session task. Run the CI-built binary using the command above.

The HTTP listener is loopback-only, behind a TLS reverse proxy. Preserve exact
Host/Origin and WebSocket subprotocol headers; bound request rates, connections
and headers, and suppress capability/SDP/payload/header logging. Proxy both paths.
`/stream?target=IP:port` carries binary WSS/TCP; `/rtc?target=IP:port` exchanges
one offer/answer and carries bytes over the reliable, ordered `gipny-ntcp2-v1`
DataChannel. Signaling remains open as the session lifetime control; closing it
ends the session. One request owns one TCP connection, PeerConnection and
DataChannel.

Proxy timeouts: signaling is silent after the answer, so the bridge sends a
WebSocket Ping every 20 seconds on `/rtc` and `/stream`. Ping/Pong is not
activity; `--idle-seconds` counts NTCP2 bytes only. nginx defaults (60-second
`proxy_read_timeout`/`proxy_send_timeout`) and Cloudflare's ~100-second idle
limit are compatible. A proxy that times out before 25 seconds, or answers pings
itself without forwarding them, will break WebRTC sessions.

Targets: authentication uses subprotocol `gipny.ntcp2.v1.<43-character token>`,
never a URL; a client-visible token does not stop an informed malicious client.
Anyone can sign a RouterInfo naming someone else's `host:port`, so a target must
be a canonical, allowlisted public IPv4 with a live lease (signature, network,
address, identity and freshness rechecked) on a port of at least 1024 that is not
in `BLOCKED_NTCP2_PORTS` (`browser-router/src/ntcp2.rs`: Java I2P
`TransportUtil.isValidPort` exclusions plus proxies, databases, Redis, memcached,
Docker/etcd/Kubernetes, RDP/VNC/WinRM, MQTT/AMQP, Bitcoin and Tor control). The
same policy applies to target parsing, TSV loading, RouterInfo verification,
`prepare-netdb`, bootstrap and the WASM dialer; blocked TSV rows are skipped.
Each IP:port gets 4 dials in a burst, then one per 15 seconds (HTTP 429), on top
of the global burst of 32 refilled at 1/s. `/stream` dials before the upgrade;
`/rtc` authorizes and reserves its permit before the upgrade but dials only once
the DataChannel is open, so slow ICE/TURN/DTLS cannot consume the peer's
~10-second NTCP2 handshake timeout.

WebRTC is served only when answers cannot leak internal addresses (otherwise 404):
`--rtc-public-ip` for 1:1 NAT/cloud VMs (ideally with `--rtc-bind <private
interface IP>:0`), or `--rtc-bind <public IPv4>:0`. A wildcard or private bind
without `--rtc-public-ip` is a startup error. webrtc 0.21.1 stores but never
applies `with_nat_1to1_ips`, so the bridge rewrites its answer: only component-1
UDP host candidates remain, carrying `--rtc-public-ip` with unchanged ports. VPC,
docker0 and other interface addresses are never advertised. mDNS is disabled.
`--allow-fixture-ice-private` (CI only) permits wildcard binds and private
candidates.

UDP ports: webrtc 0.21.1 has no UDP mux and opens one socket per PeerConnection
without SO_REUSEADDR. Use port 0 so each session gets a kernel ephemeral port
(`net.ipv4.ip_local_port_range`, usually 32768–60999) and allow inbound UDP to
that range. To narrow it, run the bridge on its own VM or network namespace /
container with a smaller range (at least `--max-connections`). A fixed port
admits one RTC session and is therefore accepted only with `--max-connections 1`.

Browser ICE: `relay` (default) needs TURN; `all` needs at least one public browser
candidate, i.e. STUN in `rtc_ice_servers` or a public address. LAN/mDNS/IPv6
candidates are dropped and candidate-less offers are rejected; peer-reflexive
discovery is not used as a workaround.

TURN: credentials in the public `browser-config.json` are effectively long-lived
and world-readable, so the TURN server must relay only to the bridge. For coturn:
`allowed-peer-ip=<bridge IP>` plus deny-all (`denied-peer-ip=0.0.0.0-255.255.255.255`
and the IPv6 equivalent; coturn treats an address both allowed and denied as
allowed), at minimum `denied-peer-ip` for 0/8, 10/8, 100.64/10, 127/8,
169.254/16, 172.16/12, 192.168/16 and IPv6 loopback/ULA/link-local, plus
`no-multicast-peers` and `no-tcp-relay`. coturn ranges are written `start-end`
(`denied-peer-ip=10.0.0.0-10.255.255.255`), not CIDR. When the
configuration is refreshed frequently, prefer TURN REST (`use-auth-secret` +
`static-auth-secret`) with short lifetimes. Otherwise TURN is an open relay
operating from your server.

Bounds: at most 32 connections per process, burst 32 replenished at 1/s, 64 KiB
WSS frames, 16 KiB RTC messages, 128 KiB SCTP send/receive buffers, 600-second
maximum lifetime, 300-second maximum idle and 64 MiB total bidirectional traffic.
Defaults are 16 sessions, 600/120 seconds and 64 MiB; these are bounded initial
settings, not measured performance optima. Estimated (unmeasured) cost per
session: one TCP connection, plus for WebRTC a PeerConnection with DTLS/SCTP, a
UDP socket and a driver task; buffers up to 128 KiB SCTP each way and 2 × 64 KiB
WSS, i.e. a few hundred KiB. Every NTCP2 session of a browser router is a separate
bridge session and permit, and one user holds several at once (as many as its
tunnels need), so 16–32 connections serve only a few concurrent users; scale out
with more instances or hosts. Browsers reconnect after session, idle or byte limits.

The allowlist is reverified every five minutes and holds at most 2049 rows:
up to 2048 public rows from `prepare-netdb` plus one fixture row. Each lease
expires within ten minutes of verification or 36 hours of RI publication. Failed
reloads do not extend existing leases. Operators atomically replace the TSV; the
forwarder does not perform reseed. Keep browser snapshot, digest and allowlist
consistent. An old TSV is not a permanent service. Never enable fixture flags in
public deployments. No public service is deployed by this change; NAT, blocked
UDP and TURN/TLS need explicit CI/device validation.
[Browser configuration](../../docs/browser-messenger.md).
