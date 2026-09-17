# android-router — сборка i2pd для Android в виде JNI-библиотеки `libi2pd.so`

Исходники i2pd и зависимостей берутся из сабмодулей `third_party/i2pd` и `third_party/i2pd-android`. Здесь лежат только наши файлы сборки и две точки входа JNI.

## Что здесь

| Файл | За что отвечает |
|---|---|
| `jni/gipny_i2pd_jni.cpp` | `Java_app_gipny_GipnyService_nativeStartSam` (запуск i2pd с SAM, возвращает ошибку или `null`) и `Java_app_gipny_GipnyService_nativeStopSam`. Заменяет JNI-обёртку апстрима `org.purplei2p.i2pd` |
| `jni/Android.mk` | Какие исходники i2pd, `DaemonAndroid.cpp` и зависимости входят в библиотеку |
| `jni/Application.mk` | Флаги: `-std=c++20`, `-DNO_TORRENTS`, без UPnP, `c++_static` |
| `prebuilt/<abi>/libi2pd.so` | (не в git) Место для локально подложенной библиотеки; сюда же её скачивает `release.yml` |

## Куда вносить правки

| Задача | Где |
|---|---|
| Сигнатура или поведение запуска и остановки роутера | `jni/gipny_i2pd_jni.cpp` + `external fun` в `core/gen/android/app/src/main/java/app/gipny/GipnyService.kt` |
| Набор исходников и библиотек | `jni/Android.mk` |
| Флаги компилятора и фичи i2pd | `jni/Application.mk` |
| Версия i2pd | Пин сабмодуля `third_party/i2pd` (двигает джоба `bump` в `e2e-i2pd.yml`) |
| Ревизия Boost-for-Android, NDK | `BOOST_FOR_ANDROID_REV`, `NDK_VERSION` в `release.yml` (джоба `android-router`) и `i2pd-build.yml` |

## Инварианты и грабли

- **Имена JNI-символов должны совпадать с пакетом и классом Kotlin.**
- **`NO_TORRENTS` обязателен.** BitTorrent-клиент апстрима требует `boost_json`, а `build_boost.sh` эту библиотеку не собирает.
- **Холодная сборка идёт около часа на ABI** (Boost и OpenSSL из исходников). `release.yml` кэширует результат по пину i2pd, NDK, ревизии Boost и хэшу `android-router/jni/**`, поэтому правка здесь сбрасывает кэш.

## Как проверить

Только в CI: `i2pd-build.yml` (джобы `android …`, все три ABI) или джоба `router (i2pd, android …)` в `release.yml`. Что библиотека реально попала в APK, проверяет шаг `verify signed APKs` в `release.yml`.
