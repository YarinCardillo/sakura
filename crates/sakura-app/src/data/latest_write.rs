use gpui::BackgroundExecutor;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;

pub type Completion = watch::Receiver<Option<bool>>;

struct State<T> {
    next: Option<T>,
    completion: Option<watch::Sender<Option<bool>>>,
    is_writing: bool,
}

/// Serializes writes of complete file snapshots, keeping only the latest pending snapshot.
pub struct LatestWrite<T>(Arc<Mutex<State<T>>>);

impl<T> Clone for LatestWrite<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for LatestWrite<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(State {
            next: None,
            completion: None,
            is_writing: false,
        })))
    }
}

impl<T: Send + 'static> LatestWrite<T> {
    /// True while a writer is scheduled or still processing snapshots.
    #[cfg(not(feature = "boxes"))]
    pub fn is_writing(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_writing
    }

    pub fn submit(
        &self,
        next: T,
        write: impl Fn(T) -> bool + Send + 'static,
        executor: &BackgroundExecutor,
    ) {
        self.queue(next, false, write, executor);
    }

    #[cfg(any(test, not(feature = "boxes")))]
    pub fn submit_with_receipt(
        &self,
        next: T,
        write: impl Fn(T) -> bool + Send + 'static,
        executor: &BackgroundExecutor,
    ) -> Completion {
        self.queue(next, true, write, executor)
            .expect("a requested receipt")
    }

    fn queue(
        &self,
        next: T,
        wants_completion: bool,
        write: impl Fn(T) -> bool + Send + 'static,
        executor: &BackgroundExecutor,
    ) -> Option<Completion> {
        let saved = {
            let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            state.next = Some(next);
            let saved = wants_completion.then(|| {
                state
                    .completion
                    .get_or_insert_with(|| watch::channel(None).0)
                    .subscribe()
            });
            if state.is_writing {
                return saved;
            }
            state.is_writing = true;
            saved
        };
        let queue = self.0.clone();
        executor
            .spawn(async move {
                loop {
                    let (next, completion) = {
                        let mut state = queue.lock().unwrap_or_else(PoisonError::into_inner);
                        let Some(next) = state.next.take() else {
                            state.is_writing = false;
                            return;
                        };
                        (next, state.completion.take())
                    };
                    let did_save = write(next);
                    if let Some(completion) = completion {
                        completion.send_replace(Some(did_save));
                    }
                }
            })
            .detach();
        saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_completions<T>(writer: &LatestWrite<T>) -> usize {
        usize::from(writer.0.lock().unwrap().completion.is_some())
    }

    #[gpui::test]
    fn review_queued_writes_keep_one_shared_completion(cx: &mut gpui::TestAppContext) {
        let writer = LatestWrite::default();
        let receipts: Vec<_> = (0..32)
            .map(|value| writer.submit_with_receipt(value, |_| true, &cx.executor()))
            .collect();
        assert_eq!(pending_completions(&writer), 1);
        cx.run_until_parked();
        assert!(receipts
            .iter()
            .all(|receipt| *receipt.borrow() == Some(true)));
    }

    #[gpui::test]
    fn review_fire_and_forget_writes_allocate_no_completion(cx: &mut gpui::TestAppContext) {
        let writer = LatestWrite::default();
        writer.submit(1, |_| true, &cx.executor());
        assert_eq!(pending_completions(&writer), 0);
    }

    #[gpui::test]
    fn only_the_latest_snapshot_waits_for_disk(cx: &mut gpui::TestAppContext) {
        let writer = LatestWrite::default();
        let written = Arc::new(Mutex::new(Vec::new()));
        for value in 0..32 {
            let written = written.clone();
            drop(writer.submit_with_receipt(
                value,
                move |value| {
                    written.lock().unwrap().push(value);
                    true
                },
                &cx.executor(),
            ));
        }
        cx.run_until_parked();
        assert_eq!(*written.lock().unwrap(), [31]);
    }
}
