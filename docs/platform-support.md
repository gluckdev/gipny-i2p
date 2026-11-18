# Поддержка платформ и установщики

Состояние на **2026-11-16**. Здесь отдельно указаны собираемые пакеты,
проверенный runtime и планируемое расширение. Alpha2 не публикуется; beta
требует общего проверенного SHA, межплатформенной совместимости, адаптивных
network/UI и защищённого переноса профиля. Наличие target в компиляторе само
по себе не подтверждает поддержку устройства.

## Текущее покрытие

| Платформа | Сейчас собирается | Фактическая граница и пробелы |
|---|---|---|
| Windows | x86_64 MSVC, NSIS `.exe`; x64 agent. ARM64 app/agent добавлены в CI matrix, первый native build/runtime ещё не выполнен | Rust 1.93.1, Tauri 2.12.5 / Wry 0.57.1, наш shim задаёт `_WIN32_WINNT/WINVER=0x0A00`. Проверенный shipping baseline — x64; ARM64 gate на Windows 11. x86 пакета нет. WebView2 устанавливается стандартным механизмом Tauri; отдельного minimumWebview2Version нет. |
| macOS | Отдельные ARM64 и x86_64 `.app/.dmg`, отдельные агенты | И `core/tauri.conf.json`, и release env задают **15.0**. Ad-hoc signing (`"-"`), не Developer ID/notarization. Intel и Apple Silicon различаются упаковкой, а не сетевым протоколом. |
| Linux GUI | x86_64 и AArch64, AppImage + DEB, builders Ubuntu 22.04 | WebKitGTK 4.1, GTK, desktop session; статический Boost, OpenSSL/zlib из системы сборки/упаковки. AppImage не отменяет glibc/graphics/Secret Service ограничения. Точный максимум требуемых GLIBC/GLIBCXX символов shipping ELF пока не извлечён. |
| Linux agent | amd64 и arm64 GNU tar.gz | Не GUI, но SQLCipher и device binding остаются. Службе нужен доступный Secret Service её пользователя; system/root user не наследует desktop credential store автоматически. |
| Android native | Подписанные ARM64 и ARMv7 APK с настоящим i2pd | `minSdk=24` (Android 7), compile/targetSdk=36; NDK 27.3.13750724. Пакет `app.gipny.i2p`. Native UI build держит syntax floor Chromium 83. Обязателен аппаратный TEE/StrongBox Keystore; один minSdk не гарантирует его наличие. Это не обещание актуальных исправлений безопасности старого WebView. |
| Android x86/x86_64 | Есть debug/stub/emulator plumbing | Отсутствуют shipping APK с доказанным реальным router для этих ABI. Debug x86_64 smoke не равен native I2P compatibility. |
| OpenWrt agent | Static musl AArch64, ARMv7 soft-float, x86_64, i686/SSE2 | Реальные C++/crypto/ABI probes и `--help` под QEMU; не full profile/live I2P на физических роутерах. На обычном OpenWrt защищённый credential provider не предоставлен: runtime может и должен fail closed. |
| iOS/iPadOS native | Shipping target отсутствует | Наличие `cfg(ios)` не означает порт. Нет generated Apple project/release job и нет зарегистрированного iOS DeviceKeyProvider: текущий код вернёт provider unavailable. |
| Browser | Отдельная WASM/Emissary сборка, не C++ i2pd | Build floor Safari/iOS 16.4, Chrome/Edge108, Firefox111. Реальный Chrome live WSS/WebRTC PASS; Playwright WebKit create/reload PASS; physical Safari/WKWebView и Firefox live пока не доказаны. Публичный byte-forwarder/config необходим при browser-owned router. |

Основные собственные источники: `.github/workflows/release.yml`,
`core/tauri.conf.json`, `core/gen/android/app/build.gradle.kts`,
`i2p-embed/build.rs`, `libcore/src/device_binding.rs`,
`ui/vite.config.ts`, `ui/vite.browser.config.ts`, `agent/README.md`,
`docs/openwrt.md`, `scripts/install-agent.{sh,ps1}`, `site/build.mjs`.

Минимум ОС в конфигурации, доступность Rust target, прошедшая сборка, запуск
профиля с credential provider и успешная связь — пять разных утверждений.
В таблице они не подменяют друг друга.

## Android 16 KiB: исправление и незакрытый runtime gate

Проверены именно готовые signed preview APK 2139725, а не промежуточный `.so`:

| APK | Библиотека | ELF LOAD alignment |
|---|---|---|
| ARM64 | `libgipny_lib.so` | `0x4000`, все сегменты |
| ARM64 | `libi2pd.so` | **`0x1000`, все сегменты** |
| ARMv7 | обе `.so` | `0x1000`, все сегменты |

ARM64 router не проходит структурное условие 16 KiB Android. Упаковка
`useLegacyPackaging=true` не исправляет выравнивание ELF. По текущей инструкции
Android NDK≤27 нужны оба linker flags `-Wl,-z,max-page-size=16384` и
`-Wl,-z,common-page-size=16384`; проверять следует каждую native библиотеку,
готовый APK через `zipalign -c -P 16 4` и реальный 16-KiB runtime.
[Официальная инструкция Android](https://developer.android.com/guide/practices/page-sizes).

Собственное исправление сохранено в коммите `97d15b4`:

- `android-router/jni/Android.mk`: оба flags у конечного `libi2pd.so`, без vendor edits.
- `scripts/verify-android-pages.py`: ограниченный ELF/ZIP parser, проверка
  class/machine по фактическим байтам, всех `.so`, LOAD alignment/congruence;
  вызов SDK zipalign; JSON с `runtime_tested:false`.
- `scripts/tests/test_verify_android_pages.py`: 15 regressions PASS, включая
  ложное ABI имя, повреждение ELF, непредусмотренные/повторяющиеся ZIP paths,
  плохую дополнительную `.so`, ограничения размера и central directory до allocation, zipalign failure/timeout.
- `.github/workflows/release.yml`: проверки standalone router после strip и
  подписанного APK; receipts отдельными `android-*-pages-*` артефактами вне
  `gipny-*` release collector; тест verifier в раннем job.

Старый ARM64 APK новым verifier отвергнут на `libi2pd.so`; его Rust library
отдельно проходит. Независимый source review не нашёл blocker. **Новый NDK
build, новый signed APK и runtime 16 KiB ещё не проверены**. ARMv7 сохраняет
корректную 4-KiB политику; 16-KiB Android относится к 64-bit устройствам.

Следующий обязательный gate: fresh APK на Android 15+ image/device, где
`adb shell getconf PAGE_SIZE` возвращает 16384; без режимов 4-KiB compatibility.
Проверить загрузку обеих библиотек, создание/повторное открытие профиля,
Keystore, реальные 3-hop tunnels и сообщение/файл. Повторить на 4-KiB ARM64
и старом ARMv7 устройстве. Руководство Android отдельно описывает
[16-KiB emulator/device testing](https://android-developers.googleblog.com/2024/12/get-your-apps-ready-for-16-kb-page-size-devices.html).

## Целевая матрица beta: пока не заявление поддержки

| Приоритет | Предлагаемая поддержка | Что нужно доказать до заявления поддержки |
|---|---|---|
| P0 | Сохранить Windows x64, оба Mac, оба Linux GUI, Android ARM64/ARMv7 | Полные exact-SHA shipping builds и запуск готовых пакетов, device binding, обмен между платформами, upgrade/lock/reopen. |
| P0 | Android ARM64 на 4/16 KiB | Закрыть дефект выше, runtime обеих конфигураций. minSdk 24 не повышать автоматически; Android 7–9 считать legacy функциональной совместимостью, отдельно от безопасности текущего ОС/WebView. |
| P1 | Windows ARM64 native GUI+agent | Target `aarch64-pc-windows-msvc`, ARM64 MSVC, соответствующие Boost/OpenSSL/zlib/SQLCipher; настоящий ARM64 runner/device и его WebView2. |
| P1 | Один macOS universal DMG наряду с двумя небольшими downloads | Обе slices из одного SHA, все native dependencies правильной архитектуры, общий packaging/signing после объединения, native smoke на обоих Mac. |
| P1 | Linux broader distro support | Удержать oldest build base; fresh Ubuntu 22.04/24.04 и Debian 12/13 с X11/Wayland, Secret Service, проверкой ELF dependencies и AppImage запуском. RPM/Flatpak только отдельными проверенными форматами. |
| P1 | Android x86_64 реальный router | Дополнить native router matrix, Rust+Gradle target и unsigned fixture runtime в x86_64 emulator; shipping только при работающем защищённом key provider. |
| P2 | macOS 12/13/14 как исследуемый lower floor | Пересобрать все native зависимости под выбранный deployment target и проверить на самой старой заявленной ОС. До этого остаётся 15+. Понижение одного Info.plist недостаточно. |
| P2 | Windows x86, Linux ARMv7 GUI | Отдельные полные native dependency chains, современный renderer и достаточная память. Это новые targets, не изменение подписи существующего artifact. |
| Отдельный порт | iOS native ARM64 | Keychain provider, iOS C++ dependency/sysroot build, приложение/Xcode signing, lifecycle/network/file permissions, физический iPhone/iPad. Сейчас не включать как готовую beta платформу. |
| Отдельный runtime проект | OpenWrt на физических устройствах | Защищённый provider и provisioning, измеренная память/CPU/flash, реальная I2P; только потом user-facing support. MIPS/MIPSel/ARMv6/RISC-V не подтверждены. |

Android NDK поддерживает четыре ABI, но ARMv7 не покрывает ARMv5/v6, а
Android x86_64 имеет более высокий SIMD baseline, чем произвольный старый
64-bit PC. Одно имя «ARM» или год выпуска устройства не описывает ABI.
[Официальная таблица Android ABI](https://developer.android.com/ndk/guides/abis).

## macOS universal и bootstrap selector решают разные задачи

Universal Mach-O действительно содержит ARM64+x86_64; macOS выбирает нужную
slice. Каждая статическая/динамическая зависимость должна соответствовать
slice. Сборка universal на Intel возможна, но native ARM запуск на Intel
невозможен. Rosetta не заменяет native ARM проверку.
[Apple: universal binaries](https://developer.apple.com/documentation/apple-silicon/building-a-universal-macos-binary.md).

В нашей release matrix логично сохранить независимые native builds для тестов
и provenance, добавить packaging job `lipo`/universal bundle с проверкой двух
архитектур, затем подпись/notarization и DMG. Альтернатива — Tauri
`--target universal-apple-darwin` с обоими Rust targets и двумя настроенными
native dependency prefixes. Текущий один `brew --prefix` не обеспечивает
кросс-архитектурные библиотеки автоматически. Homebrew source build также
сейчас идёт до задания deployment target в GITHUB_ENV: lower-floor job обязан
контролировать deployment target **при сборке зависимостей**.

Не объединять `.dmg` файлы: объединяется Mach-O и корректно собирается app
bundle. Не заявлять, что universal снижает minimumSystemVersion: сейчас 15.0.
Framework default10.13 — лишь значение Tauri config, не доказанный минимум
нашего C++/Rust/WebKit приложения.
[Tauri: minimum system version](https://v2.tauri.app/distribute/macos-application-bundle/).

На macOS сейчас ad-hoc подпись; для обычной browser-download установки без
предупреждений нужен отдельный Developer ID/notarization процесс и credentials
владельца. Их отсутствие не исправляется выбором архитектуры.
[Tauri: signing/notarization](https://v2.tauri.app/distribute/sign/macos/).

Один Windows/Linux файл не станет universal от названия. Удобный «Скачать»
может выбирать существующий проверенный пакет по OS/arch, сохраняя ручной
выбор. Сайт не должен угадывать Apple Silicon по одному macOS user-agent:
при неопределённости предложить universal или выбор Intel/Apple Silicon.
Shell installer теперь учитывает `sysctl.proc_translated` и
`hw.optional.arm64`: на Apple Silicon даже из x86_64 shell под Rosetta он
выбирает ARM64 agent. Оба установщика требуют точное имя существующего asset
и единственный `SHA256SUMS`; отсутствие ARM64 файла не вызывает перехода на
x64 пакет. Unix fixture checks с Intel, native ARM64 и Rosetta проходят.
Это проверка выбора пакета, а не запуск на физическом Mac под Rosetta.

Windows installer определяет архитектуру самой ОС через `IsWow64Process2`
(на старой системе без этой API — `GetNativeSystemInfo`), независимо от
разрядности PowerShell и переменных окружения. Если релиз не содержит
`windows-arm64.tar.gz`, он сообщает об отсутствии нативного пакета до
загрузки и установки. Windows ARM64 shipping target теперь настроен в CI,
но готового проверенного ARM64 пакета ещё нет.
Новые PowerShell fixtures включены в существующий installer test script;
их выполнение на Windows CI после этого изменения ещё требуется.

Новый Windows build glue разделяет x64/ARM64 MSVC и pinned vcpkg static-md,
cache keys и Cargo output paths. Shipping gate устанавливает NSIS в disposable
runner, сравнивает установленный executable с исходным и проверяет PE machine
app/agent/app-local DLL. Сам NSIS launcher остаётся x86 по контракту Tauri.
ABI/installation receipt сохраняется отдельно от shipping assets, его hash
включён в build provenance. Эти steps подготовлены, но Windows CI ещё не
запускался; profile/DPAPI/live/upgrade runtime остаются отдельными gates.
Локальные checks: PE verifier 6, dependency fixtures 3, updater 27, site 27
и provenance 10 tests PASS. PowerShell runtime ими не проверен.

## Windows и Linux: границы зависимостей

В нашем Cargo.lock уже Tauri 2.12.5; релиз Tauri 2.12 официально убрал Windows7.
Не переносить старый раздел документации про Win7 installer в текущую
поддержку GIPNY. Rust target MSVC тоже обозначает Windows 10+.
[Tauri 2.12](https://v2.tauri.app/blog/tauri-2.12/),
[Rust platform support](https://doc.rust-lang.org/rustc/platform-support.html).

Для Windows beta разумный проверочный baseline — Windows 10 22H2 и Windows 11
с обновляемым WebView2. Microsoft обещает WebView2 updates для Win10 22H2 как
минимум до октября 2028; это не продление поддержки самой ОС. Edge отдельно
указывает SSE3 начиная с 128: CPU/renderer capabilities надо проверять, а не
обещать все старые x86. [Microsoft supported OS](https://learn.microsoft.com/en-us/deployedge/microsoft-edge-supported-operating-systems).

Tauri умеет ARM64 native executable; NSIS installer при этом остаётся x86 и
работает под эмуляцией. Для offline установки существует вариант с включённым
WebView2 runtime; текущий малый online installer требует доступ к загрузке
runtime, если его ещё нет. Не предлагать `skip` как решение проблемы установки.
[Tauri Windows installers](https://v2.tauri.app/distribute/windows-installer/).

Linux AppImage не универсален по glibc. Tauri рекомендует строить на старейшей
поддерживаемой системе с WebKitGTK 4.1; Ubuntu 22.04 и Debian 12 — подходящие
примеры. ARM AppImage требует ARM build/emulation. Поэтому upgrade builder
до ubuntu-latest ради удобства может незаметно поднять minimumOS.
[Tauri AppImage limitations](https://v2.tauri.app/distribute/appimage/).

Наши дополнительные Linux ограничения: `/usr/bin/secret-tool`, unlocked
Secret Service/DBus session и graphics stack. DEB объявляет `libsecret-tools`,
AppImage не создаёт пользователю защищённый credential store. Headless
systemd service нужен явный provisioning, а не plaintext keyfile fallback.
Перед расширением distro claims сохранять ELF `DT_NEEDED`, GLIBC/GLIBCXX
versions и package dependencies в compatibility receipt; запускать тот же
downloaded artifact в минимальной VM. Container/QEMU-user не проверяет другой
kernel и полный desktop login/keyring сценарий.

## iOS, браузер и защищённое хранилище

Нативный iOS пока блокирует наш код: `device_binding::provider()` имеет только
Windows/macOS/Linux, Android регистрирует JNI provider отдельно; iOS аналога
нет. i2p-embed build glue не задаёт iOS SDK/sysroot/frameworks/dependencies и
ветку lifecycle для порта. Нельзя объявить поддержку, просто добавив target.
macOS ARM64 static libraries не являются iOS ARM64 libraries.

Следующая отдельная задача iOS: зарегистрировать Keychain provider с явной
политикой non-sync/device binding; создать own Apple project; собрать i2pd,
Boost/OpenSSL/zlib/SQLCipher под device и simulator SDK; затем signing и
physical-device тесты. Tauri подтверждает требования signing/provisioning
для end-user устройств. [Tauri iOS signing](https://v2.tauri.app/distribute/sign/ios/).

Нельзя обещать постоянно работающий I2P router в фоне iOS: Apple предоставляет
ограниченное время завершения фоновых задач и может suspend/terminate app.
Звонки, push и фоновая доставка требуют отдельного privacy/lifecycle дизайна;
не подменять browser-owned/native-owned router серверным ядром.
[Apple background strategies](https://developer.apple.com/documentation/backgroundtasks/choosing-background-strategies-for-your-app.md).

Browser вариант способен расширить охват без нативного installer, но требует
Worker WebAssembly, OPFS/crypto capabilities и доступного публичного byte
forwarder. Floor сборки не является гарантией фактического API availability:
проверять в том Worker, где работает приложение, и объяснять отсутствующую
возможность без отключения защит браузера. Chrome сейчас требует Android 10+;
старые Android 7–9 native APK/старый WebView и современный browser route —
разные обещания. [Chrome system requirements](https://support.google.com/chrome/answer/16737843?hl=en).

Текущая native UI компилируется для `chrome83`, что не тестирует WKWebView и
WebKitGTK отдельно. Beta matrix должна включить реальный Tauri renderer:
macOS WKWebView, Linux WebKitGTK, Windows WebView2 и vendor Android WebView.
Playwright WebKit — полезная regression, но не физический iOS/WKWebView gate.

Device binding уже запрещает открыть raw-copy профиль на другом credential
provider. Новый installer не должен мигрировать ключи обходом этого условия.
Защищённый экспорт/импорт требует отдельного протокола: локальная
аутентификация, зашифрованный bounded container, validation до публикации,
новый destination credential binding, rollback при сбое. Проверка этой
функции выполняется отдельно; она не считается готовой по результатам проверки установщиков.

## План расширения собственных сборок и проверок

1. Завершить Android page patch новым native build и 4/16-KiB runtime gates;
   включить в `.github/workflows/build.yml` или отдельный Android compatibility
   workflow запуск готового реального router APK. Stub smoke оставить отдельным.
2. Ввести один machine-readable compatibility manifest: platform, arch, ABI,
   minOS, renderer floor, credential provider, artifact, measured status. На
   его основе валидировать `.github/scripts/release-notes.sh`, `site/build.mjs`,
   installer selectors и release provenance. Сейчас там несколько жёстких
   списков: новый ARM64 Windows файл иначе останется неизвестным сайту.
3. Native architecture detection и точный unsupported-asset ответ реализованы
   в установщиках (371c5f0). Windows ARM64 build/security/live matrix,
   updater/catalog/exact asset inventory подготовлены; остаётся выполнить
   genuine native сборку и runtime gates на общем SHA.
4. `scripts/install-agent.ps1` использует только `/releases/latest`; Unix
   installer имеет `--version`. Для beta нужен явный version/channel selector
   с проверкой exact tag и manifest; не менять stable default молча. Сайт и
   updater тоже сейчас intentionally stable-only, beta не появится в latest.
5. Windows ARM64 target-aware native-dependency builder добавлен; проверить
   его на native runner. Для Mac universal — отдельный merge/sign
   job и native validation обеих slices, затем обновить exact asset inventory.
6. Lower macOS floor и дополнительные Linux formats делать отдельными jobs,
   с собственными dependency/version receipts; не редактировать upstream defaults.
7. Добавить cross-platform compatibility suite: создание/lock/reopen профиля,
   Android↔Windows↔Linux↔macOS↔browser authenticated text/files, keyboard/IME,
   DPI/rotation/safe areas, network transition и bounded recovery. Межплатформенный
   успех протокола не доказывает одинаковые возможности UI/фоновой работы.
8. Отдельный upgrade gate: установить предыдущий подписанный пакет поверх
   существующего профиля, подтвердить identity/key access/данные, затем новый.
   Никакого uninstall как части проверки сохранности ключей.

Реально измеренные ресурсы нужны до численных minimumRAM claims: vault KDF в
нашем коде использует 256 MiB desktop и 64 MiB mobile, сверх router/UI/SQLCipher.
Нельзя автоматически ослаблять KDF, padding или 3-hop policy для слабого
устройства. Допустимы bounded queues, concurrency и UI effects, с повторными
измерениями goodput/latency/loss/CPU/RSS и отдельной проверкой privacy.

## Доказательства и пределы baseline

`43891757344` (589ad57, publish=false): Windows/Linux/Android/OpenWrt/source
jobs PASS; macOS ARM отменён системой до выделения runner по подтверждённой
capacity; macOS Intel завершился PASS. Итог run — failure, aggregate release
manifest и checksums не созданы; 17 из 19 shipping files доступны, отсутствуют
ARM Mac DMG и agent. Повтор старого ARM job не запускался: для beta понадобится
новый общий SHA. `43891745805` agent installer regressions PASS на старом SHA;
это не CI-проверка последующего изменения detection в 371c5f0.
Это baseline, не разрешение публикации и не доказательство всех runtime rows.

Публичные certificate digests обоих APK совпали с опубликованными rc3:
`1d0255acd744dd5b5ffa13025c1e7746eef0e67f7377f97690c41f9ee47ff3c0`.
Пакет совпал (`app.gipny.i2p`), versionCode старых и новых равен 5000.
Фактический upgrade/Keystore retention не проводился; перед beta нужно
определить монотонный versionCode и проверить реальную установку поверх.

Исходные артефакты: [preview 43891757344](https://github.com/gluckdev/gipny-i2p/actions/runs/43891757344),
[installer checks](https://github.com/gluckdev/gipny-i2p/actions/runs/43891745805).
Установка через updater не была проверена: GitHub ответил через outproxy,
но `/releases/latest` пока не имеет stable release (`NoStableRelease`).
Preview metadata и Android signing integrity не заменяют эти отсутствующие
испытания.
