# Зеркала сайта в I2P / I2P website mirrors

## Русский

Статус на **2026-11-16**: запланировано, действующих адресов нет.
Зеркала распространяют публичную страницу проекта и документацию.
Они не хранят профили и не заменяют I2P-роутер приложения серверным ядром.

Используем терминологию I2P: Destination, адреса `.b32.i2p` и имена `.i2p`
из адресной книги. Garlic routing описывает сетевые механизмы I2P, а не
отдельную доменную зону. Читаемое имя не является глобально уникальным DNS:
публикация зеркала должна включать проверяемый криптографический адрес.
[Именование I2P](https://i2p.net/en/docs/overview/naming/),
[терминология garlic routing](https://i2p.net/en/docs/overview/garlic-routing/).

1. Подготовить неизменяемый комплект `site/` со всеми восемью языками из того
   же коммита, что и основная страница. Шрифты, стили, изображения и скрипты
   должны быть локальными; GitHub Releases остаётся явной внешней ссылкой.
2. Создать Destination на контролируемом хосте и независимо проверить
   соответствие его ключей полному адресу. Ключи хранятся отдельно от Git,
   публичного CI, логов и артефактов; подготовить защищённое восстановление.
   Читаемое имя в адресной книге можно добавить после проверки адреса.
3. Размещать статические файлы отдельно от данных мессенджера. Локальный
   HTTP listener обслуживается через I2P; возможность доступа из обычного
   Интернета не подразумевается. Проверить обновление из артефакта, rollback,
   достижимость через I2P и сохранение адреса после восстановления хоста.
4. Перед публикацией разместить согласованный manifest на основной странице
   и зеркале: исходный SHA, digest комплекта и полный адрес. После проверки
   добавить адрес на сайт. Компрометация ключа требует смены адреса и отдельного
   объявления, а не незаметной подмены записи.
5. Возможность запуска WASM-клиента проверить отдельно на origin зеркала:
   `isSecureContext`, WebCrypto, Worker WebAssembly, OPFS, Web Locks,
   Service Worker и доступный транспорт. Сам по себе HTTP-адрес `.i2p` не
   обеспечивает браузеру secure context. Зеркало страницы может работать,
   даже когда браузерная среда не поддерживает запуск мессенджера.
   [W3C Secure Contexts](https://www.w3.org/TR/secure-contexts/).

Критерии готовности: криптографический адрес независимо проверен; комплект
всех языков совпадает по SHA; внешний доступ и фоновые запросы соответствуют
опубликованной конфигурации; обновление, откат и восстановление ключей проверены.
Создание адреса и развёртывание этого плана пока не выполнялись. Возможности
веб-клиента и его byte-forwarder описаны отдельно в
[browser-messenger.md](browser-messenger.md).

## English

Status on **2026-11-16**: planned, with no live addresses. Mirrors serve the
public project page and documentation. They hold no profiles and do not replace
an application's own I2P router with a server-side core.

Use I2P terminology: Destination, `.b32.i2p` addresses and `.i2p` address-book
names. Garlic routing describes I2P networking mechanisms; it is not a separate
domain suffix. Address-book names are locally meaningful, rather than globally
unique DNS names, so publish the independently verified cryptographic address.
[I2P naming](https://i2p.net/en/docs/overview/naming/),
[garlic routing terminology](https://i2p.net/en/docs/overview/garlic-routing/).

Publish the same immutable eight-language `site/` artifact and source SHA as
the primary site, with local assets and explicit external release links.
Create the Destination on a controlled host, independently verify its full
address, and keep its keys and protected recovery copies out of Git, public CI,
logs and artifacts. Add a readable address-book name only after verification.

Serve static files on a local HTTP listener reached through I2P, separately
from messenger data. Verify reachability, artifact updates, rollback and key
recovery. Cross-publish a manifest containing the source SHA, artifact digest
and full address before adding a live mirror link. Preserve the address across
host recovery; a compromised key requires an announced address rotation.

Validate browser secure-context APIs, Worker WebAssembly, OPFS and transport
availability independently before offering the WASM messenger on the mirror.
An HTTP I2P page does not automatically qualify as a
[secure context](https://www.w3.org/TR/secure-contexts/). The informational site
can remain useful when its origin cannot run the messenger. Address generation
and deployment have not been performed for this plan.
