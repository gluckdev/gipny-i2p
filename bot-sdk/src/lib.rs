use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use gipny_libcore::{
    Db, DuressMode, SessionEvent, TorNode, UnlockOutcome, Vault, WireButton,
};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

pub use gipny_libcore;
pub use gipny_libcore::{SessionManager, WireAttachment};

pub type HandlerFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;
pub type MessageHandler = Arc<dyn Fn(Context, IncomingMessage) -> HandlerFuture + Send + Sync>;
pub type CommandHandler = Arc<dyn Fn(Context, String) -> HandlerFuture + Send + Sync>;
pub type CallbackHandler = Arc<dyn Fn(Context, String) -> HandlerFuture + Send + Sync>;

const HANDLER_MAX_ATTEMPTS: u32 = 6;
const HANDLER_BASE_BACKOFF_MS: u64 = 1_000;
const HANDLER_MAX_BACKOFF_MS: u64 = 60_000;
const CALLBACK_SEEN_TTL_MS: i64 = 24 * 3600 * 1000;
const CHAT_QUEUE_CAPACITY: usize = 128;
const HOUSEKEEPING_INTERVAL_SECS: u64 = 600;

#[derive(Clone, Debug)]
pub enum BotTarget {
    Contact(i64),
    Group(Vec<u8>),
}

#[derive(Clone)]
pub struct IncomingMessage {
    pub contact_id: i64,
    pub sender_sign_pk: Vec<u8>,
    pub body: String,
    pub sent_at: i64,
    pub attachments: Vec<WireAttachment>,
    pub message_id: i64,
    pub group_id: Option<Vec<u8>>,
}

#[derive(Clone)]
pub struct Context {
    pub session: Arc<SessionManager>,
    pub contact_id: i64,
    pub origin_msg_id: Option<u64>,
    pub target: BotTarget,
}

impl Context {
    pub fn is_group(&self) -> bool { matches!(self.target, BotTarget::Group(_)) }

    pub fn group_id(&self) -> Option<&[u8]> {
        match &self.target { BotTarget::Group(g) => Some(g), _ => None }
    }

    pub async fn reply(&self, text: impl Into<String>) -> anyhow::Result<i64> {
        match &self.target {
            BotTarget::Contact(cid) =>
                Ok(self.session.send_message(*cid, text.into(), vec![], None, None).await?),
            BotTarget::Group(gid) =>
                Ok(self.session.send_to_group(gid, text.into(), vec![], None).await?),
        }
    }

    pub async fn reply_with_buttons(
        &self,
        text: impl Into<String>,
        buttons: Vec<Vec<(String, String)>>,
    ) -> anyhow::Result<i64> {
        let wire = to_wire_buttons(buttons);
        match &self.target {
            BotTarget::Contact(cid) =>
                Ok(self.session.send_message(*cid, text.into(), vec![], None, Some(wire)).await?),
            BotTarget::Group(gid) =>
                Ok(self.session.send_to_group(gid, text.into(), vec![], Some(wire)).await?),
        }
    }

    pub async fn send_attachment(&self, text: impl Into<String>, name: String, data: Vec<u8>) -> anyhow::Result<i64> {
        self.send_files(text, vec![(name, data)], None).await
    }

    pub async fn send_attachment_with_buttons(
        &self,
        text: impl Into<String>,
        name: String,
        data: Vec<u8>,
        buttons: Vec<Vec<(String, String)>>,
    ) -> anyhow::Result<i64> {
        self.send_files(text, vec![(name, data)], Some(buttons)).await
    }

    pub async fn send_attachments(
        &self,
        text: impl Into<String>,
        files: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<i64> {
        self.send_files(text, files, None).await
    }

    pub async fn send_attachments_with_buttons(
        &self,
        text: impl Into<String>,
        files: Vec<(String, Vec<u8>)>,
        buttons: Vec<Vec<(String, String)>>,
    ) -> anyhow::Result<i64> {
        self.send_files(text, files, Some(buttons)).await
    }

    async fn send_files(
        &self,
        text: impl Into<String>,
        files: Vec<(String, Vec<u8>)>,
        buttons: Option<Vec<Vec<(String, String)>>>,
    ) -> anyhow::Result<i64> {
        let wire = buttons.map(to_wire_buttons);
        match &self.target {
            BotTarget::Contact(cid) =>
                Ok(self.session.send_message(*cid, text.into(), files, None, wire).await?),
            BotTarget::Group(gid) =>
                Ok(self.session.send_to_group(gid, text.into(), files, wire).await?),
        }
    }

    pub async fn edit(&self, message_id: u64, new_text: impl Into<String>) -> anyhow::Result<()> {
        match &self.target {
            BotTarget::Contact(cid) =>
                Ok(self.session.send_edit(*cid, message_id, new_text.into(), None).await?),
            BotTarget::Group(gid) =>
                Ok(self.session.send_edit_group(gid, message_id, new_text.into(), None).await?),
        }
    }

    pub async fn edit_with_buttons(
        &self,
        message_id: u64,
        new_text: impl Into<String>,
        buttons: Vec<Vec<(String, String)>>,
    ) -> anyhow::Result<()> {
        let wire = to_wire_buttons(buttons);
        match &self.target {
            BotTarget::Contact(cid) =>
                Ok(self.session.send_edit(*cid, message_id, new_text.into(), Some(wire)).await?),
            BotTarget::Group(gid) =>
                Ok(self.session.send_edit_group(gid, message_id, new_text.into(), Some(wire)).await?),
        }
    }
}

fn to_wire_buttons(buttons: Vec<Vec<(String, String)>>) -> Vec<Vec<WireButton>> {
    buttons.into_iter().map(|row| {
        row.into_iter().map(|(text, data)| WireButton { text, callback_data: data }).collect()
    }).collect()
}

fn open_bot_db(data_dir: &std::path::Path, db_path: &std::path::Path, passphrase: Option<&str>) -> anyhow::Result<Db> {
    let pass = match passphrase {
        Some(p) if !p.is_empty() => p,
        _ => return Ok(Db::open_plain(db_path)?),
    };
    let vault = if Vault::exists(data_dir) {
        Vault::open(data_dir)?
    } else {
        if db_path.exists() {
            anyhow::bail!(
                "BOT_VAULT_PASS set but {} already exists in plaintext form; \
                 either move the existing data dir aside (cold start with new identity) \
                 or remove BOT_VAULT_PASS to keep using plaintext",
                db_path.display()
            );
        }
        Vault::create(data_dir, pass, None, DuressMode::Wipe, 0)?
    };
    let mk = match vault.unlock(pass)? {
        UnlockOutcome::Primary(mk) => mk,
        UnlockOutcome::Decoy(_) => anyhow::bail!("vault decoy passphrase used; refusing to open bot DB"),
        UnlockOutcome::Wiped => anyhow::bail!("vault wiped on duress; restart bot to re-create"),
    };
    Ok(Db::open(db_path, &mk)?)
}

pub struct Bot {
    data_dir: PathBuf,
    relay_onion: Option<String>,
    display_name: Option<String>,
    vault_passphrase: Option<String>,
    on_message: Option<MessageHandler>,
    on_command: Vec<(String, CommandHandler)>,
    on_callback: Option<CallbackHandler>,
}

impl Bot {
    pub fn builder() -> BotBuilder {
        BotBuilder {
            data_dir: None,
            relay_onion: None,
            display_name: None,
            vault_passphrase: None,
            on_message: None,
            on_command: vec![],
            on_callback: None,
        }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let running = self.start().await?;
        running.handle.await?
    }

    pub async fn start(self) -> anyhow::Result<RunningBot> {
        std::fs::create_dir_all(&self.data_dir)?;
        let db_path = self.data_dir.join("bot.db");
        let db = Arc::new(open_bot_db(&self.data_dir, &db_path, self.vault_passphrase.as_deref())?);
        let node = Arc::new(TorNode::start(&self.data_dir, None).await?);
        let (session, mut events) = SessionManager::start(self.data_dir.clone(), db, node).await?;

        if let Some(onion) = &self.relay_onion {
            session.set_relay_onion(onion)?;
        }
        if let Some(name) = &self.display_name {
            session.set_display_name(name)?;
        }

        let card = session.my_card();
        eprintln!("============================================");
        eprintln!("[bot] Identity card (share with users):");
        eprintln!("  sign_pk: {}", hex(&card.sign_pk));
        eprintln!("  dh_pk:   {}", hex(&card.dh_pk));
        eprintln!("============================================");

        let handlers = Arc::new(Handlers {
            on_message: self.on_message,
            on_command: self.on_command,
            on_callback: self.on_callback,
        });
        let workers: Arc<Mutex<HashMap<ChatKey, mpsc::Sender<Dispatch>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let session_for_loop = session.clone();
        let session_for_house = session.clone();

        let house_handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(HOUSEKEEPING_INTERVAL_SECS));
            tick.tick().await;
            loop {
                tick.tick().await;
                let cutoff = now_ms() - CALLBACK_SEEN_TTL_MS;
                let _ = session_for_house.db.purge_old_callback_seen(cutoff);
                let _ = session_for_house.db.purge_old_dead_letters(cutoff);
            }
        });

        let handle: JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
            let session = session_for_loop;
            let _hh = house_handle;
            while let Some(ev) = events.recv().await {
                match &ev {
                    SessionEvent::Connected => eprintln!("[bot] relay connected"),
                    SessionEvent::Disconnected => eprintln!("[bot] relay disconnected"),
                    SessionEvent::ContactAdded { contact_id } => eprintln!("[bot] contact added id={}", contact_id),
                    SessionEvent::ContactUpdated { contact_id } => eprintln!("[bot] contact updated id={}", contact_id),
                    SessionEvent::MessageDelivered { message_id } => eprintln!("[bot] delivered mid={}", message_id),
                    SessionEvent::MessageEdited { message_id, .. } => eprintln!("[bot] edited mid={}", message_id),
                    SessionEvent::MessagePinned { message_id, .. } => eprintln!("[bot] pinned mid={}", message_id),
                    SessionEvent::MessageUnpinned { message_id, .. } => eprintln!("[bot] unpinned mid={}", message_id),
                    SessionEvent::IncomingPayload { contact_id, payload, message_id } =>
                        eprintln!("[bot] incoming contact={} mid={} group={} body={:?} cb={:?} atts={} buttons={}",
                            contact_id, message_id,
                            payload.group.as_ref().map(|g| hex(&g.id)).unwrap_or_else(|| "-".into()),
                            payload.body, payload.callback_data,
                            payload.attachments.len(), payload.buttons.as_ref().map(|b| b.len()).unwrap_or(0)),
                }
                let SessionEvent::IncomingPayload { contact_id, payload, message_id } = ev else { continue; };

                let target = match &payload.group {
                    Some(g) => BotTarget::Group(g.id.clone()),
                    None => BotTarget::Contact(contact_id),
                };
                let ctx = Context {
                    session: session.clone(),
                    contact_id,
                    origin_msg_id: if payload.origin_msg_id > 0 { Some(payload.origin_msg_id) } else { None },
                    target: target.clone(),
                };

                let sender_sign_pk = match session.db.get_contact(contact_id) {
                    Ok(Some(c)) => c.identity_sign,
                    _ => { eprintln!("[bot] no contact record for id={}", contact_id); continue; }
                };

                let dispatch = if let Some(data) = payload.callback_data {
                    Dispatch::Callback { ctx, data, sender_sign_pk: sender_sign_pk.clone(), sent_at: payload.sent_at }
                } else if payload.body.is_empty() && payload.attachments.is_empty() {
                    continue;
                } else if let Some(cmd) = parse_command(&payload.body) {
                    Dispatch::Command { ctx, name: cmd.name, args: cmd.args }
                } else {
                    let msg = IncomingMessage {
                        contact_id,
                        sender_sign_pk,
                        body: payload.body.clone(),
                        sent_at: payload.sent_at,
                        attachments: payload.attachments.clone(),
                        message_id,
                        group_id: payload.group.as_ref().map(|g| g.id.clone()),
                    };
                    Dispatch::Message { ctx, msg }
                };

                let key = match &target {
                    BotTarget::Contact(c) => ChatKey::Contact(*c),
                    BotTarget::Group(g) => ChatKey::Group(g.clone()),
                };

                let tx = {
                    let mut w = workers.lock().await;
                    if let Some(tx) = w.get(&key) {
                        if !tx.is_closed() { tx.clone() }
                        else {
                            w.remove(&key);
                            spawn_chat_worker(&session, key.clone(), handlers.clone(), &mut w)
                        }
                    } else {
                        spawn_chat_worker(&session, key.clone(), handlers.clone(), &mut w)
                    }
                };
                if let Err(e) = tx.send(dispatch).await {
                    eprintln!("[bot] chat queue dropped: {}", e);
                }
            }
            Ok(())
        });

        Ok(RunningBot { session, handle })
    }
}

#[derive(Clone, Hash, PartialEq, Eq)]
enum ChatKey {
    Contact(i64),
    Group(Vec<u8>),
}

enum Dispatch {
    Command { ctx: Context, name: String, args: String },
    Callback { ctx: Context, data: String, sender_sign_pk: Vec<u8>, sent_at: i64 },
    Message { ctx: Context, msg: IncomingMessage },
}

struct Handlers {
    on_message: Option<MessageHandler>,
    on_command: Vec<(String, CommandHandler)>,
    on_callback: Option<CallbackHandler>,
}

fn spawn_chat_worker(
    session: &Arc<SessionManager>,
    key: ChatKey,
    handlers: Arc<Handlers>,
    table: &mut HashMap<ChatKey, mpsc::Sender<Dispatch>>,
) -> mpsc::Sender<Dispatch> {
    let (tx, mut rx) = mpsc::channel::<Dispatch>(CHAT_QUEUE_CAPACITY);
    let session = session.clone();
    let key_for_dl = key.clone();
    tokio::spawn(async move {
        while let Some(item) = rx.recv().await {
            run_with_retry(&session, &handlers, &key_for_dl, item).await;
        }
    });
    table.insert(key, tx.clone());
    tx
}

async fn run_with_retry(
    session: &Arc<SessionManager>,
    handlers: &Handlers,
    key: &ChatKey,
    item: Dispatch,
) {
    if let Dispatch::Callback { sender_sign_pk, sent_at, data, .. } = &item {
        match session.db.callback_seen(sender_sign_pk, *sent_at, data) {
            Ok(true) => {
                eprintln!("[bot] callback dup, skip: data={} sent_at={}", data, sent_at);
                return;
            }
            Ok(false) => { let _ = session.db.record_callback_seen(sender_sign_pk, *sent_at, data); }
            Err(e) => eprintln!("[bot] callback_seen err: {:?}", e),
        }
    }

    let kind = item.kind();
    let mut attempt: u32 = 0;
    loop {
        let r = run_once(handlers, item.clone_dispatch()).await;
        match r {
            Ok(()) => return,
            Err(e) => {
                attempt += 1;
                eprintln!("[bot] handler err ({}): attempt {} of {}: {:?}", kind, attempt, HANDLER_MAX_ATTEMPTS, e);
                if attempt >= HANDLER_MAX_ATTEMPTS {
                    save_dead_letter(session, key, &item, &format!("{:?}", e));
                    return;
                }
                let backoff = backoff_ms(attempt);
                tokio::time::sleep(Duration::from_millis(backoff)).await;
            }
        }
    }
}

async fn run_once(handlers: &Handlers, item: Dispatch) -> anyhow::Result<()> {
    match item {
        Dispatch::Command { ctx, name, args } => {
            for (n, h) in handlers.on_command.iter() {
                if *n == name {
                    return (h)(ctx, args).await;
                }
            }
            eprintln!("[bot] no handler for /{}", name);
            Ok(())
        }
        Dispatch::Callback { ctx, data, .. } => {
            match &handlers.on_callback {
                Some(h) => (h)(ctx, data).await,
                None => { eprintln!("[bot] no callback handler"); Ok(()) }
            }
        }
        Dispatch::Message { ctx, msg } => {
            match &handlers.on_message {
                Some(h) => (h)(ctx, msg).await,
                None => { eprintln!("[bot] no on_message handler"); Ok(()) }
            }
        }
    }
}

impl Dispatch {
    fn kind(&self) -> &'static str {
        match self {
            Dispatch::Command { .. } => "command",
            Dispatch::Callback { .. } => "callback",
            Dispatch::Message { .. } => "message",
        }
    }

    fn clone_dispatch(&self) -> Dispatch {
        match self {
            Dispatch::Command { ctx, name, args } => Dispatch::Command {
                ctx: ctx.clone(), name: name.clone(), args: args.clone(),
            },
            Dispatch::Callback { ctx, data, sender_sign_pk, sent_at } => Dispatch::Callback {
                ctx: ctx.clone(), data: data.clone(),
                sender_sign_pk: sender_sign_pk.clone(), sent_at: *sent_at,
            },
            Dispatch::Message { ctx, msg } => Dispatch::Message {
                ctx: ctx.clone(), msg: msg.clone(),
            },
        }
    }
}

fn save_dead_letter(session: &Arc<SessionManager>, key: &ChatKey, item: &Dispatch, err: &str) {
    let (contact_id, group_id_owned) = match key {
        ChatKey::Contact(c) => (Some(*c), None),
        ChatKey::Group(g) => (None, Some(g.clone())),
    };
    let (kind, payload) = match item {
        Dispatch::Command { name, args, .. } => ("command", format!("{}\t{}", name, args).into_bytes()),
        Dispatch::Callback { data, sent_at, .. } => ("callback", format!("{}\t{}", data, sent_at).into_bytes()),
        Dispatch::Message { msg, .. } => ("message", msg.body.clone().into_bytes()),
    };
    let _ = session.db.add_dead_letter(contact_id, group_id_owned.as_deref(), kind, &payload, err);
    eprintln!("[bot] dead-letter recorded ({}): {}", kind, err);
}

fn backoff_ms(attempt: u32) -> u64 {
    let shift = attempt.min(16);
    let v = HANDLER_BASE_BACKOFF_MS.saturating_mul(1u64 << shift);
    v.min(HANDLER_MAX_BACKOFF_MS)
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub struct RunningBot {
    pub session: Arc<SessionManager>,
    pub handle: JoinHandle<anyhow::Result<()>>,
}

pub struct BotBuilder {
    data_dir: Option<PathBuf>,
    relay_onion: Option<String>,
    display_name: Option<String>,
    vault_passphrase: Option<String>,
    on_message: Option<MessageHandler>,
    on_command: Vec<(String, CommandHandler)>,
    on_callback: Option<CallbackHandler>,
}

impl BotBuilder {
    pub fn data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(path.into()); self
    }
    pub fn relay(mut self, onion: impl Into<String>) -> Self {
        self.relay_onion = Some(onion.into()); self
    }
    pub fn display_name(mut self, name: impl Into<String>) -> Self {
        self.display_name = Some(name.into()); self
    }
    pub fn vault_passphrase(mut self, pass: impl Into<String>) -> Self {
        let p = pass.into();
        self.vault_passphrase = if p.is_empty() { None } else { Some(p) };
        self
    }
    pub fn on_message<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(Context, IncomingMessage) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.on_message = Some(Arc::new(move |ctx, msg| Box::pin(f(ctx, msg))));
        self
    }
    pub fn on_command<F, Fut>(mut self, name: impl Into<String>, f: F) -> Self
    where
        F: Fn(Context, String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let h: CommandHandler = Arc::new(move |ctx, args| Box::pin(f(ctx, args)));
        self.on_command.push((name.into(), h));
        self
    }
    pub fn on_callback<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(Context, String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.on_callback = Some(Arc::new(move |ctx, data| Box::pin(f(ctx, data))));
        self
    }
    pub fn build(self) -> anyhow::Result<Bot> {
        Ok(Bot {
            data_dir: self.data_dir.ok_or_else(|| anyhow::anyhow!("data_dir required"))?,
            relay_onion: self.relay_onion,
            display_name: self.display_name,
            vault_passphrase: self.vault_passphrase,
            on_message: self.on_message,
            on_command: self.on_command,
            on_callback: self.on_callback,
        })
    }
}

struct ParsedCommand { name: String, args: String }

fn parse_command(body: &str) -> Option<ParsedCommand> {
    if !body.starts_with('/') { return None; }
    let rest = &body[1..];
    let (name, args) = match rest.find(char::is_whitespace) {
        Some(i) => (rest[..i].to_string(), rest[i..].trim().to_string()),
        None => (rest.to_string(), String::new()),
    };
    if name.is_empty() { return None; }
    Some(ParsedCommand { name, args })
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b { s.push_str(&format!("{:02x}", x)); }
    s
}
