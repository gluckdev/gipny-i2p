/**
 * A mocked Tauri backend for looking at the UI in a plain browser.
 *
 * `npm run dev`, then open /dev/preview.html. Nobody can see the mobile layout
 * without a CI-built APK otherwise, which is how a sidebar three screens wide
 * shipped. The fixtures are hostile on purpose: host names, b32 addresses and
 * 516-character destinations with no break opportunity, long unbroken tokens
 * inside messages, a group with a sentence for a name. If the layout survives
 * these it survives real data.
 *
 * Not part of the production bundle: Vite builds from index.html only.
 *
 * Query parameters of /dev/frame.html:
 *   scene = list | chat | console | settings | identity | add-contact | agent | auth | boot
 *   theme = light | dark
 */
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import type { AgentMaster, Attachment, Contact, Group, GroupMember, Message } from '../src/api';
import { CONSOLE_COMMAND, CONSOLE_GRANT, CONSOLE_OUTPUT } from '../src/api';

const params = new URLSearchParams(location.search);
const scene = params.get('scene') ?? 'list';
const theme = params.get('theme');
// ?relay=starting shows the app before its built-in relay has an address.
// The `boot` scene is the loading screen itself: the relay is still coming up,
// and vault_unlock takes as long as it does in life instead of resolving at once.
const relayStarting = params.get('relay') === 'starting' || scene === 'boot';

const DEST = 'q7Hk2-~Lm9XzRt'.repeat(37).slice(0, 516) + 'AAAA';
const RELAY = 'Vb8~nP3-sQw1YeUi'.repeat(33).slice(0, 516) + 'AAAA';
const B32 = 'zqeubwvvtm5f3cto5s3r6ftj36j4x4nsvvh3znux7uruhxjgk5za.b32.i2p';
const hex = (seed: string): string => seed.repeat(64).slice(0, 64);
const now = Date.now();
const min = 60_000;

const contacts: Contact[] = [
  {
    id: 1, sign_pk: hex('a1'), dh_pk: hex('b2'), onion: DEST,
    name: 'gpu-worker-17.eu-central-1.compute.internal.example-company.net',
    trust: 1, created_at: now - 900 * min, last_seen: now - 2 * min, is_bot: false,
    pinned_at: now - 800 * min, last_message_at: now - 3 * min, relay: RELAY, agent_granted: true,
    request: 'none',
  },
  {
    id: 2, sign_pk: hex('c3'), dh_pk: hex('d4'), onion: B32, name: 'Анна',
    trust: 1, created_at: now - 700 * min, last_seen: now - 1 * min, is_bot: false,
    pinned_at: null, last_message_at: now - 1 * min, relay: null, agent_granted: false,
    request: 'none',
  },
  {
    id: 3, sign_pk: hex('e5'), dh_pk: hex('f6'), onion: DEST,
    name: 'a_single_token_name_without_spaces_or_any_other_break_opportunity_0123456789abcdef',
    trust: 0, created_at: now - 600 * min, last_seen: null, is_bot: false,
    pinned_at: null, last_message_at: now - 90 * min, relay: null, agent_granted: false,
    request: 'outgoing',
  },
  {
    id: 4, sign_pk: hex('07'), dh_pk: hex('18'), onion: B32, name: 'deploy-bot',
    trust: 1, created_at: now - 500 * min, last_seen: now - 30 * min, is_bot: true,
    pinned_at: null, last_message_at: now - 240 * min, relay: null, agent_granted: false,
    request: 'none',
  },
  {
    id: 5, sign_pk: hex('29'), dh_pk: hex('3a'), onion: B32, name: 'backup-nas.home.arpa',
    trust: 2, created_at: now - 400 * min, last_seen: null, is_bot: false,
    pinned_at: null, last_message_at: null, relay: null, agent_granted: false,
    request: 'none',
  },
  {
    id: 6, sign_pk: hex('4b'), dh_pk: hex('5c'), onion: '', name: 'Stranger with a rather long display name',
    trust: 0, created_at: now - 5 * min, last_seen: null, is_bot: false,
    pinned_at: null, last_message_at: now - 4 * min, relay: RELAY, agent_granted: false,
    request: 'incoming',
  },
];

const groups: Group[] = [
  {
    id: hex('9f'), name: 'Команда инфраструктуры и эксплуатации — дежурные смены и инциденты',
    created_at: now - 300 * min, pinned_at: null, last_message_at: now - 12 * min,
  },
];

const members: GroupMember[] = [
  { sign_pk: hex('00'), dh_pk: hex('01'), onion: DEST, name: 'я', is_self: true },
  ...contacts.slice(0, 3).map((c) => ({ sign_pk: c.sign_pk, dh_pk: c.dh_pk, onion: c.onion, name: c.name, is_self: false })),
];

let nextId = 1000;
const msg = (p: Partial<Message> & { body: string }): Message => ({
  id: nextId++, contact_id: null, group_id: null, sender_sign_pk: null, outgoing: false,
  sent_at: now, sent: true, delivered: true, read: true, expires_at: null, buttons: null,
  reply_to: null, console: null, ...p,
});

const LONG_TOKEN = 'sha256:' + 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'.repeat(2);

const byContact: Record<number, Message[]> = {
  1: [
    msg({ contact_id: 1, body: '[agent on]', sent_at: now - 40 * min, console: { kind: CONSOLE_GRANT, exit_code: null, duration_ms: null, truncated: false } }),
    msg({ contact_id: 1, outgoing: true, body: 'uptime', sent_at: now - 30 * min, console: { kind: CONSOLE_COMMAND, exit_code: null, duration_ms: null, truncated: false } }),
    msg({ contact_id: 1, body: ' 18:42:07 up 41 days,  3:12,  2 users,  load average: 0.42, 0.37, 0.31\n', sent_at: now - 30 * min + 5000, console: { kind: CONSOLE_OUTPUT, exit_code: 0, duration_ms: 12, truncated: false } }),
    msg({ contact_id: 1, outgoing: true, body: 'ls -la /var/lib/docker/overlay2 | head -5', sent_at: now - 20 * min, console: { kind: CONSOLE_COMMAND, exit_code: null, duration_ms: null, truncated: false } }),
    msg({
      contact_id: 1, sent_at: now - 20 * min + 6000,
      body: 'total 1284\ndrwx--x--- 312 root root 36864 Sep 16 18:40 .\ndrwx--x---  12 root root  4096 Aug  2 09:11 ..\ndrwx--x---   4 root root  4096 Sep 11 07:55 0a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3e4f5061728394a5b6c7d8e9f\ndrwx--x---   4 root root  4096 Sep 11 07:55 0a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3e4f5061728394a5b6c7d8e9f-init\n',
      console: { kind: CONSOLE_OUTPUT, exit_code: 0, duration_ms: 48, truncated: true },
    }),
    msg({ contact_id: 1, outgoing: true, body: 'как там бэкап?', sent_at: now - 3 * min }),
  ],
  2: [
    msg({ contact_id: 2, body: 'привет! карточку релея пришлю ниже', sent_at: now - 50 * min }),
    msg({ contact_id: 2, body: `gipny:v2:${DEST}:${hex('c3')}:${hex('d4')}:${RELAY}`, sent_at: now - 49 * min }),
    msg({ contact_id: 2, outgoing: true, body: 'получил. контрольная сумма образа:\n' + LONG_TOKEN, sent_at: now - 45 * min }),
    msg({ contact_id: 2, body: 'ок, вот ссылка на инструкцию http://' + B32 + '/docs/installation/linux/systemd-unit-with-a-very-long-path-segment-that-never-ends.html', sent_at: now - 44 * min }),
    msg({ contact_id: 2, outgoing: true, body: 'фото со стенда', sent_at: now - 10 * min }),
    msg({ contact_id: 2, body: 'выглядит хорошо 👍', sent_at: now - 1 * min, buttons: [[{ text: 'Подтвердить выкладку на прод', callback_data: 'ok' }, { text: 'Отложить', callback_data: 'later' }]] }),
  ],
  3: [msg({ contact_id: 3, body: 'ping', sent_at: now - 90 * min })],
  4: [msg({ contact_id: 4, body: 'deploy #4182 finished: success', sent_at: now - 240 * min })],
};
const photoMsg = byContact[2]?.[4];

const byGroup: Record<string, Message[]> = {
  [groups[0]!.id]: [
    msg({ group_id: groups[0]!.id, sender_sign_pk: contacts[0]!.sign_pk, body: 'диск на ' + contacts[0]!.name + ' заполнен на 91%', sent_at: now - 14 * min }),
    msg({ group_id: groups[0]!.id, outgoing: true, body: 'смотрю', sent_at: now - 12 * min }),
  ],
};

const attachments: Record<number, Attachment[]> = photoMsg
  ? { [photoMsg.id]: [{ id: 1, message_id: photoMsg.id, name: 'IMG_20260916_165301_rack-B-row-3-position-17-front-panel-closeup.jpg', size: 2_481_152 }] }
  : {};

// 1×1 PNG, enough for an <img> to have something to decode.
const PIXEL = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYPj/HwADAgH/ur0HVwAAAABJRU5ErkJggg==';

const agentMaster: AgentMaster | null = scene === 'agent'
  ? { contact_id: 1, name: contacts[0]!.name, sign_pk: contacts[0]!.sign_pk }
  : null;

type Args = Record<string, unknown> | undefined;
const num = (a: Args, k: string): number => Number((a ?? {})[k] ?? 0);

let folders = JSON.stringify([
  { id: 'f-infra', name: 'Инфраструктура', collapsed: false, contacts: [1, 4, 5] },
  { id: 'f-empty', name: 'Пустая папка с очень длинным названием, которое не помещается', collapsed: true, contacts: [] },
]);

let avatars = '{}';

mockWindows('main');
mockIPC((cmd, payload) => {
  const a = payload as Args;
  switch (cmd) {
    case 'list_profiles': return scene === 'auth' ? [] : ['ops'];
    case 'vault_status': return { exists: true, unlocked: false };
    case 'vault_unlock': return scene === 'boot' ? new Promise(() => {}) : null;
    case 'my_card': return { sign_pk: hex('00'), dh_pk: hex('01') };
    case 'my_onion': return DEST;
    case 'my_fingerprint': return hex('5e');
    case 'my_bundle': return { sign_pk: hex('00'), dh_pk: hex('01'), signed_prekey: hex('aa'), signed_prekey_sig: hex('bb') + hex('cc'), one_time_prekey: null, one_time_id: null };
    case 'get_display_name': return 'admin@workstation-with-a-rather-long-hostname';
    case 'get_relay_address': return relayStarting ? '' : RELAY;
    case 'get_attachment_privacy': return true;
    case 'get_relay_info': return {
      mode: 'builtin', external: '',
      hosted: relayStarting ? { state: 'starting' } : { state: 'ready', address: RELAY },
    };
    case 'list_unreachable_contacts': return scene === 'chat' ? [2] : [];
    case 'get_agent_mode': return agentMaster;
    case 'get_router_settings': return { transit: 'balanced', yggdrasil: 'auto' };
    case 'update_configured': return false;
    case 'current_version': return '0.4.1';
    case 'list_apk_artifacts': return { version: '0.4.1', artifacts: [{ arch: 'arm64', size: 12_933_976 }, { arch: 'armv7', size: 10_919_808 }] };
    case 'check_update': return null;
    case 'update_installs_itself': return false;
    case 'read_previous_log': return '12:01:02 [i2p] router ready (SAM up on 7656)\n12:01:40 [relay-hosted] built-in relay ready\n12:09:55 [relay-client] peer relay 5AyDtq unreachable: Io';
    case 'log_settings': return { enabled: true, path: '/home/you/.local/share/gipny-i2p/debug.log' };
    case 'set_log_enabled': return null;
    case 'clear_debug_log': return null;
    case 'verify_passphrase': return String((payload as Record<string, unknown>)?.pass ?? '') === 'preview' ? 'ok' : Promise.reject('invalid passphrase');
    case 'qr_svg': return '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8" shape-rendering="crispEdges">'
      + '<rect width="8" height="8" fill="#fff"/><path d="M0 0h3v3H0zM5 0h3v3H5zM0 5h3v3H0zM4 4h1v1H4zM6 5h1v1H6zM5 7h2v1H5z" fill="#000"/></svg>';
    case 'restart_app': return null;
    case 'list_contacts': return contacts;
    case 'get_ui_data': return (a ?? {}).key === 'contact_folders' ? folders : (a ?? {}).key === 'avatars' ? avatars : null;
    case 'set_ui_data':
      if ((a ?? {}).key === 'contact_folders') folders = String((a ?? {}).json ?? '[]');
      if ((a ?? {}).key === 'avatars') avatars = String((a ?? {}).json ?? '{}');
      return null;
    case 'get_dht_status': return { peers: 14, items: 37, bytes: 912_384, joined: !relayStarting, stores: true, seeds: 1 };
    case 'get_contact': return contacts.find((c) => c.id === num(a, 'id')) ?? null;
    case 'list_groups': return groups;
    case 'list_group_members': return members;
    case 'list_muted': return [];
    case 'unread_count': return num(a, 'contactId') === 2 ? 1 : 0;
    case 'group_unread_count': return 2;
    // Newest first, as the core returns them; the store reverses.
    case 'list_messages': return [...(byContact[num(a, 'contactId')] ?? [])].reverse();
    case 'list_group_messages': return [...(byGroup[String((a ?? {})['groupId'])] ?? [])].reverse();
    case 'list_pinned_contact': return num(a, 'contactId') === 2 ? [byContact[2]![1]] : [];
    case 'list_pinned_group': return [];
    case 'list_attachments': return attachments[num(a, 'messageId')] ?? [];
    case 'load_attachment': return PIXEL;
    case 'list_media_contact': case 'list_media_group': case 'search_messages': return [];
    case 'message_position': return null;
    case 'read_debug_log': return '[i2p] router ready\n[session] relay connected & authed\n';
    case 'plugin:notification|is_permission_granted': return true;
    default:
      // Writes and everything unlisted: succeed quietly.
      return null;
  }
}, { shouldMockEvents: true });

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

async function waitFor(find: () => HTMLElement | null | undefined, timeout = 6000): Promise<HTMLElement | null> {
  const t0 = performance.now();
  while (performance.now() - t0 < timeout) {
    const el = find();
    if (el) return el;
    await sleep(50);
  }
  return null;
}

const byText = (root: string, text: string | RegExp): HTMLElement | undefined =>
  [...document.querySelectorAll<HTMLElement>(root)].find((el) => {
    const t = (el.textContent ?? '').trim();
    return typeof text === 'string' ? t === text : text.test(t);
  });

async function drive(): Promise<void> {
  if (scene === 'auth') return;
  // profile-select → unlock → main
  (await waitFor(() => byText('.auth-card button', /\bops$/)))?.click();
  const pass = await waitFor(() => document.querySelector<HTMLInputElement>('input[placeholder="пароль"]'));
  if (pass) {
    (pass as HTMLInputElement).value = 'preview';
    byText('.auth-card button', 'Открыть')?.click();
  }
  const { emit: emitBoot } = await import('@tauri-apps/api/event');
  // The boot screen is driven by `boot_status`; replay a plausible sequence so
  // it can be looked at without a backend. The `boot` scene stops here.
  const script: [string, string, string, number][] = [
    ['vault', 'active', 'unlocking the vault (argon2id)', 0],
    ['vault', 'done', 'profile opened (data.db)', 900],
    ['router', 'active', 'launching router /usr/lib/gipny-i2p/resources/i2pd (SAM 127.0.0.1:7656)', 200],
    ['tunnels', 'active', 'waiting for SAM: 1s of 180s', 1200],
    ['tunnels', 'active', 'waiting for SAM: 6s of 180s', 1500],
    ['tunnels', 'done', 'router ready (SAM up on 7656)', 1500],
    ['session', 'active', 'generating ephemeral destination for this session...', 300],
    ['session', 'done', 'SAM session open', 1200],
    ['core', 'active', 'starting the messenger core', 200],
    ['core', 'done', 'core running', 700],
    ['relay', 'active', 'building the relay\'s tunnels', 200],
  ];
  for (const [stage, state, detail, wait] of script) {
    await sleep(wait);
    await emitBoot('boot_status', { stage, state, detail });
  }
  if (scene === 'boot') return;
  await sleep(900);
  await emitBoot('boot_status', { stage: 'relay', state: 'done', detail: 'built-in relay ready at 5AyDtqoCoahb' });
  await emitBoot('boot_status', { stage: 'dht', state: 'done', detail: 'relay network: 3 node(s) known' });
  // The app waits for the relay before leaving the boot screen; the real core
  // announces it on `core_event`. Poll, because the listener is attached a few
  // awaits after the unlock resolves.
  const { emit } = await import('@tauri-apps/api/event');
  for (let i = 0; i < 40 && !document.querySelector('.main'); i++) {
    await emit('core_event', 'RelayConnected');
    await sleep(100);
  }
  await waitFor(() => document.querySelector<HTMLElement>('.main'));
  await sleep(150);

  const openContact = async (name: string): Promise<void> => {
    (await waitFor(() => [...document.querySelectorAll<HTMLElement>('.contact')].find((c) => (c.textContent ?? '').includes(name))))?.click();
    await waitFor(() => document.querySelector<HTMLElement>('.chat-header'));
  };

  if (scene === 'chat') await openContact('Анна');
  if (scene === 'console' || scene === 'agent') {
    await openContact('gpu-worker-17');
    if (scene === 'console') (await waitFor(() => byText('.agent-controls button', 'console')))?.click();
  }
  if (scene === 'settings') document.querySelector<HTMLElement>('.icon-btn[title="Настройки"]')?.click();
  if (scene === 'identity') document.querySelector<HTMLElement>('.sidebar-me')?.click();
  if (scene === 'add-contact') {
    document.querySelector<HTMLElement>('.icon-btn-accent')?.click();
    (await waitFor(() => byText('.ctx-menu-item', 'Добавить контакт')))?.click();
  }
}

await import('../src/main');
if (theme === 'light' || theme === 'dark') document.documentElement.setAttribute('data-theme', theme);
void drive();
