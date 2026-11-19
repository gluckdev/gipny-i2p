# Старт, prewarm и производительность native/browser

Состояние собственного кода: 2026-11-16. Документ разделяет уже работающие
механизмы, исправления этой серии и предложения. Новых результатов скорости
или времени запуска здесь пока нет. Методика native throughput находится в
[`performance-anonymity.md`](performance-anonymity.md).

Цель — уменьшить ожидание при первом запуске, повторном открытии и первой
отправке, затем увеличить полезную пропускную способность для будущих voice
и screen sharing. «Максимальный prewarm» означает раннее выполнение полезной
работы в ограниченном бюджете. Неограниченное открытие соединений, прогрев
всех контактов до unlock и обход проверки свежести не следуют из этой цели.
Стоимость в CPU, RAM, трафике и наблюдаемое время активности измеряются вместе
с задержкой. Сохранение защитных настроек не является доказательством
неизменности анонимности при любом наблюдателе.

## Разные движки и исходная политика

| Свойство | Native | Browser |
| --- | --- | --- |
| Движок | C++ i2pd через собственный Rust/FFI слой `i2p-embed` | Rust Emissary, собственный WASM runtime и JS Worker |
| Сеть | Native NTCP2 и SSU2; встроенный router | Client-only NTCP2 через настроенный WSS/WebRTC forwarder; нет native UDP/SSU2 или local SAM listener |
| Время жизни router | Один `OnceLock` на процесс; первый каталог/settings остаются до выхода | `TaskScope` и Worker; lock/close прекращают текущий runtime |
| Клиентские туннели | Обычно inbound/outbound 3/3; benchmark проверяет оба клиента и оба relay | Собственный adapter явно создаёт 3/3; исторический baseline наследовал default 2/2 |
| Exploratory pool | По 2 туннеля длиной 2 в каждом направлении | По 3 туннеля длиной 2 в каждом направлении |
| Router capacity | Настройка TransitProfile; default Balanced: O, share 50%, transit 256 | Client-only; NTCP2 max 16; профиль native bandwidth сюда не переносится |
| Состояние профиля | Зашифрованная БД; router public cache отдельно по назначению | Зашифрованный профиль OPFS; private state в Worker/WASM, public caches отдельно |

Источники: [`embedded.rs`](../libcore/src/embedded.rs),
[`router.rs`](../libcore/src/router.rs),
[`browser router configuration`](../browser-router/src/router.rs),
[`browser network`](../browser-router/src/network.rs),
собственный [`Emissary adapter patch`](../browser-router/patches/client-only-ntcp2.patch).
Upstream-исходники изучались read-only; большие чужие файлы не реорганизуются.

Исходная разница browser 2/2 и native 3/3 обнаружена в **нашем добавленном модуле**
`ClientHandle::create(identity)` внутри `client-only-ntcp2.patch`: его
`TunnelPoolConfig` наследовал upstream default 2/2. Это собственный adapter,
который теперь явно задаёт 3/3, сохраняя чужие defaults. UI получает hops из
Rust `application_tunnel_hops()`, а не отдельной константы. Source verification,
WASM release build и UI builds прошли; новая live проверка ещё ожидается.
`Config.exploratory` меняет другой pool и сам по себе не решает
эту задачу. Исторический browser baseline 2/2 нельзя смешивать с будущим 3/3
при оценке скорости. Native sweep также не заменяет browser измерение.
Exploratory readiness отдельно не доказывает готовность клиентского Destination.

Browser forwarder — отдельная точка наблюдения транспорта: он видит подключение
браузера и выбранные им публичные router endpoints. Шифрование внутри I2P не
делает native и browser топологии одинаковыми. Поэтому даже одинаковая
длина клиентских туннелей не доказывает одинаковую наблюдаемость; prefetch,
число bridge connections и reconnect bursts также входят в измерительный план.

## Что уже работает

| Механизм | Native: текущее поведение | Browser: текущее поведение | Цена и границы |
| --- | --- | --- | --- |
| Начало до unlock | UI начинает prewarm выбранного существующего профиля; опрашивает статус раз в 2 с | Worker/WASM/config загружаются параллельно UI; public-only prewarm проверяет bootstrap без router/profile | Native уже создаёт сетевую активность до пароля; browser public prefetch не открывает bridge connections или identity |
| Публичные настройки | `router.hint` содержит transit/UPnP, без ключей/контактов | Deployment config network-first, deadline 2.5 с, затем validated CacheStorage fallback; предел 64 KiB, optional cache operation ограничена 250 мс | Отмена не ждёт зависший CacheStorage; public cache не заменяет авторизацию bridge и проверку RouterInfo |
| Код приложения | Исполняемые файлы локальны | Service Worker кеширует shell, JS и WASM; UI/worker/config стартуют параллельно | Profile open/network operations ждут config barrier; Worker API проверяется в его собственном realm |
| Bootstrap | Packaged netDb распаковывается при числе RouterInfo файлов меньше 90; Android seed встроен | Snapshot cache по digest и отдельный learned public netDb cache; RI проверяются заново | Native порог 90 считает файлы, сам по себе не доказывает их свежесть; валидность далее решает router |
| Проверка bootstrap | Актуальные reseed certificates укладываются при старте, stale certificates удаляются; `reseed.verify=true` | Digest, signature, network/freshness, public address, bounds проверяются перед использованием | Кеш не повод обходить проверку или продлевать RI freshness |
| Тёплый router | Public netDb, profiles/addressbook сохраняются на диск; текущий router живёт до выхода | Learned RI сохраняются в публичном архиве, отдельно от snapshot | Browser получает новые RI через upstream backup примерно раз в 15 мин; простое сохранение текущего cache при close не извлечёт ещё не переданные RI |
| Client readiness | Проверка обоих направлений и native IsReady, poll 200 мс, deadline 600 с; peers проверяются 5 с, offline threshold 30 с | `ClientSession::create` должен завершиться; startup deadline 300 с; gauges служат промежуточным статусом | Готовность не подменяется таймером, количеством скачанных peers или красивым процентом |
| Личный relay | После единственного router bootstrap строится параллельно client Destination; owner назначается после unlock | Owner-only mailbox начинается после готовности ClientSession | Native остаются ровно 1 client + 1 relay Destination; более ранняя публичная relay activity требует измерения наблюдаемости |
| Контакты | После unlock прогреваются до 5 последних подходящих контактов, до готовности собственного relay; новый адрес также прогревается | После готовности сети до двух параллельных проверок. Первый проход охватывает все подходящие контакты. Для недоступных контактов ещё четыре прохода с паузами 10, 20, 40 и 60 с | Blocked/incoming requests исключены; ранние probes меняют timing, поэтому «никакой новой наблюдаемости» не доказано |
| Повторное использование stream | Persistent relay connections; дополнительные file/collect lanes сохраняются | Authenticated idle pool до 8 total/4 на peer, idle 15 с | Больше idle streams занимает память и transport slots; не равнозначно большему goodput |
| Файлы | Части 192 KiB, общее окно 16; default до 4 lanes; измерительный override 1…8 | Отдельные JS outbox/file workers поверх browser streams | Native 1…8 override не действует на browser и не задаёт будущий media transport |
| Ограничения browser streams | Не применяются к native | До 16 app streams всего, accept<=8; connect 90 с/read 120 с/write 60 с | Считать активные/занятые slots отдельно от NTCP2 sessions; увеличение одного cap может упереться в другой |
| Network change/retry | Сигнал router о смене сети, отдельные reconnect paths | После network failure retry 30 с; online может запустить reconnect | Resume/сеть могли сделать раннюю readiness устаревшей |
| Lock/смена профиля | Подготовленные Destinations должны прекращаться и освобождаться; router остаётся общим процессу | Lock завершает Worker, emergency deadline 2 с; pagehide/freeze прекращают Worker/RTC и очищают DOM/files; BFCache resume требует locked reload | Ни private keys, ни decrypted contacts не переносятся в public cache; скрытая вкладка сама по себе не гарантирует background progress |

Основные точки кода:
[`native lifecycle`](../core/src/lifecycle.rs),
[`native prewarm coordinator`](../core/src/lifecycle/prewarm.rs),
[`native relay warming`](../core/src/core/relay.rs),
[`native netDb seeding`](../libcore/src/router.rs),
[`browser entry`](../ui/src/browser-main.ts),
[`browser client`](../ui/src/browser-core/client.ts),
[`browser backend`](../ui/src/browser-core/backend.ts),
[`browser configuration cache`](../ui/src/browser-core/networking/config.ts),
[`browser relay pool`](../ui/src/browser-core/networking/relay.ts),
[`bootstrap validation`](../browser-router/src/bootstrap.rs),
[`bootstrap JS cache`](../browser-router/src/bootstrap-cache.js).

Browser bootstrap ограничен 4 MiB compressed, 16 MiB tar, 2048 принятыми RI,
4096 archive entries; нужно минимум 16 usable RI, возраст RI не более 36 ч.
Теперь published cache и learned cache проверяются параллельно; если хватает
usable RI, HTTP download не выполняется. Только при недостаточном local cache
запрашивается snapshot с прежним deadline 45 с. Вся public prewarm job, включая
CacheStorage, ограничена 60 с. Оба JS cache readers ограничивают streaming read
пределом 4 MiB до создания итогового массива. Signature/digest и freshness
проверки не отменены.

## Изменения этой серии и предложения

| Изменение | Статус | Что проверяет выигрыш | Privacy/resource граница |
| --- | --- | --- | --- |
| Persistent native file cursor между короткими batches | Реализовано, выигрыш ещё не измерен | Per-lane распределение, goodput и p95 echo под нагрузкой | Не увеличивает окно или размер padding; сохраняет последовательную фиксацию ratchet |
| Native controlled cap 1…8 и JSON transport counters | Реализовано в benchmark | Один pinned build, последовательный sweep 32 MiB, повтор baseline | Default 4 сохранён; open peak не означает использование всех streams |
| Standalone relay client hops 2→3, readiness sidecar | Исторический baseline 16 ноября. Текущий harness проверяет длину туннелей личных терминалов через runtime API | Destination binding и inbound/outbound policy каждого relay | Устраняет несовпадение baseline; sidecar описывает принятую конфигурацию, не трассу пакета |
| Browser application hops 2→3 в собственном adapter patch | Реализовано; source verification и WASM/UI builds прошли, live CI ожидается | Explicit config assertions и live browser messaging на новом baseline | Чужой `TunnelPoolConfig::default` и exploratory pool не меняются; ускорение от сокращения пути не допускается |
| Native finished-prewarm cancellation | Исправлено: completed outer result больше не теряет child relay JoinHandle; child abort+await | Детерминированные ownership tests; profile-switch smoke test | Сначала корректное завершение старых задач, затем новый prewarm |
| Awaitable shutdown подготовленного native relay | Реализовано; прямой libcore regression прошёл | После completed builder явно abort+await его accept/gc tasks | Вложенные client tasks отменяются через JoinSet Drop; глобальный router живёт, мгновенное отсутствие всех packets не заявлено |
| Native router/client/relay build overlap | Реализовано; lifecycle 20/20, Core all-targets и libcore 214/214 прошли | Время отдельно router ready/client ready/relay ready/first send | Один sync router bootstrap, затем existing bounded pools под единым owner/profile/settings/generation |
| Native readiness recheck при unlock; cancel при profile delete/duress wipe | Реализовано и проверено в той же lifecycle серии | Долгий экран пароля, смена сети, sleep/resume, wipe во время build | Delete завершает builders до erase; duress завершает их после синхронного wipe, до возврата IPC |
| Public config + JS/WASM download/compile параллельно | Реализовано; JS startup 12/12 и actual worker 4/4 smoke прошли | Cold shell/config/WASM timings, UI responsiveness | Public-only; не открывать профиль/контакты до авторизации; ограничить работу hidden tabs |
| Browser verified local-cache-first bootstrap | Реализовано; targeted Rust bootstrap 2/2 прошли | Warm offline и stale snapshot + fresh learned cache; убрать лишнее ожидание 45 с | Проверки digest/signature/freshness остаются; policy generation и network config согласованы при start |
| Browser public snapshot prefetch до unlock | Реализовано; WASM lib 8/8 с deadline regression прошли | Время unlock→ready отдельно от затрат до unlock | Abortable/idempotent, whole-job deadline 60 с; без router/ClientIdentity, не открывать peer relays до unlock |
| Browser bounded learned-cache reader | Реализовано; oversized streaming reader проверен | Oversize cache и memory peak tests | Прерывать чтение сверх 4 MiB, не материализовать неограниченное тело |
| Browser раннее сохранение learned RI | Требуется отдельный adapter/API дизайн | Короткий сеанс<15 мин, close/reopen offline | В текущем public API нет получения всех ещё не переданных RI; пустой flush не решает задачу |
| Browser prepared neutral router | Отложено до ownership/API дизайна | Lock-during-start, profile-switch, TaskScope exclusivity, actual client hops | Public transport можно обсуждать отдельно от private identity; нельзя сохранять identity-bearing runtime после lock |
| Общий queue/byte budget и приоритет коротких сообщений | Предложение после измерений | File goodput одновременно с p95/p99, retransmits и queue wait | Ёмкость резервируется до encrypt/ratchet commit; ciphertext нельзя дублировать гонкой отмен |

Таблица не утверждает, что все предложения ускорят запуск. Сначала выбирается
одна причина задержки, сохраняется baseline и измеряется её изменение. Нельзя
одновременно увеличить tunnel quantity, bandwidth class, file window и streams,
а затем приписать результат одному параметру. Политику отбора peers и длину
туннелей ради скорости не меняем. Новая browser hop parity требует отдельной
проверки и отдельного baseline.

Native foreground operations (unlock/create/lock/delete) сериализованы отдельным
guard. Фоновый prewarm не удерживает этот guard: unlock может получить его и
дождаться освобождения prewarm slot, а фоновая замена после cleanup уступает
управление foreground. Конкурирующие foreground operations получают busy,
чтобы destructive command не ждал в очереди многоминутный boot со старым
состоянием UI. Regression test immediate unlock во время fire-and-forget
replacement prewarm входит в прошедшие 20 lifecycle tests. Отдельный libcore
тест awaitable relay shutdown также прошёл. Эти проверки подтверждают ownership
и поведение отмены; они не являются замером задержки старта живой I2P-сети.

Foreground ownership хранится через atomic compare/exchange и RAII release;
prewarm читает его без захвата foreground mutex. Native FFI старт выполняется
синхронно внутри принадлежащей prewarm async task, без detached `spawn_blocking`.
Отмена может ждать завершения текущего FFI-вызова. Router initialization теперь
сериализован до `Router::start` side effects. Coordinator владеет обоими builders
с момента запуска; relay ждёт signal единственного router bootstrap, затем
client/relay pool setup перекрываются. При отмене оба builders сначала получают
abort, затем join; ошибка клиента отменяет relay. Handoff проверяет профиль,
settings и generation; relay остаётся unclaimed до unlock. Искусственные 400 мс
ожидания именно в prewarm teardown/failed-builder fallback удалены; остальные
Core lock/unlock delays этим изменением не затронуты.

При reuse готовность client Destination проверяется непосредственно перед
передачей в boot: оба направления и `is_ready`. Отрицательный результат
завершает child builder, освобождает подготовленную пару и запускает обычный
startup с прежним deadline 600 с. Отдельного дополнительного ожидания readiness
поверх этого fallback нет. Это не непрерывная гарантия состояния сети после
проверки. `prewarm_status` также сверяет live направления/IsReady: stale Ready
отображается как building, без разрушения пары только из-за опроса UI.

Duress/attempt-limit erasure вызывается синхронно внутри `Vault::unlock`;
завершение prewarm выполняется после `Wiped` либо ошибки с уже удалённым vault,
перед ответом вызывающей стороне. Правка не обещает остановить builders до
начала такого wipe и не останавливает process-global router. Его публичные
cache writes могут продолжаться до выхода процесса. Обычная ошибка пароля
с существующим vault сохраняет подготовленные Destinations.

Browser diagnostics различают public bootstrap `loading/ready/failed` и
application readiness: legacy indicator выдаёт только building/off, а не
готовые туннели после загрузки seeds. При unlock bootstrap cache проверяется
заново; сохранённый in-memory receipt не обходит freshness. При достаточном
local cache background HTTP refresh не запускается. Если CacheStorage недоступен,
startup после public prewarm может скачать snapshot повторно. `page_startup_ms`
измеряет milestones от создания/reset клиента, `profile_startup_ms` — от начала
открытия профиля, включая последующие retry; это не независимые длительности
стадий. Ни одно из этих изменений пока не имеет подтверждённого live ускорения.

Диагностика WASM теперь проверяет API в настоящем Worker realm. Отсутствие
`Window.WebAssembly` само по себе не доказывает отсутствие API в Worker.
Ошибки `WASM_API_UNAVAILABLE` и `WASM_INIT_FAILED` сохраняют исходную причину,
не приписывая её настройке телефона по User-Agent. Проверены normal startup,
отсутствие API только в Window, отсутствие API в Worker и реальная
`CompileError`; pending RPC завершаются без unhandled rejection. Это улучшение
диагностики, а не доказательство исправления конкретного realme или размещения
network service. Состояние проверенного Pages URL и история работающего
browser-owned path описаны в [`browser-network-deployment.md`](browser-network-deployment.md).

Короткие transient notices показываются одной полупрозрачной карточкой по
центру доступной области, с учётом клавиатуры и safe areas. Обычная карточка
не перехватывает clicks и focus; modal confirmation расположена выше неё.
Время показа зависит от длины текста, предыдущий таймер отменяется, lock очищает
таймер. Persistent form errors и network status остаются в своих views.
Очень длинный текст получает ограниченную прокручиваемую область без ellipsis
или удаления текста. Только фактически переполненная карточка принимает
pointer events, имеет `tabindex=0` и `touch-action: pan-y`; automatic focus не
вызывается. `ResizeObserver` пересчитывает overflow при изменении keyboard/
viewport, modal controls остаются выше карточки. Время показа по-прежнему
составляет `max(5500, 2000 + 55 × codepoints)` мс. Проверены обычная карточка и
текст длиннее 1000 символов при viewport 360×780, доступной высоте 430 px и
открытой клавиатуре: последняя строка достижима прокруткой, обычные clicks и
modal controls работают. Весь произвольный текст одновременно на экране не
обещается.

В [`browser-router.yml`](../.github/workflows/browser-router.yml) после browser
build добавлены `node tests/worker-startup.test.mjs` и
`node tests/toast-layout.test.mjs` из каталога `ui`; они используют настоящий
Chromium. Второй сохраняет screenshots `browser-toast-keyboard.png` и
`browser-toast-keyboard-long.png` в artifact `browser-frontend-regressions`.
Это отдельные регрессии к прежним WebKit smoke,
WASM tests и live messaging gates. Локальная layout matrix прошла 316/316;
новые шаги ещё должны пройти на финальном CI SHA.

## Измерения startup и готовности будущего media

Для native и browser нужны независимые результаты. В каждом сохранить exact
application SHA, router revision/patches, OS/browser/device, network topology,
защитную конфигурацию, наличие caches и status/error. Определения:

- **Cold:** новый процесс/Worker и пустой public cache в отдельном тестовом
  каталоге/профиле; отдельно отметить наличие bundled snapshot.
- **Warm disk/cache:** новый процесс/Worker с ранее проверенными публичными
  peers и тем же тестовым профилем; это не прежние готовые туннели.
- **Warm live:** router/prewarm уже работает; отдельно фиксировать возраст
  готовности, network change и длительность ожидания пароля.
- **Resume:** sleep/background/freeze/pagehide различаются; frozen browser
  Worker не считать продолжающим работу без измерения.

Для каждого сценария записывать монотонные интервалы: shell visible → config
available → WASM initialized/router started → verified bootstrap available →
client inbound/outbound ready → личный relay ready → первый authenticated peer
stream → первый end-to-end receipt. Unlock wait хранить отдельно, иначе время
ввода пароля ошибочно становится сетевым ускорением. Измерять CPU/RSS или WASM
memory, bytes downloaded, cache hit/freshness, число открытых streams и failures.
Собирать несколько повторов и p50/p95, а не только лучший запуск.

Существующие точки CI можно расширять, сохраняя обычные проверки:

| Проверка | Уже доступный запуск внутри подготовленного CI | Чего она пока не доказывает |
| --- | --- | --- |
| Native file + interactive echo | [`i2p-throughput.yml`](../.github/workflows/i2p-throughput.yml), либо manual `e2e-i2pd.yml` с `throughput=true` | Startup percentile, независимые устройства, реальные media deadlines |
| Browser cold bootstrap/exploratory receipt | `python3 browser-router/tests/run_bootstrap_ci.py` | Готовность application Destination и быстрый unlock; CPU-throttled desktop Chrome не является настоящим телефоном |
| Browser messaging/recovery WSS | `python3 browser-router/tests/run_messenger_ci.py` | Количественный optimum lanes или отдельный полный cold/warm timing breakdown |
| Browser messaging/recovery WebRTC | Тот же messenger script с `GIPNY_BROWSER_TRANSPORT=webrtc` | Выигрыш media только от смены browser transport; это всё ещё I2P messaging сценарий |

Эти команды требуют окружения и сборок из
[`browser-router.yml`](../.github/workflows/browser-router.yml), включая
native netDb source, compiled WASM, forwarder, UI и Chrome. Они не являются
самодостаточным локальным запуском без подготовки. Публичные receipts и
ограниченные diagnostics сохраняются; private Chrome/OPFS directories,
capabilities, ключи и CDP/network dumps не должны входить в artifacts.

Обязательные сценарии проверки новых prewarm изменений: lock во время bootstrap,
смена профиля во время client/relay build, failed setup, завершённый outer task с
живым child, изменившаяся сеть после Ready, stale snapshot со свежим learned
cache, недоступный config host, отказ CacheStorage/quota и oversized cache.
После завершения не должно оставаться забытых builders, потоков или private
данных другого профиля.

Защищённый native throughput workflow уже измеряет verified file goodput и
application echo RTT под нагрузкой, но не настоящий аудио-/видеопоток. До
вывода о пригодности voice/screensharing нужны отдельные bounded media-like
сценарии с frame deadlines, jitter, loss, late-frame discard и несколькими
получателями, на независимых устройствах и сетях. Не увеличивать durable outbox
без границ для устаревших media frames. Число FPS/битрейт/латентность пока не
обещаны; их определят фактические результаты и отдельный протокол доставки.

Выбор concurrency/controller опирается на Pareto-сравнение goodput, p95/p99 RTT,
потерь и CPU/RSS. Почти одинаковая скорость файла при меньшей задержке сообщений
может оправдать дополнительный поток для интерактивного сценария. Один порог
прироста bytes/s этого не отражает. RTT-jitter двухсекундных text probes —
отдельная метрика: она не является RTP jitter и не подтверждает качество звонка.

## Cromite 153.0.8010.37: воспроизведение запуска

2026-11-16 владелец уточнил версию на Honor и подтвердил, что ранее браузерная
версия там работала. Это не следует заменять предположением о настройках:
точный открытый URL и состояние существующего профиля ещё не установлены.

Для отдельной проверки скачан официальный Linux выпуск
[Cromite153.0.8010.37](https://github.com/uazo/cromite/releases/tag/v153.0.8010.37-11507ac1061b5ea227806f5e84db5a57df6ccf6a),
`chrome-lin64.tar.gz`, SHA256
`6fe67a4a75a3a5013cc470a3061606c44dc148531c42d8c56317ef0c7f8340cf`.
Hash сверен с metadata выпуска; запуск использовал временные профили, без
профиля телефона. Android-устройств по ADB на момент проверки не было.

| Контрольное условие | Window / Worker | Результат |
| --- | --- | --- |
| Чистый профиль, штатная политика Cromite | WebAssembly undefined в обоих | Опубликованный Pages показывает прежнее общее сообщение; новая локальная сборка — WASM_API_UNAVAILABLE |
| Новый временный профиль, разрешение JIT только для проверяемого origin | WebAssembly object, минимальная compile успешна в обоих | Старая Pages и новая локальная сборка доходят до создания профиля; pageerror отсутствуют |

Разрешение во второй строке задавалось только контрольным временным профилям;
приложение и профиль владельца его не меняют. В
[патче именно этого выпуска](https://github.com/uazo/cromite/blob/11507ac1061b5ea227806f5e84db5a57df6ccf6a/build/patches/JIT-site-settings.patch)
default JAVASCRIPT_JIT равен BLOCK, а UI связывает разрешение с WebAssembly.
Это объясняет чистое воспроизведение, но не устанавливает причину изменения
на Honor. Проверка Linux не заменяет запуск на Android и не проверяет сетевую
доставку: у проверенного Pages отсутствует public network configuration.

По коду browser service worker хранит согласованный набор HTML/JS/WASM в
версионированном shell cache; обновление ждёт закрытия старых окон и не
перезагружает unlocked профиль. Этот механизм не менялся в текущей серии.
Старый shell может сохранять прежнюю формулировку ошибки. Причину отсутствия
самого WebAssembly API из этого установить нельзя. Не удалять OPFS/профили
или все данные сайта ради обновления shell: это разные хранилища.

Остаточный диагностический шаг — сравнить тот же URL, прежний рабочий origin
и реальные site permissions на Honor. Перенос Window-проверки в Worker
исправляет неверную проверку другого realm; он не включает API, отсутствующий
в обоих. Runtime остаётся browser-owned JS/WASM; pure-JS fallback в проекте нет.
