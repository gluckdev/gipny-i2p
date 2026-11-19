use super::*;
use gipny_libcore::db::Contact;
use gipny_libcore::relay_server::{ViewOnceAccept, ViewOnceHandler};
use gipny_libcore::view_once::{self, Offer, Request};
use zeroize::Zeroizing;

#[derive(Clone, Debug, Serialize)]
pub struct ViewOnceNotice {
    pub id: String,
    pub contact_id: i64,
    pub sent_at: i64,
    pub expires_at: i64,
}

fn trusted_peer(contact: &Contact) -> Result<IdentityCard> {
    if contact.request_state != RequestState::None || contact.trust == TrustLevel::Blocked {
        return Err(CoreError::State);
    }
    Ok(IdentityCard {
        sign_pk: contact
            .identity_sign
            .as_slice()
            .try_into()
            .map_err(|_| CoreError::State)?,
        dh_pk: contact
            .identity_dh
            .as_slice()
            .try_into()
            .map_err(|_| CoreError::State)?,
    })
}

pub(super) struct OutgoingLetter {
    contact_id: i64,
    peer: IdentityCard,
    request: Request,
    deadline: i64,
    body: view_once::QueuedText,
}
struct Attempt {
    letter: Arc<OutgoingLetter>,
    next_attempt: i64,
    in_flight: bool,
}
pub(super) struct Outbox(std::sync::Mutex<Option<HashMap<[u8; 16], Attempt>>>);
impl Default for Outbox {
    fn default() -> Self {
        Self(std::sync::Mutex::new(Some(HashMap::new())))
    }
}
impl Outbox {
    pub(super) fn close(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
    fn forget(&self, id: &[u8; 16]) {
        if let Some(queue) = self.0.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            queue.remove(id);
        }
    }
}
impl Core {
    pub(super) fn view_once_handler(self: &Arc<Self>) -> ViewOnceHandler {
        let weak = Arc::downgrade(self);
        Arc::new(move |from, request| {
            let core = weak.upgrade()?;
            let contact = core.db.find_contact_by_sign_pk(&from).ok()??;
            let peer = trusted_peer(&contact).ok()?;
            {
                let mut live = core.view_once.lock().ok()?;
                let inbox = live.as_mut()?;
                inbox.retain(|_, (meta, _)| meta.expires_at > now_ms());
                if inbox.len() >= view_once::MAX_PENDING
                    && !core
                        .db
                        .reack_view_once(&request.id, contact.id, request.expires_at, now_ms())
                        .ok()?
                {
                    return None;
                }
            }
            let (offer, public) =
                Offer::issue(&core.identity, peer.clone(), request, now_ms()).ok()?;
            let weak = weak.clone();
            let accept: ViewOnceAccept = Box::new(move |envelope| {
                let Some(core) = weak.upgrade() else {
                    return false;
                };
                let now = now_ms();
                let Ok(letter) = offer.accept(&envelope, now) else {
                    return false;
                };
                core.keep_view_once(contact.id, request, letter)
            });
            Some((public, accept))
        })
    }

    fn keep_view_once(&self, contact_id: i64, request: Request, letter: view_once::Letter) -> bool {
        let now = now_ms();
        let Ok(Some(contact)) = self.db.get_contact(contact_id) else {
            return false;
        };
        if request.expires_at <= now
            || trusted_peer(&contact).ok().as_ref() != Some(letter.sender())
        {
            return false;
        }
        let Ok(mut live) = self.view_once.lock() else {
            return false;
        };
        let Some(inbox) = live.as_mut() else {
            return false;
        };
        if self
            .db
            .reack_view_once(&request.id, contact_id, request.expires_at, now)
            .unwrap_or(false)
        {
            return true;
        }
        inbox.retain(|_, (meta, _)| meta.expires_at > now);
        if inbox.len() >= view_once::MAX_PENDING
            || !self
                .db
                .claim_received_view_once(&request.id, contact_id, request.expires_at, now)
                .unwrap_or(false)
        {
            return false;
        }
        let meta = ViewOnceNotice {
            id: to_hex(&request.id),
            contact_id,
            sent_at: now_ms(),
            expires_at: request.expires_at,
        };
        inbox.insert(meta.id.clone(), (meta.clone(), letter));
        let _ = self.events.try_send(CoreEvent::ViewOnceAvailable(meta));
        true
    }

    pub(super) fn spawn_view_once_expiry(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            let mut last_cleanup = 0i64;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(core) = weak.upgrade() else {
                    return;
                };
                let mut live = core.view_once.lock().unwrap_or_else(|e| e.into_inner());
                let Some(inbox) = live.as_mut() else {
                    return;
                };
                let now = now_ms();
                inbox.retain(|_, (meta, _)| meta.expires_at > now);
                drop(live);
                if now < last_cleanup || now.saturating_sub(last_cleanup) >= 5_000 {
                    last_cleanup = now;
                    if let Ok(notices) = core.db.expire_view_once_deliveries(now) {
                        for notice in notices {
                            let _ = core
                                .events
                                .try_send(CoreEvent::ViewOnceDeliveryChanged(notice));
                        }
                    }
                }
            }
        });
        self.tasks.lock().unwrap().push(task);
        let weak = Arc::downgrade(self);
        self.track(tokio::spawn(async move {
            loop {
                let Some(core) = weak.upgrade() else {
                    return;
                };
                let now = now_ms();
                if let Ok(due) = core.db.due_view_once_receipts(now) {
                    for (contact, id) in due {
                        let _ = tokio::time::timeout(
                            Duration::from_secs(60),
                            core.send_view_once_control(contact, &id, false),
                        )
                        .await;
                    }
                }
                drop(core);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }));
    }

    pub async fn send_view_once(
        &self,
        contact_id: i64,
        body: String,
        ttl: Option<Duration>,
    ) -> Result<String> {
        let body = Zeroizing::new(body);
        if body.is_empty() || body.len() > view_once::MAX_TEXT {
            return Err(CoreError::State);
        }
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(CoreError::NotFound)?;
        let peer = trusted_peer(&contact)?;
        let now = now_ms();
        let end = gipny_libcore::db::MessageLifetime::from(ttl)
            .expiry(now)?
            .ok_or(CoreError::State)?;
        let request = Request::new(peer.sign_pk, end);
        let deadline = end.min(now.saturating_add(7 * 86_400_000));
        let mut live = self
            .view_once_outbox
            .0
            .lock()
            .map_err(|_| CoreError::State)?;
        let queue = live.as_mut().ok_or(CoreError::State)?;
        if end <= now {
            self.db
                .begin_view_once_delivery(&request.id, contact_id, deadline)?;
            for status in self.db.expire_view_once_deliveries(now)? {
                let _ = self
                    .events
                    .try_send(CoreEvent::ViewOnceDeliveryChanged(status));
            }
            return Ok(to_hex(&request.id));
        }
        if queue.len() >= view_once::MAX_PENDING {
            return Err(CoreError::State);
        }
        let body = view_once::QueuedText::wrap(&request.id, body.as_bytes())?;
        let letter = Arc::new(OutgoingLetter {
            contact_id,
            peer,
            request,
            deadline,
            body,
        });
        self.db
            .begin_view_once_delivery(&request.id, contact_id, deadline)?;
        queue.insert(
            request.id,
            Attempt {
                letter,
                next_attempt: now,
                in_flight: false,
            },
        );
        self.view_once_kick.notify_one();
        Ok(to_hex(&request.id))
    }

    pub(super) fn spawn_view_once_outbox(self: &Arc<Self>) {
        let core = self.clone();
        self.track(tokio::spawn(async move{
            loop {
                {
                    let now=now_ms();
                    let mut live=core.view_once_outbox.0.lock().unwrap_or_else(|e|e.into_inner());
                    let Some(queue)=live.as_mut() else{return;};
                    queue.retain(|_,a|a.letter.deadline>now);
                    let available=4usize.saturating_sub(queue.values().filter(|a|a.in_flight).count());
                    let mut due:Vec<_>=queue.values_mut().filter(|a|!a.in_flight && a.next_attempt<=now).collect();
                    due.sort_by_key(|a|a.next_attempt);
                    for attempt in due.into_iter().take(available) {
                        attempt.in_flight=true;
                        let letter=attempt.letter.clone();
                        let worker=core.clone();
                        core.track(tokio::spawn(async move{
                            let id=letter.request.id;
                            let time=Duration::from_millis(letter.deadline.saturating_sub(now_ms()).max(0) as u64);
                            let mut locked=worker.view_once_locked.subscribe();
                            let stored=tokio::select! {
                                biased;
                                _=locked.wait_for(|closed|*closed)=>false,
                                result=tokio::time::timeout(time.min(Duration::from_secs(90)),worker.try_view_once(&letter))=>result.is_ok_and(|r|r),
                            };
                            let now=now_ms();
                            if stored {
                                if let Ok(Some(status))=worker.db.view_once_relayed(&id){
                                    let _=worker.events.try_send(CoreEvent::ViewOnceDeliveryChanged(status));
                                }
                                worker.view_once_outbox.forget(&id);
                            } else if let Some(attempt)=worker.view_once_outbox.0.lock().unwrap_or_else(|e|e.into_inner()).as_mut().and_then(|q|q.get_mut(&id)){
                                attempt.in_flight=false;
                                attempt.next_attempt=now.saturating_add(30_000);
                            }
                            worker.view_once_kick.notify_one();
                        }));
                    }
                }
                tokio::select!{_=tokio::time::sleep(Duration::from_secs(2))=>{},_=core.view_once_kick.notified()=>{}}
            }
        }));
    }

    async fn try_view_once(self: &Arc<Self>, letter: &OutgoingLetter) -> bool {
        let Ok(Some(contact)) = self.db.get_contact(letter.contact_id) else {
            return false;
        };
        if trusted_peer(&contact).ok().as_ref() != Some(&letter.peer) {
            return false;
        }
        let candidates = super::relay_loop::terminal_addresses(&contact);
        let per_address = if candidates.len() > 1 {
            Duration::from_secs(40)
        } else {
            Duration::from_secs(85)
        };
        for address in candidates {
            // Recheck trust, TTL and lock after the network round trips and
            // immediately before releasing the guarded local plaintext.
            let open = || {
                if *self.view_once_locked.borrow() || now_ms() >= letter.deadline {
                    return None;
                }
                let current = self.db.get_contact(letter.contact_id).ok()??;
                if trusted_peer(&current).ok().as_ref() != Some(&letter.peer) {
                    return None;
                }
                letter.body.open(&letter.request.id).ok()
            };
            if matches!(
                tokio::time::timeout(
                    per_address,
                    relay::send_view_once(
                        &self.node,
                        address,
                        &self.identity,
                        &letter.peer,
                        letter.request.fresh_attempt(),
                        open
                    )
                )
                .await,
                Ok(Ok(()))
            ) {
                return true;
            }
        }
        self.maybe_look_up_address(&contact).await;
        false
    }

    async fn send_view_once_control(
        self: &Arc<Self>,
        contact_id: i64,
        id: &[u8; 16],
        confirmed: bool,
    ) -> Result<()> {
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(CoreError::NotFound)?;
        trusted_peer(&contact)?;
        let mut p = view_once::receipt_payload(id, confirmed, now_ms());
        self.send_to_contact(contact_id, &mut p).await
    }

    pub(super) async fn handle_view_once_control(
        self: &Arc<Self>,
        contact_id: i64,
        p: &WirePayload,
    ) -> Result<bool> {
        let Some(receipt) = view_once::Receipt::from_payload(p) else {
            return Ok(false);
        };
        let (confirmed, id) = match receipt {
            view_once::Receipt::Confirmed(id) => (true, id),
            view_once::Receipt::Received(id) => (false, id),
            view_once::Receipt::Invalid => return Ok(true),
        };
        let Some(contact) = self.db.get_contact(contact_id)? else {
            return Ok(true);
        };
        if trusted_peer(&contact).is_err() {
            return Ok(true);
        }
        if confirmed {
            self.db.confirm_view_once_receipt(contact_id, &id)?;
        } else {
            if let Some(status) = self.db.view_once_received(contact_id, &id)? {
                self.view_once_outbox.forget(&id);
                let _ = self
                    .events
                    .try_send(CoreEvent::ViewOnceDeliveryChanged(status));
                let _ = tokio::time::timeout(
                    Duration::from_secs(15),
                    self.send_view_once_control(contact_id, &id, true),
                )
                .await;
            }
        }
        Ok(true)
    }
    pub fn list_view_once(&self, contact_id: i64) -> Result<Vec<ViewOnceNotice>> {
        trusted_peer(
            &self
                .db
                .get_contact(contact_id)?
                .ok_or(CoreError::NotFound)?,
        )?;
        let mut live = self.view_once.lock().map_err(|_| CoreError::State)?;
        let inbox = live.as_mut().ok_or(CoreError::State)?;
        let now = now_ms();
        inbox.retain(|_, (meta, _)| meta.expires_at > now);
        let mut result: Vec<_> = inbox
            .values()
            .filter(|(m, _)| m.contact_id == contact_id)
            .map(|(m, _)| m.clone())
            .collect();
        result.sort_by_key(|m| m.sent_at);
        Ok(result)
    }

    pub fn open_view_once(&self, id: &str) -> Result<String> {
        open_letter(&self.view_once, &self.identity, id, |contact_id| {
            let contact = self
                .db
                .get_contact(contact_id)?
                .ok_or(CoreError::NotFound)?;
            trusted_peer(&contact)
        })
    }
}

pub(super) type Inbox =
    std::sync::Mutex<Option<HashMap<String, (ViewOnceNotice, view_once::Letter)>>>;

fn open_letter(
    inbox: &Inbox,
    identity: &Identity,
    id: &str,
    contact: impl FnOnce(i64) -> Result<IdentityCard>,
) -> Result<String> {
    let mut live = inbox.lock().map_err(|_| CoreError::State)?;
    let entries = live.as_mut().ok_or(CoreError::State)?;
    let (meta, letter) = entries.get(id).ok_or(CoreError::ViewOnceUnavailable)?;
    if meta.expires_at <= now_ms() {
        entries.remove(id);
        return Err(CoreError::ViewOnceUnavailable);
    }
    if contact(meta.contact_id)? != *letter.sender() {
        return Err(CoreError::State);
    }
    let (_, letter) = entries.remove(id).ok_or(CoreError::ViewOnceUnavailable)?;
    let body = letter
        .open(identity, now_ms())
        .map_err(|_| CoreError::ViewOnceConsumed)?;
    String::from_utf8(body.to_vec()).map_err(|_| CoreError::ViewOnceConsumed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn view_once_open_preserves_preflight_failures_and_releases_once_to_concurrent_callers() {
        let sender = Identity::generate();
        let owner = Arc::new(Identity::generate());
        let now = now_ms();
        let req = Request::new(owner.card().sign_pk, now + 60_000);
        let (offer, cert) = Offer::issue(&owner, sender.card(), req, now).unwrap();
        let wire = view_once::seal(&sender, &owner.card(), &req, &cert, b"open once", now).unwrap();
        let letter = offer.accept(&wire, now).unwrap();
        let mut entries = HashMap::new();
        let id = to_hex(&req.id);
        entries.insert(
            id.clone(),
            (
                ViewOnceNotice {
                    id: id.clone(),
                    contact_id: 7,
                    sent_at: now,
                    expires_at: req.expires_at,
                },
                letter,
            ),
        );
        let inbox = Arc::new(std::sync::Mutex::new(Some(entries)));
        assert!(open_letter(&inbox, &owner, &id, |_| Err(CoreError::State)).is_err());
        assert!(open_letter(&inbox, &owner, &id, |_| Ok(Identity::generate().card())).is_err());
        let jobs: Vec<_> = (0..8)
            .map(|_| {
                let (inbox, owner, id, peer) =
                    (inbox.clone(), owner.clone(), id.clone(), sender.card());
                std::thread::spawn(move || {
                    open_letter(&inbox, &owner, &id, |contact_id| {
                        assert_eq!(contact_id, 7);
                        Ok(peer)
                    })
                    .ok()
                })
            })
            .collect();
        assert_eq!(
            jobs.into_iter()
                .filter_map(|j| j.join().unwrap())
                .collect::<Vec<_>>(),
            vec!["open once"]
        );
    }
}
