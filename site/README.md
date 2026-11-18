# Сайт проекта / Project site

## Русский

`site/` — короткая публичная страница: что такое Gipny, что доступно,
ограничения безопасности, скачивание и переход к WASM-клиенту.
Это не сервер переписки. Поддерживаются 8 языков (`ru`, `en`, `uk`, `de`, `km`,
`vi`, `hi`, `zh`) с одинаковым набором ключей: язык по умолчанию выбирается по той
же логике, что и в приложении — явный `?lang=` или переключатель на странице →
русский для русской локали или русской раскладки клавиатуры → предпочтительный
системный язык из списка (`navigator.languages` и локаль `Intl`), причём если
первый совпавший язык — `en`, а в системе обнаружена неанглийская раскладка
клавиатуры (`navigator.keyboard.getLayoutMap()`), выбирается язык раскладки →
язык системной раскладки → `en`. Без JavaScript показывается русский.
Секция «Как это выглядит» берёт снимки из `assets/screens/` (демонстрационный
профиль `ui/dev/demo.ts`, `?demo=1`); снимок подбирается под язык через `:lang()`.

- `index.html`: семантическая разметка, fallback-текст и ссылка `./app/browser.html`.
- `main.js`: тексты на 8 языках (`ru`, `en`, `uk`, `de`, `km`, `vi`, `hi`, `zh`), выбор языка по локалям/клавиатуре и определение ОС/архитектуры локально,
  без аналитики и передачи сведений об устройстве.
- `build.mjs`: `_site/`, каталог GitHub Releases и проверка имён/размеров assets.
  Пустой каталог означает отсутствие доступного релиза, без вымышленных ссылок.
- `tests/site.test.mjs`: тексты, локализация, ссылки, каталог и publication policy.
- `tests/browser-check.mjs`: визуальное/браузерное поведение в CI.

Блок загрузки показывает одну ссылку на подходящий нативный пакет и ссылку
на все релизы GitHub. Если архитектура скрыта браузером, посетитель выбирает её
из списка; MacIntel сам по себе не доказывает наличие процессора Intel.
Для Linux предпочтён AppImage; DEB и серверные компоненты остаются в релизах.
Для неподдерживаемой ОС предлагается веб-клиент. Без JavaScript работает
ссылка на все релизы. Раздел WASM объясняет локальное выполнение, отсутствие
сервера переписки GitHub, роль моста и границы повторного запуска без хостинга.
[Зеркала страницы в I2P](../docs/site-mirrors.md) пока только в плане;
действующих адресов нет. Garlic — термин маршрутизации I2P, а не отдельная
доменная зона. [Платформы и проверенные ограничения](../docs/platform-support.md)
отделены от планируемого расширения поддержки.

[Pages workflow](../.github/workflows/pages.yml) собирает сайт и WASM UI из одного
SHA, помещает клиент в `_site/app/`, проверяет публичную конфигурацию через
`scripts/prepare-browser-config.py`. Переменная репозитория
`GIPNY_BROWSER_CONFIG_JSON` публикуется как `app/browser-config.json`.
Это **публичные** данные, не место для master/API credentials. TURN credentials
должны быть ограниченными и обновляемыми. Без конфигурации доступна только
локальная offline-работа, сетевую готовность страница не обещает.

Внешние snapshot, WSS/Rust bridge и TURN настраивает оператор;
см. [размещение](../docs/browser-messenger.md#hosting-preparation).
GitHub Actions публикует проверенный артефакт из `main`. В Chrome проверяются
разметка главной страницы и повторное открытие экрана профиля без сети.
Порядок публикации и проверки доступен в [руководстве по деплою](../scripts/deploy/README.md).

Карточка сетевой защиты описывает текущие трёхузловые туннели приложения
в обоих направлениях и включённое выравнивание кадров. Скорость зависит от I2P;
ускорение и адаптация не объявляются поводом ослаблять эту политику.
Текст не должен обещать неразличимость трафика, защиту от захваченной ОС,
гарантированное удаление у получателя или реализованные лишь в планах функции.
WSS через CDN и WebRTC через ICE/TURN объясняются отдельно.

## English

`site/` is the short public landing page: purpose, available features, security
limits, downloads and entry to the WASM messenger. It is not a messaging server.
All 8 languages (`ru`, `en`, `uk`, `de`, `km`, `vi`, `hi`, `zh`) share the exact
same keys and default selection priority as the app: explicit `?lang=` or page
switch → Russian for a Russian locale or keyboard layout → first matching
preferred system language (yielding to a detected non-English keyboard layout
when the first matched locale is `en`) → detected keyboard layout → `en`. Without
JavaScript the page is Russian. App screenshots in `assets/screens/` come from the demo profile
(`ui/dev/demo.ts`, `?demo=1`) and follow the page language through `:lang()`.

`index.html` contains markup and `./app/browser.html`; `main.js` handles language
and copy, and detects the device locally without sending that information;
`build.mjs` prepares `_site/` and validates the release asset catalogue.
An empty catalogue produces no invented download links. Data/copy checks live in
`tests/site.test.mjs`; browser behavior is tested in CI.

Downloads show one selected native package plus GitHub Releases. Hidden CPU
architecture requires an explicit selection; MacIntel does not prove an Intel
processor. Linux prefers AppImage; DEB and server components remain in Releases.
Unsupported systems are offered the web client. Releases also work without JS.
The WASM section explains local execution, GitHub's hosting role, independent
transport and limits of restarting without hosting. [I2P page mirrors](../docs/site-mirrors.md)
are planned, with no live addresses. Garlic is I2P routing terminology, not a
separate domain suffix. [Platform support and verified limits](../docs/platform-support.md)
distinguish current evidence from planned expansion.

The Pages workflow builds the site and WASM UI from one SHA and places the app
under `_site/app/`. The validated repository variable
`GIPNY_BROWSER_CONFIG_JSON` becomes public `app/browser-config.json`; never put
master/API secrets there. Use bounded, renewable TURN credentials. Without a
network configuration the client provides local offline functionality only.
Operators separately provide the netDb snapshot, Rust bridge and TURN service.
GitHub Actions deploys the tested artifact from `main`. Chrome checks the landing
page layouts and reloads the profile screen offline. See the [deployment guide](../scripts/deploy/README.md).

The network protection card describes current three-hop application tunnels
in both directions and enabled frame padding. Throughput depends on I2P;
performance adaptation must preserve this policy.
Copy must distinguish implemented behavior, plans and validation. It must not
promise traffic indistinguishability, protection from a compromised OS, or
forced erasure at a recipient. CDN WSS and ICE/TURN RTC are distinct paths.
