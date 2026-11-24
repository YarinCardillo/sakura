//! Engine waits drive desktop notifications and the Dock badge.

use super::engine_desk::Desk;
use super::notify::{self, Notifications, Wanted};
use crate::prefs::Preferences;
use gpui::{App, Context, WeakEntity, Window};
use sakura_core::engine::snapshot::Wait;
use sakura_core::notify::{Lavoro, Stage};
use sakura_core::tabs::Page;

/// The notifications of one window.
pub(super) struct Notifying {
    layer: Notifications,
    /// A launch that takes pictures or must not take the person's attention tells the system nothing.
    is_silent: bool,
}

impl Notifying {
    /// Over the Dock's badge; silent for a snapshot and for a launch that must not activate.
    pub(super) fn new() -> Self {
        Self {
            layer: Notifications::new(notify::dock_badge()),
            is_silent: !crate::window::can_activate(),
        }
    }
}

pub(super) fn wait_title(wait: &Wait) -> String {
    format!(
        "{}/{}: {}",
        wait.project,
        wait.lane_name(),
        super::inbox::model::ask_title_words(wait)
    )
}

fn observe_waits(layer: &mut Notifications, waits: &[Wait], now: i64, is_in_front: bool, cx: &App) {
    let lavori: Vec<_> = waits
        .iter()
        .filter(|wait| !wait.is_snoozed_at(now))
        .map(|wait| Lavoro {
            name: wait.id.clone(),
            stage: Stage::NeedsPerson,
            fact: String::new(),
        })
        .collect();
    let title = |id: &str| {
        waits
            .iter()
            .find(|wait| wait.id == id)
            .map_or_else(|| id.to_string(), wait_title)
    };
    layer.observe_with_titles(&lavori, is_in_front, title, cx);
}

impl Desk {
    /// Tells the notifications what waits now and whether Sakura is in front.
    pub(super) fn tell_notifications(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.facts.read(cx).snapshot() else {
            return;
        };
        let preferences = Preferences::try_current(cx).unwrap_or_default();
        let wanted = Wanted {
            needs_person: !preferences.is_needs_person_muted && !self.notifying.is_silent,
            finished: false,
        };
        self.notifying.layer.want(wanted);
        let is_in_front = cx.active_window().is_some();
        observe_waits(
            &mut self.notifying.layer,
            &snapshot.waits,
            super::unix_now(),
            is_in_front,
            cx,
        );
    }

    /// A press on a notification brings the window forward on the Waiting page.
    pub(super) fn answer_notifications(window: &Window, cx: &mut Context<Self>) {
        let (desk, handle) = (cx.weak_entity(), window.window_handle());
        cx.on_system_notification_response(move |response, cx| {
            if notify::task_of(&response.tag).is_none() {
                return;
            }
            show_waiting(&desk, cx);
            crate::window::activate_app(cx);
            if let Err(error) = handle.update(cx, |_, window, _| crate::window::activate(window)) {
                eprintln!("sakura: the window of the notification is gone: {error}");
            }
        });
    }
}

/// Shows the Inbox in the desk.
fn show_waiting(desk: &WeakEntity<Desk>, cx: &mut App) {
    if desk
        .update(cx, |desk, cx| desk.navigate(Page::Inbox, false, cx))
        .is_err()
    {
        eprintln!("sakura: the window of the notification is gone");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l10n::{self, Language};
    use gpui::TestAppContext;
    use sakura_core::engine::snapshot::Snapshot;
    use std::cell::RefCell;
    use std::rc::Rc;

    const FIXTURE: &str = include_str!("../../../sakura-core/tests/fixtures/engine-snapshot.json");

    #[derive(Default)]
    struct Noted(RefCell<Vec<Option<String>>>);

    impl notify::Badge for Noted {
        fn set(&self, label: Option<&str>) {
            self.0.borrow_mut().push(label.map(str::to_string));
        }
    }

    /// The waits of the engine's own lanes in the fixture.
    fn waits() -> Vec<Wait> {
        let all = Snapshot::parse(FIXTURE.as_bytes())
            .expect("the fixture")
            .waits;
        all.into_iter().filter(|wait| !wait.is_outside()).collect()
    }

    fn observe(
        layer: &mut Notifications,
        waits: &[Wait],
        is_in_front: bool,
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| observe_waits(layer, waits, super::super::unix_now(), is_in_front, cx));
    }

    #[gpui::test]
    fn new_wait_notifies_once_when_app_not_in_front(cx: &mut TestAppContext) {
        l10n::set_for_test(Language::English);
        cx.update(|cx| cx.set_app_identity("dev.sakura.test", "Sakura"));
        let badge = Rc::new(Noted::default());
        let mut layer = Notifications::new(badge.clone());
        let mut all = waits();
        let last = all.pop().expect("a wait");
        observe(&mut layer, &all, false, cx);
        assert!(
            cx.shown_system_notifications().is_empty(),
            "what waits at launch is not news"
        );
        all.push(last);
        observe(&mut layer, &all, false, cx);
        observe(&mut layer, &all, false, cx);
        let shown = cx.shown_system_notifications();
        assert_eq!(shown.len(), 1, "a new wait is told once");
        assert_eq!(shown[0].title.as_ref(), "repo/demo: Check row R1");
        assert_eq!(shown[0].body.as_ref(), "Needs you.");
        assert_eq!(badge.0.borrow().last(), Some(&Some("3".to_string())));
        observe(&mut layer, &all, true, cx);
        assert_eq!(cx.shown_system_notifications().len(), 1);
    }

    #[gpui::test]
    fn a_snoozed_wait_clears_the_dock_badge_and_notifies_once_when_it_wakes(
        cx: &mut TestAppContext,
    ) {
        l10n::set_for_test(Language::English);
        cx.update(|cx| cx.set_app_identity("dev.sakura.test", "Sakura"));
        let badge = Rc::new(Noted::default());
        let mut layer = Notifications::new(badge.clone());
        let mut document: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        document["waits"].as_array_mut().unwrap().truncate(1);
        let observe = |document: &serde_json::Value,
                       now,
                       layer: &mut Notifications,
                       cx: &mut TestAppContext| {
            let snapshot = Snapshot::parse(document.to_string().as_bytes()).unwrap();
            cx.update(|cx| observe_waits(layer, &snapshot.waits, now, false, cx));
        };
        let wake = sakura_core::engine::moment::iso_seconds("2099-11-22T18:00:00Z").unwrap();
        observe(&document, wake - 1, &mut layer, cx);
        assert_eq!(badge.0.borrow().last(), Some(&Some("1".into())));
        for wakes_early in [false, true] {
            document["waits"][0]["is_snoozed"] = true.into();
            document["waits"][0]["snoozed_until"] = "2099-11-22T18:00:00Z".into();
            let before = cx.shown_system_notifications().len();
            observe(&document, wake - 1, &mut layer, cx);
            assert_eq!(badge.0.borrow().last(), Some(&None));
            assert_eq!(cx.shown_system_notifications().len(), before);
            let now = if wakes_early {
                document["waits"][0]["is_snoozed"] = false.into();
                wake - 1
            } else {
                wake
            };
            observe(&document, now, &mut layer, cx);
            observe(&document, now, &mut layer, cx);
            assert_eq!(cx.shown_system_notifications().len(), before + 1);
            assert_eq!(badge.0.borrow().last(), Some(&Some("1".into())));
        }
    }

    #[gpui::test]
    fn waking_a_wait_with_the_same_title_as_an_active_wait_notifies_once(cx: &mut TestAppContext) {
        l10n::set_for_test(Language::English);
        cx.update(|cx| cx.set_app_identity("dev.sakura.test", "Sakura"));
        let badge = Rc::new(Noted::default());
        let mut layer = Notifications::new(badge.clone());
        let first = waits().remove(0);
        let mut second = first.clone();
        second.id.push_str("-another");
        second.is_snoozed = Some(true);
        observe(&mut layer, &[first.clone(), second.clone()], false, cx);
        second.is_snoozed = Some(false);
        let both = [first, second];
        observe(&mut layer, &both, false, cx);
        observe(&mut layer, &both, false, cx);
        assert_eq!(cx.shown_system_notifications().len(), 1);
        assert_eq!(badge.0.borrow().last(), Some(&Some("2".into())));
    }

    #[gpui::test]
    fn same_wait_in_two_projects_is_two_notifications_and_two_on_the_badge(
        cx: &mut TestAppContext,
    ) {
        l10n::set_for_test(Language::English);
        cx.update(|cx| cx.set_app_identity("dev.sakura.test", "Sakura"));
        let badge = Rc::new(Noted::default());
        let mut layer = Notifications::new(badge.clone());
        let first = waits().remove(0);
        let mut second = first.clone();
        (second.repo, second.project) = ("/work/other".into(), "other".into());
        second.id = format!("{}-other", first.id);
        observe(&mut layer, &[], true, cx);
        observe(&mut layer, &[first, second], false, cx);
        assert_eq!(cx.shown_system_notifications().len(), 2);
        assert_eq!(badge.0.borrow().last(), Some(&Some("2".to_string())));
    }

    #[gpui::test]
    fn nothing_is_told_while_the_app_is_in_front_and_the_badge_still_counts(
        cx: &mut TestAppContext,
    ) {
        l10n::set_for_test(Language::English);
        cx.update(|cx| cx.set_app_identity("dev.sakura.test", "Sakura"));
        let badge = Rc::new(Noted::default());
        let mut layer = Notifications::new(badge.clone());
        let all = waits();
        observe(&mut layer, &all[..1], true, cx);
        observe(&mut layer, &all, true, cx);
        assert!(cx.shown_system_notifications().is_empty());
        assert_eq!(badge.0.borrow().last(), Some(&Some("3".to_string())));
    }
}
