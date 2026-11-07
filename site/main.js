export const messages = {
  "ru": {
    "title": "gipny.i2p — Личная переписка через I2P.",
    "description": "Мессенджер с открытым кодом: сквозное шифрование, встроенный I2P и локальные профили без номера телефона. Приложения для компьютеров, Android и браузера.",
    "skip": "Перейти к содержимому",
    "navLabel": "Основная навигация",
    "languageLabel": "Язык сайта",
    "navPrinciples": "Как работает",
    "navApps": "Скачать",
    "navBrowser": "Веб-версия",
    "heroEyebrow": "Переписка со сквозным шифрованием",
    "heroTitleOne": "Ваши разговоры.",
    "heroTitleTwo": "Ваши ключи.",
    "heroDescription": "Общайтесь и передавайте файлы без привязки к номеру телефона. Ключи и история хранятся на вашем устройстве, а встроенный роутер подключает приложение к I2P.",
    "download": "Скачать приложение",
    "source": "Открыть исходники",
    "availability": "Клиенты для Linux, Windows, macOS, Android и браузера.",
    "downloadsEyebrow": "Готовые пакеты",
    "downloadsTitle": "Скачайте для своего устройства.",
    "downloadsBody": "Одна загрузка для вашей системы. Если браузер скрывает тип процессора, уточните устройство в списке. Все пакеты, контрольные суммы и инструкции доступны в релизах GitHub.",
    "catalogRelease": "Опубликованный релиз:",
    "pathEyebrow": "Путь сообщения",
    "pathSender": "Ваше устройство",
    "pathRecipient": "Устройство собеседника",
    "pathSealed": "Сначала шифрование — затем отправка.",
    "pathCaption": "I2P переносит трафик, а сквозное шифрование защищает текст и вложения. Релеи передают и временно хранят шифротекст; собеседник расшифровывает его на своём устройстве.",
    "pathLocal": "Ключи на вашем устройстве",
    "pathNoAccount": "Без номера телефона",
    "principlesEyebrow": "Как это устроено",
    "principlesTitle": "Контроль начинается с вашего устройства.",
    "identityTitle": "Ключ контакта вместо номера телефона.",
    "identityBody": "Профиль создаётся локально, без регистрации по телефону или почте. Обменяйтесь карточками контакта и сверьте отпечатки по доверенному каналу.",
    "encryptionTitle": "Шифрование до отправки.",
    "encryptionBody": "Личные чаты используют X3DH и Double Ratchet. Сообщения и файлы шифруются до передачи релеям, а расшифровываются на устройствах участников.",
    "networkTitle": "I2P встроен в приложение.",
    "networkBody": "Отдельный роутер устанавливать не нужно. При недоступности I2P или релеев доставка может задержаться; прямой IP-канал для сообщений не используется.",
    "securityLabel": "Защита и её границы",
    "securityBoundary": "Релеи видят служебные данные доставки. Скомпрометированное устройство может раскрыть переписку. Сохранённые собеседником копии нельзя гарантированно удалить.",
    "i2pEyebrow": "Технология I2P",
    "i2pTitle": "Приватная доставка с сетевыми компромиссами.",
    "i2pIntro": "I2P передаёт сообщения через зашифрованные туннели. Для связи не нужны номер телефона и прямое соединение с собеседником.",
    "i2pProTunnelsTitle": "Чесночная маршрутизация и однонаправленные туннели.",
    "i2pProTunnelsBody": "Отдельные входящие и исходящие туннели распределяют маршрут между несколькими узлами. Защита зависит от выбранного профиля и возможностей наблюдателя.",
    "i2pProIdentityTitle": "Криптографические адреса вместо IP.",
    "i2pProIdentityBody": "Вместо IP собеседники обмениваются контактными ключами и I2P-адресами. Отпечаток ключа проверяйте по доверенному каналу.",
    "i2pProBuiltinTitle": "Внутренняя сеть без выходных узлов.",
    "i2pProBuiltinBody": "Роутер встроен в нативное приложение; в веб-клиенте он работает как WASM. Переписка доставляется внутри I2P.",
    "i2pConBootstrapTitle": "Долгий холодный старт.",
    "i2pConBootstrapBody": "При первом запуске роутер получает сведения об узлах и строит туннели. Это может занять несколько минут.",
    "i2pConLatencyTitle": "Задержки и накладные расходы.",
    "i2pConLatencyBody": "Многослойное шифрование и прохождение через несколько промежуточных узлов увеличивают отклик, расход трафика и нагрузку на батарею по сравнению с централизованными мессенджерами.",
    "i2pConVisibilityTitle": "Транспорт и наблюдение за трафиком.",
    "i2pConVisibilityBody": "Нативный I2P использует NTCP2/SSU2. Для браузера предусмотрены WSS и WebRTC через транспортный мост. Они меняют вид соединения, но не гарантируют нераспознаваемость; WebRTC и CDN требуют отдельной проверки.",
    "nativeStatus": "Приложения для компьютеров и Android",
    "nativeTitle": "Выберите приложение для своего устройства.",
    "nativeBody": "Личные чаты, группы, вложения и проверка контактов работают в приложении. На странице релиза есть пакеты, поддерживаемые архитектуры и инструкции; перед установкой проверьте требования к системе.",
    "nativeDownload": "Открыть загрузки",
    "stepOneTitle": "Скачайте приложение.",
    "stepOneBody": "Выберите пакет для своей системы и архитектуры. Прочитайте инструкцию по установке и требования к защите устройства.",
    "stepTwoTitle": "Создайте локальный профиль.",
    "stepTwoBody": "Задайте пароль. Приложение создаст личность и ключи локально; для хранилища нужен поддерживаемый механизм защиты операционной системы. Дайте I2P время подключиться.",
    "stepThreeTitle": "Обменяйтесь карточками.",
    "stepThreeBody": "Передайте свою карточку по доверенному каналу и добавьте карточку собеседника. Прежде чем обсуждать важное, сравните отпечатки контакта.",
    "browserStatus": "Экспериментальная WASM-версия",
    "browserTitle": "Самостоятельное приложение. В вашем браузере.",
    "browserBody": "Вы запускаете приложение на своём устройстве: WASM-ядро шифрует сообщения, хранит профиль и работает с I2P. GitHub Pages раздаёт код и обновления; сервер GitHub не участвует в доставке переписки.",
    "browserTimeline": "Браузерный клиент проходит проверку. Сетевому режиму нужен доступный транспортный мост; при его отказе нет скрытого обходного подключения.",
    "browserBoundary": "Перед вами сайт проекта. Он не отправляет и не принимает сообщения, не создаёт профили и не хранит ваши ключи.",
    "browserOpen": "Открыть веб-мессенджер",
    "browserSetup": "Возможности и требования веб-версии",
    "browserLocal": "Нужны HTTPS, WebAssembly, Worker и OPFS. Очистка данных сайта удаляет локальный профиль. Перенос и зашифрованный экспорт браузерного профиля пока не реализованы.",
    "browserStandaloneBody": "Ключи и зашифрованная история остаются в OPFS. WASM работает в отдельном Worker. Браузер не предоставляет все механизмы защиты памяти и хранилища нативного приложения.",
    "hostingLabel": "О посещении сайта",
    "hostingPrivacy": "На этой странице нет аналитики и внешних скриптов. GitHub Pages видит IP и время посещения. Веб-клиент доверяет коду своего хостинга; транспортный мост и TURN видят сетевые метаданные.",
    "plansEyebrow": "В планах",
    "plansTitle": "Что дальше.",
    "plansIntro": "Направления развития; наличие пункта в планах не означает готовую функцию.",
    "planNameTitle": "Имя, которым удобно делиться.",
    "planNameBody": "Удобные имена для обмена контактами. Владение именем и защита от подмены ещё требуют доработки; сейчас используйте карточки и отпечатки.",
    "planCallsTitle": "Защищённые видеозвонки и конференции.",
    "planCallsBody": "Аудио и видео — отдельное расширение. Транспорт WebRTC для переписки сам по себе ещё не означает поддержку звонков.",
    "planCertTitle": "Проверяемые сборки.",
    "planCertBody": "Независимая подпись издателя и происхождение пакетов. Возможность убрать предупреждения ОС зависит от требований платформы.",
    "plansBoundary": "Собеседник всё равно сможет сохранить текст или сделать снимок экрана.",
    "openEyebrow": "Открытая разработка",
    "openTitle": "Открытый код. Понятные ограничения.",
    "openBody": "Исходники, архитектура и описание защиты доступны публично. Следите за разработкой, изучайте известные ограничения и сообщайте о проблемах.",
    "resourceCode": "Исходники и задачи",
    "resourceDocs": "Документация RU / EN",
    "resourceSecurity": "Политика безопасности",
    "footerAttribution": "I2P-форк проекта",
    "footerReleases": "Все релизы на GitHub",
    "catalogSystem": "Ваше устройство",
    "catalogChoose": "Выберите систему и процессор",
    "catalogDownload": "Скачать для",
    "catalogAll": "Все релизы на GitHub",
    "catalogHelp": "Система определяется на вашем устройстве; эти сведения никуда не отправляются. Выбор можно изменить.",
    "catalogUnavailable": "Для этой системы нет подходящего нативного пакета в выбранном релизе. Можно открыть веб-клиент или посмотреть все релизы.",
    "storageTitle": "Зашифрованный локальный профиль.",
    "storageBody": "История и ключи сохраняются на устройстве. Нативное приложение использует защищённое хранилище ОС; веб-клиент — зашифрованное хранилище браузера. Пароль защищает закрытый профиль, но не заражённую систему.",
    "attachmentTitle": "Вложения под той же защитой.",
    "attachmentBody": "Файлы передаются зашифрованными частями. Получатель проверяет целостность файла до подтверждения завершения. Поддерживаемые изображения проходят очистку метаданных, если она включена; очистка PDF пока доступна только в нативном приложении.",
    "securityPracticeTitle": "Проверяйте, кому доверяете.",
    "securityPracticeBody": "Сверьте отпечаток контакта отдельным каналом. Обновляйте приложение из проверенного источника. Открытый код позволяет изучить защиту, но сам по себе не означает независимый аудит.",
    "browserDifferenceTitle": "Чем отличается веб-клиент Gipny",
    "browserDifferenceBody": "В отличие от веб-оболочки, которая управляет запущенным на сервере мессенджером, Gipny выполняет эту работу в вашем браузере. Аккаунт GitHub, установленное приложение на телефоне и сопряжение с ним не нужны.",
    "browserDataTitle": "Профиль на вашем устройстве",
    "browserDataBody": "Профиль создаётся локально. Пароль, ключи и история не отправляются GitHub; сообщения расшифровываются здесь же.",
    "browserNetworkTitle": "Свой I2P-роутер",
    "browserNetworkBody": "WASM-роутер строит I2P-туннели на устройстве. Из-за ограничений браузера выход в сеть проходит через WSS- или WebRTC-мост. Это транспорт зашифрованных данных; он видит IP, время и объём соединений.",
    "browserHostingTitle": "Хостинг для установки и обновлений",
    "browserHostingBody": "После сохранения приложения браузером его оболочка и история открываются без хостинга. Для переписки нужны сеть I2P, рабочий мост и актуальные сведения об узлах. Первый запуск и получение новой версии требуют доступного источника файлов.",
    "planMirrorsTitle": "Зеркала в Tor и I2P.",
    "planMirrorsBody": "Планируем зеркала страницы в .onion и .i2p с адресами, содержащими gipny. Ключи и адреса будут проверены перед публикацией; действующих зеркал пока нет.",
    "planMirrorsLink": "План зеркал и проверки адресов",
    "navSecurity": "Защита",
    "navDevices": "Устройства",
    "heroWeb": "Открыть в браузере",
    "webCompareTitle": "Чем это не похоже на веб-версии других мессенджеров",
    "webCompareLead": "Сравните, где лежат ключи и что делает сервер",
    "webCompareQuestion": "Вопрос",
    "webCompareThem": "Обычная веб-версия",
    "webCompareUs": "Gipny в браузере",
    "webRowKeys": "Где ключи шифрования",
    "webRowKeysThem": "На сервере сервиса или в телефоне, к которому привязан браузер",
    "webRowKeysUs": "Только в этом браузере, в зашифрованном профиле",
    "webRowDecrypt": "Где расшифровываются сообщения",
    "webRowDecryptThem": "Смотря какой сервис: на сервере или на связанном телефоне",
    "webRowDecryptUs": "Здесь же, в WebAssembly-ядре на вашем устройстве",
    "webRowHistory": "Где хранится история",
    "webRowHistoryThem": "Обычно на серверах сервиса",
    "webRowHistoryUs": "В зашифрованном хранилище OPFS этого браузера",
    "webRowServer": "Что делает сервер сайта",
    "webRowServerThem": "Принимает, хранит и пересылает переписку",
    "webRowServerUs": "Только раздаёт файлы приложения и обновления",
    "webRowAccount": "Что нужно для входа",
    "webRowAccountThem": "Номер телефона или аккаунт сервиса",
    "webRowAccountUs": "Пароль или ключ доступа к локальному профилю",
    "webMagicTitle": "Где происходит магия",
    "webMagicIntro": "Нажмите на слой, чтобы узнать, что он делает и что может видеть.",
    "layerTabTitle": "Вкладка браузера",
    "layerTabLead": "Тот же интерфейс, что в приложениях",
    "layerTabBody": "Окно показывает чаты и передаёт команды ядру. Ключи и хранилище профиля в коде страницы не живут: они в отдельном потоке. Для WebRTC окно держит соединение, но через него проходят только зашифрованные байты.",
    "layerWorkerTitle": "Web Worker: Rust, скомпилированный в WebAssembly",
    "layerWorkerLead": "Ядро мессенджера и I2P-роутер работают здесь",
    "layerWorkerBody": "В отдельном потоке браузера работает то же ядро на Rust, что и в приложениях. X3DH и Double Ratchet шифруют каждое сообщение, файлы делятся на зашифрованные части, а встроенный I2P-роутер сам строит туннели. Узлы сети роутер запоминает между запусками, поэтому повторный старт быстрее. Сервер сайта во всём этом не участвует.",
    "layerVaultTitle": "Зашифрованный профиль в OPFS",
    "layerVaultLead": "История и ключи не покидают устройство",
    "layerVaultBody": "Профиль хранится в приватной файловой системе браузера и зашифрован ключом из пароля через Argon2id или из ключа доступа (passkey с расширением PRF), если браузер его поддерживает. Файлы до 2 ГиБ лежат отдельными зашифрованными блоками. Очистка данных сайта удаляет профиль.",
    "layerBridgeTitle": "Мост WSS или WebRTC",
    "layerBridgeLead": "Единственный посредник, и он видит только шифр",
    "layerBridgeBody": "Браузер не умеет открывать прямые TCP-соединения, поэтому до первого узла I2P байты доходят через мост. Внутри уже зашифрованный трафик I2P поверх вашего сквозного шифрования. Мост пускает только к проверенным узлам I2P на допустимых портах. Он видит ваш IP, время и объём, но не ключи, не собеседников и не текст.",
    "layerNetworkTitle": "Сеть I2P и собеседник",
    "layerNetworkLead": "Туннели, почтовый релей и любой клиент на другой стороне",
    "layerNetworkBody": "Сообщение уходит через исходящий туннель в почтовый релей собеседника и ждёт там зашифрованным, пока он его не заберёт. Собеседник может сидеть в браузере, на компьютере или на Android: протокол один и тот же.",
    "devicesEyebrow": "Под капотом",
    "devicesTitle": "Как Gipny устроен на вашем устройстве.",
    "devicesIntro": "Везде одно ядро на Rust и один протокол. Отличается то, где живёт роутер I2P и чем защищено хранилище.",
    "deviceDesktopTitle": "Компьютер",
    "deviceDesktopTag": "Linux, Windows и macOS",
    "deviceDesktopLead": "Роутер i2pd внутри приложения, без открытых портов",
    "deviceDesktop1": "Роутер i2pd скомпилирован прямо в приложение: ставить I2P отдельно не нужно, а локальные порты (SAM, консоль, прокси) не открываются.",
    "deviceDesktop2": "Профиль хранится в базе SQLCipher. Её ключ выводится из пароля через Argon2id и привязан к хранилищу учётных данных системы, поэтому скопированные файлы профиля на другом компьютере не откроются.",
    "deviceDesktop3": "Обновления скачиваются через I2P и проверяются по контрольной сумме перед установкой.",
    "deviceDesktop4": "UPnP сам открывает порт на домашнем роутере, чтобы подключение было быстрее; это можно выключить в настройках.",
    "deviceDesktop5": "Окно сворачивается в трей, а приложение продолжает получать сообщения.",
    "deviceAndroidTitle": "Телефон на Android",
    "deviceAndroidTag": "Android 7 и новее, 64 бита",
    "deviceAndroidLead": "Тот же роутер в фоновой службе",
    "deviceAndroid1": "Роутер i2pd встроен в приложение и работает в фоновой службе, чтобы сообщения приходили и при свёрнутом окне.",
    "deviceAndroid2": "Ключ профиля защищён хранилищем ключей Android; копия данных приложения на другом телефоне не откроется.",
    "deviceAndroid3": "Снимки экрана и превью в списке задач запрещены системным флагом FLAG_SECURE.",
    "deviceAndroid4": "При переходе между Wi-Fi и мобильной сетью роутер сам перестраивает подключения.",
    "deviceBrowserTitle": "Браузер",
    "deviceBrowserTag": "Без установки",
    "deviceBrowserLead": "Всё ядро в WebAssembly на вашем устройстве",
    "deviceBrowser1": "Ядро и роутер I2P работают в Web Worker, профиль зашифрован в OPFS.",
    "deviceBrowser2": "Вход по паролю или по ключу доступа (passkey), если браузер поддерживает расширение PRF.",
    "deviceBrowser3": "Файлы до 2 ГиБ; при скачивании файл расшифровывается по частям, без промежуточной открытой копии.",
    "deviceBrowser4": "Узлы сети I2P запоминаются между запусками, поэтому повторный старт быстрее.",
    "deviceBrowser5": "Нужен транспортный мост: браузер не может сам соединиться с узлами I2P.",
    "deviceAgentTitle": "Агент на сервере или роутере",
    "deviceAgentTag": "Linux и OpenWrt",
    "deviceAgentLead": "Удалённая консоль без открытых портов",
    "deviceAgent1": "gipny-agent — программа без интерфейса со своим профилем и своим роутером I2P.",
    "deviceAgent2": "Вы управляете машиной из обычного чата: команды и их вывод идут зашифрованными сообщениями.",
    "deviceAgent3": "Команды выполняются только от назначенного хозяина; право проверяется прямо перед запуском.",
    "deviceAgent4": "Есть сборки для x86, ARM и роутеров OpenWrt; входящие порты на машине не нужны.",
    "securityEyebrow": "Слои защиты",
    "securityTitle": "Шесть уровней — от ключей до памяти.",
    "securityIntro": "Каждый слой закрывает свою угрозу. Откройте любой, чтобы увидеть, от чего он защищает и где проходит его граница.",
    "secIdentityTitle": "Личность без номера",
    "secIdentityLead": "Ключи создаются на вашем устройстве",
    "secIdentityBody": "При создании профиля генерируются ключ подписи Ed25519 и ключ обмена X25519. Ваш контакт — карточка с этими ключами, а не номер телефона. Сверьте отпечаток карточки по доверенному каналу: это защищает от подмены собеседника.",
    "secE2eTitle": "Сквозное шифрование",
    "secE2eLead": "X3DH и Double Ratchet",
    "secE2eBody": "Первое сообщение устанавливает сессию по X3DH, дальше каждое сообщение шифруется новым ключом Double Ratchet (ChaCha20-Poly1305). Утечка одного ключа не раскрывает прошлую переписку, а после компрометации сессия снова становится секретной. В обычном режиме сообщения дополняются до фиксированных размеров, чтобы длина не выдавала содержание.",
    "secNetworkTitle": "Сеть I2P",
    "secNetworkLead": "Собеседник и релеи не видят ваш IP",
    "secNetworkBody": "Сообщения идут через раздельные входящие и исходящие туннели I2P из нескольких узлов, и каждый узел знает только своих соседей. Адрес приложения в I2P новый при каждом запуске; постоянная личность — ваш ключ, а не сетевой адрес. Прямого IP-соединения с собеседником нет никогда.",
    "secDeliveryTitle": "Доставка без потерь",
    "secDeliveryLead": "Релей хранит только шифротекст",
    "secDeliveryBody": "Если собеседник не в сети, письмо ждёт его в почтовом релее зашифрованным. Получатель сначала сохраняет расшифрованное сообщение вместе с состоянием шифра одной транзакцией и только потом подтверждает релею доставку, поэтому сбой или переполненный диск не теряют сообщение.",
    "secStorageTitle": "Хранилище на устройстве",
    "secStorageLead": "Зашифровано и привязано к устройству",
    "secStorageBody": "Профиль — зашифрованная база (SQLCipher в приложениях, зашифрованный OPFS в браузере), ключ выводится из пароля через Argon2id. В приложениях ключ дополнительно привязан к защищённому хранилищу системы. По желанию история стирается при каждой блокировке, а сообщения — по таймеру.",
    "secMemoryTitle": "Память и удаление",
    "secMemoryLead": "Секреты не лежат открыто дольше нужного",
    "secMemoryBody": "Ключи в памяти хранятся запечатанными и затираются после использования. «Удалить у обоих» стирает сообщение и вложения на обоих устройствах и оставляет отметку, которую подтверждает собеседник; удалённое не возвращается при повторной доставке.",
    "featuresEyebrow": "Возможности",
    "featuresTitle": "Главное, что умеет Gipny.",
    "featNoPhoneTitle": "Без номера и почты",
    "featNoPhoneBody": "Профиль — это ключи на вашем устройстве. Можно держать несколько профилей с разными паролями.",
    "featFilesTitle": "Файлы до 2 ГиБ",
    "featFilesBody": "Файл идёт зашифрованными частями, докачивается после обрыва и проверяется по SHA-256, прежде чем станет доступен. Из изображений и PDF можно убирать метаданные (PDF — в приложениях).",
    "featDeleteTitle": "Удаление у обоих",
    "featDeleteBody": "Сообщение и его файлы исчезают у вас и у собеседника, а на их месте остаётся короткая отметка о том, кто и когда удалил. Собеседник подтверждает её кнопкой «ОК».",
    "featTimerTitle": "Исчезающие сообщения",
    "featTimerBody": "Задайте срок жизни сообщений в чате: по его истечении они удаляются на обоих устройствах. Историю можно не хранить вовсе и стирать при каждой блокировке.",
    "featNoticesTitle": "Отметки о копировании",
    "featNoticesBody": "Если собеседник скопировал ваш текст, сохранил ваше вложение или нажал сочетание клавиш снимка экрана, под сообщением появится отметка. Содержимое при этом никуда не передаётся. Это сигнал, а не запрет: сфотографировать экран телефоном по-прежнему можно.",
    "featGroupsTitle": "Группы",
    "featGroupsBody": "Группы до 64 участников. Каждое сообщение шифруется отдельно для каждого участника, а менять состав может только тот, кто уже в группе.",
    "featRequestsTitle": "Запросы контактов",
    "featRequestsBody": "Незнакомец не может сразу писать вам в чат: сначала приходит запрос, и до вашего согласия его файлы не скачиваются.",
    "featConsoleTitle": "Удалённая консоль",
    "featConsoleBody": "Назначьте контакт хозяином, и он сможет выполнять команды на вашем компьютере или сервере через зашифрованный чат. Режим включается явно и выключается одним нажатием.",
    "featSpeedTitle": "Режимы скорости",
    "featSpeedBody": "Обычный режим бережёт анонимность. Быстрые режимы укорачивают туннели и отключают выравнивание размеров ради скорости, и эту цену вы выбираете сами.",
    "screensEyebrow": "Как это выглядит",
    "screensTitle": "Привычный мессенджер с другой начинкой.",
    "screensIntro": "Снимки сделаны на демонстрационном профиле: имена и переписка вымышлены.",
    "screensCaption": "На снимках — отметка «Собеседник скопировал текст», удаление, которое собеседник подтверждает кнопкой «ОК», файл, запрос контакта и таймер исчезновения сообщений."
  },
  "en": {
    "title": "gipny.i2p — Private conversations over I2P.",
    "description": "An open-source messenger with end-to-end encryption, built-in I2P and local profiles without a phone number. Apps for desktop, Android and the browser.",
    "skip": "Skip to content",
    "navLabel": "Main navigation",
    "languageLabel": "Site language",
    "navPrinciples": "How it works",
    "navApps": "Get the app",
    "navBrowser": "Web version",
    "heroEyebrow": "End-to-end encrypted messaging",
    "heroTitleOne": "Your conversations.",
    "heroTitleTwo": "Your own keys.",
    "heroDescription": "Chat and share files without linking a phone number. Your keys and history stay on your device; the built-in router connects the app to I2P.",
    "download": "Get the native app",
    "source": "Explore the source",
    "availability": "Clients for Linux, Windows, macOS, Android and the browser.",
    "downloadsEyebrow": "Release packages",
    "downloadsTitle": "Download for your device.",
    "downloadsBody": "One download for your system. If your browser hides the processor type, choose your device below. All packages, checksums and installation notes are on GitHub Releases.",
    "catalogRelease": "Published release:",
    "pathEyebrow": "The path of a message",
    "pathSender": "Your device",
    "pathRecipient": "Their device",
    "pathSealed": "Encryption first. Sending second.",
    "pathCaption": "I2P carries the traffic; end-to-end encryption protects text and attachments. Relays forward and temporarily store ciphertext. The recipient decrypts it on their device.",
    "pathLocal": "Keys on your device",
    "pathNoAccount": "No phone number",
    "principlesEyebrow": "How it works",
    "principlesTitle": "Control starts on your device.",
    "identityTitle": "A contact key, not a phone number.",
    "identityBody": "Create a local profile without registering a phone number or email address. Exchange contact cards and compare fingerprints through a trusted channel.",
    "encryptionTitle": "Encrypted before sending.",
    "encryptionBody": "Private chats use X3DH and Double Ratchet. Messages and files are encrypted before reaching a relay and decrypted on the participants’ devices.",
    "networkTitle": "I2P comes with the app.",
    "networkBody": "There is no separate router to install. Delivery may be delayed when I2P or relays are unavailable; messages never fall back to a direct IP connection.",
    "securityLabel": "Protection and its limits",
    "securityBoundary": "Relays see delivery metadata. A compromised device can expose conversations. Copies saved by a recipient cannot be reliably erased remotely.",
    "i2pEyebrow": "I2P technology",
    "i2pTitle": "Private delivery with network trade-offs.",
    "i2pIntro": "I2P carries messages through encrypted tunnels. No phone number or direct connection to your contact is required.",
    "i2pProTunnelsTitle": "Garlic routing and unidirectional tunnels.",
    "i2pProTunnelsBody": "Separate inbound and outbound tunnels distribute the route across several nodes. Protection depends on the selected profile and the observer’s capabilities.",
    "i2pProIdentityTitle": "Cryptographic addresses instead of IP.",
    "i2pProIdentityBody": "Contacts exchange public keys and I2P addresses instead of IP addresses. Compare fingerprints through a trusted channel.",
    "i2pProBuiltinTitle": "Internal network without exit nodes.",
    "i2pProBuiltinBody": "The native app includes a router; the web client runs its router as WASM. Conversations are delivered inside I2P.",
    "i2pConBootstrapTitle": "Long cold start.",
    "i2pConBootstrapBody": "On first launch, the router obtains peer information and builds tunnels. This can take several minutes.",
    "i2pConLatencyTitle": "Latency and protocol overhead.",
    "i2pConLatencyBody": "Multi-hop routing and layered encryption increase response time, bandwidth usage and battery drain compared to centralized messengers.",
    "i2pConVisibilityTitle": "Transport and traffic observation.",
    "i2pConVisibilityBody": "Native I2P uses NTCP2/SSU2. The browser supports WSS and WebRTC through a transport bridge. These change the connection’s appearance, without guaranteeing indistinguishability; WebRTC and CDN paths need separate validation.",
    "nativeStatus": "Desktop and Android apps",
    "nativeTitle": "Choose the app for your device.",
    "nativeBody": "Private chats, groups, attachments and contact verification run in the app. The release page lists packages, architectures and installation notes; check that your system meets the requirements.",
    "nativeDownload": "See downloads",
    "stepOneTitle": "Get the app.",
    "stepOneBody": "Choose a package for your system and architecture. Read the installation notes and security requirements before starting.",
    "stepTwoTitle": "Create a local profile.",
    "stepTwoBody": "Choose a password. The app creates your identity and keys locally and requires the supported operating-system protection for storage. Give I2P time to connect.",
    "stepThreeTitle": "Exchange contact cards.",
    "stepThreeBody": "Send your card through a channel you trust and add the other person’s card. Compare fingerprints before sharing sensitive information.",
    "browserStatus": "Experimental WASM client",
    "browserTitle": "A standalone app. In your browser.",
    "browserBody": "You run the app on your own device: its WASM core encrypts messages, stores the profile and connects to I2P. GitHub Pages serves code and updates; GitHub servers do not participate in message delivery.",
    "browserTimeline": "The browser client is under validation. Online messaging needs an available transport bridge; failures do not trigger a hidden fallback.",
    "browserBoundary": "This is the project website. It does not send or receive messages, create profiles or hold your keys.",
    "browserOpen": "Open web messenger",
    "browserSetup": "Web client capabilities and requirements",
    "browserLocal": "HTTPS, WebAssembly, Workers and OPFS are required. Clearing site data deletes the local profile. Browser profile migration and encrypted export are not implemented yet.",
    "browserStandaloneBody": "Keys and encrypted history stay in OPFS. WASM runs in a dedicated Worker. The browser does not provide all native memory and storage protections.",
    "hostingLabel": "Visiting this site",
    "hostingPrivacy": "This page has no analytics or external scripts. GitHub Pages sees your IP and visit time. The web client trusts its hosting code; the transport bridge and TURN see network metadata.",
    "plansEyebrow": "Planned",
    "plansTitle": "What comes next.",
    "plansIntro": "Development directions; a roadmap entry is not an available feature.",
    "planNameTitle": "A name you can share.",
    "planNameBody": "Convenient names for sharing contacts. Ownership and impersonation resistance still need work; use contact cards and fingerprints today.",
    "planCallsTitle": "Secure video calls and conferences.",
    "planCallsBody": "Audio and video are a separate extension. A WebRTC transport for messaging does not by itself provide calls.",
    "planCertTitle": "Verifiable packages.",
    "planCertBody": "Independent publisher signatures and package provenance. Avoiding operating-system warnings depends on each platform’s requirements.",
    "plansBoundary": "The other person will still be able to save text or take a screenshot.",
    "openEyebrow": "Open development",
    "openTitle": "Read the code. Understand the limits.",
    "openBody": "The source, architecture and security documentation are public. Follow the work, review known limitations or report a problem.",
    "resourceCode": "Source code & issues",
    "resourceDocs": "Documentation RU / EN",
    "resourceSecurity": "Security policy",
    "footerAttribution": "An I2P fork of",
    "footerReleases": "All releases on GitHub",
    "catalogSystem": "Your device",
    "catalogChoose": "Choose your system and processor",
    "catalogDownload": "Download for",
    "catalogAll": "All releases on GitHub",
    "catalogHelp": "Your system is detected on your device; this information is not sent anywhere. You can change the selection.",
    "catalogUnavailable": "This release has no matching native package for this system. You can open the web client or browse all releases.",
    "storageTitle": "An encrypted local profile.",
    "storageBody": "History and keys are stored on your device. The native app uses protected OS storage; the web client uses encrypted browser storage. A password protects a locked profile, not a compromised system.",
    "attachmentTitle": "Protection for attachments, too.",
    "attachmentBody": "Files travel in encrypted chunks. The recipient verifies file integrity before confirming completion. Supported images undergo metadata cleaning when enabled; PDF cleaning is currently native-only.",
    "securityPracticeTitle": "Verify who you trust.",
    "securityPracticeBody": "Compare contact fingerprints through a separate channel. Update the app from a verified source. Open code makes the protection inspectable, but does not itself constitute an independent audit.",
    "browserDifferenceTitle": "What makes the Gipny web client different",
    "browserDifferenceBody": "Unlike a web interface controlling a messenger that runs on a server, Gipny does that work in your browser. It needs no GitHub account, installed phone app or pairing with one.",
    "browserDataTitle": "Your profile stays on your device",
    "browserDataBody": "Profiles are created locally. Passwords, keys and history are not sent to GitHub; messages are decrypted here as well.",
    "browserNetworkTitle": "Your own I2P router",
    "browserNetworkBody": "The WASM router builds I2P tunnels on your device. Browser restrictions require a WSS or WebRTC bridge to reach the network. It carries encrypted traffic and sees connection IP addresses, timing and volume.",
    "browserHostingTitle": "Hosting for installation and updates",
    "browserHostingBody": "After the browser has saved the app, its shell and history can open without the host. Messaging needs I2P, a working bridge and current peer information. First launch and new versions need an available source for application files.",
    "planMirrorsTitle": "Mirrors in Tor and I2P.",
    "planMirrorsBody": "We plan .onion and .i2p page mirrors with addresses containing gipny. Keys and addresses will be verified before publication; there are no live mirrors yet.",
    "planMirrorsLink": "Mirror and address verification plan",
    "navSecurity": "Security",
    "navDevices": "Devices",
    "heroWeb": "Open in the browser",
    "webCompareTitle": "How this differs from other messengers’ web versions",
    "webCompareLead": "Compare where the keys live and what the server does",
    "webCompareQuestion": "Question",
    "webCompareThem": "A typical web version",
    "webCompareUs": "Gipny in the browser",
    "webRowKeys": "Where the encryption keys live",
    "webRowKeysThem": "On the service’s servers or on the phone the browser is paired with",
    "webRowKeysUs": "Only in this browser, inside an encrypted profile",
    "webRowDecrypt": "Where messages are decrypted",
    "webRowDecryptThem": "Depends on the service: on its servers or on a linked phone",
    "webRowDecryptUs": "Right here, in the WebAssembly core on your device",
    "webRowHistory": "Where the history is kept",
    "webRowHistoryThem": "Usually on the service’s servers",
    "webRowHistoryUs": "In this browser’s encrypted OPFS storage",
    "webRowServer": "What the website’s server does",
    "webRowServerThem": "Receives, stores and forwards conversations",
    "webRowServerUs": "Only serves the application files and updates",
    "webRowAccount": "What signing in requires",
    "webRowAccountThem": "A phone number or an account with the service",
    "webRowAccountUs": "A password or passkey for the local profile",
    "webMagicTitle": "Where the magic happens",
    "webMagicIntro": "Open a layer to see what it does and what it can see.",
    "layerTabTitle": "The browser tab",
    "layerTabLead": "The same interface as the apps",
    "layerTabBody": "The window shows your chats and passes commands to the core. Keys and the profile store do not live in the page’s code: they sit in a separate thread. For WebRTC the window holds the connection, but only encrypted bytes pass through it.",
    "layerWorkerTitle": "Web Worker: Rust compiled to WebAssembly",
    "layerWorkerLead": "The messenger core and the I2P router run here",
    "layerWorkerBody": "A separate browser thread runs the same Rust core as the apps. X3DH and Double Ratchet encrypt every message, files are split into encrypted parts, and the built-in I2P router builds its own tunnels. The router remembers network peers between starts, so the next start is faster. The website’s server takes no part in any of this.",
    "layerVaultTitle": "An encrypted profile in OPFS",
    "layerVaultLead": "History and keys never leave the device",
    "layerVaultBody": "The profile lives in the browser’s private file system, encrypted with a key derived from your password through Argon2id, or from a passkey with the PRF extension where the browser supports it. Files up to 2 GiB are kept as separate encrypted blocks. Clearing the site’s data deletes the profile.",
    "layerBridgeTitle": "The WSS or WebRTC bridge",
    "layerBridgeLead": "The only intermediary, and it sees only ciphertext",
    "layerBridgeBody": "Browsers cannot open direct TCP connections, so bytes reach the first I2P router through a bridge. What passes is already-encrypted I2P traffic on top of your end-to-end encryption. The bridge only connects to verified I2P routers on allowed ports. It sees your IP, timing and volume, but not your keys, your contacts or any text.",
    "layerNetworkTitle": "The I2P network and your contact",
    "layerNetworkLead": "Tunnels, a mail relay and any client on the other side",
    "layerNetworkBody": "A message leaves through an outbound tunnel to your contact’s mail relay and waits there, encrypted, until they collect it. Your contact can be in a browser, on a desktop or on Android: the protocol is the same.",
    "devicesEyebrow": "Under the hood",
    "devicesTitle": "How Gipny works on your device.",
    "devicesIntro": "One Rust core and one protocol everywhere. What differs is where the I2P router lives and what protects storage.",
    "deviceDesktopTitle": "Desktop",
    "deviceDesktopTag": "Linux, Windows and macOS",
    "deviceDesktopLead": "The i2pd router inside the app, with no open ports",
    "deviceDesktop1": "The i2pd router is compiled into the app: no separate I2P install, and no local ports (SAM, console, proxies) are opened.",
    "deviceDesktop2": "The profile is a SQLCipher database. Its key is derived from your password through Argon2id and bound to the system’s credential store, so copied profile files will not open on another computer.",
    "deviceDesktop3": "Updates download through I2P and are checked against their checksum before installing.",
    "deviceDesktop4": "UPnP opens a port on your home router for faster connections; you can turn it off in Settings.",
    "deviceDesktop5": "The window minimises to the tray while the app keeps receiving messages.",
    "deviceAndroidTitle": "Android phone",
    "deviceAndroidTag": "Android 7 or later, 64-bit",
    "deviceAndroidLead": "The same router in a background service",
    "deviceAndroid1": "The i2pd router is built into the app and runs in a background service, so messages arrive while the app is minimised.",
    "deviceAndroid2": "The profile key is protected by the Android Keystore; a copy of the app’s data will not open on another phone.",
    "deviceAndroid3": "Screenshots and recent-apps previews are blocked by the system’s FLAG_SECURE flag.",
    "deviceAndroid4": "When you switch between Wi-Fi and mobile data, the router rebuilds its connections by itself.",
    "deviceBrowserTitle": "Browser",
    "deviceBrowserTag": "Nothing to install",
    "deviceBrowserLead": "The whole core in WebAssembly on your device",
    "deviceBrowser1": "The core and the I2P router run in a Web Worker; the profile is encrypted in OPFS.",
    "deviceBrowser2": "Sign in with a password, or with a passkey where the browser supports the PRF extension.",
    "deviceBrowser3": "Files up to 2 GiB; downloads are decrypted part by part, without an intermediate plain copy.",
    "deviceBrowser4": "I2P peers are remembered between starts, so the next start is faster.",
    "deviceBrowser5": "A transport bridge is required: a browser cannot connect to I2P routers by itself.",
    "deviceAgentTitle": "An agent on a server or router",
    "deviceAgentTag": "Linux and OpenWrt",
    "deviceAgentLead": "A remote console with no open ports",
    "deviceAgent1": "gipny-agent is a headless program with its own profile and its own I2P router.",
    "deviceAgent2": "You run the machine from an ordinary chat: commands and their output travel as encrypted messages.",
    "deviceAgent3": "Commands run only for the assigned owner, and that permission is checked right before each one runs.",
    "deviceAgent4": "Builds exist for x86, ARM and OpenWrt routers; the machine needs no inbound ports.",
    "securityEyebrow": "Layers of protection",
    "securityTitle": "Six layers, from your keys to memory.",
    "securityIntro": "Each layer covers a different threat. Open any of them to see what it protects against and where it stops.",
    "secIdentityTitle": "Identity without a phone number",
    "secIdentityLead": "Keys are created on your device",
    "secIdentityBody": "Creating a profile generates an Ed25519 signing key and an X25519 exchange key. Your contact is a card with these keys, not a phone number. Compare the card’s fingerprint through a trusted channel: it protects you from an impostor.",
    "secE2eTitle": "End-to-end encryption",
    "secE2eLead": "X3DH and Double Ratchet",
    "secE2eBody": "The first message sets up a session with X3DH; after that every message is encrypted with a fresh Double Ratchet key (ChaCha20-Poly1305). One leaked key does not reveal past conversations, and the session regains secrecy after a compromise. In the normal mode messages are padded to fixed sizes so their length does not give their content away.",
    "secNetworkTitle": "The I2P network",
    "secNetworkLead": "Contacts and relays do not see your IP",
    "secNetworkBody": "Messages travel through separate inbound and outbound I2P tunnels made of several routers, each knowing only its neighbours. The app’s I2P address is new on every start; your lasting identity is your key, not a network address. There is never a direct IP connection to your contact.",
    "secDeliveryTitle": "Delivery that does not lose messages",
    "secDeliveryLead": "Relays hold only ciphertext",
    "secDeliveryBody": "If your contact is offline, the letter waits for them in a mail relay, encrypted. The recipient first saves the decrypted message together with the cipher state in one transaction, and only then confirms delivery to the relay, so a crash or a full disk does not lose the message.",
    "secStorageTitle": "Storage on the device",
    "secStorageLead": "Encrypted and bound to the device",
    "secStorageBody": "The profile is an encrypted database (SQLCipher in the apps, encrypted OPFS in the browser), with a key derived from your password through Argon2id. In the apps the key is also bound to the system’s protected storage. If you wish, history is wiped at every lock and messages expire on a timer.",
    "secMemoryTitle": "Memory and deletion",
    "secMemoryLead": "Secrets are not left exposed longer than needed",
    "secMemoryBody": "Keys are kept sealed in memory and wiped after use. Delete for both erases the message and its attachments on both devices and leaves a notice your contact confirms; deleted content does not come back with a repeated delivery.",
    "featuresEyebrow": "Features",
    "featuresTitle": "What Gipny does best.",
    "featNoPhoneTitle": "No phone number, no email",
    "featNoPhoneBody": "A profile is a set of keys on your device. You can keep several profiles with different passwords.",
    "featFilesTitle": "Files up to 2 GiB",
    "featFilesBody": "A file travels in encrypted parts, resumes after interruptions and is checked with SHA-256 before it becomes available. Metadata can be stripped from images and PDFs (PDFs in the apps).",
    "featDeleteTitle": "Delete for both",
    "featDeleteBody": "The message and its files disappear for you and your contact, leaving a short notice of who deleted it and when. Your contact confirms it with an OK button.",
    "featTimerTitle": "Disappearing messages",
    "featTimerBody": "Set how long messages in a chat live: when it runs out, they are deleted on both devices. You can also keep no history at all and wipe it at every lock.",
    "featNoticesTitle": "Copy notices",
    "featNoticesBody": "If your contact copies your text, saves your attachment or presses a screenshot shortcut, a notice appears under the message. No content is sent anywhere. It is a signal, not a lock: a phone camera can still photograph the screen.",
    "featGroupsTitle": "Groups",
    "featGroupsBody": "Groups of up to 64 members. Each message is encrypted separately for every member, and only someone already in the group can change who is in it.",
    "featRequestsTitle": "Contact requests",
    "featRequestsBody": "A stranger cannot write straight into your chats: a request arrives first, and their files are not downloaded until you accept.",
    "featConsoleTitle": "Remote console",
    "featConsoleBody": "Make a contact the owner and they can run commands on your computer or server through an encrypted chat. The mode is switched on explicitly and off with a single tap.",
    "featSpeedTitle": "Speed modes",
    "featSpeedBody": "The normal mode protects anonymity. Faster modes shorten tunnels and turn off size padding for speed, and you choose that trade yourself.",
    "screensEyebrow": "What it looks like",
    "screensTitle": "A familiar messenger with different insides.",
    "screensIntro": "Screenshots of a demo profile: the names and messages are made up.",
    "screensCaption": "Shown: a copy notice, a deletion your contact confirms with OK, a file, a contact request and the disappearing-message timer."
  }
};

/** An explicit ?lang= always wins. Otherwise Russian only for a Russian
 *  system — a Russian locale among the preferred ones, or a Russian keyboard
 *  layout — and English for everyone else. */
export function resolveLanguage(search = '', languages = [], russianLayout = false) {
  const explicit = new URLSearchParams(search).get('lang');
  if (Object.hasOwn(messages, explicit)) return explicit;
  return russianLayout || languages.some(tag => /^ru(?:-|$)/i.test(String(tag ?? ''))) ? 'ru' : 'en';
}

/** Letters only a Russian layout has together (Ukrainian lacks ы/ъ/э,
 *  Belarusian lacks ъ), from KeyboardLayoutMap values. */
export function isRussianLayout(keys) {
  const letters = new Set(keys);
  return ['ы', 'ъ', 'э'].every(letter => letters.has(letter));
}

function preferredLanguages(browser) {
  const navigator = browser.navigator || {};
  const languages = [...(navigator.languages || []), navigator.language];
  // The system locale, which can differ from the browser's UI language.
  try { languages.push(browser.Intl?.DateTimeFormat().resolvedOptions().locale); } catch { /* absent or restricted */ }
  return languages.filter(Boolean);
}

/** UA hints stay local. macOS/Windows may report Intel while running on ARM. */
export function detectDevice(navigator = {}, hints = {}) {
  const ua = navigator.userAgent || '';
  const platform = hints.platform || navigator.userAgentData?.platform || navigator.platform || '';
  const ios = /iPhone|iPad|iPod/i.test(ua) || (/Mac/i.test(platform + ua) && navigator.maxTouchPoints > 1);
  const os = ios ? 'iOS' : /Android/i.test(platform + ua) ? 'Android' : /Win/i.test(platform + ua) ? 'Windows'
    : /Mac/i.test(platform + ua) ? 'macOS' : /CrOS/i.test(ua) ? 'ChromeOS' : /Linux/i.test(platform + ua) ? 'Linux' : null;
  let architecture = null;
  if (hints.architecture === 'arm') architecture = hints.bitness === '64' ? 'arm64' : hints.bitness === '32' ? 'arm' : null;
  else if (hints.architecture === 'x86') architecture = hints.bitness === '64' ? 'x64' : hints.bitness === '32' ? 'x86' : null;
  else if (/aarch64|arm64/i.test(ua + ' ' + platform)) architecture = 'arm64';
  else if (/armv7/i.test(ua + ' ' + platform)) architecture = 'arm';
  else if (os === 'Linux' && /x86_64|amd64/i.test(ua + ' ' + platform)) architecture = 'x64';
  return { platform: os, architecture };
}

export function chooseDownload(choices, device) {
  if (!device.platform || !device.architecture) return null;
  return choices.find(choice => choice.platform === device.platform && choice.architecture === device.architecture) || null;
}

function renderDownload(root) {
  const select = root.querySelector('#download-system');
  if (!select) return;
  const button = root.querySelector('#download-primary');
  const detail = root.querySelector('#download-detail');
  const option = select.selectedOptions[0];
  const copy = messages[root.documentElement.lang] || messages.ru;
  const web = select.dataset.web === 'true' && !select.value;
  button.hidden = !select.value && !web;
  button.textContent = web ? copy.browserOpen : option?.value ? copy.catalogDownload + ' ' + option.dataset.platform : '';
  if (select.value || web) button.setAttribute('href', web ? './app/browser.html' : select.value);
  else button.removeAttribute('href');
  detail.textContent = select.value ? option.textContent + ' · ' + option.dataset.size : '';
  root.querySelector('#download-unavailable').hidden = !web;
}

async function initializeDownloads(root, browser) {
  const select = root.querySelector('#download-system');
  if (!select) return;
  root.querySelector('#download-choice').hidden = false;
  let chosen = false;
  select.addEventListener('change', () => { chosen = true; renderDownload(root); });
  const choices = [...select.options].filter(option => option.value).map(option => ({ ...option.dataset, value: option.value }));
  const apply = hints => {
    if (chosen) return; // A late hint must not undo a person's choice.
    const device = detectDevice(browser.navigator, hints);
    const match = chooseDownload(choices, device);
    select.value = match?.value || '';
    select.dataset.web = String(Boolean(device.platform && (!choices.some(choice => choice.platform === device.platform) || (device.architecture && !match))));
    renderDownload(root);
  };
  apply({});
  try {
    const hints = await browser.navigator?.userAgentData?.getHighEntropyValues(['architecture', 'bitness']);
    if (hints) apply(hints);
  } catch { /* Privacy-restricted browsers keep the explicit device selector. */ }
}

export function applyLanguage(language, root) {
  const selected = Object.hasOwn(messages, language) ? language : 'ru';
  const copy = messages[selected];
  root.documentElement.lang = selected;
  root.title = copy.title;
  root.querySelector('meta[name="description"]').setAttribute('content', copy.description);
  root.querySelector('meta[property="og:title"]').setAttribute('content', copy.title);
  root.querySelector('meta[property="og:description"]').setAttribute('content', copy.description);
  root.querySelector('meta[property="og:locale"]').setAttribute('content', selected === 'ru' ? 'ru_RU' : 'en_US');
  root.querySelector('meta[property="og:locale:alternate"]').setAttribute('content', selected === 'ru' ? 'en_US' : 'ru_RU');
  for (const node of root.querySelectorAll('[data-i18n]')) node.textContent = copy[node.dataset.i18n];
  for (const node of root.querySelectorAll('[data-i18n-aria]')) node.setAttribute('aria-label', copy[node.dataset.i18nAria]);
  for (const button of root.querySelectorAll('[data-language]')) button.setAttribute('aria-pressed', String(button.dataset.language === selected));
  renderDownload(root);
  return selected;
}

export function initialize(root, browser) {
  const languages = preferredLanguages(browser);
  let russianLayout = false;
  let chosen = false;
  const preferred = () => resolveLanguage(browser.location.search, languages, russianLayout);
  applyLanguage(preferred(), root);
  void initializeDownloads(root, browser);
  root.querySelector('.language-switch').hidden = false;
  // The keyboard layout is only known asynchronously (Chromium only); a
  // Russian one switches to Russian unless a link or a click already chose.
  browser.navigator?.keyboard?.getLayoutMap?.().then(map => {
    russianLayout = isRussianLayout(map.values());
    if (russianLayout && !chosen && !new URLSearchParams(browser.location.search).has('lang')) applyLanguage('ru', root);
  }).catch(() => { /* No layout information: the locale decides. */ });
  for (const button of root.querySelectorAll('[data-language]')) {
    button.addEventListener('click', () => {
      chosen = true;
      const selected = applyLanguage(button.dataset.language, root);
      const url = new URL(browser.location.href);
      url.searchParams.set('lang', selected);
      browser.history.replaceState(null, '', url);
    });
  }
  browser.addEventListener('popstate', () => applyLanguage(preferred(), root));
}

if (typeof document !== 'undefined') initialize(document, window);
