//! BrowserEngine WASM actor owning all private cryptographic state and identities.
//!
//! JS Worker owns public metadata JSON (contacts/messages/outbox), but persists
//! it only encrypted together with private state.
//! In-process emissary ClientIdentity private bytes and libcore ratchet/identity
//! keys never cross into JavaScript.

#[cfg(target_arch = "wasm32")]
mod implementation {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
    use std::rc::Rc;

    use bincode::Options;
    use emissary_core::ClientIdentity;
    use gipny_libcore::{
        card::ContactCard,
        crypto::{self, Identity, PreKeyBundle, PreKeyPair, RatchetState, X3dhInitial},
        relay::{auth_v2_message, destination_hash, EnvelopeBlob},
        session::{
            build_ad, decode_payload, encode_payload, pad_payload, unpad_payload, WirePayload,
        },
    };
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use wasm_bindgen::prelude::*;
    use zeroize::{Zeroize, Zeroizing};

    use crate::vault::{newest, Passkey, Record, Vault};
    use crate::BrowserRuntime;

    const TARGET_OPK: usize = 20;
    const MAX_OPK: usize = 256;
    const MAX_SESSIONS: usize = 1024;
    const MAX_ALTERNATES: usize = 3;
    const FRESH_SESSION_GRACE_MS: i64 = 60_000;
    const MAX_ENVELOPE_BYTES: u64 = 16 * 1024 * 1024;

    fn bincode_options() -> impl bincode::Options {
        bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .reject_trailing_bytes()
            .with_limit(MAX_ENVELOPE_BYTES)
    }

    fn bincode_bundle_options() -> impl bincode::Options {
        bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .reject_trailing_bytes()
            .with_limit(64 * 1024)
    }

    fn now_ms() -> i64 {
        js_sys::Date::now() as i64
    }

    fn to_hex(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            use std::fmt::Write;
            let _ = write!(s, "{:02x}", b);
        }
        s
    }

    fn hex_nibble(b: u8) -> Result<u8, JsValue> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            b'A'..=b'F' => Ok(b - b'A' + 10),
            _ => Err(JsValue::from_str("invalid hex character")),
        }
    }

    fn parse_hex32(hex_str: &str) -> Result<[u8; 32], JsValue> {
        let s = hex_str.trim();
        if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(JsValue::from_str(
                "expected 64 ASCII hex characters (32 bytes)",
            ));
        }
        let bytes = s.as_bytes();
        let mut out = [0u8; 32];
        for i in 0..32 {
            let hi = hex_nibble(bytes[i * 2])?;
            let lo = hex_nibble(bytes[i * 2 + 1])?;
            out[i] = (hi << 4) | lo;
        }
        Ok(out)
    }

    fn parse_hex32_opt(hex_str: &str) -> Option<[u8; 32]> {
        let s = hex_str.trim();
        if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let bytes = s.as_bytes();
        let mut out = [0u8; 32];
        for i in 0..32 {
            let hi = match bytes[i * 2] {
                b'0'..=b'9' => bytes[i * 2] - b'0',
                b'a'..=b'f' => bytes[i * 2] - b'a' + 10,
                b'A'..=b'F' => bytes[i * 2] - b'A' + 10,
                _ => return None,
            };
            let lo = match bytes[i * 2 + 1] {
                b'0'..=b'9' => bytes[i * 2 + 1] - b'0',
                b'a'..=b'f' => bytes[i * 2 + 1] - b'a' + 10,
                b'A'..=b'F' => bytes[i * 2 + 1] - b'A' + 10,
                _ => return None,
            };
            out[i] = (hi << 4) | lo;
        }
        Some(out)
    }

    fn ours_stands(my_sign: &[u8], their_sign: &[u8]) -> bool {
        my_sign < their_sign
    }

    fn decode_with_padding_fallback(pt: &[u8]) -> Result<WirePayload, JsValue> {
        if let Some(unpadded) = unpad_payload(pt) {
            if let Ok(p) = decode_payload(&unpadded) {
                return Ok(p);
            }
        }
        decode_payload(pt)
            .map_err(|e| JsValue::from_str(&format!("failed to decode WirePayload: {e}")))
    }

    #[derive(Clone)]
    struct EngineState {
        profile: String,
        vault: Vault,
        identity: Identity,
        client_identity_bytes: Zeroizing<Vec<u8>>,
        spk: PreKeyPair,
        spk_id: i64,
        otks: Vec<(i64, PreKeyPair)>,
        next_otk_id: i64,
        sessions: HashMap<[u8; 32], RatchetState>,
        peer_dhs: HashMap<[u8; 32], [u8; 32]>,
        lost_inits: HashMap<[u8; 32], RatchetState>,
        own_inits: HashMap<[u8; 32], ([u8; 32], i64)>,
        alternates: HashMap<[u8; 32], Vec<RatchetState>>,
    }

    #[derive(Serialize, Deserialize)]
    struct SerializedSession {
        peer_sign: [u8; 32],
        peer_dh: [u8; 32],
        bytes: Vec<u8>,
    }

    impl Drop for SerializedSession {
        fn drop(&mut self) {
            self.bytes.zeroize();
        }
    }

    #[derive(Serialize, Deserialize)]
    struct SerializedLostInit {
        peer_sign: [u8; 32],
        peer_dh: [u8; 32],
        bytes: Vec<u8>,
    }

    impl Drop for SerializedLostInit {
        fn drop(&mut self) {
            self.bytes.zeroize();
        }
    }

    #[derive(Serialize, Deserialize)]
    struct SerializedOwnInit {
        peer_sign: [u8; 32],
        dh: [u8; 32],
        ts: i64,
    }

    #[derive(Serialize, Deserialize)]
    struct SerializedAlternate {
        peer_sign: [u8; 32],
        bytes: Vec<u8>,
    }

    impl Drop for SerializedAlternate {
        fn drop(&mut self) {
            self.bytes.zeroize();
        }
    }

    #[derive(Serialize, Deserialize)]
    struct VaultPayload {
        version: u8,
        profile: String,
        identity_sign_seed: [u8; 32],
        identity_dh_sk: [u8; 32],
        client_identity_bytes: Vec<u8>,
        spk_id: i64,
        spk_secret: [u8; 32],
        otks: Vec<(i64, [u8; 32])>,
        next_otk_id: i64,
        sessions: Vec<SerializedSession>,
        lost_inits: Vec<SerializedLostInit>,
        own_inits: Vec<SerializedOwnInit>,
        alternates: Vec<SerializedAlternate>,
        metadata_json: String,
    }

    impl Drop for VaultPayload {
        fn drop(&mut self) {
            self.identity_sign_seed.zeroize();
            self.identity_dh_sk.zeroize();
            self.client_identity_bytes.zeroize();
            self.spk_secret.zeroize();
            for (_, sec) in &mut self.otks {
                sec.zeroize();
            }
            self.metadata_json.zeroize();
        }
    }

    impl EngineState {
        fn serialize_vault_payload(
            &self,
            metadata_json: &str,
        ) -> Result<Zeroizing<Vec<u8>>, String> {
            let (identity_sign_seed, identity_dh_sk) = self
                .identity
                .with_secret_bytes(|sign, dh| (Zeroizing::new(*sign), Zeroizing::new(*dh)))
                .map_err(|e| format!("export identity secret: {e:?}"))?;

            let spk_secret = {
                let guard = self
                    .spk
                    .expose_secret()
                    .map_err(|e| format!("expose spk secret: {e:?}"))?;
                Zeroizing::new(*guard)
            };

            let mut otks = Zeroizing::new(Vec::with_capacity(self.otks.len()));
            for (id, pair) in &self.otks {
                let sec = {
                    let guard = pair
                        .expose_secret()
                        .map_err(|e| format!("expose otk secret: {e:?}"))?;
                    Zeroizing::new(*guard)
                };
                otks.push((*id, *sec));
            }

            let mut sessions = Vec::with_capacity(self.sessions.len());
            for (sign, state) in &self.sessions {
                let dh = self.peer_dhs.get(sign).copied().unwrap_or([0u8; 32]);
                let bytes = state
                    .to_bytes()
                    .map_err(|e| format!("serialize session: {e:?}"))?;
                sessions.push(SerializedSession {
                    peer_sign: *sign,
                    peer_dh: dh,
                    bytes: bytes.to_vec(),
                });
            }

            let mut lost_inits = Vec::with_capacity(self.lost_inits.len());
            for (sign, state) in &self.lost_inits {
                let dh = self.peer_dhs.get(sign).copied().unwrap_or([0u8; 32]);
                let bytes = state
                    .to_bytes()
                    .map_err(|e| format!("serialize lost init: {e:?}"))?;
                lost_inits.push(SerializedLostInit {
                    peer_sign: *sign,
                    peer_dh: dh,
                    bytes: bytes.to_vec(),
                });
            }

            let mut own_inits = Vec::with_capacity(self.own_inits.len());
            for (sign, (dh, ts)) in &self.own_inits {
                own_inits.push(SerializedOwnInit {
                    peer_sign: *sign,
                    dh: *dh,
                    ts: *ts,
                });
            }

            let mut alternates = Vec::new();
            for (sign, states) in &self.alternates {
                for state in states {
                    let bytes = state
                        .to_bytes()
                        .map_err(|e| format!("serialize alternate: {e:?}"))?;
                    alternates.push(SerializedAlternate {
                        peer_sign: *sign,
                        bytes: bytes.to_vec(),
                    });
                }
            }

            let payload = VaultPayload {
                version: 1,
                profile: self.profile.clone(),
                identity_sign_seed: *identity_sign_seed,
                identity_dh_sk: *identity_dh_sk,
                client_identity_bytes: self.client_identity_bytes.to_vec(),
                spk_id: self.spk_id,
                spk_secret: *spk_secret,
                otks: std::mem::take(&mut *otks),
                next_otk_id: self.next_otk_id,
                sessions,
                lost_inits,
                own_inits,
                alternates,
                metadata_json: metadata_json.to_string(),
            };

            let bytes = bincode::serialize(&payload)
                .map_err(|e| format!("serialize vault payload: {e}"))?;
            Ok(Zeroizing::new(bytes))
        }

        fn deserialize_vault_payload(
            profile: &str,
            vault: Vault,
            bytes: &[u8],
        ) -> Result<(Self, String), String> {
            let mut payload: VaultPayload = bincode_options()
                .deserialize(bytes)
                .map_err(|e| format!("deserialize vault payload: {e}"))?;

            // 1. Version and profile validation
            if payload.version != 1 {
                return Err("unsupported vault payload version".into());
            }
            if payload.profile != profile {
                return Err("profile name mismatch in payload".into());
            }

            // 2. Count limits validation
            if payload.sessions.len() > MAX_SESSIONS {
                return Err("payload exceeds max sessions limit".into());
            }
            if payload.otks.len() > MAX_OPK {
                return Err("payload exceeds max OTKs limit".into());
            }
            if payload.lost_inits.len() > MAX_SESSIONS {
                return Err("payload exceeds max lost inits limit".into());
            }
            if payload.own_inits.len() > MAX_SESSIONS {
                return Err("payload exceeds max own inits limit".into());
            }

            // 3. Duplicates validation
            let mut seen_sessions = HashSet::new();
            for s in &payload.sessions {
                if !seen_sessions.insert(s.peer_sign) {
                    return Err("duplicate peer_sign in sessions".into());
                }
            }

            let mut seen_lost = HashSet::new();
            for l in &payload.lost_inits {
                if !seen_lost.insert(l.peer_sign) {
                    return Err("duplicate peer_sign in lost_inits".into());
                }
            }

            let mut seen_own = HashSet::new();
            for o in &payload.own_inits {
                if !seen_own.insert(o.peer_sign) {
                    return Err("duplicate peer_sign in own_inits".into());
                }
            }

            let mut seen_otks = HashSet::new();
            for (id, _) in &payload.otks {
                if !seen_otks.insert(*id) {
                    return Err("duplicate otk_id in otks".into());
                }
            }

            // 4. Session alternates bounds validation
            let mut alt_counts = HashMap::new();
            for a in &payload.alternates {
                let count = alt_counts.entry(a.peer_sign).or_insert(0usize);
                *count += 1;
                if *count > MAX_ALTERNATES {
                    return Err("peer exceeds max session alternates limit".into());
                }
            }

            // 5. Construct state using zeroizing guards and drain/take to avoid moving out of Drop
            let sign_seed = Zeroizing::new(payload.identity_sign_seed);
            let dh_sk = Zeroizing::new(payload.identity_dh_sk);
            let identity = Identity::try_from_bytes(*sign_seed, *dh_sk)
                .map_err(|e| format!("restore identity: {e:?}"))?;

            let client_identity_raw =
                Zeroizing::new(std::mem::take(&mut payload.client_identity_bytes));
            let client_identity = ClientIdentity::from_bytes(&client_identity_raw)
                .map_err(|e| format!("restore client identity: {e:?}"))?;
            let client_identity_bytes = Zeroizing::new(client_identity.to_bytes());

            let spk_secret = Zeroizing::new(payload.spk_secret);
            let spk = PreKeyPair::try_from_secret(*spk_secret)
                .map_err(|e| format!("restore spk: {e:?}"))?;

            let mut otks = Vec::with_capacity(payload.otks.len());
            for (id, sec) in &mut payload.otks {
                let sec_guard = Zeroizing::new(*sec);
                sec.zeroize();
                let pair = PreKeyPair::try_from_secret(*sec_guard)
                    .map_err(|e| format!("restore otk: {e:?}"))?;
                otks.push((*id, pair));
            }

            let mut sessions = HashMap::new();
            let mut peer_dhs = HashMap::new();
            for mut s in payload.sessions.drain(..) {
                let bytes = Zeroizing::new(std::mem::take(&mut s.bytes));
                let state = RatchetState::from_bytes(&bytes)
                    .map_err(|e| format!("restore session ratchet: {e:?}"))?;
                sessions.insert(s.peer_sign, state);
                peer_dhs.insert(s.peer_sign, s.peer_dh);
            }

            let mut lost_inits = HashMap::new();
            for mut l in payload.lost_inits.drain(..) {
                let bytes = Zeroizing::new(std::mem::take(&mut l.bytes));
                let state = RatchetState::from_bytes(&bytes)
                    .map_err(|e| format!("restore lost init ratchet: {e:?}"))?;
                lost_inits.insert(l.peer_sign, state);
                peer_dhs.insert(l.peer_sign, l.peer_dh);
            }

            let mut own_inits = HashMap::new();
            for o in payload.own_inits.drain(..) {
                own_inits.insert(o.peer_sign, (o.dh, o.ts));
            }

            let mut alternates = HashMap::new();
            for mut a in payload.alternates.drain(..) {
                let bytes = Zeroizing::new(std::mem::take(&mut a.bytes));
                let state = RatchetState::from_bytes(&bytes)
                    .map_err(|e| format!("restore alternate ratchet: {e:?}"))?;
                let list: &mut Vec<RatchetState> = alternates.entry(a.peer_sign).or_default();
                if list.len() < MAX_ALTERNATES {
                    list.push(state);
                }
            }

            let state = Self {
                profile: payload.profile,
                vault,
                identity,
                client_identity_bytes,
                spk,
                spk_id: payload.spk_id,
                otks,
                next_otk_id: payload.next_otk_id,
                sessions,
                peer_dhs,
                lost_inits,
                own_inits,
                alternates,
            };

            Ok((state, payload.metadata_json))
        }

        fn refill_otks(&mut self) -> Result<(), JsValue> {
            while self.otks.len() < TARGET_OPK && self.otks.len() < MAX_OPK {
                let pair = PreKeyPair::try_generate()
                    .map_err(|e| JsValue::from_str(&format!("otk generate error: {e:?}")))?;
                let id = self.next_otk_id;
                self.next_otk_id = self.next_otk_id.wrapping_add(1);
                self.otks.push((id, pair));
            }
            if self.otks.len() > MAX_OPK {
                self.otks.truncate(MAX_OPK);
            }
            Ok(())
        }
    }

    struct CandidateState {
        state: EngineState,
        sealed_gen: Option<u64>,
    }

    struct EngineInner {
        committed: Option<EngineState>,
        candidate: Option<CandidateState>,
        metadata: Option<String>,
        closed: bool,
    }

    impl EngineInner {
        fn committed_state(&self) -> Result<&EngineState, JsValue> {
            if self.closed {
                return Err(JsValue::from_str("engine is closed"));
            }
            self.committed
                .as_ref()
                .ok_or_else(|| JsValue::from_str("engine state not initialized"))
        }

        fn active_candidate_mut(&mut self) -> Result<&mut CandidateState, JsValue> {
            if self.closed {
                return Err(JsValue::from_str("engine is closed"));
            }
            let cand = self
                .candidate
                .as_mut()
                .ok_or_else(|| JsValue::from_str("no active transaction (call begin first)"))?;
            if cand.sealed_gen.is_some() {
                return Err(JsValue::from_str(
                    "candidate is sealed (call commit or rollback)",
                ));
            }
            Ok(cand)
        }
    }

    #[derive(Serialize)]
    struct PasskeyDescriptor {
        #[serde(rename = "credentialId")]
        credential_id: Vec<u8>,
        #[serde(rename = "rpId")]
        rp_id: String,
        salt: Vec<u8>,
    }

    #[derive(Serialize)]
    struct PublicInfo {
        card: String,
        sign_pk: String,
        dh_pk: String,
        destination: String,
        onion: String,
        fingerprint: String,
    }

    #[derive(Serialize)]
    struct DecryptResult<'a> {
        sign_pk: String,
        dh_pk: String,
        payload: &'a WirePayload,
    }

    #[derive(Default)]
    pub(crate) struct EngineLifetime {
        closed: Cell<bool>,
        callbacks: RefCell<Vec<Box<dyn FnOnce()>>>,
    }

    impl EngineLifetime {
        pub(crate) fn on_close(&self, callback: impl FnOnce() + 'static) {
            if self.closed.get() {
                callback();
            } else {
                self.callbacks.borrow_mut().push(Box::new(callback));
            }
        }
        fn close(&self) {
            if !self.closed.replace(true) {
                let callbacks = std::mem::take(&mut *self.callbacks.borrow_mut());
                for callback in callbacks {
                    callback();
                }
            }
        }
    }

    #[wasm_bindgen]
    pub struct BrowserEngine {
        inner: RefCell<EngineInner>,
        lifetime: Rc<EngineLifetime>,
    }

    #[wasm_bindgen]
    impl BrowserEngine {
        /// Creates a brand new profile with fresh keys, emissary ClientIdentity,
        /// and an initialized Vault.
        pub fn create(profile: &str, password: &str) -> Result<BrowserEngine, JsValue> {
            if profile.is_empty() || profile.len() > 128 {
                return Err(JsValue::from_str("invalid profile name length"));
            }
            let vault = Vault::create(profile, password).map_err(|e| JsValue::from_str(&e))?;
            let identity = Identity::try_generate()
                .map_err(|e| JsValue::from_str(&format!("identity generation failed: {e:?}")))?;

            let client_identity = ClientIdentity::generate::<BrowserRuntime>();
            let client_identity_bytes = Zeroizing::new(client_identity.to_bytes());

            let spk = PreKeyPair::try_generate()
                .map_err(|e| JsValue::from_str(&format!("spk generation failed: {e:?}")))?;
            let spk_id = 1;

            let mut otks = Vec::with_capacity(TARGET_OPK);
            for id in 1..=(TARGET_OPK as i64) {
                let pair = PreKeyPair::try_generate()
                    .map_err(|e| JsValue::from_str(&format!("otk generation failed: {e:?}")))?;
                otks.push((id, pair));
            }
            let next_otk_id = (TARGET_OPK as i64) + 1;

            let state = EngineState {
                profile: profile.to_string(),
                vault,
                identity,
                client_identity_bytes,
                spk,
                spk_id,
                otks,
                next_otk_id,
                sessions: HashMap::new(),
                peer_dhs: HashMap::new(),
                lost_inits: HashMap::new(),
                own_inits: HashMap::new(),
                alternates: HashMap::new(),
            };

            let engine = BrowserEngine {
                lifetime: Rc::default(),
                inner: RefCell::new(EngineInner {
                    committed: Some(state),
                    candidate: None,
                    metadata: Some("{}".to_string()),
                    closed: false,
                }),
            };
            Ok(engine)
        }

        /// Opens an existing profile using password from the newest complete record of slotA / slotB.
        pub fn open_password(
            profile: &str,
            password: &str,
            slot_a: &[u8],
            slot_b: &[u8],
        ) -> Result<BrowserEngine, JsValue> {
            let newest_bytes = newest([slot_a, slot_b]).map_err(|e| JsValue::from_str(&e))?;
            let (vault, plaintext) = Vault::open_password(profile, password, newest_bytes)
                .map_err(|e| JsValue::from_str(&e))?;

            let (state, metadata) =
                EngineState::deserialize_vault_payload(profile, vault, &plaintext)
                    .map_err(|e| JsValue::from_str(&e))?;

            let engine = BrowserEngine {
                lifetime: Rc::default(),
                inner: RefCell::new(EngineInner {
                    committed: Some(state),
                    candidate: None,
                    metadata: Some(metadata),
                    closed: false,
                }),
            };
            Ok(engine)
        }

        /// Opens an existing profile using WebAuthn passkey PRF from newest slot.
        pub fn open_passkey(
            profile: &str,
            credential_id: &[u8],
            prf: &[u8],
            slot_a: &[u8],
            slot_b: &[u8],
        ) -> Result<BrowserEngine, JsValue> {
            let newest_bytes = newest([slot_a, slot_b]).map_err(|e| JsValue::from_str(&e))?;
            let (vault, plaintext) = Vault::open_passkey(profile, credential_id, prf, newest_bytes)
                .map_err(|e| JsValue::from_str(&e))?;

            let (state, metadata) =
                EngineState::deserialize_vault_payload(profile, vault, &plaintext)
                    .map_err(|e| JsValue::from_str(&e))?;

            let engine = BrowserEngine {
                lifetime: Rc::default(),
                inner: RefCell::new(EngineInner {
                    committed: Some(state),
                    candidate: None,
                    metadata: Some(metadata),
                    closed: false,
                }),
            };
            Ok(engine)
        }

        /// Inspects slotA / slotB, chooses newest valid record, and returns passkey descriptors as JSON:
        /// `[{ "credentialId": number[], "rpId": string, "salt": number[] }]`
        pub fn passkey_entries(slot_a: &[u8], slot_b: &[u8]) -> Result<String, JsValue> {
            let newest_bytes = match newest([slot_a, slot_b]) {
                Ok(b) => b,
                Err(_) => return Ok("[]".to_string()),
            };
            let record = match Record::parse(newest_bytes) {
                Ok(r) => r,
                Err(_) => return Ok("[]".to_string()),
            };

            let list: Vec<PasskeyDescriptor> = record
                .passkeys()
                .iter()
                .map(|p| PasskeyDescriptor {
                    credential_id: p.credential_id.clone(),
                    rp_id: p.rp_id.clone(),
                    salt: p.salt.to_vec(),
                })
                .collect();

            serde_json::to_string(&list)
                .map_err(|e| JsValue::from_str(&format!("JSON serialization failed: {e}")))
        }

        /// Takes the decrypted metadata JSON (default "{}" on create).
        /// Can only be taken once per open/create; clears plaintext from memory.
        pub fn take_metadata(&self) -> Result<String, JsValue> {
            let mut inner = self.inner.borrow_mut();
            if inner.closed {
                return Err(JsValue::from_str("engine is closed"));
            }
            Ok(inner.metadata.take().unwrap_or_default())
        }

        /// Returns public information from COMMITTED state as JSON:
        /// `{ "card": string, "sign_pk": hex, "dh_pk": hex, "destination": base64, "onion": "....b32.i2p", "fingerprint": hex }`
        pub fn public_info(&self) -> Result<String, JsValue> {
            let inner = self.inner.borrow();
            let state = inner.committed_state()?;

            let client_identity = ClientIdentity::from_bytes(&state.client_identity_bytes)
                .map_err(|e| JsValue::from_str(&format!("restore client identity: {e:?}")))?;
            let dest_bytes = client_identity.destination().serialize();
            let destination = emissary_core::crypto::base64_encode(&dest_bytes);
            let dest_hash: [u8; 32] = Sha256::digest(&dest_bytes).into();
            let onion = format!(
                "{}.b32.i2p",
                emissary_core::crypto::base32_encode(&dest_hash).to_ascii_lowercase()
            );

            let sign_pk = state.identity.card().sign_pk;
            let dh_pk = state.identity.card().dh_pk;

            let card_obj = ContactCard {
                onion: onion.clone(),
                sign_pk,
                dh_pk,
                relay: None,
                name: None,
            };
            let card = card_obj.encode().unwrap_or_default();
            let fingerprint = to_hex(&state.identity.fingerprint());

            let info = PublicInfo {
                card,
                sign_pk: to_hex(&sign_pk),
                dh_pk: to_hex(&dh_pk),
                destination,
                onion,
                fingerprint,
            };

            serde_json::to_string(&info)
                .map_err(|e| JsValue::from_str(&format!("JSON serialization failed: {e}")))
        }

        /// Returns bincode-serialized PreKeyBundle from COMMITTED state for public relay distribution.
        pub fn my_bundle(&self) -> Result<Vec<u8>, JsValue> {
            let inner = self.inner.borrow();
            let state = inner.committed_state()?;

            let opk = state.otks.first().map(|(id, kp)| (*id, kp));
            let bundle = PreKeyBundle::new(&state.identity, &state.spk, opk);
            bincode::serialize(&bundle)
                .map_err(|e| JsValue::from_str(&format!("bundle serialization: {e}")))
        }

        /// Returns whether a ratchet session exists for the given peer signing PK (hex).
        pub fn has_session(&self, peer_sign_hex: &str) -> bool {
            let peer_sign = match parse_hex32(peer_sign_hex) {
                Ok(b) => b,
                Err(_) => return false,
            };
            let inner = self.inner.borrow();
            match inner.committed_state() {
                Ok(state) => state.sessions.contains_key(&peer_sign),
                Err(_) => false,
            }
        }

        /// Forget a peer only inside a durable transaction. Removing an
        /// unsent initial from the outbox must also discard that ratchet.
        pub fn forget_peer(&self, peer_sign_hex: &str) -> Result<(), JsValue> {
            let peer = parse_hex32(peer_sign_hex)?;
            let mut inner = self.inner.borrow_mut();
            let state = &mut inner.active_candidate_mut()?.state;
            state.sessions.remove(&peer);
            state.peer_dhs.remove(&peer);
            state.lost_inits.remove(&peer);
            state.own_inits.remove(&peer);
            state.alternates.remove(&peer);
            Ok(())
        }

        /// Begins a transaction: clones committed private state and Vault into a candidate.
        pub fn begin(&self) -> Result<(), JsValue> {
            let mut inner = self.inner.borrow_mut();
            if inner.closed {
                return Err(JsValue::from_str("engine is closed"));
            }
            if inner.candidate.is_some() {
                return Err(JsValue::from_str("transaction already in progress"));
            }
            let committed = inner
                .committed
                .as_ref()
                .ok_or_else(|| JsValue::from_str("engine not initialized"))?;
            inner.candidate = Some(CandidateState {
                state: committed.clone(),
                sealed_gen: None,
            });
            Ok(())
        }

        /// Discards the current candidate state without touching committed crypto.
        pub fn rollback(&self) {
            let mut inner = self.inner.borrow_mut();
            inner.candidate = None;
        }

        /// Serializes candidate private state and caller-provided metadata JSON,
        /// encrypts via Vault::seal_next, freezes candidate, and returns the encrypted record bytes.
        pub fn seal(&self, metadata_json: &str) -> Result<Vec<u8>, JsValue> {
            let mut inner = self.inner.borrow_mut();
            if inner.closed {
                return Err(JsValue::from_str("engine is closed"));
            }
            let cand = inner
                .candidate
                .as_mut()
                .ok_or_else(|| JsValue::from_str("no active transaction (call begin first)"))?;
            if cand.sealed_gen.is_some() {
                return Err(JsValue::from_str("candidate already sealed"));
            }

            let plaintext = cand
                .state
                .serialize_vault_payload(metadata_json)
                .map_err(|e| JsValue::from_str(&e))?;

            let (gen, slot_bytes) = cand
                .state
                .vault
                .seal_next(&plaintext)
                .map_err(|e| JsValue::from_str(&e))?;

            cand.sealed_gen = Some(gen);
            Ok(slot_bytes)
        }

        /// Returns pending generation number as a string, or empty if unsealed.
        pub fn pending_generation(&self) -> String {
            let inner = self.inner.borrow();
            inner
                .candidate
                .as_ref()
                .and_then(|c| c.sealed_gen)
                .map(|g| g.to_string())
                .unwrap_or_default()
        }

        /// Commits the candidate state, replacing committed keys and incrementing vault generation.
        pub fn commit(&self) -> Result<(), JsValue> {
            let mut inner = self.inner.borrow_mut();
            if inner.closed {
                return Err(JsValue::from_str("engine is closed"));
            }
            let mut cand = inner
                .candidate
                .take()
                .ok_or_else(|| JsValue::from_str("no active transaction (call begin first)"))?;

            let gen = cand
                .sealed_gen
                .ok_or_else(|| JsValue::from_str("candidate must be sealed before commit"))?;

            cand.state
                .vault
                .committed(gen)
                .map_err(|e| JsValue::from_str(&e))?;

            inner.committed = Some(cand.state);
            Ok(())
        }

        /// Encrypts WirePayload JSON for peer. Operates ONLY on candidate.
        /// If session exists: produces EnvelopeBlob::Ratchet.
        /// If new session: verifies pinned prekey bundle and produces EnvelopeBlob::X3dhInit.
        /// Rejects new session if MAX_SESSIONS reached.
        pub fn encrypt(
            &self,
            peer_sign_hex: &str,
            peer_dh_hex: &str,
            bundle_bytes: &[u8],
            payload_json: &str,
        ) -> Result<Vec<u8>, JsValue> {
            let peer_sign = parse_hex32(peer_sign_hex)?;
            let peer_dh = parse_hex32(peer_dh_hex)?;

            let payload: WirePayload = serde_json::from_str(payload_json)
                .map_err(|e| JsValue::from_str(&format!("invalid WirePayload JSON: {e}")))?;

            let wire = encode_payload(&payload)
                .map_err(|e| JsValue::from_str(&format!("encode payload error: {e}")))?;
            let pt = pad_payload(&wire);

            let mut inner = self.inner.borrow_mut();
            let cand = inner.active_candidate_mut()?;

            let ad = build_ad(&cand.state.identity.card().dh_pk, &peer_dh);

            if let Some(session) = cand.state.sessions.get_mut(&peer_sign) {
                let (header, ct) = session
                    .encrypt(&pt, &ad)
                    .map_err(|e| JsValue::from_str(&format!("ratchet encrypt failed: {e:?}")))?;
                let envelope = EnvelopeBlob::Ratchet {
                    header,
                    ciphertext: ct,
                };
                return bincode::serialize(&envelope)
                    .map_err(|e| JsValue::from_str(&format!("serialize envelope error: {e}")));
            }

            // New session: reject if session table is full (no silent eviction of live sessions)
            if cand.state.sessions.len() >= MAX_SESSIONS {
                return Err(JsValue::from_str("maximum session limit reached"));
            }

            if bundle_bytes.is_empty() {
                return Err(JsValue::from_str(
                    "no existing session with peer and no prekey bundle provided",
                ));
            }

            let bundle: PreKeyBundle = bincode_bundle_options()
                .deserialize(bundle_bytes)
                .map_err(|e| JsValue::from_str(&format!("deserialize bundle failed: {e}")))?;

            bundle.verify().map_err(|e| {
                JsValue::from_str(&format!("bundle signature verification failed: {e:?}"))
            })?;

            if bundle.identity.sign_pk != peer_sign {
                return Err(JsValue::from_str("bundle signing public key mismatch"));
            }
            if bundle.identity.dh_pk != peer_dh {
                return Err(JsValue::from_str("bundle DH public key mismatch"));
            }

            let (session, init) = crypto::x3dh_initiate(&cand.state.identity, &bundle, &pt, &ad)
                .map_err(|e| JsValue::from_str(&format!("x3dh initiate failed: {e:?}")))?;

            cand.state
                .own_inits
                .insert(peer_sign, (init.header.dh, now_ms()));
            cand.state.lost_inits.remove(&peer_sign);
            cand.state.peer_dhs.insert(peer_sign, peer_dh);

            if let Some(old_session) = cand.state.sessions.insert(peer_sign, session) {
                let alts = cand.state.alternates.entry(peer_sign).or_default();
                alts.insert(0, old_session);
                if alts.len() > MAX_ALTERNATES {
                    alts.truncate(MAX_ALTERNATES);
                }
            }

            let envelope = EnvelopeBlob::X3dhInit(init);
            bincode::serialize(&envelope)
                .map_err(|e| JsValue::from_str(&format!("serialize envelope error: {e}")))
        }

        /// Decrypts incoming EnvelopeBlob. Operates ONLY on candidate.
        /// Handles simultaneous init collision (ours_stands lower signing key wins if own initial fresh).
        /// Refills consumed OTKs to ~20 (bounded <= 256).
        /// For Ratchet: tries primary session + up to 3 alternates using probe clones so failures don't corrupt state.
        /// Returns JSON: `{ "sign_pk": hex, "dh_pk": hex, "payload": WirePayload }`.
        pub fn decrypt(
            &self,
            from_sign_hex: &str,
            expected_dh_hex: &str,
            envelope: &[u8],
        ) -> Result<String, JsValue> {
            if (envelope.len() as u64) > MAX_ENVELOPE_BYTES {
                return Err(JsValue::from_str("envelope exceeds size limit"));
            }

            let blob: EnvelopeBlob = bincode_options()
                .deserialize(envelope)
                .map_err(|e| JsValue::from_str(&format!("invalid envelope format: {e}")))?;

            let from_sign_opt = if !from_sign_hex.trim().is_empty() {
                Some(parse_hex32(from_sign_hex)?)
            } else {
                None
            };

            let expected_dh_opt = if !expected_dh_hex.trim().is_empty() {
                Some(parse_hex32(expected_dh_hex)?)
            } else {
                None
            };

            let mut inner = self.inner.borrow_mut();
            let cand = inner.active_candidate_mut()?;

            match blob {
                EnvelopeBlob::X3dhInit(init) => {
                    let sender_sign = init.identity.sign_pk;
                    let sender_dh = init.identity.dh_pk;

                    if let Some(expected_sign) = from_sign_opt {
                        if expected_sign != sender_sign {
                            return Err(JsValue::from_str(
                                "init sender signing key does not match relay from PK",
                            ));
                        }
                    }
                    if let Some(expected_dh) = expected_dh_opt {
                        if expected_dh != sender_dh {
                            return Err(JsValue::from_str(
                                "init sender DH key does not match pinned DH key",
                            ));
                        }
                    }

                    let ad = build_ad(&cand.state.identity.card().dh_pk, &sender_dh);

                    let crossed = cand.state.own_inits.contains_key(&sender_sign)
                        && cand.state.sessions.contains_key(&sender_sign)
                        && !cand.state.lost_inits.contains_key(&sender_sign);

                    let is_fresh =
                        if let Some((_, init_time)) = cand.state.own_inits.get(&sender_sign) {
                            (now_ms() - init_time).abs() < FRESH_SESSION_GRACE_MS
                        } else {
                            false
                        };

                    if crossed
                        && is_fresh
                        && ours_stands(&cand.state.identity.card().sign_pk, &sender_sign)
                    {
                        // Ours stands: accept their X3dhInit into lost_inits, but keep our session in sessions.
                        let (theirs_session, pt) = accept_x3dh(&mut cand.state, &init, &ad)?;
                        cand.state.lost_inits.insert(sender_sign, theirs_session);
                        cand.state.peer_dhs.insert(sender_sign, sender_dh);

                        let payload = decode_with_padding_fallback(&pt)?;
                        return format_decrypt_result(&sender_sign, &sender_dh, &payload);
                    }

                    // Otherwise, theirs stands or standard new session.
                    // Reject new session if full and not already present in sessions
                    if !cand.state.sessions.contains_key(&sender_sign)
                        && cand.state.sessions.len() >= MAX_SESSIONS
                    {
                        return Err(JsValue::from_str("maximum session limit reached"));
                    }

                    cand.state.own_inits.remove(&sender_sign);
                    cand.state.lost_inits.remove(&sender_sign);

                    let (new_session, pt) = accept_x3dh(&mut cand.state, &init, &ad)?;

                    if let Some(old_session) = cand.state.sessions.insert(sender_sign, new_session)
                    {
                        let alts = cand.state.alternates.entry(sender_sign).or_default();
                        alts.insert(0, old_session);
                        if alts.len() > MAX_ALTERNATES {
                            alts.truncate(MAX_ALTERNATES);
                        }
                    }
                    cand.state.peer_dhs.insert(sender_sign, sender_dh);

                    let payload = decode_with_padding_fallback(&pt)?;
                    format_decrypt_result(&sender_sign, &sender_dh, &payload)
                }
                EnvelopeBlob::Ratchet { header, ciphertext } => {
                    if let Some(peer_sign) = from_sign_opt {
                        let peer_dh = expected_dh_opt
                            .or_else(|| cand.state.peer_dhs.get(&peer_sign).copied())
                            .ok_or_else(|| {
                                JsValue::from_str("unknown peer DH key for ratchet decryption")
                            })?;

                        let ad = build_ad(&cand.state.identity.card().dh_pk, &peer_dh);

                        // 1. Try primary session
                        if let Some(session) = cand.state.sessions.get_mut(&peer_sign) {
                            let mut probe = session.clone();
                            if let Ok(pt) = probe.decrypt(&header, &ciphertext, &ad) {
                                *session = probe;
                                cand.state.own_inits.remove(&peer_sign);
                                let payload = decode_with_padding_fallback(&pt)?;
                                return format_decrypt_result(&peer_sign, &peer_dh, &payload);
                            }
                        }

                        // 2. Try lost_inits session
                        if let Some(lost) = cand.state.lost_inits.get_mut(&peer_sign) {
                            let mut probe = lost.clone();
                            if let Ok(pt) = probe.decrypt(&header, &ciphertext, &ad) {
                                *lost = probe;
                                let payload = decode_with_padding_fallback(&pt)?;
                                return format_decrypt_result(&peer_sign, &peer_dh, &payload);
                            }
                        }

                        // 3. Try alternates (up to 3)
                        if let Some(alts) = cand.state.alternates.get_mut(&peer_sign) {
                            for alt in alts.iter_mut() {
                                let mut probe = alt.clone();
                                if let Ok(pt) = probe.decrypt(&header, &ciphertext, &ad) {
                                    *alt = probe;
                                    let payload = decode_with_padding_fallback(&pt)?;
                                    return format_decrypt_result(&peer_sign, &peer_dh, &payload);
                                }
                            }
                        }

                        return Err(JsValue::from_str("ratchet decryption failed"));
                    }

                    // Unknown sender (sealed envelope): search candidate sessions
                    for (sign, dh) in cand.state.peer_dhs.clone() {
                        let ad = build_ad(&cand.state.identity.card().dh_pk, &dh);

                        if let Some(session) = cand.state.sessions.get_mut(&sign) {
                            let mut probe = session.clone();
                            if let Ok(pt) = probe.decrypt(&header, &ciphertext, &ad) {
                                *session = probe;
                                cand.state.own_inits.remove(&sign);
                                let payload = decode_with_padding_fallback(&pt)?;
                                return format_decrypt_result(&sign, &dh, &payload);
                            }
                        }

                        if let Some(lost) = cand.state.lost_inits.get_mut(&sign) {
                            let mut probe = lost.clone();
                            if let Ok(pt) = probe.decrypt(&header, &ciphertext, &ad) {
                                *lost = probe;
                                let payload = decode_with_padding_fallback(&pt)?;
                                return format_decrypt_result(&sign, &dh, &payload);
                            }
                        }

                        if let Some(alts) = cand.state.alternates.get_mut(&sign) {
                            for alt in alts.iter_mut() {
                                let mut probe = alt.clone();
                                if let Ok(pt) = probe.decrypt(&header, &ciphertext, &ad) {
                                    *alt = probe;
                                    let payload = decode_with_padding_fallback(&pt)?;
                                    return format_decrypt_result(&sign, &dh, &payload);
                                }
                            }
                        }
                    }

                    Err(JsValue::from_str(
                        "ratchet decryption failed for all known sessions",
                    ))
                }
            }
        }

        /// Signs a challenge for relay authentication using auth_v2_message bound to destination hash.
        /// Always operates on COMMITTED state.
        pub fn sign_auth(&self, destination: &str, challenge: &[u8]) -> Result<Vec<u8>, JsValue> {
            let inner = self.inner.borrow();
            let state = inner.committed_state()?;

            let dest_hash = destination_hash(destination)
                .ok_or_else(|| JsValue::from_str("invalid destination address"))?;

            let challenge_arr: [u8; 32] = challenge
                .try_into()
                .map_err(|_| JsValue::from_str("challenge must be 32 bytes"))?;

            let msg = auth_v2_message(&dest_hash, &challenge_arr);
            let sig = state.identity.sign(&msg);
            Ok(sig.to_vec())
        }

        /// Verifies a relay auth signature for the host relay.
        pub fn verify_auth(
            sign_hex: &str,
            destination: &str,
            challenge: &[u8],
            sig: &[u8],
        ) -> bool {
            let sign_bytes = match parse_hex32_opt(sign_hex) {
                Some(b) => b,
                None => return false,
            };
            let challenge_arr: [u8; 32] = match challenge.try_into() {
                Ok(b) => b,
                Err(_) => return false,
            };
            let sig_arr: [u8; 64] = match sig.try_into() {
                Ok(b) => b,
                Err(_) => return false,
            };
            let dest_hash = match destination_hash(destination) {
                Some(h) => h,
                None => return false,
            };
            let msg = auth_v2_message(&dest_hash, &challenge_arr);
            let card = crypto::IdentityCard {
                sign_pk: sign_bytes,
                dh_pk: [0u8; 32],
            };
            card.verify(&msg, &sig_arr)
        }

        /// Modifies the candidate vault password (transaction required).
        pub fn change_password(&self, old_pass: &str, new_pass: &str) -> Result<(), JsValue> {
            let mut inner = self.inner.borrow_mut();
            let cand = inner.active_candidate_mut()?;
            cand.state
                .vault
                .change_password(old_pass, new_pass)
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Verifies passphrase against COMMITTED vault.
        pub fn verify_password(&self, pass: &str) -> Result<(), JsValue> {
            let inner = self.inner.borrow();
            let state = inner.committed_state()?;
            state
                .vault
                .verify_password(pass)
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Enrolls a WebAuthn passkey in candidate vault (transaction required).
        pub fn enroll_passkey(
            &self,
            credential_id: &[u8],
            rp_id: &str,
            salt: &[u8],
            prf: &[u8],
        ) -> Result<(), JsValue> {
            let salt_arr: [u8; 32] = salt
                .try_into()
                .map_err(|_| JsValue::from_str("salt must be 32 bytes"))?;

            let mut inner = self.inner.borrow_mut();
            let cand = inner.active_candidate_mut()?;
            cand.state
                .vault
                .enroll_passkey(credential_id.to_vec(), rp_id.to_string(), salt_arr, prf)
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Wipes all sensitive memory, drops vault, candidate, and private keys.
        pub fn close(&self) {
            self.lifetime.close();
            let mut inner = self.inner.borrow_mut();
            inner.closed = true;
            inner.committed = None;
            inner.candidate = None;
            if let Some(mut m) = inner.metadata.take() {
                m.zeroize();
            }
        }
    }

    // Rust-only pub(crate) method, NEVER wasm_bindgen JS getter:
    impl BrowserEngine {
        pub(crate) fn lifetime(&self) -> Rc<EngineLifetime> {
            self.lifetime.clone()
        }
        pub(crate) fn destination_identity(&self) -> Result<ClientIdentity, String> {
            let inner = self.inner.borrow();
            let state = inner.committed_state().map_err(|e| format!("{e:?}"))?;
            ClientIdentity::from_bytes(&state.client_identity_bytes)
                .map_err(|e| format!("restore client identity: {e:?}"))
        }
    }

    impl Drop for BrowserEngine {
        fn drop(&mut self) {
            self.close();
        }
    }

    fn accept_x3dh(
        state: &mut EngineState,
        init: &X3dhInitial,
        ad: &[u8],
    ) -> Result<(RatchetState, Vec<u8>), JsValue> {
        let opk_idx_opt = if let Some(opk_id) = init.one_time_id {
            let idx = state
                .otks
                .iter()
                .position(|(id, _)| *id == opk_id)
                .ok_or_else(|| JsValue::from_str("stale or unknown one-time prekey id"))?;
            Some(idx)
        } else {
            None
        };

        let opk_pair_ref = opk_idx_opt.map(|idx| &state.otks[idx].1);

        let res = crypto::x3dh_respond(&state.identity, &state.spk, opk_pair_ref, init, ad);
        match res {
            Ok((ratchet, pt)) => {
                if let Some(idx) = opk_idx_opt {
                    state.otks.remove(idx);
                    state.refill_otks()?;
                }
                Ok((ratchet, pt))
            }
            Err(e) => Err(JsValue::from_str(&format!("x3dh respond failed: {e:?}"))),
        }
    }

    fn format_decrypt_result(
        sign_pk: &[u8; 32],
        dh_pk: &[u8; 32],
        payload: &WirePayload,
    ) -> Result<String, JsValue> {
        let res = DecryptResult {
            sign_pk: to_hex(sign_pk),
            dh_pk: to_hex(dh_pk),
            payload,
        };
        serde_json::to_string(&res)
            .map_err(|e| JsValue::from_str(&format!("JSON serialization failed: {e}")))
    }
}

#[cfg(target_arch = "wasm32")]
pub use implementation::BrowserEngine;
