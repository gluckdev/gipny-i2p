//! Core construction, background maintenance, and shutdown.

use super::*;

impl Core {
    pub async fn start(
        data_dir: PathBuf,
        db: Arc<Db>,
        node: Arc<I2pNode>,
        prebuilt_relay: Option<PrebuiltRelay>,
    ) -> Result<(Arc<Self>, mpsc::Receiver<CoreEvent>)> {
        // A staged Windows update is applied earlier than this, in `lib.rs`'s
        // `boot()` — before the vault unlock and the router wait below, not
        // after them.
        std::fs::create_dir_all(data_dir.join(ATTACHMENTS_DIR))?;
        let identity = Arc::new(Self::load_or_create_identity(&db)?);
        db.configure_dm_identity(&identity.card().sign_pk)?;
        db.cleanup_deleted_files(&data_dir.join(ATTACHMENTS_DIR))?;
        let (events_tx, events_rx) = mpsc::channel(EVENTS_CAPACITY);
        let updater = Arc::new(Updater::new(node.clone(), UpdateComponent::App));
        let (agent_tx, agent_rx) = mpsc::unbounded_channel();
        let core = Arc::new(Self {
            db: db.clone(),
            node: node.clone(),
            identity,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            events: events_tx,
            data_dir,
            relay_out: Arc::new(RwLock::new(None)),
            peer_relays: Arc::new(Mutex::new(HashMap::new())),
            bundle_waiters: Arc::new(Mutex::new(HashMap::new())),
            bundle_cache: Arc::new(Mutex::new(HashMap::new())),
            send_kick: Arc::new(tokio::sync::Notify::new()),
            own_inits: Arc::new(Mutex::new(HashMap::new())),
            lost_inits: Arc::new(Mutex::new(HashMap::new())),
            inbox_busy: Arc::default(),
            heard: Arc::new(std::sync::Mutex::new(HashMap::new())),
            file_lanes: Arc::new(Mutex::new(HashMap::new())),
            file_lanes_dialling: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            part_seen: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            flows: Arc::new(std::sync::Mutex::new(HashMap::new())),
            collect_lanes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            collect_kick: Arc::new(tokio::sync::Notify::new()),
            session_created_at: Arc::new(Mutex::new(HashMap::new())),
            incoming_since_send: Arc::new(Mutex::new(HashMap::new())),
            updater,
            pending_update: Arc::new(Mutex::new(None)),
            update_install: Arc::new(Mutex::new(())),
            prepared_update: Arc::new(Mutex::new(None)),
            tasks: Arc::new(std::sync::Mutex::new(Vec::new())),
            agent_tx,
            hosted_relay: Arc::new(std::sync::Mutex::new(None)),
            hosted_state: Arc::new(std::sync::RwLock::new(HostedRelayState::Off)),
            relay_dial: Arc::new(std::sync::RwLock::new(DialState::default())),
            hosted_task: Arc::new(std::sync::Mutex::new(None)),
            prebuilt_relay: std::sync::Mutex::new(prebuilt_relay),
            announce_pending: Arc::new(Mutex::new(HashSet::new())),
            announce_sent_at: Arc::new(Mutex::new(HashMap::new())),
            relay_down_since: Arc::new(Mutex::new(HashMap::new())),
            unreachable_reported: Arc::new(Mutex::new(HashSet::new())),
            dht: dht_client::new_node(node.clone(), db.clone()),
            dht_addr_asked: Arc::new(Mutex::new(HashMap::new())),
            rtt: Arc::new(Mutex::new(HashMap::new())),
        });
        core.ensure_prekeys().await?;
        let _ = core.db.cleanup_orphan_pins();
        core.clone().spawn_relay_loop();
        core.clone().spawn_collect_lanes();
        core.clone().spawn_resume_acks();
        core.clone().spawn_send_loop();
        core.clone().spawn_purge_loop();
        let _ = core.db.delete_setting(RETIRED_UI_DATA);
        core.clone().spawn_update_loop();
        core.clone().spawn_agent_worker(agent_rx);
        core.clone().spawn_dht_loop();
        if core.relay_mode() == RelayMode::Builtin {
            core.start_hosted_relay();
        } else if let Some(task) = core
            .prebuilt_relay
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            // Built before unlock on the chance it was wanted; it was not, and
            // a relay left running keeps a destination published for nothing.
            task.abort();
        }
        core.clone().warm_peer_relays();
        if let Some(m) = core.agent_master() {
            // Commands that arrived while the app was closed are on disk with
            // their pending markers; pick them up in order.
            core.enqueue_pending_commands(m.contact_id);
        }
        Ok((core, events_rx))
    }

    /// Keep a task to stop with the rest at shutdown; finished ones go.
    pub(super) fn track(&self, handle: JoinHandle<()>) {
        let mut v = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        v.retain(|h| !h.is_finished());
        v.push(handle);
    }

    pub fn shutdown(&self) {
        let mut v = self.tasks.lock().unwrap();
        for h in v.drain(..) {
            h.abort();
        }
        drop(v);
        // The last sender of a connection gone is what closes it. Held here,
        // a stopped instance kept every connection open, and its relays went
        // on dealing mail to them until they fell silent (90 s).
        if let Ok(mut out) = self.relay_out.try_write() {
            *out = None;
        }
        if let Ok(mut peers) = self.peer_relays.try_lock() {
            peers.clear();
        }
        if let Ok(mut lanes) = self.file_lanes.try_lock() {
            lanes.clear();
        }
    }

    pub fn db(&self) -> &Arc<Db> {
        &self.db
    }

    pub(super) fn spawn_purge_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(PURGE_INTERVAL_SECS));
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            tick.tick().await;
            let mut bundle_refresh =
                tokio::time::interval(Duration::from_secs(BUNDLE_REFRESH_SECS));
            bundle_refresh.set_missed_tick_behavior(MissedTickBehavior::Skip);
            bundle_refresh.tick().await;
            // Letters left in the inbox by a crash or a storage failure.
            let mut inbox_retry = tokio::time::interval(Duration::from_secs(INBOX_RETRY_SECS));
            inbox_retry.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = inbox_retry.tick() => this.drain_inbox().await,
                    _ = tick.tick() => {
                        let _ = this.db.purge_expired(now_ms());
                        this.gc_files();
                        let _ = this.db.purge_old_deferred_pins(now_ms() - 7 * 24 * 3600 * 1000);
                    }
                    _ = bundle_refresh.tick() => {
                        this.republish_bundle().await;
                    }
                }
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }
}
