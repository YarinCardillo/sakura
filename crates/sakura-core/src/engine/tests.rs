use super::cli::{Act, ActResult, EngineCli, Env, DEADLINE};
use super::snapshot::Snapshot;
use crate::error::ErrorCode;
use crate::said::{Said, Value};
use crate::test_support::write_script;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const FIXTURE: &str = include_str!("../../tests/fixtures/engine-snapshot.json");

/// An engine that is a shell script with `body`, in `dir`.
fn fake_engine(dir: &Path, body: &str) -> EngineCli {
    let path = dir.join("sakura");
    write_script(&path, &format!("#!/bin/sh\n{body}\n"));
    EngineCli::at(path).with_deadline(Duration::from_secs(180))
}

/// An engine that prints `text` and exits with `code`.
fn engine_saying(dir: &Path, text: &str, code: i32) -> EngineCli {
    let answer = dir.join("answer.json");
    std::fs::write(&answer, text).unwrap();
    let body = format!("cat '{}'\nexit {code}", answer.display());
    fake_engine(dir, &body)
}

fn action(key: &str, needs: &[&str], optional: &[&str], confirm: bool) -> super::snapshot::Action {
    super::snapshot::Action {
        key: key.to_string(),
        label: "Label".to_string(),
        needs: needs.iter().map(|name| name.to_string()).collect(),
        optional: optional.iter().map(|name| name.to_string()).collect(),
        confirm,
    }
}

fn act(action: super::snapshot::Action, inputs: &[(&str, &str)]) -> Act {
    Act {
        action,
        target: r#"["wait","/work/repo","gated","approval","spec"]"#.to_string(),
        inputs: inputs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect::<BTreeMap<_, _>>(),
        confirm: None,
    }
}

#[tokio::test]
async fn snapshot_parses_waits_items_and_progress() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), FIXTURE, 0);
    let snapshot = cli.snapshot().await.unwrap();

    let kinds: Vec<(&str, &str)> = snapshot
        .waits
        .iter()
        .map(|wait| (wait.kind.as_str(), wait.task.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("approval", "gated"),
            ("blocked", "stuck"),
            ("owner_row", "demo"),
            ("owner_row", "measure-help"),
            ("question", "measure-help")
        ]
    );
    let approval = &snapshot.waits[0];
    assert_eq!(approval.project, "repo");
    assert_eq!(approval.actions[0].key, "a");
    assert!(approval.actions[0].confirm);

    let demo = snapshot.items.iter().find(|item| item.task() == "demo");
    let demo = demo.expect("the demo lane is listed");
    let progress = demo.progress.as_ref().expect("the lane has progress");
    assert_eq!((progress.done, progress.total), (1, 4));
    assert_eq!(progress.phase, "build");
    assert_eq!(demo.project(), "repo");
    assert_eq!(demo.models["builder"], "claude:sonnet");
    let fresh = snapshot.items.iter().find(|item| item.task() == "fresh");
    let fresh = fresh.expect("the fresh lane is listed");
    assert_eq!(fresh.progress.as_ref().map(|p| p.total), Some(0));
    assert_eq!(snapshot.events.len(), 8);
}

#[test]
fn snapshot_skips_item_kinds_it_does_not_show() {
    let text = r#"{"api": 1, "waits": [], "events": [], "problems": [],
        "items": [
          {"id": "a", "key": ["lane", "g", "n"], "kind": "lane", "repo": "/r"},
          {"id": "[\"flow\",\"/work/repo\",\"one\"]", "key": ["flow", "/work/repo", "one"], "kind": "flow",
           "repo": "/work/repo", "worktree": null, "branch": "", "last_commit": "", "stack": "down",
           "agent_state": "working", "pr": "", "models": {}, "pending": [], "since": null,
           "last_activity_at": null, "next_command": "", "progress": null, "actions": []}
        ]}"#;
    let snapshot = Snapshot::parse(text.as_bytes()).unwrap();
    let tasks: Vec<&str> = snapshot.items.iter().map(|item| item.task()).collect();
    assert_eq!(tasks, ["one"]);
}

/// The engine's own `lane_steps.of_lane` for a lane in its build, as `tests/fixtures/make-engine-real.py` captures it.
const REAL_LANE_STEPS: &str = include_str!("../../tests/fixtures/engine-real-lane-steps.json");

#[test]
fn the_real_engine_s_lane_steps_are_read_with_their_state_and_what_can_be_toggled() {
    #[derive(serde::Deserialize)]
    struct Document {
        pipeline: Vec<super::snapshot::PipelineStep>,
    }
    let read: Document = serde_json::from_str(REAL_LANE_STEPS).expect("the engine's steps");
    let step = |id: &str| read.pipeline.iter().find(|step| step.id == id).expect(id);
    assert_eq!(read.pipeline.len(), 9);
    assert!(step("plan").state == "done" && !step("plan").can_toggle);
    assert!(step("build").state == "running" && !step("build").reason.is_empty());
    assert!(step("task_review").can_toggle && step("task_review").reason.is_empty());
}

#[test]
fn item_reads_the_pipeline_of_every_step_with_its_state_and_whether_it_can_be_toggled() {
    let snapshot = Snapshot::parse(FIXTURE.as_bytes()).expect("the fixture");
    let demo = &snapshot.items[0];
    let ids: Vec<&str> = demo.pipeline.iter().map(|step| step.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "plan",
            "plan_review",
            "build",
            "task_review",
            "final_review",
            "checks",
            "evaluator",
            "deliver",
            "reporter"
        ]
    );
    let final_review = &demo.pipeline[4];
    assert!(!final_review.is_on && final_review.can_toggle && final_review.state == "off");
    let plan = &demo.pipeline[0];
    assert!(!plan.can_toggle && plan.reason.contains("already done"));
    assert!(
        snapshot.items[1].pipeline.is_empty(),
        "an older engine gives none"
    );
}

#[test]
fn item_reads_its_preset_and_steps_and_has_none_from_an_older_engine() {
    let lane = |extra: &str| {
        format!(
            r#"{{"api": 1, "waits": [], "events": [], "problems": [], "items": [
              {{"id": "x", "key": ["flow", "/work/repo", "one"], "kind": "flow", "repo": "/work/repo",
                "agent_state": "working", "since": null, "last_activity_at": null, "progress": null{extra}}}
            ]}}"#
        )
    };
    let with = lane(
        r#", "preset": "careful", "steps": [
            {"id": "plan", "name": "Plan", "state": "done"},
            {"id": "build", "name": "Build", "state": "now"},
            {"id": "checks", "name": "Checks", "state": "next"}]"#,
    );
    let read = Snapshot::parse(with.as_bytes()).unwrap();
    let item = &read.items[0];
    assert_eq!(item.preset.as_deref(), Some("careful"));
    let states: Vec<(&str, &str)> = item
        .steps
        .iter()
        .map(|step| (step.id.as_str(), step.state.as_str()))
        .collect();
    assert_eq!(
        states,
        [("plan", "done"), ("build", "now"), ("checks", "next")]
    );
    let older = Snapshot::parse(lane("").as_bytes()).unwrap();
    assert_eq!(older.items[0].preset, None);
    assert!(older.items[0].steps.is_empty());
}

#[test]
fn snapshot_reads_the_pace_the_engine_reports_and_none_from_an_older_engine() {
    let with = r#"{"api": 1, "waits": [], "items": [], "events": [], "problems": [],
        "engine": {"pace": "normal", "target_pct": 90}}"#;
    let snapshot = Snapshot::parse(with.as_bytes()).unwrap();
    assert_eq!(snapshot.pace.as_deref(), Some("normal"));
    let without = r#"{"api": 1, "waits": [], "items": [], "events": [], "problems": []}"#;
    let snapshot = Snapshot::parse(without.as_bytes()).unwrap();
    assert_eq!(snapshot.pace, None);
}

#[test]
fn snapshot_reads_the_decided_asks_and_none_from_an_older_engine() {
    let with = r#"{"api": 1, "waits": [], "items": [], "events": [], "problems": [],
        "decided": [{"id": "w1", "key": ["wait", "/work/repo", "demo", "owner_row", "R1"],
                     "kind": "owner_row", "project": "repo", "task": "demo", "title": "Check row R1",
                     "answer": "mark_ok", "choice": null, "text": "looks right", "actor": "cli:me",
                     "at": "2026-11-18T10:00:00Z"}]}"#;
    let decided = Snapshot::parse(with.as_bytes()).unwrap().decided;
    let decided = decided.expect("the engine reported its decided asks");
    assert_eq!(decided.len(), 1);
    assert_eq!(decided[0].answer, "mark_ok");
    assert_eq!(decided[0].text.as_deref(), Some("looks right"));
    assert_eq!(decided[0].choice, None);
    let empty =
        r#"{"api": 1, "waits": [], "items": [], "events": [], "problems": [], "decided": []}"#;
    assert_eq!(
        Snapshot::parse(empty.as_bytes()).unwrap().decided,
        Some(vec![])
    );
    let without = r#"{"api": 1, "waits": [], "items": [], "events": [], "problems": []}"#;
    assert_eq!(Snapshot::parse(without.as_bytes()).unwrap().decided, None);
}

#[test]
fn a_lane_is_named_by_the_title_the_engine_prints_for_it_and_else_by_its_id() {
    let mut document: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let flow = document["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["kind"] == "flow")
        .unwrap();
    let id = flow["key"][2].as_str().unwrap().to_string();
    flow["title"] = "Retry uploads with a backoff".into();
    let named = Snapshot::parse(document.to_string().as_bytes()).unwrap();
    let lane = named.items.iter().find(|item| item.task() == id).unwrap();
    assert_eq!(lane.name(), "Retry uploads with a backoff");
    assert_eq!(
        lane.task(),
        id,
        "the id stays what the engine's actions use"
    );
    document["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["key"][2] == id.as_str())
        .unwrap()["title"] = "".into();
    let unnamed = Snapshot::parse(document.to_string().as_bytes()).unwrap();
    let lane = unnamed.items.iter().find(|item| item.task() == id).unwrap();
    assert_eq!(lane.name(), id, "no title: the id");
}

#[test]
fn a_preset_is_named_by_the_engine_s_label_else_by_its_id_with_a_capital() {
    let item = |extra: &str| {
        format!(
            r#"{{"api": 1, "events": [], "problems": [], "waits": [], "items": [
              {{"id": "i", "key": ["flow", "/a/repo", "one"], "kind": "flow", "repo": "/a/repo",
               "agent_state": "working", "since": null, "last_activity_at": null, "progress": null {extra}}}]}}"#
        )
    };
    let read = |extra: &str| {
        Snapshot::parse(item(extra).as_bytes())
            .unwrap()
            .items
            .remove(0)
    };
    assert_eq!(
        read(r#", "preset": "lean", "preset_label": "Lean (Haiku)""#)
            .preset_name()
            .as_deref(),
        Some("Lean (Haiku)")
    );
    assert_eq!(
        read(r#", "preset": "lean""#).preset_name().as_deref(),
        Some("Lean")
    );
    assert_eq!(read("").preset_name(), None);
}

#[test]
fn lane_key_tells_apart_same_id_in_two_projects_and_an_outside_lane() {
    let text = r#"{"api": 1, "events": [], "problems": [],
        "waits": [
          {"id": "w1", "key": ["wait", "/a/repo", "one", "approval", "spec"], "kind": "approval",
           "repo": "/a/repo", "project": "repo", "task": "one", "title": "t", "since": null, "source": "flow"},
          {"id": "w2", "key": ["wait", "outside", "one", "question", "q"], "kind": "question",
           "repo": "/a/repo", "project": "repo", "task": "one", "title": "t", "since": null, "source": "outside"}
        ],
        "items": [
          {"id": "i1", "key": ["flow", "/a/repo", "one"], "kind": "flow", "repo": "/a/repo",
           "agent_state": "working", "since": null, "last_activity_at": null, "progress": null},
          {"id": "i2", "key": ["flow", "/b/repo", "one"], "kind": "flow", "repo": "/b/repo",
           "agent_state": "working", "since": null, "last_activity_at": null, "progress": null},
          {"id": "i3", "key": ["outside", "one"], "kind": "outside", "repo": "/a/repo",
           "agent_state": "waiting", "since": null, "last_activity_at": null, "progress": null}
        ]}"#;
    let snapshot = Snapshot::parse(text.as_bytes()).unwrap();
    let keys: Vec<String> = snapshot.items.iter().map(|item| item.lane_key()).collect();
    assert_eq!(keys.len(), 3);
    assert!(keys[0] != keys[1] && keys[0] != keys[2] && keys[1] != keys[2]);
    assert_eq!(
        snapshot.waits[0].lane_key(),
        keys[0],
        "a flow wait names its flow lane"
    );
    assert_eq!(
        snapshot.waits[1].lane_key(),
        keys[2],
        "an outside wait names its outside lane"
    );
}

#[tokio::test]
async fn outside_item_parses_title_author_links_unread() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), FIXTURE, 0);
    let snapshot = cli.snapshot().await.unwrap();

    let lane = snapshot.items.iter().find(|item| item.is_outside());
    let lane = lane.expect("the outside lane is listed");
    assert_eq!(lane.task(), "measure-help", "its id, not an engine task");
    assert_eq!(lane.title, "Add --help to measure.py");
    assert_eq!(lane.author, "outside:fixture");
    assert_eq!(lane.agent(), "fixture");
    assert_eq!(lane.project(), "repo");
    assert_eq!(lane.unread, 0);
    assert_eq!(lane.links.len(), 1);
    assert_eq!(lane.links[0].label, "Notes");
    assert_eq!(lane.links[0].url, "https://example.com/notes");
    let progress = lane.progress.as_ref().expect("its rows over its rows");
    assert_eq!((progress.done, progress.total), (0, 1));
    assert_eq!(progress.phase, "draft");
    let keys: Vec<&str> = lane.actions.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(keys, ["d", "w"]);

    let question = snapshot.waits.iter().find(|wait| wait.kind == "question");
    let question = question.expect("the question is a wait");
    assert!(question.is_outside());
    assert_eq!(question.agent(), "fixture");
    assert_eq!(question.choices, ["yes", "no"]);
    let flow = snapshot.waits.iter().find(|wait| wait.task == "gated");
    assert!(!flow.expect("an engine wait").is_outside());
}

#[tokio::test]
async fn snapshot_unknown_api_refused_with_version_named() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), r#"{"api": 2, "waits": []}"#, 0);
    let error = cli.snapshot().await.unwrap_err();
    assert_eq!(error.code, ErrorCode::EngineFailed);
    assert_eq!(error.said.key.0, "error.engine.api");
    assert_eq!(error.said.arg("found"), Some(&Value::Count(2)));
    assert_eq!(error.said.arg("expected"), Some(&Value::Count(1)));
}

#[tokio::test]
async fn snapshot_without_a_version_is_unreadable_not_shown_in_part() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), r#"{"waits": []}"#, 0);
    let error = cli.snapshot().await.unwrap_err();
    assert_eq!(error.said, Said::new("error.engine.unreadable_answer"));
}

#[tokio::test]
async fn snapshot_past_deadline_names_ten_seconds() {
    assert_eq!(
        DEADLINE,
        Duration::from_secs(10),
        "the deadline of the plan"
    );
    let dir = tempfile::tempdir().unwrap();
    let cli = fake_engine(dir.path(), "sleep 30").with_deadline(Duration::from_millis(300));
    let started = std::time::Instant::now();
    let error = cli.snapshot().await.unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "no endless wait"
    );
    assert_eq!(error.said.key.0, "error.engine.timeout");
    let limit = error
        .said
        .arg("limit")
        .expect("the sentence names its limit");
    let Value::Said(limit) = limit else {
        panic!("the limit is said as a duration");
    };
    assert_eq!(limit.key.0, "limit.seconds", "{limit:?}");
    assert_eq!(
        crate::process::limit_said(DEADLINE).arg("count"),
        Some(&Value::Count(10))
    );
}

#[tokio::test]
async fn a_failing_engine_omits_child_stderr_from_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let cli = fake_engine(
        dir.path(),
        "echo 'fixture-value /private/example/settings.toml' >&2\nexit 2",
    );
    let error = cli.snapshot().await.unwrap_err();
    assert_eq!(error.said.key.0, "error.engine.failed");
    assert_eq!(error.said.arg("code"), Some(&Value::Count(2)));
    assert_eq!(error.details, None);
}

#[tokio::test]
async fn malformed_engine_responses_omit_serde_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(
        dir.path(),
        r#"{"api":1,"waits":"/private/example/settings.toml","result":"/private/example/settings.toml"}"#,
        0,
    );
    let snapshot = cli.snapshot().await.unwrap_err();
    let action = cli
        .act(&act(action("e", &[], &[], false), &[]))
        .await
        .unwrap_err();
    for error in [snapshot, action] {
        assert_eq!(error.said.key.0, "error.engine.unreadable_answer");
        assert_eq!(error.details, None);
    }
}

#[tokio::test]
async fn act_exit_two_is_refused_with_reason() {
    let dir = tempfile::tempdir().unwrap();
    let text = r#"{"result": "refused", "reason": "no longer waiting: the gate moved on"}"#;
    let cli = engine_saying(dir.path(), text, 2);
    let result = cli
        .act(&act(action("a", &[], &[], true), &[]))
        .await
        .unwrap();
    assert_eq!(
        result,
        ActResult::Refused {
            reason: "no longer waiting: the gate moved on".to_string(),
            reason_code: None,
            reason_args: None,
        }
    );
}

#[tokio::test]
async fn stale_action_refusal_reports_a_missing_project_folder() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("demo");
    let reason =
        "no longer waiting: the engine has moved on since the page was drawn; nothing was run";
    let text = serde_json::json!({"result": "refused", "reason": reason}).to_string();
    let cli = engine_saying(dir.path(), &text, 2);
    let mut request = act(action("R", &[], &[], true), &[]);
    request.target = serde_json::json!(["flow", project, "gate"]).to_string();
    let error = cli.act(&request).await.expect_err("missing project folder");
    assert_eq!(error.said.key.0, "error.engine.project_folder_missing");
    std::fs::create_dir(&project).unwrap();
    assert_eq!(
        cli.act(&request).await.unwrap(),
        ActResult::Refused {
            reason: reason.into(),
            reason_code: None,
            reason_args: None,
        }
    );
}

#[tokio::test]
async fn act_confirm_answer_carries_token_expiry_and_summary() {
    let dir = tempfile::tempdir().unwrap();
    let text = r#"{"result": "confirm", "token": "17.ab", "expires": "2026-11-16T08:02:00Z", "summary": "Approve on demo: run in /work/repo"}"#;
    let cli = engine_saying(dir.path(), text, 0);
    let result = cli
        .act(&act(action("a", &[], &[], true), &[]))
        .await
        .unwrap();
    assert_eq!(
        result,
        ActResult::Confirm {
            token: "17.ab".to_string(),
            expires: "2026-11-16T08:02:00Z".to_string(),
            summary: "Approve on demo: run in /work/repo".to_string(),
            display_args: None,
        }
    );
}

#[tokio::test]
async fn act_done_keeps_the_exit_code_the_output_and_the_audit_note() {
    let dir = tempfile::tempdir().unwrap();
    let text = r#"{"result": "done", "exit_code": 3, "output": "stopped demo\n", "audit": "not written: disk full"}"#;
    let cli = engine_saying(dir.path(), text, 0);
    let result = cli
        .act(&act(action("s", &[], &[], true), &[]))
        .await
        .unwrap();
    assert_eq!(
        result,
        ActResult::Done {
            exit_code: 3,
            output: "stopped demo\n".to_string(),
            audit: Some("not written: disk full".to_string()),
            connector: None,
        }
    );
}

#[tokio::test]
async fn act_passes_the_arguments_in_the_documented_order_and_a_dash_value_with_an_equals_sign() {
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("seen.txt");
    let body = format!(
        "for a in \"$@\"; do echo \"[$a]\"; done > '{}'\necho '{{\"result\": \"done\", \"exit_code\": 0, \"output\": \"\"}}'",
        seen.display()
    );
    let cli = fake_engine(dir.path(), &body);
    let mark = action("k", &["verdict"], &["note"], false);
    let mut request = act(mark, &[("verdict", "ok"), ("note", "-looks fine")]);
    request.confirm = Some("17.ab".to_string());
    cli.act(&request).await.unwrap();
    let seen = std::fs::read_to_string(seen).unwrap();
    let words: Vec<&str> = seen.lines().collect();
    assert_eq!(
        words,
        [
            "[cockpit]",
            "[act]",
            "[k]",
            r#"[["wait","/work/repo","gated","approval","spec"]]"#,
            "[--note=-looks fine]",
            "[--verdict]",
            "[ok]",
            "[--confirm]",
            "[17.ab]",
            "[--source]",
            "[app]",
            "[--json]"
        ]
    );
}

#[tokio::test]
async fn act_passes_the_choice_of_a_reply_as_its_own_option() {
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("seen.txt");
    let body = format!(
        "for a in \"$@\"; do echo \"[$a]\"; done > '{}'\necho '{{\"result\": \"done\", \"exit_code\": 0, \"output\": \"\"}}'",
        seen.display()
    );
    let cli = fake_engine(dir.path(), &body);
    let reply = action("r", &[], &["choice", "text"], false);
    cli.act(&act(reply, &[("choice", "yes")])).await.unwrap();
    let seen = std::fs::read_to_string(seen).unwrap();
    let words: Vec<&str> = seen.lines().skip(4).collect();
    assert_eq!(
        words,
        ["[--choice]", "[yes]", "[--source]", "[app]", "[--json]"]
    );
}

#[tokio::test]
async fn act_passes_the_preset_and_level_of_a_new_lane_as_their_own_options() {
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("seen.txt");
    let body = format!(
        "for a in \"$@\"; do echo \"[$a]\"; done > '{}'\necho '{{\"result\": \"done\", \"exit_code\": 0, \"output\": \"\"}}'",
        seen.display()
    );
    let cli = fake_engine(dir.path(), &body);
    let start = action("L", &["text"], &["preset", "level"], false);
    let inputs = [
        ("text", "add a parser"),
        ("preset", "careful"),
        ("level", "fast"),
    ];
    cli.act(&act(start, &inputs)).await.unwrap();
    let seen = std::fs::read_to_string(seen).unwrap();
    let words: Vec<&str> = seen.lines().skip(4).collect();
    assert_eq!(
        words,
        [
            "[--level]",
            "[fast]",
            "[--preset]",
            "[careful]",
            "[--text]",
            "[add a parser]",
            "[--source]",
            "[app]",
            "[--json]"
        ]
    );
}

#[tokio::test]
async fn act_input_the_action_does_not_take_is_refused_before_anything_runs() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let cli = fake_engine(dir.path(), &format!("touch '{}'", marker.display()));
    let approve = action("a", &[], &["remember"], true);
    let error = cli
        .act(&act(approve.clone(), &[("text", "x")]))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    let mark = action("k", &["verdict"], &["note"], false);
    let error = cli.act(&act(mark, &[])).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(!marker.exists(), "no process was started");
}

#[tokio::test]
async fn act_output_that_is_not_a_result_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), "not json", 0);
    let error = cli
        .act(&act(action("e", &[], &[], false), &[]))
        .await
        .unwrap_err();
    assert_eq!(error.said, Said::new("error.engine.unreadable_answer"));
}

fn env(sakura_bin: Option<&str>, configured: Option<&str>, path: &str) -> Env {
    Env {
        sakura_bin: sakura_bin.map(PathBuf::from),
        configured: configured.map(PathBuf::from),
        path: path.to_string(),
    }
}

fn executable(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    write_script(&path, "#!/bin/sh\n");
    path
}

#[test]
fn missing_engine_names_every_place_looked() {
    let dir = tempfile::tempdir().unwrap();
    let (first, second) = (dir.path().join("one"), dir.path().join("two"));
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let path = format!("{}:{}", first.display(), second.display());
    let error = EngineCli::locate(&env(None, None, &path)).unwrap_err();
    assert_eq!(error.code, ErrorCode::EngineMissing);
    assert_eq!(error.said.key.0, "error.engine.missing");
    let Some(Value::Text(places)) = error.said.arg("places") else {
        panic!("the places are named");
    };
    assert!(
        places.contains(&first.join("sakura").display().to_string()),
        "{places}"
    );
    assert!(
        places.contains(&second.join("sakura").display().to_string()),
        "{places}"
    );
}

#[test]
fn a_named_engine_is_the_only_place_looked_and_wins_over_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let on_path = executable(dir.path(), "sakura");
    let named = dir.path().join("elsewhere");
    let error = EngineCli::locate(&env(
        Some(named.to_str().unwrap()),
        None,
        dir.path().to_str().unwrap(),
    ))
    .unwrap_err();
    let Some(Value::Text(places)) = error.said.arg("places") else {
        panic!("the places are named");
    };
    assert_eq!(
        places,
        &named.display().to_string(),
        "a wrong SAKURA_BIN is not hidden by the PATH"
    );
    assert!(on_path.exists());
}

#[test]
fn locate_takes_sakura_bin_then_the_path_set_in_settings_then_the_search_path() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, c) = (
        dir.path().join("a"),
        dir.path().join("b"),
        dir.path().join("c"),
    );
    for folder in [&a, &b, &c] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let (bin_a, bin_b, bin_c) = (
        executable(&a, "sakura"),
        executable(&b, "sakura"),
        executable(&c, "sakura"),
    );
    // The engine is run by its real path, the one that was checked.
    let real = |path: &PathBuf| path.canonicalize().unwrap();
    let search = c.to_str().unwrap();
    let both = env(bin_a.to_str(), bin_b.to_str(), search);
    assert_eq!(EngineCli::locate(&both).unwrap().bin(), real(&bin_a));
    let settings = env(None, bin_b.to_str(), search);
    assert_eq!(EngineCli::locate(&settings).unwrap().bin(), real(&bin_b));
    let searched = env(None, None, search);
    assert_eq!(EngineCli::locate(&searched).unwrap().bin(), real(&bin_c));
    let empty = env(Some(""), None, search);
    assert_eq!(
        EngineCli::locate(&empty).unwrap().bin(),
        real(&bin_c),
        "an empty variable is unset"
    );
}

const MASTER_SPEC: &str = r#"{"argv": ["/opt/acp/claude-agent-acp"], "env": {"SAKURA_ENGINE_HOME": "/engine/home"},
 "cwd": "/engine/master", "options": {"settings": {"permissions": {}}, "settingSources": [], "model": "haiku"},
 "mcp_servers": [{"name": "sakura-master", "command": "/bin/sakura", "args": ["master", "serve", "p"], "env": []}],
 "settings_path": "/engine/s.json", "mcp_config_path": "/engine/m.json", "instructions_path": "/engine/i.md",
 "terminal_argv": [], "start_mode": "default", "close_sessions": true,
 "unset_env": ["CLAUDECODE", "TMUX"], "env_allow": ["PATH", "HOME"],
 "node": "/opt/homebrew/bin/node", "adapter": "/opt/acp/claude-agent-acp", "path": "/opt/homebrew/bin:/usr/bin",
 "system_prompt_append": "You are the master."}"#;

#[tokio::test]
async fn master_spec_is_read_from_the_engine_s_json() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), MASTER_SPEC, 0);
    let spec = cli.master_spec(None).await.unwrap();
    assert_eq!(spec.argv, ["/opt/acp/claude-agent-acp"]);
    assert_eq!(spec.env["SAKURA_ENGINE_HOME"], "/engine/home");
    assert_eq!(spec.cwd, Path::new("/engine/master"));
    assert_eq!(spec.start_mode, "default");
    assert!(spec.close_sessions);
    assert_eq!(spec.unset_env, ["CLAUDECODE", "TMUX"]);
    assert_eq!(spec.env_allow, ["PATH", "HOME"]);
    assert_eq!(spec.node, "/opt/homebrew/bin/node");
    assert_eq!(spec.adapter, "/opt/acp/claude-agent-acp");
    assert_eq!(spec.path, "/opt/homebrew/bin:/usr/bin");
    assert_eq!(spec.system_prompt_append, "You are the master.");
    assert_eq!(
        spec.session_meta()["systemPrompt"]["append"],
        "You are the master."
    );
    assert_eq!(spec.mcp_servers[0]["name"], "sakura-master");
    assert_eq!(
        spec.session_meta()["claudeCode"]["options"]["model"],
        "haiku",
        "the options travel under the adapter's own name"
    );
}

const ACCOUNT_SPEC: &str = r#"{"argv": [], "env": {}, "cwd": "/engine/master", "options": {}, "mcp_servers": [],
 "start_mode": "default", "close_sessions": true, "unset_env": [],
 "account": "work", "account_provider": "codex", "account_folder": "/home/p/.codex-2",
 "codex_home": "/home/p/.codex-2"}"#;

#[tokio::test]
async fn master_spec_on_an_account_names_it_to_the_engine_and_reads_its_fields() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!(
        "printf '%s\\n' \"$*\" >> '{}'\ncat '{}'",
        dir.path().join("calls.log").display(),
        {
            let answer = dir.path().join("answer.json");
            std::fs::write(&answer, ACCOUNT_SPEC).unwrap();
            answer.display().to_string()
        }
    );
    let cli = fake_engine(dir.path(), &body);
    let spec = cli.master_spec(Some("work")).await.unwrap();
    let calls = std::fs::read_to_string(dir.path().join("calls.log")).unwrap();
    assert_eq!(calls.trim(), "master spec --json --account work");
    assert_eq!(spec.account, "work");
    assert_eq!(spec.account_provider, "codex");
    assert_eq!(spec.codex_home, "/home/p/.codex-2");
    let plain = cli.master_spec(None).await.unwrap();
    let calls = std::fs::read_to_string(dir.path().join("calls.log")).unwrap();
    assert_eq!(
        calls.lines().last(),
        Some("master spec --json"),
        "no account named"
    );
    drop(plain);
}

#[tokio::test]
async fn master_spec_failure_omits_child_stdout_from_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), "the master's folder is not a real folder\n", 2);
    let error = cli.master_spec(None).await.unwrap_err();
    assert_eq!(error.said.key.0, "error.engine.failed");
    assert_eq!(error.details, None);
}

#[tokio::test]
async fn master_decide_sends_the_request_on_stdin_and_reads_the_decision() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!(
        "cat > '{}'\nprintf '%s' '{{\"decision\": \"ask\", \"reason\": \"no rule covers WebFetch\"}}'",
        dir.path().join("stdin.json").display()
    );
    let cli = fake_engine(dir.path(), &body);
    let request =
        serde_json::json!({ "method": "session/request_permission", "params": { "x": 1 } });
    let decided = cli.master_decide(&request).await.unwrap();
    assert_eq!(decided.decision, super::master::Decision::Ask);
    assert_eq!(decided.reason, "no rule covers WebFetch");
    let received = std::fs::read_to_string(dir.path().join("stdin.json")).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&received).unwrap(),
        request
    );
}

#[tokio::test]
async fn master_decide_reads_each_of_the_three_decisions() {
    for (word, wanted) in [
        ("allow", super::master::Decision::Allow),
        ("deny", super::master::Decision::Deny),
        ("ask", super::master::Decision::Ask),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(r#"{{"decision": "{word}", "reason": "r"}}"#);
        let cli = engine_saying(dir.path(), &text, 0);
        let decided = cli.master_decide(&serde_json::json!({})).await.unwrap();
        assert_eq!(decided.decision, wanted);
    }
}

#[tokio::test]
async fn master_decide_with_an_answer_nobody_can_read_is_an_error_not_an_allow() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), r#"{"decision": "maybe"}"#, 0);
    assert!(cli.master_decide(&serde_json::json!({})).await.is_err());
    let cli = engine_saying(dir.path(), "refused: inside a cage", 2);
    assert!(cli.master_decide(&serde_json::json!({})).await.is_err());
}

const FEED_PAGE: &str = r#"{"note": "Entries come from agent runs.", "entries": [
 {"run": "build-1", "role": "builder", "workspace": "/work/repo-demo", "kind": "text", "text": "I read the parser.", "tool": "", "path": "", "line": null},
 {"run": "build-1", "role": "builder", "workspace": "/work/repo-demo", "kind": "tool", "text": "", "tool": "Edit", "path": "src/parser.rs", "line": 12, "truncated": true}],
 "cursor": "build-1=2", "more": true}"#;

#[tokio::test]
async fn feed_reads_a_page_run_in_the_lane_s_repository_after_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!(
        "pwd -P > '{0}/cwd'\nprintf '%s\\n' \"$*\" > '{0}/args'\ncat '{0}/page.json'",
        dir.path().display()
    );
    std::fs::write(dir.path().join("page.json"), FEED_PAGE).unwrap();
    let cli = fake_engine(dir.path(), &body);
    let repo = tempfile::tempdir().unwrap();
    let page = cli.feed(repo.path(), "demo", "build-1=0").await.unwrap();
    assert_eq!(page.cursor, "build-1=2");
    assert!(page.more);
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[0].role, "builder");
    assert!(!page.entries[0].is_tool());
    assert_eq!(page.entries[0].text, "I read the parser.");
    assert!(page.entries[1].is_tool());
    assert_eq!(
        (page.entries[1].tool.as_str(), page.entries[1].line),
        ("Edit", Some(12))
    );
    assert!(page.entries[1].truncated);
    let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
    assert_eq!(args.trim(), "flow feed --json --since=build-1=0 -- demo");
    let cwd = std::fs::read_to_string(dir.path().join("cwd")).unwrap();
    assert_eq!(
        cwd.trim(),
        repo.path().canonicalize().unwrap().display().to_string()
    );
}

#[tokio::test]
async fn feed_from_the_start_sends_no_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!(
        "printf '%s\\n' \"$*\" > '{0}/args'\nprintf '%s' '{{\"entries\": [], \"cursor\": \"\", \"more\": false}}'",
        dir.path().display()
    );
    let cli = fake_engine(dir.path(), &body);
    let repo = tempfile::tempdir().unwrap();
    cli.feed(repo.path(), "demo", "").await.unwrap();
    let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
    assert_eq!(args.trim(), "flow feed --json -- demo");
}

#[tokio::test]
async fn feed_refuses_a_task_that_reads_as_an_option_and_a_folder_that_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), r#"{"entries": [], "cursor": ""}"#, 0);
    let repo = tempfile::tempdir().unwrap();
    let error = cli.feed(repo.path(), "--json", "").await.unwrap_err();
    assert_eq!(error.said.key.0, "error.engine.input_refused");
    let gone = repo.path().join("nowhere");
    assert!(cli.feed(&gone, "demo", "").await.is_err());
}

#[tokio::test]
async fn lane_commands_reject_invalid_task_ids_before_starting_the_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let cli = fake_engine(dir.path(), "touch started\nexit 3");
    for task in [
        "two/parts",
        "with_under",
        "-leading",
        "Upper",
        ".",
        &"a".repeat(49),
    ] {
        let errors = [
            cli.feed(dir.path(), task, "").await.unwrap_err(),
            cli.agents(dir.path(), task).await.unwrap_err(),
            cli.agent_feed(dir.path(), task, "run", None)
                .await
                .unwrap_err(),
            cli.write_to_agent(dir.path(), task, "run", "note")
                .await
                .unwrap_err(),
            cli.changes(dir.path(), task).await.unwrap_err(),
            cli.changes_file(dir.path(), task, "a.rs")
                .await
                .unwrap_err(),
        ];
        for error in errors {
            assert_eq!(error.code, ErrorCode::InvalidArgument, "{task}");
        }
    }
    assert!(!dir.path().join("started").exists());
}

#[tokio::test]
async fn a_lane_folder_that_is_gone_names_the_lane_and_the_project_never_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), "{}", 0);
    let repo = tempfile::tempdir().unwrap();
    let gone = repo.path().join("orchard");
    let refused = [
        cli.feed(&gone, "add-search", "").await.unwrap_err(),
        cli.changes(&gone, "add-search").await.unwrap_err(),
        cli.changes_file(&gone, "add-search", "src/a.rs")
            .await
            .unwrap_err(),
    ];
    for error in refused {
        assert_eq!(error.said.key.0, "error.engine.lane_folder_missing");
        assert_eq!(
            error.said.arg("lane"),
            Some(&crate::said::Value::Text("add-search".into()))
        );
        assert_eq!(
            error.said.arg("project"),
            Some(&crate::said::Value::Text("orchard".into()))
        );
        assert_eq!(error.details, None, "no raw line with the path");
        let shown = format!("{error:?}");
        let parent = repo.path().to_string_lossy().into_owned();
        assert!(!shown.contains(&parent), "{shown}");
    }
}

#[tokio::test]
async fn feed_failure_omits_child_stdout_from_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), "unknown task demo; tasks: none\n", 2);
    let repo = tempfile::tempdir().unwrap();
    let error = cli.feed(repo.path(), "demo", "").await.unwrap_err();
    assert_eq!(error.said.key.0, "error.engine.failed");
    assert_eq!(error.details, None);
}

#[test]
fn engine_path_is_built_from_known_folders_with_the_persons_own_first() {
    use super::cli::known_path;
    let home = Path::new("/home/someone");
    assert_eq!(
        known_path(Some(home)),
        "/home/someone/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
    );
    assert_eq!(
        known_path(None),
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
        "without a home there is no folder of the person's own"
    );
}

#[tokio::test]
async fn every_engine_command_runs_with_that_path_whatever_the_app_was_launched_with() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!(
        "printf '%s' \"$PATH\" > '{0}/path'\nprintf '%s' '{{\"entries\": [], \"cursor\": \"\", \"more\": false}}'",
        dir.path().display()
    );
    let cli = fake_engine(dir.path(), &body);
    let repo = tempfile::tempdir().unwrap();
    cli.feed(repo.path(), "demo", "").await.unwrap();
    let path = std::fs::read_to_string(dir.path().join("path")).unwrap();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    assert_eq!(path, super::cli::known_path(home.as_deref()));
    assert!(path.contains("/.local/bin:"), "{path}");
    let env = Env::from_process(None);
    assert_eq!(
        env.path, path,
        "the engine is looked for in the same folders"
    );
}

fn wait_named(name: &str) -> super::snapshot::Wait {
    let key = serde_json::json!(["wait", "/work/repo", "demo", "approval", name]);
    serde_json::from_value(serde_json::json!({
        "id": key.to_string(), "key": key, "kind": "approval", "repo": "/work/repo", "project": "repo",
        "task": "demo", "title": "t", "since": null, "actions": []
    }))
    .unwrap()
}

#[test]
fn a_wait_s_gate_is_its_name_whatever_revision_the_engine_binds_it_to() {
    let bound = wait_named("overlap@a9102f70c7c7");
    assert_eq!(bound.key_name(), "overlap");
    assert_eq!(
        bound.key_revision(),
        "a9102f70c7c7",
        "the revision stays for the engine"
    );
    assert!(
        bound.id.contains("overlap@a9102f70c7c7"),
        "and the id the engine named is untouched"
    );
    let plain = wait_named("checks");
    assert_eq!((plain.key_name(), plain.key_revision()), ("checks", ""));
}

#[test]
fn a_wait_names_its_lane_by_the_lane_title_or_by_the_slug_when_there_is_none() {
    let untitled = wait_named("checks");
    assert_eq!(untitled.lane_name(), "demo");
    let mut titled = wait_named("checks");
    titled.lane_title = "Add a demo".into();
    assert_eq!(titled.lane_name(), "Add a demo");
    assert_eq!(titled.task, "demo", "the slug stays the id");
}

mod trust {
    use super::super::cli::{EngineCli, Env};
    use super::super::trust::{check, check_with, check_with_groups, person, Groups, Untrusted};
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};

    /// A program in a folder of its own, under a root only the person can write; `mode` is the program's.
    fn program(root: &Path, mode: u32) -> PathBuf {
        let folder = root.join("bin");
        std::fs::create_dir_all(&folder).unwrap();
        let program = folder.join("sakura");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(mode)).unwrap();
        program
    }

    /// A group lookup that says every group has this name, these members (none: they cannot be read) and this
    /// answer about nesting.
    struct Fake {
        name: &'static str,
        members: Option<Vec<u32>>,
        nested: bool,
    }

    impl Groups for Fake {
        fn name(&self, _gid: u32) -> Option<String> {
            Some(self.name.to_string())
        }
        // The temporary files of a test are in the group of the person, not gid 80: the fake judges by name
        // alone, and the number has its own test.
        fn is_trusted(&self, _gid: u32, name: &str) -> bool {
            name == "admin"
        }
        fn members(&self, _gid: u32) -> Option<Vec<u32>> {
            self.members.clone()
        }
        fn has_nested(&self, _name: &str) -> bool {
            self.nested
        }
    }

    fn group(name: &'static str, members: Vec<u32>) -> Fake {
        Fake {
            name,
            members: Some(members),
            nested: false,
        }
    }

    fn private_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        root
    }

    #[test]
    fn a_program_the_person_owns_that_others_cannot_write_is_trusted_by_its_real_path() {
        let root = private_root();
        let program = program(root.path(), 0o755);
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&program, &link).unwrap();
        assert_eq!(check(&link, person()), Ok(program.canonicalize().unwrap()));
    }

    #[test]
    fn a_program_someone_else_owns_is_refused() {
        let root = private_root();
        let program = program(root.path(), 0o755);
        let other = person() + 1;
        assert!(matches!(
            check(&program, other),
            Err(Untrusted::NotOwned(_))
        ));
    }

    #[test]
    fn a_program_under_a_folder_owned_by_another_account_is_refused_naming_that_folder() {
        let root = private_root();
        let program = program(root.path(), 0o755);
        // With no trusted owner but the person, the folders of root above the temporary one are not trusted.
        let Err(Untrusted::FolderOwned(_, at)) = check_with(&program, person(), &[]) else {
            panic!("a folder above is owned by root");
        };
        assert!(at.is_dir());
        assert_ne!(
            std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&at).unwrap()),
            person(),
            "{}",
            at.display()
        );
        assert!(
            check(&program, person()).is_ok(),
            "root and the person are trusted by default"
        );
    }

    #[test]
    fn a_sticky_folder_that_others_can_write_is_allowed_when_the_entry_in_it_is_the_persons() {
        let root = private_root();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let program = program(&shared, 0o755);
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o1777)).unwrap();
        assert!(check(&program, person()).is_ok());
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            matches!(check(&program, person()), Err(Untrusted::Writable(..))),
            "without the sticky bit it is not"
        );
    }

    #[test]
    fn group_write_is_allowed_when_every_member_of_the_group_is_the_person_or_root() {
        let me = person();
        let root = private_root();
        let program = program(root.path(), 0o775);
        let admin_like = group("admin", vec![me, 0]);
        assert!(
            check_with_groups(&program, me, &[0], &admin_like).is_ok(),
            "the file"
        );
        // A folder above that the group can write.
        let program = self::program(&root.path().join("deeper"), 0o755);
        let folder = program.parent().unwrap();
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o775)).unwrap();
        assert!(
            check_with_groups(&program, me, &[0], &admin_like).is_ok(),
            "the folder"
        );
    }

    #[test]
    fn group_write_is_refused_when_another_account_is_in_the_group() {
        let me = person();
        let root = private_root();
        let program = program(root.path(), 0o775);
        let shared = group("admin", vec![me, 0, me + 1]);
        assert!(matches!(
            check_with_groups(&program, me, &[0], &shared),
            Err(Untrusted::Writable(..))
        ));
        let folder_program = self::program(&root.path().join("deeper"), 0o755);
        std::fs::set_permissions(
            folder_program.parent().unwrap(),
            std::fs::Permissions::from_mode(0o775),
        )
        .unwrap();
        let Err(Untrusted::Writable(_, at)) = check_with_groups(&folder_program, me, &[0], &shared)
        else {
            panic!("the folder is writable by a group with a stranger in it");
        };
        assert_eq!(at, folder_program.parent().unwrap().canonicalize().unwrap());
    }

    #[test]
    fn world_write_is_refused_whatever_the_group() {
        let me = person();
        let root = private_root();
        let program = program(root.path(), 0o757);
        let only_me = group("admin", vec![me]);
        assert!(matches!(
            check_with_groups(&program, me, &[0], &only_me),
            Err(Untrusted::Writable(..))
        ));
    }

    #[test]
    fn every_group_but_admin_is_refused_whoever_is_listed_in_it() {
        let me = person();
        let root = private_root();
        let program = program(root.path(), 0o775);
        for name in [
            "everyone",
            "staff",
            "localaccounts",
            "com.apple.access_ssh",
            "wheel",
            "friends",
        ] {
            let only_me = group(name, vec![me]);
            let refused = check_with_groups(&program, me, &[0], &only_me);
            assert!(
                matches!(refused, Err(Untrusted::Writable(..))),
                "{name}: {refused:?}"
            );
        }
        assert!(check_with_groups(&program, me, &[0], &group("admin", vec![me, 0])).is_ok());
    }

    #[test]
    fn admin_is_trusted_by_its_name_and_its_number_together() {
        struct Real;
        impl Groups for Real {
            fn name(&self, _gid: u32) -> Option<String> {
                None
            }
            fn members(&self, _gid: u32) -> Option<Vec<u32>> {
                None
            }
            fn has_nested(&self, _name: &str) -> bool {
                true
            }
        }
        assert!(Real.is_trusted(80, "admin"));
        assert!(!Real.is_trusted(81, "admin"), "another number is not admin");
        assert!(!Real.is_trusted(80, "wheel"), "another name is not admin");
    }

    #[test]
    fn a_group_with_nested_groups_is_refused_even_with_only_the_person_and_root_listed() {
        let me = person();
        let root = private_root();
        let program = program(root.path(), 0o775);
        let nested = Fake {
            name: "admin",
            members: Some(vec![me, 0]),
            nested: true,
        };
        assert!(matches!(
            check_with_groups(&program, me, &[0], &nested),
            Err(Untrusted::Writable(..))
        ));
    }

    #[test]
    fn a_group_that_cannot_be_read_whole_is_refused() {
        let me = person();
        let root = private_root();
        let program = program(root.path(), 0o775);
        let unreadable = Fake {
            name: "admin",
            members: None,
            nested: false,
        };
        assert!(
            matches!(
                check_with_groups(&program, me, &[0], &unreadable),
                Err(Untrusted::Writable(..))
            ),
            "an unresolved name or an error walking the accounts is a refusal, not an empty group"
        );
    }

    #[test]
    fn a_group_the_system_cannot_name_is_refused() {
        struct Nameless;
        impl Groups for Nameless {
            fn name(&self, _gid: u32) -> Option<String> {
                None
            }
            fn members(&self, _gid: u32) -> Option<Vec<u32>> {
                Some(vec![person()])
            }
            fn has_nested(&self, _name: &str) -> bool {
                false
            }
        }
        let root = private_root();
        let program = program(root.path(), 0o775);
        assert!(matches!(
            check_with_groups(&program, person(), &[0], &Nameless),
            Err(Untrusted::Writable(..))
        ));
    }

    #[test]
    fn the_real_lookup_finds_no_nested_groups_in_admin_and_refuses_what_it_cannot_read() {
        use super::super::trust::System;
        assert!(
            !System::default().has_nested("admin"),
            "admin has no NestedGroups on a Mac"
        );
        assert!(
            System::default().has_nested("no-such-group-here"),
            "a failed lookup is a refusal"
        );
    }

    #[test]
    fn the_real_group_lookup_names_the_person_among_the_members_of_their_own_primary_group() {
        let system = super::super::trust::System::default();
        assert!(
            system
                .members(0)
                .is_some_and(|members| members.contains(&0)),
            "root's own group holds root"
        );
        let mine = rustix::process::getegid().as_raw();
        assert!(system
            .members(mine)
            .is_some_and(|members| members.contains(&person())));
        assert_eq!(
            system.members(u32::MAX - 7),
            None,
            "an unknown group is refused, not read as an empty one"
        );
    }

    #[test]
    fn the_real_admin_group_is_trusted_on_this_mac_and_holds_only_the_person_and_root() {
        use super::super::trust::System;
        let system = System::default();
        let admin = system.name(80);
        assert_eq!(admin.as_deref(), Some("admin"));
        let members = system.members(80).expect("admin can be read whole");
        assert!(
            members.iter().all(|uid| *uid == person() || *uid == 0),
            "{members:?}"
        );
    }

    #[test]
    fn a_program_under_a_folder_others_can_write_is_refused_naming_that_folder() {
        let root = private_root();
        let program = program(root.path(), 0o755);
        let folder = program.parent().unwrap();
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o777)).unwrap();
        let Err(Untrusted::Writable(_, at)) = check(&program, person()) else {
            panic!("the folder is writable by others");
        };
        assert_eq!(at, folder.canonicalize().unwrap());
    }

    #[test]
    fn a_link_into_a_folder_others_can_write_is_judged_by_where_it_leads() {
        let root = private_root();
        let open = root.path().join("open");
        std::fs::create_dir(&open).unwrap();
        let real = open.join("sakura");
        std::fs::write(&real, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        let link = root.path().join("sakura");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(
            check(&link, person()),
            Err(Untrusted::Writable(..))
        ));
    }

    #[test]
    fn locating_the_engine_refuses_an_untrusted_program_with_a_message_naming_its_path() {
        let root = private_root();
        let program = program(root.path(), 0o777);
        let env = Env {
            sakura_bin: Some(program.clone()),
            configured: None,
            path: String::new(),
        };
        let error = EngineCli::locate(&env).unwrap_err();
        assert_eq!(error.said.key.0, "error.engine.untrusted_writable");
        let named = format!("{:?}", error.said.args);
        assert!(named.contains(&program.display().to_string()), "{named}");
    }

    #[test]
    fn locating_the_engine_on_the_path_takes_a_trusted_program_and_runs_it_by_its_real_path() {
        let root = private_root();
        let program = program(root.path(), 0o755);
        let env = Env {
            sakura_bin: None,
            configured: None,
            path: program.parent().unwrap().display().to_string(),
        };
        let cli = EngineCli::locate(&env).unwrap();
        assert_eq!(cli.bin(), program.canonicalize().unwrap());
    }
}

#[test]
fn snapshot_reads_a_project_with_no_lane_and_what_starts_a_lane_in_it() {
    let text = r#"{"api": 1, "waits": [], "events": [], "problems": [], "items": [],
        "projects": [{"id": "[\"project\",\"/work/empty\"]", "key": ["project", "/work/empty"],
                      "name": "empty", "folder": "/work/empty", "open_lanes": 0,
                      "actions": [{"key": "L", "label": "Start lane", "needs": ["text"],
                                   "optional": [], "confirm": false}]}]}"#;
    let snapshot = Snapshot::parse(text.as_bytes()).unwrap();
    let project = &snapshot.projects[0];
    assert_eq!(project.name, "empty");
    assert_eq!(project.folder, "/work/empty");
    assert_eq!(project.open_lanes, 0);
    assert_eq!(project.id, r#"["project","/work/empty"]"#);
    assert_eq!(
        project.start_lane().map(|action| action.key.as_str()),
        Some("L")
    );
}

#[test]
fn a_snapshot_without_projects_lists_none() {
    let text = r#"{"api": 1, "waits": [], "events": [], "problems": [], "items": []}"#;
    assert!(Snapshot::parse(text.as_bytes())
        .unwrap()
        .projects
        .is_empty());
}

#[tokio::test]
async fn add_project_asks_the_engine_to_register_the_folder_and_reads_its_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let text = r#"{"result": "refused", "reason": "the folder is not a Git repository; choose a folder of one"}"#;
    let log = dir.path().join("calls.log");
    let body = format!(
        "printf '%s\\n' \"$*\" >> '{}'\ncat <<'JSON'\n{text}\nJSON\nexit 2",
        log.display()
    );
    let cli = fake_engine(dir.path(), &body);
    let result = cli.add_project("/work/plain").await.unwrap();
    assert_eq!(
        result,
        ActResult::Refused {
            reason: "the folder is not a Git repository; choose a folder of one".to_string(),
            reason_code: None,
            reason_args: None,
        }
    );
    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        calls.trim(),
        "cockpit act add-project /work/plain --source app --json"
    );
}

#[tokio::test]
async fn add_project_refuses_a_path_that_is_not_absolute_before_anything_starts() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let cli = fake_engine(dir.path(), &format!("touch '{}'", marker.display()));
    for folder in ["relative/path", "-x", ""] {
        let error = cli.add_project(folder).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{folder:?}");
    }
    assert!(!marker.exists(), "the engine was never started");
}

mod leeway {
    use super::super::leeway::{bundled_helper, LEEWAY_VAR};
    use super::fake_engine;
    use crate::test_support::write_script;
    use std::path::{Path, PathBuf};

    /// A folder laid out like `Sakura.app`, with the app's program in it; the helper is made when `helper`
    /// names a file mode.
    fn bundle(root: &Path, helper: Option<u32>) -> (PathBuf, PathBuf) {
        let contents = root.join("Sakura.app/Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::create_dir_all(contents.join("Helpers")).unwrap();
        let program = contents.join("MacOS/sakura");
        write_script(&program, "#!/bin/sh\n");
        let helper_path = contents.join("Helpers/leeway");
        if let Some(mode) = helper {
            write_script(&helper_path, "#!/bin/sh\n");
            let permissions = std::os::unix::fs::PermissionsExt::from_mode(mode);
            std::fs::set_permissions(&helper_path, permissions).unwrap();
        }
        (program, helper_path)
    }

    #[test]
    fn bundled_helper_in_a_bundle_is_the_file_in_contents_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let (program, helper) = bundle(dir.path(), Some(0o755));
        assert_eq!(bundled_helper(&program), Some(helper));
    }

    #[test]
    fn bundled_helper_without_a_helper_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let (program, _) = bundle(dir.path(), None);
        assert_eq!(bundled_helper(&program), None);
    }

    #[test]
    fn bundled_helper_that_cannot_be_run_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let (program, _) = bundle(dir.path(), Some(0o644));
        assert_eq!(bundled_helper(&program), None);
    }

    #[test]
    fn bundled_helper_for_a_program_outside_a_bundle_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let (_, helper) = bundle(dir.path(), Some(0o755));
        let loose = helper.parent().unwrap().join("../../../debug/sakura");
        assert_eq!(bundled_helper(&loose), None);
        let beside = dir.path().join("Sakura.app/Contents/Helpers/sakura");
        assert_eq!(bundled_helper(&beside), None, "Helpers is not MacOS");
    }

    #[tokio::test]
    async fn engine_command_carries_the_helper_the_app_names() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!(
            "printf '%s' \"${LEEWAY_VAR}\" > '{0}/seen'\nprintf '%s' '{{\"entries\": [], \"cursor\": \"\", \"more\": false}}'",
            dir.path().display()
        );
        let helper = dir.path().join("leeway");
        let cli = fake_engine(dir.path(), &body).with_leeway(Some(helper.clone()));
        let repo = tempfile::tempdir().unwrap();
        cli.feed(repo.path(), "demo", "").await.unwrap();
        let seen = std::fs::read_to_string(dir.path().join("seen")).unwrap();
        assert_eq!(seen, helper.display().to_string());
    }

    #[tokio::test]
    async fn engine_command_without_a_helper_leaves_the_variable_as_the_app_got_it() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!(
            "if [ -n \"${{{LEEWAY_VAR}+set}}\" ]; then printf set > '{0}/seen'; else printf unset > '{0}/seen'; fi\nprintf '%s' '{{\"entries\": [], \"cursor\": \"\", \"more\": false}}'",
            dir.path().display()
        );
        let cli = fake_engine(dir.path(), &body).with_leeway(None);
        let repo = tempfile::tempdir().unwrap();
        cli.feed(repo.path(), "demo", "").await.unwrap();
        let seen = std::fs::read_to_string(dir.path().join("seen")).unwrap();
        assert_eq!(
            seen,
            if std::env::var_os(LEEWAY_VAR).is_some() {
                "set"
            } else {
                "unset"
            }
        );
    }
}

mod tmux {
    use super::super::tmux::{bundled_tmux, TMUX_VAR};
    use super::fake_engine;
    use crate::test_support::write_script;
    use std::path::{Path, PathBuf};

    /// A folder laid out like `Sakura.app`, with the app's program in it; `Helpers/tmux` is made when
    /// `helper` names a file mode.
    fn bundle(root: &Path, helper: Option<u32>) -> (PathBuf, PathBuf) {
        let contents = root.join("Sakura.app/Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::create_dir_all(contents.join("Helpers")).unwrap();
        let program = contents.join("MacOS/sakura");
        write_script(&program, "#!/bin/sh\n");
        let helper_path = contents.join("Helpers/tmux");
        if let Some(mode) = helper {
            write_script(&helper_path, "#!/bin/sh\n");
            let permissions = std::os::unix::fs::PermissionsExt::from_mode(mode);
            std::fs::set_permissions(&helper_path, permissions).unwrap();
        }
        (program, helper_path)
    }

    #[test]
    fn bundled_tmux_in_a_bundle_is_the_file_in_contents_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let (program, helper) = bundle(dir.path(), Some(0o755));
        assert_eq!(bundled_tmux(&program), Some(helper));
    }

    #[test]
    fn bundled_tmux_without_the_file_or_when_it_cannot_run_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let (program, _) = bundle(dir.path(), None);
        assert_eq!(bundled_tmux(&program), None);
        let other = tempfile::tempdir().unwrap();
        let (program, _) = bundle(other.path(), Some(0o644));
        assert_eq!(bundled_tmux(&program), None);
    }

    #[test]
    fn bundled_tmux_for_a_program_outside_a_bundle_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let (_, helper) = bundle(dir.path(), Some(0o755));
        let loose = helper.parent().unwrap().join("../../../debug/sakura");
        assert_eq!(bundled_tmux(&loose), None);
    }

    #[tokio::test]
    async fn engine_command_carries_the_tmux_the_app_names() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!(
            "printf '%s' \"${TMUX_VAR}\" > '{0}/seen'\nprintf '%s' '{{\"entries\": [], \"cursor\": \"\", \"more\": false}}'",
            dir.path().display()
        );
        let helper = dir.path().join("tmux");
        let cli = fake_engine(dir.path(), &body).with_tmux(Some(helper.clone()));
        let repo = tempfile::tempdir().unwrap();
        cli.feed(repo.path(), "demo", "").await.unwrap();
        let seen = std::fs::read_to_string(dir.path().join("seen")).unwrap();
        assert_eq!(seen, helper.display().to_string());
    }

    #[tokio::test]
    async fn engine_command_without_a_tmux_leaves_the_variable_as_the_app_got_it() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!(
            "if [ -n \"${{{TMUX_VAR}+set}}\" ]; then printf set > '{0}/seen'; else printf unset > '{0}/seen'; fi\nprintf '%s' '{{\"entries\": [], \"cursor\": \"\", \"more\": false}}'",
            dir.path().display()
        );
        let cli = fake_engine(dir.path(), &body).with_tmux(None);
        let repo = tempfile::tempdir().unwrap();
        cli.feed(repo.path(), "demo", "").await.unwrap();
        let seen = std::fs::read_to_string(dir.path().join("seen")).unwrap();
        assert_eq!(
            seen,
            if std::env::var_os(TMUX_VAR).is_some() {
                "set"
            } else {
                "unset"
            }
        );
    }
}

#[test]
fn snapshot_reads_a_lanes_goal_steps_reviews_preset_and_pace_and_none_from_an_older_engine() {
    let document = r#"{"api": 1, "waits": [], "events": [], "problems": [], "items": [
      {"id": "[\"flow\",\"/work/repo\",\"demo\"]", "key": ["flow", "/work/repo", "demo"], "kind": "flow",
       "repo": "/work/repo", "worktree": "/work/repo-demo", "agent_state": "working", "since": null,
       "last_activity_at": null, "progress": null, "actions": [],
       "goal": "Make uploads retry.\nFive tries at most.", "opened_by": "master", "preset": "careful",
       "pace": "normal", "in_review": true,
       "steps": [{"id": "plan", "name": "Plan", "state": "done", "model": "opus", "account": "work"},
                 {"id": "build", "name": "Build", "state": "now", "model": null, "account": null}],
       "reviews": [{"id": "plan_review", "on": true, "model": "codex", "account": null, "state": "done"},
                   {"id": "final_review", "on": false, "model": null, "account": null, "state": "skipped"}]},
      {"id": "[\"flow\",\"/work/repo\",\"old\"]", "key": ["flow", "/work/repo", "old"], "kind": "flow",
       "repo": "/work/repo", "agent_state": "working", "since": null, "last_activity_at": null,
       "progress": null, "actions": []}]}"#;
    let items = Snapshot::parse(document.as_bytes()).unwrap().items;
    let demo = &items[0];
    assert_eq!(
        demo.goal.as_deref(),
        Some("Make uploads retry.\nFive tries at most.")
    );
    assert_eq!(demo.opened_by.as_deref(), Some("master"));
    assert_eq!(demo.preset.as_deref(), Some("careful"));
    assert_eq!(demo.pace.as_deref(), Some("normal"));
    assert_eq!(demo.worktree.as_deref(), Some("/work/repo-demo"));
    assert!(demo.in_review);
    assert_eq!(demo.steps.len(), 2);
    assert_eq!(demo.steps[0].model.as_deref(), Some("opus"));
    assert_eq!(demo.steps[0].account.as_deref(), Some("work"));
    assert_eq!(demo.steps[1].model, None);
    assert_eq!(demo.reviews.len(), 2);
    assert!(demo.reviews[0].is_on && !demo.reviews[1].is_on);
    let old = &items[1];
    assert_eq!(
        (
            &old.goal,
            &old.opened_by,
            &old.preset,
            &old.pace,
            &old.worktree
        ),
        (&None, &None, &None, &None, &None)
    );
    assert!(old.steps.is_empty() && old.reviews.is_empty() && !old.in_review);
}

#[test]
fn feed_reads_result_notice_and_board_entries_and_plain_ones_still() {
    let page = r##"{"entries": [
     {"run": "build-1", "role": "evaluator", "kind": "result", "text": "", "line": null,
      "result": {"verdict": "pass", "summary": "214 passed, none failed."}},
     {"run": "", "role": "engine", "kind": "notice", "tag": "build", "text": "build done",
      "at": "2026-11-18T21:36:00Z", "line": null},
     {"run": "", "role": "builder", "kind": "board", "from_task": "impersonation", "from_title": "#4992 impersonation",
      "text": "I am changing client.ts too.", "at": "2026-11-18T21:35:00Z", "line": null},
     {"run": "build-1", "role": "builder", "kind": "text", "text": "Done.", "line": null}],
     "cursor": "x", "more": false}"##;
    let page: super::feed::FeedPage = serde_json::from_str(page).unwrap();
    let result = page.entries[0].result.as_ref().expect("a result");
    assert_eq!(
        (result.verdict.as_str(), result.summary.as_str()),
        ("pass", "214 passed, none failed.")
    );
    assert_eq!(page.entries[1].tag, "build");
    assert_eq!(page.entries[1].at.as_deref(), Some("2026-11-18T21:36:00Z"));
    assert_eq!(page.entries[2].from_title, "#4992 impersonation");
    assert!(page.entries[3].result.is_none() && page.entries[3].at.is_none());
}

const CHANGES_PAGE: &str = r#"{"task": "demo", "base": "a1b2c3d", "gone": false, "added": 98, "removed": 12,
 "files": [{"path": "src/upload/uploadQueue.ts", "status": "modified", "added": 86, "removed": 12},
           {"path": "logo.png", "status": "added", "added": null, "removed": null}]}"#;

#[tokio::test]
async fn changes_reads_the_lanes_files_run_in_its_repository_and_a_patch_by_path() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!(
        "printf '%s\\n' \"$*\" >> '{0}/args'\ncase \"$*\" in *--file*) cat '{0}/patch.json';; *) cat '{0}/page.json';; esac",
        dir.path().display()
    );
    std::fs::write(dir.path().join("page.json"), CHANGES_PAGE).unwrap();
    std::fs::write(
        dir.path().join("patch.json"),
        r#"{"task": "demo", "path": "src/a.rs", "patch": "@@ -1 +1 @@\n-a\n+b\n", "truncated": false}"#,
    )
    .unwrap();
    let cli = fake_engine(dir.path(), &body);
    let repo = tempfile::tempdir().unwrap();
    let page = cli.changes(repo.path(), "demo").await.unwrap();
    assert_eq!(
        (page.added, page.removed, page.gone),
        (Some(98), Some(12), false)
    );
    assert_eq!(page.files[0].added, Some(86));
    assert_eq!(page.files[1].added, None, "a binary file has no counts");
    let patch = cli
        .changes_file(repo.path(), "demo", "src/a.rs")
        .await
        .unwrap();
    assert!(patch.patch.contains("+b") && !patch.truncated);
    let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        [
            "flow changes --json -- demo",
            "flow changes --json --file=src/a.rs -- demo"
        ]
    );
}

#[tokio::test]
async fn changes_refuses_a_path_that_leaves_the_worktree_or_reads_as_an_option_before_running() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), "{}", 0);
    let repo = tempfile::tempdir().unwrap();
    for path in ["../x", "a/../../x", "/etc/passwd", "-rf", ""] {
        let error = cli
            .changes_file(repo.path(), "demo", path)
            .await
            .unwrap_err();
        assert_eq!(error.said.key.0, "error.engine.input_refused", "{path:?}");
    }
    assert!(cli.changes(repo.path(), "--json").await.is_err());
}

#[tokio::test]
async fn a_lane_that_is_gone_has_an_empty_change_list_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let cli = engine_saying(
        dir.path(),
        r#"{"task": "demo", "base": "", "gone": true, "files": [], "added": 0, "removed": 0}"#,
        0,
    );
    let repo = tempfile::tempdir().unwrap();
    let page = cli.changes(repo.path(), "demo").await.unwrap();
    assert!(page.gone && page.files.is_empty());
}

#[tokio::test]
async fn act_takes_the_level_of_a_pace_and_the_preset_of_a_switch() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!(
        "printf '%s\\n' \"$*\" >> '{}/args'\necho '{{\"result\": \"done\", \"exit_code\": 0}}'",
        dir.path().display()
    );
    let cli = fake_engine(dir.path(), &body);
    let pace = act(action("p", &["level"], &[], false), &[("level", "fast")]);
    cli.act(&pace).await.unwrap();
    let switch = act(action("S", &["preset"], &[], false), &[("preset", "lean")]);
    cli.act(&switch).await.unwrap();
    let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    assert!(
        lines[0].contains("cockpit act p") && lines[0].contains("--level fast"),
        "{args}"
    );
    assert!(
        lines[1].contains("cockpit act S") && lines[1].contains("--preset lean"),
        "{args}"
    );
}

#[test]
fn a_step_agent_carries_the_engine_s_note_of_a_provider_with_no_account() {
    let agent: super::rules::RuleAgent = serde_json::from_str(
        r#"{"account": "project", "provider": "codex", "model": "gpt", "effort": "high",
            "note": "No Claude account exists, so the builder runs on the Codex account (Codex)."}"#,
    )
    .expect("an agent");
    assert!(agent.note.starts_with("No Claude account exists"));
    let older: super::rules::RuleAgent =
        serde_json::from_str(r#"{"account": "a"}"#).expect("older");
    assert_eq!(older.note, "");
}

#[test]
fn a_step_agent_and_an_agents_entry_carry_the_account_name_and_whether_their_lists_are_kept() {
    let agent: super::rules::RuleAgent = serde_json::from_str(
        r#"{"account": "claude-aa0b", "provider": "claude", "model": "haiku", "effort": "medium",
            "account_label": "Claude", "models_state": "refreshing",
            "models_note": "Reading the models of Claude."}"#,
    )
    .expect("an agent");
    assert_eq!(agent.account_label, "Claude");
    assert_eq!(agent.models_state, "refreshing");
    assert_eq!(agent.models_note, "Reading the models of Claude.");
    let entry: super::rules::AgentAccount = serde_json::from_str(
        r#"{"id": "a", "label": "Claude", "provider": "claude", "models": [],
            "models_state": "unavailable", "models_note": "The models of Claude could not be read: no answer."}"#,
    )
    .expect("an entry");
    assert_eq!(entry.models_state, "unavailable");
    let older: super::rules::AgentAccount =
        serde_json::from_str(r#"{"id": "a", "provider": "claude"}"#).expect("older");
    assert_eq!(
        (older.models_state.as_str(), older.models_note.as_str()),
        ("", "")
    );
}

#[test]
fn a_lane_is_in_one_step_the_engine_s_current_step_whatever_else_still_says_running() {
    let step = |id: &str, state: &str| serde_json::json!({"id": id, "name": id, "state": state, "on": true});
    let lane = |current: Option<serde_json::Value>| -> super::snapshot::Item {
        let mut document: serde_json::Value = serde_json::from_str(FIXTURE).expect("the fixture");
        let demo = document["items"][0].as_object_mut().expect("a lane");
        demo.insert(
            "pipeline".into(),
            serde_json::json!([
                step("build", "running"),
                step("final_review", "running"),
                step("deliver", "next")
            ]),
        );
        demo.insert(
            "current_step".into(),
            current.unwrap_or(serde_json::Value::Null),
        );
        let snapshot = Snapshot::parse(document.to_string().as_bytes()).expect("a snapshot");
        snapshot.items.into_iter().next().expect("a lane")
    };
    let states = |item: &super::snapshot::Item| -> Vec<String> {
        item.current_pipeline()
            .iter()
            .map(|step| step.state.clone())
            .collect()
    };
    let with_current = lane(Some(
        serde_json::json!({"id": "final_review", "name": "Final review", "waiting": false, "note": ""}),
    ));
    assert_eq!(states(&with_current), ["done", "running", "next"]);
    assert_eq!(
        states(&lane(None)),
        ["done", "running", "next"],
        "no current step: the last that runs"
    );
    let waits = lane(Some(
        serde_json::json!({"id": "deliver", "name": "Deliver", "waiting": true, "note": "Waits at the pull request."}),
    ));
    let shown = waits.current_pipeline();
    assert_eq!(shown[2].state, "waiting");
    assert_eq!(shown[2].note, "Waits at the pull request.");
    assert_eq!(shown[0].state, "done");
}

#[test]
fn contract_work_is_shown_in_the_plan_pipeline_step() {
    let mut document: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let lane = &mut document["items"][0];
    lane["pipeline"] = serde_json::json!([
        {"id":"plan","state":"next","on":true},
        {"id":"plan_review","state":"off","on":false},
        {"id":"build","state":"next","on":true}
    ]);
    lane["current_step"] = serde_json::json!({"id":"contract"});
    let snapshot = Snapshot::parse(document.to_string().as_bytes()).unwrap();
    let pipeline = snapshot.items[0].current_pipeline();
    assert_eq!(pipeline[0].state, "running");
    assert_eq!(pipeline[1].state, "off");
}

fn runs_with(extra: &[(&str, &str, &str)]) -> Vec<super::agents::AgentRun> {
    let mut runs: Vec<super::agents::AgentRun> =
        serde_json::from_str(include_str!("../../tests/fixtures/engine-agents.json"))
            .expect("the agents fixture");
    let template = runs[0].clone();
    for (name, role, phase_and_step) in extra {
        let (phase, step) = phase_and_step.split_once('/').expect("phase/step");
        let mut run = template.clone();
        run.name = name.to_string();
        run.role = role.to_string();
        run.phase = phase.to_string();
        run.step = step.to_string();
        runs.push(run);
    }
    runs
}

#[test]
fn a_plan_review_only_the_arbiter_ran_is_not_a_plan_review() {
    use super::agents::is_plan_review_only_the_arbiters;
    // The arbiter runs under the plan reviewer's role, so the engine says the step it serves is the plan review.
    let arbiter = ("arbiter-1", "plan_reviewer", "arbiter/plan_review");
    assert!(is_plan_review_only_the_arbiters(&runs_with(&[arbiter])));
    let reviewed = ("review-plan-1", "plan_reviewer", "contract/plan_review");
    assert!(
        !is_plan_review_only_the_arbiters(&runs_with(&[arbiter, reviewed])),
        "a real plan review ran as well"
    );
    assert!(
        !is_plan_review_only_the_arbiters(&runs_with(&[])),
        "no arbiter ran"
    );
    assert!(
        !is_plan_review_only_the_arbiters(&[]),
        "nothing is known yet"
    );
}

#[test]
fn a_lane_without_its_plan_review_lists_it_off_in_the_pipeline_and_the_reviews() {
    let document: serde_json::Value = serde_json::from_str(FIXTURE).expect("the fixture");
    let snapshot = Snapshot::parse(document.to_string().as_bytes()).expect("a snapshot");
    let demo = snapshot.items.into_iter().next().expect("the demo lane");
    let plan_review = |item: &super::snapshot::Item| {
        let step = item
            .current_pipeline()
            .iter()
            .find(|s| s.id == "plan_review")
            .cloned();
        let review = item.reviews.iter().find(|r| r.id == "plan_review").cloned();
        (step.expect("a step"), review.expect("a review"))
    };
    let (step, review) = plan_review(&demo);
    assert!(step.is_on && review.is_on, "the fixture has it on and done");
    let mut demo = demo;
    demo.pipeline
        .iter_mut()
        .find(|step| step.id == "plan_review")
        .unwrap()
        .reason = "plan review is already done on this lane".to_string();
    let corrected = demo.without_plan_review();
    let (step, review) = plan_review(&corrected);
    assert_eq!((step.is_on, step.state.as_str()), (false, "off"));
    assert!(step.reason.is_empty(), "the old completion reason is stale");
    assert_eq!((review.is_on, review.state.as_str()), (false, "skipped"));
    assert_eq!(
        corrected.pipeline.len(),
        demo.pipeline.len(),
        "the other steps stay"
    );
}

#[test]
fn track_snapshot_rejects_malformed_known_fields() {
    let track = serde_json::json!({
        "id": "t-01234567", "name": "Release", "goal": "Ship", "summary": "",
        "status": "unknown", "updated_at": "2026-11-23T10:00:00+00:00",
        "projects": [], "lanes": [], "conversations": [], "decisions": [], "actions": []
    });
    for fields in [
        serde_json::json!({"tracks": [track]}),
        serde_json::json!({"tracks": [], "track_actions": false}),
        serde_json::json!({"tracks": null, "track_problem": false}),
        serde_json::json!({"tracks": [], "projects": [
            {"id": "project", "name": "App", "folder": "/app", "track_choices": false}
        ]}),
    ] {
        let mut document = serde_json::json!({"api": 1, "items": [], "waits": []});
        document
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        assert_eq!(
            Snapshot::parse(document.to_string().as_bytes())
                .unwrap_err()
                .said
                .key
                .0,
            "error.engine.unreadable_answer"
        );
    }
}

#[tokio::test]
async fn track_act_keeps_multiline_arguments_and_retry_input() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("argv");
    let cli = fake_engine(
        dir.path(),
        &format!(
            "printf '%s\\0' \"$@\" >> '{}'\nprintf '{{\"result\":\"done\",\"exit_code\":0}}'",
            log.display()
        ),
    );
    let mut request = act(
        action("tracksettle", &["answer", "op"], &[], false),
        &[("answer", "-First line\nSecond line"), ("op", "same-op")],
    );
    request.target = r#"["wait","track","t-01234567","track_decision","1"]"#.into();
    for _ in 0..2 {
        cli.act(&request).await.unwrap();
    }
    let argv = std::fs::read_to_string(log).unwrap();
    let words = argv.split_terminator('\0').collect::<Vec<_>>();
    assert_eq!(words[..9], words[9..]);
    assert_eq!(
        words[..9],
        [
            "cockpit",
            "act",
            "tracksettle",
            &request.target,
            "--answer=-First line\nSecond line",
            "--op=same-op",
            "--source",
            "app",
            "--json"
        ]
    );
}
