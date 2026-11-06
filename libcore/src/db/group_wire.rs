//! A group reference received inside an authenticated direct message. The
//! session proves who sent it, not that they may change the group: that is
//! decided here, against what is already stored, before anything is written.
//! The rules match the browser client's `ingestGroup`.
use super::*;
use crate::session::WireGroupRef;

pub const MAX_GROUP_MEMBERS: usize = 64;
const MAX_GROUP_NAME_CHARS: usize = 128;
const MAX_MEMBER_NAME_CHARS: usize = 96;
const MAX_MEMBER_ADDRESS_BYTES: usize = 1024;

impl Db {
    /// Validate `gref` from `sender_sign` and merge it in one transaction:
    /// create the group if it is new (the sender and we must both be listed),
    /// otherwise accept it only from a current member, and only add members.
    /// Identity keys already known (ours, a contact's, a member's) are pinned:
    /// a reference that pairs a known signing key with another DH key is
    /// refused whole. Members we have no contact for become contacts, as
    /// before, so pairwise sessions can be opened to them.
    pub fn apply_group_ref(
        &self, gref: &WireGroupRef, sender_sign: &[u8], my_sign: &[u8], my_dh: &[u8],
    ) -> Result<()> {
        if gref.id.len() != 32
            || gref.name.chars().count() > MAX_GROUP_NAME_CHARS
            || gref.members.len() < 2
            || gref.members.len() > MAX_GROUP_MEMBERS
        {
            return Err(DbError::State);
        }
        let mut seen = std::collections::HashSet::new();
        for m in &gref.members {
            if m.sign_pk.len() != 32 || m.dh_pk.len() != 32
                || m.name.chars().count() > MAX_MEMBER_NAME_CHARS
                || m.onion.len() > MAX_MEMBER_ADDRESS_BYTES
                || !seen.insert(m.sign_pk.as_slice())
            {
                return Err(DbError::State);
            }
        }
        if !seen.contains(my_sign) || !seen.contains(sender_sign) {
            return Err(DbError::State);
        }
        self.with_tx(|tx| {
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM groups WHERE id=?1)",
                params![gref.id], |r| r.get(0))?;
            if exists {
                let sender_is_member: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM group_members WHERE group_id=?1 AND sign_pk=?2)",
                    params![gref.id, sender_sign], |r| r.get(0))?;
                if !sender_is_member { return Err(DbError::State); }
            }
            // Pin every identity we already know before writing anything.
            for m in &gref.members {
                if m.sign_pk.as_slice() == my_sign {
                    if m.dh_pk.as_slice() != my_dh { return Err(DbError::State); }
                    continue;
                }
                let known: Vec<Vec<u8>> = tx.prepare_cached(
                    "SELECT identity_dh FROM contacts WHERE identity_sign=?1
                     UNION ALL SELECT dh_pk FROM group_members WHERE group_id=?2 AND sign_pk=?1")?
                    .query_map(params![m.sign_pk, gref.id], |r| r.get(0))?.collect_rows()?;
                if known.iter().any(|dh| dh.as_slice() != m.dh_pk.as_slice()) { return Err(DbError::State); }
                // A DH key is one contact's: another signing key cannot claim it.
                let dh_owner: Option<Vec<u8>> = tx.query_row(
                    "SELECT identity_sign FROM contacts WHERE identity_dh=?1",
                    params![m.dh_pk], |r| r.get(0)).optional()?;
                if dh_owner.is_some_and(|sign| sign != m.sign_pk) { return Err(DbError::State); }
            }
            if !exists {
                tx.execute("INSERT INTO groups (id, name, created_at) VALUES (?1, ?2, ?3)",
                    params![gref.id, gref.name, now_ms()])?;
            }
            let mut count: i64 = tx.query_row("SELECT COUNT(*) FROM group_members WHERE group_id=?1",
                params![gref.id], |r| r.get(0))?;
            for m in &gref.members {
                let present: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM group_members WHERE group_id=?1 AND sign_pk=?2)",
                    params![gref.id, m.sign_pk], |r| r.get(0))?;
                if present { continue; }
                if count as usize >= MAX_GROUP_MEMBERS { return Err(DbError::TooLarge); }
                count += 1;
                let is_self = m.sign_pk.as_slice() == my_sign;
                let name = if m.name.is_empty() { short_hex(&m.sign_pk) } else { m.name.clone() };
                tx.execute("INSERT INTO group_members (group_id, sign_pk, dh_pk, onion, display_name, is_self)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![gref.id, m.sign_pk, m.dh_pk, m.onion, name, is_self as i64])?;
                if !is_self {
                    tx.execute("INSERT OR IGNORE INTO contacts (identity_sign, identity_dh, onion_address, display_name, trust, created_at)
                        VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                        params![m.sign_pk, m.dh_pk, m.onion, name, now_ms()])?;
                }
            }
            Ok(())
        })
    }
}

fn short_hex(b: &[u8]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::WireMember;

    fn member(sign: u8, dh: u8) -> WireMember {
        WireMember { sign_pk: vec![sign; 32], dh_pk: vec![dh; 32], onion: String::new(), name: format!("m{sign}") }
    }

    fn group(members: Vec<WireMember>) -> WireGroupRef {
        WireGroupRef { id: vec![9; 32], name: "team".into(), members }
    }

    fn database() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("t.db")).unwrap();
        (dir, db)
    }

    fn members(db: &Db) -> Vec<Vec<u8>> {
        let mut v: Vec<_> = db.list_group_members(&[9; 32]).unwrap().into_iter().map(|m| m.sign_pk).collect();
        v.sort();
        v
    }

    // Me = 1, Alice = 2 (the creator), Eve = 5.
    const ME: [u8; 32] = [1; 32];
    const ME_DH: [u8; 32] = [11; 32];

    #[test]
    fn a_new_group_needs_both_the_sender_and_us() {
        let (_dir, db) = database();
        assert!(db.apply_group_ref(&group(vec![member(2, 12), member(3, 13)]), &[2; 32], &ME, &ME_DH).is_err());
        assert!(db.apply_group_ref(&group(vec![member(1, 11), member(3, 13)]), &[2; 32], &ME, &ME_DH).is_err());
        db.apply_group_ref(&group(vec![member(1, 11), member(2, 12)]), &[2; 32], &ME, &ME_DH).unwrap();
        assert_eq!(members(&db), vec![vec![1; 32], vec![2; 32]]);
    }

    #[test]
    fn a_non_member_cannot_change_an_existing_group() {
        let (_dir, db) = database();
        db.apply_group_ref(&group(vec![member(1, 11), member(2, 12)]), &[2; 32], &ME, &ME_DH).unwrap();
        // Eve knows the id and lists herself: refused, nothing written.
        let err = db.apply_group_ref(&group(vec![member(1, 11), member(2, 12), member(5, 15)]), &[5; 32], &ME, &ME_DH);
        assert!(err.is_err());
        assert_eq!(members(&db), vec![vec![1; 32], vec![2; 32]]);
        assert!(db.find_contact_by_sign(&[5; 32]).unwrap().is_none());
        // A member may introduce someone.
        db.apply_group_ref(&group(vec![member(1, 11), member(2, 12), member(3, 13)]), &[2; 32], &ME, &ME_DH).unwrap();
        assert_eq!(members(&db).len(), 3);
    }

    #[test]
    fn known_identity_keys_are_pinned() {
        let (_dir, db) = database();
        db.add_contact(&[2; 32], &[12; 32], "", "alice", None).unwrap();
        db.add_contact(&[3; 32], &[13; 32], "", "bob", None).unwrap();
        // Bob's signing key with another DH key, our own key swapped, or
        // someone else claiming Bob's DH key: each refused.
        for bad in [member(3, 99), member(4, 13)] {
            assert!(db.apply_group_ref(&group(vec![member(1, 11), member(2, 12), bad]), &[2; 32], &ME, &ME_DH).is_err());
        }
        assert!(db.apply_group_ref(&group(vec![member(1, 77), member(2, 12)]), &[2; 32], &ME, &ME_DH).is_err());
        assert!(db.list_group_members(&[9; 32]).unwrap().is_empty());
        assert!(db.get_group_name(&[9; 32]).unwrap().is_none());
    }

    #[test]
    fn malformed_keys_and_oversized_groups_are_refused_without_panic() {
        let (_dir, db) = database();
        for len in [0usize, 1, 31, 33] {
            let mut bad = member(3, 13);
            bad.dh_pk = vec![13; len];
            assert!(db.apply_group_ref(&group(vec![member(1, 11), member(2, 12), bad]), &[2; 32], &ME, &ME_DH).is_err());
        }
        let many: Vec<_> = (0..=MAX_GROUP_MEMBERS as u8).map(|i| member(i.wrapping_add(1), i.wrapping_add(100))).collect();
        assert!(db.apply_group_ref(&group(many), &[2; 32], &ME, &ME_DH).is_err());
        let mut dup = group(vec![member(1, 11), member(2, 12), member(2, 12)]);
        assert!(db.apply_group_ref(&dup, &[2; 32], &ME, &ME_DH).is_err());
        dup.id = vec![9; 31];
        assert!(db.apply_group_ref(&dup, &[2; 32], &ME, &ME_DH).is_err());
    }
}
