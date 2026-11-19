//! Large-file delivery and receiver-restart regression scenario.

use super::{BotHandle, BIG_FILE_BODY};
use anyhow::{bail, Context, Result};
use gipny_libcore::{SessionEvent, SessionManager};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Notify};

pub(super) fn payload(bytes: usize) -> Vec<u8> {
    (0..bytes)
        .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect()
}

pub(super) struct Transfer<'a> {
    pub a: &'a BotHandle,
    pub b: &'a BotHandle,
    pub contact_b_in_a: i64,
    pub work_dir: &'a PathBuf,
    pub big_file: usize,
    pub restart: bool,
    pub b_file: Arc<Mutex<Option<Vec<u8>>>>,
    pub b_file_notify: Arc<Notify>,
    pub b_parts: Arc<(std::sync::atomic::AtomicU32, std::sync::atomic::AtomicU32)>,
    pub timeout: Duration,
    pub t_start: Instant,
}

impl Transfer<'_> {
    pub async fn run(self) -> Result<()> {
        let Self {
            a,
            b,
            contact_b_in_a,
            work_dir,
            big_file,
            restart,
            b_file,
            b_file_notify,
            b_parts,
            timeout,
            t_start,
        } = self;
        let data = payload(big_file);
        let t0 = Instant::now();
        a.session
            .send_message(
                contact_b_in_a,
                BIG_FILE_BODY.into(),
                vec![("big.bin".into(), data.clone())],
                None,
                None,
                None,
            )
            .await
            .context("send the big file")?;
        eprintln!("[e2e] bot-a: sent a {big_file}-byte file in parts");
        if restart {
            // A third in, bot-b goes away and comes back on the same data.
            let left = timeout.saturating_sub(t_start.elapsed());
            tokio::time::timeout(left, async {
                loop {
                    let (done, total) = (
                        b_parts.0.load(std::sync::atomic::Ordering::SeqCst),
                        b_parts.1.load(std::sync::atomic::Ordering::SeqCst),
                    );
                    if total > 0 && done * 3 >= total {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            })
            .await
            .context("timeout: bot-b never got a third of the file")?;
            let held = b_parts.0.load(std::sync::atomic::Ordering::SeqCst);
            eprintln!("[e2e] bot-b: stopping with {held} parts in");
            b.session.shutdown_and_wait().await;
            tokio::time::sleep(Duration::from_secs(10)).await;
            let (again, mut again_events) = SessionManager::start(
                work_dir.join("bot-b"),
                b.session.db.clone(),
                b.session.node.clone(),
            )
            .await
            .context("bot-b: start again")?;
            eprintln!("[e2e] bot-b: started again");
            let (b_file, b_file_notify) = (b_file.clone(), b_file_notify.clone());
            tokio::spawn(async move {
                while let Some(ev) = again_events.recv().await {
                    if let SessionEvent::FileReceived { attachment_id, .. } = ev {
                        let got = again
                            .db
                            .get_attachment(attachment_id)
                            .ok()
                            .flatten()
                            .and_then(|a| again.read_attachment(&a).ok());
                        eprintln!(
                            "[e2e] bot-b (again): file whole ({} bytes)",
                            got.as_ref().map_or(0, |g| g.len())
                        );
                        *b_file.lock().await = got;
                        b_file_notify.notify_one();
                    }
                }
            });
        }
        let left = timeout.saturating_sub(t_start.elapsed());
        tokio::time::timeout(left, async {
            loop {
                let n = b_file_notify.notified();
                if b_file.lock().await.is_some() {
                    break;
                }
                n.await;
            }
        })
        .await
        .context("timeout: the big file never arrived whole")?;
        let got = b_file.lock().await.take().unwrap_or_default();
        if got != data {
            bail!(
                "big file: {} bytes arrived, not equal to the {} sent",
                got.len(),
                data.len()
            );
        }
        let secs = t0.elapsed().as_secs_f64();
        eprintln!(
            "[e2e] big file: {big_file} bytes whole and equal in {secs:.1} s ({:.0} KiB/s)",
            big_file as f64 / 1024.0 / secs
        );
        Ok(())
    }
}
