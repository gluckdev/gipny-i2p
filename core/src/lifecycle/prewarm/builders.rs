//! One owner for the two existing destination builders and their handoff.

use std::future::Future;
use tokio::{sync::oneshot, task::JoinHandle};

pub struct Builders<Node, Relay, Error> {
    client: Option<JoinHandle<Result<Node, Error>>>,
    relay: Option<JoinHandle<Relay>>,
}

impl<Node, Relay, Error> Drop for Builders<Node, Relay, Error> {
    fn drop(&mut self) {
        // Explicit cancel below also joins. This fallback prevents detaching
        // builders if an owning foreground future is itself cancelled.
        if let Some(client) = &self.client {
            client.abort();
        }
        if let Some(relay) = &self.relay {
            relay.abort();
        }
    }
}

impl<Node, Relay, Error> Builders<Node, Relay, Error> {
    pub(super) fn client_finished(&self) -> bool {
        self.client.as_ref().is_some_and(JoinHandle::is_finished)
    }

    pub(super) async fn finish<Cleanup: Future<Output = ()>>(
        mut self,
        cleanup: impl FnOnce(Relay) -> Cleanup,
    ) -> Option<(Node, JoinHandle<Relay>)> {
        // Await by reference so Drop retains an abort handle throughout.
        let result = self.client.as_mut()?.await;
        self.client.take();
        match result {
            Ok(Ok(node)) => Some((node, self.relay.take()?)),
            _ => {
                if let Some(relay) = self.relay.take() {
                    super::drop_prepared((), relay, cleanup).await;
                }
                None
            }
        }
    }

    pub(super) async fn cancel<Cleanup: Future<Output = ()>>(
        mut self,
        cleanup: impl FnOnce(Relay) -> Cleanup,
    ) {
        // Cancel both before waiting for either: neither pool keeps building
        // while synchronous native cleanup on the other is finishing.
        if let Some(client) = &self.client {
            client.abort();
        }
        if let Some(relay) = &self.relay {
            relay.abort();
        }
        let node = match self.client.as_mut() {
            Some(client) => client.await.ok().and_then(Result::ok),
            None => None,
        };
        self.client.take();
        if let Some(relay) = self.relay.take() {
            super::drop_prepared(node, relay, cleanup).await;
        }
    }
}

/// Bootstrap once in the client task; the relay cannot run before that result.
/// After the handoff both destination futures progress independently.
pub(in crate::lifecycle) fn spawn<Router, Node, Relay, Error, ClientFuture, RelayFuture>(
    bootstrap: impl FnOnce() -> Result<Router, Error> + Send + 'static,
    client: impl FnOnce(Router) -> ClientFuture + Send + 'static,
    relay: impl FnOnce(Option<Router>) -> RelayFuture + Send + 'static,
) -> Builders<Node, Relay, Error>
where
    Router: Clone + Send + 'static,
    Node: Send + 'static,
    Relay: Send + 'static,
    Error: Send + 'static,
    ClientFuture: Future<Output = Result<Node, Error>> + Send + 'static,
    RelayFuture: Future<Output = Relay> + Send + 'static,
{
    let (ready, router) = oneshot::channel();
    let client = tokio::spawn(async move {
        let router = bootstrap()?;
        let _ = ready.send(router.clone());
        client(router).await
    });
    let relay = tokio::spawn(async move { relay(router.await.ok()).await });
    Builders {
        client: Some(client),
        relay: Some(relay),
    }
}

#[cfg(test)]
mod tests;
