# Документация / Documentation

## Русский

Основной язык — русский; английское описание следует вторым в каждом текущем
руководстве. Срез исходников: **2026-11-06**, базовый `a6e41ca`; изменения
транспорта и документации в рабочей копии требуют собственной проверки CI.
Номер версии не означает опубликованный релиз.

Начните с [README](../README.md), [состояния функций](current-state.md) и
[модели безопасности](../SECURITY.md). Откройте
[браузерный клиент](https://gluckdev.github.io/gipny-i2p/app/browser.html)
или [руководство браузера](browser-messenger.md): работоспособность сети
зависит от опубликованной конфигурации форвардера и свежего netDb.

| Задача | Руководство |
|---|---|
| Пользоваться WASM-клиентом, хранить профиль, настроить сеть | [Браузер](browser-messenger.md) |
| Разместить WSS/WebRTC транспорт | [Rust-форвардер](../browser-router/forwarder/README.md) |
| Понять текущее состояние и планы | [Матрица функций](current-state.md), [планы](roadmap.md) |
| Обновить upstream и проверить настройки | [Источники и настройки](upstreams-and-settings.md) |
| Сборка, provenance, релиз | [GitHub Actions](build-release.md), [workflows](../.github/workflows/README.md) |
| Установить или обновить native клиент | [Платформы](platform-updates.md), [проверка обновления](update-verification.md), [OpenWrt](openwrt.md) |
| Защита профиля, памяти, журналов | [Device binding](device-bound-storage.md), [RAM](memory-security.md), [diagnostics](release-diagnostics.md) |
| Контакты и доставка | [Карточки](contact-cards.md), [группы](group-delivery.md), [удаление](message-deletion.md) |
| Размеры, хранение и подтверждения вложений | [Передача файлов](file-transfer.md) |
| Сеть роутера, UPnP | [UPnP](upnp.md), [источники и настройки](upstreams-and-settings.md) |
| Что нового в версии | [0.5.0-rc3](releases/0.5.0-rc3.md), [передача контекста](handoff-brief.md) |
| Android и переводы | [JNI](android-jni-build.md), [локализация](localization.md) |
| Карта исходников | [Core](../core/README.md), [libcore](../libcore/README.md), [UI](../ui/README.md), [DHT](../dht/README.md), [agent](../agent/README.md), [bot SDK](../bot-sdk/README.md) |
| Сайт проекта и его независимость от переписки | [Site](../site/README.md), [план зеркал Tor/I2P](site-mirrors.md) |

Native — Rust/Tauri → i2p-embed → C++ libi2pd. Web — WASM/Worker/OPFS →
WSS или WebRTC → Rust-форвардер → I2P. Отдельный локальный `gipny-web`
удалён. Упоминания SAM, daemon gateway или Python production forwarder в старых
планах не являются текущей инструкцией.

### Исторические материалы

`plans/`, `releases/`, `today-2026-10-28.md`, `handoff-brief.md`,
`relay-independence.md`, оценки старых транспортов и корневой `MIGRATION-i2p.md`
сохраняют решения и результаты своих дат. Они не заменяют матрицу текущего
состояния. [Старые результаты браузерного CI](archive/browser-validation-2026-11-01.md)
относятся к указанным SHA и не подтверждают новый Rust/WebRTC транспорт.
Планы не следует читать как список готовых возможностей.

## English

Russian is primary; English follows in current guides. Source snapshot:
2026-11-06, baseline `a6e41ca`. Working-tree transport/documentation changes need
new CI evidence. A source version is not a published release.

Start with the [English README](../README.en.md), [feature status](current-state.md)
and [security model](../SECURITY.md). The table above links browser setup,
forwarder operation, upstream/configuration, builds/releases, platform updates,
profile/memory/logging security, contact cards, delivery, Android, localization,
router UPnP, the 0.5.0-rc3 notes and component maps. Each current guide provides
an English section.

Native clients embed i2pd through Rust/C++ FFI. The web client uses
WASM/Worker/OPFS with WSS or WebRTC via a Rust transport forwarder. The separate
local `gipny-web` gateway is removed. Historical SAM/daemon/Python-forwarder
instructions are not current deployment instructions.

Dated plans, release notes, the old handoff brief and migration/evaluation papers
are historical evidence. Their claims apply only to their dates and revisions;
they do not establish current feature readiness or validate the new transport.
