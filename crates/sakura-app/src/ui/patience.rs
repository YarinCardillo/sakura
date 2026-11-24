//! A reading line appears only when a read is slow enough to be seen. Reads are made in-process and
//! answer within a frame or two; a line that flashed on every click would read as a wait. A view in
//! its reading state draws nothing for [`SLOW_READ`], and is drawn again then, if it still waits, to
//! show its line.

use gpui::Context;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

pub use crate::theme::SLOW_READ;

/// When a view entered its reading state; none while it is not in it.
#[derive(Debug, Default, Clone)]
pub struct Patience {
    since: Rc<Cell<Option<Instant>>>,
}

impl Patience {
    /// Whether the reading line shows now, for a view drawn in its reading state: not for the first
    /// [`SLOW_READ`], after which the view is drawn again if it is still in that state.
    pub fn shows<V: 'static>(&mut self, cx: &mut Context<V>) -> bool {
        // The executor's clock: the test platform's own in tests.
        let now = cx.background_executor().now();
        if let Some(since) = self.since.get() {
            return now.saturating_duration_since(since) >= SLOW_READ;
        }
        let since = now;
        self.since.set(Some(since));
        let waiting = self.since.clone();
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(SLOW_READ).await;
            if waiting.get() == Some(since) {
                drop(view.update(cx, |_, cx| cx.notify()));
            }
        })
        .detach();
        false
    }

    /// The view is not in its reading state.
    pub fn reset(&mut self) {
        self.since.set(None);
    }
}
