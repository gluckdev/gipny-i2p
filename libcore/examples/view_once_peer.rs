//! In-memory native endpoint for the cross-platform one-time letter test.
//! stdin/stdout carry synthetic test input only; no network or profile files.
use gipny_libcore::{
    view_once::{self, Letter, Offer, Request},
    Identity, IdentityCard,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, Write},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize, Serialize)]
struct Card {
    sign_pk: String,
    dh_pk: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(s: &str) -> Option<[u8; 32]> {
    if !s.is_ascii() || s.len() != 64 {
        return None;
    }
    let mut result = [0; 32];
    for (i, x) in result.iter_mut().enumerate() {
        *x = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(result)
}

#[derive(Deserialize)]
struct Args {
    op: String,
    address: Option<String>,
    challenge: Option<[u8; 32]>,
    proof: Option<Vec<u8>>,

    peer: Option<Card>,
    id: Option<[u8; 16]>,
    nonce: Option<[u8; 32]>,
    expires_at: Option<i64>,
    body: Option<String>,
    offer: Option<Vec<u8>>,
    envelope: Option<Vec<u8>>,
    index: Option<u16>,
    total: Option<u16>,
    bytes: Option<Vec<u8>>,
}

struct Peer {
    identity: Identity,
    offer: Option<Offer>,
    letter: Option<Letter>,
    parts: view_once::PartBuffer,
}

impl Peer {
    fn execute(&mut self, arg: Args) -> Option<Value> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis() as i64;
        match arg.op.as_str() {
            "issue" | "seal" | "seal_parts" => {
                let card = arg.peer?;
                let peer = IdentityCard {
                    sign_pk: unhex(&card.sign_pk)?,
                    dh_pk: unhex(&card.dh_pk)?,
                };
                let to = if arg.op == "issue" {
                    self.identity.card().sign_pk
                } else {
                    peer.sign_pk
                };
                let req = Request {
                    to,
                    id: arg.id?,
                    nonce: arg.nonce?,
                    expires_at: arg.expires_at?,
                };
                if arg.op == "issue" {
                    let (key, wire) = Offer::issue(&self.identity, peer, req, now).ok()?;
                    self.offer = Some(key);
                    self.parts = Default::default();
                    Some(json!({"offer": wire}))
                } else {
                    let wire = view_once::seal(
                        &self.identity,
                        &peer,
                        &req,
                        &arg.offer?,
                        arg.body?.as_bytes(),
                        now,
                    )
                    .ok()?;
                    if arg.op == "seal_parts" {
                        let parts = view_once::parts(&wire);
                        let total = parts.len();
                        let frames: Vec<_> = parts.enumerate().map(|(index, bytes)| json!({ "index": index, "total": total, "bytes": bytes })).collect();
                        return Some(json!({ "parts": frames }));
                    }
                    Some(json!({"envelope": wire}))
                }
            }
            "accept" => {
                self.letter = Some(self.offer.take()?.accept(&arg.envelope?, now).ok()?);
                Some(json!({"accepted": true}))
            }
            "part" => {
                let index = arg.index?;
                let _ = self.offer.as_ref()?;
                match self.parts.push(index, arg.total?, &arg.bytes?) {
                    Ok(None) => Some(json!({"ack":index})),
                    Ok(Some(wire)) => {
                        self.letter = Some(self.offer.take()?.accept(&wire, now).ok()?);
                        Some(json!({"accepted":true}))
                    }
                    Err(_) => {
                        self.offer = None;
                        None
                    }
                }
            }
            "open" => {
                let body = self.letter.take()?.open(&self.identity, now).ok()?;
                Some(json!({"body": std::str::from_utf8(&body).ok()?}))
            }
            "issue_terminal_proof" | "verify_terminal_proof" => {
                let peer = arg.peer?;
                let peer_key = unhex(&peer.sign_pk)?;
                let hash = gipny_libcore::relay::destination_hash(&arg.address?)?;
                let creating = arg.op == "issue_terminal_proof";
                let requester = if creating {
                    peer_key
                } else {
                    self.identity.card().sign_pk
                };
                let msg = gipny_libcore::relay::terminal_proof_message(
                    &hash,
                    &requester,
                    &arg.challenge?,
                    &arg.nonce?,
                );
                if creating {
                    Some(json!({"proof":self.identity.sign(&msg).to_vec()}))
                } else {
                    let card = IdentityCard {
                        sign_pk: peer_key,
                        dh_pk: unhex(&peer.dh_pk)?,
                    };
                    Some(json!({"verified":card.verify(&msg,&arg.proof?.try_into().ok()?)}))
                }
            }
            _ => None,
        }
    }
}

fn main() -> io::Result<()> {
    let mut peer = Peer {
        identity: Identity::generate(),
        offer: None,
        letter: None,
        parts: Default::default(),
    };
    let keys = peer.identity.card();
    let mut out = io::stdout().lock();
    writeln!(
        out,
        "{}",
        json!(Card {
            sign_pk: hex(&keys.sign_pk),
            dh_pk: hex(&keys.dh_pk)
        })
    )?;
    out.flush()?;
    for line in io::stdin().lock().lines() {
        let result = serde_json::from_str(&line?)
            .ok()
            .and_then(|args| peer.execute(args));
        writeln!(out, "{}", result.unwrap_or_else(|| json!({"error": true})))?;
        out.flush()?;
    }
    Ok(())
}
