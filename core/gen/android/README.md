# core/gen/android — Android-обёртка приложения (Gradle-проект Tauri)

Проект сгенерирован Tauri, но правлен руками и хранится в git. Rust-часть приложения собирается в `libgipny_lib.so`, роутер i2pd приходит готовым `libi2pd.so` и работает в том же процессе через JNI.

## Что здесь

| Путь | За что отвечает |
|---|---|
| `app/src/main/java/app/gipny/MainActivity.kt` | Активити Tauri; один раз просит исключить приложение из оптимизации батареи (`maybeRequestBatteryWhitelist`) |
| `app/src/main/java/app/gipny/GipnyService.kt` | Foreground-сервис: запускает встроенный роутер (`startEmbeddedRouter` → `nativeStartSam`), готовит каталог и конфиг i2pd с сертификатами reseed (`prepareDataDir`, `copyAssetDir`), останавливает (`stopEmbeddedRouter`). SAM слушает 7656 |
| `app/src/main/AndroidManifest.xml` | Разрешения, сервис, провайдер файлов |
| `app/src/main/res/` | Строки, темы, иконки (mipmap генерирует `tools/gen-icons.sh`), `xml/file_paths.xml` |
| `app/build.gradle.kts` | Сборка приложения; подкладка роутера (свойства `skipRouter`, `routerAbis`) |
| `buildSrc/.../RustPlugin.kt`, `BuildTask.kt` | Сборка Rust-крейта под ABI (от Tauri) |
| `buildSrc/.../I2pdRouterTask.kt` | Копирует готовый `libi2pd.so` в `jniLibs`: сначала из `$GIPNY_I2PD_JNILIBS/<abi>/`, иначе из `android-router/prebuilt/<abi>/` |
| `build.gradle.kts`, `settings.gradle`, `gradle.properties`, `gradle/wrapper/` | Корень Gradle |

## Куда вносить правки

| Задача | Где |
|---|---|
| Поведение сервиса, запуск и остановка роутера | `GipnyService.kt` |
| Конфиг i2pd на телефоне (транзит, порты) | `GipnyService.kt`: `prepareDataDir` |
| Разрешения и компоненты | `AndroidManifest.xml` |
| Откуда берётся роутер при сборке, какие ABI | `I2pdRouterTask.kt`, `app/build.gradle.kts` |
| Подпись релизного APK | `release.yml`, шаг `configure Android signing` (секреты `ANDROID_KEY*`) |
| Имя, цвета, темы | `res/values*/` |
| Точки входа JNI | `android-router/jni/gipny_i2pd_jni.cpp` **и** `external fun` в `GipnyService.kt` |

## Инварианты и грабли

- **Сборка без роутера — ошибка.** Чтобы собрать без него, передайте `-PskipRouter`, или в CI `ORG_GRADLE_PROJECT_skipRouter=true`: CLI Tauri не пробрасывает аргументы в Gradle. Такой APK ставится и запускается, но в сеть не выходит.
- **Имена JNI-функций привязаны к пакету и классу** (`Java_app_gipny_GipnyService_*`). Переименование `GipnyService` ломает загрузку роутера.
- **Релизный APK обязан содержать** `lib/<abi>/libi2pd.so`, `libgipny_lib.so` и `assets/certificates/reseed/`. `release.yml` это проверяет.
- **В релизе ABI `arm64-v8a` и `armeabi-v7a`,** по отдельному APK на каждый. x86_64 только в `build.yml` для эмулятора.
- **HTTP-прокси i2pd на Android выключен,** поэтому автообновление здесь не работает. APK обновляется вручную.

## Как проверить

Локально Android не собирается. `build.yml` (джоба `android APK`) проверяет сборку и упаковку всех трёх ABI без роутера, `release.yml` (джоба `android`) собирает подписанные APK с роутером. Rust-часть проверяется так: `env -u CC -u CXX cargo check -p gipny`.
