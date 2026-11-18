# Сборки Gipny и Hiddify v5.5.5

Сравнение от 18 ноября 2026. Для Hiddify проверены артефакты опубликованного тега, матрица GitHub Actions, цели Makefile и exportOptions.plist. Для Gipny проверены workflows интеграционной ветки. В колонке Gipny указаны цели сборки. Подтверждённые устройства и ограничения перечислены отдельно в [матрице платформ](platform-support.md).

| Платформа | Hiddify v5.5.5 | Gipny beta |
|---|---|---|
| iOS | macOS 26, Flutter. Валидация release без подписи и отдельный запуск simulator. Для release — Fastforge и exportOptions с ручной Apple Distribution подписью и профилями приложения и PacketTunnel. В GitHub Release лежит Hiddify-iOS.ipa с пометкой «iOS (Jailbreak)» | Нативный iOS исключён из beta. В эксперименте Tauri собрал device archive, проверка IPA завершилась ошибкой, сборка Simulator не дошла до запуска. [Результат CI](https://github.com/gluckdev/gipny-i2p/actions/runs/44302046396) |
| Android | Universal APK и три APK для arm64, arm7, x86_64 | Два release APK с реальным i2pd для arm64-v8a и armeabi-v7a. Проверки ELF и выравнивания ZIP на 16 KiB. x86_64 emulator/debug — отдельная сборка |
| Windows | x64 Setup EXE через Inno Setup, MSIX, отдельный portable ZIP. В workflow есть шаги Authenticode для контейнеров и PE | NSIS EXE для x64 и native ARM64 в release matrix. Собственная проверка точного ABI у артефакта и выбор установщика. Подпись manifest/digest проверяется отдельно от доверия Windows к издателю |
| macOS | Universal DMG и PKG, отдельные AppStore DMG и PKG. Релиз указывает минимум 10.15 | Два отдельных DMG с .app для Apple Silicon и Intel. Цель macOS 15, ad-hoc подпись. Developer ID, notarization, universal PKG и App Store — отдельные возможности |
| Linux desktop | x64 AppImage, архив AppImage.tar.gz, Debian x64 DEB | AppImage и Debian DEB для x86_64 и AArch64. Отдельные headless-артефакты для агента и релея |
| Web / OpenWrt | Среди файлов этого тега отсутствуют браузерный мессенджер и OpenWrt-пакеты | PWA с JS/WASM и отдельным сетевым bootstrap, headless OpenWrt agent. Эти цели поставляются независимо от графических desktop-пакетов |

В опубликованном Gipny v0.5.0-rc3 было 22 артефакта. Из графических клиентов перечислены два APK, два DMG, Windows x64 Setup, два AppImage и два DEB. Остальные файлы — агент, исходники, manifests и контрольные суммы. Цель Windows ARM64 добавлена в последующей beta-ветке. [Артефакты Gipny rc3](https://github.com/gluckdev/gipny-i2p/releases/tag/v0.5.0-rc3).

Источники — [релиз Hiddify](https://github.com/hiddify/hiddify-app/releases/tag/v5.5.5), [его workflow](https://github.com/hiddify/hiddify-app/blob/v5.5.5/.github/workflows/build.yml), [цели iOS](https://github.com/hiddify/hiddify-app/blob/v5.5.5/Makefile), [параметры экспорта](https://github.com/hiddify/hiddify-app/blob/v5.5.5/ios/exportOptions.plist), [release workflow Gipny](../.github/workflows/release.yml) и [историческая конфигурация эксперимента iOS](https://github.com/gluckdev/gipny-i2p/blob/e8a236e/.github/workflows/ios.yml).

## Публикация с нулевым бюджетом

Для Gipny выбран публичный GitHub Release с APK и desktop-пакетами. В публичном репозитории вычислительное время стандартного GitHub-hosted macOS runner бесплатно. [Условия GitHub](https://docs.github.com/en/billing/concepts/product-billing/github-actions).

На обычном iPhone одного скачивания unsigned IPA недостаточно. Бесплатная Personal Team в Xcode позволяет владельцу установить приложение на своё устройство. Provisioning profile истекает через 7 дней, после чего требуется повторная установка с обновлённым профилем. Обычная программа распространения Apple стоит 99 USD в год; для отдельных категорий организаций Apple может одобрить освобождение от взноса. Для релиза Gipny на GitHub вступать в программу не требуется. [Установка на личное устройство](https://developer.apple.com/help/account/basics/about-your-developer-account/) · [Условия Apple](https://developer.apple.com/help/account/membership/fee-waivers/).

Для пользователей iPhone доступен также [веб-клиент](https://gluckdev.github.io/gipny-i2p/app/browser.html). На 18 ноября его публичная конфигурация моста отсутствовала; для переписки через I2P требуется рабочее подключение. [Состояние браузерного развёртывания](browser-network-deployment.md).

## English

Gipny distributes Android and desktop packages through GitHub. Hiddify v5.5.5 also published an iOS IPA. Gipny’s experimental unsigned device archive built, but its verification failed and the simulator build failed. The project removed native iOS from the beta release scope. The browser client remains a separate delivery option with its own network and device requirements.
