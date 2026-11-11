//! What the window tells the system and how a lavoro's work is ended, in a test window: the notifications
//! and the Dock's count follow the lavori's state, on GPUI's test platform, which records what is posted
//! to the system and posts nothing.

use super::conversation_tests as live;
use super::desk::Desk;
use super::interaction_tests::{centre, click, draw, open_with, wait_for};
use super::launch_tests::calls;
use super::tabs::haptics::Recorded;
use crate::l10n::{self, Language};
use crate::theme;
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use sakura_core::model::Live;
use std::rc::Rc;
use std::time::Duration;

/// Two lavori of the sample data, neither with an agent at work.
const FIRST: &str = "sample-accent";
const SECOND: &str = "headings-26";

/// The desk on the sample data, in English, its rows on screen.
fn open(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<Desk>) {
    l10n::set_for_test(Language::English);
    cx.update(|cx| cx.set_app_identity("dev.sakura.test", "Sakura"));
    let (cx, desk, _) = open_with(cx, None);
    wait_for(cx, "the rows", |cx| {
        cx.debug_bounds("row-sample-accent").is_some()
    });
    (cx, desk)
}

/// Says what the conversation of `task` says of its agent, as the conversation does.
fn agent_is(cx: &mut VisualTestContext, desk: &Entity<Desk>, task: &str, live: Live) {
    let facts = desk.read_with(cx, |desk, _| desk.facts.clone());
    facts.update(cx, |facts, cx| facts.set_live(task, live, cx));
    cx.run_until_parked();
    draw(cx);
}

/// Says whether the agent of `task` waits for the person.
fn agent_waits(cx: &mut VisualTestContext, desk: &Entity<Desk>, task: &str, waits: bool) {
    let facts = desk.read_with(cx, |desk, _| desk.facts.clone());
    facts.update(cx, |facts, cx| facts.set_waiting(task, waits, cx));
    cx.run_until_parked();
    draw(cx);
}

/// What was posted to the system so far, as (title, body).
fn posted(cx: &mut VisualTestContext) -> Vec<(String, String)> {
    let shown = cx.shown_system_notifications();
    let pair = |note: &gpui::SystemNotification| (note.title.to_string(), note.body.to_string());
    shown.iter().map(pair).collect()
}

fn badge(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> Option<String> {
    desk.read_with(cx, |desk, _| desk.notifying.badge().map(str::to_string))
}

#[gpui::test]
fn nothing_is_told_while_the_window_is_in_front_and_the_badge_still_counts(
    cx: &mut TestAppContext,
) {
    let (cx, desk) = open(cx);
    agent_is(cx, &desk, FIRST, Live::Working);
    agent_waits(cx, &desk, FIRST, true);
    assert!(posted(cx).is_empty(), "the person is looking at it");
    assert_eq!(badge(cx, &desk).as_deref(), Some("1"));
    agent_waits(cx, &desk, FIRST, false);
    assert_eq!(badge(cx, &desk), None, "answered: nothing needs the person");
}

#[gpui::test]
fn behind_another_app_a_lavoro_that_needs_the_person_and_an_agent_that_finished_are_told_once(
    cx: &mut TestAppContext,
) {
    let (cx, desk) = open(cx);
    agent_is(cx, &desk, FIRST, Live::Working);
    agent_is(cx, &desk, SECOND, Live::Working);
    cx.deactivate_window();
    draw(cx);
    assert!(posted(cx).is_empty(), "work under way is not news");
    agent_waits(cx, &desk, FIRST, true);
    agent_waits(cx, &desk, FIRST, true);
    assert_eq!(posted(cx), [(FIRST.to_string(), "Needs you.".to_string())]);
    assert_eq!(badge(cx, &desk).as_deref(), Some("1"));
    agent_is(cx, &desk, SECOND, Live::Answered);
    assert_eq!(
        posted(cx).last(),
        Some(&(SECOND.to_string(), "The agent finished.".to_string()))
    );
    assert_eq!(posted(cx).len(), 2);
    // Answered, at work again, then the same need a second time: that is news again.
    agent_waits(cx, &desk, FIRST, false);
    agent_waits(cx, &desk, FIRST, true);
    assert_eq!(posted(cx).len(), 3, "a second question is told");
}

#[gpui::test]
fn news_the_person_muted_is_not_told_and_the_badge_still_counts(cx: &mut TestAppContext) {
    let (cx, desk) = open(cx);
    cx.update(|_, cx| {
        crate::prefs::Preferences::change(cx, |preferences| {
            preferences.is_needs_person_muted = true;
        });
    });
    agent_is(cx, &desk, FIRST, Live::Working);
    cx.deactivate_window();
    draw(cx);
    agent_waits(cx, &desk, FIRST, true);
    assert!(posted(cx).is_empty());
    assert_eq!(badge(cx, &desk).as_deref(), Some("1"));
}

/// The request the fake agent answers with a pull request, and the pull request it names.
const FAST: &str = "--delay-ms 2";
/// What the service lists when the person has one login on this Mac.
const MAC_LOGIN: &str = r#"[{"name":"mio","where":"mac","exists":true,"loggedIn":null}]"#;
const PULL_REQUEST: &str = "https://github.com/acme/desk/pull/12";
/// The fixture's lavoro in a box whose head was taken.
const TAKEN: &str = "headings-26";

fn is_on_screen(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

#[gpui::test]
fn publish_asks_the_lavoros_own_agent_and_the_pull_request_it_names_appears_in_the_header(
    cx: &mut TestAppContext,
) {
    l10n::set_for_test(Language::English);
    // A lavoro on this Mac, whose agent runs on the person's one login here.
    let acp = format!("exec \"{}\" {FAST}", live::fake_agent().display());
    let (dir, script) = live::box_with(&acp, MAC_LOGIN);
    live::move_to_the_mac(dir.path(), FIRST);
    let (cx, desk) = live::open(cx, script, FIRST);
    assert!(is_on_screen(cx, "header-publish"));
    assert!(
        !is_on_screen(cx, "header-pull-request"),
        "nothing published"
    );
    let conversation = live::conversation(cx, &desk, FIRST);
    let write = |cx: &mut VisualTestContext, text: &'static str| {
        cx.update(|window, cx| {
            conversation.update(cx, |conversation, cx| conversation.write(text, window, cx))
        });
    };
    // The person is in the middle of writing something else: the request goes as its own message.
    write(cx, "half a thought");
    click(cx, "header-publish");
    assert!(
        desk.read_with(cx, |desk, _| desk.is_publishing(FIRST)),
        "a draft in the composer does not hold Publish back"
    );
    let draft = conversation.read_with(cx, |conversation, cx| conversation.draft(cx));
    assert_eq!(
        draft, "half a thought",
        "what the person is writing is neither sent with the request nor thrown away"
    );
    live::wait_for(cx, "the pull request's link", |cx| {
        is_on_screen(cx, "header-pull-request")
    });
    let link = desk.read_with(cx, |desk, _| desk.pull_request_of(FIRST));
    assert_eq!(
        link.as_deref(),
        Some(PULL_REQUEST),
        "without what followed it"
    );
    assert!(!desk.read_with(cx, |desk, _| desk.is_publishing(FIRST)));
    let draft = conversation.read_with(cx, |conversation, cx| conversation.draft(cx));
    assert_eq!(
        draft, "half a thought",
        "the request went and the draft is still in the composer"
    );
    let said = conversation.read_with(cx, |conversation, _| {
        let entries = conversation.thread().entries();
        format!("{entries:?}")
    });
    assert!(
        !said.contains("half a thought"),
        "the draft was never part of a message"
    );
    assert_eq!(
        live::asked(&dir, &format!("acp {FIRST} mio")),
        1,
        "its own agent, on the person's login"
    );
    let ended = |call: &String| call.starts_with("rm ") || call.starts_with("new ");
    assert!(!calls(&dir).iter().any(ended), "publishing closes nothing");
    assert!(is_on_screen(cx, "row-sample-accent"));
}

#[gpui::test]
fn after_take_publish_starts_a_lavoro_on_this_mac_from_the_branch_taken(cx: &mut TestAppContext) {
    l10n::set_for_test(Language::English);
    let (dir, script) = live::agent_box(FAST);
    let (cx, desk) = live::open(cx, script, FIRST);
    assert!(
        !is_on_screen(cx, "header-publish"),
        "in a box, nothing is published before it is taken"
    );
    click(cx, "row-headings-26");
    live::wait_for(cx, "the taken lavoro's header", |cx| {
        is_on_screen(cx, "header-publish")
    });

    click(cx, "header-publish");
    live::wait_for(cx, "the lavoro on this Mac", |_| {
        calls(&dir).iter().any(|call| call.starts_with("new "))
    });
    let made = calls(&dir)
        .into_iter()
        .find(|call| call.starts_with("new "));
    let made = made.expect("a lavoro was requested");
    assert!(
        made.ends_with("example-app --base box/headings-26 --where mac"),
        "from the branch taken, on this Mac: {made}"
    );
    assert!(!calls(&dir).iter().any(|call| call.starts_with("rm ")));
    assert!(!desk.read_with(cx, |desk, _| desk.is_publishing(TAKEN)));
    assert!(
        is_on_screen(cx, "row-headings-26"),
        "the lavoro in the box stays"
    );
}

/// The overflow's last item ends the lavoro, and is called what it does: the removal itself, its hold
/// when work would be lost and its refusal are the ones the removal's own tests cover.
#[test]
fn what_ends_a_lavoro_is_called_archive_in_both_languages() {
    let words = [
        (
            Language::English,
            ["Archive", "Archive anyway", "Archiving"],
        ),
        (
            Language::Italian,
            ["Archivia", "Archivia comunque", "Archivio"],
        ),
    ];
    for (language, [item, anyway, running]) in words {
        l10n::set_for_test(language);
        assert_eq!(l10n::tr(l10n::Key("task_menu.delete")), item);
        assert_eq!(l10n::tr(l10n::Key("remove.anyway")), anyway);
        assert_eq!(l10n::tr(l10n::Key("remove.removing")), running);
    }
}

#[gpui::test]
fn the_hold_that_takes_taps_the_trackpad_when_it_completes_and_a_click_never_does(
    cx: &mut TestAppContext,
) {
    let (cx, desk) = open(cx);
    let taps = Rc::new(Recorded::default());
    desk.update(cx, |desk, _| desk.haptics = taps.clone());
    click(cx, "row-sample-folders");
    click(cx, "hold-sample-folders");
    cx.executor().advance_clock(theme::HOLD * 2);
    cx.run_until_parked();
    assert!(taps.taps().is_empty(), "an ordinary click taps nothing");

    let at = centre(cx, "hold-sample-folders");
    cx.simulate_mouse_move(at, None, Modifiers::none());
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    cx.executor()
        .advance_clock(theme::HOLD + Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(
        taps.taps(),
        ["alignment"],
        "once, when the hold came to its end"
    );
}
