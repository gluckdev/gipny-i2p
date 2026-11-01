# Агент GIPNY на OpenWrt: архитектуры, сборка и установка

Требование владельца от 2026-10-28: выпускать `gipny-agent` для максимально
широкого реального набора устройств OpenWrt 2019–2026. Поддержка определяется
CPU, ABI, ядром и ресурсами конкретной прошивки, а не годом выпуска роутера.
Изменение выпущено в `v0.4.19` (2026-10-29) из интеграционной ветки.
Все четыре target jobs на `38892f6` прошли GitHub CI; ссылки и границы
этой проверки приведены ниже. Это не подтверждение запуска на любом роутере.

## Матрица

Каждый архив называется `gipny-agent_<версия>_<Rust target>.tar.gz`.
Это полностью статический musl-бинарник с настоящим встроенным i2pd, Boost,
OpenSSL и zlib. Внешний i2pd, SAM, динамический загрузчик и пакеты этих библиотек
на устройстве не нужны. Релизная сборка не включает подробные диагностические
журналы; [политика диагностики](release-diagnostics.md) действует и здесь.

| Rust target | CPU / ABI | Применимость | OpenWrt toolchain для сборки |
|---|---|---|---|
| `aarch64-unknown-linux-musl` | ARMv8-A, 64-bit, little endian | 64-битное ядро и userspace AArch64; семейства Cortex-A53 и новее при достаточных ресурсах | `armsr/armv8` |
| `armv7-unknown-linux-musleabi` | ARMv7-A, 32-bit, little endian, soft-float | ARMv7-A без обязательного FPU/NEON; например, класс Cortex-A7/A9. Не ARMv6 и не ARM big endian | `bcm53xx/generic`, Cortex-A9 / soft-float |
| `x86_64-unknown-linux-musl` | x86-64, 64-bit, базовый ISA | x86/64 роутеры, мини-ПК и виртуальные машины | `x86/64` |
| `i686-unknown-linux-musl` | x86, 32-bit, Pentium 4 / SSE2 | 32-битные x86 с SSE2; не любой процессор с названием i686 | `x86/generic` |

Выбор ABI относится ко **всему** бинарнику: Rust std, C/C++, libc, libstdc++,
OpenSSL и Boost. В ARM EABI числа передаются в обычных регистрах, в EABIHF —
в регистрах FPU; смешивать архивы этих ABI нельзя.
[Rust Arm ABI](https://doc.rust-lang.org/rustc/platform-support/arm-linux.html).
Поскольку здесь все библиотеки статические и используют soft-float ABI,
динамические hard-float библиотеки прошивки не подмешиваются.

Toolchain закреплён на официальном **OpenWrt 24.10.4, GCC 13.3.0, musl 1.2.5**,
Rust — на **1.93.1**. Для каждого архива в `openwrt-targets.sh` записана SHA256
из первоисточника: [AArch64](https://downloads.openwrt.org/releases/24.10.4/targets/armsr/armv8/),
[ARMv7](https://downloads.openwrt.org/releases/24.10.4/targets/bcm53xx/generic/),
[x86_64](https://downloads.openwrt.org/releases/24.10.4/targets/x86/64/),
[i686](https://downloads.openwrt.org/releases/24.10.4/targets/x86/generic/).
Это версия **среды сборки**, а не требование установить именно такую прошивку.
Rust 1.93 использует musl 1.2.5 для этих targets:
[объявление Rust](https://blog.rust-lang.org/2025/12/05/Updating-musl-1.2.5/).

## Что сознательно не включено

- **MIPS / MIPSel**: `mips-unknown-linux-musl` и `mipsel-unknown-linux-musl`
  относятся к Tier 3; готового std через rustup для этой версии нет.
  OpenWrt `ath79` обычно требует big endian O32/soft-float, `ramips/mt7621` —
  little endian O32/soft-float. Обычный Rust triple сам по себе не подтверждает
  совпадение float ABI. Нужны отдельная сборка std современным nightly,
  согласованный soft-float target, проверка TLS/crypto и реальный аппаратный
  прогон. Подмена на MIPS64/N64, старый Rust или stub не добавляется.
  [Tier policy и таблица Rust 1.93.1](https://github.com/rust-lang/rust/blob/1.93.1/src/doc/rustc/src/platform-support.md),
  [OpenWrt ath79](https://github.com/openwrt/openwrt/blob/openwrt-24.10/target/linux/ath79/Makefile),
  [OpenWrt MT7621](https://github.com/openwrt/openwrt/blob/openwrt-24.10/target/linux/ramips/mt7621/target.mk).
- **ARMv6 soft-float**: Rust std существует, но проверенный официальный
  `bcm2708` toolchain с `_musl_eabi` в имени фактически имеет GCC default
  `-mfloat-abi=hard` и такой же prebuilt runtime. Переименование или флаг soft
  при финальной линковке это не исправляет. Нужен настоящий soft-float sysroot
  либо отдельный честно обозначенный hard-float релиз. ARMv5, ARM big endian,
  PowerPC, RISC-V и vendor-прошивки без OpenWrt также не заявляются.
- **Все устройства 2019–2026**: название модели и год не подтверждают ABI,
  свободную память, место на flash, доступный интернет или нужные возможности
  ядра. Малые устройства без места под бинарник и состояние i2pd требуют
  постоянного USB/extroot; установка в `/tmp` или `/var` теряет ключи при
  перезагрузке. Нагрузка встроенного роутера на физическом устройстве ещё
  не измерена. Рабочую память нужно проверить до включения на основном роутере.

При проверке Cortex-A7/MT7629 toolchain обнаружен default `armv7ve`;
его runtime нельзя объявлять generic ARMv7-A, просто добавив другой `-march`.
Выбран Cortex-A9 soft-float sysroot. Новые C/C++ исходники собираются с
`-march=armv7-a -mtune=generic-armv7-a -mfloat-abi=soft`; `-march=native`
не используется. Предварительно скомпилированный runtime проверяется отдельно.

## Ядро и 32-битное время

Историческая нижняя граница многих Linux targets Rust — Linux 3.2; некоторые
архитектуры требуют более нового ядра, а для части Tier 2 musl минимальная версия
не документирована столь же точно. Нельзя механически перенести требования
glibc-target на musl или считать номер toolchain гарантией старого ядра.
[Область требований Rust](https://blog.rust-lang.org/2022/08/01/Increasing-glibc-kernel-requirements/).

В проверенном исходнике musl 1.2.5 `time_t` — 64-bit и на ARM/i686. В
`src/time/clock_gettime.c` есть переход с `clock_gettime64` на старый syscall
при `ENOSYS`; в `src/thread/__timedwait.c` — с `futex_time64` на `futex`.
Поэтому отсутствие syscalls Linux 5.1 само по себе не означает обязательный
отказ на ядре OpenWrt 19.07 / Linux 4.14. Это **вывод по коду**, не выполненный
прогон GIPNY на 4.14; fallback time32 не даёт гарантии корректного времени после
2038 года на старом ядре.
[Официальный исходник musl 1.2.5](https://musl.libc.org/releases/musl-1.2.5.tar.gz),
SHA256 `a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4`.
CI QEMU user-mode использует **ядро runner**, а не ядро OpenWrt, и не доказывает
совместимость с Linux 4.14 или любой конкретной моделью роутера.

## Установка

Сначала определите архитектуру (`uname -m`) и постоянный каталог с запасом
места. Установщик требует минимум 64 MiB свободного места на выбранном разделе;
это порог установки, а не гарантия достаточного места для дальнейшей работы.
Понадобятся root, procd, `uci`, `jsonfilter`, `sha256sum`, `tar` и HTTPS-загрузчик
с CA. Недостающие пакеты устанавливаются штатным `opkg`/`apk` вашей прошивки.

Скачайте [install-agent-openwrt.sh](../scripts/install-agent-openwrt.sh), прочтите
его и запустите, например, с каталогом на уже настроенном постоянном USB:

```sh
sh install-agent-openwrt.sh --prefix /mnt/usb/gipny-agent --data /mnt/usb/gipny-agent-data
```

Установщик запросит карточку мастера через `/dev/tty`. Для неинтерактивного
запуска передайте `--master 'gipny:v3:…'`; прежняя v2 с релеем также
принимается. Карточка не печатается в лог. [Формат карточек](contact-cards.md).
`--name` задаётся только явно; без него бинарник сохраняет стабильное кошачье
имя. Установка без параметров использует `/opt/gipny-agent`, данные — прежнюю
настройку службы или `<prefix>/data`.

Перед изменением службы установщик определяет единственный exact asset для
полученного тега, скачивает `SHA256SUMS.txt`, проверяет **весь** manifest, затем
SHA256 архива и запуск `--help` нового бинарника. Отсутствующая/неверная сумма,
повторная строка, ошибка HTTP или чужая версия останавливают установку до
изменения существующего агента и данных. Это контроль целостности относительно
того же GitHub release, не независимая подпись издателя.

Затем устанавливаются бинарник, снимок сети, `master.card` с ограниченными
правами и служба `/etc/init.d/gipny-agent`; параметры находятся в UCI
`/etc/config/gipny-agent`. Существующие identity/база не удаляются. Служба
включается на следующий boot и запускается сейчас. Автоматического respawn
нет: команда OFF мастера должна оставить процесс остановленным. Команды
выполняются с правами службы (в этом установщике — root).

```sh
/etc/init.d/gipny-agent status
logread -e gipny-agent
/etc/init.d/gipny-agent stop
/etc/init.d/gipny-agent disable
```

Принятый procd запуск ещё не означает установленные I2P-туннели: собственная
карточка появится в `<data>/card.txt` после готовности встроенного роутера.
Master card/ключи/контакты не отправляются установщиком на GitHub. Сам
**явно запущенный установщик** получает только публичные release assets по
обычному HTTPS; это отдельное действие от I2P-транспорта агента и его обновлятора.

## Как собирается и что проверено

[openwrt.yml](../.github/workflows/openwrt.yml) запускает настоящие четыре
`cargo build --locked --release -p gipny-agent --target …` для PR/ручного запуска
и вызывается из [release.yml](../.github/workflows/release.yml). Публикация
зависит от успеха всех четырёх jobs; strict release table требует все архивы.
Десктопные macOS Apple Silicon/Intel, Windows/Linux и Android jobs сохраняются.

Подготовка `.github/scripts/openwrt-prepare.sh` проверяет pinned toolchain SHA,
собирает статические Boost `program_options`, OpenSSL из exact `openssl-src`
и checksum текущего `Cargo.lock`, zlib 1.3.1. SQLCipher использует тот же OpenSSL.
Настоящий i2pd компилируется из pinned submodule; stub, skip-native и внешний
prebuilt router в `openwrt-build.sh` запрещены. Нужен C++20. GCC specs OpenWrt
не добавляют static unwinder сами, поэтому `libgcc_eh` линкуется явно.

Обязательные gates: ELF machine/разрядность/little endian, отсутствие
`PT_INTERP` и `DT_NEEDED`, отсутствие hard-float ARM ABI, production release
traces и отдельный diagnostics probe. Native C++ probe проверяет `time_t`,
floating-point ABI, атомики, Boost, SHA256 OpenSSL и round-trip zlib; затем
бинарник агента исполняет `--help` под QEMU соответствующего CPU (x86_64 —
на runner). Это не живой I2P-тест и не замер памяти на роутере.

На **2026-10-28**, SHA `38892f6691b81a98f4deb55a27e3874fe1358f2e`,
[GitHub run 40759359675](https://github.com/gluckdev/gipny-i2p/actions/runs/40759359675)
завершился успешно для всех четырёх targets:

| Rust target | Результат GitHub CI |
|---|---|
| `i686-unknown-linux-musl` | [PASS](https://github.com/gluckdev/gipny-i2p/actions/runs/40759359675/job/123066773934) |
| `armv7-unknown-linux-musleabi` | [PASS](https://github.com/gluckdev/gipny-i2p/actions/runs/40759359675/job/123066773941) |
| `aarch64-unknown-linux-musl` | [PASS](https://github.com/gluckdev/gipny-i2p/actions/runs/40759359675/job/123066773966) |
| `x86_64-unknown-linux-musl` | [PASS](https://github.com/gluckdev/gipny-i2p/actions/runs/40759359675/job/123066774182) |

Это реальные сборки встроенного роутера и static agent с gates и probes,
описанными выше, включая запуск `--help`. Они не проверяют OS credential
provider, открытие рабочего профиля, живую I2P или память на физическом
устройстве. Для runtime по-прежнему требуется поддерживаемый OS provider;
его отсутствие означает fail-closed, а не raw-key fallback.
Результат `38892f6` не распространяется автоматически на более поздний SHA;
новый опубликованный релиз и аппаратная совместимость им не заявлены.

Повторная проверка `a66d2239186c38ff5be73a4647b0fe5c1d8587c9` также завершилась
успешно для [AArch64](https://github.com/gluckdev/gipny-i2p/actions/runs/40763085172/job/123079901132),
[i686](https://github.com/gluckdev/gipny-i2p/actions/runs/40763085172/job/123079901665),
[ARMv7 soft-float](https://github.com/gluckdev/gipny-i2p/actions/runs/40763085172/job/123079901734)
и [x86_64](https://github.com/gluckdev/gipny-i2p/actions/runs/40763085172/job/123079902088).
Границы runtime/provider/kernel этой проверкой не расширены.

Исторически, до требования GitHub-only builds, локально 2026-10-28 прошли
offline regression tests парсера manifest/master card и отрицательные проверки
на ELF; BusyBox ash syntax и YAML/run blocks также проверялись. Эти результаты
не заменяют приведённые jobs для конкретного SHA.

Команды для воспроизведения шагов в GitHub Actions
([правило сборок](build-release.md)):

```bash
python3 .github/scripts/test-openwrt.py
busybox sh -n scripts/install-agent-openwrt.sh
bash .github/scripts/openwrt-prepare.sh armv7-unknown-linux-musleabi /tmp/gipny-openwrt-armv7
rustup target add --toolchain 1.93.1 armv7-unknown-linux-musleabi
bash .github/scripts/openwrt-build.sh armv7-unknown-linux-musleabi /tmp/gipny-openwrt-armv7
```

Для последних двух команд нужны согласованный Rust 1.93.1, источники submodule,
текущие reseed certificates и `qemu-arm`; среда сборки требует существенно
больше места, чем готовый архив. Каждая архитектура собирается отдельно.
