# Привязка хранилища к устройству

Требование владельца от **2026-10-28**: копия файлов телефона на ПК не должна
позволять открыть профиль. В рабочей ветке `feat/today-integration-20261028`
введён новый формат; это пока описание исходного кода, а не завершённая
сертификация устройств или результат новой сборки.

## Формат v2 и граница доверия

В старой реализации `DeviceBind::ensure` записывал случайные 32 байта открытым
hex в `.device` рядом с `vault.bin`. Файл переносился вместе с профилем, поэтому
не давал защиты от копирования. Формат v2 не использует этот файл и не допускает
тихий fallback на `0600` keyfile, пароль, hostname или публичный `machine-id`.

`vault.bin` теперь имеет `GIPNY002`, версию `2` и отдельный HKDF domain
`gipny/kek/v2`. В `.device-binding-v2.json` сохраняются только public random ID,
тип OS-провайдера, ciphertext при необходимости и HMAC проверки привязки.
Значение HMAC/ID не является секретом. Сам device secret получает защиту
провайдера ОС, расположенного за пределами файлов профиля.

`DeviceKeyProvider` предоставляет `seal`, `unseal`, `delete` и точный `kind`.
Смешивать metadata разных провайдеров нельзя. Device secret возвращается как
`Zeroizing<[u8; 32]>` только в короткое синхронное окно KDF/HMAC; MasterKey имеет
отдельную защищённую область памяти. Это не обещание отсутствия временных копий
внутри системных API, WebView, Java или криптобиблиотек.

Открытие строго разделяет:

1. Чтение ограниченной metadata и получение существующего OS credential.
   Потеря credential, заблокированный сервис, другой провайдер или неверный HMAC
   привязки возвращают ошибку **до** Argon2, счётчика пароля и wipe.
2. Проверку HMAC заголовка vault на текущем device secret. Заголовок перечитывается
   и проверяется также при повторном unlock уже открытого объекта.
3. Argon2id + HKDF с паролем и device secret, затем AEAD unwrap MasterKey.
   Только неверный пароль после успешных предыдущих этапов меняет счётчик.
   Ошибка выделения KDF workspace не считается неверным паролем.

Операции одного объекта `Vault` сериализуют throttle, чтение проверенного
заголовка, аутентификацию и запись результата. Конкурентные неудачные попытки
не сбрасывают счётчик устаревшей копией, а изменение пароля не использует
аутентификацию, выполненную до другой операции. Это не межпроцессная блокировка:
независимые объекты/процессы для одного профиля требуют отдельного контракта.

Legacy `.device`/vault v1 и legacy plaintext headless DB отклоняются явно.
Автоматическое удаление или «исправление» старых данных отсутствует. Ранее
созданные данные не получают новую гарантию задним числом.

## Платформенные возможности

| Платформа | Реализация | Что защищает | Ограничение |
|---|---|---|---|
| Android | Неэкспортируемый AES-GCM key в Android Keystore, TEE/StrongBox обязателен; 12-byte IV + 48-byte ciphertext для 32-byte secret, ID как AAD | Копия файлов профиля и software storage без аппаратного wrapping key не открывается | Software-only Keystore отклоняется. Clear data/uninstall/reset Keystore может навсегда лишить доступа без явного экспорта |
| Windows | Native DPAPI: внутренняя user-scope обёртка, затем внешняя `LOCAL_MACHINE`, обе без UI | Копия профиля на другой OS account/машину не получает необходимых DPAPI secrets | Это не TPM. Полный клон ОС, её secrets и привилегии администратора — другая угроза |
| macOS | Native Security.framework Data Protection Keychain, `WhenUnlockedThisDeviceOnly`, `Synchronizable=false`, без legacy file Keychain fallback | Копирование файлов профиля и cloud sync credential не являются способом переноса | Требуются работающий Data Protection Keychain и допустимая подпись/entitlements; неподдерживаемая конфигурация отклоняется, SEP-аттестация не заявлена |
| Linux desktop | OS Secret Service через `/usr/bin/secret-tool`, секрет по stdin, не argv; credential вне профиля | Копия только каталога профиля не содержит credential | **Не защищает от клона всей ОС/keyring вместе с учётными данными. Не TPM и не hardware binding** |
| Linux headless / OpenWrt | Шифрованная DB требует заранее доступного OS credential provider | Нет скрытого plaintext/keyfile fallback | TPM2 provider ещё не реализован; без Secret Service/session/provider запуск отказывает до создания DB. Наличие release binary не означает доступность защищённого unattended runtime |

Обычный user-scope DPAPI имеет исключение для roaming profile; поэтому
используются две обёртки, а `LOCAL_MACHINE` не применяется единственным фактором.
Это следует из [контракта Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata).
Два слоя не превращают software OS credentials в аппаратный TPM.

Android проверяет аппаратное происхождение ключа при создании и каждом
использовании, а не доверяет флагу из копируемой metadata. Неэкспортируемость
Keystore API и hardware tier — разные свойства:
[Android Keystore](https://developer.android.com/privacy-and-security/keystore).
Live root, компрометация процесса после unlock и снимок его памяти остаются
отдельной моделью угроз; они не устраняются device binding.

## Enrollment, отказ и перенос

Enrollment сначала создаёт защищённый OS credential, проверяет roundtrip и
публикует metadata через собственный temporary file, sync и no-clobber rename.
Vault также записывается через уникальный temporary file; первоначальная
публикация не перезаписывает уже созданный vault. Ошибка до публикации не удаляет
чужую metadata/credential. После публикации metadata credential не удаляется
из-за последующего сбоя sync. Остаточный orphan после аварийного завершения
внутри OS API возможен; автоматический поиск и удаление посторонних key aliases
не выполняются.

Если процесс остановился после binding metadata, но до `vault.bin`, следующий
enrollment использует ту же привязку. Если существует vault без доступного
credential, новая привязка автоматически не создаётся. Сброс Keystore не
превращается в неверный пароль или самоуничтожение данных.

Агент и bot по умолчанию открывают SQLCipher через
`open_device_bound_database`, с отдельным HKDF domain и именем DB. Пароль не
запрашивается на каждое сообщение. Unattended service должен иметь доступ к
заранее provisioned OS credential; копия plaintext SQLite не мигрирует молча.
Нет `--unbound-storage` и переменной с переносимым raw DB key.

Разрешённый перенос — только явный зашифрованный export после unlock и import
с паролем backup. Существующий export содержит логические данные и секреты
профиля под AEAD backup encryption; OS wrapping key и `.device-binding-v2.json`
не экспортируются. Import создаёт новый vault/OS binding на целевом устройстве
и восстанавливает данные, включая DM sidecar. Это сознательно переносимый
зашифрованный файл: имеющий его и пароль может выполнить import. Его нужно
защищать и удалять по выбранной пользователем политике. Сырые `vault.bin`, DB
и metadata этим capability не обладают.

`backup_open` и `backup_open_3factor` возвращают `Zeroizing<Vec<u8>>` сразу на
границе дешифрования: выходной plaintext очищается также при ошибке import или
отмене ожидающей async-операции. Временные копии внутри разбора/сериализации
данных остаются отдельной областью аудита.

Wipe сначала удаляет доступный OS credential, затем выполняет best-effort
очистку профиля. Перезапись flash/SSD не гарантирует физического удаления,
а старые явные exports, внешние OS backups и чужие копии находятся вне этой
операции. DPAPI не предоставляет per-profile revocation своего machine key.

## Проверка

Добавлены regression-тесты с независимыми mock OS stores: raw copy на другое
устройство, missing/unavailable/rotated credential без изменения counter/wipe,
legacy rejection, enrollment interruption, одновременная публикация,
сохранность чужой metadata и encrypted headless reopen. Native Windows test
проверяет реальный DPAPI roundtrip, чужой ID/AAD и tampering.

После уточнения владельца новые Rust/JNI/SDK сборки и тесты запускаются **только
в GitHub Actions**, см. [build-release.md](build-release.md). Эти новые тесты
ещё не объявлены PASS. Mock provider не доказывает работу TEE/TPM/SEP; требуются
отдельные hardware/device проверки, Android reinstall/backup-transfer negative
cases и native OS credential lifecycle. Software emulator не считается
успешным аппаратным Android provider.

В `core/gen/android/app/src/androidTest/java/app/gipny/DeviceKeystoreTest.kt` написаны
проверки отсутствующего alias, очистки входного массива при неверном ID,
hardware roundtrip и повреждённого AES-GCM ciphertext. Их компиляция включена
в `build.yml`; фактический instrumented запуск ещё не выполнен. На эмуляторе
без TEE/StrongBox ожидается отказ enrollment и hardware-positive test помечается
skipped: такой результат нельзя записывать как успешную защиту устройства.
Положительный тест нужно запускать на реальном hardware-backed устройстве,
отдельно проверяя недоступность raw-copy после переноса и отсутствие
автоматического восстановления credential после clear-data/uninstall.
