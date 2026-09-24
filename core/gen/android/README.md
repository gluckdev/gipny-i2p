# core/gen/android — Android-обёртка приложения (Gradle-проект Tauri)

Проект сгенерирован Tauri, но правлен руками и хранится в git. Rust-часть приложения собирается в `libgipny_lib.so` и линкуется с готовым `libi2pd.so` (libi2pd + прослойка `i2p-embed/shim`, собирается из `android-router/jni`): роутер запускает и использует Rust-сторона, в том же процессе, как на десктопе (`libcore/src/embedded.rs`). Ни SAM, ни HTTP-прокси, ни портов.

## Что здесь

| Путь | За что отвечает |
|---|---|
| `app/src/main/java/app/gipny/MainActivity.kt` | Активити Tauri; один раз просит исключить приложение из оптимизации батареи (`maybeRequestBatteryWhitelist`) |
| `app/src/main/java/app/gipny/GipnyService.kt` | Foreground-сервис: держит процесс живым и сообщает роутеру о смене сети (`watchNetwork` → `nativeNetworkChanged`). Грузит `libi2pd.so` (`System.loadLibrary("i2pd")`); роутером не управляет |
| `app/src/main/AndroidManifest.xml` | Разрешения, сервис, провайдер файлов |
| `app/src/main/res/` | Строки, темы, иконки (mipmap генерирует `tools/gen-icons.sh`), `xml/file_paths.xml` |
| `app/build.gradle.kts` | Сборка приложения; подкладка роутера (свойства `skipRouter`, `routerAbis`) |
| `buildSrc/.../RustPlugin.kt`, `BuildTask.kt` | Сборка Rust-крейта под ABI (от Tauri) |
| `buildSrc/.../I2pdRouterTask.kt` | Копирует готовый `libi2pd.so` в `jniLibs`: сначала из `$GIPNY_I2PD_JNILIBS/<abi>/`, иначе из `android-router/prebuilt/<abi>/` |
| `build.gradle.kts`, `settings.gradle`, `gradle.properties`, `gradle/wrapper/` | Корень Gradle |

## Куда вносить правки

| Задача | Где |
|---|---|
| Поведение сервиса, реакция на смену сети | `GipnyService.kt` |
| Флаги роутера на телефоне (транзит и прочее) | `libcore/src/embedded.rs` — общие с десктопом |
| Снимок netDb для первого запуска | вшивается в libcore при сборке (`GIPNY_NETDB_SEED`, шаг в `release.yml`): Rust не читает assets APK |
| Разрешения и компоненты | `AndroidManifest.xml` |
| Откуда берётся роутер при сборке, какие ABI | `I2pdRouterTask.kt`, `app/build.gradle.kts` |
| Подпись релизного APK | `release.yml`, шаг `configure Android signing` (секреты `ANDROID_KEY*`) |
| Имя, цвета, темы | `res/values*/` |
| Точка входа JNI (одна: `nativeNetworkChanged`) | `android-router/jni/gipny_i2pd_jni.cpp` **и** `external fun` в `GipnyService.kt` |
| Как Rust линкуется с `libi2pd.so` | `I2P_EMBED_PREBUILT_DIR` в `i2p-embed/build.rs`, шаг в `release.yml` (джоба `android`) |

## Инварианты и грабли

- **Сборка без роутера — ошибка.** Чтобы собрать без него, передайте `-PskipRouter`, или в CI `ORG_GRADLE_PROJECT_skipRouter=true`: CLI Tauri не пробрасывает аргументы в Gradle. Такой APK ставится и запускается, но в сеть не выходит; Rust-часть тогда собирается с заглушкой прослойки (`I2P_EMBED_STUB=1`), как в `build.yml`.
- **Имя JNI-функции привязано к пакету и классу** (`Java_app_gipny_GipnyService_nativeNetworkChanged`). Переименование `GipnyService` ломает сообщение о смене сети.
- **Релизный APK обязан содержать** `lib/<abi>/libi2pd.so` и `libgipny_lib.so`, слинкованную с ней, и ровно один ABI. `release.yml` (шаг `verify signed APKs`) это проверяет. Сертификаты reseed и снимок netDb вшиты в Rust-сторону, в assets от роутера ничего нет.
- **В релизе ABI `arm64-v8a` и `armeabi-v7a`,** по отдельному APK на каждый. x86_64 только в `build.yml` для эмулятора.
- **Проверка и скачивание обновлений работают через i2p** (свой поток к outproxy, `libcore/src/i2p_http.rs`), а APK ставит сам пользователь: файл сохраняется через *Настройки → Android-приложение* и открывается системным установщиком.

## Как проверить

Локально Android не собирается. `build.yml` (джоба `android APK`) проверяет сборку и упаковку всех трёх ABI без роутера (заглушка `I2P_EMBED_STUB`), `i2p-embed.yml` (джоба `android`) собирает `libi2pd.so` для arm64 и линкует с ней Rust, `release.yml` (джоба `android`) собирает подписанные APK с роутером. Rust-часть проверяется так: `env -u CC -u CXX cargo check -p gipny`.
