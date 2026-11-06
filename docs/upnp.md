# UPnP встроенного роутера / Embedded-router UPnP

## Русский

Состояние на 2026-11-06 (0.5.0-rc3): реализация и модульные тесты; CI и реальные
домашние роутеры ещё не проверены. Сам флаг `upnp.enabled` UPnP не запускает:
приложение использует API libi2pd без daemon-компонента i2pd, поэтому порты
открывает собственный менеджер `i2p-embed`.

**Включение.** Настройки → Роутер i2p → «Автоматически открывать порт на
роутере (UPnP)». По умолчанию включено; изменение действует после перезапуска
приложения. Выключено — менеджер не запускается совсем: нет SSDP-поиска, правил
и файлов. В браузерном клиенте UPnP нет.

**Что открывается.** Только реальные IPv4-порты NTCP2/TCP и SSU2/UDP встроенного
роутера. SAM, HTTP-консоль, SOCKS и приложение наружу не открываются;
hidden/proxy-профиль обычный публичный маппинг не получает.

**Приватность.**
* Описание правила нейтральное — `UPnP <10 hex>`, без имени приложения.
  User-Agent HTTP-запросов — `<ОС> UPnP/1.1 MiniUPnPc/2.3.3`.
* Метка владения — HMAC-SHA256 локального секрета по UDN и LAN-адресу шлюза:
  у каждого шлюза своя, по меткам разные сети между собой не связать. Роутеры
  одной модели с общим UDN и одинаковым LAN-адресом дают одинаковую метку.
* Внешний IP передаётся роутеру i2p только после подтверждённого маппинга на
  этом шлюзе и только если адрес публичный. Тем не менее правило в таблице
  шлюза видят все устройства локальной сети и провайдер, управляющий роутером:
  это раскрывает, что на компьютере работает P2P-программа. При VPN с доступом
  к локальной сети маппинг на домашнем шлюзе может опубликовать домашний IP —
  в таком случае UPnP лучше выключить.

**Поведение.**
* Обнаружение через SSDP без отдельного daemon: не больше 64 ответов и 8
  описаний; HTTP Location — числовой частный IPv4 отправителя ответа, SOAP URL —
  на том же устройстве; без DNS и переходов на внешние адреса.
* Перед AddPortMapping проверяется существующее правило; только ответ «нет
  записи» разрешает создание. Чужое правило не трогается и не сбрасывает шлюз;
  после ошибки 718 правило перепроверяется. Состояние ведётся для каждого правила.
* Журнал владения (`PROTO PORT CLIENT GATEWAYID`, до 16 записей) пишется
  атомарно до изменения шлюза; «нет такого маппинга» снимает запись только для
  того же шлюза. Повреждённый файл секрета пересоздаётся (временный файл +
  rename), журнал при этом очищается.
* Аренда 600 с, продление каждые 180 с; для шлюзов только с постоянной арендой
  (ошибка 725) — постоянное правило с записью в журнале.
* Фокус окна и смена сети больше не пересоздают правила: события сглаживаются
  (2 с), проверка шлюза — не чаще раза в 30 с и только перечитывает его описание.
  Шлюз сбрасывается лишь если он другой или недоступен; удаления при этом нет.
* Без шлюза поиск повторяется с паузой 60, 120 … до 1800 с; смена сети сбрасывает
  паузу.
* При выходе удаляются только постоянные правила; 600-секундные истекают сами.
  Ожидание остановки ограничено ~3 с, текущий HTTP-запрос прерывается на
  следующем принятом блоке.

**Ограничения.** UPnP доверяет локальной сети и шлюзу: атомарной операции
«заменить только это правило» нет, член LAN может вмешаться между проверкой и
записью. CGNAT провайдера и второй NAT правило на домашнем шлюзе не обходит.
При аварийном завершении или потере питания постоянное правило может остаться до
следующего запуска в той же сети. В релизных сборках UPnP ничего не пишет в журнал.

Исходники: `i2p-embed/src/upnp.rs`, C-граница в `i2p-embed/shim/`, закреплённый
импорт `i2p-embed/vendor/miniupnpc` с хешами `miniupnpc-source.json`
(`python3 -I i2p-embed/check-source.py`). Тесты: 16 в `upnp.rs` (шлюз в памяти) и
3 в `upnp_fixture.rs` (настоящая miniupnpc против локального IGD); в CI для
Linux/macOS/Windows и проверка экспортов Android. Физические NAT, переключение
Wi-Fi/mobile и перезапуск IGD проверяются отдельно.

## English

Status on 2026-11-06 (0.5.0-rc3): implemented with unit tests; CI and physical
home routers are pending. `upnp.enabled` alone does nothing: the app embeds the
libi2pd API without i2pd's daemon, so `i2p-embed` runs its own manager.

**Switch.** Settings → i2p router → "Open the router port automatically (UPnP)".
On by default; a change applies after the app restarts. Off, the manager never
starts: no SSDP search, no rules, no files. The browser client has no UPnP.

**Scope.** Only the router's actual IPv4 NTCP2/TCP and SSU2/UDP listeners. No
SAM, HTTP console, SOCKS or application service; hidden/proxy profiles get no
ordinary public mapping.

**Privacy.** The rule description is neutral (`UPnP <10 hex>`) and the HTTP
User-Agent is `<OS> UPnP/1.1 MiniUPnPc/2.3.3`. The ownership tag is an
HMAC-SHA256 of a local secret over the gateway's UDN and LAN address, so tags on
different gateways cannot be linked (gateways of one model sharing a UDN and LAN
address yield the same tag). The external IP reaches the i2p router only after a
mapping on that gateway is confirmed, and only if it is public. The rule itself
still tells every LAN device, and an ISP that manages the router, that a P2P
program runs here; with a VPN that allows LAN access, a home-gateway mapping can
publish the home IP — switch UPnP off there.

**Behaviour.** Bounded SSDP discovery (64 replies, 8 descriptions; numeric
private Location matching the datagram source; SOAP on the same device; no DNS
or redirects). Existing rules are checked first; only explicit absence allows
creation; foreign rules neither get touched nor drop the gateway; error 718 is
re-queried; state is kept per rule. The ownership journal
(`PROTO PORT CLIENT GATEWAYID`, at most 16 entries) is written atomically before
any change and retired only by that gateway's "no such mapping". A corrupt secret
file is regenerated via temp file and rename, clearing the journal. Leases are
600 s, renewed every 180 s; permanent-only gateways (error 725) get journalled
permanent rules. Focus and network events are debounced (2 s) and re-verify the
gateway at most every 30 s without deleting or re-adding rules. With no gateway,
discovery backs off 60, 120 … 1800 s and resets on a network change. At exit only
permanent rules are deleted; stop waits about 3 s and aborts an in-flight request.

**Limits.** UPnP trusts the LAN and gateway; there is no compare-and-swap for
rules. Provider CGNAT or a second NAT is not solved by a home-gateway rule. A
crash or power loss can leave a permanent rule until the next start on the same
network. Release builds log nothing about UPnP.

Sources: `i2p-embed/src/upnp.rs`, the C boundary in `i2p-embed/shim/`, pinned
`i2p-embed/vendor/miniupnpc` with `miniupnpc-source.json`. Tests: 16 in `upnp.rs`
(in-memory gateway), 3 in `upnp_fixture.rs` (real miniupnpc against a local IGD),
run in CI on Linux/macOS/Windows, plus Android export checks. Physical NAT,
Wi-Fi/mobile transitions and IGD restarts need separate evidence.
