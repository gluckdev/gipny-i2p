# ui — интерфейс gipny на TypeScript без фреймворка

Всё, что видит пользователь. Одна кодовая база для десктопа, Android, loopback WebUI и автономного браузерного режима. Нативный бэкенд вызывается через Tauri (`invoke`), события приходят как `core_event`; браузерный адаптер сохраняет тот же API. Сборка — Vite (`npm run build` = `tsc --noEmit && vite build`).

## Точки входа и браузерное ядро

`index.html` / `src/main.ts` обслуживают Tauri и loopback `gipny-web`.
`browser.html` / `src/browser-main.ts` запускают автономный PWA без нативного
процесса. Worker содержит Rust WASM `BrowserEngine` и client-only I2P-роутер.

| Файл | Назначение |
|---|---|
| `src/browser-core/client.ts` | Worker RPC, события для существующего UI, завершение Worker при блокировке |
| `src/browser-core/worker.ts` | Загрузка WASM и Worker backend |
| `src/browser-core/backend.ts` | Транзакции, контакты, история, группы, outbox, TTL и релей |
| `src/browser-core/storage.ts` | OPFS sync handles, Web Lock, `vault-a.bin` / `vault-b.bin` |
| `src/browser-core/passkeys.ts` | WebAuthn PRF на главном потоке по действию пользователя |
| `src/browser-core/relay.ts` | Приём и отправка через in-process I2P destination |
| `src/browser-core/model.ts` | Метаданные профиля, DTO, истечение истории |
| `src/browser-files.ts` | Небольшие вложения браузера |
| `vite.browser.config.ts` | Отдельная PWA сборка, manifest и Service Worker |

`src/about.ts` показывает отдельное описание хранения, доставки и безопасности
для браузерного режима; его тексты поддерживаются на русском и английском.

## Особенности и границы браузерного режима

- **Транзакции и долговечность:** Запись в хранилище OPFS защищена Web Locks. Новые данные
  и продвижение ключей сессии Double Ratchet сбрасываются на диск (`flush`) **до** отправки
  сообщения в сеть. Входящие сообщения и квитанции фиксируются в OPFS **до** подтверждения их получения.
- **Двухслотовая схема и смена пароля:** Профиль хранится в двух чередующихся зашифрованных слотах.
  Операция смены пароля (`change_password`) последовательно записывает и flush-ит новый wrapper в оба слота,
  исключая откат к старому паролю через восстановительный слот.
- **Аутентификация Passkey:** Поддерживается через расширение WebAuthn PRF.
  Подписи учетных данных или идентификаторы ключей не используются вместо секрета шифрования.
- **Очистка метаданных:** Реализована на чистом коде в WASM для JPEG, PNG и WebP.
  Очистка документов PDF в браузере отклоняется из соображений приватности; пользователь может
  явно отключить фильтр метаданных в настройках для отправки исходного файла.
- **Ограничения по объёму:** Инлайн-вложения ограничены 1 МиБ на сообщение; общий размер профиля
  в OPFS ограничен 16 МиБ.
- **Жизненный цикл вкладки:** События `pagehide` и `freeze` немедленно завершают работу воркера,
  сбрасывают доступное UI состояние и блокируют интерфейс; возвращение на страницу через `pageshow`/`resume`
  вызывает перезагрузку окна для повторного ввода учетных данных. Это не гарантия физического стирания RAM браузера.
- **Кеширование Service Worker:** Оффлайн-кеш Service Worker содержит только неизменяемый
  статический шелл приложения (HTML, JS, CSS, WASM). Сетевая конфигурация `browser-config.json`,
  снапшоты NetDb и данные пользователей строго исключены из кеша.
- **Неподдерживаемые функции в браузере:** Консоль выполнения команд агента, передача крупных
  многосегментных файлов, нативный узел DHT, транзитные туннели и duress-пароли требуют будущей реализации.

Подробное руководство по развёртыванию веб-версии: [docs/browser-messenger.md](../docs/browser-messenger.md).

## Что здесь

| Файл | За что отвечает |
|---|---|
| `src/i18n.ts`, `src/locales/en.ts`, `src/locales/ru.ts` | Язык устройства, типизированные ключи и полные словари; [правила поддержки](../docs/localization.md) |
| `src/language-picker.ts`, `src/error-text.ts` | Выбор языка до входа и отображение известных ошибок backend |
| `tests/localization.test.mjs` | DOM-проверки языков, черновика, подписок и вывода недоверенного текста |
| `src/main.ts` | Точка входа |
| `src/api.ts` | Все вызовы бэкенда (`Api.*`) и типы данных (`Contact`, `Message`, `UpdateInfo`, union `CoreEvent`…), карточка `encodeCard`/`decodeCard`/`isValidI2pAddress` |
| `src/state.ts` | `Store` — всё состояние в `Signal<T>` (`contacts`, `groups`, `messages`, `unread`, `selectedChat`, `relayInfo`, `updateAvailable`…), загрузка (`refreshAll`/`refreshContacts`), **обработка всех `CoreEvent`**, уведомления и звуки, тосты `showToast`; `targetKey`, `sameTarget` |
| `src/view.ts` | Основа: `View` (подписки `sub`), `h()` для создания элементов, `avatar()` (инициалы и цвет от ключа), форматтеры (`fmtTime`, `fmtAgo`, `short`, `humanSize`, `trustLabel`…), `logo` |
| `src/app.ts` | `App`: раскладка, `openModal`, модалка обновления `openUpdateModal` |
| `src/auth.ts` | Экраны `AuthCreate`, `AuthUnlock`, `AuthBooting`. Экран загрузки показывается **до** вызова `vault_unlock` (там все минуты) и рисует настоящие этапы из события `boot_status`: `bootLabels()` — подписи, `bootSteps`/`bootLog`/`bootCanEnter` в `state.ts` — данные |
| `src/profile.ts` | Выбор профиля `ProfileSelect` |
| `src/sidebar.ts` | Список: шапка (моя карточка, меню «+», поиск), сворачиваемые секции «Группы», «Запросы» (`requestRow`), **папки контактов** (`folderBlock`, `folderMenu`; хранятся в хранилище через `get/set_contact_folders`), «Без папки» |
| `src/chat.ts` | `ChatView`: лента, ввод, вложения, ответ, TTL, шапка, пометка «контакт недоступен», переключатель чат/консоль агента |
| `src/actions.ts` | Контекстные меню (`ContextMenu`, `attachContextMenu`, `messageMenuItems`), `PinnedBanner`, `EditInline` |
| `src/contact.ts` | `ContactModal` (имя, доверие, сброс сессии, удаление), `AddContactModal` (вставка карточки и **сканирование QR** камерой через `QrScanner`) |
| `src/qr-scan.ts` | Камера и распознавание QR (`jsqr`); разрешение у Android спрашивает сам webview (wry), отказ и отсутствие камеры — сообщением |
| `src/group.ts` | `GroupModal`, `CreateGroupModal` |
| `src/identity.ts` | `IdentityModal` — «моя карточка»: QR карточки (`Api.qrSvg` → команда `qr_svg`, рисует Rust), само значение карточки, имя, аватарка |
| `src/settings.ts` | `SettingsModal`: релей, роутер, приватность вложений, автообновление, агент, бэкап, отладочный лог |
| `src/about.ts` | `AboutModal` — «О gipny и безопасности». Каждое утверждение должно совпадать с кодом (ссылки в комментарии к классу) |
| `src/avatars.ts`, `src/avatar-picker.ts`, `public/avatars.webp` | Аватарки: спрайт 6×4 по 96 px (≈47 КБ) из гравюр в общественном достоянии, случайная по ключу (`avatarIndex`), выбор человека хранится в хранилище (`get/set_ui_data` ключ `avatars`); источники — в комментарии в `avatars.ts`. Новый набор: пересобрать спрайт тем же размером сетки и обновить `avatarNames()` |
| `src/icons.ts` | SVG-иконки `icon(name)` и иллюстрации (`emptyChatArt`, `networkArt`) |
| `src/search.ts`, `src/media.ts`, `src/forward.ts` | Поиск, галерея вложений, пересылка |
| `src/theme.ts` | Тема: `getTheme`, `setTheme`, `applyTheme` |
| `src/styles.css` | Все стили; цвета — токены в `:root` и `:root[data-theme="dark"]`. Текущий облик — блок «Messenger look» перед адаптивной частью; адаптивные правила — последними |
| `public/sounds/` | Звуки уведомлений |
| `dev/mock.ts` | Мок Tauri IPC: ответы на все команды и фикстуры (неудобные намеренно) |
| Сцена `boot` в превью | `dev/mock.ts`: `vault_unlock` не отвечает, а `boot_status` эмитится по сценарию — так экран загрузки видно без бэкенда |
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
| Новый этап при запуске | `BOOT_STEPS` и `BootStep` в `state.ts` + `bootLabels()` в `auth.ts` + `boot_status` в `core/src/lib.rs` |

## Что менять вместе

- **`api.ts` ↔ `core/src/lib.rs` (команды, DTO) ↔ `core/src/core.rs` (`CoreEvent`) ↔ `browser-core/backend.ts`.** Имена полей совпадают с serde (snake_case).
- **`api.ts` ↔ `dev/mock.ts`.** Каждая используемая команда должна быть в моке, иначе превью ломается.
- **`encodeCard`/`decodeCard` ↔ `libcore/src/card.rs`.**
- **Список контактов в UI фильтруется по `request`:** запросы (`'incoming'`) не должны попадать в пикеры групп, пересылки и выбора мастера агента, а также в счётчики непрочитанного.

## Инварианты и грабли

- **Фреймворка нет.** Компонент — наследник абстрактного `View` (`view.ts`) с полем `el`; подписки делаются через `this.sub(signal, fn)` и снимаются в `destroy()`.
- **Весь интерфейс имеет английскую и русскую локаль.** Новая подпись, ошибка, placeholder, tooltip и aria-label добавляются в оба словаря. Не переводите данные пользователя и протокольные ключи. См. [локализацию](../docs/localization.md).
- **`localStorage` — только для удобств отдельного устройства;** всё важное хранится в бэкенде.
- **Длинные адреса i2p (516+ символов без пробелов) ломают раскладку.** Фикстуры в `dev/mock.ts` это ловят, проверяйте в превью.

## Как проверить

Сборки и выполнение тестов проводятся в GitHub Actions.
Джоба `ui typecheck` в [build.yml](../.github/workflows/build.yml) проверяет
TypeScript и UI-тесты. [Browser router WASM](../.github/workflows/browser-router.yml)
генерирует WASM, собирает `ui/browser-dist`, запускает UI-тесты и live-сценарий
двух браузеров; артефакт `browser-messenger-pwa` предназначен для HTTPS-хостинга.
Текущий статус и границы подтверждения:
[docs/browser-messenger.md](../docs/browser-messenger.md#validation-and-artifacts).

Превью принимает `lang=en` или `lang=ru`, например
`/dev/preview.html?scene=chat&lang=en`. Меняйте язык также в самом приложении.
Документация и оба словаря обновляются вместе с изменением поведения.
