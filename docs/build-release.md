# Сборки и происхождение релизов

Требование владельца, уточнено **2026-10-28**: все дальнейшие сборки GIPNY
выполняются только в **GitHub Actions** публичного
[gluckdev/gipny-i2p](https://github.com/gluckdev/gipny-i2p). Слово GitLab в
предыдущем указании было оговоркой. GitLab остаётся местом публикации библиотеки
AiBlow, а не сборок приложения. Локальные результаты предыдущих проверок —
исторические наблюдения, их нельзя выдавать за новый CI результат.

## Что собирается

`release.yml` компилирует код текущего публичного commit, pinned submodules и
зависимости из публичных исходников. Готовые внешние `gipny`, `gipny-agent`,
`gipny-relay`, `i2pd` или `libi2pd.so` как замена source build не скачиваются.

| Платформа | GitHub runner | Результат |
|---|---|---|
| Linux x86_64 | `ubuntu-22.04` | AppImage, deb, GNU agent и relay |
| Linux arm64 | `ubuntu-22.04-arm` | AppImage, deb, GNU agent и relay |
| Windows x64 | `windows-latest` | NSIS и agent; MSVC, статические зависимости vcpkg |
| macOS arm64 | `macos-15` | DMG и agent |
| macOS Intel | `macos-15-intel` | DMG и agent |
| Android arm64 / armv7 | `ubuntu-22.04` с NDK | Собственный `libi2pd.so`, затем APK каждого ABI |
| OpenWrt / musl, четыре ABI | `ubuntu-24.04` с cross toolchain | Статический agent; ELF gates и QEMU/native CPU smoke |

macOS и Windows собираются на настоящих macOS/Windows runners, а не объявляются
результатом Linux cross-check. Названия macOS runners сверены с
[официальной матрицей GitHub](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
OpenWrt не означает любой роутер: точные ABI и ограничения перечислены в
[openwrt.md](openwrt.md).

Rust закреплён на `1.93.1`, Android NDK — `27.3.13750724`. `tauri-cli 2.12.2`
компилируется через `cargo install --locked` из публичного crate; fallback на
готовый CLI удалён. Прямые shipping Cargo builds используют `--locked`, UI —
`npm ci`; итоговый gate отклоняет изменения reviewed lockfiles при Tauri build. Windows ports закреплены на публичном vcpkg commit
`9eec002df1cd109f8f104cd966a763daf9751489` (peeled commit тега `2026.10.16`),
внешний vcpkg binary registry отключён. SHA объекта annotated tag отличается
от SHA commit: guard проверяет тип объекта до checkout и точный `HEAD` после.
Первый preview на `a66d223` выявил такую ошибку pin и остановил Windows job
до сборки зависимостей; следующий preview должен подтвердить исправление.

Windows memory-security CI также выявил upstream SQLCipher logger recursion
при `VirtualLock` quota failure. Workspace и отдельный relay manifest/lock
обновлены на rusqlite 0.41.1 / libsqlite3-sys 0.39.1; SQLCipher 4.19.0 приходит
как публичный source crate `sqlcipher-src`, не готовая DLL. Обе lockfile должны
оставаться согласованными. Отдельный Windows child-process regression
форсирует lock failure при включённом `cipher_memory_security`.
macOS запрашивает source build Boost/OpenSSL через Homebrew. OpenWrt проверяет
SHA256 официального toolchain, Boost, OpenSSL source crate и zlib до сборки.

Windows jobs явно включают MSVC x64 environment и native Strawberry Perl
через `windows-source-tools.ps1`. Vendored OpenSSL использует абсолютный
`OPENSSL_SRC_PERL`, чтобы Git Bash не подменил его MSYS Perl с несовместимыми
путями/неполным набором модулей. Это выбор публичного инструмента сборки,
а не загрузка готовой библиотеки приложения.

Официальные Rust/Node/NDK, MSVC, Xcode, Android SDK/JDK, операционная система и
инструменты упаковки являются **инструментами сборки**. Они могут поставляться
издателем готовыми; MSVC/Xcode/platform SDK не обещаются свободными или
собранными самим проектом. Требование к открытому приложению не превращается в
ложное обещание полностью свободного SDK. Состав и версии доступных инструментов
фиксируются в manifest. Это также не обещание побитово воспроизводимого релиза:
runner images, системные пакеты, Homebrew и подписи требуют отдельной работы.

## Source archive, manifest и контрольные суммы

Перед публикацией создаются:

- `gipny-source_<version>_<commit>.tar.gz` — committed исходники приложения и
  всех рекурсивных git submodules. Нельзя подменить gitlink другим checkout;
  `.git`, credentials, `target/`, локальные signing keys и downloaded binaries
  в этот архив не берутся;
- `SOURCE-MANIFEST.json` — публичный Git SHA, gitlinks и SHA256 source archive;
- `BUILD-MANIFEST.json` — per-job target, CI run/attempt, версии инструментов,
  lockfile hashes, actual source commits и точный SHA256/размер каждого asset;
- `SHA256SUMS.txt` — суммы всех опубликованных пакетов, source archive и manifests.

Android build manifest содержит provenance `libi2pd.so` из router job того же
workflow SHA. Файл перед сборкой APK и после неё повторно проверяется по хэшу;
внешний native binary не считается равноценным input. Router application-code
cache в релизе не используется. Зависимости могут кэшироваться по source pins.
Android выравнивает nested i2pd на desktop pin и Boost-for-Android на явный
`BOOST_FOR_ANDROID_REV`; actual commits отражаются в build manifest. Source
archive хранит исходные gitlinks; публичные override commits и преобразование
API 16 → 21 зафиксированы в workflow, а не скрыты внутри бинарника.

Сертификаты reseed берутся из публичного upstream; их точный commit сохраняется
как `GIPNY_RESEED_SOURCE_SHA`. Снимок netDb содержит публичные RouterInfos,
а не executable и не локальную identity клиента. Crates/npm и платформенные
frameworks получают исходники/пакеты по lockfiles; source archive не обещает
включать весь внешний SDK и каждый registry package.

Сборка с `I2P_EMBED_STUB=1`, `I2P_EMBED_SKIP_NATIVE=1` или release debug assertions
не проходит shipping provenance gate. Debug stub APK из `build.yml` служат только
проверке UI/lifecycle; в release assets они не попадают.

Финальная collection падает при отсутствующем/повреждённом/лишнем artifact,
повторе basename, несоответствии workflow SHA или отсутствии provenance.
`release-notes.sh` дополнительно требует полную платформенную матрицу. Каждый
platform job должен завершиться успешно до публикации; partial release не
объявляется готовым. SHA256 из той же release page проверяет целостность;
это не независимая подпись издателя.

## Как запускать и проверять

В GitHub Actions → **release** → **Run workflow** можно выбрать проверяемую ветку.
Ручной запуск создаёт preview assets/страницу и ничего не публикует. Автоматическая
публикация выполняется только по установленному контракту version bump в `main`;
пуш в `main`, публикация и смена production не являются частью подготовки PR.

Для PR запускаются `build.yml`, installer/provenance integrity, OpenWrt и
профильные native/I2P workflows. `security-fast.yml` параллельно проверяет
Rust callers и unit tests на Linux, Windows, macOS arm64/Intel с заглушкой роутера,
а Android arm64 job проверяет Rust/JNI types без часовой C++ сборки. Никакие
application artifacts из этой проверки не публикуются. `build.yml` дополнительно
собирает Kotlin instrumentation APK; компиляция теста не доказывает наличие TEE,
прохождение hardware-positive тестов или реальный запуск на телефоне.
Локально допустимы чтение кода, YAML/shell syntax,
ссылки и проверки данных без компиляции приложения; `cargo build/test/check`,
Rust probes, `npm run build` и сборка APK выполняются в Actions.

Этот документ описывает реализацию в рабочей ветке на 2026-10-28. Новая полная
матрица ещё должна пройти GitHub CI после публикации согласованного commit.
Unit/host check, compile/link и реальная установка/обновление на устройстве —
разные результаты. Платформенный lifecycle описан в
[platform-updates.md](platform-updates.md).
