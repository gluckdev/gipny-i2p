# tests/e2e-harness — сквозной тест доставки по живой i2p

Запускает два headless-клиента на роутере i2p внутри своего процесса (у каждого своя destination), связывает их, гоняет сообщения с вложением и эхом и печатает задержки. Это единственная проверка того, что сообщения реально доходят. Запускается только в CI (`.github/workflows/e2e-i2pd.yml`).

## Что здесь

| Файл | За что отвечает |
|---|---|
| `src/main.rs` | `main` выбирает сценарий по переменным окружения. `start_bot` — клиент на `SessionManager`. `start_in_process_relays` — два личных `EphemeralRelay` в процессе. `run_agent_mode` — сценарий с настоящим `gipny-agent`. `run_dht_offline_mode` и `LiveBot` — сеть релеев, стороны уходят по очереди (сид кладётся в таблицу узлов бота через `put_seeds` перед каждым входом). `run_update_check_mode` — обновлятор через аутпрокси. `no_local_ports` (и `src/ports.rs`) — по `/proc` проверяет, что ни харнесс, ни агент не слушают loopback. `wait_for` и `compute_latencies` — ожидания и таймингы. Успех — строка `[e2e] SUCCESS`, тайминги — блок `[e2e-timing]` |

**Переменные окружения:**

- `E2E_RELAY_DEST`, `E2E_RELAY_DEST_A`, `E2E_RELAY_DEST_B` — адреса внешних релеев;
- `E2E_IN_PROCESS_RELAYS=1` — релеи внутри процесса;
- `E2E_AGENT_BIN` — путь к `gipny-agent`, включает режим агента;
- `E2E_DHT_OFFLINE=1` и `E2E_DHT_SEED_DEST` — сценарий сети релеев и адрес сида (`gipny-relay --dht`, отдельный процесс со своим роутером);
- `E2E_UPDATE_CHECK=1` — один роутер, обновлятор запрашивает последний релиз у GitHub через аутпрокси и скачивает самый маленький артефакт с проверкой SHA256 из того же релиза; сам `SHA256SUMS.txt` исключён;
- `E2E_N_MESSAGES` — число сообщений;
- `E2E_TIMEOUT_SECS` — таймаут;
- `E2E_BOTH_FIRST=1` — бот B пишет первым одновременно с A: встречные X3dhInit;
- `E2E_WORK_DIR` — каталог состояния;
- `GIPNY_I2P_LOGLEVEL` — уровень лога роутера (`i2pd.log` в каталоге роутера), только для диагностики.

## Сценарии в CI

| Джоба | Переменные | Что доказывает |
|---|---|---|
| `e2e (relay + two bots)` | `E2E_RELAY_DEST_A`/`_B` (два `gipny-relay`) | Письмо кладётся на релей **получателя**; один из четырёх live gates |
| `e2e (relays inside the bots)` | `E2E_IN_PROCESS_RELAYS=1` | Встроенный релей `libcore` работает по живой i2p |
| `e2e (agent binary)` | `E2E_AGENT_BIN`, `E2E_IN_PROCESS_RELAYS=1` | Агент: GRANT, команды по очереди, загрузка файла, OFF → REVOKE |
| `e2e (relay network, each side away in turn)` | `E2E_DHT_OFFLINE=1`, `E2E_DHT_SEED_DEST` | B уходит → A пишет и уходит → B возвращается на новом адресе, читает, отвечает, уходит → A возвращается, читает и знает новый адрес B |
| `updater through the outproxy (live)` в `i2p-embed.yml` | `E2E_UPDATE_CHECK=1` | Обновлятор достаёт список релизов и файл с GitHub через аутпрокси i2p |

### Какая ревизия роутера проверяется

PR и обычный ручной запуск `e2e-i2pd.yml` используют **точный gitlink**
`HEAD:third_party/i2pd` проверяемого application commit. Job `head` публикует
полные `pinned_sha`, `sha` и `mode=pinned`; все четыре сценария компилируют
один и тот же immutable SHA. Обычная проверка релиза не меняет pin.

Ночной `schedule` сохраняет probing tracked upstream branch. Для такого же
явного ручного probe есть boolean input `test_upstream`, по умолчанию `false`.
Только schedule или ручной `test_upstream=true` получают `mode=upstream`;
ветка разрешается в полный SHA один раз до запуска сценариев.
Job `bump` доступен только в upstream-mode на branch ref и требует успеха
**всех четырёх** jobs: `e2e`, `e2e-in-process`, `e2e-dht`, `e2e-agent`, а также
`delivered=true` основного сценария. Коммит начинается от проверенного
application SHA; push без force не перезаписывает продвинувшуюся ветку.
PR, tag и обычный ручной pinned-check pin не изменяют.

Это исправляет обнаруженный release verification gap: на `bd73d8b` все четыре
сценария прошли, но [head job](https://github.com/gluckdev/gipny-i2p/actions/runs/40781835085/job/123145580713)
показывает pin `7761d85409bd508909f130f95815d575aa18b9f2` и фактически выбранный
upstream `6ba5fd5c`. Эти PASS относятся к upstream router, а не доказывают
доставку shipping pin. Изменённый выбор ревизии требует нового GHA run с
`mode=pinned` и равными полными `sha`/`pinned_sha`; старый результат не переносится.

Каждый сценарий, пока роутеры живы, проверяет, что процесс не слушает ни одного порта на loopback (Linux, `/proc`): у роутера нет ни SAM, ни HTTP-прокси, ни консоли. Слушать можно только наружу, как транспорт NTCP2.

Агент вызывает `PR_SET_DUMPABLE=0`: обычный соседний процесс того же пользователя
не может читать его `/proc/<pid>/fd`. Поэтому при `PermissionDenied` harness
запускает через `sudo -n` metadata-only режим своего же executable:
`--inspect-process-listeners <PID> <START_TICKS>`. Он выполняется до Tokio,
профилей и роутера, принимает только два положительных десятичных числа и читает
фиксированные `/proc/<pid>/stat`, `fd`, `net/tcp{,6}`. Он не открывает произвольные
пути, не выполняет команды и не пишет файлы. Ответ ограничен 64 KiB и связан с
PID/starttime; живой owned child проверяется до и после снимка. Ошибки доступа,
sudo, timeout, parse или смена identity остаются ошибками теста. Hardening
производственного агента сохраняется.
[Linux `proc_pid_fd`](https://man7.org/linux/man-pages/man5/proc_pid_fd.5.html)
описывает ptrace access check для fd links, а
[`PR_SET_DUMPABLE`](https://man7.org/linux/man-pages/man2/PR_SET_DUMPABLE.2const.html)
— влияние non-dumpable режима на доступ к `/proc/<pid>`.

В checkpoint `a66d223` agent e2e выполнил 7/7 команд и OFF → REVOKE → exit 0,
но [job](https://github.com/gluckdev/gipny-i2p/actions/runs/40763085219/job/123080056599)
завершился ошибкой именно на прежнем cross-process `/proc` чтении. Это не общий
PASS agent e2e. Исправление observer и его новые tests требуют следующего GHA run.
Отдельный шаг после сборки задаёт абсолютный
`E2E_OBSERVER_BIN="$GITHUB_WORKSPACE/target/release/e2e-harness"`
и выполняет `cargo test --locked --release -p e2e-harness`: настоящий hardened
child с loopback должен быть недоступен обычному reader и обнаружен privileged
observer; loopback и неверный starttime должны отвергаться. Отсутствие binary/env
не превращается в пропуск проверки. Это разрешение sudo только для тестовой
среды, без запуска агента или пользовательских команд от root.
На `93295f1` первые 6/7 tests прошли, но реальный child test остановился до
spawn: прежний относительный путь искался из каталога Cargo package. Workflow
исправлен на абсолютный путь. При этом отдельный
[Linux security job на том же SHA](https://github.com/gluckdev/gipny-i2p/actions/runs/40772462125/job/123112992138),
где путь изначально был абсолютным, прошёл все 7/7 observer tests, включая
реальный non-dumpable child, sudo observation, loopback rejection и неверный
starttime. Этот job использует stub router: он доказывает OS/proc-проверку,
но не заменяет ещё не выполненный после path fix живой agent e2e.

`security-fast.yml` повторяет observer regressions на Linux с debug/stub
бинарником: `cargo build --locked -p e2e-harness`, затем
`E2E_OBSERVER_BIN="$PWD/target/debug/e2e-harness" cargo test --locked -p e2e-harness`.
Это реальная проверка `/proc` и hardening, но не native router или I2P transport.

## Контролируемый throughput benchmark

`E2E_PERFORMANCE=1` включает отдельный сценарий: после подключения и одного
warmup echo отправляется большой файл, одновременно с ним — короткие echo-пробы.
Обычные e2e, restart и offline-сценарии остаются отдельными проверками.

| Переменная | Значение |
| --- | --- |
| `E2E_PERF_REPORT` | Обязательный путь JSON-отчёта |
| `E2E_BIG_FILE` | Размер файла: по умолчанию 10 MiB, допустимо 1–256 MiB |
| `E2E_TIMEOUT_SECS` | Бюджет файловой фазы: по умолчанию 900 секунд, 1–7200 |
| `GIPNY_FILE_LANES` | Явное число прикладных потоков 1–8; без переменной штатная политика с cap 4 |
| `E2E_PERF_PROBE_INTERVAL_MS` | Интервал проб: 2000 мс, минимум 250, максимум 60000 |
| `E2E_PERF_PROBE_TIMEOUT_SECS` | Deadline каждой пробы: 60 секунд, 1–300 |
| `E2E_PERF_MAX_IN_FLIGHT` | Одновременно ожидаемые пробы: 4, допустимо 1–16 |

Оба клиента используют штатные 3-hop туннели и padding сообщений. Для
`E2E_IN_PROCESS_RELAYS=1` длина туннелей обоих personal relay проверяется через
runtime API. При внешних релеях нужны `E2E_PERF_RELAY_A_MANIFEST` и
`E2E_PERF_RELAY_B_MANIFEST`: файлы `destination-policy.json`, которые relay
пишет рядом с `dest.pub` после готовности destination. Harness требует совпадения
destination с `E2E_RELAY_DEST_A/B` (или общим `E2E_RELAY_DEST`) и значений
`inbound_hops=3`, `outbound_hops=3`. Это проверка контролируемых локальных
процессов, а не удалённая аттестация чужого сервера.

Benchmark отклоняет даже выставленные в `0` переменные `GIPNY_FAST`,
`GIPNY_FAST_HOPS`, `GIPNY_SERVER_HOPS`, `I2P_EMBED_STUB`, `I2P_EMBED_SKIP_NATIVE`:
их нужно убрать из окружения. Комбинации с agent, offline DHT, updater, restart
и simultaneous-first-message также отклоняются. Каждый run создаёт новые
профили в подкаталоге `E2E_WORK_DIR`, не удаляя предыдущие результаты.

В JSON `file.goodput_bytes_per_sec` — размер **одного полностью прочитанного и
побайтно проверенного** файла, делённый на время от начала enqueue до проверки
у получателя. Startup, warmup и ожидание оставшихся проб в знаменатель не входят.
Повторы частей, padding и повторные события завершения не увеличивают числитель.
До успешной проверки goodput равен `null`, а не выдуманному нулю.

`probes.rtt_ms` содержит nearest-rank p50/p95/p99 только уникальных ответов,
пришедших до deadline. `jitter_ms` — средняя абсолютная разница соседних RTT в
порядке получения. При недостатке данных значение `null`. Это задержка
прикладного echo с очередями, а не оценка односторонней задержки аудио или видео.
`lost`, `timed_out`, `interrupted`, `send_errors`, `enqueue_outcome_unknown`,
`late`, `duplicates` и все
индивидуальные samples сохраняются отдельно. Поздний ответ не превращает timeout
в успешную пробу. При потерях или ошибке файла run завершается ненулевым кодом,
сохранив отчёт; внешнее аварийное завершение может оставить `status=running`.
Поле `phase` различает `setup`, `measurement` и успешное `complete`; ошибка
setup не является результатом измерения производительности.

Лимит in-flight относится к отслеживаемым ожидаемым ответам. Durable outbox
может продолжать повторную доставку уже просроченной пробы, поэтому после
первого timeout новые пробы прекращаются. Дополнительно действует общий бюджет
1024 попытки; его исчерпание отражается в JSON и делает run неуспешным. Отправка
и echo используют по одному worker с ограниченной очередью на endpoint,
а чтение/проверка большого файла выполняется отдельно от цикла получения событий.
Блокировка enqueue или диска не останавливает обработку входящих RTT-проб.
Если deadline прервал enqueue до получения его результата, отчёт явно считает
исход неизвестным: такое сообщение могло уже попасть в durable outbox, поэтому
его нельзя достоверно включить ни в queued, ни в lost. Уже выполняющийся
`spawn_blocking` с чтением файла невозможно отменить; JSON с ошибкой сохраняется
по deadline, но окончательный выход процесса может ждать завершения чтения.
Внешний runner ограничивает время жизни процесса независимо от этого.

`transport.sender/receiver` содержат диагностические snapshots SessionManager.
`peak_send_lanes` — открытые потоки, доступные при успешном enqueue части;
это не доказательство загрузки каждого потока. `peak_receive_lanes` фиксирует
успешные подключения. Настроенный cap хранится отдельно от этих наблюдений.
Оба клиента harness разделяют один libi2pd router; внешние relay работают на
своих router. Эти topology явно различаются в отчёте и не должны смешиваться
при сравнении. Один удачный run не доказывает пригодность для звонков/скриншера.

## Куда вносить правки

| Задача | Где |
|---|---|
| Новый сценарий | ветка в `main` по новой переменной `E2E_*` + джоба в `e2e-i2pd.yml` |
| Число и содержимое сообщений, критерий успеха | `main` (для агента — `run_agent_mode`) |
| Какая ревизия i2pd компилируется в тест | `.github/scripts/i2pd-under-test.sh`, джоба `head` в `e2e-i2pd.yml` |
| Отчёт по задержкам | `compute_latencies`, шаг `timing summary` в `e2e-i2pd.yml` |

## Инварианты и грабли

- **Харнесс работает на `SessionManager`, а не на `Core`.** Логику приложения (запросы в контакты, режимы релея в `Core`) он не проверяет.
- **Обновлятор требует manifest выбранного релиза.** Ошибку получения или проверки суммы нельзя заменять скачиванием без проверки. [Контракт обновлятора](../../docs/update-verification.md).
- **Один роутер на процесс:** libi2pd держит глобальное состояние. Оба бота харнесса — destination на одном роутере; релеи `gipny-relay` и агент — отдельные процессы, у каждого свой роутер.
- **Холодный роутер и туннели занимают минуты.** Один прогон длится около 10–15 минут при лимите джобы 45.
- **Мок-режима нет, и это намеренно:** зелёный прогон на моке ничего не говорил.

## Как проверить

```
env -u CC -u CXX cargo check -p e2e-harness
```

Настоящий прогон — ручной запуск `e2e-i2pd` на нужной ветке со страницы Actions (из `gh` запуск возвращает 403). В логах смотреть `[e2e] SUCCESS`.
