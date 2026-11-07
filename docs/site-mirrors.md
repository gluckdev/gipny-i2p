# План зеркал сайта / Website mirror plan

## Русский

Статус: требование владельца от **2026-11-06**, запланировано. Это зеркала
публичной страницы проекта и документации, а не новый сервер переписки.
Адреса ещё не созданы и не опубликованы.

1. Подготовить неизменяемый комплект сайта RU/EN из того же коммита, что и
   основная страница. Шрифты, стили, изображения и скрипты — локальные. Переход
   к GitHub Releases остаётся явной внешней ссылкой, без автоматических запросов.
2. Подобрать **v3 onion-адрес, начинающийся с `gipny`**. Генератор — проверенная
   закреплённая ревизия [mkp224o](https://github.com/cathugger/mkp224o).
   Подбор выполнять на контролируемой машине, с ограничением числа потоков и
   времени; сохранить ключи службы в отдельном защищённом хранилище.
   Адрес кодирует публичный ключ, checksum и версию, поэтому произвольное имя
   не назначается вручную: [формат onion v3](https://spec.torproject.org/rend-spec/encoding-onion-addresses.html).
3. Подобрать I2P Destination с адресом **`gipny….b32.i2p`**. Кандидат инструмента —
   `vain` из [i2pd-tools](https://github.com/PurpleI2P/i2pd-tools); проверить
   конкретную ревизию, форматы ключей и соответствие выведенного адреса перед
   использованием. Дополнительно опубликовать привязку читаемого имени вроде
   `gipny.i2p` через поддерживаемые адресные книги. Имя не является глобально
   уникальным DNS-доменом: основной проверяемый адрес — полный b32/Destination.
   См. [именование I2P](https://i2p.net/en/docs/overview/naming/).
4. Развернуть статический сервер отдельно от данных мессенджера; локальный
   listener доступен только Tor/i2pd. Настроить обновления обоих зеркал из
   проверенного артефакта, health checks, rollback и резервные копии ключей.
   Ключи зеркал не включать в Git, публичный CI, логи или сборочные артефакты.
5. Подтвердить владение адресами публикацией одинакового манифеста на основной
   странице и обоих зеркалах; включить исходный SHA и digest артефакта. Затем
   добавить проверенные адреса на сайт и в документацию. Смена хостинга должна
   сохранять адреса; компрометация ключа требует отдельного объявления и замены.
6. Отдельно проверить возможность запуска WASM-клиента на каждом origin:
   `isSecureContext`, WebCrypto, Worker, OPFS, Web Locks, Service Worker,
   WebRTC/WSS и политику Tor Browser. HTTP-страница `.i2p` сама по себе не
   гарантирует доступ к этим API; разрешения и требования браузера описаны в
   [Secure Contexts](https://www.w3.org/TR/secure-contexts/). Если среда не подходит,
   зеркало остаётся страницей проекта без обещания работающего веб-клиента.

Критерии готовности: адреса имеют нужный префикс и независимо пересчитаны из
ключей; страницы доступны через соответствующие сети; RU/EN совпадают;
внешних фоновых запросов нет; обновление/откат и восстановление ключей проверены.
Подбор адресов и развёртывание ещё не выполнялись. Предсказуемые секретные ключи,
передача ключей стороннему сервису подбора и ослабление алгоритмов недопустимы.

## English

Status: owner requirement dated **2026-11-06**, planned. These are mirrors of the
public project page/documentation, not messaging servers. No addresses exist yet.

Publish identical RU/EN static artifacts from the same commit, with local assets
and explicit external release links. Generate a **`gipny`-prefixed v3 onion**
using a reviewed pinned [mkp224o](https://github.com/cathugger/mkp224o), and a
**`gipny….b32.i2p` Destination** using a reviewed pinned tool such as `vain` from
[i2pd-tools](https://github.com/PurpleI2P/i2pd-tools). Independently derive and
verify each address from its keys. Run bounded generation on a controlled host;
keep service keys out of Git, public CI, logs and artifacts, with protected backups.
The [onion encoding](https://spec.torproject.org/rend-spec/encoding-onion-addresses.html)
requires key search rather than assigning an arbitrary hostname. An additional
friendly I2P address-book name is useful but not globally unique; publish the full
cryptographic address as explained by [I2P naming](https://i2p.net/en/docs/overview/naming/).

Serve static files on a local listener reached by Tor/i2pd. Validate updates,
rollback, reachability and key recovery before publishing cross-referenced mirror
manifests/addresses. Preserve addresses across hosting changes. Key compromise
requires a separate rotation announcement. Check secure-context APIs and transport
availability independently before offering WASM on either mirror; an HTTP I2P
page does not automatically qualify under [Secure Contexts](https://www.w3.org/TR/secure-contexts/).
If necessary, offer the informational mirror first. No vanity generation or
deployment has been performed for this plan.
