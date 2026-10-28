# Android: JNI установщика и подготовка APK

Состояние на 2026-10-28: исправление подготовлено к включению в общую ветку дня,
ещё не выпущено. База — `26487a5`; версии библиотек закреплены lockfile:
`jni 0.21.1`, Tauri `2.11.5`, Wry `0.55.1`.

## Что исправлено

Базовый код не компилировался: неправильные преобразования `AndroidContext`,
ссылки `JString`/`JObject`, отсутствующий `call_void_method`, повторное потребление
URI. Сигнатуры fluent-методов `Intent` также были неверными.

Получение Android context теперь идёт через живой Tauri webview:
`AppHandle` → `with_webview` → `jni_handle().exec`. Этот callback предоставляет
JNI environment и Activity на потоке webview. `ndk_context::android_context()`
не используется: Tauri не инициализирует глобальное состояние этого crate.
Новые unsafe-владельцы JavaVM и Activity не создаются.

Временные JNI references принадлежат local frame. `FileProvider` загружается
через ClassLoader Activity; URI используется один раз. Методы вызываются с
сигнатурами закреплённой версии JNI. Ошибка Java превращается в Rust error,
исключение очищается без печати приватных путей и stack trace.

`ACTION_VIEW` передаёт APK с MIME `application/vnd.android.package-archive`,
`FLAG_ACTIVITY_NEW_TASK` и правом чтения конкретного URI. Manifest содержит
`REQUEST_INSTALL_PACKAGES`. Разрешение источника и подтверждение установки
остаётся за человеком в системном интерфейсе Android.

## Копия для FileProvider

Каталог берётся из `app.path().app_cache_dir()`, который на Android соответствует
`Activity.cacheDir`. Только полностью скопированный и повторно проверенный по
SHA256 файл сохраняется в `cache/updates/` под уникальным именем с расширением
`.apk`. Ошибка копирования или checksum не оставляет установочного файла и не
удаляет проверенный download. Повторная попытка не перезаписывает APK, который
установщик уже может читать.

FileProvider не экспортирован. Он обслуживает `cache/updates/` и приватный
`ExternalFilesDir/Pictures`, необходимый camera capture Wry; доступ ко всему
внешнему накопителю больше не объявляется.

Очередь callback имеет 15-секундный deadline. `InstallerDispatch` атомарно
разрешает гонку `Pending → Running` против `Pending → Cancelled`. Просроченный,
ещё не запущенный callback не начинает установку. Если JNI уже выполняется,
Rust асинхронно ждёт его реальный результат, не удаляя APK по одному таймауту.
Проверенная копия после успешного intent остаётся доступна Android; это cache,
а не подтверждение фактической установки пакета.

## Проверки

На Linux выполнены шесть `update_staging::tests`: успешная копия, изменённый
source/hash mismatch, отсутствующий source, независимость повторных попыток,
отмена ожидающего callback и ожидание уже начавшегося callback.

```bash
env -u CC -u CXX I2P_EMBED_STUB=1 cargo test --locked -p gipny --lib update_staging::tests
```

Реальная cross-проверка Rust-кода для **Android arm64** выполнена официальным
Rust `1.93.1` и NDK `27.3.13750724` (r27d). До исправления воспроизведены 14
ошибок JNI базовой версии; после исправления `cargo check` проходит. Пример
настройки уже установленного NDK на Linux:

```bash
export GIPNY_NDK_TOOLCHAIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
export PATH="$GIPNY_NDK_TOOLCHAIN:$PATH"
export CC_aarch64_linux_android="$GIPNY_NDK_TOOLCHAIN/aarch64-linux-android24-clang"
export AR_aarch64_linux_android="$GIPNY_NDK_TOOLCHAIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
env -u CC -u CXX I2P_EMBED_STUB=1 cargo check --locked -p gipny --lib --target aarch64-linux-android
```

Нужна стандартная библиотека Android от того же toolchain. Одинаковый номер
дистрибутивного rustc и официальной rust-std не гарантирует совместимость
metadata; в проверенном стенде использованы оба официальных компонента.
NDK binutils `*-ar`/`*-ranlib`, необходимые vendored OpenSSL, настроены так же,
как в `build.yml`. Сертификаты берутся из pinned i2pd через `I2P_EMBED_CERTS_DIR`.

`I2P_EMBED_STUB=1` проверяет JNI и Rust-код без линковки реального роутера.
Это не сборка подписанного APK и не проверка установки на устройстве. Для
релиза нужны текущий Android CI, настоящая router library, Gradle packaging
и device smoke с подтверждением установки и повторным запуском приложения.
