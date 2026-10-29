export const messages = {
  en: {
    title: 'gipny.i2p — Private conversations over I2P.',
    description: 'An open-source messenger for desktop and Android: end-to-end encryption, built-in I2P and contacts without a phone number.',
    skip: 'Skip to content',
    navLabel: 'Main navigation',
    languageLabel: 'Site language',
    navPrinciples: 'How it works',
    navApps: 'Get the app',
    navBrowser: 'In the browser?',
    heroEyebrow: 'End-to-end encrypted messaging',
    heroTitleOne: 'Your conversations.',
    heroTitleTwo: 'Your own keys.',
    heroDescription: 'Chat and share files without linking a phone number. Your keys and history stay on your device; the built-in router connects the app to I2P.',
    download: 'Get the native app',
    source: 'Explore the source',
    availability: 'Downloads and installation notes for Linux, Windows, macOS and Android.',
    downloadsEyebrow: 'Release packages',
    downloadsTitle: 'Download for your device.',
    downloadsBody: 'Choose your system, architecture and format. These files belong to the published release; sizes refer to the downloadable packages.',
    catalogRelease: 'Published release:',
    catalogAdvanced: 'Agents, relays and OpenWrt',
    catalogAdvancedNote: 'Components for servers, automation and network nodes. Read the installation and storage-security requirements first; a package does not guarantee compatibility with every router.',
    catalogAgents: 'Agents',
    catalogRelays: 'Relays',
    catalogOpenWrt: 'OpenWrt / static musl agents',
    catalogSource: 'Full source archive',
    catalogVerification: 'Check the package against SHA256SUMS.txt. Source and build manifests are linked below when the release includes them.',
    pathEyebrow: 'The path of a message',
    pathSender: 'Your device',
    pathRecipient: 'Their device',
    pathSealed: 'Encryption first. Sending second.',
    pathCaption: 'I2P carries the traffic; end-to-end encryption protects text and attachments. Relays forward and temporarily store ciphertext. The recipient decrypts it on their device.',
    pathLocal: 'Keys on your device',
    pathNoAccount: 'No phone number',
    principlesEyebrow: 'How it works',
    principlesTitle: 'Control starts on your device.',
    identityTitle: 'A contact key, not a phone number.',
    identityBody: 'Create a local profile without registering a phone number or email address. Exchange contact cards and compare fingerprints through a trusted channel.',
    encryptionTitle: 'Encrypted before sending.',
    encryptionBody: 'Private chats use X3DH and Double Ratchet. Messages and files are encrypted before reaching a relay and decrypted on the participants’ devices.',
    networkTitle: 'I2P comes with the app.',
    networkBody: 'There is no separate router to install. Delivery may be delayed when I2P or relays are unavailable; messages never fall back to a direct IP connection.',
    securityLabel: 'Protection and its limits',
    securityBoundary: 'Relays see delivery metadata, and your internet provider may notice I2P use. A compromised device can expose your conversations. Copies the recipient saves separately cannot be deleted remotely.',
    nativeStatus: 'Desktop and Android apps',
    nativeTitle: 'Choose the app for your device.',
    nativeBody: 'Private chats, groups, attachments and contact verification run in the app. The release page lists packages, architectures and installation notes; check that your system meets the requirements.',
    nativeDownload: 'See downloads',
    stepOneTitle: 'Get the app.',
    stepOneBody: 'Choose a package for your system and architecture. Read the installation notes and security requirements before starting.',
    stepTwoTitle: 'Create a local profile.',
    stepTwoBody: 'Choose a password. The app creates your identity and keys locally and requires the supported operating-system protection for storage. Give I2P time to connect.',
    stepThreeTitle: 'Exchange contact cards.',
    stepThreeBody: 'Send your card through a channel you trust and add the other person’s card. Compare fingerprints before sharing sensitive information.',
    browserStatus: 'In development · not available yet',
    browserTitle: 'Coming soon: a web client.',
    browserBody: 'The code will run in your browser on your device; your keys, encryption and storage will stay with you. Hosting will only serve the site and code. Browser-to-I2P transport is still being worked out.',
    browserTimeline: 'We plan to release the web client before spring 2027.',
    browserBoundary: 'This is the project website. It does not send or receive messages, create profiles or hold your keys.',
    hostingLabel: 'Visiting this site',
    hostingPrivacy: 'This site has no analytics or external scripts. Its host, GitHub Pages, still sees page requests, including your IP address and access time. Opening the site does not itself enable I2P protection; the site has no access to your messages, contacts or keys.',
    plansEyebrow: 'Planned',
    plansTitle: 'Talk, then let it go.',
    plansIntro: 'These are plans for future versions. The current apps store conversation history locally.',
    planHistoryTitle: 'No history by default.',
    planHistoryBody: 'We plan to leave conversation history unsaved by default. At the start of each new chat, the app will explain this and offer encrypted local storage.',
    planNameTitle: 'A name you can share.',
    planNameBody: 'We want to add names you can share, such as @Петрович666, without a central account. A name will not replace a key: contact fingerprints will still need checking through a trusted channel.',
    planConsentTitle: 'Contacts with your consent.',
    planConsentBody: 'A contact request will arrive with a notification. The contact will appear in your list only after acceptance; you will be able to decline unwanted requests.',
    plansBoundary: 'The other person will still be able to save text or take a screenshot.',
    openEyebrow: 'Open development',
    openTitle: 'Read the code. Understand the limits.',
    openBody: 'The source, architecture and security documentation are public. Follow the work, review known limitations or report a problem.',
    resourceCode: 'Source code & issues',
    resourceDocs: 'Project documentation (Russian)',
    resourceSecurity: 'Security policy',
    footerAttribution: 'An I2P fork of',
    footerReleases: 'Release history',
  },
  ru: {
    title: 'gipny.i2p — Личная переписка через I2P.',
    description: 'Мессенджер с открытым кодом для компьютеров и Android: сквозное шифрование, встроенный I2P и контакты без номера телефона.',
    skip: 'Перейти к содержимому',
    navLabel: 'Основная навигация',
    languageLabel: 'Язык сайта',
    navPrinciples: 'Как работает',
    navApps: 'Скачать',
    navBrowser: 'А в браузере?',
    heroEyebrow: 'Переписка со сквозным шифрованием',
    heroTitleOne: 'Ваши разговоры.',
    heroTitleTwo: 'Ваши ключи.',
    heroDescription: 'Общайтесь и передавайте файлы без привязки к номеру телефона. Ключи и история хранятся на вашем устройстве, а встроенный роутер подключает приложение к I2P.',
    download: 'Скачать приложение',
    source: 'Открыть исходники',
    availability: 'Загрузки и инструкции для Linux, Windows, macOS и Android.',
    downloadsEyebrow: 'Готовые пакеты',
    downloadsTitle: 'Скачайте для своего устройства.',
    downloadsBody: 'Выберите систему, архитектуру и формат. Ниже — файлы опубликованного релиза; размер указан для скачиваемого пакета.',
    catalogRelease: 'Опубликованный релиз:',
    catalogAdvanced: 'Агенты, релеи и OpenWrt',
    catalogAdvancedNote: 'Компоненты для серверов, автоматизации и узлов сети. Перед установкой прочитайте инструкции и требования к защите хранилища: наличие пакета не гарантирует совместимость с любым роутером.',
    catalogAgents: 'Агенты',
    catalogRelays: 'Релеи',
    catalogOpenWrt: 'OpenWrt / статические агенты musl',
    catalogSource: 'Полный архив исходников',
    catalogVerification: 'Сверьте пакет с SHA256SUMS.txt. Ссылки на манифесты исходников и сборок приведены ниже, если они входят в релиз.',
    pathEyebrow: 'Путь сообщения',
    pathSender: 'Ваше устройство',
    pathRecipient: 'Устройство собеседника',
    pathSealed: 'Сначала шифрование — затем отправка.',
    pathCaption: 'I2P переносит трафик, а сквозное шифрование защищает текст и вложения. Релеи передают и временно хранят шифротекст; собеседник расшифровывает его на своём устройстве.',
    pathLocal: 'Ключи на вашем устройстве',
    pathNoAccount: 'Без номера телефона',
    principlesEyebrow: 'Как это устроено',
    principlesTitle: 'Контроль начинается с вашего устройства.',
    identityTitle: 'Ключ контакта вместо номера телефона.',
    identityBody: 'Профиль создаётся локально, без регистрации по телефону или почте. Обменяйтесь карточками контакта и сверьте отпечатки по доверенному каналу.',
    encryptionTitle: 'Шифрование до отправки.',
    encryptionBody: 'Личные чаты используют X3DH и Double Ratchet. Сообщения и файлы шифруются до передачи релеям, а расшифровываются на устройствах участников.',
    networkTitle: 'I2P встроен в приложение.',
    networkBody: 'Отдельный роутер устанавливать не нужно. При недоступности I2P или релеев доставка может задержаться; прямой IP-канал для сообщений не используется.',
    securityLabel: 'Защита и её границы',
    securityBoundary: 'Релеи видят служебные данные доставки, а интернет-провайдер может заметить использование I2P. Заражённое устройство может раскрыть переписку. Копии, которые собеседник сохранил отдельно, нельзя удалить удалённо.',
    nativeStatus: 'Приложения для компьютеров и Android',
    nativeTitle: 'Выберите приложение для своего устройства.',
    nativeBody: 'Личные чаты, группы, вложения и проверка контактов работают в приложении. На странице релиза есть пакеты, поддерживаемые архитектуры и инструкции; перед установкой проверьте требования к системе.',
    nativeDownload: 'Открыть загрузки',
    stepOneTitle: 'Скачайте приложение.',
    stepOneBody: 'Выберите пакет для своей системы и архитектуры. Прочитайте инструкцию по установке и требования к защите устройства.',
    stepTwoTitle: 'Создайте локальный профиль.',
    stepTwoBody: 'Задайте пароль. Приложение создаст личность и ключи локально; для хранилища нужен поддерживаемый механизм защиты операционной системы. Дайте I2P время подключиться.',
    stepThreeTitle: 'Обменяйтесь карточками.',
    stepThreeBody: 'Передайте свою карточку по доверенному каналу и добавьте карточку собеседника. Прежде чем обсуждать важное, сравните отпечатки контакта.',
    browserStatus: 'В разработке · пока недоступно',
    browserTitle: 'Скоро — веб-клиент.',
    browserBody: 'Код будет выполняться в браузере на вашем устройстве; ключи, шифрование и хранилище останутся у вас. Хостинг будет раздавать только сайт и код. Связь браузера с I2P ещё прорабатывается.',
    browserTimeline: 'Планируем выпустить веб-клиент до весны 2027 года.',
    browserBoundary: 'Перед вами сайт проекта. Он не отправляет и не принимает сообщения, не создаёт профили и не хранит ваши ключи.',
    hostingLabel: 'О посещении сайта',
    hostingPrivacy: 'На сайте нет аналитики и внешних скриптов. Но его хостинг, GitHub Pages, видит запросы страниц, включая IP-адрес и время посещения. Открытие сайта само по себе не включает защиту I2P; сайт не получает ваши сообщения, контакты или ключи.',
    plansEyebrow: 'В планах',
    plansTitle: 'Поговорили и забыли.',
    plansIntro: 'Это планы для будущих версий. Сейчас приложения сохраняют историю переписки локально.',
    planHistoryTitle: 'Без истории по умолчанию.',
    planHistoryBody: 'Планируем не сохранять историю переписки по умолчанию. В начале каждого нового чата приложение сообщит об этом и предложит включить зашифрованное локальное хранение.',
    planNameTitle: 'Имя, которым удобно делиться.',
    planNameBody: 'Хотим добавить имена для обмена — например, @Петрович666 — без центрального аккаунта. Имя не заменит ключ: отпечаток контакта по-прежнему нужно будет сверять по доверенному каналу.',
    planConsentTitle: 'Контакт — после вашего согласия.',
    planConsentBody: 'Запрос на добавление будет приходить с уведомлением. Контакт появится в списке только после принятия; нежелательный запрос можно будет отклонить.',
    plansBoundary: 'Собеседник всё равно сможет сохранить текст или сделать снимок экрана.',
    openEyebrow: 'Открытая разработка',
    openTitle: 'Открытый код. Понятные ограничения.',
    openBody: 'Исходники, архитектура и описание защиты доступны публично. Следите за разработкой, изучайте известные ограничения и сообщайте о проблемах.',
    resourceCode: 'Исходники и задачи',
    resourceDocs: 'Документация проекта',
    resourceSecurity: 'Политика безопасности',
    footerAttribution: 'I2P-форк проекта',
    footerReleases: 'История релизов',
  },
};

export function resolveLanguage(search = '') {
  const explicit = new URLSearchParams(search).get('lang');
  if (Object.hasOwn(messages, explicit)) return explicit;
  return 'ru';
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
  return selected;
}

export function initialize(root, browser) {
  const preferred = () => resolveLanguage(browser.location.search);
  applyLanguage(preferred(), root);
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
