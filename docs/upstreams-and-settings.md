# Источники и настройки / Upstreams and settings

## Русский

Срез рабочей копии на **2026-11-06**. Обновление источников не подтверждает,
что новая сборка прошла CI или доставила сообщения на реальных устройствах.

| Компонент | Закреплённая ревизия / источник |
|---|---|
| i2pd desktop и Android library | `2ad5c1a537193064070a63fc7ef8fe897663f364` |
| Android upstream build/dependencies | `df7e2420b6a5108caa7463bde420d3ab48ff2888` |
| emissary для WASM | `620fb9debb0cbfd41de664ca5297c1deab8bd40b`, `browser-router/emissary-source.json` |
| miniupnpc | 2.3.3/API 21, `2cb87d04034587bdb0f51549b3936e0d9e143bcb`, `i2p-embed/miniupnpc-source.json` |

i2pd сохраняет публичный patch очистки собственных секретных буферов из
`i2p-embed/patches/`: скрипт отказывается накладывать его на несовпадающий
контекст. emissary сохраняет проверяемый client-only patch. miniupnpc содержит
ограничения HTTP/SSDP и проверку источника Location; исходные и изменённые хеши
записаны отдельно. Вложенный i2pd Android должен совпадать с desktop pin.

Нативный роутер: NTCP2 и SSU2 включены, подпись reseed обязательна, SAM/локальные
прокси/веб-консоль выключены. [UPnP](upnp.md) управляется отдельным модулем,
а не оставлен неработающим флагом. Bandwidth/share/transit следуют профилю;
первый запущенный native router задаёт процессу настройки до перезапуска.
Нулевые хопы и снятие защиты не считаются общей настройкой производительности.

Браузер использует явно выбранный WSS или WebRTC; WebRTC по умолчанию требует
TURN relay. Очереди и входящие кадры ограничены, исправные релейные соединения
переиспользуются, до четырёх операций доставки перекрываются по времени.
[Файлы](file-transfer.md) хранятся и передаются частями. Настройки не заменяют
измерения: скорость, задержку, батарею, потери и поведение NAT нужно сравнивать
на одной версии клиентов в одинаковых условиях.

Полное обновление Cargo/npm/Boost/OpenSSL и остальных вложенных зависимостей
не объявляется завершённым этой таблицей. Используйте соответствующие lockfiles,
зафиксируйте изменения API/MSRV/лицензий и результаты всех платформ в CI.
Репозиторий не должен собирать движущуюся ветку upstream вместо выбранного SHA.

## English

Working-tree snapshot: **2026-11-06**. The table above records desktop/Android
i2pd, Android upstream, browser emissary and miniupnpc pins. Source updates do
not prove successful builds or real-device delivery. Retain the public i2pd
secret-buffer cleanup patch, emissary's client-only patch and miniupnpc's bounded
HTTP/SSDP/origin validation; refuse mismatched patch contexts. Android's nested
i2pd must match the desktop pin. Original and modified miniupnpc hashes are separate.

Native NTCP2/SSU2 and signed reseed are enabled; SAM/proxies/web console remain
disabled. [UPnP](upnp.md) has an actual lifecycle manager. Bandwidth/share/transit
follow the selected process-wide router profile; zero-hop/privacy reductions
are not general performance defaults. The browser explicitly selects WSS or
WebRTC, defaulting to TURN relay for WebRTC, with bounded queues, connection
reuse and at most four overlapping delivery operations. [Files](file-transfer.md)
stream in chunks. Measure latency, throughput, loss, power and NAT behavior with
matching clients and controlled conditions before claiming optimal settings.

This table does not claim completion of every Cargo/npm/Boost/OpenSSL or nested
dependency update. Keep lockfiles, API/MSRV/license review and platform CI evidence
with each change. Never substitute a moving upstream branch for the reviewed SHA.
