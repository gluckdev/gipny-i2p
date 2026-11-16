//! Database schema and ordered migrations.

use super::*;

const MIGRATE_TABLES: &[&str] = &["messages", "contacts", "groups", "attachments"];

const SCHEMA_V1: &str = r#"
CREATE TABLE contacts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    identity_sign BLOB NOT NULL,
    identity_dh BLOB UNIQUE NOT NULL,
    i2p_address TEXT NOT NULL,
    display_name TEXT NOT NULL,
    trust INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    last_seen INTEGER
);
CREATE TABLE sessions (
    contact_id INTEGER PRIMARY KEY,
    state BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (contact_id) REFERENCES contacts(id) ON DELETE CASCADE
);
CREATE TABLE prekeys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind INTEGER NOT NULL,
    private BLOB NOT NULL,
    public BLOB NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_prekeys_kind ON prekeys(kind);
CREATE TABLE groups (
    id BLOB PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE group_members (
    group_id BLOB NOT NULL,
    sign_pk BLOB NOT NULL,
    dh_pk BLOB NOT NULL,
    i2p_address TEXT NOT NULL,
    display_name TEXT NOT NULL,
    is_self INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (group_id, sign_pk),
    FOREIGN KEY (group_id) REFERENCES groups(id) ON DELETE CASCADE
);
CREATE INDEX idx_group_members_group ON group_members(group_id);
CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    contact_id INTEGER,
    group_id BLOB,
    sender_sign_pk BLOB,
    direction INTEGER NOT NULL,
    body TEXT NOT NULL,
    sent_at INTEGER NOT NULL,
    delivered INTEGER NOT NULL DEFAULT 0,
    read INTEGER NOT NULL DEFAULT 0,
    expires_at INTEGER,
    FOREIGN KEY (contact_id) REFERENCES contacts(id) ON DELETE CASCADE,
    FOREIGN KEY (group_id) REFERENCES groups(id) ON DELETE CASCADE
);
CREATE INDEX idx_messages_contact ON messages(contact_id, sent_at) WHERE contact_id IS NOT NULL;
CREATE INDEX idx_messages_group ON messages(group_id, sent_at) WHERE group_id IS NOT NULL;
CREATE INDEX idx_messages_expires ON messages(expires_at) WHERE expires_at IS NOT NULL;
CREATE TABLE attachments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    size INTEGER NOT NULL,
    key BLOB NOT NULL,
    path TEXT NOT NULL,
    FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
);
CREATE INDEX idx_attachments_message ON attachments(message_id);
CREATE TABLE settings (
    k TEXT PRIMARY KEY,
    v BLOB NOT NULL
);
"#;

impl Db {
    pub(super) fn migrate(conn: &Connection) -> Result<()> {
        let has_messages: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'messages'",
            [],
            |r| r.get(0),
        )?;
        if has_messages == 0 {
            conn.execute_batch(SCHEMA_V1)?;
        }

        Self::ensure_column(conn, "messages", "origin_msg_id", "INTEGER")?;
        if Self::ensure_column(conn, "messages", "sent", "INTEGER NOT NULL DEFAULT 0")? {
            conn.execute_batch("UPDATE messages SET sent = delivered WHERE direction = 1;")?;
        }
        Self::ensure_column(conn, "messages", "last_attempt_at", "INTEGER")?;
        Self::ensure_column(
            conn,
            "messages",
            "send_attempts",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        Self::ensure_column(conn, "messages", "reply_to", "INTEGER")?;

        let has_fts: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'messages_fts'",
            [],
            |r| r.get(0),
        )?;
        if has_fts == 0 {
            conn.execute_batch("
                CREATE VIRTUAL TABLE messages_fts USING fts5(body, content='messages', content_rowid='id', tokenize='unicode61 remove_diacritics 2');
                INSERT INTO messages_fts(messages_fts) VALUES('rebuild');
            ")?;
        }

        conn.execute_batch("
            DELETE FROM messages WHERE id IN (
                SELECT id FROM (
                    SELECT id, ROW_NUMBER() OVER (PARTITION BY contact_id, direction, origin_msg_id ORDER BY id) AS rn
                    FROM messages
                    WHERE contact_id IS NOT NULL AND origin_msg_id IS NOT NULL
                ) WHERE rn > 1
            );
            DELETE FROM messages WHERE id IN (
                SELECT id FROM (
                    SELECT id, ROW_NUMBER() OVER (PARTITION BY group_id, sender_sign_pk, origin_msg_id ORDER BY id) AS rn
                    FROM messages
                    WHERE group_id IS NOT NULL AND sender_sign_pk IS NOT NULL AND origin_msg_id IS NOT NULL
                ) WHERE rn > 1
            );

            DROP INDEX IF EXISTS idx_messages_origin;
            DROP INDEX IF EXISTS idx_messages_group_origin;

            CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_origin
                ON messages(contact_id, direction, origin_msg_id)
                WHERE contact_id IS NOT NULL AND origin_msg_id IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_group_origin
                ON messages(group_id, sender_sign_pk, origin_msg_id)
                WHERE group_id IS NOT NULL AND sender_sign_pk IS NOT NULL AND origin_msg_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_messages_outbox
                ON messages(contact_id, sent, delivered)
                WHERE direction = 1;
            CREATE INDEX IF NOT EXISTS idx_messages_reply_to
                ON messages(reply_to)
                WHERE reply_to IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_messages_unread_contact
                ON messages(contact_id)
                WHERE contact_id IS NOT NULL AND direction = 0 AND read = 0;
            CREATE INDEX IF NOT EXISTS idx_messages_unread_group
                ON messages(group_id)
                WHERE group_id IS NOT NULL AND direction = 0 AND read = 0;
            CREATE INDEX IF NOT EXISTS idx_group_members_sign
                ON group_members(sign_pk);

            CREATE TABLE IF NOT EXISTS pinned_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                contact_id INTEGER,
                group_id BLOB,
                message_id INTEGER NOT NULL,
                pinned_at INTEGER NOT NULL,
                FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE,
                FOREIGN KEY (contact_id) REFERENCES contacts(id) ON DELETE CASCADE,
                FOREIGN KEY (group_id) REFERENCES groups(id) ON DELETE CASCADE
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_pinned_contact_msg
                ON pinned_messages(contact_id, message_id) WHERE contact_id IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS idx_pinned_group_msg
                ON pinned_messages(group_id, message_id) WHERE group_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_pinned_contact
                ON pinned_messages(contact_id) WHERE contact_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_pinned_group
                ON pinned_messages(group_id) WHERE group_id IS NOT NULL;

            CREATE TABLE IF NOT EXISTS deferred_pins (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                contact_id INTEGER,
                group_id BLOB,
                sender_sign_pk BLOB NOT NULL,
                origin_msg_id INTEGER NOT NULL,
                unpin INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (contact_id) REFERENCES contacts(id) ON DELETE CASCADE,
                FOREIGN KEY (group_id) REFERENCES groups(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_deferred_pins_contact
                ON deferred_pins(contact_id, sender_sign_pk, origin_msg_id)
                WHERE contact_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_deferred_pins_group
                ON deferred_pins(group_id, sender_sign_pk, origin_msg_id)
                WHERE group_id IS NOT NULL;

            CREATE TABLE IF NOT EXISTS resync_log (
                contact_id INTEGER PRIMARY KEY,
                last_resync_at INTEGER NOT NULL,
                FOREIGN KEY (contact_id) REFERENCES contacts(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS callback_seen (
                sender_sign_pk BLOB NOT NULL,
                sent_at INTEGER NOT NULL,
                callback_data TEXT NOT NULL,
                seen_at INTEGER NOT NULL,
                PRIMARY KEY (sender_sign_pk, sent_at, callback_data)
            );
            CREATE INDEX IF NOT EXISTS idx_callback_seen_age ON callback_seen(seen_at);

            CREATE TABLE IF NOT EXISTS dead_letters (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                contact_id INTEGER,
                group_id BLOB,
                payload BLOB NOT NULL,
                kind TEXT NOT NULL,
                error TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                first_seen_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_dead_letters_age ON dead_letters(last_seen_at);

            CREATE TABLE IF NOT EXISTS notify_seen (
                idempotency_key TEXT PRIMARY KEY,
                message_id INTEGER NOT NULL,
                seen_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_notify_seen_age ON notify_seen(seen_at);

            CREATE TABLE IF NOT EXISTS pending_outbound (
                msg_id INTEGER NOT NULL,
                recipient_contact_id INTEGER NOT NULL,
                last_attempt_at INTEGER,
                send_attempts INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                PRIMARY KEY (msg_id, recipient_contact_id)
            );
            CREATE INDEX IF NOT EXISTS idx_pending_outbound_recipient ON pending_outbound(recipient_contact_id);
            CREATE INDEX IF NOT EXISTS idx_pending_outbound_age ON pending_outbound(created_at);

            -- Relay network (gipny-dht): items held for others, opaque
            -- ciphertext under opaque keys, and nodes that answered us.
            CREATE TABLE IF NOT EXISTS dht_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                k BLOB NOT NULL,
                v BLOB NOT NULL,
                v_hash BLOB NOT NULL,
                delete_hash BLOB,
                expires_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_dht_items_k ON dht_items(k);
            CREATE INDEX IF NOT EXISTS idx_dht_items_exp ON dht_items(expires_at);

            -- Letters taken from the relay network and already handled: a
            -- ratchet envelope decrypted twice looks like a broken session and
            -- would trigger a resync, and deletion from the network is not
            -- guaranteed to reach every node.
            CREATE TABLE IF NOT EXISTS dht_seen (
                envelope_hash BLOB PRIMARY KEY,
                seen_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_dht_seen_age ON dht_seen(seen_at);

            CREATE TABLE IF NOT EXISTS dht_peers (
                destination TEXT PRIMARY KEY,
                stores INTEGER NOT NULL,
                first_seen INTEGER NOT NULL,
                last_ok INTEGER NOT NULL
            );

            CREATE TRIGGER IF NOT EXISTS tr_pending_outbound_msg_del
                AFTER DELETE ON messages
                BEGIN
                    DELETE FROM pending_outbound WHERE msg_id = OLD.id;
                END;

            CREATE TRIGGER IF NOT EXISTS tr_pending_outbound_contact_del
                AFTER DELETE ON contacts
                BEGIN
                    DELETE FROM pending_outbound WHERE recipient_contact_id = OLD.id;
                END;

            CREATE TRIGGER IF NOT EXISTS tr_buttons_msg_del
                AFTER DELETE ON messages
                BEGIN
                    DELETE FROM settings WHERE k = 'buttons_' || OLD.id;
                END;

            CREATE TRIGGER IF NOT EXISTS tr_console_msg_del
                AFTER DELETE ON messages
                BEGIN
                    DELETE FROM settings WHERE k = 'console_' || OLD.id OR k = 'console_pending_' || OLD.id;
                END;

            CREATE TRIGGER IF NOT EXISTS tr_messages_fts_ai AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, body) VALUES (NEW.id, NEW.body);
            END;
            CREATE TRIGGER IF NOT EXISTS tr_messages_fts_ad AFTER DELETE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, body) VALUES('delete', OLD.id, OLD.body);
            END;
            CREATE TRIGGER IF NOT EXISTS tr_messages_fts_au AFTER UPDATE OF body ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, body) VALUES('delete', OLD.id, OLD.body);
                INSERT INTO messages_fts(rowid, body) VALUES (NEW.id, NEW.body);
            END;

            PRAGMA user_version = 9;
        ")?;
        // Files sent in parts (crate::files).
        Self::ensure_column(conn, "attachments", "chunk_size", "INTEGER")?;
        conn.execute_batch("
            -- A file we send in parts: its attachment (sealed in parts on disk)
            -- and the offer the recipients check it against.
            CREATE TABLE IF NOT EXISTS file_out (
                file_id BLOB PRIMARY KEY,
                attachment_id INTEGER NOT NULL,
                message_id INTEGER NOT NULL,
                sha256 BLOB NOT NULL,
                cancelled INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            );
            -- How far each recipient has it (crate::files::Sending).
            CREATE TABLE IF NOT EXISTS file_out_peer (
                file_id BLOB NOT NULL,
                contact_id INTEGER NOT NULL,
                next INTEGER NOT NULL DEFAULT 0,
                acked INTEGER NOT NULL DEFAULT 0,
                resend BLOB,
                last_sent_at INTEGER,
                last_ack_at INTEGER,
                failed INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (file_id, contact_id)
            );
            -- A file coming in parts, until it is whole and becomes an
            -- attachment of its message.
            CREATE TABLE IF NOT EXISTS file_in (
                file_id BLOB NOT NULL,
                contact_id INTEGER NOT NULL,
                message_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                size INTEGER NOT NULL,
                sha256 BLOB NOT NULL,
                chunk_size INTEGER NOT NULL,
                path TEXT NOT NULL,
                key BLOB NOT NULL,
                bits BLOB NOT NULL,
                received INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                PRIMARY KEY (file_id, contact_id)
            );
            -- Parts that came before their offer.
            CREATE TABLE IF NOT EXISTS file_early (
                file_id BLOB NOT NULL,
                contact_id INTEGER NOT NULL,
                idx INTEGER NOT NULL,
                data BLOB NOT NULL,
                at INTEGER NOT NULL,
                PRIMARY KEY (file_id, contact_id, idx)
            );
            CREATE TRIGGER IF NOT EXISTS tr_file_out_msg_del
                AFTER DELETE ON messages
                BEGIN
                    DELETE FROM file_out_peer WHERE file_id IN (SELECT file_id FROM file_out WHERE message_id = OLD.id);
                    DELETE FROM file_out WHERE message_id = OLD.id;
                    DELETE FROM file_in WHERE message_id = OLD.id;
                END;
            CREATE TRIGGER IF NOT EXISTS tr_file_contact_del
                AFTER DELETE ON contacts
                BEGIN
                    DELETE FROM file_out_peer WHERE contact_id = OLD.id;
                    DELETE FROM file_in WHERE contact_id = OLD.id;
                    DELETE FROM file_early WHERE contact_id = OLD.id;
                END;
        ")?;
        Self::ensure_column(conn, "contacts", "is_bot", "INTEGER NOT NULL DEFAULT 0")?;
        let added_contact_lma =
            Self::ensure_column(conn, "contacts", "last_message_at", "INTEGER")?;
        Self::ensure_column(conn, "contacts", "pinned_at", "INTEGER")?;
        // Per-contact relay, carried in their contact card. Before this every
        // client had exactly one relay for everyone, which meant somebody had to
        // run infrastructure for the whole network.
        Self::ensure_column(conn, "contacts", "relay_address", "TEXT")?;
        Self::ensure_column(
            conn,
            "contacts",
            "agent_granted",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        Self::ensure_column(
            conn,
            "contacts",
            "request_state",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        let added_group_lma = Self::ensure_column(conn, "groups", "last_message_at", "INTEGER")?;
        Self::ensure_column(conn, "groups", "pinned_at", "INTEGER")?;
        if added_contact_lma {
            conn.execute_batch(
                "
                UPDATE contacts SET last_message_at = (
                    SELECT MAX(sent_at) FROM messages m WHERE m.contact_id = contacts.id
                ) WHERE last_message_at IS NULL;
            ",
            )?;
        }
        if added_group_lma {
            conn.execute_batch(
                "
                UPDATE groups SET last_message_at = (
                    SELECT MAX(sent_at) FROM messages m WHERE m.group_id = groups.id
                ) WHERE last_message_at IS NULL;
            ",
            )?;
        }
        conn.execute_batch(
            "
            CREATE TRIGGER IF NOT EXISTS tr_messages_bump_contact_last
                AFTER INSERT ON messages WHEN NEW.contact_id IS NOT NULL
                BEGIN
                    UPDATE contacts SET last_message_at = NEW.sent_at
                    WHERE id = NEW.contact_id
                      AND (last_message_at IS NULL OR last_message_at < NEW.sent_at);
                END;
            CREATE TRIGGER IF NOT EXISTS tr_messages_bump_group_last
                AFTER INSERT ON messages WHEN NEW.group_id IS NOT NULL
                BEGIN
                    UPDATE groups SET last_message_at = NEW.sent_at
                    WHERE id = NEW.group_id
                      AND (last_message_at IS NULL OR last_message_at < NEW.sent_at);
                END;
            CREATE INDEX IF NOT EXISTS idx_contacts_sort
                ON contacts(pinned_at, last_message_at);
            CREATE INDEX IF NOT EXISTS idx_groups_sort
                ON groups(pinned_at, last_message_at);
        ",
        )?;
        Self::migrate_dm(conn)?;
        Self::migrate_inbox(conn)?;
        Ok(())
    }

    pub(super) fn ensure_column(
        conn: &Connection,
        table: &str,
        column: &str,
        decl: &str,
    ) -> Result<bool> {
        assert!(
            MIGRATE_TABLES.contains(&table),
            "ensure_column: table {table} not in whitelist"
        );
        assert!(
            column
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "ensure_column: column name {column:?} has unsafe chars"
        );
        assert!(
            decl.chars()
                .all(|c| c.is_ascii_alphanumeric() || c.is_whitespace() || c == '_'),
            "ensure_column: decl {decl:?} has unsafe chars"
        );
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            params![table, column],
            |r| r.get(0),
        )?;
        if n == 0 {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
