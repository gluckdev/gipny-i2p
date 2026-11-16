//! Link measurements and reachability.

use super::*;

/// How a letter to this contact leaves right now.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkRoute {
    /// Straight to the relay they collect from.
    Relay,
    /// Their relay is silent; the network holds the letter until they return.
    Archive,
    /// Nowhere to put it — their relay is unknown and we are not in the
    /// network either.
    None,
}

/// The channel to one contact, as the readout above the chat shows it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct LinkStats {
    /// Hops on our outbound tunnel — our leg, our exposure.
    pub our_hops: u8,
    /// Hops on the inbound tunnel of the relay they collect from — their leg.
    pub their_hops: u8,
    /// Whether messages are still padded to fixed size buckets.
    pub padded: bool,
    pub route: LinkRoute,
    /// Median of the last few measured round trips, absent until one message
    /// has been acknowledged.
    pub rtt_ms: Option<u32>,
}

/// The last handful of round trips to one contact.
///
/// The median, not the mean: one message that waited out a tunnel rebuild
/// would drag an average somewhere it has never been, and the number exists to
/// tell the user what the link feels like.
#[derive(Default, Clone)]
pub struct Rtt {
    samples: Vec<u32>,
}

impl Rtt {
    const KEEP: usize = 7;

    pub(super) fn push(&mut self, ms: u32) {
        self.samples.push(ms);
        if self.samples.len() > Self::KEEP {
            self.samples.remove(0);
        }
    }

    pub fn median_ms(&self) -> Option<u32> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        Some(sorted[sorted.len() / 2])
    }
}

impl Core {
    /// What the channel to this contact is doing right now, for the readout
    /// above the chat.
    ///
    /// There is no single «connection» to describe: a letter travels our
    /// outbound tunnel into their relay's inbound tunnel, and each side owns
    /// its own leg. So the readout names both, and says «архив» when their
    /// relay is silent and the network is holding the letter instead.
    pub async fn link_stats(self: &Arc<Self>, contact_id: i64) -> LinkStats {
        let route = match self.db.get_contact(contact_id).ok().flatten() {
            Some(contact) => match self.route_for(&contact).await {
                Some(Route::Relay(_)) => LinkRoute::Relay,
                Some(Route::Dht) => LinkRoute::Archive,
                None => LinkRoute::None,
            },
            None => LinkRoute::None,
        };
        let their_hops = self.node.hops();
        LinkStats {
            our_hops: their_hops,
            their_hops: self.hosted_hops().unwrap_or(their_hops),
            padded: true,
            route,
            rtt_ms: self
                .rtt
                .lock()
                .await
                .get(&contact_id)
                .and_then(Rtt::median_ms),
        }
    }

    pub(super) fn hosted_hops(&self) -> Option<u8> {
        self.hosted_relay
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|r| r.hops())
    }

    /// Reports a contact whose relay has been silent for a while with mail
    /// waiting, once, and reports it again when the relay answers.
    pub(super) async fn note_reachability(
        &self,
        contact: &gipny_libcore::db::Contact,
        reachable: bool,
        has_mail: bool,
    ) {
        let Some(relay) = contact
            .relay_address
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        else {
            return;
        };
        if reachable {
            self.relay_down_since.lock().await.remove(relay);
            if self.unreachable_reported.lock().await.remove(&contact.id) {
                let _ = self.events.try_send(CoreEvent::ContactReachability {
                    contact_id: contact.id,
                    unreachable: false,
                });
            }
            return;
        }
        let since = *self
            .relay_down_since
            .lock()
            .await
            .entry(relay.to_string())
            .or_insert_with(Instant::now);
        if has_mail
            && since.elapsed() >= CONTACT_UNREACHABLE_AFTER
            && self.unreachable_reported.lock().await.insert(contact.id)
        {
            gipny_libcore::debug_trace!(
                "[relay-client] contact {} has been unreachable for {:?}",
                contact.id,
                since.elapsed()
            );
            let _ = self.events.try_send(CoreEvent::ContactReachability {
                contact_id: contact.id,
                unreachable: true,
            });
        }
    }

    /// Contacts currently reported as unreachable, for a UI that starts late.
    pub async fn unreachable_contacts(&self) -> Vec<i64> {
        self.unreachable_reported
            .lock()
            .await
            .iter()
            .copied()
            .collect()
    }
}
