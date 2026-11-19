# Браузерный мессенджер / Browser messenger

Сверка / Reviewed: 2026-11-18. Public Pages `browser-config.json` по последней
проверке отвечает 404; публичный bridge не подтверждён. На baseline `2139725`
[CI WSS и WebRTC](https://github.com/gluckdev/gipny-i2p/actions/runs/43891713781)
прошёл по 11 acceptance checks, включая файл и восстановление после reload/остановки
хостинга. Это fixture, не физический телефон или TURN. Новые beta изменения
требуют своей проверки. / Baseline transport CI passed; public deployment,
physical phones, TURN and the final beta matrix remain separate gates.

## Русский

Веб-клиент — `ui/browser.html`: общий интерфейс, Rust WASM-ядро в Dedicated Worker,
client-only emissary и зашифрованный профиль в OPFS. Отдельный локальный
`gipny-web` и сопряжение с ним удалены. Сайт ведёт к `/app/browser.html`.
Pages workflow собирает этот WASM-клиент; факт публикации проверяется отдельно.

### Использование

Откройте HTTPS-страницу, создайте профиль с паролем, дождитесь I2P и обменяйтесь
карточками. Проверьте отпечатки по доверенному каналу. При наличии настоящего
WebAuthn PRF можно добавить passkey; пароль сохраняйте как способ восстановления.
Добавление PWA на главный экран зависит от браузера. После первого кэширования
оболочки сохранённая история доступна без сети. Очистка данных сайта и квоты
браузера могут удалить профиль. Для beta проектируется [перенос профиля через I2P](profile-transfer.md)
с финальным ОК/Отмена на источнике. Он пока не готов; прежний native export
не является полным переносом и не означает его наличие в WASM-клиенте.

### Требования к браузеру

Cromite153.0.8010.37 отдельно проверен на Linux: при штатной политике JIT WASM
недоступен в Window и Worker; с разрешением для origin минимальная compile
и загрузка клиента проходят. Это не Android-тест и не установленная причина
регрессии Honor. [Точные результаты](startup-performance.md). Удалять данные
сайта/OPFS ради диагностики нельзя. Проверка возможностей выполняется в Worker.

Нужны актуальные Chrome или Firefox либо Safari/iOS 16.4. Клиент проверяет
совместимость при загрузке и называет причину отказа. В Lockdown Mode WebAssembly
отключён: клиент не запускается. В приватном режиме нет постоянного приватного
хранилища (OPFS): профиль создать нельзя, откройте обычное окно.

<a id="hosting-status"></a>
### Состояние размещения

18 ноября публичный `app/browser-config.json` на GitHub Pages вернул HTTP 404.
В репозитории отсутствовала переменная `GIPNY_BROWSER_CONFIG_JSON`. В свежем
профиле доступны создание личности и чтение локальной истории. Адрес рабочего
публичного моста для этого развёртывания пока не подтверждён.

В beta при восстановлении интернета открытый профиль повторно запрашивает
конфигурацию, если сеть не была настроена или запуск завершился ошибкой.
Если интернет вернулся ещё на экране входа, клиент выполняет эту проверку
сразу после создания или разблокировки профиля.
Когда конфигурация появляется, клиент запускает I2P в том же профиле.
Успешная проверка прямого релея принятого контакта позволяет раньше повторить
отправку ожидающих сообщений и квитанций. Если в это время уже идёт отправка,
очередь запоминает сигнал до её завершения. При ошибке та же зашифрованная копия сообщения
становится доступна для повтора через две секунды. Статус доставки меняется
после подтверждения получателя.

На `875a318` восстановление очереди через WebRTC прошло с квитанцией и одной
копией в истории. На `b53459c` WSS превысил 300 секунд ожидания сообщения после
перезапуска, а тест WebRTC завершился на двустороннем звуке до проверки очереди.
[Прогон 875a318](https://github.com/gluckdev/gipny-i2p/actions/runs/44312581501) · [Прогон b53459c](https://github.com/gluckdev/gipny-i2p/actions/runs/44322195198).

Чтобы выйти в сеть, оператор делает по порядку:

1. Хост с публичным IP и доменом.
2. TLS-сертификат для домена.
3. Rust-форвардер `browser-router/forwarder`: путь `/stream` для WSS или `/rtc` и TURN для WebRTC; [его README](../browser-router/forwarder/README.md).
4. Снапшот netDb, его SHA-256 и allowlist из `collect-netdb` / `prepare-netdb` в Actions.
5. Capability-токен моста.
6. Repository variable `GIPNY_BROWSER_CONFIG_JSON` по [схеме](#hosting-preparation) и повторный деплой Pages.
7. Проверка: `curl -I https://gluckdev.github.io/gipny-i2p/app/browser-config.json` возвращает 200, экран загрузки проходит шаг релея.

### Самостоятельное приложение и роль хостинга

Браузер запускает само приложение: профиль, криптография, история и I2P-роутер
находятся на устройстве. GitHub Pages раздаёт код и обновления, но не принимает
сообщения, ключи или контакты. Аккаунт GitHub и сопряжение с телефоном не нужны.
Это отличается от веб-интерфейса к мессенджеру, работающему на сервере.

После установки Service Worker оболочка открывается без хостинга. Публичная
сетевая конфигурация сохраняется отдельно (до 64 КиБ). При недоступности хостинга,
включая 403/404/410/429/5xx, или некорректной новой конфигурации используется
последняя проверенная копия. Исчезновение репозитория не удаляет локальную настройку.
Повреждённый локальный кэш отвергается; корректная новая конфигурация заменяет
старую. Авторизация моста проверяется отдельно, транспорт не меняется молча.
Последний проверенный архив netDb хранится отдельно (до 4 МиБ); SHA-256,
подписи, network ID и свежесть RouterInfo проверяются при каждом запуске.
Кроме того, роутер сохраняет то, что узнал о сети за сессию: каждые 15 минут
работы emissary отдаёт найденные RouterInfo, и полный набор (до 2048 записей,
сначала самые свежие) пишется в отдельный публичный кэш тем же форматом архива.
При старте сначала берётся опубликованный снапшот (его узлы есть в allowlist
моста), затем — сохранённые записи; если снапшот недоступен, достаточно
сохранённых. Каждая запись проверяется так же, как в снапшоте.

Для повторного сетевого запуска без хостинга нужны рабочий мост, доступная I2P
и ещё свежие записи узлов. RouterInfo старше 36 часов не принимается: наличие
кэша не продлевает этот срок. Если кэш устарел, нужен доступный источник свежего
bootstrap и соответствующая конфигурация. Сохранённый живой netDb продлевает
тёплый старт, пока клиент регулярно работает (узлы переиздают свои записи), но
не заменяет первый bootstrap. Первый запуск, обновление приложения и доверие к загружаемому коду
остаются связаны с выбранным хостингом; обещание «вообще никаких зависимостей»
было бы неверным. Проверка запуска и переписки с остановленным сервером страницы
добавлена в CI; результат для новых исходников ещё не получен.

Поддерживаются личные сообщения, групповая рассылка и передача файлов частями
до 2 ГиБ. Вложения хранятся отдельно в зашифрованном OPFS, в пределах доступной
квоты сайта с резервом 64 МиБ. Профиль без байтов вложений ограничен 16 МиБ.
Предпросмотр и очистка метаданных в памяти ограничены 32 МиБ; ограничения и
подтверждения описаны в [передаче файлов](file-transfer.md). Нативная консоль,
DHT-узел, duress и аппаратная привязка ОС не перенесены.
WebRTC DataChannel — транспорт данных, не функция звонков.

### Одноразовые сообщения

В личном чате с принятым контактом кнопка ① открывает отдельное поле для текста до 16 КиБ в UTF-8. Текст не попадает в черновики, историю или резервную копию. Клиент шифрует его для одноразового ключа получателя и передаёт конверт частями. При первом открытии клиент забирает ключ и конверт из очереди в памяти до расшифрования. Даже два одновременных запроса могут получить текст только один раз. Если экран заблокировался во время открытия, приложение закрывает окно и отбрасывает результат.

Если получатель офлайн, отправитель держит зашифрованные части и ключ только в оперативной памяти. Когда личный терминал получателя доступен, отправитель проверяет ключ владельца и шифрует письмо для одноразового ключа получателя. Выход из профиля или перезапуск процесса очищает очередь этого устройства. Блокировка экрана сохраняет очередь, пока процесс жив. Идентификатор, срок и состояние доставки сохраняются отдельно, чтобы исключить повторное открытие и показать уведомление об истечении срока. Получатель может самостоятельно скопировать уже показанный текст.

### Хранение и граница доверия

Ключи личности, prekeys, ключи destination и ratchet принадлежат Rust-ядру.
JavaScript получает данные для отображения; пароль и результат WebAuthn PRF
также проходят через JS при разблокировке. Код origin и расширения браузера
входят в границу доверия: Worker не защищает от компрометации всего браузера.

Профиль использует Web Lock и эксклюзивные OPFS sync handles. Два слота содержат
XChaCha20-Poly1305 записи; Argon2id использует 64 МиБ, 3 итерации, 1 поток.
Полный подтверждённый record выбирается до проверки пароля. Изменение пароля
и логическое удаление обновляют оба live-слота; прежние внешние копии остаются.
Сбой записи останавливает профиль. Подготовленный ciphertext и ratchet сохраняются
до отправки, входящее состояние — до ACK. Шифротекст, принятый релеем
получателя, удаляется из профиля и повторно не отправляется; повтор — это новое
шифрование (см. [передачу файлов](file-transfer.md)). Это отдельный от SQLCipher механизм.

Выход из профиля и уход со страницы останавливают Worker и транспорт. При скрытии или заморозке страницы клиент закрывает переписку защитным экраном. Если браузер сохранил процесс, повторная проверка владельца возвращает в ту же сессию. Если процесс выгружен, требуется обычная разблокировка профиля.
Service Worker кэширует оболочку, WASM и статические ресурсы, без профиля.
Отдельные кэши приложения хранят публичные настройки и проверенный netDb;
профили и байты переписки туда не попадают. Обновление ждёт закрытия старых окон.
Нет нативных `mlock`/`mprotect`, полного контроля JS GC и физического стирания SSD.
Диагностический журнал есть только в предварительных сборках. Он хранится в
памяти Worker и консоли DevTools. При блокировке копия сессии записывается в
IndexedDB `gipny-browser-log`: она не зашифрована и общая для всех профилей
сайта. В журнал попадают номера контактов, размеры и состояния, но не имена,
ключи, адреса I2P и текст. Очистка журнала удаляет и эту копию, как и удаление
профиля; стабильная сборка удаляет её при запуске. Не помещайте в отчёты
профиль, пароли, PRF, токены и текст переписки.

### Транспорт

`transport: "wss"` использует бинарный WSS до Rust-форвардера. `"webrtc"`
использует WSS для SDP и надёжный упорядоченный DataChannel для NTCP2-байтов.
RTCPeerConnection находится в Window; ограниченный MessagePort соединяет его
с Worker. Ключи мессенджера и NTCP2 не передаются мосту. Мост проверяет Host,
Origin, capability, подпись, сеть и свежесть RouterInfo и квоты. Цель — только
публичный IPv4 из allowlist на порту не ниже 1024 и не из списка сервисных портов
(`BLOCKED_NTCP2_PORTS` в `browser-router/src/ntcp2.rs`: исключения Java I2P плюс
прокси, СУБД, Redis, RDP/VNC, SIP, Bitcoin, Tor control и др.); тот же фильтр
применяют `prepare-netdb`, bootstrap и WASM-клиент. Один IP:port — до 4 подключений
подряд, затем одно в 15 с. Подпись RouterInfo сама по себе не делает адрес
допустимым: её может создать кто угодно. Неизвестный режим и отказ транспорта
не включают скрытый fallback.

По умолчанию WebRTC требует TURN и `iceTransportPolicy: "relay"`. Явный `all`
разрешает прямой ICE к транспортному мосту; прямого WebRTC между собеседниками нет.
ICE-кандидаты LAN/mDNS/IPv6 не используются текущей реализацией, поэтому браузеру
за NAT для `all` нужен STUN в `rtc_ice_servers` (или публичный адрес): оффер без
публичного кандидата отвергается. Answer моста содержит только его публичный
адрес. Исключение для частной сети разрешено только CI-конфигурацией со страницы
loopback.
Сигнализация WSS сама по себе может раскрывать IP мосту/CDN; relay-only ICE не
анонимизирует её. TURN знает IP клиента, время и объём. NTCP2 остаётся сквозным
между WASM-роутером и I2P-пиром. Нераспознаваемость трафика не доказана.

<a id="hosting-preparation"></a>
### Размещение

Сборка в GitHub Actions: `wasm-pack build --target web --release --locked` в
`browser-router`, затем `npm ci --prefix ui` и `npm run build:browser --prefix ui`.
Для подпапки задайте `VITE_BASE` (Pages: `/gipny-i2p/app/`). Публикуются файлы
`ui/browser-dist/`; стартовая страница — `browser.html`.

Хостинг: HTTPS, правильный MIME `.wasm`, CSP из HTML, отсутствие сторонних
скриптов; по возможности `nosniff`, `no-referrer`, `frame-ancestors 'none'`.
Не изменяйте политику хранения и сеть через сторонний Service Worker.
Рядом разместите некэшируемый `browser-config.json`:

```json
{
  "transport": "webrtc",
  "snapshot_url": "https://chat.example.org/snapshot.tar.gz",
  "snapshot_sha256": "<64 hex characters>",
  "forwarder_url": "wss://bridge.example.org/rtc",
  "capability": "<43 base64url characters>",
  "rtc_ice_policy": "relay",
  "rtc_ice_servers": [{
    "urls": ["turns:turn.example.org:5349?transport=tcp"],
    "username": "<short-lived username>",
    "credential": "<short-lived TURN credential>"
  }]
}
```

Для WSS задайте `transport: "wss"`, путь `/stream` и уберите `rtc_*`.
Не размещайте TURN provider API key в этом файле: браузеру нужны временные
клиентские credentials. Файл публичный; capability не является паролем профиля
или защитой от клиента, уже знающего токен. Нужны серверные лимиты и политика доступа.
TURN-логин из этого файла фактически долговечен и известен всем, поэтому TURN
должен ретранслировать только к мосту: в coturn `allowed-peer-ip=<IP моста>` и
`denied-peer-ip` для всего остального (как минимум loopback, частных, CGNAT и
link-local диапазонов IPv4/IPv6), `no-multicast-peers`, `no-tcp-relay`; при частом
обновлении конфигурации — TURN REST (`use-auth-secret`) с коротким сроком.
Иначе это открытый relay. Прокси перед мостом должен пропускать WebSocket Ping
(мост шлёт его каждые 20 с); для `"webrtc"` мосту нужен `--rtc-public-ip` или
публичный `--rtc-bind` и открытый диапазон эфемерных UDP-портов.
Pages берёт эту публичную конфигурацию из repository variable
`GIPNY_BROWSER_CONFIG_JSON`; отсутствие переменной оставляет клиент offline
(так на 2026-11-08, см. [состояние размещения](#hosting-status)).
`scripts/prepare-browser-config.py` проверяет данные без вывода их значений.

Снимок и allowlist формируют `collect-netdb` / `prepare-netdb` в GitHub Actions.
Обновляйте согласованно snapshot, SHA256 и allowlist. Для постоянно доступного
сервиса нужен оператор свежих данных. [Rust-мост и TLS/CDN](../browser-router/forwarder/README.md).
Публичный мост, TURN, сертификаты и домен этой правкой не развёртываются.

<a id="validation-and-artifacts"></a>
### Проверки

`Browser router WASM` проверяет Rust-мост, WASM, границы UI, реальные NTCP2,
холодный старт, обмен двух независимых браузеров через WSS и через WebRTC,
вложения, PRF, блокировку, смену пароля и восстановление очереди/истории.
Прямой ICE на CI не доказывает работу TURN, CDN или телефона.
Перед живым NTCP2 job собирает свежий netDb роутером pinned i2pd (`collect-netdb`)
и подсаживает нативному пиру только записи моложе 36 ч (`tests/netdb_seed.py`);
reseed остаётся запасным путём, снапшот холодного старта делается из того же набора.
До этого (2026-11-06…08) пир стартовал от одного reseed с устаревшими записями,
и шаг рукопожатий падал на данных, а не на коде.
Нужны отдельные проверки Chrome/Firefox/Safari, NAT, запрета UDP, TURN/TLS,
сна, смены Wi-Fi/mobile, переполнения очередей и ухода со страницы.
Движок Safari проверяется в CI: job `wasm` открывает собранный клиент в
Playwright WebKit на Linux (`ui/tests/webkit-smoke.mjs`, артефакт
`browser-webkit-smoke`): страница отдаётся без `browser-config.json`, как на
Pages сегодня, создаётся профиль, чаты должны открыться с уведомлением о
хостинге, профиль должен остаться после перезагрузки. Нужен persistent-контекст:
в эфемерном WebKit не даёт OPFS. Системный WebKitGTK 2.52 этот путь не выполняет
(нет OPFS sync access handles). Настоящий Safari и телефон CI не заменяет.
[Прежние результаты](archive/browser-validation-2026-11-01.md) относятся к старым SHA.

## English

The web client is `ui/browser.html`: shared UI, a Rust WASM Dedicated Worker,
client-only emissary and encrypted OPFS storage. The local `gipny-web` gateway
and pairing flow have been removed. Pages builds this client at `/app/browser.html`;
a passing deployment is separate from source implementation.

### Use and storage

Open the HTTPS page, create a password-protected profile, wait for I2P and exchange
cards. Compare fingerprints through a trusted channel. Enroll a passkey only
when WebAuthn provides an actual PRF secret; retain the password for recovery.
PWA installation depends on the browser. After shell caching, saved history is
available offline. Clearing site data or storage eviction can remove the profile;
the [I2P-only profile move](profile-transfer.md) is not implemented yet. Native export support
does not imply that this capability exists in the WASM client.

### Browser requirements

Current Chrome or Firefox, or Safari/iOS 16.4. The client checks
compatibility at load time and states the reason when it refuses. Lockdown Mode
disables WebAssembly: the client does not start. Private browsing has no persistent
private storage (OPFS): no profile can be created; use a normal window.

### Hosting status

On November 18 the public Pages deployment returned HTTP 404 for
`app/browser-config.json`. A fresh profile could open offline and read saved
chats. The deployment still needs a verified public bridge.

When connectivity returns, the beta client checks for updated configuration if
the router was unconfigured or startup failed. If connectivity returns on the
sign-in screen, the client runs that check after profile creation or unlock.
A newly published configuration starts I2P in the open profile. A successful
probe of an accepted contact's direct relay can bring queued retries forward.
If a deposit is already in flight, the queue remembers the ping until that
attempt finishes. On failure the same ciphertext is eligible for retry after
two seconds. Delivery still requires the recipient's receipt.

On `875a318` WebRTC recovery produced a receipt and a single history entry.
On `b53459c` the WSS receiver timed out after 300 seconds waiting for the
queued message, and the WebRTC run stopped at the bidirectional audio check
before it could test recovery. [Run on 875a318](https://github.com/gluckdev/gipny-i2p/actions/runs/44312581501) · [Run on b53459c](https://github.com/gluckdev/gipny-i2p/actions/runs/44322195198).

To go live, an operator does, in order:

1. A host with a public IP and a domain.
2. A TLS certificate for the domain.
3. The Rust forwarder `browser-router/forwarder`: path `/stream` for WSS, or `/rtc` plus TURN for WebRTC; [its README](../browser-router/forwarder/README.md).
4. A netDb snapshot, its SHA-256 and the allowlist from `collect-netdb` / `prepare-netdb` in Actions.
5. The bridge capability token.
6. The repository variable `GIPNY_BROWSER_CONFIG_JSON` per the [schema](#hosting-preparation), then a Pages redeploy.
7. Verification: `curl -I https://gluckdev.github.io/gipny-i2p/app/browser-config.json` returns 200 and the boot screen passes the relay step.

### A standalone application and the hosting boundary

The browser runs the application itself: profile, cryptography, history and
I2P router are local. GitHub Pages serves code and updates without receiving
messages, keys or contacts. No GitHub account or phone pairing is needed.
This differs from a web interface controlling a messenger running on a server.

The installed Service Worker opens the shell without hosting. A separate cache
stores validated public network settings (64 KiB maximum), reused when hosting is
unavailable, including HTTP 403/404/410/429/5xx or an invalid replacement. Removing
the repository does not erase local settings. Corrupt local data is rejected;
valid new settings replace it. Bridge authorization is still checked separately,
without an implicit transport switch. Another cache holds the
last verified netDb archive (4 MiB maximum). Every read rechecks SHA-256, peer
signatures, network ID and RouterInfo freshness. The router also keeps what it
learned: every 15 minutes of uptime emissary hands over discovered RouterInfos,
and the full set (at most 2048, newest first) is written to a separate public
cache in the same archive format. Startup takes the published snapshot first
(its routers are in the bridge allowlist), then the saved records; without a
snapshot the saved records alone suffice. Each record gets the same checks.

Restarting online without the host requires a working bridge, I2P and still-fresh
peer records. RouterInfos older than 36 hours are rejected; caching does not
extend their lifetime. An expired cache requires fresh bootstrap data and its
matching configuration. The saved live netDb keeps restarts warm while the
client runs regularly (routers republish their records), but does not replace
the first bootstrap. First launch, updates
and trust in downloaded code still involve the chosen host. The CI scenario now
stops the application server, restarts both clients and checks delivery; no new
runtime result is available yet.

Direct/group messaging and multipart files up to 2 GiB are implemented. Encrypted
OPFS attachments use available origin quota with a 64 MiB reserve; profile
metadata remains limited to 16 MiB. Previews and in-memory metadata cleaning
are limited to 32 MiB; see [file transfer](file-transfer.md) for boundaries and
validation status. Native agent commands, DHT, duress and OS hardware binding
are not browser features. WebRTC transport is
not audio/video calling.

Rust owns identity, prekeys, destination keys and ratchets. JS receives display
plaintext and temporarily handles passwords/PRF output. The hosting origin,
extensions and browser remain trusted; a Worker is not an isolation boundary
against a compromised browser. OPFS uses Web Locks and exclusive sync handles,
two XChaCha20-Poly1305 slots and Argon2id (64 MiB, 3 iterations, 1 lane).
Credential changes and logical deletion rewrite both live slots, not external
backups. Write ambiguity stops the profile. Ciphertext/outgoing ratchet are
flushed before network send; incoming state is flushed before ACK. Ciphertext
accepted by the recipient's relay leaves the profile and is never sent again;
a retry is a new encryption (see [file transfer](file-transfer.md)).

Signing out and leaving the page stop the Worker and transport. Hiding or freezing the page covers the chat with a locked screen. Authentication resumes the same session if the browser kept the process alive. A discarded process requires a fresh vault unlock. The Service Worker
caches the shell, WASM and static assets, excluding profiles. Separate app caches
hold public configuration and verified netDb; they never hold conversation data.
Updates wait for old windows to close. Browser memory lacks native
mlock/mprotect and physical-erasure guarantees. The diagnostic log exists in
prerelease builds only: Worker memory and the DevTools console, plus an
unencrypted IndexedDB copy (`gipny-browser-log`) of the last session written on
lock and shared by all profiles of the site. Lines name contact numbers, sizes
and states, never names, keys, I2P addresses or text. Clearing the log or
deleting a profile removes the copy; stable builds delete it at startup. Exclude
private data and tokens from evidence.

### Transport and hosting

`transport: "wss"` carries binary NTCP2 over WSS. `"webrtc"` uses authenticated
WSS SDP signaling and a reliable ordered DataChannel. RTCPeerConnection lives
in the Window; a bounded MessagePort connects it to the Worker. The Rust bridge
rechecks signed, fresh RouterInfos and allows only allowlisted public IPv4 NTCP2
targets on ports of at least 1024 outside `BLOCKED_NTCP2_PORTS`
(`browser-router/src/ntcp2.rs`: Java I2P exclusions plus proxies, databases,
Redis, RDP/VNC, SIP, Bitcoin, Tor control and similar), at most 4 dials in a burst
then one per 15 seconds per IP:port, with Host/Origin/capability, time, byte and
connection limits. A valid signature alone proves nothing about a target, since
anyone can sign a RouterInfo. `prepare-netdb`, bootstrap and the WASM dialer apply
the same port policy. There is no implicit transport fallback.

WebRTC defaults to TURN with `iceTransportPolicy: "relay"`. Explicit `all`
allows direct ICE to the bridge, never to a chat contact. This implementation
filters LAN/mDNS/IPv6 candidates, so a browser behind NAT needs STUN in
`rtc_ice_servers` (or a public address) for `all`; offers without a public
candidate are rejected. The bridge answer advertises only its public address.
Private fixtures require a loopback page.
Relay-only ICE does not hide signaling metadata from the WSS endpoint/CDN.
TURN observes client IP, timing and volume. NTCP2 remains encrypted end to end
between the browser router and I2P peer; traffic indistinguishability is unproven.

Local builds/tests are authorized; final verification runs in GitHub Actions: generate WASM with `wasm-pack build --target web
--release --locked`, then `npm ci --prefix ui` and `npm run build:browser --prefix ui`.
Set `VITE_BASE` for a subpath. Serve `ui/browser-dist/` over HTTPS with the
provided CSP and WASM MIME type; start at `browser.html`.
Use the JSON schema above beside that page with `Cache-Control: no-store`.
For WSS choose `/stream` and omit `rtc_*`; WebRTC uses `/rtc`. TURN credentials
must be short-lived client credentials, never provider API keys. The config is
public; its capability is neither a profile password nor protection against
an informed malicious client. TURN credentials published here are effectively
long-lived and world-readable, so TURN must relay only to the bridge: in coturn,
`allowed-peer-ip=<bridge IP>` and `denied-peer-ip` for everything else (at least
loopback, private, CGNAT and link-local IPv4/IPv6), `no-multicast-peers`,
`no-tcp-relay`; with frequently refreshed configuration prefer TURN REST
(`use-auth-secret`) with short lifetimes. Otherwise it is an open relay. Proxies
in front of the bridge must pass WebSocket pings (sent every 20 seconds); WebRTC
requires `--rtc-public-ip` or a public `--rtc-bind` and an open ephemeral UDP range. Pages reads `GIPNY_BROWSER_CONFIG_JSON`; without
it, the client is offline. The preparation script validates without printing values.

Generate fresh snapshots and allowlists with CI `collect-netdb` / `prepare-netdb`;
publish matching SHA256/configuration. See [the Rust bridge guide](../browser-router/forwarder/README.md)
for TLS/CDN and operating limits. This source change does not deploy a public
bridge, TURN service, domain or certificate.

### Validation

The workflow covers bridge boundaries, WASM/UI, NTCP2, cold starts, two-browser
WSS/WebRTC messaging, attachments, PRF, locking and profile/outbox recovery.
CI direct ICE is not TURN/CDN/physical-mobile validation. Before the live NTCP2
step the job collects a fresh netDb with the pinned router (`collect-netdb`) and
seeds the native peer only with records younger than 36 h (`tests/netdb_seed.py`);
reseed stays the fallback and the cold-start snapshot is made from the same set.
Until then (2026-11-06…08) the peer started from one stale reseed and the handshake
step failed on data, not code. Chrome/Firefox/Safari,
NAT, blocked UDP, TURN/TLS, sleep, network changes, bounded queues and lifecycle
need explicit results. The Safari engine is checked in CI: the `wasm` job opens
the built client in Playwright WebKit on Linux (`ui/tests/webkit-smoke.mjs`,
artifact `browser-webkit-smoke`): the page is served without `browser-config.json`
as Pages serves it today, a profile is created, the chats must open with the
hosting notice and the profile must survive a reload. It needs a persistent
context; WebKit has no OPFS in an ephemeral one. The system WebKitGTK 2.52 does
not run this path (no OPFS sync access handles). CI does not replace real Safari
or a phone.
[Historical evidence](archive/browser-validation-2026-11-01.md)
does not validate these new changes.
