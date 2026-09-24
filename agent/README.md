# agent — `gipny-agent`: headless-агент, выполняющий команды мастера из чата

Один бинарь, `src/main.rs`, поверх `SessionManager` из `libcore`. Запускает рядом свой i2pd (или подключается к `--sam`), поднимает личный релей, пишет мастеру первым, выполняет присланные команды и сам обновляется. Пользовательская инструкция по запуску — в `README.txt` внутри архива (генерируется в `release.yml`) и в корневом `README.md`.

## Что здесь

| Файл | За что отвечает |
|---|---|
| `src/main.rs` | `parse_args`/`usage` (флаги `--data`, `--master`, `--name`, `--relay`, `--sam`, `--cwd`, `--timeout`); `data_dir` (`--data` или `GIPNY_AGENT_DATA`); `load_master`/`load_relay`, которые запоминают карточку мастера и `--relay` в `master.card` и `relay.txt`; `main` (роутер → `SessionManager::start` → встроенный `EphemeralRelay` или внешний релей → GRANT мастеру → цикл событий: команды, OFF); `load_attachments`; `run_update_loop` (автообновление); `begin_stop` (REVOKE и выход) |
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
- **База агента открывается без шифрования** (`Db::open_plain`). Каталог данных создаётся приватным для пользователя.
- **Адрес личного релея новый при каждом запуске.** Мастер узнаёт его из GRANT, который агент шлёт при каждом старте.
- **Путь к бинарю для самообновления берётся один раз** до цикла: после первой подмены `current_exe()` указывал бы на удалённый файл.
- **i2pd ищется рядом с исполняемым файлом.** Поэтому он лежит в архиве, и smoke-тест в `release.yml` это проверяет.

## Как проверить

```
env -u CC -u CXX cargo check -p gipny-agent
```

По живой i2p агента проверяет джоба `e2e (agent binary)` в `e2e-i2pd.yml` (`E2E_AGENT_BIN`). Запуск из архива без системного i2pd — smoke-тест в джобе `agent (linux …)` в `release.yml`.
