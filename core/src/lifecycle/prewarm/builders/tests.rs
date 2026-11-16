use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct Resource(Arc<AtomicUsize>);
impl Drop for Resource {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

async fn discard<T>(output: T) {
    drop(output);
}

#[tokio::test]
async fn relay_starts_after_router_but_before_client_readiness() {
    let bootstraps = Arc::new(AtomicUsize::new(0));
    let count = bootstraps.clone();
    let (client_started, client_running) = oneshot::channel();
    let (relay_started, relay_running) = oneshot::channel();
    let (client_ready, client_wait) = oneshot::channel();
    let pair = spawn(
        move || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ()>(Arc::new("router"))
        },
        |router| async move {
            client_started.send(router.clone()).unwrap();
            client_wait.await.unwrap();
            Ok(router)
        },
        |router| async move {
            let router = router.expect("bootstrap must finish first");
            relay_started.send(router).unwrap();
        },
    );
    let client_router = client_running.await.unwrap();
    // No client readiness signal has been sent: a sequential implementation
    // would hang here instead of starting its relay pool.
    let relay_router = tokio::time::timeout(std::time::Duration::from_secs(2), relay_running)
        .await
        .expect("relay must overlap client readiness")
        .unwrap();
    assert!(Arc::ptr_eq(&client_router, &relay_router));
    assert_eq!(bootstraps.load(Ordering::SeqCst), 1);
    assert!(!pair.client_finished());
    client_ready.send(()).unwrap();
    let (_, relay) = pair.finish(discard).await.unwrap();
    relay.await.unwrap();
}

#[tokio::test]
async fn failed_bootstrap_never_runs_a_client_and_closes_relay_handoff() {
    let client_calls = Arc::new(AtomicUsize::new(0));
    let calls = client_calls.clone();
    let (observed, result) = oneshot::channel();
    let pair = spawn(
        || Err::<(), _>("router failed"),
        move |_| async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        |router| async move {
            observed.send(router.is_none()).unwrap();
        },
    );
    assert!(result.await.unwrap());
    assert!(pair.finish(discard).await.is_none());
    assert_eq!(client_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn client_failure_cancels_and_joins_the_still_building_relay() {
    let drops = Arc::new(AtomicUsize::new(0));
    let resource = Resource(drops.clone());
    let (started, running) = oneshot::channel();
    let (fail, failed) = oneshot::channel();
    let pair = spawn(
        || Ok::<_, ()>(()),
        |_| async move {
            failed.await.unwrap();
            Err::<(), _>(())
        },
        |_| async move {
            let _resource = resource;
            started.send(()).unwrap();
            std::future::pending::<()>().await
        },
    );
    running.await.unwrap();
    fail.send(()).unwrap();
    assert!(pair.finish(discard).await.is_none());
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn dropping_the_coordinator_aborts_both_owned_builders() {
    let client_drops = Arc::new(AtomicUsize::new(0));
    let relay_drops = Arc::new(AtomicUsize::new(0));
    let client_resource = Resource(client_drops.clone());
    let relay_resource = Resource(relay_drops.clone());
    let (client_started, client_running) = oneshot::channel();
    let (relay_started, relay_running) = oneshot::channel();
    let pair = spawn(
        || Ok::<_, ()>(()),
        |_| async move {
            let _resource = client_resource;
            client_started.send(()).unwrap();
            std::future::pending::<Result<(), ()>>().await
        },
        |_| async move {
            let _resource = relay_resource;
            relay_started.send(()).unwrap();
            std::future::pending::<()>().await
        },
    );
    client_running.await.unwrap();
    relay_running.await.unwrap();
    drop(pair);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while client_drops.load(Ordering::SeqCst) == 0 || relay_drops.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("drop must not detach either builder");
}

#[tokio::test]
async fn completed_outputs_are_cleaned_on_cancel_instead_of_detached() {
    let nodes = Arc::new(AtomicUsize::new(0));
    let relays = Arc::new(AtomicUsize::new(0));
    let node = Resource(nodes.clone());
    let relay = Resource(relays.clone());
    let pair = spawn(
        || Ok::<_, ()>(()),
        |_| async { Ok(node) },
        |_| async { relay },
    );
    while !pair.client_finished() || !pair.relay.as_ref().unwrap().is_finished() {
        tokio::task::yield_now().await;
    }
    pair.cancel(discard).await;
    assert_eq!(nodes.load(Ordering::SeqCst), 1);
    assert_eq!(relays.load(Ordering::SeqCst), 1);
}
