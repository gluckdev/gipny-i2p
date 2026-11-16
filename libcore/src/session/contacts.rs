//! Contact creation and profile hints.

use super::*;

impl SessionManager {
    pub async fn add_contact(
        self: &Arc<Self>,
        card: &crate::crypto::IdentityCard,
        onion: &str,
        name: &str,
    ) -> Result<i64> {
        self.add_contact_via(card, onion, name, None).await
    }

    /// Add a contact, recording the relay their card named.
    ///
    /// `relay` is where *they* receive: messages to this contact are deposited
    /// there, not on whatever relay this client happens to use. `None` keeps the
    /// old behaviour of falling back to this client's configured relay.
    pub async fn add_contact_via(
        self: &Arc<Self>,
        card: &crate::crypto::IdentityCard,
        onion: &str,
        name: &str,
        relay: Option<&str>,
    ) -> Result<i64> {
        let id = self
            .db
            .add_contact(&card.sign_pk, &card.dh_pk, onion, name, relay)?;
        let _ = self
            .events
            .send(SessionEvent::ContactAdded { contact_id: id })
            .await;
        self.dht_kick.notify_one();
        self.send_kick.notify_one();
        self.warm_relay_of(id);
        Ok(id)
    }

    pub async fn reset_contact_session(&self, contact_id: i64) -> Result<()> {
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(SessionError::NotFound)?;
        self.request_resync(&contact).await
    }

    pub(super) async fn apply_peer_name(&self, contact_id: i64, name: &str) {
        let contact_changed = self
            .db
            .update_contact_name(contact_id, name)
            .unwrap_or(false);
        let mut group_changed = false;
        if let Ok(Some(c)) = self.db.get_contact(contact_id) {
            if let Ok(groups) = self.db.list_groups_with_member(&c.identity_sign) {
                for gid in groups {
                    if self
                        .db
                        .update_group_member_name(&gid, &c.identity_sign, name)
                        .unwrap_or(false)
                    {
                        group_changed = true;
                    }
                }
            }
        }
        if contact_changed || group_changed {
            let _ = self
                .events
                .send(SessionEvent::ContactUpdated { contact_id })
                .await;
        }
    }
}
