//! Contact requests, trust, removal, and peer names.

use super::*;

impl Core {
    /// Name/trust update that also drops agent mode when the master is blocked.
    pub async fn update_contact(&self, id: i64, name: &str, trust: TrustLevel) -> Result<()> {
        self.db.update_contact(id, name, trust)?;
        if trust == TrustLevel::Blocked && self.agent_master().is_some_and(|m| m.contact_id == id) {
            self.disable_agent_mode(false).await?;
        }
        Ok(())
    }

    /// Delete a contact and purge all associated in-memory state.
    ///
    /// The DB row deletion cascades to sessions/messages/attachments via
    /// `ON DELETE CASCADE`. This method additionally removes the five
    /// in-memory maps that would otherwise hold stale entries forever
    /// (sessions, own_inits and lost_inits, session_created_at, incoming_since_send,
    /// and the bundle_waiters entry keyed on the contact's signing key).
    pub async fn delete_contact(self: &Arc<Self>, contact_id: i64) -> Result<()> {
        // Capture identity_sign before the DB row is gone so we can purge
        // the bundle_waiters map (keyed by sign_pk, not by contact id).
        let sign_pk: Option<[u8; 32]> = self
            .db
            .get_contact(contact_id)?
            .and_then(|c| c.identity_sign.as_slice().try_into().ok());

        // A master that is gone cannot be told, and a mode with no master is
        // just a stale key.
        if self
            .agent_master()
            .is_some_and(|m| m.contact_id == contact_id)
        {
            let _ = self.disable_agent_mode(false).await;
        }

        self.db.delete_contact(contact_id)?;

        // Purge all in-memory state for this contact.
        self.sessions.lock().await.remove(&contact_id);
        self.session_created_at.lock().await.remove(&contact_id);
        self.own_inits.lock().await.remove(&contact_id);
        self.lost_inits.lock().await.remove(&contact_id);
        self.incoming_since_send.lock().await.remove(&contact_id);
        if let Some(pk) = sign_pk {
            self.bundle_waiters.lock().await.remove(&pk);
        }
        let _ = self.db.delete_setting(&wipe_key(contact_id));
        Ok(())
    }

    /// Delete a contact here **and** ask them to delete the chat and us.
    ///
    /// Our side goes at once: the messages now, the contact hidden. The
    /// contact row and its session stay only until the request is handed over
    /// — to their relay, or to the relay network while they are away — and
    /// then go too; after `WIPE_TTL_MS` they go regardless. Their client
    /// deletes the chat and the contact when the request arrives. That is a
    /// request, not a guarantee: a modified client can ignore it, and nothing
    /// takes back what they have already read, copied or screenshotted.
    pub async fn delete_contact_for_both(self: &Arc<Self>, contact_id: i64) -> Result<()> {
        let Some(contact) = self.db.get_contact(contact_id)? else {
            return Ok(());
        };
        // Someone we never accepted has nothing of ours to delete.
        if contact.request_state == RequestState::Incoming {
            return self.delete_contact(contact_id).await;
        }
        self.db.delete_messages_for_contact(contact_id)?;
        self.db
            .set_setting(&wipe_key(contact_id), now_ms().to_string().as_bytes())?;
        if self
            .agent_master()
            .is_some_and(|m| m.contact_id == contact_id)
        {
            let _ = self.disable_agent_mode(false).await;
        }
        let _ = self
            .events
            .try_send(CoreEvent::ContactUpdated { contact_id });
        self.send_kick.notify_one();
        Ok(())
    }

    /// When we asked this contact to delete everything, if we did.
    pub fn wipe_pending_since(&self, contact_id: i64) -> Option<i64> {
        self.db
            .get_setting(&wipe_key(contact_id))
            .ok()
            .flatten()
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|s| s.parse().ok())
    }

    /// Hand the delete request over; true once it has been, and the contact
    /// is gone here too.
    pub(super) async fn send_wipe(
        self: &Arc<Self>,
        contact: &gipny_libcore::db::Contact,
        since: i64,
    ) -> Result<bool> {
        if now_ms() - since > WIPE_TTL_MS {
            gipny_libcore::debug_trace!(
                "[wipe] contact {}: request not handed over in a week; deleting here",
                contact.id
            );
            self.delete_contact(contact.id).await?;
            return Ok(true);
        }
        let Some(route) = self.route_for(contact).await else {
            return Ok(false);
        };
        if self.ensure_session_for(contact, &route).await.is_err() {
            return Ok(false);
        }
        let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
        payload.wipe = Some(true);
        if let Err(e) = self
            .send_payload_via_relay(contact, &mut payload, &route)
            .await
        {
            gipny_libcore::debug_trace!("[wipe] contact {}: {e:?}; trying again", contact.id);
            return Ok(false);
        }
        gipny_libcore::debug_trace!(
            "[wipe] contact {}: delete request handed over; deleting here",
            contact.id
        );
        self.delete_contact(contact.id).await?;
        Ok(true)
    }

    pub(super) fn apply_peer_name(&self, contact_id: i64, name: &str) {
        let contact_changed = self
            .db
            .update_contact_name(contact_id, name)
            .unwrap_or(false);
        if contact_changed {
            let _ = self
                .events
                .try_send(CoreEvent::ContactUpdated { contact_id });
        }
        if let Ok(Some(c)) = self.db.get_contact(contact_id) {
            if let Ok(groups) = self.db.list_groups_with_member(&c.identity_sign) {
                for gid in groups {
                    if self
                        .db
                        .update_group_member_name(&gid, &c.identity_sign, name)
                        .unwrap_or(false)
                    {
                        let _ = self.events.try_send(CoreEvent::GroupUpdated {
                            group_id: to_hex(&gid),
                        });
                    }
                }
            }
        }
    }

    /// Accept an introduction: show what they sent, acknowledge it, and tell
    /// them where we collect.
    pub async fn accept_contact_request(self: &Arc<Self>, contact_id: i64) -> Result<()> {
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(CoreError::NotFound)?;
        if contact.request_state != RequestState::Incoming {
            return Ok(());
        }
        self.accept_request_now(contact_id).await?;
        let _ = self
            .events
            .try_send(CoreEvent::ContactUpdated { contact_id });
        Ok(())
    }

    pub(super) async fn accept_request_now(self: &Arc<Self>, contact_id: i64) -> Result<()> {
        self.db
            .set_contact_request_state(contact_id, RequestState::None)?;
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(CoreError::NotFound)?;
        // An ack that finds no relay yet is not lost: the sender retries, and
        // the duplicate is acknowledged on arrival.
        for origin in self.db.incoming_origins(contact_id)? {
            if let Some(mid) = self.db.find_message_by_origin(contact_id, origin)? {
                self.activate_message_files(contact_id, mid).await?;
            }
            let original =
                WirePayload::simple(origin as u64, String::new(), Vec::new(), now_ms(), None);
            let _ = self.send_ack(&contact, &original).await;
        }
        // Also what tells them we accepted when there was nothing to ack.
        self.announce_pending.lock().await.insert(contact_id);
        self.send_kick.notify_one();
        Ok(())
    }

    /// Decline an introduction: forget them and whatever they sent. They can
    /// ask again; blocking is what keeps them out.
    pub async fn decline_contact_request(self: &Arc<Self>, contact_id: i64) -> Result<()> {
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(CoreError::NotFound)?;
        if contact.request_state != RequestState::Incoming {
            return Err(CoreError::State);
        }
        self.delete_contact(contact_id).await
    }

    /// Add a contact, recording the relay their card named.
    ///
    /// `relay` is where *they* receive: messages to this contact are deposited
    /// there, not on whatever relay this client happens to use. `None` keeps the
    /// old behaviour of falling back to this client's configured relay.
    pub async fn add_contact_via(
        self: &Arc<Self>,
        card: &gipny_libcore::crypto::IdentityCard,
        i2p_address: &str,
        name: &str,
        relay: Option<&str>,
    ) -> Result<i64> {
        let known = self.db.find_contact_by_sign_pk(&card.sign_pk)?;
        let id = self
            .db
            .add_contact(&card.sign_pk, &card.dh_pk, i2p_address, name, relay)?;
        match known.map(|c| c.request_state) {
            // A fresh card: introduce ourselves right away, so they see us
            // without waiting for a first message.
            None => {
                self.db
                    .set_contact_request_state(id, RequestState::Outgoing)?;
            }
            // Adding the card of someone who asked is accepting them.
            Some(RequestState::Incoming) => {
                self.accept_request_now(id).await?;
            }
            Some(_) => {}
        }
        let _ = self
            .events
            .try_send(CoreEvent::ContactAdded { contact_id: id });
        self.send_kick.notify_one();
        self.warm_relay_of(id);
        Ok(id)
    }

    pub async fn reset_contact_session(&self, contact_id: i64) -> Result<()> {
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(CoreError::NotFound)?;
        self.request_resync(&contact).await
    }
}
