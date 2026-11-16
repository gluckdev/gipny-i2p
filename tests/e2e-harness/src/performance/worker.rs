//! One bounded enqueue worker per endpoint, so a slow enqueue cannot block
//! consumption of incoming events in the measurement loop.

use gipny_libcore::SessionManager;
use std::{future::Future, sync::Arc, time::Instant};
use tokio::{sync::mpsc, task::JoinHandle};

pub(super) struct Job {
    pub id: Option<usize>,
    pub body: String,
    pub deadline: Instant,
}

pub(super) struct Completion {
    pub id: Option<usize>,
    pub error: Option<String>,
}

pub(super) struct Worker {
    tx: mpsc::Sender<Job>,
    rx: mpsc::Receiver<Completion>,
    task: JoinHandle<()>,
    outstanding: usize,
    limit: usize,
}

impl Worker {
    pub fn new(session: Arc<SessionManager>, contact: i64, limit: usize) -> Self {
        Self::spawn(limit, move |job| {
            let session = session.clone();
            async move {
                super::send(&session, contact, job.body, job.deadline)
                    .await
                    .map_err(|e| format!("{e:#}"))
            }
        })
    }

    fn spawn<F, Fut>(limit: usize, mut send: F) -> Self
    where
        F: FnMut(Job) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let (tx, mut jobs) = mpsc::channel::<Job>(limit);
        let (done, rx) = mpsc::channel(limit);
        let task = tokio::spawn(async move {
            while let Some(job) = jobs.recv().await {
                let id = job.id;
                let error = send(job).await.err();
                if done.send(Completion { id, error }).await.is_err() {
                    break;
                }
            }
        });
        Self {
            tx,
            rx,
            task,
            outstanding: 0,
            limit,
        }
    }

    /// Includes the worker's current job, buffered jobs, and unread completions.
    pub fn offer(&mut self, job: Job) -> bool {
        if self.outstanding >= self.limit || self.tx.try_send(job).is_err() {
            return false;
        }
        self.outstanding += 1;
        true
    }

    pub fn pending(&self) -> usize {
        self.outstanding
    }

    pub async fn completed(&mut self) -> Option<Completion> {
        let completion = self.rx.recv().await?;
        self.outstanding -= 1;
        Some(completion)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::Semaphore;

    fn job(id: usize) -> Job {
        Job {
            id: Some(id),
            body: id.to_string(),
            deadline: Instant::now() + Duration::from_secs(10),
        }
    }

    #[tokio::test]
    async fn slow_enqueue_keeps_the_caller_free_and_bounds_running_plus_queued_work() {
        let gate = Arc::new(Semaphore::new(0));
        let permit_gate = gate.clone();
        let mut worker = Worker::spawn(2, move |_| {
            let gate = permit_gate.clone();
            async move {
                gate.acquire().await.unwrap().forget();
                Ok(())
            }
        });
        assert!(worker.offer(job(1)));
        assert!(worker.offer(job(2)));
        assert!(!worker.offer(job(3)));
        assert_eq!(worker.pending(), 2);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), worker.completed())
                .await
                .is_err()
        );
        gate.add_permits(1);
        let completion = tokio::time::timeout(Duration::from_secs(1), worker.completed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completion.id, Some(1));
        assert!(completion.error.is_none());
        assert!(worker.offer(job(3)));
        assert_eq!(worker.pending(), 2);
    }

    #[tokio::test]
    async fn enqueue_failure_is_returned_with_its_probe_identity() {
        let mut worker = Worker::spawn(1, |_| async { Err("queue failed".to_owned()) });
        assert!(worker.offer(job(7)));
        let completion = worker.completed().await.unwrap();
        assert_eq!(completion.id, Some(7));
        assert_eq!(completion.error.as_deref(), Some("queue failed"));
        assert_eq!(worker.pending(), 0);
    }
}
