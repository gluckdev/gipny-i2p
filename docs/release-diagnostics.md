# Журналы по типу сборки / Diagnostics by build type

## Русский

Текущее требование владельца от **2026-11-06**: стабильный релиз не содержит
подробного журнала, предрелизная/разработческая/тестовая сборка всегда ведёт его.
Пользователь не включает и не выключает журнал. Блок Application log удалён из
общих настроек desktop, Android и браузера.

| Поведение | Stable | Prerelease / development / tests |
|---|---|---|
| Уровень приложения | `Null` | `Full` |
| Runtime env, прежний `log.conf`, UI | Не меняют политику | Не меняют политику |
| `debug_trace!` / `debug_error!` | Ветви отсутствуют после оптимизации; аргументы не вычисляются | Вывод включён |
| Native router | `warn` в stderr, без файла | `debug`; файл при переданном пути |
| Android WebView в `chrome://inspect` (USB-отладка) | Закрыт | Открыт: страница читается с компьютера, даже если она не смогла отрисоваться |
| Browser diagnostic store | Не создаётся; оставшаяся от пре-релиза база IndexedDB удаляется при старте | Кольцевой журнал в памяти Worker; при блокировке копия в IndexedDB `gipny-browser-log` (номера контактов, без имён и ключей), стирается очисткой и удалением профиля |

`i2p-embed/src/diagnostics.rs` задаёт compile-time policy. Release workflow
передаёт `GIPNY_RELEASE_LOGS=full` при сборке предрелиза; для stable значение
пустое. Это переменная **сборки**, а не переключатель установленного приложения.
`build-provenance.py` проверяет её согласованность с версией и публикует
`developer_log` в манифестах. `ui/build-policy.ts` выбирает browser policy по
режиму разработки/предрелизному номеру версии; настройки пользователя её не меняют.

Native `Router::start` нормализует аргументы логирования до запуска C++.
Shim выставляет уровень до `InitI2P` и передаёт настоящий stderr-поток;
передача null в upstream API могла бы создать собственный файл даже в stable.
Core и SessionManager используют общие macros. Операционные ошибки в stderr,
CLI-справка, OS-журналы и внешний захват stderr могут существовать и в stable;
это не подробный журнал приложения.

### Где находятся данные

На Unix (Linux/macOS/Android) приложение перехватывает stderr в `debug.log`
с ротацией в `debug.prev.log`; у native router есть отдельный `i2pd.log`.
Агент и выделенный релей выводят диагностику в stderr для своего оператора.
На Windows существующий общий capture основан на Unix `dup2` и не переносится:
`debug.log` GUI пока не создаётся этим механизмом, остаётся журнал роутера и
доступный процессу stderr. Полноценный Windows capture требует отдельной работы.
Веб-клиент хранит диагностические записи локально в браузере, не на GitHub: в
памяти Worker и, после блокировки, в незашифрованной IndexedDB `gipny-browser-log`,
доступной до разблокировки. В записях — номера контактов, не имена; ключи и
адреса маскируются. Запись `stall` (не чаще раза в минуту, до пяти подряд)
называет работу, застрявшую дольше двух минут: транзакцию профиля, шаг отправки
или очередь без депозитов, только счётчиками.

Журналы могут раскрывать адреса, идентификаторы, имена файлов и время действий.
Пароли, ключи, токены, текст сообщений/команд и callback-данные не должны попадать
в них даже в предрелизе. Перед публикацией evidence проверяйте содержимое;
профили, дампы OPFS и приватные конфигурации не являются публичным артефактом CI.
Старые файлы от предыдущих версий не исчезают автоматически после установки
stable; duress/явная очистка имеют собственную границу, внешние копии сохраняются.

На Android предрелизная сборка (и любая debug-сборка) вызывает
`WebView.setWebContentsDebuggingEnabled(true)` в `MainActivity`: при включённой
USB-отладке страница приложения видна в `chrome://inspect` на компьютере вместе
с консолью JS. Это единственный способ увидеть ошибку, из-за которой страница не
отрисовалась: wry пишет сообщения консоли в logcat только в debug-сборках. Так
2026-11-07 на Honor KOZ-AL00 (Huawei WebView 11 = Chromium 83) rc3 открывался
пустым экраном без единой строки в logcat; [ui/README](../ui/README.md)
описывает нижнюю границу WebView и проверку сборки. Stable-сборка этот вход не
открывает. Снимки экрана при этом по-прежнему запрещены `FLAG_SECURE`.

### Проверка

Сборки и исполняемые проверки выполняются **в GitHub Actions**. Основные
сценарии: `scripts/check-release-diagnostics.sh`, policy unit tests,
`i2p-embed/tests/diagnostics_probe.rs`, UI-тесты отсутствия переключателя и
проверки provenance/ELF. Они проверяют stable без trace-строки и без вычисления
аргументов, неизменяемый `Full` предрелиза и отсутствие обхода через env/старые
настройки. Результаты прежних SHA не подтверждают текущую правку.

## English

Owner requirement dated **2026-11-06**: stable releases omit detailed diagnostics;
prerelease/developer/test builds always enable them. There is no runtime toggle,
and the Application log section is removed from desktop, Android and browser
settings. Stable uses `Null`, diagnostic builds use `Full`; legacy settings and
runtime environment cannot alter the policy.

The native policy lives in `i2p-embed/src/diagnostics.rs`. Release CI supplies
compile-time `GIPNY_RELEASE_LOGS=full` for prereleases and an empty value for
stable; provenance validates version consistency and records `developer_log`.
The browser build uses `ui/build-policy.ts` and its prerelease version. Router
options are normalized before C++ startup: stable uses stderr warnings without
a file, diagnostics use debug/file logging. Stable may still emit operational
errors or be observed by OS/external stderr collectors; it does not promise an
empty process output.

Unix native apps capture stderr in `debug.log`/`debug.prev.log`; the router has
`i2pd.log`. Agents/relays expose stderr to operators. Windows GUI does not yet
have the Unix-based general capture: its router file and available stderr are
separate, and complete Windows capture remains work. The browser log is local
browser storage, not GitHub storage: Worker memory and, after lock, the
unencrypted IndexedDB `gipny-browser-log` readable before unlock. Entries carry
contact numbers, not names; keys and addresses are masked. A `stall` entry (at
most once a minute, five in a row) names work stuck for over two minutes — a
profile transaction, a send step or an outbox with no deposits — as counts only.
Stable builds delete that database at startup.

On Android a prerelease build (and any debug build) calls
`WebView.setWebContentsDebuggingEnabled(true)` in `MainActivity`: with USB
debugging on, the app's page shows up in `chrome://inspect` on a computer, JS
console included. That is the only way to read an error that stopped the page
from drawing, because wry forwards console messages to logcat in debug builds
only. On 2026-11-07 rc3 opened as a blank screen on an Honor KOZ-AL00 (Huawei
WebView 11 = Chromium 83) with nothing in logcat; [ui/README](../ui/README.md)
states the WebView floor and the build check. Stable builds never open this
door, and `FLAG_SECURE` still blocks screenshots.

Diagnostics can expose addresses, identifiers, filenames and timing. Passwords,
keys, tokens, message/command bodies and callbacks must be excluded even in
prereleases. Inspect evidence before publication; profiles, OPFS dumps and private
configuration do not belong in public CI artifacts. Installing stable does not
automatically erase older logs or external copies.

Builds/executable checks run in GitHub Actions: policy probes, unit/UI tests,
provenance and ELF inspection verify immutable logging and absent stable traces.
Historical passing results do not validate this working-tree change.
