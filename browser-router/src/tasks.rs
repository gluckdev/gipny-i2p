//! Own all core tasks for a browser router run, including cancellation paths.
use futures::future::{AbortHandle, Abortable};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct State {
    next: u64,
    tasks: BTreeMap<u64, AbortHandle>,
    closed: bool,
}
thread_local! {
    static CURRENT: RefCell<Option<Arc<Mutex<State>>>> = const { RefCell::new(None) };
}

pub(crate) struct TaskScope(Arc<Mutex<State>>);

impl TaskScope {
    pub fn enter() -> Result<Self, &'static str> {
        CURRENT.with(|slot| {
            let mut current = slot.borrow_mut();
            if current.is_some() {
                return Err("one router run per browser thread is supported");
            }
            let state = Arc::new(Mutex::new(State::default()));
            *current = Some(state.clone());
            Ok(Self(state))
        })
    }

    pub fn abort_all(&self) {
        let handles = {
            let mut state = self.0.lock().unwrap();
            state.closed = true;
            std::mem::take(&mut state.tasks)
        };
        for handle in handles.into_values() {
            handle.abort();
        }
    }
}

impl Drop for TaskScope {
    fn drop(&mut self) {
        CURRENT.with(|slot| {
            let mut current = slot.borrow_mut();
            if current.as_ref().is_some_and(|s| Arc::ptr_eq(s, &self.0)) {
                *current = None;
            }
        });
        let handles = {
            let mut state = self.0.lock().unwrap();
            state.closed = true;
            std::mem::take(&mut state.tasks)
        };
        for handle in handles.into_values() {
            handle.abort();
        }
    }
}

pub(crate) fn spawn<F>(future: F)
where
    F: Future + Send + 'static,
    F::Output: Send,
{
    let scope = CURRENT.with(|slot| slot.borrow().clone());
    if let Some(scope) = scope {
        let (abort, registration) = AbortHandle::new_pair();
        let id = {
            let mut state = scope.lock().unwrap();
            if state.closed {
                return;
            }
            let id = state.next;
            state.next += 1;
            state.tasks.insert(id, abort);
            id
        };
        wasm_bindgen_futures::spawn_local(async move {
            let _ = Abortable::new(future, registration).await;
            scope.lock().unwrap().tasks.remove(&id);
        });
    } else {
        wasm_bindgen_futures::spawn_local(async move {
            future.await;
        });
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use crate::BrowserRuntime;
    use emissary_core::runtime::Runtime;
    use wasm_bindgen_test::*;

    #[wasm_bindgen_test]
    async fn dropping_router_scope_cancels_an_already_running_child() {
        let scope = TaskScope::enter().unwrap();
        assert!(TaskScope::enter().is_err());
        let (started, ready) = futures::channel::oneshot::channel();
        let (done, finished) = futures::channel::oneshot::channel();
        BrowserRuntime::spawn(async move {
            let _ = started.send(());
            BrowserRuntime::delay(std::time::Duration::from_secs(3600)).await;
            let _ = done.send(());
        });
        ready.await.unwrap();
        drop(scope);
        assert!(finished.await.is_err());
        assert!(TaskScope::enter().is_ok());
    }
}
