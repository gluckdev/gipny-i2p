export const messages = {
  "ru": {
    "title": "gipny.i2p — Личная переписка через I2P.",
    "description": "Мессенджер с открытым кодом: сквозное шифрование, встроенный I2P и локальные профили без номера телефона. Приложения для компьютеров, Android и браузера.",
    "skip": "Перейти к содержимому",
    "navLabel": "Основная навигация",
    "languageLabel": "Язык сайта",
    "navPrinciples": "Как работает",
    "navI2p": "Сеть I2P",
    "navApps": "Скачать",
    "navBrowser": "Открыть веб",
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
    "planMirrorsLink": "План зеркал и проверки адресов"
  },
  "en": {
    "title": "gipny.i2p — Private conversations over I2P.",
    "description": "An open-source messenger with end-to-end encryption, built-in I2P and local profiles without a phone number. Apps for desktop, Android and the browser.",
    "skip": "Skip to content",
    "navLabel": "Main navigation",
    "languageLabel": "Site language",
    "navPrinciples": "How it works",
    "navI2p": "I2P network",
    "navApps": "Get the app",
    "navBrowser": "Open web app",
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
    "planMirrorsLink": "Mirror and address verification plan"
  }
};

export function resolveLanguage(search = '') {
  const explicit = new URLSearchParams(search).get('lang');
  if (Object.hasOwn(messages, explicit)) return explicit;
  return 'ru';
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
  const preferred = () => resolveLanguage(browser.location.search);
  applyLanguage(preferred(), root);
  void initializeDownloads(root, browser);
  root.querySelector('.language-switch').hidden = false;
  for (const button of root.querySelectorAll('[data-language]')) {
    button.addEventListener('click', () => {
      const selected = applyLanguage(button.dataset.language, root);
      const url = new URL(browser.location.href);
      url.searchParams.set('lang', selected);
      browser.history.replaceState(null, '', url);
    });
  }
  browser.addEventListener('popstate', () => applyLanguage(preferred(), root));
}

if (typeof document !== 'undefined') initialize(document, window);
