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
