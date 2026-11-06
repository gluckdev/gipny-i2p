# Текущее состояние / Current state

Сверка: 2026-11-06, исходный HEAD `a6e41caa07ffff94a834f72a4c13acd73f188224`
и изменения ревью к `0.5.0-rc3`. Версия исходников не подтверждает публикацию
всех перечисленных функций; локальные unit-тесты пройдены, CI и устройства —
отдельно. / Reviewed against that baseline and the rc3 review changes; local unit
tests pass, while CI and device validation are separate evidence.

## Русский

| Возможность | Состояние |
|---|---|
| Native I2P, личные сообщения, группы, файлы | Реализация в main; безопасность и отказоустойчивость проверяются отдельно |
| WASM / Worker / OPFS, пароль и WebAuthn PRF | В дереве; прежние результаты CI привязаны к своим SHA |
| Файлы до 2 ГиБ в браузере | Потоковое зашифрованное OPFS-хранилище, SHA-256 и финальные ACK; повтор части — новым шифрованием, ограниченная история повторов, окно на контакт. CI и проверка телефон ↔ ПК ожидаются |
| Сообщения с файлами и запросы контактов | Общая атомарная запись metadata для Core/SessionManager; browser policy согласована, новые проверки подготовлены |
| Надёжный приём (durable inbox) | Ratchet, расход OPK и письмо фиксируются одной синхронной транзакцией до ACK релею; сбои хранилища повторяются. Core и SessionManager, unit-тесты; crash-injection на устройстве не выполнялся |
| Проверка X3DH, групп, длины ключей | Init проверяется до изменения состояния; состав группы меняет только участник; неверная длина ключа — ошибка, не panic |
| Локальный gipny-web | Удалён по решению владельца; веб-клиент — WASM |
| WSS / WebRTC Rust-мост | Только NTCP2-цели на допустимых портах, keepalive для прокси, TCP после DataChannel; unit/e2e-тесты моста пройдены локально, живые TURN/NAT — нет |
| Pages /app/browser.html | Workflow готовит WASM-клиент; публикация требует успешного запуска main |
| Онлайн веб-сервис | Дополнительно нужны HTTPS-хостинг, свежий netDb, доступный мост и конфигурация |
| Повторный запуск без GitHub/хостинга | Код, публичные настройки и проверенный netDb кэшируются; свежесть RI ограничена 36 часами. Сценарий CI подготовлен, долгосрочное обновление bootstrap без хостинга ещё не завершено |
| WebRTC через TURN | Настройка предусмотрена; отдельные NAT/TURN, CDN и мобильные проверки обязательны |
| Маскировка native NTCP2/SSU2 | В нативный транспорт ещё не интегрирована |
| UPnP в desktop/Android | Включён по умолчанию с переключателем; нейтральные описание и User-Agent, метка своя для каждого шлюза, IP публикуется после проброса. Unit/fixture-тесты есть; реальные роутеры не проверены |
| Yggdrasil | Удалён по решению владельца: только I2P |
| Сайт RU/EN, безопасность и загрузки | Расширено описание, одна основная загрузка по системе/архитектуре и ссылка на все релизы; локальная data-проверка 18/18 |
| Зеркала `.onion` / `.i2p` с `gipny` | Записан план, адреса и развёртывание ещё не подготовлены |
| История по выбору, таймеры, запросы контактов | Локальные механизмы присутствуют; независимые копии не контролируются |
| DHT-имена | Не завершена интеграция в клиент и модель владения; используйте карточки |
| Android биометрия / входящие Share intents | Есть заготовки; завершённый пользовательский путь не подтверждён |
| Аватар | Локальный выбор; синхронизация с контактом не подтверждена |
| Уведомления о копировании/сохранении/снимке | Автор сообщения видит, что собеседник скопировал текст, сохранил вложение или нажал сочетание клавиш снимка; без содержимого, удаляются вместе с сообщением. Native и браузер; сочетание клавиш не доказывает снимок |
| Звонки, XMR, SMS/PSTN, почта, MIPS | Направления развития, не заявленные рабочие функции |
| Независимая подпись релизов | План; checksum с того же сервера не заменяет подпись издателя |

Старые данные и протоколы можно менять без миграций. Это не разрешает потерю
новых сообщений, частичное сохранение ratchet или ложное подтверждение доставки.
Исполняемые компоненты разрабатываются на Rust/C; существующий native i2pd —
C++ upstream. Python и другие языки разрешены в CI и служебных инструментах.

## English

| Capability | Status |
|---|---|
| Native I2P, direct/group messages and files | Implemented in main; security and recovery require separate validation |
| WASM / Worker / OPFS, password and WebAuthn PRF | Present; historical CI results apply only to their recorded commits |
| Browser files up to 2 GiB | Streamed encrypted OPFS storage, SHA-256 and final receipts; lost parts are re-encrypted, replay history is bounded, windows are per contact. CI and physical phone/PC checks pending |
| File messages and contact requests | Shared atomic metadata publication in Core/SessionManager; browser policy aligned and new scenarios prepared |
| Durable inbox | Ratchet state, OPK use and the letter commit in one synced transaction before the relay ACK; storage failures retry. Core and SessionManager, unit-tested; no on-device crash injection yet |
| X3DH, group and key-length checks | Inits are authenticated before state changes; only members change a group; bad key lengths are errors, not panics |
| Local gipny-web | Removed by owner decision; WASM is the web client |
| Rust WSS / WebRTC bridge | NTCP2 targets on allowed ports only, proxy keepalive, TCP after the DataChannel opens; bridge tests pass locally, live TURN/NAT pending |
| Pages /app/browser.html | Workflow prepares WASM; publication needs a successful main deployment |
| Online browser service | Also needs HTTPS hosting, fresh netDb, an available bridge and configuration |
| Restart without GitHub/hosting | Code, public settings and validated netDb are cached; RI freshness is limited to 36 hours. CI scenario prepared; long-term bootstrap refresh without hosting remains unfinished |
| TURN WebRTC | Configurable; NAT/TURN, CDN and physical mobile checks remain separate |
| Native NTCP2/SSU2 camouflage | Not integrated into the native transport |
| Desktop/Android UPnP | On by default with a switch; neutral description and User-Agent, per-gateway tags, IP published after a confirmed mapping. Unit/fixture tests exist; physical routers untested |
| Yggdrasil | Removed by owner decision: I2P only |
| RU/EN site, security and downloads | Expanded copy, one primary OS/architecture download and all-releases link; local data checks 18/18 |
| `.onion` / `.i2p` mirrors with `gipny` | Planned; addresses and deployment are not prepared yet |
| History choice, timers and contact requests | Local mechanisms exist; independent copies remain outside their control |
| DHT names | Client integration and ownership design are unfinished; use contact cards |
| Android biometrics / incoming Share intents | Partial implementation; a complete user flow is unverified |
| Avatars | Local selection; peer synchronization unverified |
| Copy/save/screenshot notices | Authors see when the contact copied their text, saved their attachment or pressed a screenshot shortcut; no content, deleted with the message. Native and browser; a shortcut does not prove a screenshot |
| Calls, XMR, SMS/PSTN, mail and MIPS | Development directions, not released capabilities |
| Independent release signing | Planned; a same-server checksum is not a publisher signature |

Old data/protocol compatibility and migrations are not required. New-data
integrity, ratchet atomicity and accurate delivery states remain requirements.
Runtime components use Rust/C; the existing native i2pd upstream is C++.
Python and other languages are permitted in tests, CI and tooling.

[Документация / Documentation](README.md) · [Планы / Roadmap](roadmap.md)
