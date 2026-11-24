//! Whether a dialog (Settings, the digest's) is open over the window. The tab keys are app-wide commands that
//! run while the window is being dispatched to, when the window cannot be asked, so a dialog says it is open
//! itself: it holds a [`DialogToken`] for as long as it lives.

use gpui::{App, Global};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// How many dialogs are open.
#[derive(Clone, Default)]
pub(super) struct OpenDialogs(Arc<AtomicUsize>);

impl Global for OpenDialogs {}

/// Held by a dialog while it is open; dropping it says the dialog is gone.
pub struct DialogToken(Arc<AtomicUsize>);

impl DialogToken {
    /// Says a dialog is open until the token is dropped.
    pub fn take(cx: &mut App) -> Self {
        let open = cx.default_global::<OpenDialogs>().0.clone();
        open.fetch_add(1, Ordering::SeqCst);
        Self(open)
    }
}

impl Drop for DialogToken {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether a dialog is open, so the keys of the tabs under it do nothing.
pub fn is_open(cx: &App) -> bool {
    cx.try_global::<OpenDialogs>()
        .is_some_and(|open| open.0.load(Ordering::SeqCst) > 0)
}
