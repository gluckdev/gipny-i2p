# core/gen/android — Android-обёртка приложения (Gradle-проект Tauri)

Проект сгенерирован Tauri, но правлен руками и хранится в git. Rust-часть
приложения собирается в `libgipny_lib.so` и линкуется с готовым `libi2pd.so`
(`libi2pd` + C++ shim `i2p-embed/shim`, собирается из `android-router/jni`).
Роутер запускает и использует Rust-сторона в том же процессе, как на десктопе
(`libcore/src/embedded.rs`). Служебные local SAM/proxy/web console отключены;
IP-транспорты роутера остаются. Отдельный daemon не нужен.

## Что здесь

| Путь | За что отвечает |
|---|---|
| `app/src/main/java/app/gipny/MainActivity.kt` | Активити Tauri; «назад» прячет приложение; проверяет исключение из оптимизации батареи, при его отсутствии просит не чаще раза в сутки (`maybeRequestBatteryWhitelist`) |
| `app/src/main/java/app/gipny/GipnyService.kt` | Foreground-сервис: держит процесс живым и сообщает роутеру о смене сети (`watchNetwork` → `nativeNetworkChanged`). Грузит `libi2pd.so` (`System.loadLibrary("i2pd")`); роутером не управляет |
| `app/src/main/AndroidManifest.xml` | Разрешения, сервис, FileProvider с `exported=false` и временным read grant installer |
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
| Проверенная копия APK, доступный каталог и запуск installer | `core/src/update_staging.rs`, `core/src/android.rs`, настройка app cache в `core/src/lib.rs`, `res/xml/file_paths.xml` |
| Откуда берётся роутер при сборке, какие ABI | `I2pdRouterTask.kt`, `app/build.gradle.kts` |
| Подпись релизного APK | `release.yml`, шаг `configure Android signing` (секреты `ANDROID_KEY*`) |
| Имя, цвета, темы | `res/values*/` |
| JNI-вход роутера (`nativeNetworkChanged`) | `android-router/jni/gipny_i2pd_jni.cpp` **и** `external fun` в `GipnyService.kt`; installer JNI отдельно в `core/src/android.rs` |
| Как Rust линкуется с `libi2pd.so` | `I2P_EMBED_PREBUILT_DIR` в `i2p-embed/build.rs`, шаг в `release.yml` (джоба `android`) |

## Инварианты и грабли

- **Сборка без роутера — ошибка.** Чтобы собрать без него, передайте `-PskipRouter`, или в CI `ORG_GRADLE_PROJECT_skipRouter=true`: CLI Tauri не пробрасывает аргументы в Gradle. Такой APK ставится и запускается, но в сеть не выходит; Rust-часть тогда собирается с заглушкой прослойки (`I2P_EMBED_STUB=1`), как в `build.yml`.
- **Имя JNI-функции привязано к пакету и классу** (`Java_app_gipny_GipnyService_nativeNetworkChanged`). Переименование `GipnyService` ломает сообщение о смене сети.
- **Релизный APK обязан содержать** `lib/<abi>/libi2pd.so` и `libgipny_lib.so`, слинкованную с ней, и ровно один ABI. `release.yml` (шаг `verify signed APKs`) это проверяет. Сертификаты reseed и снимок netDb вшиты в Rust-сторону, в assets от роутера ничего нет.
- **В релизе ABI `arm64-v8a` и `armeabi-v7a`,** по отдельному APK на каждый. x86_64 только в `build.yml` для эмулятора.
- **Обновления идут через I2P с обязательным SHA256** (свой поток к outproxy,
  `libcore/src/i2p_http.rs`). Отсутствие или ошибка manifest останавливает
  скачивание, включая ручное сохранение APK.
- **Для installer публикуется отдельная проверенная копия.** Исходник из
  `update_dl` повторно хешируется при копировании в `Context.cacheDir/updates`.
  FileProvider открывает только `cache-path` `updates/`; произвольный внешний
  каталог и корень cache не выдаются. Каждая попытка владеет своим файлом.
  После успешного dispatch копия остаётся для чтения ОС; при ошибке удаляется
  только собственная копия, исходная загрузка сохраняется.
- **JNI installer использует живой Tauri webview context и app ClassLoader.**
  `JNIEnv` и borrowed references не переносятся между потоками; работа
  выполняется в callback Tauri. Таймаут до начала callback отменяет dispatch.
  Android всё равно требует разрешить установку из этого источника и пройти
  системный installer. `UpdateInstallerOpened` означает только открытие
  системного установщика, без отметки установленной версии или restart modal.
- **Подробные журналы отсутствуют в release.** Старые env/settings не включают
  файловый debug journal; UI показывает диагностику только при соответствующей
  capability сборки. [Политика](../../../docs/release-diagnostics.md).

## Как проверить

`build.yml` (джоба `android APK`) проверяет сборку и упаковку трёх ABI без
роутера (`I2P_EMBED_STUB=1`), `i2p-embed.yml` (джоба `android`) собирает
`libi2pd.so` для arm64 и линкует с ней Rust, `release.yml` (джоба `android`)
собирает подписанные APK с роутером. Локальная Android-сборка требует SDK/NDK
и зависимостей тех же workflow; host `cargo check` не проверяет Android JNI.

Host regression для файлов и dispatch без устройства:

```bash
env -u CC -u CXX I2P_EMBED_STUB=1 cargo test --locked -p gipny --lib update_staging::tests
```

На 2026-10-28 эти 6 tests прошли. Изолированный
`cargo check --locked -p gipny --lib --target aarch64-linux-android`
с официальным Rust `1.93.1`, NDK r27d и stub router тоже прошёл.
Он исправляет воспроизведённые 14 JNI compile errors baseline, но не доказывает
сборку полного APK, native router, permission flow или установку на устройстве.
[Снимок объединённой ветки](../../../docs/today-2026-10-28.md).
