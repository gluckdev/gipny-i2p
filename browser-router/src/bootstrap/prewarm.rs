//! Cancellable preparation of public seeds only. No router, identity, socket
//! transport configuration or TaskScope exists at this stage.
use futures::future::{select, AbortHandle, Abortable, Either};
use std::future::Future;
use wasm_bindgen::prelude::*;

/// Includes optional CacheStorage reads/writes, which need not settle when a
/// browser is shutting down or storage is unhealthy. The HTTP sublimit is 45s.
const PUBLIC_BOOTSTRAP_DEADLINE_MS: u32 = 60_000;

async fn bounded<F: Future<Output = Result<T, String>>, T>(
    work: F,
    timeout: impl Future<Output = ()>,
) -> Result<T, String> {
    futures::pin_mut!(work, timeout);
    match select(work, timeout).await {
        Either::Left((result, _)) => result,
        Either::Right(_) => Err("public bootstrap deadline (60s)".into()),
    }
}

#[wasm_bindgen]
pub struct PublicBootstrap {
    abort: AbortHandle,
    result: js_sys::Promise,
}

#[wasm_bindgen]
impl PublicBootstrap {
    #[wasm_bindgen(constructor)]
    pub fn new(url: &str, digest: &str) -> Result<PublicBootstrap, JsValue> {
        super::browser::validate_source(url, digest).map_err(|e| JsValue::from_str(&e))?;
        let url = url.to_owned();
        let digest = digest.to_owned();
        let (abort, registration) = AbortHandle::new_pair();
        let result = wasm_bindgen_futures::future_to_promise(async move {
            let started = web_time::Instant::now();
            let work = bounded(
                super::bootstrap_routers(&url, &digest),
                gloo_timers::future::TimeoutFuture::new(PUBLIC_BOOTSTRAP_DEADLINE_MS),
            );
            let snapshot = Abortable::new(work, registration)
                .await
                .map_err(|_| JsValue::from_str("public bootstrap cancelled"))?
                .map_err(|e| JsValue::from_str(&e))?;
            // The caller may join this work, but network startup revalidates
            // cached records. An old in-memory result never bypasses freshness.
            let value = serde_json::json!({
                "elapsed_ms": started.elapsed().as_millis(),
                "snapshot": snapshot.receipt,
            });
            Ok(JsValue::from_str(&value.to_string()))
        });
        Ok(Self { abort, result })
    }

    /// Every caller joins the same bounded operation.
    pub fn ready(&self) -> js_sys::Promise {
        self.result.clone()
    }

    pub fn close(&self) {
        // Dropping the fetch future also drops AbortFetch and aborts its HTTP
        // request. Already committed public cache entries need no rollback.
        self.abort.abort();
    }
}

impl Drop for PublicBootstrap {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    async fn public_bootstrap_deadline_drops_a_never_settling_cache_job() {
        struct Dropped(Rc<Cell<bool>>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let dropped = Rc::new(Cell::new(false));
        let guard = Dropped(dropped.clone());
        let work = async move {
            let _guard = guard;
            futures::future::pending::<Result<(), String>>().await
        };
        let result = bounded(work, gloo_timers::future::TimeoutFuture::new(5)).await;
        assert!(result.unwrap_err().contains("deadline"));
        assert!(
            dropped.get(),
            "deadline must drop cache/fetch work, not detach it"
        );
    }
}
