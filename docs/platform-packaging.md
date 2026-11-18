# Сборки Gipny и Hiddify v5.5.5

Сравнение от 18 ноября 2026. Для Hiddify проверены артефакты опубликованного тега, матрица GitHub Actions, цели Makefile и exportOptions.plist. Для Gipny проверены workflows интеграционной ветки. В колонке Gipny указаны цели сборки. Подтверждённые устройства и ограничения перечислены отдельно в [матрице платформ](platform-support.md).

| Платформа | Hiddify v5.5.5 | Gipny beta |
|---|---|---|
| iOS | macOS 26, Flutter. Валидация release без подписи и отдельный запуск simulator. Для release — Fastforge и exportOptions с ручной Apple Distribution подписью и профилями приложения и PacketTunnel. В GitHub Release лежит Hiddify-iOS.ipa с пометкой «iOS (Jailbreak)» | macOS 26, XcodeGen, Tauri CLI 2.12.2, Rust и C++ i2pd. Отдельные arm64 device и arm64 simulator. CI формирует unsigned IPA, проверяет bundle ID, архитектуру и платформу Mach-O, запускает отдельную сборку в Simulator. Для подписанной дистрибуции требуются сертификат и профиль владельца Gipny |
| Android | Universal APK и три APK для arm64, arm7, x86_64 | Два release APK с реальным i2pd для arm64-v8a и armeabi-v7a. Проверки ELF и выравнивания ZIP на 16 KiB. x86_64 emulator/debug — отдельная сборка |
| Windows | x64 Setup EXE через Inno Setup, MSIX, отдельный portable ZIP. В workflow есть шаги Authenticode для контейнеров и PE | NSIS EXE для x64 и native ARM64 в release matrix. Собственная проверка точного ABI у артефакта и выбор установщика. Подпись manifest/digest проверяется отдельно от доверия Windows к издателю |
| macOS | Universal DMG и PKG, отдельные AppStore DMG и PKG. Релиз указывает минимум 10.15 | Два отдельных DMG с .app для Apple Silicon и Intel. Цель macOS 15, ad-hoc подпись. Developer ID, notarization, universal PKG и App Store — отдельные возможности |
| Linux desktop | x64 AppImage, архив AppImage.tar.gz, Debian x64 DEB | AppImage и Debian DEB для x86_64 и AArch64. Отдельные headless-артефакты для агента и релея |
| Web / OpenWrt | Среди файлов этого тега отсутствуют браузерный мессенджер и OpenWrt-пакеты | PWA с JS/WASM и отдельным сетевым bootstrap, headless OpenWrt agent. Эти цели поставляются независимо от графических desktop-пакетов |

В опубликованном Gipny v0.5.0-rc3 было 22 артефакта. Из графических клиентов перечислены два APK, два DMG, Windows x64 Setup, два AppImage и два DEB. Остальные файлы — агент, исходники, manifests и контрольные суммы. Windows ARM64 и iOS появились в последующей beta-ветке. [Артефакты Gipny rc3](https://github.com/gluckdev/gipny-i2p/releases/tag/v0.5.0-rc3).

Источники — [релиз Hiddify](https://github.com/hiddify/hiddify-app/releases/tag/v5.5.5), [его workflow](https://github.com/hiddify/hiddify-app/blob/v5.5.5/.github/workflows/build.yml), [цели iOS](https://github.com/hiddify/hiddify-app/blob/v5.5.5/Makefile), [параметры экспорта](https://github.com/hiddify/hiddify-app/blob/v5.5.5/ios/exportOptions.plist), [release workflow Gipny](../.github/workflows/release.yml) и [iOS workflow Gipny](../.github/workflows/ios.yml).

## Публикация с нулевым бюджетом

Публичный GitHub Release может содержать APK, desktop-пакеты и unsigned IPA. В публичном репозитории вычислительное время стандартного GitHub-hosted macOS runner бесплатно. [Условия GitHub](https://docs.github.com/en/billing/concepts/product-billing/github-actions).

На обычном iPhone одного скачивания unsigned IPA недостаточно. Бесплатная Personal Team в Xcode позволяет владельцу установить приложение на своё устройство. Provisioning profile истекает через 7 дней, после чего требуется повторная установка с обновлённым профилем. Обычная программа распространения Apple стоит 99 USD в год; для отдельных категорий организаций Apple может одобрить освобождение от взноса. Для релиза Gipny на GitHub вступать в программу не требуется. [Установка на личное устройство](https://developer.apple.com/help/account/basics/about-your-developer-account/) · [Условия Apple](https://developer.apple.com/help/account/membership/fee-waivers/).

Для пользователей iPhone доступен также [веб-клиент](https://gluckdev.github.io/gipny-i2p/app/browser.html). На 18 ноября его публичная конфигурация моста отсутствовала; для переписки через I2P требуется рабочее подключение. [Состояние браузерного развёртывания](browser-network-deployment.md).

## Получение iOS-сборки

Откройте GitHub Actions → **iOS IPA and simulator**. Из успешного device job скачайте артефакт `gipny-iOS-arm64-unsigned`, распакуйте его и выполните `shasum -a 256 -c SHA256SUMS.txt`. Внутри находится `Gipny-iOS-arm64-unsigned.ipa`. В device IPA лежит приложение для физического устройства. Симулятор использует отдельный SDK и отдельный .app из своей сборки. Для установки на обычный iPhone IPA потребуется подписать для этого устройства или выбранного Apple-канала.

При локальной сборке повторите шаги workflow на macOS с Xcode и соответствующим SDK. Генерация Xcode-проекта выполняется из каталога `core` командой `cargo tauri ios init --ci --skip-targets-install`. Команда device-пакета — `cargo tauri ios build --target aarch64 --no-sign`. Для этого процесса сертификат Apple, его пароль и Apple Developer Team не требуются. Владелец выбрал сборку без сертификата.

Реализация использует существующий embedded router, профиль в Library/Application Support и iOS Keychain с `WhenUnlockedThisDeviceOnly`. Уведомления и звонки в фоне, доставка на физическом устройстве, поведение при блокировке и установка подписанного артефакта требуют собственных проверок.

## English

Hiddify v5.5.5 distributes one IPA, Android universal and per-ABI APKs, Windows x64 Setup/MSIX/portable, universal macOS packages and Linux x64 AppImage/DEB. Gipny uses native Tauri, Rust and embedded i2pd. Its iOS workflow builds separate device and simulator targets on macOS, verifies the IPA and launches the simulator build. The unsigned device artifact is intended for subsequent signing. Apple Distribution requires Gipny’s own certificate, provisioning profile and registered bundle ID. Platform support claims depend on artifact and device tests.
