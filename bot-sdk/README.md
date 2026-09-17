# bot-sdk — `gipny-bot`: библиотека для ботов gipny

Бот — обычный аккаунт gipny без интерфейса, с тем же сквозным шифрованием. Работает поверх `SessionManager` из `libcore`. Описание API для авторов ботов с примерами — `docs.md`.

## Что здесь

| Файл | За что отвечает |
|---|---|
| `src/lib.rs` | `Bot::builder()` → `BotBuilder` (`data_dir`, `relay`, `display_name`, `vault_passphrase`, `on_message`, `on_command`, `on_callback`, `build`); `Bot::run`/`Bot::start` → `RunningBot`; `Context` для обработчиков (`reply`, `reply_with_buttons`, `send_attachment*`, `edit`, `edit_with_buttons`, `with_sound`, `is_group`, `group_id`); `IncomingMessage`, `BotTarget`; реэкспорт `gipny_libcore` |
| `docs.md` | Документация для пользователей SDK: builder, обработчики, контекст, файлы в коллбеках, инлайн-клавиатуры, развёртывание |

## Куда вносить правки

| Задача | Где |
|---|---|
| Новый метод для обработчика | `Context` в `src/lib.rs` + `docs.md` |
| Новый тип обработчика или параметр запуска | `BotBuilder` и цикл событий в `Bot::start` + `docs.md` |
| Кнопки, коллбеки, звуки на уровне протокола | `libcore/src/session.rs` (`WirePayload`: `buttons`, `callback_data`, `notify_sound`) |
| Доставка, повторы, сессии | `libcore/src/session.rs` |

## Инварианты и грабли

- **Встроенного релея у SDK нет:** `relay(...)` задаёт внешний релей. Личный релей в процессе показан в `tests/e2e-harness` (`start_in_process_relays`).
- **Боты принимают всех:** запросов в контакты здесь нет (они есть только в приложении).
- **Меняя публичный API, обновите `docs.md`** — это единственная документация для авторов ботов.

## Как проверить

```
env -u CC -u CXX cargo check -p gipny-bot
```

По живой i2p SDK-путь (`SessionManager`) проверяют джобы `e2e-i2pd.yml`: харнесс поднимает двух ботов.
