//! Coalescing of slow identical reads. `box status` takes seconds (a sandbox container per task), and the
//! window refetches on every change event, so without this a burst of events would start a burst of
//! containers.

use crate::error::BridgeResult;
use std::future::Future;
use std::time::Instant;
use tokio::sync::Mutex;

/// Runs one computation at a time. A caller that arrives while a run is in flight waits and then shares
/// the next run (never the one that was already under way, which may predate what it wants to see).
#[derive(Debug)]
pub struct SingleFlight<T> {
    last: Mutex<Option<(Instant, BridgeResult<T>)>>,
}

impl<T> Default for SingleFlight<T> {
    fn default() -> Self {
        Self {
            last: Mutex::new(None),
        }
    }
}

impl<T: Clone> SingleFlight<T> {
    /// Returns the result of a run that started after this call did, starting one if none has.
    pub async fn run<F, Fut>(&self, work: F) -> BridgeResult<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = BridgeResult<T>>,
    {
        let arrived = Instant::now();
        let mut last = self.last.lock().await;
        if let Some((started, result)) = last.as_ref() {
            if *started >= arrived {
                return result.clone();
            }
        }
        let started = Instant::now();
        let result = work().await;
        *last = Some((started, result.clone()));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn concurrent_callers_share_one_follow_up_run() {
        let flight = Arc::new(SingleFlight::<usize>::default());
        let runs = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..6 {
            let (flight, runs) = (Arc::clone(&flight), Arc::clone(&runs));
            handles.push(tokio::spawn(async move {
                flight
                    .run(|| async {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        Ok(runs.fetch_add(1, Ordering::SeqCst) + 1)
                    })
                    .await
            }));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        for handle in handles {
            assert!(handle.await.unwrap().is_ok());
        }
        assert!(
            runs.load(Ordering::SeqCst) <= 2,
            "{} runs",
            runs.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn a_later_call_runs_again_and_errors_are_shared_not_cached_forever() {
        let flight = SingleFlight::<u8>::default();
        let first = flight
            .run(|| async {
                Err(crate::error::BridgeError::failed(crate::said::Said::new(
                    "x",
                )))
            })
            .await;
        assert!(first.is_err());
        let second = flight.run(|| async { Ok(7) }).await;
        assert_eq!(second.unwrap(), 7);
    }
}
