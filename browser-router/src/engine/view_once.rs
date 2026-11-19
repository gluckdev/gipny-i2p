use gipny_libcore::view_once;

enum ViewOnceSecret {
    Offer(view_once::Offer),
    Letter(view_once::Letter),
}

struct ViewOnceHandle {
    owner: Rc<EngineLifetime>,
    secret: Rc<RefCell<Option<ViewOnceSecret>>>,
}

impl ViewOnceHandle {
    fn new(engine: &BrowserEngine, secret: ViewOnceSecret) -> Self {
        let secret = Rc::new(RefCell::new(Some(secret)));
        let mut keys = engine.lifetime.view_once.borrow_mut();
        keys.retain(|weak| weak.strong_count() > 0);
        keys.push(Rc::downgrade(&secret));
        Self {
            owner: engine.lifetime.clone(),
            secret,
        }
    }
    fn take(self, engine: &BrowserEngine) -> Result<ViewOnceSecret, JsValue> {
        if !Rc::ptr_eq(&engine.lifetime, &self.owner) {
            return Err(JsValue::from_str("Wrong profile session"));
        }
        self.secret
            .take()
            .ok_or_else(|| JsValue::from_str("One-time message unavailable"))
    }
}

#[wasm_bindgen]
pub struct ViewOnceOffer {
    key: ViewOnceHandle,
    public: Vec<u8>,
}
#[wasm_bindgen]
impl ViewOnceOffer {
    pub fn bytes(&self) -> Vec<u8> {
        self.public.clone()
    }
}
#[wasm_bindgen]
pub struct ViewOnceLetter {
    key: ViewOnceHandle,
}

fn view_once_request(
    to: [u8; 32],
    id: &[u8],
    nonce: &[u8],
    expiry: f64,
) -> Result<view_once::Request, JsValue> {
    if !expiry.is_finite()
        || expiry.fract() != 0.0
        || !(0.0..=9_007_199_254_740_991.0).contains(&expiry)
    {
        return Err(JsValue::from_str("Invalid expiry"));
    }
    Ok(view_once::Request {
        to,
        id: id.try_into().map_err(|_| JsValue::from_str("Invalid id"))?,
        nonce: nonce
            .try_into()
            .map_err(|_| JsValue::from_str("Invalid challenge"))?,
        expires_at: expiry as i64,
    })
}

#[wasm_bindgen]
impl BrowserEngine {
    pub fn view_once_offer(
        &self,
        peer_sign: &str,
        peer_dh: &str,
        id: &[u8],
        nonce: &[u8],
        expires_at: f64,
    ) -> Result<ViewOnceOffer, JsValue> {
        let inner = self.inner.borrow();
        let identity = &inner.committed_state()?.identity;
        let req = view_once_request(identity.card().sign_pk, id, nonce, expires_at)?;
        let sender = crypto::IdentityCard {
            sign_pk: parse_hex32(peer_sign)?,
            dh_pk: parse_hex32(peer_dh)?,
        };
        let (offer, public) =
            view_once::Offer::issue(identity, sender, req, now_ms()).map_err(JsValue::from_str)?;
        Ok(ViewOnceOffer {
            key: ViewOnceHandle::new(self, ViewOnceSecret::Offer(offer)),
            public,
        })
    }

    pub fn view_once_seal(
        &self,
        peer_sign: &str,
        peer_dh: &str,
        id: &[u8],
        nonce: &[u8],
        expires_at: f64,
        offer: &[u8],
        body: &[u8],
    ) -> Result<Vec<u8>, JsValue> {
        let inner = self.inner.borrow();
        let identity = &inner.committed_state()?.identity;
        let peer = crypto::IdentityCard {
            sign_pk: parse_hex32(peer_sign)?,
            dh_pk: parse_hex32(peer_dh)?,
        };
        let req = view_once_request(peer.sign_pk, id, nonce, expires_at)?;
        view_once::seal(identity, &peer, &req, offer, body, now_ms()).map_err(JsValue::from_str)
    }

    pub fn view_once_accept(
        &self,
        offer: ViewOnceOffer,
        envelope: &[u8],
    ) -> Result<ViewOnceLetter, JsValue> {
        let ViewOnceSecret::Offer(key) = offer.key.take(self)? else {
            return Err(JsValue::from_str("Invalid offer"));
        };
        let letter = key.accept(envelope, now_ms()).map_err(JsValue::from_str)?;
        Ok(ViewOnceLetter {
            key: ViewOnceHandle::new(self, ViewOnceSecret::Letter(letter)),
        })
    }

    pub fn view_once_open(&self, letter: ViewOnceLetter) -> Result<Vec<u8>, JsValue> {
        let ViewOnceSecret::Letter(letter) = letter.key.take(self)? else {
            return Err(JsValue::from_str("Invalid letter"));
        };
        let inner = self.inner.borrow();
        let body = letter
            .open(&inner.committed_state()?.identity, now_ms())
            .map_err(JsValue::from_str)?;
        Ok(body.to_vec())
    }
}
