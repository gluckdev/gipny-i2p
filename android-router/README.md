# android-router — роутер i2p для Android: `libi2pd.so`

Библиотека собирается из `libi2pd` апстрима (сабмодуль `third_party/i2pd`) и нашей C-прослойки над его `api.h` (`i2p-embed/shim`, та же, что компилируется в десктопные сборки). Boost и OpenSSL под NDK собирают скрипты `third_party/i2pd-android`.

Роутер запускает, использует и останавливает Rust-сторона приложения, в своём процессе: `libgipny_lib.so` линкуется с `libi2pd.so` (`I2P_EMBED_PREBUILT_DIR` в `i2p-embed/build.rs`). Ни демона, ни SAM, ни HTTP-прокси, ни одного порта. `libi2pd_client`, демон с веб-консолью и i18n в библиотеку не входят. Сертификаты reseed и снимок netDb вшиты в Rust-сторону, в assets APK от роутера ничего не лежит.

## Что здесь

| Файл | За что отвечает |
|---|---|
| `jni/gipny_i2pd_jni.cpp` | Единственная точка входа JNI: `Java_app_gipny_GipnyService_nativeNetworkChanged` (смена сети → `gipny_router_set_online`) |
| `jni/Android.mk` | Состав библиотеки: `libi2pd/*.cpp`, `i2p-embed/shim/shim.cpp`, `android-ifaddrs`, статические boost_program_options и OpenSSL |
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
| Ревизия Boost-for-Android, NDK | `BOOST_FOR_ANDROID_REV`, `NDK_VERSION` в `release.yml` (джоба `android-router`) и `i2p-embed.yml` (джоба `android`) |

## Инварианты и грабли

- **Имя JNI-символа должно совпадать с пакетом и классом Kotlin.**
- **Функции прослойки экспортируются** (`GIPNY_API` в `shim.h`): Rust-сторона ищет их в динамической таблице `libi2pd.so`. Проверяют шаги `build libi2pd.so` в `release.yml` и в `i2p-embed.yml`.
- **Холодная сборка идёт около часа на ABI** (Boost и OpenSSL из исходников). `release.yml` кэширует результат по пину i2pd, NDK, ревизии Boost и хэшу `android-router/jni/**` и `i2p-embed/shim/**`, поэтому правка здесь или в прослойке сбрасывает кэш.

## Как проверить

Только в CI: джоба `android` в `i2p-embed.yml` (arm64, на каждый push) или `router (i2pd, android …)` в `release.yml`. Шаг `verify signed APKs` в `release.yml` проверяет, что `libi2pd.so` лежит в APK и что `libgipny_lib.so` с ней слинкована.
