# Безопасность / Security

*English summary at the end.*

## Куда сообщать об уязвимости

Не открывайте публичный issue с деталями. Используйте приватный отчёт GitHub:
**Security → Report a vulnerability** —
<https://github.com/gluckdev/gipny-i2p/security/advisories/new>.
Если эта форма недоступна, откройте issue **без деталей** («есть уязвимость,
нужен канал») — вам ответят, как передать описание.

В отчёте полезно: версия (`Настройки → Обновления` или имя файла со страницы
релиза), платформа, шаги воспроизведения, что именно раскрывается или ломается.

## Что ждать

- Подтверждение получения — в течение 7 дней.
- Исправление выходит отдельным патч-релизом; в его заметках уязвимость
  описывается после того, как обновление доступно на всех платформах.
- Отчёт, который принят, упоминается в заметках релиза с указанием автора,
  если автор этого хочет.

## Поддерживаемые версии

| Версия | Исправления безопасности |
| ------ | ------------------------ |
| 0.4.x  | да                       |
| < 0.4  | нет — обновитесь         |

## Что считается уязвимостью здесь

В зоне ответственности проекта: криптография и протокол (`libcore`), релей
(`core/relay`, `libcore/src/relay_server.rs`), сеть релеев (`dht`,
`libcore/src/dht_client.rs`), режим агента и `gipny-agent`, приложение
(`core`, `ui`, Android-обвязка). Особенно интересны: чтение или подмена
сообщений третьей стороной, раскрытие отправителя релею или сети, чтение
или подмена записей в сети релеев тем, кому они не адресованы, выполнение
команд агентом не от мастера, обход блокировки или удаления контакта.

Не сюда: уязвимости самого `i2pd` (сообщайте в [PurpleI2P/i2pd](https://github.com/PurpleI2P/i2pd));
атаки, требующие root или физического доступа к разблокированному устройству;
деанонимизация свойствами сети I2P как таковой.

### Отдельная браузерная граница (WASM / Web Worker)

Браузерная версия мессенджера (`browser-router` / PWA) имеет собственную отдельную
границу доверия:

- Приватные ключи содержатся исключительно в Rust Web Worker (`BrowserEngine`), а профиль сохраняется в шифрованных слотах OPFS (Argon2id 64 MiB, PRF passkeys).
- В браузере **отсутствуют** нативные гарантии ОС: нет `mlock`/`mprotect` и `PROT_NONE`, нет SQLCipher, нет аппаратного keychain, duress-паролей и деструктивного стирания по счётчику попыток.
- Атаки, использующие вредоносные браузерные расширения, компрометацию хостинга страницы, инспекцию незаблокированной памяти процесса браузера или сборщика мусора JS, находятся вне модели угроз браузерной границы.
- WSS-форвардеры выполняют байтовую ретрансляцию с проверкой allowlist подписанных RouterInfo и не терминируют сквозные Noise-сессии NTCP2.

---

**English.** Report vulnerabilities privately via
<https://github.com/gluckdev/gipny-i2p/security/advisories/new> (or an issue
without details asking for a channel). Acknowledgement within 7 days; fixes
ship as patch releases. Supported: 0.4.x. Out of scope: i2pd itself
(upstream), attacks needing root or an unlocked device, I2P network-level
deanonymisation.
*Browser WASM boundary note:* Private keys stay in the Rust Web Worker and OPFS
is encrypted, but the browser lacks OS memory locking (mlock), SQLCipher,
hardware keychain, duress, or tamper wipe counters. Hostile extensions, origin
compromise, and browser memory inspection are out of scope.
