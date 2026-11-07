# Browser router — WASM и транспорт / WASM and transport

## Русский

Отдельный Rust workspace предоставляет client-only I2P runtime, криптографию
мессенджера и зашифрованный browser vault. Точка входа пользователя —
`ui/browser.html`; [руководство](../docs/browser-messenger.md) описывает хранение,
размещение и конфигурацию. Отдельного native `gipny-web` больше нет.

| Код | Ответственность |
|---|---|
| `src/engine.rs` | Identity, X3DH/ratchet, begin/seal/commit/rollback, сессии и прекеи |
| `src/vault.rs` | AEAD vault, Argon2id, поколения и обёртки master key |
| `src/network.rs` | ClientSession, лимиты потоков, startup/stop и таймауты |
| `src/socket.rs` | Ограниченный байтовый поток WSS/WebRTC; local memory pairs |
| `src/tasks.rs` | TaskScope и отмена задач при остановке роутера |
| `src/bootstrap.rs`, `src/ntcp2.rs` | Проверка netDb/RouterInfo, адресов, портов, свежести и NTCP2 |
| `src/ui_helpers.rs` | Карточки/QR и ограниченные inline-вложения |
| `forwarder/` | Отдельный Rust WSS/WebRTC → TCP сервис без ключей переписки |
| `tests/` | WASM и browser/I2P сценарии; Python используется как CI-инструмент |

Emissary закреплён в `emissary-source.json`. `scripts/prepare_emissary.py`
проверяет SHA и точный diff `patches/client-only-ntcp2.patch`, создаёт
`emissary-provenance.json` и отказывается заменять неизвестные локальные правки.
Патч включает client-only NTCP2, memory stream и особенности client sessions.
Актуальный gitlink native i2pd берётся из дерева Git, а не из dirty checkout.

Криптография/I2P работают в Worker. Для WebRTC объект `RTCPeerConnection`
находится в Window; MessagePort передаёт только ограниченные транспортные байты.
`BrowserTcpStream` содержит Rust-каналы без `unsafe impl Send/Sync` для JS.
Стоп роутера отменяет также драйверы внешних сокетов. WSS и WebRTC выбираются
явно; fallback отсутствует. Native TCP/UDP в WASM отсутствуют.

Данные шифруются до форвардера. Он проверяет Host/Origin/capability и подписанный
RouterInfo, ограничивает назначения, соединения, кадры, время и объём. Порты
NTCP2 проходят общую политику `ntcp2::is_allowed_ntcp2_port` (не ниже 1024, без
сервисных портов) в мосту, `prepare-netdb`, bootstrap и `BrowserTcpStream`.
WebRTC дополнительно фильтрует SDP/ICE; по умолчанию браузеру нужен TURN, для
`all` за NAT — STUN. Квоты Window/Worker для MessagePort байтовые: 128 КиБ на
отправку (как `MAX_BUFFERED` в `socket.rs`) и 1 МиБ на приём.
Подробнее: [forwarder](forwarder/README.md), [модель угроз](../SECURITY.md).

Профиль ограничен 16 МиБ, потоковые файлы — 2 ГиБ и доступной квотой OPFS
с резервом 64 МиБ. Предпросмотр/очистка в памяти — до 32 МиБ. Нет native
DHT/agent/OS commands, duress и гарантий OS keychain.
PDF metadata sanitizer в WASM недоступен: отправка без очистки требует явного
выбора. WebAuthn PRF зависит от браузера/аутентификатора.

### Проверка

Сборка и исполняемые тесты — **только GitHub Actions**,
[Browser router WASM](../.github/workflows/browser-router.yml):

1. `forwarder`: Rust format, unit/integration tests, release binary.
2. `wasm`: locked builds, запрет native dependencies в WASM, browser UI и Chrome tests.
3. `ntcp2`: native peer, реальные NTCP2, cold bootstrap, два браузера с WSS и
   WebRTC, восстановление vault/очереди, публичные receipts без секретов.

Локально, с телефоном по USB (не CI): `tests/run_browser_phone.py` — браузер
(headless Chrome, локальный форвардер, снимок от `prepare-netdb`) обменивается
запросом контакта и сообщениями с приложением на телефоне через его DevTools
(нужна debug- или предрелизная сборка). 2026-11-07 с Honor KOZ-AL00: туннели
браузера 64 с, запрос и сообщение на телефоне через 32 с, ответ с телефона в
браузере через 2 с.

Артефакты: `browser-forwarder-linux`, `browser-messenger-pwa`,
`browser-router-wasm`, `browser-bootstrap-module` и evidence для NTCP2/bootstrap/
мессенджера. Файл с capability и приватные browser profiles не публикуются.
Текущая реализация WebRTC ещё не подтверждена новым CI. CI fixture использует
локальный ICE; это не проверка внешнего TURN, CDN или физических телефонов.
[Исторические измерения](../docs/archive/browser-validation-2026-11-01.md)
отделены от текущего статуса.

## English

This standalone Rust workspace implements client-only I2P, messenger crypto and
an encrypted browser vault. Users enter through `ui/browser.html`; see the
[browser guide](../docs/browser-messenger.md). The native local gateway is removed.

The source table above maps the engine (identity/ratchet/transactions), vault
(AEAD/Argon2id/generations), network lifecycle, bounded socket adapters, task
cancellation, signed bootstrap/NTCP2 verification, UI helpers, Rust forwarder
and CI fixtures. Emissary is pinned by `emissary-source.json`; preparation checks
both the exact commit and client-only patch, records provenance, and refuses
unknown local edits. Native i2pd uses the committed gitlink.

Crypto/I2P run in a Worker. RTCPeerConnection runs in Window and exchanges
bounded ciphertext bytes through MessagePorts. Rust stream objects contain
channels, without unsafe Send/Sync claims for JS objects. Router shutdown also
cancels socket drivers. Transport is explicitly WSS or WebRTC, with no fallback
or browser native TCP/UDP. The Rust forwarder authenticates, verifies RouterInfo
and bounds destinations, sessions, frames, time and bytes. NTCP2 ports pass the
shared `ntcp2::is_allowed_ntcp2_port` policy (at least 1024, no service ports) in
the bridge, `prepare-netdb`, bootstrap and `BrowserTcpStream`. RTC adds SDP/ICE
checks; the browser defaults to relay-only ICE requiring TURN (`all` behind NAT
needs STUN). Window/Worker MessagePort quotas are byte based: 128 KiB outbound
(matching `MAX_BUFFERED` in `socket.rs`) and 1 MiB inbound.

Browser limits are a 16 MiB profile and streaming files up to 2 GiB within
available OPFS quota, reserving 64 MiB. Previews/cleaning use at most 32 MiB.
Native DHT, agent commands, duress and OS keychain guarantees are unavailable. PDF cleaning is unavailable in WASM; unfiltered sending requires
an explicit choice. Passkey PRF support varies.

Builds and executable tests run only in GitHub Actions: Rust forwarder checks,
locked WASM/UI builds and Chrome tests, native NTCP2/cold bootstrap, and two
browser messenger scenarios over WSS and RTC. Artifacts include source/hash
provenance and public receipts, never capabilities or private profiles. New RTC
code still needs CI. Local ICE fixtures do not validate external TURN/CDN or
physical phones. Historical measurements are linked separately above.
