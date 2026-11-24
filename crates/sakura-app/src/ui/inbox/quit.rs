//! Quitting with answers still held. GPUI gives the app's quit hooks 200 ms, far less than the engine may take
//! to answer a confirmed call, so the Quit command does not quit at once: it sends what may be sent and quits
//! when every call has reached an outcome, each bounded by [`crate::theme::inbox::CONFIRM_BOUND`].

use super::InboxPage;
use crate::prefs::Preferences;
use gpui::{App, Global, WeakEntity};

/// The Inbox pages of the app's windows, which hold answers.
#[derive(Default)]
struct Pages(Vec<WeakEntity<InboxPage>>);

impl Global for Pages {}

/// Prevents a second Quit while answers or preferences are still being saved.
#[derive(Default)]
struct Waiting(bool);

impl Global for Waiting {}

/// Makes `page` one whose held answers a quit waits for.
pub(super) fn register(page: WeakEntity<InboxPage>, cx: &mut App) {
    cx.default_global::<Pages>().0.push(page);
}

/// Waits for held answers and outstanding preference writes, then quits.
pub fn quit_after_answers(cx: &mut App) {
    quit_after_answers_then(cx, |cx| cx.quit());
}

/// Calls `quit` after held answers reach an outcome and the latest preferences reach disk.
pub fn quit_after_answers_then(cx: &mut App, quit: impl FnOnce(&mut App) + 'static) {
    if cx.default_global::<Waiting>().0 {
        return;
    }
    let pages: Vec<_> = cx.default_global::<Pages>().0.clone();
    let mut sent = Vec::new();
    for page in pages.iter().filter_map(WeakEntity::upgrade) {
        sent.extend(page.update(cx, |page, cx| page.flush_held(cx)));
    }
    let save = sent
        .is_empty()
        .then(|| Preferences::save_before_quit(cx))
        .flatten();
    if sent.is_empty() && save.is_none() {
        return quit(cx);
    }
    cx.default_global::<Waiting>().0 = true;
    cx.spawn(async move |cx| {
        let save = if sent.is_empty() {
            save
        } else {
            for pending in sent {
                drop(pending.await);
            }
            cx.update(Preferences::save_before_quit)
        };
        if let Some(saved) = save {
            saved.await;
        }
        cx.update(|cx| {
            quit(cx);
            cx.default_global::<Waiting>().0 = false;
        });
    })
    .detach();
}
