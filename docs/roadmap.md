# Направления развития / Development roadmap

## Русский

Сначала — сохранность текущих сообщений, отзыв полномочий, границы хранения,
проверяемые релизы и реальная доставка после сбоев. Затем — измерение скорости,
памяти, трафика и восстановления при сохранении выбранного уровня приватности.

WASM — основной веб-клиент. WSS и WebRTC должны проходить одну матрицу доставки,
отказов, блокировки профиля и лимитов. CDN обслуживает поддерживаемые им протоколы;
TURN — отдельная услуга. Нераспознаваемость трафика требует собственной оценки.

Дальнейшие продуктовые направления: удобные контактные имена с проверяемым
владением; согласованная политика истории; завершение Android Share/биометрии;
синхронизация аватаров; аудио/видео; дополнительные архитектуры OpenWrt/MIPS;
независимая подпись издателя. Дополнительные идеи — Monero, опциональный майнинг,
телефонные звонки/SMS и I2P-почта — требуют отдельных моделей безопасности,
операторов и пользовательского согласия. Они не входят в текущую поставку.
Телефонный шлюз и обычная почта не получают анонимность и E2EE автоматически.
Бесплатный сертификат сам по себе не гарантирует доверие Windows/macOS.

Для каждой функции нужны код, интеграция в UI, отрицательные сценарии,
результат CI на точном SHA и актуальные RU/EN инструкции. Сроков без оценки нет.

### Сайт, независимый веб-клиент и зеркала — уточнение 2026-11-06

Описание, выбор одной загрузки и план зеркал подготовлены в рабочей копии.
Повторный запуск с остановленным хостингом добавлен в CI; выполнение, долгосрочное
обновление bootstrap без хостинга и развёртывание зеркал ещё впереди.

- Расширить понятное описание защиты переписки, локального профиля, вложений,
  удаления и ограничений браузера; одинаковое содержание RU/EN.
- Одна основная загрузка для определённой системы/архитектуры посетителя и
  ссылка «Все релизы на GitHub». При неопределённой архитектуре сначала уточнить
  систему в интерфейсе; не подставлять несовместимый пакет. Не показывать десятки
  агентских, OpenWrt и вспомогательных файлов на главном экране загрузки.
- Объяснить самостоятельность WASM-клиента: GitHub поставляет код/обновления,
  локальное приложение хранит профиль и выполняет шифрование и I2P-маршрутизацию.
  Переписка не использует GitHub как сервер сообщений. Проверить холодный запуск
  ранее установленного приложения без GitHub, включая публичную сетевую
  конфигурацию и bootstrap; не обещать независимость до этой проверки.
- Подготовить зеркала **страницы проекта** в Tor `.onion` и I2P. Подбирать
  криптографические адреса с префиксом **`gipny`**; для I2P также подготовить
  читаемое имя через адресную книгу. Подробный план — [зеркала](site-mirrors.md).
  Действующих адресов пока нет; они публикуются после развёртывания и проверки.

## English

Prioritize current-message integrity, authority revocation, storage boundaries,
verifiable releases and delivery after failures. Then measure latency, memory,
traffic and recovery while preserving the selected privacy profile.

WASM is the web client. WSS and WebRTC need the same delivery, failure, profile
locking and resource-limit acceptance matrix. CDN support is protocol-specific;
TURN is a separate service. Traffic indistinguishability needs separate evidence.

Product directions include verifiable contact-name ownership, consistent history
policy, complete Android Share/biometric flows, avatar synchronization, audio/video,
additional OpenWrt/MIPS targets and independent publisher signing. Monero,
optional mining, telephone/SMS gateways and I2P mail remain separate proposals
requiring threat models, operators and user consent. They are not current features.
Telephone and mail gateways do not automatically provide anonymity or E2EE.
A free certificate alone does not establish Windows/macOS trust.

Each feature needs implementation, UI integration, failure cases, CI at the exact
commit and current RU/EN instructions. No unassessed delivery dates are promised.

### Website, independent web client and mirrors — 2026-11-06 clarification

Copy, one-download selection and the mirror plan are prepared locally. A host-stopped
restart scenario is added to CI; execution, long-term bootstrap refresh without
hosting and mirror deployment remain pending.

Explain messaging/profile/file security and browser limitations in both languages.
Offer one matching native download and an “All releases on GitHub” link; ask for
missing architecture information instead of guessing. Keep specialized artifacts
off the main download surface. Present WASM as an application running locally:
GitHub supplies code/updates, while local code owns profiles, crypto and I2P
routing; GitHub is not a messaging server. Verify a cold launch of an installed
client with GitHub unavailable, including configuration/bootstrap, before claiming
that property. Plan Tor `.onion` and I2P mirrors of the project page, with `gipny`
vanity prefixes and an additional I2P address-book name. See the [mirror plan](site-mirrors.md);
no live mirror addresses are claimed yet.
