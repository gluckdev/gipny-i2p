# Публикация веб-клиента на GitHub Pages

Публичная точка входа — [Gipny](https://gluckdev.github.io/gipny-i2p/).
Мессенджер открывается по [адресу проекта](https://gluckdev.github.io/gipny-i2p/app/browser.html).
Pages раздаёт статические файлы. Браузер запускает WASM и хранит профиль локально.
На 18 ноября 2026 года публичный `app/browser-config.json` возвращал HTTP 404.
При первом запуске в таком состоянии доступны локальные функции. Для доставки
сообщений нужны работающий мост и начальные данные I2P.

## Порядок публикации

1. В настройках репозитория для Pages должен быть выбран источник **GitHub Actions**.
   Изменения сайта, UI, WASM и входных файлов сборки проверяет workflow
   [Project website](../../.github/workflows/pages.yml). Pull request собирает артефакт
   для проверки. Публикация выполняется из `main`.
2. Для ручной публикации откройте **Actions → Project website → Run workflow**,
   выберите `main`. То же действие из командной строки приведено ниже.
   Перезапускайте workflow и после изменения `GIPNY_BROWSER_CONFIG_JSON`.
3. В одном запуске должны успешно завершиться **Validate and build static site**
   и **Deploy reviewed main to GitHub Pages**. В отчёте `site-browser-<run_id>/report.json`
   поле `offlineApp.passed` подтверждает открытие экрана профиля после отключения сети
   на локальной копии артефакта. Адрес результата выдаёт job деплоя.
4. Откройте опубликованный веб-клиент в новом профиле браузера. После первого
   успешного запуска и активации Service Worker перезагрузите страницу без сети.
   Для проверки обновления закройте все вкладки и окна установленной PWA и откройте
   её снова. Очистка данных сайта может удалить профиль и переписку.

```sh
gh workflow run pages.yml --repo gluckdev/gipny-i2p --ref main
gh run list --repo gluckdev/gipny-i2p --workflow pages.yml --branch main --limit 5
```

Для сетевого размещения оператор публикует HTTPS-снимок netDb и WSS-мост, задаёт
их адреса, SHA-256 снимка и capability в публичной переменной репозитория
`GIPNY_BROWSER_CONFIG_JSON`. Режим WebRTC также использует настройки ICE.
Схема и требования описаны в [руководстве веб-клиента](../../docs/browser-messenger.md#hosting-preparation)
и [руководстве моста](../../browser-router/forwarder/README.md). После публикации
проверьте обмен сообщениями между двумя тестовыми профилями. Для проверки доставки
важно получить сообщение на втором устройстве.

Каталог на главной показывает пакеты последнего стабильного выпуска.
Предварительные версии перечислены в [GitHub Releases](https://github.com/gluckdev/gipny-i2p/releases).
Публикация Pages имеет отдельный workflow и жизненный цикл.

## English

The [landing page](https://gluckdev.github.io/gipny-i2p/) and
[WASM messenger](https://gluckdev.github.io/gipny-i2p/app/browser.html)
are served under the repository prefix. On 18 November 2026 the public
`app/browser-config.json` returned HTTP 404. A fresh browser can use local
features in that state. Message delivery requires a working bridge and bootstrap data.

Select GitHub Actions as the Pages source. Pull requests build and test the site.
Changes to the matching paths on `main` trigger publication. Run the two commands
above for a manual deployment, including after a network configuration change.
Check both jobs, the artifact's `offlineApp.passed` result, and the live URL.
In a fresh browser profile, wait for Service Worker activation, then reload
offline. Close all open app windows to activate a waiting update. Clearing site
data can erase profiles and messages.

For online use, follow the [public configuration schema](../../docs/browser-messenger.md#hosting-preparation)
and [bridge guide](../../browser-router/forwarder/README.md). Set the public
`GIPNY_BROWSER_CONFIG_JSON` repository variable, redeploy, and confirm receipt
between two test profiles. The landing page selects the newest stable native
release. Prereleases are available from GitHub Releases.
