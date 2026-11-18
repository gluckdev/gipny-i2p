# Текущее состояние / Current state

Сверка: **2026-11-18**. Ветка `release/0.5.0-beta.1`; alpha2 не публикуется.
Метаданные пока `0.5.0-beta.1`. [Условия beta](plans/2026-11-16-beta.md)
обязательны до выпуска. Ни версия исходников, ни старый успешный CI сами по
себе не означают готовую бету или развёрнутый публичный сервис.

На beta-коммите `bb68aa7` с i2pd 3.0.0.1 прошли live jobs встроенного
роутера на Windows x64/ARM64 и macOS Intel/ARM64, сборка Android arm64
и две передачи файлов (10 МиБ с перезапуском получателя, 20 МиБ частями).
GitHub ответил через outproxy. Стабильного релиза для скачивания ещё не было,
поэтому проверка обновления завершилась ошибкой. Для неё задано
`continue-on-error`, поэтому весь запуск имеет статус success.
[Задания i2p-embed](https://github.com/gluckdev/gipny-i2p/actions/runs/44240401850).

## Реализовано и проверено

| Возможность | Реализация и границы доказательства |
|---|---|
| Native I2P, сообщения, группы, файлы | Собственный Core/Session поверх встроенного C++i2pd. Native external/embedded/agent/offline CI на `2139725` PASS; новые beta изменения требуют повторения |
| Браузерная самостоятельная сеть | JavaScript/Worker управляет локальным WASM router/криптографией и OPFS. WSS/WebRTC bridge переносит NTCP2-байты; сервер не хранит профиль и не заменяет browser Core |
| Browser WSS и WebRTC | [Baseline CI](https://github.com/gluckdev/gipny-i2p/actions/runs/43891713781): по11 assertions, 5MiB файл, переписка, receipts, reload и работа после остановки app host. Физические телефоны/TURN не проверены |
| Защитная политика | Application Destination:3 hops inbound/3 outbound, padding включён; caffeine/cocaine режимы удалены. Exploratory pool — отдельная политика; не приравнивать hops, tunnels, streams и router sessions |
| Файловые lanes | Default до4, measurement override1…8; окно16. Первый sweep1…8 доставил все файлы, но нарушил probes deadline на1/3/8. Native пробует по одному потоку и сравнивает скорость подтверждения частей; браузер регулирует число одновременных отправок по подтверждениям и таймаутам |
| Предзапуск | Native единый router, параллельные client/relay и owned cancellation; browser public bootstrap/cache/preload до unlock, generation guards. Измеренный выигрыш cold/warm ещё не установлен |
| Durable inbox | Ratchet, расход OPK и ciphertext inbox фиксируются синхронно до relay ACK. Unit и live delivery есть; устойчивость к реальной потере питания проверяется отдельно |
| Native storage | SQLCipher и OS credential provider; изоляция копий/удаление проверяются отдельными платформенными jobs. OpenWrt требует собственного provider. Нативный iOS снят с beta после проверки порта |
| Browser storage | Два AEAD-слота OPFS, пароль/WebAuthn PRF; зашифрованные вложения отдельно. Очистка сайта/eviction могут уничтожить профиль |
| Файлы в браузере | Multipart до2GiB, ограничение metadata16MiB и quota reserve, проверка полного SHA-256. Малый live файл не подтверждает2GiB на каждом телефоне |
| UPnP | Собственный native IPv4 IGD manager, контроль владения mappings и отмены; браузер не управляет IGD. Физические NAT/маршрутизаторы требуют отдельной проверки |
| Уведомления | Полный текст в центральной полупрозрачной карточке; обычное уведомление не перехватывает клики, длинное можно прокручивать. Layout/keyboard/modal tests есть |
| Новые взаимодействия | Auto-add полной валидной вставленной/сканированной карточки без повышения trust, QR возле имени, лёгкие hover/focus/blur, reduced-effects, camera cancellation. [Описание](ui-interactions.md); после naming нужна финальная матрица |
| Android16KiB | Linker flags и verifier ELF/APK сохранены,15 unit tests PASS. Старый native router ELF не соответствовал16KiB. Новый APK и runtime ещё ожидаются |
| Голос | [Личные I2P-звонки, голосовые сообщения и проверки](voice.md) в beta. Публичный браузер требует настроенного маршрута |

## Не готово или требует отдельной среды

| Область | Что остаётся |
|---|---|
| Публичный web | По последней проверке известного Pages URL `browser-config.json`404 и нет подтверждённого публичного endpoint. Нужен [deployment](browser-network-deployment.md); пользовательский URL ещё не известен |
| Honor Cromite153.0.8010.37 | Точная Linux-версия с site JIT permission запускает WASM; это не Android и не диагноз старого профиля Honor. Не очищать OPFS. [Результаты](startup-performance.md) |
| realme Chrome / все актуальные браузеры | Реальные устройства, Firefox/WebKit/Chrome/Cromite capability matrix, ограничения политик WASM/OPFS/WebAuthn; нельзя обещать обход блокировки движка |
| Автовыбор транспорта | Native NTCP2/TCP + SSU2/UDP есть; browser WSS/WebRTC выбор пока явный. Automatic fallback, TURN UDP/TCP/TLS и IPv6-only требуют реализации/проверки |
| QUIC, WebTransport, ICMP | Не реализованы как новые I2P transports; не включать в список готовых fallback |
| Адаптивные lanes и resume | [Исследование/план](network-adaptation.md): сначала корректные measurement и fault tests, потом bounded adaptation; received-byte durability зависит от ACK/AEAD/storage boundaries |
| Перенос профиля | [Проект](profile-transfer.md): толькоI2P, оба пароля, полный карантин, sourceОК/Отмена, delete-before-activate. Старый backup не является готовой заменой |
| Установщики/OS/ABI | [Матрица](platform-support.md): universal packaging, расширенные targets и нижние OS floors требуют настоящих зависимостей и runtime checks |
| Звонки/экран | Будущее направление; WebRTC byte transport не реализует звонки, file goodput не доказывает media readiness |
| Другие незавершённые функции | DHT-имена/владение, Android biometric/share flow, синхронизация аватаров, расширенные group roles, независимая подпись издателя |
| ЗеркалаI2P | [План](site-mirrors.md); адресов и работающего deployment пока нет |

История/TTL/удаление и уведомления о копировании не контролируют внешние копии,
скриншоты или вредоносный клиент. Передача релею не равна получению собеседником.
Никакой benchmark не доказывает абсолютную анонимность или неотличимость трафика.

## Доказательства и следующая проверка

Baseline `2139725` прошёл build, security, native e2e, browser, CodeQL,
installer checks. Release preview был неполным (macOS runner capacity); он не
публиковался. Новый benchmark harness `f73fd42` исправляет deadline/checkpoint:
предыдущий повтор6,4,6,4 имеет две завершённые точки, третью с неизвестным
результатом и незапущенную четвёртую. [Измерения](performance-anonymity.md).
Новые тесты, provenance и установка финальных beta artifacts нужны на одном SHA.

Старые данные/форматы/API можно менять без aliases и миграций. Это прямое
решение владельца; реальные пользовательские файлы не удаляются автоматически.
Целостность новых данных, atomic ratchet commit, replay-защита и честные статусы
доставки обязательны. Чужой upstream/generated-код не рефакторится ради naming.

## English

On beta commit `bb68aa7`, embedded-router jobs passed on Windows x64/ARM64
and macOS Intel/ARM64. The Android arm64 router build and both file-delivery
jobs passed. GitHub answered through the outproxy. The optional updater job failed because there was no stable release to download.
[CI jobs](https://github.com/gluckdev/gipny-i2p/actions/runs/44240401850).

Development targets beta; alpha2 is not being published. Baseline `2139725`
passed native delivery, browser WSS/WebRTC, build/security and CodeQL checks.
Those results do not validate subsequent beta code, public hosting, physical
phones or TURN. The web client owns its I2P router and keys locally; the byte
forwarder does not replace it with a server-owned messenger.

Application tunnels retain3/3 hops and padding. Default file lanes remain4;
the first1…8 sweep delivered every file but missed interactive deadlines.
Live validation of adaptive concurrency, stronger resume verification, wider platform
coverage and the I2P-only profile move remain beta gates. UI improvements and
Android16KiB structural fixes are implemented; final integration and runtime
checks are still required. No calls, arbitrary QUIC/ICMP fallback, universal
traffic camouflage or guaranteed immunity to device/storage failure is claimed.

Old-data/API compatibility and migrations are explicitly unnecessary. New-data
integrity, cryptographic atomicity and truthful receipt states remain mandatory.
Existing user files must not be silently deleted. See the [beta gates](plans/2026-11-16-beta.md),
[transfer design](profile-transfer.md), [network plan](network-adaptation.md) and
[platform matrix](platform-support.md).
