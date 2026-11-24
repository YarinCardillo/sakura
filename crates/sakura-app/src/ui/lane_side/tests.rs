//! The column of a lane's page on GPUI's test platform: Details with the preset and pace controls, Changes
//! with the files and the patch of the one picked, Files that follow the agent, and the toggle that shows and
//! hides the column.

use super::super::agent_rig_tests::{calls_of, open_with, settle, tools_page, EMPTY_PAGE};
use super::super::engine_desk::Desk;
use super::super::engine_rig_tests::{Rig, SNAPSHOT};
use super::super::lane_tests::{bounds, click};
use crate::menus::ToggleChanges;
use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::Value;

/// The fixture with the lane `demo` changed by `change`.
fn with_demo(change: impl FnOnce(&mut Value)) -> String {
    let mut snapshot: Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let items = snapshot["items"].as_array_mut().expect("the items");
    change(
        items
            .iter_mut()
            .find(|item| item["key"][2] == "demo")
            .expect("demo"),
    );
    snapshot.to_string()
}

/// The fixture whose lane `demo` offers the engine's pace and preset controls.
fn with_controls() -> String {
    with_demo(|demo| {
        let actions = demo["actions"].as_array_mut().expect("actions");
        for (key, input) in [("q", "level"), ("S", "preset")] {
            actions.push(serde_json::json!({"key": key, "label": key, "needs": [input], "optional": [], "confirm": false}));
        }
    })
}

type Shown<'a> = (
    Rig,
    Entity<Desk>,
    String,
    &'a mut VisualTestContext,
    tempfile::TempDir,
);

/// The lane `demo` shown with its column, over `snapshot` and the first page of the feed.
fn show<'a>(cx: &'a mut TestAppContext, snapshot: &str, first_page: &str) -> Shown<'a> {
    let repo = tempfile::tempdir().expect("a repository folder");
    let pages = [first_page, EMPTY_PAGE, EMPTY_PAGE];
    let (rig, desk, key, cx) = open_with(cx, repo.path(), snapshot, pages);
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    settle(cx, "the column", |cx| bounds(cx, "lane-side").is_some());
    (rig, desk, key, cx, repo)
}

/// The calls of `sakura cockpit act` the fake engine logged.
fn acts(rig: &Rig) -> Vec<String> {
    let calls = std::fs::read_to_string(rig.dir.path().join("calls.log")).unwrap_or_default();
    let sent = calls.lines().filter(|line| line.starts_with("cockpit act"));
    sent.map(str::to_string).collect()
}

#[gpui::test]
fn details_reads_steps_and_reviews_in_pipeline_order(cx: &mut TestAppContext) {
    let snapshot = with_demo(|demo| {
        demo["reviews"][2]["on"] = true.into();
        let final_review = demo["pipeline"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|step| step["id"] == "final_review")
            .unwrap();
        final_review["state"] = "done".into();
        final_review["on"] = true.into();
    });
    let (_rig, _desk, _key, cx, _repo) = show(cx, &snapshot, EMPTY_PAGE);
    let final_review = bounds(cx, "review-row-final_review").expect("Final review");
    let deliver = bounds(cx, "step-row-deliver").expect("Deliver");
    assert!(
        final_review.top() < deliver.top(),
        "{final_review:?} {deliver:?}"
    );
}

#[gpui::test]
fn details_show_every_field_the_engine_reports_and_hide_the_ones_it_does_not(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, _key, cx, _repo) = show(cx, SNAPSHOT, EMPTY_PAGE);
    for shown in [
        "detail-project",
        "detail-branch",
        "detail-preset",
        "detail-pace",
        "detail-steps",
        "step-row-plan",
        "step-row-build",
        "step-row-checks",
        "step-row-deliver",
        "review-row-plan_review",
        "review-row-task_review",
    ] {
        assert!(bounds(cx, shown).is_some(), "{shown}");
    }
    assert!(
        bounds(cx, "review-row-final_review").is_none(),
        "a review that is off is not listed"
    );
}

#[gpui::test]
fn an_older_engine_that_reports_no_preset_pace_or_steps_leaves_those_rows_out(
    cx: &mut TestAppContext,
) {
    let older = with_demo(|demo| {
        for field in [
            "preset",
            "pace",
            "steps",
            "reviews",
            "pipeline",
            "current_step",
            "in_review",
        ] {
            demo.as_object_mut().expect("an item").remove(field);
        }
    });
    let (_rig, _desk, _key, cx, _repo) = show(cx, &older, EMPTY_PAGE);
    assert!(bounds(cx, "detail-project").is_some());
    for hidden in ["detail-preset", "detail-pace", "detail-steps"] {
        assert!(bounds(cx, hidden).is_none(), "{hidden}");
    }
}

#[test]
fn the_checks_step_says_project_tests_as_the_engine_page_does_and_no_model() {
    use sakura_core::engine::snapshot::LaneStep;
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let step = |id: &str, model: &str| LaneStep {
        id: id.into(),
        name: String::new(),
        state: "next".into(),
        model: Some(model.into()),
        account: Some("work".into()),
        account_label: None,
    };
    let checks = step("checks", "claude:haiku");
    assert_eq!(
        super::details::step_model(&checks).as_deref(),
        Some("Project tests")
    );
    assert_eq!(
        super::details::step_account(&checks, None),
        None,
        "project tests run on no account"
    );
    let build = step("build", "claude:haiku");
    assert_eq!(
        super::details::step_model(&build).as_deref(),
        Some("claude:haiku")
    );
}

#[gpui::test]
fn a_step_names_its_account_only_when_the_engine_gives_one(cx: &mut TestAppContext) {
    let two_accounts = with_demo(|demo| demo["steps"][0]["account"] = "work".into());
    let (_rig, _desk, _key, cx, _repo) = show(cx, &two_accounts, EMPTY_PAGE);
    assert!(bounds(cx, "step-account-plan").is_some());
    assert!(
        bounds(cx, "step-account-build").is_none(),
        "no account named for it"
    );
}

#[gpui::test]
fn with_one_account_no_step_names_an_account(cx: &mut TestAppContext) {
    let (_rig, _desk, _key, cx, _repo) = show(cx, SNAPSHOT, EMPTY_PAGE);
    for step in ["plan", "build", "checks", "deliver"] {
        assert!(
            bounds(cx, &format!("step-account-{step}")).is_none(),
            "{step}"
        );
    }
}

/// Opens the menu of the chip `chip` and chooses its entry `nth` (counted from zero).
fn choose_in_menu(cx: &mut VisualTestContext, chip: &str, nth: usize) {
    click(cx, chip);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    for _ in 0..=nth {
        cx.dispatch_action(gpui_base::actions::SelectDown);
    }
    cx.dispatch_action(gpui_base::actions::Confirm { secondary: false });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn the_pace_menu_offers_the_five_paces_of_the_new_lane_sheet(cx: &mut TestAppContext) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let words: Vec<String> = crate::ui::lane_paces::LANE_PACES
        .iter()
        .map(|(_, key)| crate::l10n::tr(*key).to_string())
        .collect();
    assert_eq!(words, ["Slowest", "Slow", "Normal", "Fast", "Fastest"]);
    let (rig, _desk, key, cx, _repo) = show(cx, &with_controls(), EMPTY_PAGE);
    choose_in_menu(cx, "lane-pace", 0);
    settle(cx, "the call", |_| !acts(&rig).is_empty());
    assert_eq!(
        acts(&rig)[0],
        format!("cockpit act q {key} --level slowest --source app --json"),
        "the slowest is offered too"
    );
}

#[gpui::test]
fn pressing_fast_sends_the_pace_action_with_the_level(cx: &mut TestAppContext) {
    let (rig, _desk, key, cx, _repo) = show(cx, &with_controls(), EMPTY_PAGE);
    choose_in_menu(cx, "lane-pace", 3);
    settle(cx, "the call", |_| !acts(&rig).is_empty());
    assert_eq!(
        acts(&rig)[0],
        format!("cockpit act q {key} --level fast --source app --json")
    );
}

/// What the fake engine answers to every act.
fn answering(rig: &Rig, text: &str) {
    std::fs::write(rig.dir.path().join("first.json"), text).expect("an answer");
}

const SILENT: &str = r#"{"result": "done", "exit_code": 0, "output": ""}"#;
const QUEUED: &str = r#"{"result": "done", "exit_code": 0, "output": "queued: pace for demo; applies at the next safe boundary\n"}"#;
const REFUSED: &str = r#"{"result": "refused", "reason": "pace is not allowed for this lane"}"#;

/// The fixture with the pace controls whose lane `demo` has the pace `pace`, held for its next step or not.
fn with_pace(pace: &str, is_pending: bool) -> String {
    let mut snapshot: Value = serde_json::from_str(&with_controls()).expect("the fixture");
    let items = snapshot["items"].as_array_mut().expect("the items");
    let demo = items.iter_mut().find(|item| item["key"][2] == "demo");
    let demo = demo.expect("demo");
    demo["pace"] = pace.into();
    demo["pace_pending"] = is_pending.into();
    snapshot.to_string()
}

#[gpui::test]
fn a_pace_the_engine_holds_for_the_next_step_says_so_and_one_it_applied_says_nothing(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, _key, cx, _repo) = show(cx, &with_pace("slow", true), EMPTY_PAGE);
    assert!(
        bounds(cx, "detail-pace-queued").is_some(),
        "the engine says it waits"
    );
    let (_rig, _desk, _key, cx, _repo) = show(cx, &with_pace("slow", false), EMPTY_PAGE);
    assert!(
        bounds(cx, "detail-pace-queued").is_none(),
        "applied: no note"
    );
}

#[gpui::test]
fn choosing_a_pace_keeps_no_value_of_its_own_and_says_nothing_the_engine_does_not(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, _key, cx, _repo) = show(cx, &with_controls(), EMPTY_PAGE);
    answering(&rig, QUEUED);
    choose_in_menu(cx, "lane-pace", 3);
    settle(cx, "the call", |_| !acts(&rig).is_empty());
    assert!(
        bounds(cx, "detail-pace-queued").is_none(),
        "the note is the engine's pace_pending, not a local guess"
    );
}

#[gpui::test]
fn a_pace_the_engine_refused_says_why_and_not_that_it_applies_later(cx: &mut TestAppContext) {
    let (rig, _desk, key, cx, _repo) = show(cx, &with_controls(), EMPTY_PAGE);
    answering(&rig, REFUSED);
    choose_in_menu(cx, "lane-pace", 3);
    settle(cx, "the refusal", |cx| {
        bounds(cx, &format!("told-{key}#q")).is_some()
    });
    assert!(bounds(cx, "detail-pace-queued").is_none());
}

#[gpui::test]
fn a_preset_the_engine_only_queued_says_it_applies_at_the_next_step(cx: &mut TestAppContext) {
    let (rig, desk, key, cx, _repo) = show(cx, &with_controls(), EMPTY_PAGE);
    answering(&rig, SILENT);
    let page = desk.read_with(cx, |desk, cx| {
        desk.lane_for_test(&key, cx).expect("the page").clone()
    });
    page.update(cx, |page, cx| page.switch_preset("lean", cx));
    settle(cx, "the line", |cx| {
        bounds(cx, "detail-preset-queued").is_some()
    });
}

#[gpui::test]
fn a_lane_the_engine_gives_no_pace_control_shows_no_pace_menu(cx: &mut TestAppContext) {
    let (_rig, _desk, _key, cx, _repo) = show(cx, SNAPSHOT, EMPTY_PAGE);
    assert!(
        bounds(cx, "lane-pace").is_none(),
        "the engine offers no q action"
    );
}

#[gpui::test]
fn switching_the_preset_sends_the_switch_action_with_the_preset(cx: &mut TestAppContext) {
    let (rig, desk, key, cx, _repo) = show(cx, &with_controls(), EMPTY_PAGE);
    let page = desk.read_with(cx, |desk, cx| {
        desk.lane_for_test(&key, cx).expect("the page").clone()
    });
    page.update(cx, |page, cx| page.switch_preset("lean", cx));
    settle(cx, "the call", |_| !acts(&rig).is_empty());
    assert_eq!(
        acts(&rig)[0],
        format!("cockpit act S {key} --preset lean --source app --json")
    );
}

#[gpui::test]
fn changes_lists_the_files_and_shows_the_patch_of_the_one_clicked(cx: &mut TestAppContext) {
    let (rig, _desk, _key, cx, _repo) = show(cx, SNAPSHOT, EMPTY_PAGE);
    click(cx, "lane-side-view-1");
    settle(cx, "the files", |cx| bounds(cx, "change-file-2").is_some());
    assert!(
        bounds(cx, "change-file-3").is_none(),
        "three files, three rows"
    );
    click(cx, "change-file-1");
    let asked = |rig: &Rig| {
        let calls = std::fs::read_to_string(rig.dir.path().join("calls.log")).unwrap_or_default();
        calls.contains("flow changes --json --file=tests/queue.spec.ts -- demo")
    };
    settle(cx, "the patch of the file clicked", |_| asked(&rig));
    settle(cx, "the patch", |cx| bounds(cx, "change-diff").is_some());
}

#[gpui::test]
fn changes_shows_the_lines_of_an_added_file_whose_patch_has_no_git_headers(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, _key, cx, _repo) = show(cx, SNAPSHOT, EMPTY_PAGE);
    let added = r#"{"task": "demo", "path": "tests/queue.spec.ts", "truncated": false,
     "patch": "--- /dev/null\n+++ b/tests/queue.spec.ts\n+import a from 'a'\n+\n+test('a', () => {})\n"}"#;
    std::fs::write(rig.dir.path().join("patch.json"), added).expect("a patch");
    click(cx, "lane-side-view-1");
    settle(cx, "the files", |cx| bounds(cx, "change-file-1").is_some());
    click(cx, "change-file-1");
    settle(cx, "the added lines", |cx| {
        bounds(cx, "change-line-2").is_some()
    });
    assert!(bounds(cx, "change-diff-unreadable").is_none());
}

#[gpui::test]
fn changes_says_so_when_the_lanes_folder_was_cleaned_up(cx: &mut TestAppContext) {
    let (rig, _desk, _key, cx, _repo) = show(cx, SNAPSHOT, EMPTY_PAGE);
    let gone =
        r#"{"task": "demo", "base": "", "gone": true, "files": [], "added": 0, "removed": 0}"#;
    std::fs::write(rig.dir.path().join("changes.json"), gone).expect("a page");
    click(cx, "lane-side-view-1");
    settle(cx, "the sentence", |cx| {
        bounds(cx, "changes-gone").is_some()
    });
}

/// A real worktree folder with two files, and the fixture whose lane `demo` works there.
fn with_worktree() -> (tempfile::TempDir, String) {
    let worktree = tempfile::tempdir().expect("a worktree");
    for (name, body) in [
        ("a.rs", "fn a() {}\n"),
        ("b.rs", "fn b() {}\n"),
        ("c.rs", "fn c() {}\n"),
    ] {
        std::fs::write(worktree.path().join(name), body).expect("a file");
    }
    let place = worktree.path().display().to_string();
    let snapshot = with_demo(|demo| demo["worktree"] = place.into());
    (worktree, snapshot)
}

fn open_file(desk: &Entity<Desk>, key: &str, cx: &mut VisualTestContext) -> Option<String> {
    desk.read_with(cx, |desk, cx| {
        let page = desk.lane_for_test(key, cx)?;
        page.read(cx).open_file_path(cx)
    })
}

#[gpui::test]
fn the_column_toggle_hides_the_column_and_brings_it_back(cx: &mut TestAppContext) {
    let (_rig, _desk, _key, cx, _repo) = show(cx, SNAPSHOT, EMPTY_PAGE);
    cx.update(|window, cx| window.dispatch_action(Box::new(ToggleChanges), cx));
    settle(cx, "the column hidden", |cx| {
        bounds(cx, "lane-side").is_none()
    });
    cx.update(|window, cx| window.dispatch_action(Box::new(ToggleChanges), cx));
    settle(cx, "the column back", |cx| {
        bounds(cx, "lane-side").is_some()
    });
}

#[gpui::test]
fn the_bottom_rows_toggle_works_on_a_lane_page_and_a_press_hides_the_column(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, _key, cx, _repo) = show(cx, SNAPSHOT, EMPTY_PAGE);
    click(cx, "toggle-column");
    settle(cx, "the column hidden", |cx| {
        bounds(cx, "lane-side").is_none()
    });
}

#[gpui::test]
fn quick_open_is_offered_for_a_lane_with_a_folder_and_a_file_picked_opens_in_files(
    cx: &mut TestAppContext,
) {
    let (_worktree, snapshot) = with_worktree();
    let (_rig, desk, key, cx, _repo) = show(cx, &snapshot, EMPTY_PAGE);
    click(cx, "lane-side-view-2");
    settle(cx, "Files shown", |cx| {
        bounds(cx, "follow-switch").is_some()
    });
    let target = desk.read_with(cx, |desk, cx| desk.quick_open_target(cx));
    let target = target.expect("a target");
    assert_eq!(target.task, key);
    let quick_open = desk.read_with(cx, |desk, _| desk.quick_open.clone());
    quick_open.update(cx, |_, cx| {
        cx.emit(super::super::files::QuickOpenEvent::Picked {
            task: key.clone(),
            tab: target.tab,
            path: "c.rs".to_string(),
        })
    });
    settle(cx, "the picked file", |cx| {
        open_file(&desk, &key, cx).as_deref() == Some("c.rs")
    });
}

#[gpui::test]
fn quick_open_is_not_offered_where_there_is_no_lane_folder_to_search(cx: &mut TestAppContext) {
    let (_rig, desk, _key, cx, _repo) = show(cx, SNAPSHOT, EMPTY_PAGE);
    assert!(
        desk.read_with(cx, |desk, cx| desk.quick_open_target(cx))
            .is_none(),
        "the fixture's worktree is not a folder on this Mac"
    );
}

#[test]
fn a_model_reads_as_its_provider_and_its_name_never_as_the_engines_id() {
    use super::details::model_words;
    assert_eq!(model_words("claude:opus"), "Claude Opus");
    assert_eq!(model_words("claude:haiku"), "Claude Haiku");
    assert_eq!(model_words("codex:gpt-5.1"), "Codex gpt-5.1");
    assert_eq!(model_words("sonnet"), "sonnet");
}

/// The lane `demo` with a model and a very long account on its plan step and a very long branch.
fn with_long_names() -> String {
    with_demo(|demo| {
        demo["steps"][0]["account"] = "an-account-with-a-very-long-name-indeed-yes".into();
        demo["steps"][0]["state"] = "done".into();
        demo["branch"] =
            "feature/a-branch-name-that-is-much-longer-than-the-column-holds-for-sure".into();
    })
}

#[gpui::test]
fn a_step_row_gives_the_model_the_account_and_the_state_each_their_own_room(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, _key, cx, _repo) = show(cx, &with_long_names(), EMPTY_PAGE);
    let model = bounds(cx, "step-model-plan").expect("the model");
    let account = bounds(cx, "step-account-plan").expect("the account");
    let state = bounds(cx, "step-state-plan").expect("the state");
    // The account wraps under the model when it does not fit beside it; it never runs over it.
    let is_beside = model.right() <= account.left();
    let is_below = model.bottom() <= account.top();
    assert!(is_beside || is_below, "{model:?} {account:?}");
    assert!(
        account.right() <= state.left(),
        "the account runs into the state: {account:?} {state:?}"
    );
}

#[gpui::test]
fn a_branch_longer_than_the_column_stays_inside_its_row(cx: &mut TestAppContext) {
    let (_rig, _desk, _key, cx, _repo) = show(cx, &with_long_names(), EMPTY_PAGE);
    let row = bounds(cx, "detail-branch").expect("the row");
    let text = bounds(cx, "detail-branch-text").expect("the branch");
    assert!(text.right() <= row.right(), "{text:?} {row:?}");
    assert!(
        row.size.height > crate::theme::lane_side::PROP_HEIGHT,
        "{text:?} {row:?}"
    );
    assert!(text.bottom() <= row.bottom(), "{text:?} {row:?}");
}

/// The lane `demo` over `snapshot` with the conversation of its builder open, whose first page is `first_page`.
fn show_chat<'a>(cx: &'a mut TestAppContext, snapshot: &str, first_page: &str) -> Shown<'a> {
    let (rig, desk, key, cx, repo) = show(cx, snapshot, first_page);
    settle(cx, "the agents", |cx| bounds(cx, "agent-build-1").is_some());
    click(cx, "agent-build-1");
    settle(cx, "the conversation", |cx| {
        bounds(cx, "agent-chat").is_some()
    });
    (rig, desk, key, cx, repo)
}

#[gpui::test]
fn files_follow_the_agents_file_while_follow_is_on_and_stay_put_when_it_is_off(
    cx: &mut TestAppContext,
) {
    let (_worktree, snapshot) = with_worktree();
    let (rig, desk, key, cx, _repo) = show_chat(cx, &snapshot, &tools_page(&["a.rs"], "c1"));
    click(cx, "lane-side-view-2");
    settle(cx, "the first file", |cx| {
        open_file(&desk, &key, cx).as_deref() == Some("a.rs")
    });
    std::fs::write(
        rig.dir.path().join("page2.json"),
        tools_page(&["b.rs"], "c2"),
    )
    .expect("a page");
    settle(cx, "the agent's next file", |cx| {
        cx.executor()
            .advance_clock(crate::theme::lane_page::AGENT_CHAT_LIVE_TICK);
        open_file(&desk, &key, cx).as_deref() == Some("b.rs")
    });
    click(cx, "follow-switch");
    settle(cx, "follow off", |cx| bounds(cx, "follow-switch").is_some());
    std::fs::write(
        rig.dir.path().join("page2.json"),
        tools_page(&["c.rs"], "c3"),
    )
    .expect("a page");
    let before = calls_of(&rig, "flow feed").len();
    settle(cx, "another read", |cx| {
        cx.executor()
            .advance_clock(crate::theme::lane_page::AGENT_CHAT_LIVE_TICK);
        calls_of(&rig, "flow feed").len() > before + 1
    });
    cx.run_until_parked();
    assert_eq!(open_file(&desk, &key, cx).as_deref(), Some("b.rs"));
}

#[gpui::test]
fn a_path_outside_the_worktree_is_never_opened(cx: &mut TestAppContext) {
    let (_worktree, snapshot) = with_worktree();
    let outside = tools_page(&["../outside.txt", "/etc/hosts"], "c1");
    let (_rig, desk, key, cx, _repo) = show_chat(cx, &snapshot, &outside);
    click(cx, "lane-side-view-2");
    settle(cx, "the rows", |cx| bounds(cx, "chat-tool-1").is_some());
    cx.run_until_parked();
    assert_eq!(open_file(&desk, &key, cx), None);
}

#[gpui::test]
fn the_conversation_and_the_box_under_it_stand_on_one_column_with_the_column_shown_or_hidden(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, _key, cx, _repo) = show_chat(cx, SNAPSHOT, &tools_page(&["a.rs"], "c1"));
    settle(cx, "the box and a row", |cx| {
        bounds(cx, "lane-composer").is_some() && bounds(cx, "chat-column-0").is_some()
    });
    let aligned = |cx: &mut VisualTestContext| {
        let (chat, composer) = (bounds(cx, "chat-column-0"), bounds(cx, "lane-composer"));
        let (chat, composer) = (chat.expect("the chat"), composer.expect("the box"));
        (chat.left(), chat.right()) == (composer.left(), composer.right())
    };
    assert!(aligned(cx), "with the column");
    desk.update(cx, |desk, cx| desk.toggle_column(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(aligned(cx), "without the column");
}

#[gpui::test]
fn polish_s_closed_lane_hides_the_deleted_branch(cx: &mut TestAppContext) {
    let snapshot = with_demo(|demo| {
        demo["agent_state"] = "cleaned".into();
        demo["branch"] = "feat/deleted-branch".into();
        demo["worktree"] = serde_json::Value::Null;
    });
    let (_, _, _, cx, _) = show(cx, &snapshot, EMPTY_PAGE);
    assert!(bounds(cx, "detail-project").is_some());
    assert!(bounds(cx, "detail-branch").is_none());
}
