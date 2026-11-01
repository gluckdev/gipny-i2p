# android-router — роутер i2p для Android: `libi2pd.so`

Библиотека собирается из `libi2pd` апстрима (сабмодуль `third_party/i2pd`) и нашей C-прослойки над его `api.h` (`i2p-embed/shim`, та же, что компилируется в десктопные сборки).
`third_party/i2pd-android/binary/jni/fetch_deps.sh` загружает исходники
зависимостей в `deps/` с проверкой SHA256; `build_openssl.sh arm64|arm`
собирает OpenSSL. Boost program_options компилирует наш `jni/Android.mk`.

На 2026-10-31 подготовлен переход на layout i2pd-android 3.0.0 (`0d2008a`);
проверка настоящей библиотеки с этим layout в GitHub Actions ещё требуется.

Роутер запускает, использует и останавливает Rust-сторона приложения, в своём
процессе: `libgipny_lib.so` линкуется с `libi2pd.so`
(`I2P_EMBED_PREBUILT_DIR` в `i2p-embed/build.rs`). Отдельного daemon нет.
Служебные local SAM/proxy/web console отключены; IP-транспорты роутера
NTCP2/SSU2 и discovery/reseed остаются. `libi2pd_client`, демон с web console
и i18n в библиотеку не входят. Сертификаты reseed и снимок netDb вшиты
в Rust-сторону, в assets APK от роутера ничего не лежит.

## Что здесь

| Файл | За что отвечает |
|---|---|
| `jni/gipny_i2pd_jni.cpp` | Единственная точка входа JNI: `Java_app_gipny_GipnyService_nativeNetworkChanged` (смена сети → `gipny_router_set_online`) |
| `jni/Android.mk` | Состав библиотеки: `libi2pd/*.cpp`, `i2p-embed/shim/shim.cpp`, `ifaddrs`, Boost program_options из `deps/boost/libs/program_options/src/*.cpp`, статический OpenSSL из `deps/openssl/out/<abi>` |
| `jni/Application.mk` | Флаги: `-std=c++20`, без UPnP, `c++_static`; путь к прослойке `SHIM_PATH` |
| `prebuilt/<abi>/libi2pd.so` | (не в git) Место для локально подложенной библиотеки; сюда же её скачивает `release.yml` |

## Куда вносить правки

| Задача | Где |
|---|---|
| API роутера для Rust | `i2p-embed/shim/shim.h`, `shim.cpp` и `i2p-embed/src/lib.rs` — одно на все платформы |
| Реакция на смену сети | `jni/gipny_i2pd_jni.cpp` + `GipnyService.kt` (`watchNetwork`) |
| Набор исходников и библиотек | `jni/Android.mk` |
| Флаги компилятора | `jni/Application.mk` |
| Версия i2pd | Пин сабмодуля `third_party/i2pd` (двигает джоба `bump` в `e2e-i2pd.yml`) |
| Версии Boost/OpenSSL | Версии и SHA256 архивов в `third_party/i2pd-android/binary/jni/fetch_deps.sh`, закреплённом пином сабмодуля `third_party/i2pd-android` |
| Версия NDK | `NDK_VERSION` и пакет `ndk;…` в `release.yml` (джоба `android-router`) и `i2p-embed.yml` (джоба `android`); оба используют `27.3.13750724` |

## Инварианты и грабли

- **Имя JNI-символа должно совпадать с пакетом и классом Kotlin.**
- **Функции прослойки экспортируются** (`GIPNY_API` в `shim.h`): Rust-сторона ищет их в динамической таблице `libi2pd.so`. Проверяют шаги `build libi2pd.so` в `release.yml` и в `i2p-embed.yml`.
- **Кэшируются зависимости, а не роутер.** Оба workflow используют один ключ `android-deps-*`: ABI, NDK, commit i2pd-android и общий хэш `fetch_deps.sh`/`build_openssl.sh`. В кэше — `deps/boost`, `deps/openssl/out/<abi>` и `deps/*.tar.gz`; пин i2pd и изменения JNI/прослойки его не сбрасывают. Boost program_options и `libi2pd.so` каждый раз компилируются заново.
- **`deps.mk` не подключается.** Его проверка miniupnpc не выполняется. `fetch_deps.sh` всё равно распаковывает miniupnpc; архив попадает в кэш, но исходники miniupnpc, Boost JSON/URL и UPnP в библиотеку не входят.
- **NDK остаётся r27d.** Апстрим указывает `29.0.14206865` в `gradle/libs.versions.toml`, но прочитанные dependency scripts не требуют эту версию. OpenSSL собирается для API 23 без прежней подмены API 16 → 21; наша библиотека — для API 24. Совместимость подтверждает следующий CI-прогон.
- **Сбой C++ делит процесс с приложением.** Восстановление туннелей не является перезапуском отдельного daemon после crash.
- **Политика журналов общая с Rust.** В release подробный native logger и файл i2pd отключены; тестовые и диагностические сборки сохраняют диагностику. [Контракт](../docs/release-diagnostics.md).

## Как проверить

Настоящую библиотеку проверяют джоба `android` в `i2p-embed.yml` (arm64)
и `router (i2pd, android …)` в `release.yml`. Шаг `verify signed APKs`
проверяет, что `libi2pd.so` лежит в APK и что `libgipny_lib.so` с ней
слинкована. По требованию владельца сборки выполняются только в GitHub Actions;
обычный host `cargo check` проверку настоящей библиотеки не заменяет.

На 2026-10-28 изолированно прошёл Rust cross-check для arm64 с официальным
Rust `1.93.1` и NDK r27d **со stub router**. Это проверка Rust/JNI, не этой
настоящей библиотеки, APK или устройства. [Текущий статус](../docs/today-2026-10-28.md).
