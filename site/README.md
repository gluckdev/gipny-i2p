# Сайт проекта / Project site

## Русский

`site/` — короткая публичная страница: что такое Gipny, что доступно,
ограничения безопасности, скачивание и переход к WASM-клиенту.
Это не сервер переписки. Основной язык — русский, английский переключается
в интерфейсе и имеет тот же набор ключей.

- `index.html`: семантическая разметка, fallback-текст и ссылка `./app/browser.html`.
- `main.js`: RU/EN тексты, выбор языка и определение ОС/архитектуры локально,
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
[Зеркала Tor/I2P с префиксом gipny](../docs/site-mirrors.md) пока только в плане.

[Pages workflow](../.github/workflows/pages.yml) собирает сайт и WASM UI из одного
SHA, помещает клиент в `_site/app/`, проверяет публичную конфигурацию через
`scripts/prepare-browser-config.py`. Переменная репозитория
`GIPNY_BROWSER_CONFIG_JSON` публикуется как `app/browser-config.json`.
Это **публичные** данные, не место для master/API credentials. TURN credentials
должны быть ограниченными и обновляемыми. Без конфигурации доступна только
локальная offline-работа, сетевую готовность страница не обещает.

Внешние snapshot, WSS/Rust bridge и TURN настраивает оператор;
см. [размещение](../docs/browser-messenger.md#hosting-preparation).
Публикация Pages разрешена workflow только для доверенного `main`, после
проверок. Подготовка исходников не означает deploy. App builds и executable UI
checks выполняются в GitHub Actions; локально допустимы проверки текстов/данных.

Текст не должен обещать неразличимость трафика, защиту от захваченной ОС,
гарантированное удаление у получателя или реализованные лишь в планах функции.
WSS через CDN и WebRTC через ICE/TURN объясняются отдельно.

## English

`site/` is the short public landing page: purpose, available features, security
limits, downloads and entry to the WASM messenger. It is not a messaging server.
Russian is primary; English uses the same translation keys.

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
transport and limits of restarting without hosting. [Tor/I2P mirrors with gipny
prefixes](../docs/site-mirrors.md) are planned, not deployed.

The Pages workflow builds the site and WASM UI from one SHA and places the app
under `_site/app/`. The validated repository variable
`GIPNY_BROWSER_CONFIG_JSON` becomes public `app/browser-config.json`; never put
master/API secrets there. Use bounded, renewable TURN credentials. Without a
network configuration the client provides local offline functionality only.
Operators separately provide the netDb snapshot, Rust bridge and TURN service.
Deployment requires trusted main and passing workflow checks. Preparing sources
is not deployment; application builds/tests run in GitHub Actions.

Copy must distinguish implemented behavior, plans and validation. It must not
promise traffic indistinguishability, protection from a compromised OS, or
forced erasure at a recipient. CDN WSS and ICE/TURN RTC are distinct paths.
