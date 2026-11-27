//! Serialize side effects before publishing the process-global router.

use std::sync::{Mutex, OnceLock};

pub(super) fn get_or_start<T: Clone, E>(
    value: &OnceLock<T>,
    initialization: &Mutex<()>,
    start: impl FnOnce() -> Result<T, E>,
    interrupted: impl FnOnce() -> E,
) -> Result<T, E> {
    if let Some(value) = value.get() {
        return Ok(value.clone());
    }
    let _owner = initialization.lock().map_err(|_| interrupted())?;
    if let Some(value) = value.get() {
        return Ok(value.clone());
    }
    let started = start()?;
    // Keep pre-native failures retryable. The embedded router records partial
    // native initialization and rejects further starts in that process.
    Ok(value.get_or_init(|| started).clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    };

    #[test]
    fn concurrent_calls_perform_one_start_and_share_its_result() {
        let value = Arc::new(OnceLock::new());
        let initialization = Arc::new(Mutex::new(()));
        let starts = Arc::new(AtomicUsize::new(0));
        let together = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (value, initialization, starts, together) = (
                    value.clone(),
                    initialization.clone(),
                    starts.clone(),
                    together.clone(),
                );
                std::thread::spawn(move || {
                    together.wait();
                    get_or_start(
                        &value,
                        &initialization,
                        || {
                            starts.fetch_add(1, Ordering::SeqCst);
                            std::thread::yield_now();
                            Ok::<_, ()>(Arc::new("one router"))
                        },
                        || (),
                    )
                    .unwrap()
                })
            })
            .collect();
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert!(results
            .iter()
            .all(|result| Arc::ptr_eq(result, &results[0])));
    }

    #[test]
    fn ordinary_start_failure_can_retry_without_publishing_a_value() {
        let value = OnceLock::new();
        let initialization = Mutex::new(());
        assert_eq!(
            get_or_start(
                &value,
                &initialization,
                || Err::<u8, _>("failed"),
                || "poisoned"
            ),
            Err("failed")
        );
        assert!(value.get().is_none());
        assert_eq!(
            get_or_start(&value, &initialization, || Ok(3), || "poisoned"),
            Ok(3)
        );
        assert_eq!(
            get_or_start(
                &value,
                &initialization,
                || panic!("must reuse"),
                || "poisoned"
            ),
            Ok(3)
        );
    }

    #[test]
    fn panicked_initializer_does_not_attempt_a_second_native_start() {
        let value = OnceLock::<u8>::new();
        let initialization = Mutex::new(());
        assert!(std::panic::catch_unwind(|| {
            let _ = get_or_start(
                &value,
                &initialization,
                || -> Result<u8, &str> { panic!("interrupted") },
                || "poisoned",
            );
        })
        .is_err());
        assert_eq!(
            get_or_start(
                &value,
                &initialization,
                || panic!("must not restart"),
                || "poisoned"
            ),
            Err("poisoned")
        );
    }
}
