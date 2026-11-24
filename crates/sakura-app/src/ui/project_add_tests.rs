//! Adding a project as a person meets it: the empty Waiting page asks the Mac's folder panel, the engine
//! registers the folder, and what it refuses is said under the control. The sidebar lists the projects.

use super::engine_rig_tests::{wait_until, SNAPSHOT};
use super::lane_tests::{bounds, click, open_answering};
use gpui::{TestAppContext, VisualTestContext};
use std::path::PathBuf;

const ADDED: &str = r#"{"result": "done", "exit_code": 0, "output": "{}"}"#;
const REFUSED: &str = r#"{"result": "refused", "reason": "the folder is not a Git repository; choose a folder of one"}"#;
const NOTHING: &str = r#"{"api": 1, "waits": [], "items": [], "events": [], "problems": [],
    "projects": []}"#;
const FOLDER: &str = "/work/chosen";

/// A snapshot with a project that has no lane, and the fixture's lanes under the two others.
fn with_empty_project() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    snapshot["projects"].as_array_mut().expect("the projects").push(serde_json::json!({
        "id": r#"["project","/work/empty"]"#, "key": ["project", "/work/empty"], "name": "empty",
        "folder": "/work/empty", "open_lanes": 0,
        "actions": [{"key": "L", "label": "Start lane", "needs": ["text"], "optional": [], "confirm": false}]
    }));
    snapshot.to_string()
}

/// Answers the Mac's folder panel with [`FOLDER`].
fn choose_the_folder(cx: &mut VisualTestContext) {
    assert!(cx.did_prompt_for_paths(), "the Mac's folder panel opened");
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files && !options.multiple);
        Some(vec![PathBuf::from(FOLDER)])
    });
}

#[gpui::test]
fn sidebar_lists_projects_with_nested_lanes(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_answering(cx, &with_empty_project(), ADDED, ADDED);
    let top = |cx: &mut VisualTestContext, selector: &str| {
        bounds(cx, selector)
            .unwrap_or_else(|| panic!("{selector}"))
            .top()
    };
    // Projects in name order, each over its own lanes in name order.
    assert!(top(cx, "group-empty") < top(cx, "group-other"));
    assert!(top(cx, "group-other") < top(cx, "row-fresh"));
    assert!(top(cx, "row-fresh") < top(cx, "row-stuck"));
    assert!(top(cx, "row-stuck") < top(cx, "group-repo"));
    assert!(top(cx, "group-repo") < top(cx, "row-measure-help"));
    assert!(top(cx, "row-measure-help") < top(cx, "row-demo"));
    assert!(top(cx, "row-demo") < top(cx, "row-gated"));
}

#[gpui::test]
fn sidebar_project_without_lanes_shows_new_lane(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_answering(cx, &with_empty_project(), ADDED, ADDED);
    assert!(bounds(cx, "group-empty").is_some());
    assert!(
        bounds(cx, "new-lane-empty").is_some(),
        "a project with no lane still starts one"
    );
    let (name, control) = (
        bounds(cx, "group-empty").expect("the name"),
        bounds(cx, "new-lane-empty").expect("the control"),
    );
    assert!(name.left() < control.left());
    assert!(
        control.bottom() <= bounds(cx, "group-other").expect("the next project").top(),
        "no lane row stands between"
    );
}

#[gpui::test]
fn empty_engine_offers_add_project_on_waiting_page(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_answering(cx, NOTHING, ADDED, ADDED);
    wait_until(cx, "the empty page", |cx| {
        cx.debug_bounds("waiting-add-project").is_some()
    });
    click(cx, "waiting-add-project");
    choose_the_folder(cx);
    wait_until(cx, "the engine's call", |_| !rig.acts().is_empty());
    assert_eq!(
        rig.acts(),
        [format!(
            "cockpit act add-project {FOLDER} --source app --json"
        )]
    );
}

#[gpui::test]
fn a_page_with_a_project_does_not_ask_for_one(cx: &mut TestAppContext) {
    let nothing_waits = r#"{"api": 1, "waits": [], "items": [], "events": [], "problems": [],
        "projects": [{"id": "[\"project\",\"/work/empty\"]", "key": ["project", "/work/empty"],
                      "name": "empty", "folder": "/work/empty", "open_lanes": 0, "actions": []}]}"#;
    let (_rig, _desk, cx) = open_answering(cx, nothing_waits, ADDED, ADDED);
    wait_until(cx, "the sidebar", |cx| {
        cx.debug_bounds("group-empty").is_some()
    });
    assert!(cx.debug_bounds("waiting-add-project").is_none());
}

#[gpui::test]
fn add_project_refusal_shown_in_plain_words(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_answering(cx, NOTHING, REFUSED, REFUSED);
    wait_until(cx, "the empty page", |cx| {
        cx.debug_bounds("waiting-add-project").is_some()
    });
    click(cx, "waiting-add-project");
    choose_the_folder(cx);
    wait_until(cx, "the refusal", |cx| {
        cx.debug_bounds("waiting-add-project-refusal").is_some()
    });
    let (button, refusal) = (
        bounds(cx, "waiting-add-project").expect("the button"),
        bounds(cx, "waiting-add-project-refusal").expect("the words"),
    );
    assert!(button.bottom() <= refusal.top(), "under the button");
}

#[gpui::test]
fn the_empty_sidebar_says_what_a_project_is_and_offers_adding_one(cx: &mut TestAppContext) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (_rig, _desk, cx) = open_answering(cx, NOTHING, ADDED, ADDED);
    wait_until(cx, "the sidebar", |cx| {
        cx.debug_bounds("sidebar-no-projects").is_some()
    });
    assert!(
        bounds(cx, "sidebar-add-project-empty").is_some(),
        "the way in"
    );
    let line = bounds(cx, "sidebar-no-projects").expect("the line");
    let button = bounds(cx, "sidebar-add-project-empty").expect("the button");
    assert!(line.bottom() <= button.top(), "the line, then the button");
}

#[gpui::test]
fn the_sidebar_header_adds_a_project_and_a_folder_that_is_not_git_shows_the_reason(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = open_answering(cx, &with_empty_project(), REFUSED, REFUSED);
    wait_until(cx, "the header", |cx| {
        cx.debug_bounds("sidebar-add-project").is_some()
    });
    click(cx, "sidebar-add-project");
    choose_the_folder(cx);
    wait_until(cx, "the reason", |cx| {
        cx.debug_bounds("sidebar-add-project-refusal").is_some()
    });
    assert_eq!(
        rig.acts(),
        [format!(
            "cockpit act add-project {FOLDER} --source app --json"
        )]
    );
}

#[gpui::test]
fn a_folder_that_is_not_git_becomes_a_repository_only_when_the_person_asks(
    cx: &mut TestAppContext,
) {
    let folder = tempfile::tempdir().expect("a folder");
    let chosen = folder.path().to_path_buf();
    let (rig, _desk, cx) = open_answering(cx, NOTHING, REFUSED, REFUSED);
    wait_until(cx, "the empty sidebar", |cx| {
        cx.debug_bounds("sidebar-add-project-empty").is_some()
    });
    click(cx, "sidebar-add-project-empty");
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(move |_| Some(vec![chosen.clone()]));
    wait_until(cx, "the offer", |cx| {
        cx.debug_bounds("sidebar-init-project").is_some()
    });
    assert!(
        !folder.path().join(".git").exists(),
        "nothing is made unasked"
    );
    click(cx, "sidebar-init-project");
    wait_until(cx, "the second try", |_| rig.acts().len() == 2);
    assert!(folder.path().join(".git").exists(), "made when asked");
}

#[gpui::test]
fn a_project_row_opens_its_page_with_its_folder_its_lanes_and_new_lane(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_answering(cx, &with_empty_project(), ADDED, ADDED);
    wait_until(cx, "the sidebar", |cx| {
        cx.debug_bounds("group-repo").is_some()
    });
    click(cx, "group-repo");
    wait_until(cx, "the project page", |cx| {
        cx.debug_bounds("project-page").is_some()
    });
    for part in [
        "project-page-folder",
        "project-page-branch",
        "project-page-lanes",
        "project-page-new-lane",
    ] {
        assert!(bounds(cx, part).is_some(), "{part}");
    }
    assert!(bounds(cx, "project-page-folder-value").is_none());
    click(cx, "project-page-folder-reveal");
    assert!(bounds(cx, "project-page-folder-value").is_some());
}

#[gpui::test]
fn a_refusal_starts_with_a_capital_and_shows_in_the_one_place_the_person_asked_from(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open_answering(cx, NOTHING, REFUSED, REFUSED);
    wait_until(cx, "the empty page", |cx| {
        cx.debug_bounds("waiting-add-project").is_some()
    });
    click(cx, "waiting-add-project");
    choose_the_folder(cx);
    wait_until(cx, "the refusal", |cx| {
        cx.debug_bounds("waiting-add-project-refusal").is_some()
    });
    assert!(
        bounds(cx, "sidebar-add-project-refusal").is_none(),
        "the sidebar does not say it again"
    );
    assert_eq!(
        super::project_add::sentence("the folder is not a Git repository; choose a folder of one"),
        "The folder is not a Git repository; choose a folder of one"
    );
}

#[gpui::test]
fn polish_zd_project_content_aligns_with_the_header(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = super::engine_desk_tests::open(cx, super::engine_rig_tests::SNAPSHOT);
    desk.update(cx, |desk, cx| {
        desk.navigate(
            sakura_core::tabs::Page::Project("/work/repo".into()),
            false,
            cx,
        )
    });
    wait_until(cx, "the project", |cx| {
        bounds(cx, "project-page-folder").is_some()
    });
    let page = bounds(cx, "page-header").unwrap();
    let title = bounds(cx, "crumb-name").unwrap();
    let folder = bounds(cx, "project-page-folder").unwrap();
    assert_eq!(folder.left(), title.left());
    assert!(folder.top() - page.bottom() < crate::theme::GAP_WIDE * 2.);
}
