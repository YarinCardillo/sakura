//! The page header in a test window on the fixture: what is at its left and at its right, what is absent
//! when there is nothing to show, and where the sentences that left it went.

use super::desk::Desk;
use super::interaction_tests::{click, open_with, wait_for};
use gpui::{Entity, TestAppContext, VisualTestContext};

fn is_on_screen(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

fn open_on<'a>(
    cx: &'a mut TestAppContext,
    row: &'static str,
) -> (&'a mut VisualTestContext, Entity<Desk>) {
    let (cx, desk, _) = open_with(cx, None);
    wait_for(cx, "the rows", |cx| is_on_screen(cx, row));
    click(cx, row);
    wait_for(cx, "the header", |cx| is_on_screen(cx, "header-state"));
    (cx, desk)
}

in_every_language! {
fn prendi_is_absent_when_there_is_nothing_to_take_and_never_shown_disabled(cx: &mut TestAppContext) {
    let (cx, _) = open_on(cx, "row-headings-26");
    assert!(!is_on_screen(cx, "take"), "taken at HEAD: nothing to take");
    assert!(!is_on_screen(cx, "take-hold"));
    assert!(is_on_screen(cx, "task-menu"), "the overflow is still there");
    click(cx, "row-sample-accent");
    assert!(is_on_screen(cx, "take"), "verified commits ahead: Prendi");
    click(cx, "row-sample-folders");
    assert!(is_on_screen(cx, "take-hold"), "unverified work: the hold");
    assert!(!is_on_screen(cx, "take"));
}
}

in_every_language! {
fn the_state_and_the_switch_are_at_the_left_and_the_actions_at_the_right_in_order_and_the_views_are_the_pane_s(cx: &mut TestAppContext) {
    let (cx, _) = open_on(cx, "row-sample-accent");
    let left = |cx: &mut VisualTestContext, selector: &'static str| {
        cx.debug_bounds(selector).expect(selector).left()
    };
    let order = [
        "header-glyph",
        "header-state",
        "centre-view-agent",
        "header-figures",
        "header-branch",
        "take",
        "task-menu",
    ];
    let places: Vec<_> = order.iter().map(|selector| left(cx, selector)).collect();
    assert!(
        places.windows(2).all(|pair| pair[0] < pair[1]),
        "{places:?}"
    );
    let left_side_end = cx.debug_bounds("centre-views").expect("the switch").right();
    assert!(
        left(cx, "header-figures") - left_side_end > gpui::px(100.),
        "the right side is apart"
    );
    for view in ["pane-view-changes", "pane-view-checks"] {
        let header = cx.debug_bounds("page-header").expect("the header");
        let tab = cx.debug_bounds(view).expect(view);
        assert!(
            tab.top() >= header.bottom(),
            "{view} is in the right pane, not in the header"
        );
    }
}
}

in_every_language! {
fn the_switch_is_in_the_header_with_no_band_under_it_and_the_name_is_the_tab_s(cx: &mut TestAppContext) {
    let (cx, _) = open_on(cx, "row-sample-accent");
    let header = cx.debug_bounds("page-header").expect("the header");
    for part in ["centre-views", "centre-view-agent", "header-branch"] {
        let bounds = cx.debug_bounds(part).expect(part);
        assert!(
            bounds.top() >= header.top() && bounds.bottom() <= header.bottom(),
            "{part} is in the header's row: {bounds:?} in {header:?}"
        );
    }
    let centre = cx.debug_bounds("centre").expect("the centre");
    let conversation = cx.debug_bounds("centre-body").expect("the centre's view");
    assert_eq!(
        (centre.top(), conversation.top()),
        (header.bottom(), header.bottom()),
        "the view starts right under the header: no band between"
    );
    assert!(
        !is_on_screen(cx, "task-name"),
        "the tab says the name: the header does not say it again"
    );
}
}

in_every_language! {
fn a_click_on_the_branch_copies_it(cx: &mut TestAppContext) {
    let (cx, _) = open_on(cx, "row-sample-accent");
    click(cx, "header-branch");
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("box/sample-accent"));
    click(cx, "row-broken-check");
    assert!(
        !is_on_screen(cx, "header-branch"),
        "a task whose branch is not known shows none"
    );
}
}

in_every_language! {
fn the_views_count_the_changed_files_and_the_failed_checks_and_carry_no_glyph(cx: &mut TestAppContext) {
    let (cx, _) = open_on(cx, "row-sample-folders");
    assert!(is_on_screen(cx, "pane-count-changes"), "files changed");
    assert!(
        !is_on_screen(cx, "pane-count-checks"),
        "a check of it runs and none failed: nothing to count"
    );
    assert!(is_on_screen(cx, "header-stop"), "its agent works: Ferma");
    click(cx, "row-headings-26");
    assert!(
        is_on_screen(cx, "pane-count-checks"),
        "its test failed on HEAD: one"
    );
    assert!(
        !is_on_screen(cx, "tab-mark-Verifiche"),
        "the state is said once, beside the name"
    );
    click(cx, "row-broken-check");
    assert!(
        !is_on_screen(cx, "pane-count-changes"),
        "nothing read, no figure"
    );
    assert!(!is_on_screen(cx, "pane-count-checks"));
    assert!(!is_on_screen(cx, "header-figures"));
    assert!(!is_on_screen(cx, "header-stop"));
}
}

in_every_language! {
fn ferma_in_the_header_stops_the_agent_once(cx: &mut TestAppContext) {
    let (cx, _, recorder) = open_with(cx, None);
    wait_for(cx, "the rows", |cx| {
        is_on_screen(cx, "row-sample-folders")
    });
    click(cx, "row-sample-folders");
    click(cx, "header-stop");
    click(cx, "header-stop");
    assert_eq!(recorder.calls(), vec!["stop sample-folders"]);
}
}

in_every_language! {
fn the_header_says_no_sentence_and_the_reasons_are_in_verifiche(cx: &mut TestAppContext) {
    let (cx, _desk) = open_on(cx, "row-sample-folders");
    assert!(!is_on_screen(cx, "take-note") && !is_on_screen(cx, "page-notice"));
    click(cx, "pane-view-checks");
    assert!(
        is_on_screen(cx, "verify-summary"),
        "the branch and the base"
    );
    assert!(
        is_on_screen(cx, "verify-summary-reason"),
        "why taking needs a hold"
    );
}
}

in_every_language! {
fn asking_to_confirm_an_archive_moves_nothing_on_the_page(cx: &mut TestAppContext) {
    let (cx, desk) = open_on(cx, "row-sample-accent");
    let page = |cx: &mut VisualTestContext| {
        ["page-header", "pane-view-checks", "row-sample-accent"]
            .map(|part| cx.debug_bounds(part).expect(part))
    };
    let before = page(cx);
    desk.update(cx, |desk, cx| desk.ask_remove("sample-accent", cx));
    wait_for(cx, "the confirmation", |cx| {
        is_on_screen(cx, "remove-cancel")
    });
    assert!(
        is_on_screen(cx, "remove-hold"),
        "the hold stands where the header's action is, with its reason under the pointer"
    );
    assert!(
        !is_on_screen(cx, "page-notice"),
        "no line is added under the header"
    );
    assert_eq!(page(cx), before, "while the hold is asked");
    click(cx, "remove-cancel");
    wait_for(cx, "the confirmation put away", |cx| {
        !is_on_screen(cx, "remove-cancel")
    });
    assert_eq!(page(cx), before, "and after it");
}
}
