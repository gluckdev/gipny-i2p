import { h } from './view';

export class SecurityModal {
  el: HTMLElement;
  constructor(close: () => void) {
    const featureRow = (name: string, desc: string): HTMLElement =>
      h('div', { style: { marginBottom: '10px' } },
        h('div', { style: { color: '#33ff66', fontSize: '13px', fontWeight: 'bold' } }, `◆ ${name}`),
        h('div', { class: 'hint', style: { marginLeft: '14px', fontSize: '12px' } }, desc),
      );

    const cmpHeader = (): HTMLElement =>
      h('div', {
        style: {
          display: 'grid',
          gridTemplateColumns: '1.4fr 0.8fr 0.8fr 0.8fr 0.8fr 0.8fr 0.8fr',
          gap: '6px',
          padding: '6px 8px',
          borderBottom: '1px solid rgba(51,255,102,0.4)',
          color: '#ffb000',
          fontSize: '11px',
          fontWeight: 'bold',
          letterSpacing: '1px',
        },
      },
        h('div', null, 'КРИТЕРИЙ'),
        h('div', { style: { textAlign: 'center' } }, 'GIPNY'),
        h('div', { style: { textAlign: 'center' } }, 'SIGNAL'),
        h('div', { style: { textAlign: 'center' } }, 'TG'),
        h('div', { style: { textAlign: 'center' } }, 'ELEMENT'),
        h('div', { style: { textAlign: 'center' } }, 'JABBER'),
        h('div', { style: { textAlign: 'center' } }, 'TOX'),
      );

    const cmpRow = (crit: string, vals: string[]): HTMLElement => {
      const cell = (v: string, i: number): HTMLElement => {
        const isGipny = i === 0;
        let color = 'rgba(51,255,102,0.6)';
        if (v === '✓') color = '#33ff66';
        else if (v === '✗') color = '#ff4444';
        else if (v === '±') color = '#ffb000';
        return h('div', {
          style: {
            textAlign: 'center',
            color,
            fontWeight: isGipny ? 'bold' : 'normal',
          },
        }, v);
      };
      return h('div', {
        style: {
          display: 'grid',
          gridTemplateColumns: '1.4fr 0.8fr 0.8fr 0.8fr 0.8fr 0.8fr 0.8fr',
          gap: '6px',
          padding: '8px',
          borderBottom: '1px solid rgba(51,255,102,0.15)',
          fontSize: '12px',
          alignItems: 'center',
        },
      },
        h('div', { style: { color: '#33ff66' } }, crit),
        ...vals.map(cell),
      );
    };

    const section = (title: string): HTMLElement =>
      h('div', {
        style: {
          marginTop: '20px',
          marginBottom: '10px',
          color: '#ffb000',
          letterSpacing: '2px',
          fontSize: '12px',
          fontWeight: 'bold',
        },
      }, `── ${title} ──`);

    const howRow = (step: string, body: string): HTMLElement =>
      h('div', { style: { marginBottom: '8px', fontSize: '12px', lineHeight: '1.7' } },
        h('span', { style: { color: '#ffb000', fontWeight: 'bold' } }, step + ' '),
        h('span', { style: { color: 'rgba(51,255,102,0.85)' } }, body),
      );

    this.el = h('div', { class: 'modal', style: { maxWidth: '760px' } },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, '── БЕЗОПАСНОСТЬ И АНОНИМНОСТЬ ──'),
        h('button', { class: 'icon-btn', onClick: close }, 'x'),
      ),
      h('div', { class: 'modal-body' },

        h('div', {
          style: {
            padding: '10px 14px',
            border: '1px solid rgba(255,176,0,0.5)',
            background: 'rgba(255,176,0,0.05)',
            marginBottom: '14px',
            fontSize: '12px',
            lineHeight: '1.6',
          },
        },
          h('div', { style: { color: '#ffb000', fontWeight: 'bold', marginBottom: '6px' } }, 'МОДЕЛЬ УГРОЗ'),
          'gipny защищает от пассивного и активного слежения, деанонимизации по сетевым метаданным, привязки аккаунта к личности и компрометации содержимого при захвате устройства или сервера. ',
          'Идентификатор юзера — только два публичных ключа и onion-адрес. Ни телефона, ни email, ни IP, ни username — регистрация невозможна в принципе. ',
          'Сервер-реле видит ТОЛЬКО {recipient_pk, encrypted_blob, timestamp} — кто кому пишет, что пишет, и социальный граф ему недоступны.',
        ),

        section('КАК ОТПРАВЛЯЕТСЯ СООБЩЕНИЕ'),

        h('div', {
          style: {
            padding: '10px 14px',
            border: '1px solid rgba(51,255,102,0.3)',
            marginBottom: '14px',
            fontSize: '12px',
            lineHeight: '1.7',
          },
        },
          h('div', { style: { color: '#33ff66', marginBottom: '6px' } }, 'ТРАНСПОРТНАЯ ЦЕПОЧКА (снаружи → внутрь)'),
          h('div', { style: { fontFamily: 'monospace', color: '#5cff5c', marginBottom: '8px' } },
            '[ твоё устройство ]',
            h('br', null), '   ↓ (опционально) SOCKS5 внешний прокси/VPN',
            h('br', null), '   ↓ Tor (3 хопа через сеть relay)',
            h('br', null), '   ↓ HSDir lookup → onion circuit',
            h('br', null), '[ gipny-relay v3 onion ] — НЕ видит контент, НЕ видит отправителя',
            h('br', null), '   ↓ (через тот же relay, обратно через Tor)',
            h('br', null), '[ onion получателя ]',
            h('br', null), '   ↓ Double Ratchet decrypt',
            h('br', null), '[ его устройство ]',
          ),
          h('div', { class: 'hint', style: { fontSize: '11px' } },
            'Без SOCKS5 прокси провайдер видит, что ты в Tor (но не куда). С прокси — видит только что ты подключился к одному из дата-центров; до Tor-сети не доходит. ',
            'Шифрование E2E делается клиентом до отправки, ключи никогда не уходят с устройства.',
          ),
        ),

        section('КАК ПЕРЕДАЁТСЯ ИМЯ'),

        h('div', {
          style: {
            padding: '10px 14px',
            border: '1px solid rgba(51,255,102,0.3)',
            marginBottom: '14px',
            fontSize: '12px',
            lineHeight: '1.7',
            color: 'rgba(51,255,102,0.85)',
          },
        },
          'Каждый юзер сам задаёт своё отображаемое имя в Настройках → Identity → display name. ',
          'Это имя вкладывается ВНУТРЬ зашифрованного payload как поле sender_name на каждое сообщение. ',
          'Когда контакт получает первое сообщение, он автоматически обновляет имя у себя в локальной базе — никаких ручных rename не требуется. ',
          h('br', null), h('br', null),
          h('span', { style: { color: '#ffb000', fontWeight: 'bold' } }, 'Если не задал имя — собеседник увидит короткий hex (первые 16 символов твоего sign_pk).'),
          ' Это безопасно, но не информативно — поставь себе нормальное имя.',
        ),

        section('ФИЧИ В ТЕКУЩЕЙ ВЕРСИИ'),

        featureRow('Transport: Arti (Tor в Rust)',
          'Свой Tor-клиент в процессе — ни system tor, ни tor.exe. Onion service v3 (256-bit) поднимается локально для входящих. Provider не видит куда ты ходишь.'),
        featureRow('Опциональный SOCKS5 outer proxy',
          'Tor можно завернуть в SOCKS5 (любой VPS/коммерческий прокси). Цепочка становится "ISP → SOCKS5 → Tor" — провайдер видит только подключение к одному дата-центру, не Tor.'),
        featureRow('Sealed sender (Tier 1)',
          'В пакете нет sender_pk — relay не знает кто кому пишет. Получатель trial-decrypt-ом находит подходящую сессию. Аналог Signal Sealed Sender.'),
        featureRow('Padding до фиксированных бакетов',
          'Каждое сообщение паддится до ближайшего из {256B, 1KB, 4KB, 16KB, 64KB, 256KB, 1MB, 4MB, 16MB}. По размеру невозможно отличить текст от вложения или одиночное сообщение от пачки.'),
        featureRow('X3DH + Double Ratchet',
          'Тот же протокол, что в Signal. Forward secrecy + post-compromise security: даже при утечке текущего ключа прошлые и будущие сообщения защищены.'),
        featureRow('XChaCha20-Poly1305 AEAD',
          'Современный authenticated encryption. 192-битный nonce исключает коллизии; MAC защищает от подмены.'),
        featureRow('Ed25519 + X25519',
          'Подписи и key exchange на эллиптических кривых. Стандарт индустрии.'),
        featureRow('SQLCipher (AES-256 + Argon2id)',
          'База профиля целиком зашифрована локально. Argon2id для деривации ключа из пароля защищает от GPU/ASIC брутфорса. temp_store=MEMORY — SQLite не сливает временные сорты на диск plain.'),
        featureRow('Zeroize ключей в RAM',
          'Чувствительные данные стираются из памяти сразу после использования. Drop-impl на всех ключах, plus zeroize hex-строки SQLCipher-ключа во время PRAGMA.'),
        featureRow('Duress-пароль (скрытый)',
          'Второй пароль: под принуждением вводишь его — либо показывается пустой decoy-профиль, либо ВСЁ стирается. Никаких следов, что был duress.'),
        featureRow('Лимит попыток + авто-wipe',
          'Настраиваемое количество неудачных попыток; при превышении — безопасное стирание данных.'),
        featureRow('TTL-самоуничтожение сообщений',
          'Отправитель задаёт срок жизни (5 мин → 7 дней). По истечении — автоматическое удаление у обеих сторон. Pinned сообщения исключены из purge.'),
        featureRow('Auto-resync при поломке сессии',
          'Если ratchet рассинхронизировался (например, после восстановления из бэкапа на одной стороне), клиент автоматически пересогласовывает X3DH. Throttle 60s/контакт чтобы не спамить.'),
        featureRow('Watchdog reconnect (75s)',
          'Если коннект к relay умер, через 75 секунд автоматический реконнект. Параметр выверен для cellular Tor — на проводе быстрее, но на 4G RTT иногда >30s.'),
        featureRow('FTS5 поиск в зашифрованной БД',
          'Полнотекстовый индекс по сообщениям внутри SQLCipher — поиск работает быстро и приватно, без открытых индексов.'),
        featureRow('Verified-контакты по fingerprint',
          'Сверка отпечатков out-of-band (по голосу, SMS, лично) защищает от MITM. Отмечай контакт как verified только после такой сверки.'),
        featureRow('Подписанные обновления (Ed25519)',
          'Каждый релиз подписан offline-ключом. Клиент отвергает любой бинарь без валидной подписи. Сервер обновлений тоже onion — нет clearnet-trail при скачивании патчей.'),
        featureRow('Process hardening',
          'PR_SET_DUMPABLE=0, RLIMIT_CORE=0 на Linux/Windows — никаких core-dump с ключами. SQLCipher cipher_memory_security=ON — mlock на страницах БД.'),
        featureRow('Memory-safe Rust',
          'Без GC, без буферных переполнений, без use-after-free. Малая поверхность атаки.'),

        section('ЭКСПОРТ И ИМПОРТ — ПРАВИЛЬНО'),

        h('div', {
          style: {
            padding: '10px 14px',
            border: '1px solid rgba(51,255,102,0.3)',
            marginBottom: '14px',
          },
        },
          howRow('1.', 'В Settings → Backup задай passphrase ≥8 символов и нажми [ EXPORT BACKUP ]. Файл получит расширение .bin и будет зашифрован XChaCha20-Poly1305. Сам файл-блоб бесполезен без passphrase.'),
          howRow('2.', 'Перенеси файл на новое устройство любым каналом (свой шифрованный носитель, личная флешка, gipny→gipny через вложение и т.п.).'),
          howRow('3.', 'На новом устройстве: profile-select → [ IMPORT BACKUP ]. Создай НОВЫЙ профиль (имя, vault passphrase), укажи путь к .bin, введи backup passphrase. Импорт восстановит ВСЁ: переписку, файлы, контакты, группы.'),
          howRow('⚠', h('span', { style: { color: '#ff8888', fontWeight: 'bold' } },
            'ОДНА IDENTITY = ОДНО АКТИВНОЕ УСТРОЙСТВО. ') as unknown as string,
          ),
          h('div', { style: { fontSize: '12px', lineHeight: '1.7', color: 'rgba(255,170,170,0.9)', marginLeft: '20px' } },
            'После импорта на новом устройстве — закрой gipny на старом. Если будут работать оба, ratchet-сессии начнут расходиться (двойной decrypt одного header невозможен), и сообщения уйдут в auto-resync. ',
            'Принципиально это не сломает, но будет глючить и сжирать прекеи. ',
            'Если хочешь работать с двух мест — заведи второй профиль с отдельной identity.',
          ),
          h('br', null),
          howRow('★', 'Бэкап шифруется по тому же стандарту что и vault — Argon2id KDF + XChaCha20-Poly1305 AEAD. Утрата passphrase = полная потеря, recovery невозможно by design.'),
        ),

        section('СРАВНЕНИЕ С АЛЬТЕРНАТИВАМИ'),

        h('div', {
          style: {
            border: '1px solid rgba(51,255,102,0.3)',
            padding: '10px',
            marginBottom: '12px',
          },
        },
          cmpHeader(),
          cmpRow('E2E по умолчанию',         ['✓', '✓', '✗', '±', '±', '✓']),
          cmpRow('Forward secrecy',          ['✓', '✓', '±', '✓', '±', '✓']),
          cmpRow('Post-compromise sec.',     ['✓', '✓', '✗', '✓', '✗', '✓']),
          cmpRow('Без номера телефона',      ['✓', '✗', '✗', '✓', '✓', '✓']),
          cmpRow('Без email/регистрации',    ['✓', '✗', '✗', '±', '±', '✓']),
          cmpRow('Трафик через Tor',         ['✓', '✗', '✗', '✗', '±', '✗']),
          cmpRow('Скрывает IP от сервера',   ['✓', '✗', '✗', '✗', '✗', '—']),
          cmpRow('Sealed sender',            ['✓', '✓', '✗', '✗', '✗', '—']),
          cmpRow('Size-padding metadata',    ['✓', '±', '✗', '✗', '✗', '✗']),
          cmpRow('Offline-доставка',         ['✓', '✓', '✓', '✓', '±', '✗']),
          cmpRow('Duress / panic pass',      ['✓', '✗', '✗', '✗', '✗', '✗']),
          cmpRow('TTL / самоуничтож.',       ['✓', '✓', '±', '✓', '✗', '✗']),
          cmpRow('Open source клиент',       ['✓', '✓', '±', '✓', '✓', '✓']),
          cmpRow('Bot SDK',                  ['✓', '✗', '✓', '✗', '✓', '✗']),

          h('div', {
            style: {
              marginTop: '12px',
              padding: '8px',
              fontSize: '11px',
              color: 'rgba(51,255,102,0.7)',
              borderTop: '1px solid rgba(51,255,102,0.2)',
            },
          },
            '✓ = полностью / по умолчанию    ±  = частично / только в некоторых режимах    ✗ = нет / отключено    — = не применимо',
          ),
        ),

        section('НЮАНСЫ ПО КОНКУРЕНТАМ'),

        h('div', {
          style: {
            fontSize: '12px',
            lineHeight: '1.8',
            color: 'rgba(51,255,102,0.85)',
          },
        },
          h('div', { style: { marginBottom: '8px' } },
            h('span', { style: { color: '#33ff66', fontWeight: 'bold' } }, '→ Signal: '),
            'криптография на том же уровне (X3DH + Double Ratchet), но привязка к номеру телефона = деанонимизация. Сервера в Amazon AWS видят IP и часть метаданных. Sealed sender есть, но не закрывает IP.',
          ),
          h('div', { style: { marginBottom: '8px' } },
            h('span', { style: { color: '#33ff66', fontWeight: 'bold' } }, '→ Telegram: '),
            'E2E только в Secret Chats (1-на-1). Обычные чаты и группы читаются серверами TG. Привязка к номеру. Свой протокол MTProto, аудиты были, но не Signal-уровня.',
          ),
          h('div', { style: { marginBottom: '8px' } },
            h('span', { style: { color: '#33ff66', fontWeight: 'bold' } }, '→ Element (Matrix): '),
            'E2E через Olm/Megolm есть и неплох, но federated серверы видят metadata (кто с кем, когда, частоту). Часть room state не шифруется. Multi-device через cross-signing.',
          ),
          h('div', { style: { marginBottom: '8px' } },
            h('span', { style: { color: '#33ff66', fontWeight: 'bold' } }, '→ Jabber (XMPP+OMEMO): '),
            'E2E через OMEMO неплох, но зависит от клиента. Сервер XMPP видит весь граф и метаданные. Fingerprint-сверка вручную, UX не для массового юзера.',
          ),
          h('div', { style: { marginBottom: '8px' } },
            h('span', { style: { color: '#33ff66', fontWeight: 'bold' } }, '→ Tox: '),
            'P2P без серверов — сильная анонимность, но: нет offline-доставки (оба должны быть онлайн), маленькая user base, нестабильно. IP видим peer-ам напрямую.',
          ),
        ),

        section('СЛАБЫЕ МЕСТА (ЧЕСТНО)'),

        h('div', {
          style: {
            padding: '10px 14px',
            border: '1px solid rgba(255,68,68,0.4)',
            background: 'rgba(255,68,68,0.05)',
            fontSize: '12px',
            lineHeight: '1.7',
            color: 'rgba(255,170,170,0.9)',
          },
        },
          h('div', { style: { marginBottom: '6px' } }, '◇ Маленькая user base — меньше peer review, меньше battle-testing на разных сетях.'),
          h('div', { style: { marginBottom: '6px' } }, '◇ Один canonical relay — single point of failure для доставки (не для шифрования). Multi-relay в roadmap.'),
          h('div', { style: { marginBottom: '6px' } }, '◇ Не проходил независимый криптоаудит.'),
          h('div', { style: { marginBottom: '6px' } }, '◇ Молодой код — баги возможны. Обновляй регулярно через встроенный updater.'),
          h('div', { style: { marginBottom: '6px' } }, '◇ Tor имеет известные timing-correlation атаки при глобальном противнике (NSA-уровень). Использование SOCKS5 outer proxy сужает поверхность атаки.'),
          h('div', null, '◇ Multi-device пока не поддерживается — одна identity = одно устройство одновременно.'),
        ),

        section('КРИПТОСТЕК'),

        h('div', { class: 'card-block', style: { fontSize: '11px', lineHeight: '1.9' } },
          h('div', null, 'identity:           Ed25519 (подпись) + X25519 (DH)'),
          h('div', null, 'key agreement:      X3DH (Extended Triple Diffie-Hellman)'),
          h('div', null, 'session:            Double Ratchet (Signal protocol)'),
          h('div', null, 'AEAD:               XChaCha20-Poly1305 (192-bit nonce)'),
          h('div', null, 'KDF (session):      HKDF-SHA256'),
          h('div', null, 'KDF (vault pass):   Argon2id (память-стойкий, anti-GPU)'),
          h('div', null, 'vault at rest:     SQLCipher 4 (AES-256-CBC + HMAC-SHA256)'),
          h('div', null, 'search:             FTS5 внутри зашифрованной БД'),
          h('div', null, 'transport:          Tor v3 onion services (Arti, в процессе)'),
          h('div', null, 'optional outer:    SOCKS5 (внешний прокси/VPN перед Tor)'),
          h('div', null, 'sealed envelope:    sender_pk скрыт от relay'),
          h('div', null, 'padding:            бакеты 256B…16MB'),
          h('div', null, 'release signing:    Ed25519 (offline cold-storage key)'),
          h('div', null, 'language:           Rust (memory-safe, no GC)'),
        ),
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn btn-ghost', onClick: close }, '[ close ]'),
      ),
    );
  }
}
