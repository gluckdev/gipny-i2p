//! Live personal-terminal delivery. Each accepted offer owns a fresh X3DH
//! prekey. These types deliberately have no vault/backup representation.
use bincode::Options;
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::crypto::{self, Identity, IdentityCard, PreKeyBundle, PreKeyPair, X3dhInitial};

#[path = "view_once_wire.rs"]
mod wire;
pub use wire::*;

const HANDSHAKE_MS: i64 = 60_000;
const INVALID: &str = "one-time message unavailable";
/// Only the calling process can retry this message. The random key lives in
/// guarded memory and has no serialization or export path on this type.
pub struct QueuedText {
    cipher: crypto::AttachmentCipher,
    fragments: Vec<Vec<u8>>,
}
impl QueuedText {
    pub fn wrap(id: &[u8; 16], text: &[u8]) -> crypto::Result<Self> {
        if text.is_empty() || text.len() > MAX_TEXT {
            return Err(crypto::CryptoError::Crypto);
        }
        let cipher = crypto::AttachmentCipher::try_generate()?;
        let ciphertext = cipher.encrypt_chunk(0, id, text)?;
        let fragments = parts(&ciphertext).map(<[u8]>::to_vec).collect();
        Ok(Self { cipher, fragments })
    }

    pub fn open(&self, id: &[u8; 16]) -> crypto::Result<Zeroizing<Vec<u8>>> {
        self.cipher
            .decrypt_chunk(0, id, &self.fragments.concat())
            .map(Zeroizing::new)
    }
}
impl Request {
    pub fn fresh_attempt(self) -> Self {
        Self {
            nonce: crypto::random_array(),
            ..self
        }
    }
    pub fn new(to: [u8; 32], expires_at: i64) -> Self {
        Self {
            to,
            id: crypto::random_array(),
            nonce: crypto::random_array(),
            expires_at,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SignedOffer {
    bundle: PreKeyBundle,
    #[serde(with = "BigArray")]
    signature: [u8; 64],
}

#[derive(Serialize, Deserialize)]
struct SignedLetter {
    initial: X3dhInitial,
    #[serde(with = "BigArray")]
    signature: [u8; 64],
}

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit((MAX_TEXT + 2048) as u64)
        .reject_trailing_bytes()
}

fn context(
    req: &Request,
    sender: &IdentityCard,
    recipient: &IdentityCard,
    prekey: &[u8; 32],
) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"gipny/view-once/offer/v1");
    hash.update(sender.sign_pk);
    hash.update(sender.dh_pk);
    hash.update(recipient.sign_pk);
    hash.update(recipient.dh_pk);
    hash.update(req.id);
    hash.update(req.nonce);
    hash.update(req.expires_at.to_be_bytes());
    hash.update(prekey);
    hash.finalize().into()
}

fn signature_input(context: &[u8; 32], initial: &X3dhInitial) -> Result<[u8; 32], &'static str> {
    let mut hash = Sha256::new();
    hash.update(b"gipny/view-once/letter/v1");
    hash.update(context);
    hash.update(codec().serialize(initial).map_err(|_| INVALID)?);
    Ok(hash.finalize().into())
}

pub struct Offer {
    prekey: PreKeyPair,
    sender: IdentityCard,
    context: [u8; 32],
    handshake_until: i64,
    pub request: Request,
}

impl Offer {
    pub fn issue(
        identity: &Identity,
        sender: IdentityCard,
        request: Request,
        now: i64,
    ) -> Result<(Self, Vec<u8>), &'static str> {
        let recipient = identity.card();
        if request.to != recipient.sign_pk || !request.current(now) {
            return Err(INVALID);
        }
        let prekey = PreKeyPair::try_generate().map_err(|_| INVALID)?;
        let bundle = PreKeyBundle::new(identity, &prekey, None);
        let context = context(&request, &sender, &recipient, prekey.public());
        let wire = codec()
            .serialize(&SignedOffer {
                bundle,
                signature: identity.sign(&context),
            })
            .map_err(|_| INVALID)?;
        Ok((
            Self {
                prekey,
                sender,
                context,
                request,
                handshake_until: now.saturating_add(HANDSHAKE_MS),
            },
            wire,
        ))
    }

    /// Verify the sender's signature now, leaving the content encrypted until read.
    pub fn accept(self, wire: &[u8], now: i64) -> Result<Letter, &'static str> {
        if wire.len() > MAX_ENVELOPE
            || now >= self.handshake_until
            || now >= self.request.expires_at
        {
            return Err(INVALID);
        }
        let letter: SignedLetter = codec().deserialize(wire).map_err(|_| INVALID)?;
        if letter.initial.identity != self.sender
            || letter.initial.one_time_id.is_some()
            || !self.sender.verify(
                &signature_input(&self.context, &letter.initial)?,
                &letter.signature,
            )
        {
            return Err(INVALID);
        }
        Ok(Letter {
            offer: self,
            initial: letter.initial,
        })
    }
}

pub struct Letter {
    offer: Offer,
    initial: X3dhInitial,
}

impl Letter {
    pub fn sender(&self) -> &IdentityCard {
        &self.offer.sender
    }
    pub fn open(self, identity: &Identity, now: i64) -> Result<Zeroizing<Vec<u8>>, &'static str> {
        if now >= self.offer.request.expires_at || identity.card().sign_pk != self.offer.request.to
        {
            return Err(INVALID);
        }
        let (_, body) = crypto::x3dh_respond(
            identity,
            &self.offer.prekey,
            None,
            &self.initial,
            &self.offer.context,
        )
        .map_err(|_| INVALID)?;
        let body = Zeroizing::new(body);
        if body.is_empty() || body.len() > MAX_TEXT || std::str::from_utf8(&body).is_err() {
            return Err(INVALID);
        }
        Ok(body)
    }
}

pub fn seal(
    identity: &Identity,
    peer: &IdentityCard,
    request: &Request,
    offer: &[u8],
    body: &[u8],
    now: i64,
) -> Result<Vec<u8>, &'static str> {
    seal_with(identity, peer, request, offer, || Some(body), now)
}
pub(crate) fn seal_with<B: AsRef<[u8]>>(
    identity: &Identity,
    peer: &IdentityCard,
    request: &Request,
    offer: &[u8],
    expose: impl FnOnce() -> Option<B>,
    now: i64,
) -> Result<Vec<u8>, &'static str> {
    if request.to != peer.sign_pk || !request.current(now) || offer.len() > 1024 {
        return Err(INVALID);
    }
    let ready: SignedOffer = codec().deserialize(offer).map_err(|_| INVALID)?;
    let context = context(request, &identity.card(), peer, &ready.bundle.signed_prekey);
    if ready.bundle.identity != *peer
        || ready.bundle.one_time_id.is_some()
        || ready.bundle.one_time_prekey.is_some()
        || !peer.verify(&context, &ready.signature)
    {
        return Err(INVALID);
    }
    let plaintext = expose().ok_or(INVALID)?;
    let body = plaintext.as_ref();
    if body.is_empty() || body.len() > MAX_TEXT || std::str::from_utf8(body).is_err() {
        return Err(INVALID);
    }
    let (_, initial) =
        crypto::x3dh_initiate(identity, &ready.bundle, body, &context).map_err(|_| INVALID)?;
    let signature = identity.sign(&signature_input(&context, &initial)?);
    codec()
        .serialize(&SignedLetter { initial, signature })
        .map_err(|_| INVALID)
}

pub fn receipt_payload(id: &[u8; 16], confirmed: bool, now: i64) -> crate::session::WirePayload {
    let mut payload = crate::session::WirePayload::simple(0, String::new(), Vec::new(), now, None);
    let prefix = if confirmed {
        "gipny.viewonce.confirmed.v1/"
    } else {
        "gipny.viewonce.received.v1/"
    };
    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    payload.callback_data = Some(format!("{prefix}{hex}"));
    payload.ack_for = Some(0);
    payload
}

pub enum Receipt {
    Received([u8; 16]),
    Confirmed([u8; 16]),
    Invalid,
}
impl Receipt {
    /// Reserved metadata controls are consumed even when malformed. They must
    /// never reach chat callbacks or an agent's command dispatcher.
    pub fn from_payload(p: &crate::session::WirePayload) -> Option<Self> {
        let Some(tag) = p.callback_data.as_deref() else {
            return None;
        };
        let (confirmed, id) = if let Some(id) = tag.strip_prefix("gipny.viewonce.confirmed.v1/") {
            (true, id)
        } else if let Some(id) = tag.strip_prefix("gipny.viewonce.received.v1/") {
            (false, id)
        } else {
            return None;
        };
        if p.ack_for != Some(0)
            || p.origin_msg_id != 0
            || !p.body.is_empty()
            || !p.attachments.is_empty()
            || !p.files.is_empty()
            || p.buttons.is_some()
            || p.group.is_some()
            || p.file_chunk.is_some()
            || p.file_ack.is_some()
            || p.file_cancel.is_some()
            || p.dm.is_some()
            || p.reply_to.is_some()
            || p.typing.is_some()
            || p.pin.is_some()
            || p.notify_sound.is_some()
            || p.console.is_some()
            || p.wipe.is_some()
            || p.edit_of.is_some()
            || p.ttl_ms.is_some()
            || id.len() != 32
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Some(Self::Invalid);
        }
        let mut value = [0; 16];
        for (byte, pair) in value.iter_mut().zip(id.as_bytes().chunks_exact(2)) {
            *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
        Some(if confirmed {
            Self::Confirmed(value)
        } else {
            Self::Received(value)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_offer_encrypted_in_parts_and_opened_only_with_its_volatile_prekey() {
        let sender = Identity::generate();
        let recipient = Identity::generate();
        let req = Request::new(recipient.card().sign_pk, 5000);
        for size in [1, MAX_TEXT] {
            let body = vec![b'v'; size];
            let (pending, ready) = Offer::issue(&recipient, sender.card(), req, 1000).unwrap();
            let wire = seal(&sender, &recipient.card(), &req, &ready, &body, 2000).unwrap();
            let frames = parts(&wire);
            let total = frames.len() as u16;
            assert!((2..=5).contains(&total));
            let mut buffer = PartBuffer::default();
            let mut assembled = None;
            for (i, part) in frames.enumerate() {
                assembled = buffer.push(i as u16, total, part).unwrap();
                assert_eq!(assembled.is_some(), i + 1 == usize::from(total));
            }
            let letter = pending.accept(&assembled.unwrap(), 3000).unwrap();
            assert_eq!(letter.open(&recipient, 4000).unwrap().as_slice(), body);
            let (different_key, _) = Offer::issue(&recipient, sender.card(), req, 1000).unwrap();
            assert!(different_key.accept(&wire, 2000).is_err());
        }
    }

    #[test]
    fn view_once_receipts_require_an_empty_direct_control_and_exact_opaque_id() {
        let id = [0x1a; 16];
        let mut p = receipt_payload(&id, false, 100);
        assert!(matches!(Receipt::from_payload(&p),Some(Receipt::Received(r)) if r==id));
        p.body = "mixed".to_string();
        assert!(matches!(Receipt::from_payload(&p), Some(Receipt::Invalid)));
        p = receipt_payload(&id, true, 100);
        assert!(matches!(Receipt::from_payload(&p),Some(Receipt::Confirmed(r)) if r==id));
        p.callback_data = Some("gipny.viewonce.confirmed.v1/0123".into());
        assert!(matches!(Receipt::from_payload(&p), Some(Receipt::Invalid)));
    }

    #[test]
    fn recipient_contact_nonce_expiry_and_fragment_order_are_bound() {
        let sender = Identity::generate();
        let recipient = Identity::generate();
        let other = Identity::generate();
        let req = Request::new(recipient.card().sign_pk, 5000);
        let (pending, ready) = Offer::issue(&recipient, sender.card(), req, 1000).unwrap();
        let wrong_nonce = Request {
            nonce: [0; 32],
            ..req
        };
        assert!(seal(
            &sender,
            &recipient.card(),
            &wrong_nonce,
            &ready,
            b"private",
            2000
        )
        .is_err());
        assert!(seal(&other, &recipient.card(), &req, &ready, b"private", 2000).is_err());
        assert!(seal(&sender, &other.card(), &req, &ready, b"private", 2000).is_err());
        let wire = seal(&sender, &recipient.card(), &req, &ready, b"private", 2000).unwrap();
        assert!(pending
            .accept(&wire, 3000)
            .unwrap()
            .open(&recipient, 5000)
            .is_err());
        assert!(PartBuffer::default().push(1, 2, &[1]).is_err());
        let mut buf = PartBuffer::default();
        assert!(buf.push(0, 3, &[7; 4]).unwrap().is_none());
        assert!(buf.push(1, 3, &[7; 3]).is_err());
        assert!(PartBuffer::default().push(0, 6, &[7]).is_err());
    }
}

#[cfg(test)]
mod queue_tests {
    use super::*;
    #[test]
    fn queued_text_requires_its_own_key_and_message_id() {
        let id = [23; 16];
        let text = vec![17; MAX_TEXT];
        let first = QueuedText::wrap(&id, &text).unwrap();
        let mut second = QueuedText::wrap(&id, &text).unwrap();
        assert!(first.fragments.len() > 1 && first.fragments != second.fragments);
        assert_eq!(&**first.open(&id).unwrap(), &text);
        assert!(first.open(&[22; 16]).is_err());
        second.fragments = first.fragments;
        assert!(second.open(&id).is_err());
    }
}
