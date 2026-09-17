# ui — интерфейс gipny на TypeScript без фреймворка

Всё, что видит пользователь. Одна кодовая база для десктопа и Android. Бэкенд вызывается через Tauri (`invoke`), события приходят как `core_event`. Сборка — Vite (`npm run build` = `tsc --noEmit && vite build`).

## Что здесь

| Файл | За что отвечает |
|---|---|
| `src/main.ts` | Точка входа |
| `src/api.ts` | Все вызовы бэкенда (`Api.*`) и типы данных (`Contact`, `Message`, `UpdateInfo`, union `CoreEvent`…), карточка `encodeCard`/`decodeCard`/`isValidI2pAddress` |
| `src/state.ts` | `Store` — всё состояние в `Signal<T>` (`contacts`, `groups`, `messages`, `unread`, `selectedChat`, `relayInfo`, `updateAvailable`…), загрузка (`refreshAll`/`refreshContacts`), **обработка всех `CoreEvent`**, уведомления и звуки, тосты `showToast`; `targetKey`, `sameTarget` |
| `src/view.ts` | Основа: `View` (подписки `sub`), `h()` для создания элементов, `avatar()` (инициалы и цвет от ключа), форматтеры (`fmtTime`, `fmtAgo`, `short`, `humanSize`, `trustLabel`…), `logo` |
| `src/app.ts` | `App`: раскладка, `openModal`, модалка обновления `openUpdateModal` |
| `src/auth.ts` | Экраны `AuthCreate`, `AuthUnlock`, `AuthBooting` |
| `src/profile.ts` | Выбор профиля `ProfileSelect` |
| `src/sidebar.ts` | Список: шапка (моя карточка, меню «+», поиск), сворачиваемые секции «Группы», «Запросы» (`requestRow`), **папки контактов** (`folderBlock`, `folderMenu`; хранятся в хранилище через `get/set_contact_folders`), «Без папки» |
| `src/chat.ts` | `ChatView`: лента, ввод, вложения, ответ, TTL, шапка, пометка «контакт недоступен», переключатель чат/консоль агента |
| `src/actions.ts` | Контекстные меню (`ContextMenu`, `attachContextMenu`, `messageMenuItems`), `PinnedBanner`, `EditInline` |
| `src/contact.ts` | `ContactModal` (имя, доверие, сброс сессии, удаление), `AddContactModal` (вставка карточки) |
| `src/group.ts` | `GroupModal`, `CreateGroupModal` |
| `src/identity.ts` | `IdentityModal` — «моя карточка» |
| `src/settings.ts` | `SettingsModal`: релей, роутер, приватность вложений, автообновление, агент, бэкап, отладочный лог |
| `src/about.ts` | `AboutModal` — «О gipny и безопасности». Каждое утверждение должно совпадать с кодом (ссылки в комментарии к классу) |
| `src/icons.ts` | SVG-иконки `icon(name)` и иллюстрации (`emptyChatArt`, `networkArt`) |
| `src/search.ts`, `src/media.ts`, `src/forward.ts` | Поиск, галерея вложений, пересылка |
| `src/theme.ts` | Тема: `getTheme`, `setTheme`, `applyTheme` |
| `src/styles.css` | Все стили; цвета — токены в `:root` и `:root[data-theme="dark"]`. Текущий облик — блок «Messenger look» перед адаптивной частью; адаптивные правила — последними |
| `public/sounds/` | Звуки уведомлений |
| `dev/mock.ts` | Мок Tauri IPC: ответы на все команды и фикстуры (неудобные намеренно) |
| `dev/preview.html`, `dev/frame.html` | Превью приложения на ширинах телефона, планшета и десктопа с проверкой горизонтального переполнения |

## Куда вносить правки

| Задача | Где |
|---|---|
| Новый вызов бэкенда | `Api.*` в `api.ts` + case в `dev/mock.ts` (+ команда в `core/src/lib.rs`) |
| Реакция на новое событие | тип в union `CoreEvent` (`api.ts`) + ветка в обработчике событий `Store` (`state.ts`) |
| Новое поле контакта или сообщения | интерфейс в `api.ts` + фикстуры в `dev/mock.ts` (+ DTO в `core/src/lib.rs`) |
| Новая настройка | `settings.ts` (по образцу чекбоксов «приватность вложений» и «обновляться автоматически») + `Api` |
| Секции, папки и строки списка чатов | `sidebar.ts` (`renderList`, `contactRow`, `folderBlock`) |
| Новая иконка | `PATHS` в `icons.ts` |
| Шапка чата, плашки, ввод | `chat.ts` |
| Меню сообщения | `actions.ts` (`messageMenuItems`) |
| Добавление контакта, карточка | `contact.ts`, `identity.ts`, формат — `api.ts` (`encodeCard`/`decodeCard`) |
| Тосты, уведомления, звуки, счётчик непрочитанных | `state.ts` |
| Цвета и тема | токены в `styles.css`, `theme.ts` |
| Раскладка на телефоне | `styles.css` + проверка в `dev/preview.html` |
| Модалка обновления | `app.ts` (`openUpdateModal`) + обработка `Update*` в `state.ts` |
| Экраны входа | `auth.ts`, `profile.ts` |

## Что менять вместе

- **`api.ts` ↔ `core/src/lib.rs` (команды, DTO) ↔ `core/src/core.rs` (`CoreEvent`).** Имена полей совпадают с serde (snake_case).
- **`api.ts` ↔ `dev/mock.ts`.** Каждая используемая команда должна быть в моке, иначе превью ломается.
- **`encodeCard`/`decodeCard` ↔ `libcore/src/card.rs`.**
- **Список контактов в UI фильтруется по `request`:** запросы (`'incoming'`) не должны попадать в пикеры групп, пересылки и выбора мастера агента, а также в счётчики непрочитанного.

## Инварианты и грабли

- **Фреймворка нет.** Компонент — наследник абстрактного `View` (`view.ts`) с полем `el`; подписки делаются через `this.sub(signal, fn)` и снимаются в `destroy()`.
- **Подписи в интерфейсе смешанные:** в основном английские строчные, часть на русском. Держитесь стиля соседних элементов.
- **`localStorage` — только для удобств отдельного устройства;** всё важное хранится в бэкенде.
- **Длинные адреса i2p (516+ символов без пробелов) ломают раскладку.** Фикстуры в `dev/mock.ts` это ловят, проверяйте в превью.

## Как проверить

```
cd ui && npx tsc --noEmit
cd ui && npm run dev    # затем http://127.0.0.1:5173/dev/preview.html, кнопка «check horizontal overflow»
```

В CI интерфейс проверяет джоба `ui typecheck` в `build.yml`. Сборка приложения с интерфейсом — только на GitHub.
