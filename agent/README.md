# agent — `gipny-agent`: headless-агент, выполняющий команды мастера из чата

В объединённой ветке карточка мастера по умолчанию — `gipny:v3:`; прежняя
v2 с релеем также принимается. Флаг `--check-master-card` проверяет карточку
без запуска роутера, credential provider, базы и создания каталога данных;
при успехе ничего не печатает. Установщики проверяют v3 через скачанный и
проверенный бинарь до изменения установки; для этого нужен агент 0.4.19 или
новее. [Формат, совместимость и ограничения](../docs/contact-cards.md).

Один бинарь, `src/main.rs`, поверх `SessionManager` из `libcore`. Запускает
роутер I2P внутри своего процесса через Rust `i2p-embed` → C++ shim →
`libi2pd`, поднимает личный релей, пишет мастеру первым, выполняет присланные
команды и сам обновляется. Служебные local SAM/proxy/web console отключены;
IP-транспорты роутера остаются. Отдельный daemon или внешний router CLI не
нужны. Пользовательская инструкция по запуску — в `README.txt` внутри архива
(генерируется в `release.yml`) и в [корневом README](../README.md).

## Что здесь

| Файл | За что отвечает |
|---|---|
| `src/main.rs` | `parse_args`/`usage` (флаги `--data`, `--master`, `--name`, `--relay`, `--cwd`, `--timeout`); `data_dir` (`--data` или `GIPNY_AGENT_DATA`); `load_master`/`load_relay`, которые запоминают карточку мастера и `--relay` в `master.card` и `relay.txt`; `main` (роутер → `SessionManager::start` → встроенный `EphemeralRelay` или внешний релей → GRANT мастеру → цикл событий: команды, OFF); `load_attachments`; `run_update_loop` (автообновление); `begin_stop` (REVOKE и выход) |
| `gipny-agent.service` | systemd-юнит: `/var/lib/gipny-agent`, аргументы из `/etc/gipny-agent.env` (`GIPNY_AGENT_ARGS`), `Restart=on-failure` |
| `Cargo.toml` | Версия агента, должна совпадать с версией приложения |

## Куда вносить правки

| Задача | Где |
|---|---|
| Новый флаг командной строки | `src/main.rs`: `Args`, `parse_args`, `usage` + раздел Options в `README.txt` внутри `release.yml` |
| Что делает команда консоли, справка, таймауты, лимиты вывода | `libcore/src/agent.rs` (`handle_console_request`, `run_command`, `HELP_TEXT`) |
| Разбор GRANT, REVOKE, OFF | `libcore/src/agent.rs` (`parse_control`) и цикл событий в `main` |
| Встроенный и внешний релей агента | `main` в `src/main.rs` (`EphemeralRelay::start`, `set_local_relay` — свой релей читается по трубе, `set_relay_onion`, `join_dht` — вход в сеть релеев через узел самого `SessionManager`) |
| Автообновление агента | `run_update_loop` + `libcore/src/update.rs` (`Component::Agent`, `install_agent_binary_now`) |
| Приём и отправка сообщений агентом | `libcore/src/session.rs` |
| Юнит systemd | `gipny-agent.service` |
| Состав архива релиза | джоба `agent (linux …)` в `.github/workflows/release.yml` |

## Инварианты и грабли

- **Агент работает на `SessionManager`,** а не на `Core`: любые правки доставки в `libcore/src/session.rs` касаются его напрямую.
- **База агента шифруется SQLCipher** через `security::open_device_bound_database` и ключ OS credential provider. Каталог данных остаётся приватным. Отсутствующий/заблокированный provider означает явный отказ, без fallback на raw key file или plaintext DB; перенос каталога не заменяет авторизованный export/import. Прежний unbound профиль автоматически не переписывается.
- **Адрес личного релея новый при каждом запуске.** Мастер узнаёт его из GRANT, который агент шлёт при каждом старте.
- **Путь к бинарю для самообновления берётся один раз** до цикла: после первой подмены `current_exe()` указывал бы на удалённый файл.
- **Самообновление требует SHA256 из manifest релиза** (следующая версия). При отсутствии суммы или ошибке проверки `download` возвращает ошибку, и `run_update_loop` не вызывает установку. [Контракт и ограничения](../docs/update-verification.md).
- **Способ установки зависит от платформы.** Linux GNU и macOS атомарно заменяют бинарь для следующего запуска; Windows передаёт проверенный файл bootstrap `start-agent.ps1` через `pending_agent/ready.json`. Старой Windows-задаче нужен повторный запуск установщика; без bootstrap операция возвращает ошибку и сохраняет архив. Для musl выбирается только точный поддерживаемый target без GNU fallback. [Матрица и пределы](../docs/platform-updates.md).
- **Групповая очередь общая с ботами.** Подготовленные metadata и полный снимок получателей публикуются одной транзакцией; ожидание каждого получателя снимает только его E2E ACK. Это `at-least-once` до срока, не подтверждение локального enqueue. [Контракт](../docs/group-delivery.md).
- **Release не пишет подробный файловый журнал.** Диагностика доступна тестам и специальным сборкам; рабочие сообщения CLI и публичная карточка сохраняются. [Политика диагностики](../docs/release-diagnostics.md).
- **Рядом с исполняемым файлом лежит только снимок сети** (`i2pd-netdb-seed.tar.gz`): роутер вкомпилирован, отдельного i2pd в архиве нет. Smoke-тест в `release.yml` это проверяет.

## Как проверить

```
env -u CC -u CXX cargo check --locked -p gipny-agent
```

Команда с обычным окружением компилирует настоящий встроенный роутер;
`I2P_EMBED_STUB=1` проверяет только Rust integration и не даёт рабочую I2P-связь.
По живой I2P агента проверяет джоба `e2e (agent binary)` в `e2e-i2pd.yml`
(`E2E_AGENT_BIN`). Запуск из архива — smoke-тест в джобе `agent (linux …)`
в `release.yml`. Существование workflow не означает, что текущая ветка уже
прошла его: [снимок интеграции](../docs/today-2026-10-28.md).

## Установка одной командой

В GIPNY откройте **Настройки → Режим агента → Подключить удалённый сервер / агента**.
Окно подставляет текущую карточку мастера вместе с ключами и адресом его релея.
Выберите Linux/macOS с root, Linux без root или Windows и скопируйте команду.
На Windows откройте PowerShell от имени администратора.

Установщики: `scripts/install-agent.sh` и `scripts/install-agent.ps1`. Первый
поддерживает `--help`, `--status`, `--uninstall`, выбор версии и каталога данных.
Второй загружает функцию `Install-GipnyAgent`; удаление —
`Install-GipnyAgent -Uninstall`. Удаление также стирает данные и идентичность
агента; нестандартный каталог запоминается установщиком.

Для systemd без root автозапуск после выхода из системы требует linger:
установщик выводит команду `sudo loginctl enable-linger <user>`. При отсутствии
systemd/OpenRC установщик запускает агент через `nohup` и добавляет cron
`@reboot`, если доступен `crontab`; иначе выводит путь скрипта для ручного
добавления в автозапуск. На macOS используется launchd; на Windows — задача
планировщика под SYSTEM с запуском при старте и повтором при сбое.

Архив релиза должен содержать `gipny-agent` (`gipny-agent.exe` на Windows) и
`i2pd-netdb-seed.tar.gz`, а имя — соответствовать
`gipny-agent_<version>_<linux|darwin|windows>-<amd64|arm64>.tar.gz`.
Workflow релизов собирает Linux amd64/arm64, macOS amd64/arm64 и Windows
amd64. macOS и Windows используют уже подготовленные зависимости desktop-
сборок. Windows ARM64 пока не собирается. Установщик выдаёт понятную ошибку,
если для выбранной платформы в выбранном релизе нет артефакта.

В объединённой ветке имя без `--name` выбирается из ASCII-имён котиков и
запоминается как `agent_name`; явное `--name` сохраняет выбранное вами имя.
Самообновление учитывает `auto_update=0` и `GIPNY_NO_AUTO_UPDATE=1|true|yes|on`.
Передача файла установщику не отмечает версию как уже установленную:
[текущий статус](../docs/platform-updates.md).

В headless окружении нужен доступный защищённый credential provider того
пользователя, от которого запускается служба. На Linux это Secret Service;
отдельный service user/root не получает автоматически доступ к хранилищу
desktop user. OpenWrt binary target не гарантирует работающий runtime:
без предусмотренного защищённого provider запуск завершится отказом.
Права `0700` сами по себе не обеспечивают device binding.
[Provider и миграционные ограничения](../docs/device-bound-storage.md).
