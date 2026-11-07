# i2p-embed — встроенный роутер / Embedded router

## Русский

Крейт связывает Rust с upstream i2pd через небольшой C ABI. Роутер работает
внутри процесса; его глобальное состояние не позволяет считать два профиля
двумя независимыми экземплярами роутера. Приложение создаёт I2P destinations
и потоки через API, без локального SAM/SOCKS/HTTP gateway.

| Источник | Назначение |
|---|---|
| `src/lib.rs` | Rust API, жизненный цикл роутера, destinations и streams |
| `shim/shim.cpp`, `shim/shim.h` | C ABI к C++ i2pd, сеть/остановка и transport ports |
| `src/upnp.rs`, `src/upnp_fixture.rs` | Управление NAT mappings и CI fixture |
| `src/diagnostics.rs` | Неизменяемая политика журналов по типу сборки |
| `build.rs` | Native compilation/linking; Android использует подготовленную библиотеку |
| `miniupnpc-source.json`, `vendor/miniupnpc` | Зафиксированный upstream и проверяемые локальные изменения miniupnpc |
| `check-source.py` | Проверка списка файлов и SHA-256; служебный инструмент, не runtime |

Native i2pd остаётся C++ upstream; wrapper и менеджер UPnP — Rust, miniupnpc — C.
Роутер имеет NTCP2/SSU2 порты даже при выключенных локальных прокси. UPnP manager
проверяет допустимость gateway и владение правилами, продлевает аренду и выполняет
cleanup при смене сети/остановке. Android offline/online уведомления проходят
через тот же C hook. Возможности и ограничения: [UPnP](../docs/upnp.md),
[upstream и настройки](../docs/upstreams-and-settings.md).

Сборки и исполняемые тесты — только GitHub Actions. `i2p-embed.yml` проверяет
native Linux/macOS/Windows, Android exports и live I2P; `security-fast.yml`
использует также stub для проверок других компонентов. Stub не проверяет C ABI,
реальный роутер или NAT. Новые изменения UPnP требуют выполнения CI и отдельной
проверки физического роутера. [Журналы](../docs/release-diagnostics.md),
[текущее состояние](../docs/current-state.md).

## English

This crate connects Rust to upstream C++ i2pd through a small C ABI. The router
runs in-process with global state; two profiles are not independent router
instances. Destinations and streams use the API without a local SAM/SOCKS/HTTP
gateway. NTCP2/SSU2 transport ports still exist.

`src/lib.rs` owns the Rust API/lifecycle. `shim/shim.cpp` and `shim.h` implement
the C boundary and network/stop hooks. `src/upnp.rs` manages NAT rules with a
fixture in `upnp_fixture.rs`; `diagnostics.rs` defines fixed build-time logging.
`build.rs` controls native compilation/linking and Android prebuilt integration.
`miniupnpc-source.json` records pinned sources and local patches;
`check-source.py` verifies file lists and hashes as a tooling-only step.

The wrapper/UPnP manager is Rust and miniupnpc is C; existing i2pd remains C++.
The manager validates gateway scope and rule ownership, renews leases and handles
network/stop cleanup. Android notifications reach the same C hook. See
[UPnP](../docs/upnp.md) and [upstreams/settings](../docs/upstreams-and-settings.md)
for precise behavior and limitations.

Builds and executable tests belong in GitHub Actions. `i2p-embed.yml` covers
native platforms, Android exports and live I2P; `security-fast.yml` also uses a
stub for other components. Stubs do not verify C ABI, a real router or NAT. New
UPnP changes await CI and physical router validation. See [logging](../docs/release-diagnostics.md)
and [current state](../docs/current-state.md).
