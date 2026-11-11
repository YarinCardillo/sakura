//! "New task" in a test window: the one sheet the sidebar's plus, Cmd+N and the File menu open, who has
//! the keyboard when it opens, what Enter and Shift+Enter do, what is chosen before the person chooses
//! anything, and that the project used last is the one offered next.

use super::desk::Desk;
use super::focus_tests::rings;
use super::interaction_tests::{click, draw, open_with, wait_for};
use super::launch_tests::{fake_agentbox, open_live, Names, TASK};
use super::new_task::NewTaskSheet;
use crate::data::projects::Listed;
use crate::menus::NewTask;
use crate::prefs::Preferences;
use gpui::{Entity, TestAppContext, VisualTestContext};
use sakura_core::models::Place;

/// The one project of the sample data, and the branch its lavori start from.
const PROJECT: &str = "example-app";
const BASE: &str = "lab/example-app";

fn is_on_screen(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

/// The sheet that is open, once its projects are read.
pub(super) fn sheet(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> Entity<NewTaskSheet> {
    wait_for(cx, "the sheet", |cx| is_on_screen(cx, "new-task-field"));
    wait_for(cx, "the projects", |cx| {
        desk.read_with(cx, |desk, cx| {
            !matches!(desk.projects.read(cx).listed(), Listed::Unknown)
        })
    });
    desk.read_with(cx, |desk, _| {
        let open = desk.new_task.as_ref().expect("the sheet is open");
        open.sheet().clone()
    })
}

/// Opens the sheet from the sidebar's plus, with the pointer.
fn open_with_the_pointer(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> Entity<NewTaskSheet> {
    // The button moves down when the rows being read take their groups: wait for every read to end.
    wait_for(cx, "the sidebar at rest", |cx| {
        let is_reading = desk.read_with(cx, |desk, cx| desk.facts.read(cx).is_reading());
        !is_reading && is_on_screen(cx, "new-task")
    });
    click(cx, "new-task");
    sheet(cx, desk)
}

/// Opens the sheet with Cmd+N.
pub(super) fn open_with_the_key(
    cx: &mut VisualTestContext,
    desk: &Entity<Desk>,
) -> Entity<NewTaskSheet> {
    wait_for(cx, "the sidebar", |cx| is_on_screen(cx, "new-task"));
    cx.simulate_keystrokes("cmd-n");
    sheet(cx, desk)
}

/// A line of text is at most this many times as high as its text: the kit's ratio, with the half point
/// a line box is rounded to.
const LINE_BOX_AT_MOST: f32 = 1.65;

fn has_caret(cx: &mut VisualTestContext, sheet: &Entity<NewTaskSheet>) -> bool {
    cx.update(|window, cx| sheet.read(cx).has_caret(window, cx))
}

fn chosen(cx: &mut VisualTestContext, sheet: &Entity<NewTaskSheet>) -> [Option<String>; 4] {
    sheet.read_with(cx, |sheet, cx| sheet.chosen(cx))
}

#[gpui::test]
fn the_sidebars_plus_opens_the_sheet_and_the_field_waits_for_a_press(cx: &mut TestAppContext) {
    let (cx, desk, _) = open_with(cx, None);
    let sheet = open_with_the_pointer(cx, &desk);
    cx.update(|window, cx| {
        assert!(window.focused(cx).is_some(), "the sheet has the keyboard");
    });
    assert!(!has_caret(cx, &sheet), "the field waits for a press");
    assert_eq!(rings(cx), 0, "and nothing shows a ring");
    click(cx, "new-task-field");
    assert!(has_caret(cx, &sheet), "a press gives it the caret");
    assert_eq!(rings(cx), 0, "with no ring");
}

#[gpui::test]
fn cmd_n_opens_the_sheet_with_the_caret_in_the_field(cx: &mut TestAppContext) {
    let (cx, desk, _) = open_with(cx, None);
    let sheet = open_with_the_key(cx, &desk);
    assert!(has_caret(cx, &sheet), "opened with a key: typing writes");
    cx.simulate_input("wave");
    let written = sheet.read_with(cx, |sheet, cx| sheet.text(cx));
    assert_eq!(written, "wave");
    assert_eq!(rings(cx), 0);
}

#[gpui::test]
fn the_file_menu_opens_the_same_sheet(cx: &mut TestAppContext) {
    let (cx, desk, _) = open_with(cx, None);
    wait_for(cx, "the sidebar", |cx| is_on_screen(cx, "new-task"));
    // The pointer went to the menu bar: the last thing the window heard was the pointer.
    cx.simulate_mouse_move(
        gpui::point(gpui::px(400.), gpui::px(300.)),
        None,
        gpui::Modifiers::none(),
    );
    cx.dispatch_action(NewTask);
    let sheet = sheet(cx, &desk);
    assert!(!has_caret(cx, &sheet), "opened with the pointer");
    assert_eq!(cx.debug_bounds("new-task-sheet").iter().count(), 1);
}

#[gpui::test]
fn enter_asks_for_the_lavoro_once_and_shift_enter_breaks_the_line(cx: &mut TestAppContext) {
    // A project on the Mac with its own login: nothing stops the lavoro but an empty field.
    let (dir, script) = two_projects();
    let (cx, desk, _) = open_live(cx, script, Some(TASK), false);
    let sheet = open_with_the_key(cx, &desk);
    wait_for(cx, "the accounts", |cx| chosen(cx, &sheet)[3].is_some());
    let asked = |dir: &tempfile::TempDir| -> Vec<String> {
        let calls = super::launch_tests::calls(dir).into_iter();
        calls.filter(|call| call.starts_with("new ")).collect()
    };
    cx.simulate_keystrokes("enter");
    assert!(asked(&dir).is_empty(), "no sentence, nothing asked");
    assert!(is_on_screen(cx, "new-task-field"), "and the sheet stays");
    assert!(has_caret(cx, &sheet), "with the caret where it was");
    cx.simulate_input("sort");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("folders");
    let written = sheet.read_with(cx, |sheet, cx| sheet.text(cx));
    assert_eq!(written, "sort\nfolders", "Shift+Enter breaks the line");
    assert!(asked(&dir).is_empty(), "and asks nothing");
    cx.simulate_keystrokes("enter");
    draw(cx);
    wait_for(cx, "box new", |_| !asked(&dir).is_empty());
    wait_for(cx, "the sheet to go", |cx| {
        !is_on_screen(cx, "new-task-field")
    });
    let made = asked(&dir);
    assert_eq!(made.len(), 1, "asked once: {made:?}");
    assert!(
        made[0].starts_with("new sort-folders demo-site"),
        "{made:?}"
    );
    let shown = desk.read_with(cx, |desk, cx| desk.shown(cx));
    assert_eq!(shown.as_deref(), Some("sort-folders"), "its tab is shown");
}

#[gpui::test]
fn what_is_chosen_before_the_person_chooses_is_the_projects_own(cx: &mut TestAppContext) {
    let (cx, desk, _) = open_with(cx, None);
    let sheet = open_with_the_pointer(cx, &desk);
    let [project, base, place, account] = chosen(cx, &sheet);
    assert_eq!(project.as_deref(), Some(PROJECT));
    assert_eq!(base.as_deref(), Some(BASE), "the project's own branch");
    assert_eq!(
        place.as_deref(),
        Some("box"),
        "where the project's lavori live"
    );
    assert_eq!(account, None, "the sample data has no account");
    for selector in [
        "new-task-project",
        "new-task-base",
        "new-task-place",
        "new-task-account",
        "new-task-send",
    ] {
        assert!(is_on_screen(cx, selector), "{selector} is under the field");
    }
}

/// A `box` with two projects, one on the Mac with its account and one in a box, and their branches.
fn two_projects() -> (tempfile::TempDir, std::path::PathBuf) {
    let cases = r#"  projects) echo '[{"name":"demo-site","base":"main","where":"mac","account":"personal"},{"name":"engine-demo","base":"dev","where":"box"}]' ;;
  branches) echo "{\"project\":\"$2\",\"default\":\"main\",\"branches\":[{\"name\":\"main\"},{\"name\":\"dev\"},{\"name\":\"trial\"}]}" ;;
  accounts) echo '[{"name":"personal","where":"mac","exists":true,"loggedIn":true},{"name":"other","where":"mac","exists":true,"loggedIn":true},{"name":"scatola","where":"box","loggedIn":true}]' ;;
  new) echo 'cloning'; sleep 20 ;;"#;
    fake_agentbox(Names::Listed, "0", cases)
}

#[gpui::test]
fn each_choice_is_changed_in_place_and_the_last_project_is_offered_next(cx: &mut TestAppContext) {
    let (dir, script) = two_projects();
    let (cx, desk, _) = open_live(cx, script, Some(TASK), false);
    let sheet = open_with_the_pointer(cx, &desk);
    let read = |cx: &mut VisualTestContext| chosen(cx, &sheet).map(Option::unwrap_or_default);
    wait_for(cx, "the accounts", |cx| read(cx)[3] == "personal");
    assert_eq!(read(cx), ["demo-site", "main", "mac", "personal"]);

    cx.update(|window, cx| {
        sheet.update(cx, |sheet, cx| {
            sheet.choose_project("engine-demo", window, cx)
        })
    });
    assert_eq!(
        read(cx),
        ["engine-demo", "dev", "box", "scatola"],
        "its own"
    );
    cx.update(|window, cx| {
        sheet.update(cx, |sheet, cx| {
            sheet.choose_place(Place::Mac, window, cx);
            sheet.choose_account("other", window, cx);
        })
    });
    // The branch can be chosen once the project's branches are offered: the wait is on what the
    // sheet holds, not on the call that requested them.
    wait_for(cx, "the branch of the project to be offered", |cx| {
        cx.update(|window, cx| {
            sheet.update(cx, |sheet, cx| sheet.choose_base("trial", window, cx))
        });
        read(cx)[1] == "trial"
    });
    assert_eq!(read(cx), ["engine-demo", "trial", "mac", "other"]);

    cx.update(|window, cx| {
        sheet.update(cx, |sheet, cx| {
            sheet.write("sort the folders", window, cx);
            sheet.submit(cx);
        })
    });
    wait_for(cx, "box new", |_| {
        let new = "new sort-folders engine-demo --base trial --where mac";
        super::launch_tests::calls(&dir).contains(&new.to_string())
    });
    let calls = super::launch_tests::calls(&dir);
    let set = "project set engine-demo --account other".to_string();
    let told = calls.iter().position(|call| *call == set);
    let made = calls.iter().position(|call| call.starts_with("new "));
    assert!(
        told < made && told.is_some(),
        "the project is told first: {calls:?}"
    );
    let last = cx.update(|_, cx| Preferences::current(cx).last_project);
    assert_eq!(
        last.as_deref(),
        Some("engine-demo"),
        "the project used last is kept"
    );

    let again = open_with_the_pointer(cx, &desk);
    let offered = chosen(cx, &again)[0].clone();
    assert_eq!(offered.as_deref(), Some("engine-demo"), "and offered next");
}

#[gpui::test]
fn a_box_without_projects_says_so_in_the_sheet(cx: &mut TestAppContext) {
    let cases = "  projects) echo 'usage: box <command>' >&2; exit 2 ;;";
    let (_dir, script) = fake_agentbox(Names::Listed, "0", cases);
    let (cx, desk, _) = open_live(cx, script, Some(TASK), false);
    let sheet = open_with_the_pointer(cx, &desk);
    assert!(is_on_screen(cx, "new-task-note"), "one sentence says why");
    assert!(
        !is_on_screen(cx, "new-task-send"),
        "and nothing can be sent"
    );
    cx.update(|window, cx| {
        sheet.update(cx, |sheet, cx| {
            sheet.write("sort the folders", window, cx);
            sheet.submit(cx);
        })
    });
    let listed = desk.read_with(cx, |desk, cx| desk.projects.read(cx).listed().clone());
    assert_eq!(listed, Listed::Unsupported);
    assert!(is_on_screen(cx, "new-task-field"), "the sheet stays");
}

/// The four things chosen, the field and the send button: what the sheet is made of.
const PARTS: [&str; 6] = [
    "new-task-field",
    "new-task-project",
    "new-task-base",
    "new-task-place",
    "new-task-account",
    "new-task-send",
];

fn parts(cx: &mut VisualTestContext) -> Vec<gpui::Bounds<gpui::Pixels>> {
    let bounds = PARTS.iter().map(|part| cx.debug_bounds(part));
    bounds.map(|bounds| bounds.expect("a part")).collect()
}

/// On the Mac's own text system, in both languages: the sheet stands inside the window at its narrowest
/// too; the four things chosen are packed at the left on one line, each as wide as what it says and the
/// tight gap apart, the send button is at the right; hovering or writing moves nothing.
#[test]
fn the_choices_are_packed_at_the_left_as_wide_as_they_say_and_the_send_button_is_at_the_right() {
    super::real_text::run(|cx| {
        let (_dir, script) = two_projects();
        let (cx, desk, _) = open_live(cx, script, Some(TASK), false);
        cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(700.)));
        let sheet = open_with_the_pointer(cx, &desk);
        wait_for(cx, "the accounts", |cx| chosen(cx, &sheet)[3].is_some());
        let whole = cx.debug_bounds("new-task-sheet").expect("the sheet");
        let before = parts(cx);
        let near = |a: gpui::Pixels, b: gpui::Pixels| (a - b).abs() < gpui::px(0.5);
        let padding = crate::theme::thread::COMPOSER_PADDING;
        let choosers = &before[1..5];
        assert!(
            near(choosers[0].left(), whole.left() + padding),
            "the first starts on the sheet's inset: {choosers:?} in {whole:?}"
        );
        for pair in choosers.windows(2) {
            let between = pair[1].left() - pair[0].right();
            assert!(
                near(between, crate::theme::GAP_TIGHT),
                "the tight gap apart: {between:?}"
            );
            assert!(near(pair[0].top(), pair[1].top()), "on one line");
        }
        let widths: Vec<_> = choosers.iter().map(|chooser| chooser.size.width).collect();
        assert!(
            widths.windows(2).any(|pair| !near(pair[0], pair[1])),
            "each as wide as what it says, not one room for all: {widths:?}"
        );
        assert!(
            near(before[5].right(), whole.right() - padding),
            "the send button ends on the sheet's inset at the right"
        );
        assert!(choosers[3].right() < before[5].left(), "after the choices");
        assert!(whole.left() >= gpui::px(0.) && whole.right() <= gpui::px(1100.));

        cx.simulate_mouse_move(before[2].center(), None, gpui::Modifiers::none());
        draw(cx);
        assert_eq!(parts(cx), before, "the pointer over a choice moves nothing");
        cx.update(|window, cx| sheet.update(cx, |sheet, cx| sheet.write("sort", window, cx)));
        draw(cx);
        assert_eq!(parts(cx), before, "nor a sentence of one line");
    });
}

/// What the sheet opens over steps back, in both appearances: the veil the kit draws over the window
/// is black at the sheet's dim, not the kit's own faint one.
#[gpui::test]
fn what_the_sheet_opens_over_is_dimmed_in_both_appearances(cx: &mut TestAppContext) {
    use crate::theme::{Appearance, Theme, SHEET_DIM};
    cx.update(|cx| {
        gpui_component::init(cx);
        for appearance in [Appearance::Dark, Appearance::Light] {
            Theme::set(appearance, cx);
            let veil = gpui_component::Theme::global(cx).colors.overlay;
            assert_eq!((veil.l, veil.a), (0., SHEET_DIM), "{appearance:?}");
        }
    });
}

/// Down, the sheet stands on the window's middle: where the kit centres it, with no slide into place.
#[test]
fn the_sheet_stands_on_the_window_s_middle_down() {
    super::real_text::run(|cx| {
        let (_dir, script) = two_projects();
        let (cx, desk, _) = open_live(cx, script, Some(TASK), false);
        open_with_the_pointer(cx, &desk);
        let sheet = cx.debug_bounds("new-task-sheet").expect("the sheet");
        let window = cx.update(|window, _| window.viewport_size());
        assert!(
            (sheet.center().y - window.height / 2.).abs() <= gpui::px(1.),
            "down: {:?} in a window {:?} high",
            sheet.center().y,
            window.height
        );
    });
}

/// A `box` whose one project lives in a box, with `accounts` (JSON) as the accounts it knows.
fn a_project_in_a_box(accounts: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let cases = format!(
        r#"  projects) echo '[{{"name":"engine-demo","base":"dev","where":"box"}}]' ;;
  branches) echo '{{"project":"engine-demo","default":"dev","branches":[{{"name":"dev"}}]}}' ;;
  accounts) echo '{accounts}' ;;
  new) echo 'cloning'; sleep 20 ;;"#
    );
    fake_agentbox(Names::Listed, "0", &cases)
}

/// Writes a sentence and presses Enter; says whether `box new` was requested.
fn asks_for_a_lavoro(
    cx: &mut VisualTestContext,
    sheet: &Entity<NewTaskSheet>,
    dir: &tempfile::TempDir,
) -> bool {
    cx.update(|window, cx| {
        sheet.update(cx, |sheet, cx| {
            sheet.write("sort the folders", window, cx);
            sheet.submit(cx);
        })
    });
    cx.run_until_parked();
    let calls = super::launch_tests::calls(dir);
    calls.iter().any(|call| call.starts_with("new "))
}

#[gpui::test]
fn in_a_box_with_no_account_signed_in_none_is_chosen_and_choosing_one_starts_its_sign_in(
    cx: &mut TestAppContext,
) {
    let accounts = r#"[{"name":"scatola","where":"box","loggedIn":false},{"name":"seconda","where":"box","loggedIn":false}]"#;
    let (dir, script) = a_project_in_a_box(accounts);
    // With its terminals: the sign-in is one.
    let (cx, desk, _) = open_live(cx, script, Some(TASK), true);
    let sheet = open_with_the_pointer(cx, &desk);
    wait_for(cx, "the reason", |cx| is_on_screen(cx, "new-task-reason"));
    // The reason stands beside the choices at their size: a line box of the label text, no taller.
    let reason = cx.debug_bounds("new-task-reason").expect("the reason");
    assert!(
        reason.size.height <= crate::theme::TEXT_LABEL * LINE_BOX_AT_MOST,
        "the reason is set larger than the choices beside it: {:?}",
        reason.size.height
    );
    assert_eq!(
        chosen(cx, &sheet)[3],
        None,
        "none is signed in: none chosen"
    );
    assert!(!asks_for_a_lavoro(cx, &sheet, &dir), "nothing is sent");
    assert!(is_on_screen(cx, "new-task-field"), "the sheet stays");
    // One that is not signed in can be chosen (its row says so): the app's sign-in to it starts in
    // the lavoro on screen, and the sheet gives it the window.
    cx.update(|window, cx| {
        sheet.update(cx, |sheet, cx| sheet.choose_account("scatola", window, cx))
    });
    let signs_in = |call: &String| call.starts_with("login scatola");
    wait_for(cx, "the sign-in", |_| {
        super::launch_tests::calls(&dir).iter().any(signs_in)
    });
    draw(cx);
    assert!(!is_on_screen(cx, "new-task-field"), "the sheet is gone");
    let calls = super::launch_tests::calls(&dir);
    assert!(
        !calls.iter().any(|call| call.starts_with("new ")),
        "nothing is sent"
    );
}

#[gpui::test]
fn in_a_box_with_no_account_at_all_the_sheet_says_to_sign_in_and_sends_nothing(
    cx: &mut TestAppContext,
) {
    let (dir, script) = a_project_in_a_box("[]");
    let (cx, desk, _) = open_live(cx, script, Some(TASK), false);
    let sheet = open_with_the_pointer(cx, &desk);
    wait_for(cx, "the reason", |cx| is_on_screen(cx, "new-task-reason"));
    assert!(!asks_for_a_lavoro(cx, &sheet, &dir));
}

/// The one rule "centred in the panel": across, the sheet's centre is the centre of the page the panel
/// shows (its header spans it), not the window's. (Down, the kit places it on the window's middle; a test
/// window does not show that, the snapshots do.)
#[test]
fn the_sheet_stands_at_the_centre_of_the_panel() {
    super::real_text::run(|cx| {
        let (_dir, script) = two_projects();
        let (cx, desk, _) = open_live(cx, script, Some(TASK), false);
        wait_for(cx, "the page", |cx| is_on_screen(cx, "page-header"));
        let page = cx.debug_bounds("page-header").expect("the page's header");
        open_with_the_pointer(cx, &desk);
        let sheet = cx.debug_bounds("new-task-sheet").expect("the sheet");
        let window = cx.update(|window, _| window.viewport_size());
        assert!(
            (sheet.center().x - page.center().x).abs() <= gpui::px(1.),
            "across: {:?}, the page's centre is {:?}",
            sheet.center().x,
            page.center().x
        );
        assert!(
            (sheet.center().x - window.width / 2.).abs() > gpui::px(1.),
            "and that is not the window's centre while the sidebar is shown"
        );
    });
}

/// The same rule for a pane that has only a sentence to show: the sentence is centred in the pane.
#[test]
fn a_pane_with_nothing_to_list_centres_its_sentence_in_the_pane() {
    super::real_text::run(|cx| {
        let (cx, _, _) = open_with(cx, None);
        wait_for(cx, "the rows", |cx| is_on_screen(cx, "row-broken-check"));
        // The task whose worktree cannot be read has no change to list: its pane says so.
        click(cx, "row-broken-check");
        wait_for(cx, "the sentence", |cx| is_on_screen(cx, "pane-sentence"));
        let views = cx
            .debug_bounds("pane-views")
            .expect("the pane's row of views");
        let sentence = cx.debug_bounds("pane-sentence").expect("the sentence");
        assert!(
            (sentence.center().x - views.center().x).abs() <= gpui::px(1.),
            "across: {:?} in a pane centred on {:?}",
            sentence.center().x,
            views.center().x
        );
    });
}

#[gpui::test]
fn one_press_on_another_chooser_closes_the_open_one_and_opens_it(cx: &mut TestAppContext) {
    let (cx, desk, _) = open_with(cx, None);
    let sheet = open_with_the_key(cx, &desk);
    let open = |cx: &mut VisualTestContext| {
        draw(cx);
        sheet.read_with(cx, |sheet, cx| sheet.open_choosers(cx))
    };
    assert_eq!(open(cx), [false; 4]);
    click(cx, "new-task-project");
    assert_eq!(open(cx), [true, false, false, false], "one press opens");
    click(cx, "new-task-place");
    assert_eq!(
        open(cx),
        [false, false, true, false],
        "one press on another chooser: the first closed, this one open"
    );
    click(cx, "new-task-account");
    assert_eq!(open(cx), [false, false, false, true]);
    click(cx, "new-task-account");
    assert_eq!(
        open(cx),
        [false; 4],
        "a press on the open chooser closes it"
    );
}
