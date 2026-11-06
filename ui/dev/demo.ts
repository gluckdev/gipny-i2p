/**
 * Presentable fixtures for screenshots of the app (site, README): `?demo=1`.
 *
 * The default fixtures in mock.ts are hostile on purpose, to break layouts;
 * these show what the app does instead — a file, a deletion notice the other
 * side confirms, a copy notice, a disappearing message, a group and the agent
 * console — with names and text in the selected language. Nothing here is a
 * real person, key or address.
 */
import type { Attachment, Contact, Group, GroupMember, Message } from '../src/api';
import { CONSOLE_COMMAND, CONSOLE_GRANT, CONSOLE_OUTPUT } from '../src/api';

type Lang = 'ru' | 'en';
type Make = (p: Partial<Message> & { body: string }) => Message;

const copy = {
  ru: {
    me: 'Вы', server: 'Домашний сервер', anna: 'Анна', mikhail: 'Михаил', olga: 'Ольга', igor: 'Игорь',
    group: 'Поход в горы', folder: 'Семья и друзья',
    a1: 'Привет! Скинешь фото со вчерашнего?', a2: 'Держи, самое красивое', a3: 'Какой вид 😍',
    a4: 'Код от домофона — 4521. Сотри, как запишешь', a5: 'Записала, спасибо! Удаляю',
    a6: 'Во сколько выходим в субботу?', photo: 'горы-на-рассвете.jpg',
    g1: 'Палатки беру я', g2: 'Тогда я — горелку и котелок', g3: 'Встречаемся у метро в 7:00',
    c2: 'df -h /backup', c3: 'Filesystem      Size  Used Avail Use% Mounted on\n/dev/sdb1       3.6T  2.1T  1.5T  59% /backup\n',
    c4: 'Бэкап за ночь прошёл, места хватает', request: 'Привет, это Игорь с работы',
  },
  en: {
    me: 'You', server: 'Home server', anna: 'Anna', mikhail: 'Mikhail', olga: 'Olga', igor: 'Igor',
    group: 'Mountain trip', folder: 'Family and friends',
    a1: 'Hi! Could you send the photos from yesterday?', a2: 'Here you go, the best one', a3: 'What a view 😍',
    a4: 'The door code is 4521. Delete it once you have it', a5: 'Got it, thanks! Deleting',
    a6: 'What time do we leave on Saturday?', photo: 'mountains-at-sunrise.jpg',
    g1: 'I will bring the tents', g2: 'Then I will take the stove and the pot', g3: 'Meet at the station at 7:00',
    c2: 'df -h /backup', c3: 'Filesystem      Size  Used Avail Use% Mounted on\n/dev/sdb1       3.6T  2.1T  1.5T  59% /backup\n',
    c4: 'Last night’s backup finished, plenty of space left', request: 'Hi, it is Igor from work',
  },
} as const;

export function demoFixtures(lang: Lang, now: number, hex: (seed: string) => string, msg: Make, address: string) {
  const c = copy[lang];
  const min = 60_000;
  const contact = (id: number, seed: string, name: string, extra: Partial<Contact> = {}): Contact => ({
    id, sign_pk: hex(seed), dh_pk: hex(seed + '0'), onion: address, name, trust: 1,
    created_at: now - 9000 * min, last_seen: now - 2 * min, is_bot: false, pinned_at: null,
    last_message_at: now - 30 * min, relay: null, agent_granted: false, request: 'none', ...extra,
  });
  const contacts: Contact[] = [
    contact(1, 'a1', c.server, { pinned_at: now - 800 * min, last_message_at: now - 6 * min, agent_granted: true }),
    contact(2, 'c3', c.anna, { last_message_at: now - 1 * min }),
    contact(3, 'e5', c.mikhail, { last_message_at: now - 25 * min }),
    contact(4, '07', c.olga, { last_message_at: now - 40 * min }),
    contact(5, '29', c.igor, { trust: 0, last_seen: null, last_message_at: now - 4 * min, request: 'incoming' }),
  ];
  const groups: Group[] = [{ id: hex('9f'), name: c.group, created_at: now - 3000 * min, pinned_at: null, last_message_at: now - 12 * min }];
  const members: GroupMember[] = [
    { sign_pk: hex('00'), dh_pk: hex('01'), onion: address, name: c.me, is_self: true },
    ...contacts.slice(1, 4).map((x) => ({ sign_pk: x.sign_pk, dh_pk: x.dh_pk, onion: x.onion, name: x.name, is_self: false })),
  ];
  const photo = msg({ contact_id: 2, outgoing: true, body: c.a2, sent_at: now - 44 * min, dm_seq: 2 });
  const byContact: Record<number, Message[]> = {
    1: [
      msg({ contact_id: 1, body: '[agent on]', sent_at: now - 30 * min, console: { kind: CONSOLE_GRANT, exit_code: null, duration_ms: null, truncated: false } }),
      msg({ contact_id: 1, outgoing: true, body: c.c2, sent_at: now - 8 * min, console: { kind: CONSOLE_COMMAND, exit_code: null, duration_ms: null, truncated: false } }),
      msg({ contact_id: 1, body: c.c3, sent_at: now - 8 * min + 4000, console: { kind: CONSOLE_OUTPUT, exit_code: 0, duration_ms: 18, truncated: false } }),
      msg({ contact_id: 1, body: c.c4, sent_at: now - 6 * min }),
    ],
    2: [
      msg({ contact_id: 2, body: c.a1, sent_at: now - 46 * min, dm_seq: 1, expires_at: now + 23 * 60 * min }),
      photo,
      msg({ contact_id: 2, body: c.a3, sent_at: now - 43 * min, dm_seq: 3, expires_at: now + 23 * 60 * min }),
      msg({ contact_id: 2, outgoing: true, body: c.a4, sent_at: now - 20 * min, dm_seq: 4, expires_at: now + 50 * min,
        peer_events: [{ kind: 5, at: now - 18 * min, count: 1 }] }),
      msg({ contact_id: 2, body: '', sent_at: now - 17 * min, dm_seq: 5, deleted_at: now - 16 * min,
        deletion_notice: { deleted_at: now - 16 * min, by_sender: true, can_ack: true } }),
      msg({ contact_id: 2, outgoing: true, body: c.a6, sent_at: now - 1 * min, dm_seq: 6, delivered: true }),
    ],
    3: [msg({ contact_id: 3, body: c.g1, sent_at: now - 25 * min })],
    4: [msg({ contact_id: 4, body: c.g3, sent_at: now - 40 * min })],
    5: [msg({ contact_id: 5, body: c.request, sent_at: now - 4 * min })],
  };
  const byGroup: Record<string, Message[]> = {
    [groups[0]!.id]: [
      msg({ group_id: groups[0]!.id, sender_sign_pk: contacts[2]!.sign_pk, body: c.g1, sent_at: now - 20 * min }),
      msg({ group_id: groups[0]!.id, outgoing: true, body: c.g2, sent_at: now - 15 * min }),
      msg({ group_id: groups[0]!.id, sender_sign_pk: contacts[3]!.sign_pk, body: c.g3, sent_at: now - 12 * min }),
    ],
  };
  const attachments: Record<number, Attachment[]> = { [photo.id]: [{ id: 1, message_id: photo.id, name: c.photo, size: 2_481_152 }] };
  const folders = JSON.stringify([{ id: 'f-people', name: c.folder, collapsed: false, contacts: [2, 3, 4] }]);
  return { contacts, groups, members, byContact, byGroup, attachments, folders, displayName: c.me, chatWith: c.anna, serverName: c.server };
}
