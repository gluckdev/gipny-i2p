# Безопасность / Security

## Русский

Gipny — экспериментальный мессенджер поверх I2P. Сквозное шифрование,
анонимность сети и защита устройства решают разные задачи. Наличие этих
механизмов не означает независимого аудита или гарантии против любого противника.
Текущее состояние исходников и непроверенные возможности указаны в
[матрице функций](docs/current-state.md).

### Сообщить об уязвимости

Используйте [приватный отчёт GitHub](https://github.com/gluckdev/gipny-i2p/security/advisories/new).
Если форма недоступна, создайте issue **без технических подробностей** с просьбой
предоставить приватный канал. Укажите версию/commit, платформу, воспроизведение,
наблюдаемый ущерб и ожидаемое поведение. Не прикладывайте настоящий профиль,
пароли, ключи или переписку. Локальное ревью до исправления не публикуется.

Целевой срок подтверждения отчёта — семь дней. Принятые исправления и имя автора
(по его желанию) описываются после доступности обновления. Ветка 0.5.0 сейчас
содержит prerelease-код; обозначение RC не равно стабильному выпуску. Старые
release notes — история, а не подтверждение поддержки или доступного скачивания.

### Границы защиты

| Граница | Реализуемая защита | Что остаётся видимым или возможным |
|---|---|---|
| Сообщение | X3DH, Double Ratchet и аутентификация до передачи в I2P | Собеседник видит текст и может его сохранить; отсутствие потерь требует отдельных проверок состояния и ACK |
| Native сеть | Встроенный i2pd, маршруты I2P | Провайдер видит сетевую активность; глобальная корреляция трафика не исключена |
| Browser сеть | Client-only I2P в WASM; внешний WSS или WebRTC переносит NTCP2-шифротекст | Форвардер видит выбранный I2P-узел, время и объём; WSS/signaling и TURN имеют собственных операторов |
| Native профиль | SQLCipher/vault и OS credential provider | Разблокированная ОС, native/GUI copies и crash/swap требуют отдельной модели угроз |
| Browser профиль | AEAD-слоты OPFS, пароль/Argon2id, поддерживаемый WebAuthn PRF | Нет native mlock, SQLCipher или гарантии аппаратной привязки; удаление данных origin может уничтожить профиль |
| Обновления | Обязательный SHA256, лимиты загрузки, provenance | Digest на той же странице релиза не является независимой подписью издателя |

WSS может проходить через совместимый CDN. WebRTC использует ICE/DTLS/SCTP;
обычный HTTP/CDN-прокси не становится TURN. В WebRTC выбран `relay` по умолчанию:
нужны TURN credentials. Явный `all` разрешает непосредственный путь до
форвардера и раскрытие IP ему. Сигнализация WSS всё равно может раскрывать IP
её оператору. Прямого WebRTC между собеседниками и скрытого переключения
транспортов нет. Эти механизмы **не доказывают неразличимость трафика**.
Новые WebRTC-изменения требуют CI и проверок NAT/TURN/реальных устройств.

Ключи I2P и ratchet в браузере обрабатывает Rust Worker; отображаемый текст и
вводимый пароль неизбежно проходят через UI. Компрометация кода origin,
расширения или браузерного процесса может обойти эту изоляцию. Web hosting
не получает профиль или сообщения через API приложения, но контролирует
загружаемый код. Сервер байтового транспорта — отдельная граница доверия.

Удаление, TTL и очистка RAM ограничивают доступные приложению копии; они не
стирают скриншоты, резервные копии и данные на чужом устройстве. Не обещается
защита от root/скомпрометированной ОС. Ошибки проверки полномочий, lifecycle,
входных лимитов и обращения с секретами в нашем коде всё равно принимаются.
Проблемы upstream i2pd направляйте также в
[PurpleI2P/i2pd](https://github.com/PurpleI2P/i2pd).

Стабильные native releases должны иметь `Null` diagnostics. В RC разрешён
явный developer log; он не должен содержать секреты. Browser diagnostics
имеют отдельный lifecycle и не наследуют автоматически native гарантии.
См. [журналы](docs/release-diagnostics.md), [RAM](docs/memory-security.md),
[привязку профиля](docs/device-bound-storage.md) и [браузер](docs/browser-messenger.md).

## English

Gipny is experimental. End-to-end encryption, I2P anonymity and device security
are separate protections, not an independent audit or a universal guarantee.
See the [current feature matrix](docs/current-state.md) for implementation and
validation boundaries.

Report privately through [GitHub Security](https://github.com/gluckdev/gipny-i2p/security/advisories/new).
If unavailable, ask for a private channel in an issue without technical details.
Include version/commit, platform, reproduction and impact, never real profiles,
keys, passwords or conversations. The acknowledgement target is seven days;
accepted fixes and optional credit are announced after an update is available.
The 0.5.0 source tree is prerelease code. Historical release notes do not prove
current support or downloadable assets. Unfixed review details stay private.

Messages use X3DH and Double Ratchet before transport. Recipients can retain
plaintext. Native clients embed i2pd; browsers run a client-only I2P router in
WASM. Providers still observe network activity; global traffic correlation is
not excluded. WSS/WebRTC forwarders see entry peers, timing and volume, without
terminating NTCP2 or holding messenger keys. WSS can use a compatible CDN;
WebRTC uses ICE/DTLS/SCTP and needs a separate TURN service for relay-only ICE.
`relay` is the WebRTC default; explicit `all` exposes the client IP to the
forwarder. WSS signaling can still expose IP to its operator. There is no direct
contact-to-contact RTC or automatic transport fallback. None of this proves
traffic indistinguishability. New RTC code still needs CI, NAT/TURN and device
acceptance.

Native profiles use SQLCipher/vault with an OS credential provider. Browser
profiles use encrypted OPFS slots, Argon2id and supported WebAuthn PRF, without
native memory locking, SQLCipher or guaranteed hardware binding. Rust Worker
owns I2P/ratchet keys; UI still handles displayed text and password input.
Origin-code compromise, hostile extensions or a compromised browser/OS can
bypass that isolation. Static hosting supplies executable code, although the
application does not upload profiles or messages to it. Transport forwarding
is a distinct trust boundary.

Deletion, TTL and memory clearing cannot erase recipient screenshots, backups
or copies beyond application control. Our authorization, bounds, lifecycle and
secret-handling bugs remain in scope; report upstream i2pd defects upstream too.
Mandatory update hashes and provenance do not replace independent publisher
signatures. Stable native releases require Null diagnostics; RC builds may
explicitly enable developer logs without secrets. Browser logging has separate
storage and lifecycle. See the linked detailed guides above.
