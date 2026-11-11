//! The notifications' layer on GPUI's test platform, which records what is posted to the system,
//! over a badge that records what it was told to show.

use super::{task_of, Badge, Notifications};
use crate::l10n::{self, Language};
use gpui::TestAppContext;
use sakura_core::notify::{Lavoro, Stage};
use std::cell::RefCell;
use std::rc::Rc;

/// A badge that notes every label it is given.
#[derive(Default)]
struct Noted(RefCell<Vec<Option<String>>>);

impl Badge for Noted {
    fn set(&self, label: Option<&str>) {
        self.0.borrow_mut().push(label.map(str::to_string));
    }
}

fn lavoro(name: &str, stage: Stage, fact: &str) -> Lavoro {
    Lavoro {
        name: name.into(),
        stage,
        fact: fact.into(),
    }
}

/// The layer over a noting badge, on an app the system knows by a name.
fn layer(cx: &mut TestAppContext) -> (Notifications, Rc<Noted>) {
    l10n::set_for_test(Language::English);
    cx.update(|cx| cx.set_app_identity("dev.sakura.test", "Sakura"));
    let badge = Rc::new(Noted::default());
    (Notifications::new(badge.clone()), badge)
}

#[gpui::test]
fn what_is_news_is_posted_once_with_the_task_s_name_and_what_happened(cx: &mut TestAppContext) {
    let (mut layer, _) = layer(cx);
    let working = [lavoro("sort-folders", Stage::Working, "")];
    let needs = [lavoro("sort-folders", Stage::NeedsPerson, "permission-1")];
    cx.update(|cx| layer.observe(&working, false, cx));
    assert!(
        cx.shown_system_notifications().is_empty(),
        "nothing is news at launch"
    );
    cx.update(|cx| layer.observe(&needs, false, cx));
    cx.update(|cx| layer.observe(&needs, false, cx));
    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 1, "the same fact is posted once");
    assert_eq!(shown[0].title.as_ref(), "sort-folders");
    assert_eq!(shown[0].body.as_ref(), "Needs you.");
    assert!(shown[0].actions.is_empty());
    assert_eq!(task_of(&shown[0].tag), Some("sort-folders"));
    let done = [lavoro("sort-folders", Stage::Done, "head-1")];
    cx.update(|cx| layer.observe(&done, false, cx));
    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 2);
    assert_eq!(shown[1].body.as_ref(), "The agent finished.");
    assert_eq!(
        shown[1].tag, shown[0].tag,
        "a task's newer notification replaces its older one"
    );
}

#[gpui::test]
fn nothing_is_posted_while_sakura_is_in_front_and_the_badge_still_counts(cx: &mut TestAppContext) {
    let (mut layer, badge) = layer(cx);
    cx.update(|cx| layer.observe(&[lavoro("a", Stage::Working, "")], true, cx));
    cx.update(|cx| layer.observe(&[lavoro("a", Stage::NeedsPerson, "1")], true, cx));
    assert!(cx.shown_system_notifications().is_empty());
    assert_eq!(*badge.0.borrow(), [Some("1".to_string())]);
}

#[gpui::test]
fn the_badge_is_written_only_when_its_count_changes(cx: &mut TestAppContext) {
    let (mut layer, badge) = layer(cx);
    let needing = |count: usize| -> Vec<Lavoro> {
        let names = (0..count).map(|n| format!("t{n}"));
        names
            .map(|name| lavoro(&name, Stage::NeedsPerson, "f"))
            .collect()
    };
    cx.update(|cx| layer.observe(&[], false, cx));
    assert!(
        badge.0.borrow().is_empty(),
        "no badge and none before: nothing is written"
    );
    for count in [2, 2, 2, 120, 0, 0] {
        cx.update(|cx| layer.observe(&needing(count), true, cx));
    }
    let labels = [Some("2".to_string()), Some("99+".to_string()), None];
    assert_eq!(*badge.0.borrow(), labels);
}

#[test]
fn only_a_tag_of_this_layer_names_a_task() {
    assert_eq!(
        task_of("sakura.task.conta-elementi"),
        Some("conta-elementi")
    );
    assert_eq!(task_of("sakura.task."), None);
    assert_eq!(task_of("other.tag"), None);
}
