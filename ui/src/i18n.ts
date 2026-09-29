/**
 * Language support (English / Russian).
 *
 * Stored in localStorage ('gipny.lang'). Follows browser language by default if not set.
 */

export type Lang = 'ru' | 'en';

const KEY = 'gipny.lang';
const DEFAULT_LANG: Lang = 'ru';

export const translations = {
  ru: {
    // Common / Buttons
    'common.close': 'Закрыть',
    'common.cancel': 'Отмена',
    'common.save': 'Сохранить',
    'common.back': 'Назад',
    'common.delete': 'Удалить',
    'common.copied': 'Скопировано',
    'common.copy': 'Копировать',
    'common.or': 'или',
    'common.edit': 'Редактировать',
    'common.search': 'Поиск',

    // Sidebar & Actions
    'sidebar.search_placeholder': 'Поиск контактов',
    'sidebar.search_messages': 'Поиск по сообщениям',
    'sidebar.settings': 'Настройки',
    'sidebar.new_chat': 'Новый контакт или чат',
    'sidebar.new_contact': 'Новый контакт',
    'sidebar.new_group': 'Новая группа',
    'sidebar.new_folder': 'Новая папка',
    'sidebar.about': 'О gipny и безопасности',
    'sidebar.lock': 'Заблокировать',
    'sidebar.collapse': 'Свернуть список чатов',
    'sidebar.expand': 'Развернуть список чатов',
    'sidebar.need_contact_for_folder': 'Сначала добавьте хотя бы один контакт',
    'sidebar.all_chats': 'Все чаты',
    'sidebar.manage_folders': 'Папки',

    // Profile & Auth
    'profile.select_title': 'Выбор профиля',
    'profile.new_profile': '+ Новый профиль',
    'profile.import_backup': 'Импортировать копию',
    'profile.delete_confirm': 'стереть «{name}» и все его данные?',
    'profile.wipe_title': 'Удалить профиль',
    'profile.wiped_toast': 'профиль «{name}» стёрт',
    'auth.new_profile_title': 'Новый профиль',
    'auth.new_profile_sub': 'у каждого профиля свой i2p-адрес, ключи, контакты',
    'auth.profile_name': 'имя профиля',
    'auth.profile_name_hint': 'локально на этом устройстве (буквы, цифры, дефис)',
    'auth.display_name': 'отображаемое имя',
    'auth.display_name_hint': 'имя которое увидят твои контакты — приходит в каждом сообщении',
    'auth.passphrase': 'пароль',
    'auth.confirm_pass': 'подтверждение пароля',
    'auth.duress_title': 'защита под принуждением',
    'auth.duress_pass': 'пароль под принуждением',
    'auth.duress_pass_hint': 'альтернативный пароль для экстренного сценария',
    'auth.duress_wipe': 'при вводе этого пароля: СТЕРЕТЬ всё',
    'auth.max_attempts': 'макс. попыток (0 = без ограничений)',
    'auth.create_btn': 'Создать профиль',
    'auth.open_btn': 'Открыть',
    'auth.unlock_title': 'Открыть профиль',
    'auth.unlock_sub': 'введите пароль для разблокировки данных',
    'auth.pass_required': 'Введите пароль',
    'auth.wrong_pass': 'Неверный пароль',
    'auth.profile_wiped': 'Профиль стёрт',
    'auth.profile_not_selected': 'Профиль не выбран',
    'auth.net_building': 'Сеть i2p: строятся туннели, не дожидайтесь — вводите пароль',
    'auth.net_ready': 'Сеть i2p: туннели построены',

    // Booting / Connecting screen
    'boot.title': 'Открываю профиль',
    'boot.sub': 'Первый запуск занимает минуты: роутер ищет узлы i2p и строит туннели. Дальше быстрее.',
    'boot.elapsed': 'прошло {time} с',
    'boot.enter_now': 'Открыть чаты сейчас',
    'boot.details': 'Технические подробности',
    'boot.step.vault': 'Расшифровываю профиль',
    'boot.step.vault_hint': 'argon2id, это нагружает процессор',
    'boot.step.router': 'Запускаю роутер i2p',
    'boot.step.router_hint': 'он живёт рядом с приложением',
    'boot.step.tunnels': 'Строю туннели',
    'boot.step.tunnels_hint': 'самая долгая часть первого запуска',
    'boot.step.session': 'Получаю адрес в сети',
    'boot.step.session_hint': 'новый на каждый запуск',
    'boot.step.core': 'Готовлю переписку',
    'boot.step.core_hint': 'ключи, база, очереди',
    'boot.step.relay': 'Поднимаю свой релей',
    'boot.step.relay_hint': 'через него вам пишут',
    'boot.step.dht': 'Вхожу в сеть релеев',
    'boot.step.dht_hint': 'нужна для доставки в офлайне',

    // Chat
    'chat.attach_file': 'Прикрепить файл',
    'chat.input_placeholder': 'Сообщение… (Enter — отправить, Shift+Enter — новая строка)',
    'chat.send': 'Отправить',
    'chat.typing': 'печатает…',
    'chat.attach_failed': 'не удалось приложить файл: {error}',
    'chat.pick_failed': 'ошибка выбора файла: {error}',
    'chat.send_failed': 'не удалось отправить: {error}',
    'chat.saved': 'сохранено',
    'chat.save_failed': 'не удалось сохранить: {error}',
    'chat.reply_to': 'Ответ на',
    'chat.reply': 'Ответить',
    'chat.copy_text': 'Копировать текст',
    'chat.pin': 'Закрепить',
    'chat.unpin': 'Открепить',
    'chat.forward': 'Переслать',
    'chat.delete_msg': 'Удалить сообщение',
    'chat.empty_state': 'Выберите контакт или группу слева',

    // Settings
    'settings.title': 'Настройки',
    'settings.language': 'Язык интерфейса',
    'settings.lang_ru': 'Русский',
    'settings.lang_en': 'English',
    'settings.theme': 'Тема оформления',
    'settings.theme_light': 'светлая',
    'settings.theme_light_blurb': 'светлая и воздушная',
    'settings.theme_dark': 'тёмная',
    'settings.theme_dark_blurb': 'тёмная, для работы вечером',
    'settings.theme_system': 'как в системе',
    'settings.theme_system_blurb': 'следовать настройке операционной системы',
    'settings.version': 'Версия',
    'settings.relay': 'Релей',
    'settings.relay_builtin': 'встроенный',
    'settings.relay_builtin_blurb': 'релей внутри приложения, настраивать нечего. Почту принимает, пока приложение запущено.',
    'settings.relay_external': 'внешний',
    'settings.relay_external_blurb': 'релей на сервере: почтовый ящик, работающий и когда приложение закрыто.',
    'settings.save_relay': 'Сохранить адрес релея',
    'settings.router_i2p': 'Роутер i2p',
    'settings.security': 'Безопасность',
    'settings.backup': 'Резервная копия',
    'settings.export_backup': 'Экспортировать копию',
    'settings.debug_log': 'Журнал отладки',
    'settings.copy_log': 'Скопировать журнал',
    'settings.clear_log': 'Очистить журнал',
  },
  en: {
    // Common / Buttons
    'common.close': 'Close',
    'common.cancel': 'Cancel',
    'common.save': 'Save',
    'common.back': 'Back',
    'common.delete': 'Delete',
    'common.copied': 'Copied',
    'common.copy': 'Copy',
    'common.or': 'or',
    'common.edit': 'Edit',
    'common.search': 'Search',

    // Sidebar & Actions
    'sidebar.search_placeholder': 'Search contacts',
    'sidebar.search_messages': 'Search messages',
    'sidebar.settings': 'Settings',
    'sidebar.new_chat': 'New contact or chat',
    'sidebar.new_contact': 'New contact',
    'sidebar.new_group': 'New group',
    'sidebar.new_folder': 'New folder',
    'sidebar.about': 'About gipny & security',
    'sidebar.lock': 'Lock',
    'sidebar.collapse': 'Collapse chat list',
    'sidebar.expand': 'Expand chat list',
    'sidebar.need_contact_for_folder': 'Add at least one contact first',
    'sidebar.all_chats': 'All chats',
    'sidebar.manage_folders': 'Folders',

    // Profile & Auth
    'profile.select_title': 'Select profile',
    'profile.new_profile': '+ New profile',
    'profile.import_backup': 'Import backup',
    'profile.delete_confirm': 'wipe "{name}" and all its data?',
    'profile.wipe_title': 'Delete profile',
    'profile.wiped_toast': 'profile "{name}" wiped',
    'auth.new_profile_title': 'New profile',
    'auth.new_profile_sub': 'each profile has its own i2p address, keys, and contacts',
    'auth.profile_name': 'profile name',
    'auth.profile_name_hint': 'local to this device (alphanumeric + dash/underscore)',
    'auth.display_name': 'display name',
    'auth.display_name_hint': 'name visible to contacts — sent with every message',
    'auth.passphrase': 'passphrase',
    'auth.confirm_pass': 'confirm passphrase',
    'auth.duress_title': 'duress protection',
    'auth.duress_pass': 'duress passphrase',
    'auth.duress_pass_hint': 'alternate passphrase that triggers fail-safe',
    'auth.duress_wipe': 'on duress: WIPE everything',
    'auth.max_attempts': 'max attempts (0 = unlimited)',
    'auth.create_btn': 'Create profile',
    'auth.open_btn': 'Unlock',
    'auth.unlock_title': 'Unlock profile',
    'auth.unlock_sub': 'enter your passphrase to unlock data',
    'auth.pass_required': 'Enter passphrase',
    'auth.wrong_pass': 'Invalid passphrase',
    'auth.profile_wiped': 'Profile wiped',
    'auth.profile_not_selected': 'Profile not selected',
    'auth.net_building': 'i2p network: building tunnels, you can type password now',
    'auth.net_ready': 'i2p network: tunnels ready',

    // Booting / Connecting screen
    'boot.title': 'Opening profile',
    'boot.sub': 'First launch takes a few minutes: the router finds i2p peers and builds tunnels. Subsequent launches are faster.',
    'boot.elapsed': 'elapsed {time} s',
    'boot.enter_now': 'Open chats now',
    'boot.details': 'Technical details',
    'boot.step.vault': 'Decrypting profile',
    'boot.step.vault_hint': 'argon2id key derivation',
    'boot.step.router': 'Starting i2p router',
    'boot.step.router_hint': 'embedded alongside application',
    'boot.step.tunnels': 'Building tunnels',
    'boot.step.tunnels_hint': 'longest step of initial setup',
    'boot.step.session': 'Acquiring network destination',
    'boot.step.session_hint': 'fresh session per launch',
    'boot.step.core': 'Preparing messages',
    'boot.step.core_hint': 'keys, database, message queues',
    'boot.step.relay': 'Starting local relay',
    'boot.step.relay_hint': 'allows contacts to deliver messages',
    'boot.step.dht': 'Joining relay network',
    'boot.step.dht_hint': 'required for offline delivery',

    // Chat
    'chat.attach_file': 'Attach file',
    'chat.input_placeholder': 'Message… (Enter to send, Shift+Enter for new line)',
    'chat.send': 'Send',
    'chat.typing': 'typing…',
    'chat.attach_failed': 'failed to attach file: {error}',
    'chat.pick_failed': 'file pick failed: {error}',
    'chat.send_failed': 'send failed: {error}',
    'chat.saved': 'saved',
    'chat.save_failed': 'save failed: {error}',
    'chat.reply_to': 'Reply to',
    'chat.reply': 'Reply',
    'chat.copy_text': 'Copy text',
    'chat.pin': 'Pin',
    'chat.unpin': 'Unpin',
    'chat.forward': 'Forward',
    'chat.delete_msg': 'Delete message',
    'chat.empty_state': 'Select a contact or group on the left',

    // Settings
    'settings.title': 'Settings',
    'settings.language': 'Language',
    'settings.lang_ru': 'Русский',
    'settings.lang_en': 'English',
    'settings.theme': 'Theme',
    'settings.theme_light': 'light',
    'settings.theme_light_blurb': 'bright and airy',
    'settings.theme_dark': 'dark',
    'settings.theme_dark_blurb': 'dark, easier on eyes at night',
    'settings.theme_system': 'system',
    'settings.theme_system_blurb': 'follow operating system setting',
    'settings.version': 'Version',
    'settings.relay': 'Relay',
    'settings.relay_builtin': 'built-in',
    'settings.relay_builtin_blurb': 'built-in relay, zero configuration. Receives mail while app is running.',
    'settings.relay_external': 'external',
    'settings.relay_external_blurb': 'mailbox on server: works even when app is closed.',
    'settings.save_relay': 'Save relay address',
    'settings.router_i2p': 'i2p Router',
    'settings.security': 'Security',
    'settings.backup': 'Backup',
    'settings.export_backup': 'Export backup',
    'settings.debug_log': 'Debug log',
    'settings.copy_log': 'Copy log',
    'settings.clear_log': 'Clear log',
  },
} as const;

export type TranslationKey = keyof typeof translations['ru'];

let currentLang: Lang = (() => {
  try {
    const saved = localStorage.getItem(KEY);
    if (saved === 'ru' || saved === 'en') return saved;
    const nav = (navigator.language || navigator.languages?.[0] || '').toLowerCase();
    if (nav.startsWith('ru') || nav.startsWith('be') || nav.startsWith('uk')) return 'ru';
  } catch {
    // fallback
  }
  return 'en';
})();

const listeners = new Set<(lang: Lang) => void>();

export function getLang(): Lang {
  return currentLang;
}

export function setLang(lang: Lang): void {
  if (currentLang === lang) return;
  currentLang = lang;
  try {
    localStorage.setItem(KEY, lang);
  } catch {}
  for (const fn of listeners) fn(lang);
}

export function onLangChange(fn: (lang: Lang) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function t(key: TranslationKey, params?: Record<string, string | number>): string {
  const dict = translations[currentLang] || translations.ru;
  let text: string = dict[key] || translations.ru[key] || key;
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      text = text.replace(new RegExp(`\\{${k}\\}`, 'g'), String(v));
    }
  }
  return text;
}
