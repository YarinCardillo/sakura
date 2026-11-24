//! The unsent text of the composers is kept in the preferences after a pause in typing, and read back.

use super::draft_keeper::DraftKeeper;
use crate::prefs::Preferences;
use crate::theme::DRAFT_PAUSE;
use gpui::{AppContext as _, Entity, TestAppContext};
use sakura_core::drafts::Drafts;
use sakura_core::tabs::Page;
use std::time::Duration;

fn lane() -> Page {
    Page::Lane("p/a".to_string())
}

fn kept(cx: &mut TestAppContext) -> Drafts {
    cx.update(|cx| Drafts::restore(Preferences::current(cx).drafts.as_ref()))
}

fn open(cx: &mut TestAppContext, preferences: Preferences) -> Entity<DraftKeeper> {
    cx.update(|cx| preferences.clone().install(None, cx));
    cx.new(|_| DraftKeeper::new(&preferences))
}

#[gpui::test]
fn note_keeps_the_text_only_after_a_pause_in_typing(cx: &mut TestAppContext) {
    let keeper = open(cx, Preferences::default());
    keeper.update(cx, |keeper, cx| {
        keeper.note_for(&lane(), None, "", "fix the", cx)
    });
    cx.executor().advance_clock(DRAFT_PAUSE / 2);
    assert!(
        kept(cx).is_empty(),
        "nothing yet: the person is still typing"
    );
    keeper.update(cx, |keeper, cx| {
        keeper.note_for(&lane(), None, "", "fix the title", cx)
    });
    cx.executor()
        .advance_clock(DRAFT_PAUSE / 2 + Duration::from_millis(1));
    assert!(kept(cx).is_empty(), "the pause starts again at each key");
    cx.executor().advance_clock(DRAFT_PAUSE);
    assert_eq!(kept(cx).get(&lane()), Some("fix the title"));
}

#[gpui::test]
fn flush_keeps_a_waiting_draft_at_once(cx: &mut TestAppContext) {
    let keeper = open(cx, Preferences::default());
    keeper.update(cx, |keeper, cx| {
        keeper.note_for(&Page::Master, None, "", "ask", cx)
    });
    keeper.update(cx, |keeper, cx| keeper.flush(cx));
    assert_eq!(kept(cx).get(&Page::Master), Some("ask"));
}

#[gpui::test]
fn note_of_an_empty_text_forgets_the_draft(cx: &mut TestAppContext) {
    let keeper = open(cx, Preferences::default());
    keeper.update(cx, |keeper, cx| {
        keeper.note_for(&Page::Master, None, "", "ask", cx)
    });
    keeper.update(cx, |keeper, cx| keeper.flush(cx));
    keeper.update(cx, |keeper, cx| {
        keeper.note_for(&Page::Master, None, "", "", cx)
    });
    cx.executor().advance_clock(DRAFT_PAUSE * 2);
    assert!(kept(cx).is_empty(), "a sent message leaves no draft");
}

#[gpui::test]
fn new_restores_what_the_preferences_kept(cx: &mut TestAppContext) {
    let mut drafts = Drafts::default();
    drafts.set(&lane(), "left unsent");
    let preferences = Preferences {
        drafts: Some(drafts.to_json()),
        ..Preferences::default()
    };
    let keeper = open(cx, preferences);
    let restored = keeper.read_with(cx, |keeper, _| keeper.restored(&lane(), ""));
    assert_eq!(restored.as_deref(), Some("left unsent"));
    let other = keeper.read_with(cx, |keeper, _| keeper.restored(&Page::Master, ""));
    assert_eq!(other, None);
}

#[gpui::test]
fn forget_lanes_drops_the_drafts_of_lanes_the_engine_no_longer_lists(cx: &mut TestAppContext) {
    let keeper = open(cx, Preferences::default());
    keeper.update(cx, |keeper, cx| {
        keeper.note_for(&lane(), None, "", "old", cx);
        keeper.note_for(&Page::Lane("p/b".to_string()), None, "", "live", cx);
        keeper.note_for(&Page::Master, None, "", "ask", cx);
        keeper.forget_lanes_not_in(&["p/b".to_string()], cx);
        keeper.flush(cx);
    });
    let drafts = kept(cx);
    assert_eq!(drafts.get(&lane()), None);
    assert_eq!(drafts.get(&Page::Lane("p/b".to_string())), Some("live"));
    assert_eq!(drafts.get(&Page::Master), Some("ask"));
}
