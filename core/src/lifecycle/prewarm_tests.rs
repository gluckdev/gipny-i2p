use super::{abort_prewarm_build, drop_prepared};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::{sync::oneshot, task::JoinHandle};

struct Resource(Arc<AtomicUsize>);

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
async fn completed_outer_build_cannot_detach_a_still_running_relay() {
    let nodes = Arc::new(AtomicUsize::new(0));
    let relays = Arc::new(AtomicUsize::new(0));
    let relay = pending_relay(relays.clone()).await;
    let node = Resource(nodes.clone());
    let outer = tokio::spawn(async move { Ok::<_, ()>((node, relay)) });
    // Reproduce the exact race: abort will have no effect on this completed
    // outer task, while its returned relay builder is still running.
    while !outer.is_finished() {
        tokio::task::yield_now().await;
    }
    abort_prewarm_build(outer).await;
    assert_eq!(nodes.load(Ordering::SeqCst), 1);
    assert_eq!(relays.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelling_an_incomplete_build_drops_its_client_before_returning() {
    let nodes = Arc::new(AtomicUsize::new(0));
    let resource = Resource(nodes.clone());
    let (started, running) = oneshot::channel();
    let outer = tokio::spawn(async move {
        let _node = resource;
        started.send(()).unwrap();
        std::future::pending::<Result<(Resource, JoinHandle<()>), ()>>().await
    });
    running.await.unwrap();
    abort_prewarm_build(outer).await;
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
    drop_prepared(Resource(nodes.clone()), relay).await;
    assert_eq!(nodes.load(Ordering::SeqCst), 1);
    assert_eq!(relays.load(Ordering::SeqCst), 1);
}
