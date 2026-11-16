use super::*;

impl Core {
    pub(super) async fn disable_agent_mode(&self, notify_master: bool) -> Result<()> {
        let prev = self.agent_master();
        self.db.delete_setting(SETTING_AGENT_MASTER)?;
        if let (true, Some(m)) = (notify_master, &prev) {
            let _ = self
                .send_console(
                    m.contact_id,
                    BODY_REVOKE.into(),
                    WireConsole::new(CONSOLE_REVOKE),
                    vec![],
                )
                .await;
        }
        gipny_libcore::debug_trace!("[agent] agent mode off");
        let _ = self
            .events
            .try_send(CoreEvent::AgentModeChanged { master: None });
        Ok(())
    }

    pub(super) fn enqueue_pending_commands(&self, master_id: i64) {
        let ids = self
            .db
            .list_setting_ids_with_prefix("console_pending_")
            .unwrap_or_default();
        for id in ids {
            if let Ok(Some(m)) = self.db.get_message(id) {
                if m.contact_id == Some(master_id) && matches!(m.direction, Direction::In) {
                    let _ = self.agent_tx.send(id);
                }
            }
        }
    }

    pub(super) fn spawn_agent_worker(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<i64>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            while let Some(mid) = rx.recv().await {
                this.run_console_command(mid).await;
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    /// Runs one queued command. Everything is re-checked at run time, because
    /// the queue outlives mode switches: the pending marker must still be
    /// there, the mode still on, and the message still the master's.
    async fn run_console_command(self: &Arc<Self>, mid: i64) {
        let pending_key = format!("console_pending_{mid}");
        if self.db.get_setting(&pending_key).ok().flatten().is_none() {
            return;
        }
        let Some(master) = self.agent_master() else {
            return;
        };
        let msg = match self.db.get_message(mid) {
            Ok(Some(m))
                if m.contact_id == Some(master.contact_id)
                    && matches!(m.direction, Direction::In) =>
            {
                m
            }
            _ => {
                let _ = self.db.delete_setting(&pending_key);
                return;
            }
        };
        // Files still coming in parts: the command waits for them (and, the
        // worker being one queue, so do those after it). An hour at most;
        // then it runs with what is there.
        let waited_since = tokio::time::Instant::now();
        while !self
            .db
            .files_in_for_message(mid)
            .unwrap_or_default()
            .is_empty()
        {
            if !self.command_still_authorized(&pending_key, master.contact_id, mid) {
                break;
            }
            if waited_since.elapsed() > Duration::from_secs(3600) {
                eprintln!(
                    "[agent] command {mid}: its files did not all arrive; running without them"
                );
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        // The wait can last an hour: the mode may have been switched off, the
        // master changed or blocked, the message deleted. Permission is the
        // one in force now, not the one at enqueue time.
        if !self.command_still_authorized(&pending_key, master.contact_id, mid) {
            gipny_libcore::debug_trace!(
                "[agent] command {mid}: permission withdrawn while waiting; dropped"
            );
            let _ = self.db.delete_setting(&pending_key);
            return;
        }
        let _ = self.db.delete_setting(&pending_key);
        let files = self.load_attachment_data(mid).unwrap_or_default();
        gipny_libcore::debug_trace!("[agent] executing command with {} file(s)", files.len());
        let reply = agent::handle_console_request(&msg.body, &files, &ExecOptions::default()).await;
        gipny_libcore::debug_trace!(
            "[agent] exit={:?} dur={:?}ms truncated={}",
            reply.console.exit_code,
            reply.console.duration_ms,
            reply.console.truncated,
        );
        let atts = reply
            .attachments
            .into_iter()
            .map(|(name, data)| PendingAttachment {
                name,
                data,
                from: None,
            })
            .collect();
        match self
            .send_console(master.contact_id, reply.body, reply.console, atts)
            .await
        {
            Ok(_) => {
                let _ = self.events.try_send(CoreEvent::ConsoleActivity {
                    contact_id: master.contact_id,
                });
            }
            Err(e) => gipny_libcore::debug_error!("[agent] reply failed: {e}"),
        }
    }

    /// The command is still pending, its message still there, and its sender
    /// still the unblocked master.
    fn command_still_authorized(&self, pending_key: &str, master_id: i64, mid: i64) -> bool {
        self.db.get_setting(pending_key).ok().flatten().is_some()
            && self
                .agent_master()
                .is_some_and(|m| m.contact_id == master_id)
            && self
                .db
                .get_contact(master_id)
                .ok()
                .flatten()
                .is_some_and(|c| c.trust != TrustLevel::Blocked)
            && self.db.get_message(mid).ok().flatten().is_some_and(|m| {
                m.contact_id == Some(master_id) && matches!(m.direction, Direction::In)
            })
    }

    fn load_attachment_data(&self, msg_id: i64) -> Result<Vec<(String, Vec<u8>)>> {
        let mut out = Vec::new();
        for a in self.db.list_attachments(msg_id)? {
            let key = to_arr32(a.key.clone())?;
            let full = self.data_dir.join(ATTACHMENTS_DIR).join(&a.path);
            let data =
                gipny_libcore::files::read_attachment(&full, key, a.size as u64, a.chunk_size)?;
            out.push((a.name, data));
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentMaster {
    pub contact_id: i64,
    pub name: String,
    pub sign_pk: String,
}

impl Core {
    // ----- agent mode ------------------------------------------------------

    /// The master this client runs console commands for: agent mode is on and
    /// that contact still exists.
    pub fn agent_master(&self) -> Option<AgentMaster> {
        let pk = self.db.get_setting(SETTING_AGENT_MASTER).ok().flatten()?;
        let pk: [u8; 32] = pk.as_slice().try_into().ok()?;
        let c = self.db.find_contact_by_sign_pk(&pk).ok().flatten()?;
        Some(AgentMaster {
            contact_id: c.id,
            name: c.display_name,
            sign_pk: to_hex(&c.identity_sign),
        })
    }

    /// Switches agent mode on for `contact_id`, or off with `None`. On: the
    /// master is told (GRANT), and every command of theirs that arrived while
    /// the mode was off is queued to run now, oldest first.
    pub async fn set_agent_mode(self: &Arc<Self>, contact_id: Option<i64>) -> Result<()> {
        let Some(cid) = contact_id else {
            return self.disable_agent_mode(true).await;
        };
        let c = self.db.get_contact(cid)?.ok_or(CoreError::NotFound)?;
        if c.trust == TrustLevel::Blocked {
            return Err(CoreError::State);
        }
        if let Some(prev) = self.agent_master() {
            if prev.contact_id != cid {
                let _ = self
                    .send_console(
                        prev.contact_id,
                        BODY_REVOKE.into(),
                        WireConsole::new(CONSOLE_REVOKE),
                        vec![],
                    )
                    .await;
            }
        }
        self.db
            .set_setting(SETTING_AGENT_MASTER, &c.identity_sign)?;
        self.send_console(
            cid,
            BODY_GRANT.into(),
            WireConsole::new(CONSOLE_GRANT),
            vec![],
        )
        .await?;
        gipny_libcore::debug_trace!("[agent] agent mode on, master = contact {cid}");
        let _ = self.events.try_send(CoreEvent::AgentModeChanged {
            master: self.agent_master(),
        });
        self.enqueue_pending_commands(cid);
        Ok(())
    }

    /// Sends a console-framed message: a command, its output, or a control
    /// marker. The frame rides in `console_<id>` next to the row, as buttons
    /// do, so it is queued, retried and acked like any other message.
    pub async fn send_console(
        &self,
        contact_id: i64,
        body: String,
        console: WireConsole,
        attachments: Vec<PendingAttachment>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let console = bincode::serialize(&console)?;
        let staged = self.store_outgoing(&attachments)?;
        let msg_id = staged.publish_dm(
            &self.db,
            &NewOutgoingDm {
                contact_id,
                body: &body,
                sent_at,
                expires_at: None,
                reply_to: None,
                buttons: None,
                notify_sound: None,
                console: Some(&console),
            },
        )?;
        self.send_kick.notify_one();
        Ok(msg_id)
    }
}
