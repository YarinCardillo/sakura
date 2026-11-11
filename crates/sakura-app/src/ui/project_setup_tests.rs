//! The first launch, in a test window on a fake `box` with no lavoro and no project: the page asks for a
//! folder through the Mac's picker, shows what `box` recognised in it, and adds the project as confirmed;
//! what `box` refuses is said in the interface's words, with its own line under "Details".

use super::desk::Desk;
use super::interaction_tests::{click, wait_for};
use super::launch_tests::{calls, open_live};
use crate::data::projects::Listed;
use gpui::{Entity, TestAppContext, VisualTestContext};
use std::path::PathBuf;

const FOLDER: &str = "/home/someone/demo-site";
const DETECTED: &str = r#"{"repo":"/home/someone/demo-site","subdir":"","name":"demo-site","currentBranch":"main","defaultBranch":"main","branches":["main","dev"],"install":{"command":"npm ci","reason":"package-lock.json is in the folder"},"checks":[{"verb":"test","command":"npm test","reason":"package.json has a script named test"},{"verb":"lint","command":"npm run lint","reason":"package.json has a script named lint"}],"protected":[],"testFiles":[],"notes":[]}"#;

/// A `box` with no lavoro whose `projects` lists what `project add` added, and whose `project detect`
/// does `detect` (a shell command).
pub(super) fn empty_box(detect: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("sakura-first")
        .tempdir()
        .expect("a temporary directory");
    let body = format!(
        r#"#!/bin/sh
d="$(dirname "$0")"
echo "$@" >> "$d/calls"
trap 'echo "$1" >> "$d/ended"' EXIT
case "$1" in
  project) case "$2" in
    detect) {detect} ;;
    add) echo '[{{"name":"demo-site","base":"main","where":"mac","account":"personal"}}]' > "$d/projects.json" ;;
  esac ;;
  projects) cat "$d/projects.json" ;;
  accounts) echo '[{{"name":"personal","where":"mac","exists":true,"loggedIn":true}}]' ;;
  status) cat "$d/status.json" ;;
  *) echo '[]' ;;
esac
"#
    );
    std::fs::write(dir.path().join("status.json"), no_lavoro()).expect("the status");
    std::fs::write(dir.path().join("detected.json"), DETECTED).expect("the detection");
    std::fs::write(dir.path().join("projects.json"), "[]").expect("the projects");
    std::fs::write(dir.path().join("ended"), "").expect("the list of calls that ended");
    let script = dir.path().join("box");
    sakura_core::test_support::write_script(&script, &body);
    (dir, script)
}

/// The sample status with no lavoro in it.
fn no_lavoro() -> String {
    let fixture = include_str!("../../../sakura-core/tests/fixtures/status.json");
    let mut status: serde_json::Value = serde_json::from_str(fixture).expect("the fixture");
    status["tasks"] = serde_json::Value::Array(Vec::new());
    status.to_string()
}

fn is_on_screen(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

/// The first page of a window with no lavoro and no project.
fn open(cx: &mut TestAppContext, script: PathBuf) -> (&mut VisualTestContext, Entity<Desk>) {
    let (cx, desk, _) = open_live(cx, script, None, false);
    wait_for(cx, "the page that asks for a folder", |cx| {
        is_on_screen(cx, "project-choose")
    });
    (cx, desk)
}

/// Presses "Choose a folder" and answers the Mac's picker with [`FOLDER`].
fn choose_the_folder(cx: &mut VisualTestContext) {
    click(cx, "project-choose");
    assert!(cx.did_prompt_for_paths(), "the Mac's folder picker opened");
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files && !options.multiple);
        Some(vec![PathBuf::from(FOLDER)])
    });
}

#[gpui::test]
fn the_first_launch_asks_for_a_folder_and_adds_the_project_recognised_in_it(
    cx: &mut TestAppContext,
) {
    let (dir, script) = empty_box(r#"cat "$d/detected.json""#);
    let (cx, desk) = open(cx, script);
    assert!(
        !is_on_screen(cx, "no-task"),
        "one sentence and one button, nothing else"
    );
    assert!(
        !is_on_screen(cx, "project-cancel"),
        "there is nothing to go back to"
    );

    choose_the_folder(cx);
    wait_for(cx, "what was recognised", |cx| {
        is_on_screen(cx, "project-review")
    });
    assert!(calls(&dir).contains(&format!("project detect {FOLDER} --json")));
    let recognised = desk.read_with(cx, |desk, cx| {
        let detected = desk.project_setup().read(cx).detected().cloned();
        detected.expect("what was recognised")
    });
    assert_eq!(recognised.checks.len(), 2);
    assert!(is_on_screen(cx, "project-name") && is_on_screen(cx, "project-add"));

    // The second check is dropped, the rest confirmed as proposed.
    let setup = desk.read_with(cx, |desk, _| desk.project_setup().clone());
    setup.update(cx, |setup, cx| setup.keep_check(1, false, cx));
    click(cx, "project-add");
    wait_for(cx, "the project to be listed", |cx| {
        desk.read_with(cx, |desk, cx| !desk.projects.read(cx).usable().is_empty())
    });
    let added = calls(&dir)
        .into_iter()
        .find(|call| call.starts_with("project add"))
        .expect("box project add");
    assert_eq!(
        added,
        format!(
            "project add demo-site --repo {FOLDER} --base main --install npm ci --check test=npm test --account personal"
        ),
        "as confirmed, with the person's own login for its lavori on the Mac"
    );
    wait_for(cx, "the page with New task", |cx| {
        is_on_screen(cx, "first-task")
    });
    assert!(!is_on_screen(cx, "project-setup"), "the page is done");
}

#[gpui::test]
fn a_folder_box_refuses_is_said_in_sakuras_words_with_its_line_under_details(
    cx: &mut TestAppContext,
) {
    let refusal = "echo \"box: '$3' is not inside a git repository\" >&2; exit 4";
    let (_dir, script) = empty_box(refusal);
    let (cx, desk) = open(cx, script);
    choose_the_folder(cx);
    wait_for(cx, "the refusal", |cx| is_on_screen(cx, "project-refusal"));
    assert!(!is_on_screen(cx, "project-review"));
    assert!(
        !is_on_screen(cx, "project-details-text"),
        "the raw line waits"
    );
    click(cx, "project-details");
    assert!(is_on_screen(cx, "project-details-text"), "under Details");
    assert!(
        is_on_screen(cx, "project-choose"),
        "another folder can be chosen"
    );
    let listed = desk.read_with(cx, |desk, cx| desk.projects.read(cx).listed().clone());
    assert_eq!(listed, Listed::Loaded(Vec::new()), "and nothing was added");
}

#[gpui::test]
fn what_the_service_notes_about_a_folder_waits_under_details(cx: &mut TestAppContext) {
    let noted = r#"echo '{"repo":"/home/someone/demo-site","subdir":"","name":"demo-site","currentBranch":"main","defaultBranch":"main","branches":["main"],"install":null,"checks":[],"protected":[],"testFiles":[],"notes":["nothing was recognised in this folder: add the commands of the checks by hand"]}'"#;
    let (_dir, script) = empty_box(noted);
    let (cx, _desk) = open(cx, script);
    choose_the_folder(cx);
    wait_for(cx, "what was recognised", |cx| {
        is_on_screen(cx, "project-review")
    });
    assert!(
        is_on_screen(cx, "project-no-checks"),
        "the interface says in its own words that no check was recognised"
    );
    assert!(
        !is_on_screen(cx, "project-notes-text"),
        "the service's own line waits"
    );
    click(cx, "project-notes");
    assert!(is_on_screen(cx, "project-notes-text"), "under Details");
}

#[gpui::test]
fn a_box_older_than_projects_says_so_plainly(cx: &mut TestAppContext) {
    let dir = tempfile::Builder::new()
        .prefix("sakura-first")
        .tempdir()
        .expect("a temporary directory");
    let body = r#"#!/bin/sh
case "$1" in
  projects|project) echo 'usage: box <command>' >&2; exit 2 ;;
  status) cat "$(dirname "$0")/status.json" ;;
  *) echo '[]' ;;
esac
"#;
    std::fs::write(dir.path().join("status.json"), no_lavoro()).expect("the status");
    let script = dir.path().join("box");
    sakura_core::test_support::write_script(&script, body);
    let (cx, desk, _) = open_live(cx, script, None, false);
    wait_for(cx, "the projects to be requested", |cx| {
        desk.read_with(cx, |desk, cx| {
            desk.projects.read(cx).listed() == &Listed::Unsupported
        })
    });
    assert!(is_on_screen(cx, "no-task"), "one sentence says it");
    assert!(!is_on_screen(cx, "project-choose") && !is_on_screen(cx, "first-task"));
    // The brand's mark stands over the sentence, on the page's centre, at its one size.
    let page = cx.debug_bounds("no-task").expect("the page");
    let mark = cx.debug_bounds("brand-mark-empty").expect("the mark");
    let side = super::brand::MarkSize::MEDIUM.box_side();
    assert_eq!((mark.size.width, mark.size.height), (side, side));
    assert!(
        (mark.center().x - page.center().x).abs() <= gpui::px(1.),
        "{mark:?} in {page:?}"
    );
}

/// On the Mac's own text system, in both languages: the page is one column in the middle of the panel,
/// every value of what was recognised starts on one line after the labels, and nothing leaves the
/// column.
#[test]
fn the_question_is_centred_and_what_was_recognised_is_one_column_set_on_one_line() {
    super::real_text::run(|cx| {
        let (_dir, script) = empty_box(r#"cat "$d/detected.json""#);
        let (cx, desk) = open(cx, script);
        let page = cx.debug_bounds("project-setup").expect("the page");
        let ask = cx.debug_bounds("project-choose").expect("the button");
        let column = gpui::px(crate::theme::setup::COLUMN_WIDTH);
        let left = page.left() + (page.size.width - column) / 2.;
        // The question is a Mac empty state: its sentence and its button stand on the page's centre.
        assert!(
            (ask.center().x - page.center().x).abs() <= gpui::px(1.),
            "the button is centred on the page: {ask:?} in {page:?}"
        );
        let setup = desk.read_with(cx, |desk, _| desk.project_setup().clone());
        cx.update(|window, cx| {
            setup.update(cx, |setup, cx| {
                setup.look_at(FOLDER.to_string(), window, cx)
            })
        });
        wait_for(cx, "what was recognised", |cx| {
            is_on_screen(cx, "project-review")
        });
        let review = cx.debug_bounds("project-review").expect("the review");
        assert!(
            (review.left() - left).abs() < gpui::px(0.5),
            "on the same column"
        );
        assert!(
            review.right() <= left + column + gpui::px(0.5),
            "and inside it"
        );
        let name = cx.debug_bounds("project-name").expect("the name");
        let add = cx.debug_bounds("project-add").expect("the button");
        assert_eq!(name.left(), review.left());
        assert_eq!(
            add.left(),
            review.left(),
            "the buttons start where the lines do"
        );
        assert!(add.bottom() <= page.bottom(), "everything is on the page");
        assert_eq!(add.size.height, crate::theme::CONTROL_HEIGHT);
    });
}

/// On the Mac's own text system, in both languages: the brand's mark stands over the question on the
/// page's centre, at its 64 point size with its clear space, and after its one reveal it rests: no timer
/// is left and nothing asks for another frame.
#[test]
fn the_brand_s_mark_stands_over_the_question_and_rests_after_its_reveal() {
    super::real_text::run(|cx| {
        let (_dir, script) = empty_box(r#"cat "$d/detected.json""#);
        let (cx, desk) = open(cx, script);
        let page = cx.debug_bounds("project-setup").expect("the page");
        let mark = cx.debug_bounds("brand-mark-first").expect("the mark");
        let ask = cx.debug_bounds("project-choose").expect("the button");
        let side = super::brand::MarkSize::MEDIUM.box_side();
        assert_eq!((mark.size.width, mark.size.height), (side, side));
        assert!(
            (mark.center().x - page.center().x).abs() <= gpui::px(1.),
            "the mark is on the page's centre: {mark:?} in {page:?}"
        );
        assert!(
            mark.bottom() <= ask.top(),
            "over the question and its button"
        );
        let setup = desk.read_with(cx, |desk, _| desk.project_setup().clone());
        let moving = setup.read_with(cx, |setup, _| {
            setup.mark().expect("made at the first drawing")
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            !moving.read_with(cx, |mark, _| mark.is_ticking()),
            "the reveal ended and left no timer"
        );
        let asked = std::rc::Rc::new(std::cell::Cell::new(0_usize));
        let _watch = cx.update(|_, cx| {
            let asked = asked.clone();
            cx.observe(&moving, move |_, _| asked.set(asked.get() + 1))
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(600));
        cx.run_until_parked();
        assert_eq!(asked.get(), 0, "at rest the mark asks for no further frame");
    });
}
