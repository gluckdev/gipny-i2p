use super::{builders, drop_prepared, reuse_ready_destination};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::{sync::oneshot, task::JoinHandle};

struct Resource(Arc<AtomicUsize>);

async fn discard<T>(output: T) {
    drop(output);
}

impl Drop for Resource {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

async fn pending_relay(dropped: Arc<AtomicUsize>) -> JoinHandle<()> {
    let (started, running) = oneshot::channel();
    let task = tokio::spawn(async move {
        let _resource = Resource(dropped);
        started.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    running.await.unwrap();
    task
}

#[tokio::test]
async fn completed_client_build_cannot_detach_a_still_running_relay() {
    let nodes = Arc::new(AtomicUsize::new(0));
    let relays = Arc::new(AtomicUsize::new(0));
    let relay_resource = Resource(relays.clone());
    let node_resource = Resource(nodes.clone());
    let (started, running) = oneshot::channel();
    let pair = builders::spawn(
        || Ok::<_, ()>(()),
        |_| async move { Ok(node_resource) },
        |_| async move {
            let _resource = relay_resource;
            started.send(()).unwrap();
            std::future::pending::<()>().await
        },
    );
    running.await.unwrap();
    while !pair.client_finished() {
        tokio::task::yield_now().await;
    }
    pair.cancel(discard).await;
    assert_eq!(nodes.load(Ordering::SeqCst), 1);
    assert_eq!(relays.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelling_an_incomplete_build_drops_its_client_before_returning() {
    let nodes = Arc::new(AtomicUsize::new(0));
    let resource = Resource(nodes.clone());
    let (started, running) = oneshot::channel();
    let pair = builders::spawn(
        || Ok::<_, ()>(()),
        |_| async move {
            let _node = resource;
            started.send(()).unwrap();
            std::future::pending::<Result<Resource, ()>>().await
        },
        |_| std::future::pending::<()>(),
    );
    running.await.unwrap();
    pair.cancel(discard).await;
    assert_eq!(nodes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ready_prewarm_drops_a_finished_relays_owned_output() {
    let nodes = Arc::new(AtomicUsize::new(0));
    let relays = Arc::new(AtomicUsize::new(0));
    let resource = Resource(relays.clone());
    let relay = tokio::spawn(async move { resource });
    while !relay.is_finished() {
        tokio::task::yield_now().await;
    }
    drop_prepared(Resource(nodes.clone()), relay, discard).await;
    assert_eq!(nodes.load(Ordering::SeqCst), 1);
    assert_eq!(relays.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stale_readiness_releases_both_prepared_resources_before_fallback() {
    // Each failed component independently makes reuse unsafe, even though
    // the outer prewarm used to report ready before sleep/network change.
    for (directions, lease_ready) in [
        ((false, true), true),
        ((true, false), true),
        ((true, true), false),
    ] {
        let nodes = Arc::new(AtomicUsize::new(0));
        let relays = Arc::new(AtomicUsize::new(0));
        let relay = pending_relay(relays.clone()).await;
        let reused = reuse_ready_destination(
            (Resource(nodes.clone()), relay),
            directions,
            lease_ready,
            discard,
        )
        .await;
        assert!(reused.is_none());
        assert_eq!(nodes.load(Ordering::SeqCst), 1);
        assert_eq!(relays.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn current_readiness_keeps_the_original_prepared_resources() {
    let nodes = Arc::new(AtomicUsize::new(0));
    let relays = Arc::new(AtomicUsize::new(0));
    let relay = pending_relay(relays.clone()).await;
    let relay_id = relay.id();
    let (node, relay) = reuse_ready_destination(
        (Resource(nodes.clone()), relay),
        (true, true),
        true,
        discard,
    )
    .await
    .expect("all readiness components are live");
    assert_eq!(relay.id(), relay_id);
    assert_eq!(nodes.load(Ordering::SeqCst), 0);
    assert_eq!(relays.load(Ordering::SeqCst), 0);
    drop_prepared(node, relay, discard).await;
}

#[tokio::test]
async fn completed_builder_output_gets_awaited_inner_cleanup_before_client_drop() {
    let relays = Arc::new(AtomicUsize::new(0));
    let inner = pending_relay(relays.clone()).await;
    let relay = tokio::spawn(async move { inner });
    let node_drops = Arc::new(AtomicUsize::new(0));
    // Keep the separately checked node resource alive through the async
    // cleanup, as production keeps its client destination until relay stop.
    while !relay.is_finished() {
        tokio::task::yield_now().await;
    }
    let observed_nodes = node_drops.clone();
    drop_prepared(Resource(node_drops.clone()), relay, |inner| async move {
        assert_eq!(observed_nodes.load(Ordering::SeqCst), 0);
        inner.abort();
        let _ = inner.await;
        assert_eq!(observed_nodes.load(Ordering::SeqCst), 0);
    })
    .await;
    assert_eq!(node_drops.load(Ordering::SeqCst), 1);
    assert_eq!(relays.load(Ordering::SeqCst), 1);
}
