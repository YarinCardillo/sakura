//! The rows of the Checks view: one per verb, with what the runner is doing for it and its last result.

use super::result::{is_for_head, result_view, ResultView};
use super::trust::{tests_word, touched_sentence};
use super::{required_verbs, Tone};
use crate::models::{Place, RunnerState, Task};
use crate::said::{Key, Said};

pub const SHORT_SHA: usize = 7;

/// What a verb shows: the runner's work for it first, else its last result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyState {
    /// The runner runs it now for this task.
    InCorso,
    /// Waiting in the runner's queue; `position` counts from 1.
    InCoda { position: usize },
    /// Nothing queued: the last result.
    Result(ResultView),
}

impl VerifyState {
    /// The key of the word beside the glyph.
    pub fn word(self) -> Key {
        match self {
            VerifyState::InCorso => Key("verify.state.running"),
            VerifyState::InCoda { .. } => Key("verify.state.queued"),
            VerifyState::Result(view) => view.word(),
        }
    }

    /// The tone of the glyph and the word. An old result is muted whatever its exit code.
    pub fn tone(self) -> Tone {
        match self {
            VerifyState::InCorso | VerifyState::InCoda { .. } => Tone::Running,
            VerifyState::Result(ResultView::Passato) => Tone::Passed,
            VerifyState::Result(ResultView::Fallito) => Tone::Failed,
            VerifyState::Result(ResultView::Vecchio) => Tone::Old,
            VerifyState::Result(ResultView::MaiEseguito) => Tone::Neutral,
            VerifyState::Result(ResultView::Annullata | ResultView::Scartata) => Tone::Old,
            VerifyState::Result(ResultView::VerdeConRiserva(_)) => Tone::Neutral,
        }
    }
}

/// One row of the Checks view.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifyRow {
    pub verb: String,
    pub state: VerifyState,
    /// Short sha of the request the runner has for the verb (running or queued), else of the last result.
    pub sha: Option<String>,
    /// Duration of the last result, in seconds.
    pub seconds: Option<f64>,
    /// Whether that request, else the last result, is for HEAD.
    pub is_head: bool,
    /// How many tests the last result ran, in words, when `box` recorded it and no request replaces it.
    pub tests: Option<Said>,
    /// What the last result's commit did to the tests that existed at the base, when it touched any.
    pub tests_touched: Option<Said>,
    /// The last result ran in the task's folder on the Mac, not in a box: it is no evidence against the
    /// agent, and the row says so.
    pub ran_on_mac: bool,
    /// Why `box` set the last result aside, when it did and no request replaces it.
    pub discarded: Option<Said>,
}

/// The reasons `box` gives for a result it set aside that the interface has a sentence for.
const DISCARDED: [(&str, &str); 2] = [
    (
        "the worktree changed while the check ran",
        "verify.discarded.changed",
    ),
    ("commit first", "verify.discarded.uncommitted"),
];

/// What is said of `reason`, the line `box` gave for a result it set aside: the interface's sentence
/// when it has one, else the reason itself.
pub fn discarded_said(reason: &str) -> Said {
    let known = DISCARDED.iter().find(|(mark, _)| reason.contains(mark));
    match known {
        Some((_, key)) => Said::new(key),
        None => Said::new("verify.discarded.other").text("reason", reason),
    }
}

/// The rows for `task`: the required verbs first, then every other verb with a result or a request in the
/// runner, in alphabetical order. `box` does not list a profile's verbs, so a verb never run nor requested
/// cannot be shown.
pub fn verify_rows(task: &Task, runner: Option<&RunnerState>) -> Vec<VerifyRow> {
    verbs(task, runner)
        .into_iter()
        .map(|verb| match request_of(task, runner, &verb) {
            Some((state, sha)) => request_row(task, verb, state, sha),
            None => result_row(task, verb),
        })
        .collect()
}

/// A verb the runner runs or has queued: the sha of that request, and whether it is HEAD.
fn request_row(task: &Task, verb: String, state: VerifyState, sha: &str) -> VerifyRow {
    VerifyRow {
        state,
        sha: Some(short_sha(sha)),
        seconds: task.results.get(&verb).map(|r| r.seconds),
        is_head: task.head.as_deref() == Some(sha),
        tests: None,
        tests_touched: None,
        ran_on_mac: false,
        discarded: None,
        verb,
    }
}

/// A verb the runner does not have: its last result.
fn result_row(task: &Task, verb: String) -> VerifyRow {
    let result = task.results.get(&verb);
    VerifyRow {
        state: VerifyState::Result(result_view(task, &verb)),
        sha: result.map(|r| short_sha(&r.sha)),
        seconds: result.map(|r| r.seconds),
        is_head: result.is_some_and(|r| is_for_head(task, r)),
        tests: result.and_then(tests_word),
        tests_touched: result.and_then(touched_sentence),
        ran_on_mac: result.is_some_and(|r| r.place == Place::Mac),
        discarded: result
            .and_then(|r| r.discarded.as_deref())
            .map(discarded_said),
        verb,
    }
}

/// The first [`SHORT_SHA`] characters of `sha`.
pub fn short_sha(sha: &str) -> String {
    sha.chars().take(SHORT_SHA).collect()
}

fn verbs(task: &Task, runner: Option<&RunnerState>) -> Vec<String> {
    let mut others: Vec<String> = task.results.keys().cloned().collect();
    if let Some(runner) = runner {
        let current = runner.current.iter().filter(|run| run.task == task.name);
        others.extend(current.map(|run| run.verb.clone()));
        let queued = runner
            .queue
            .iter()
            .filter(|request| request.task == task.name);
        others.extend(queued.map(|request| request.verb.clone()));
    }
    let required = required_verbs(task);
    others.retain(|verb| !required.contains(verb));
    others.sort();
    others.dedup();
    required.iter().cloned().chain(others).collect()
}

/// What the runner has for `verb` of `task`, running first, with the sha it was requested.
fn request_of<'a>(
    task: &Task,
    runner: Option<&'a RunnerState>,
    verb: &str,
) -> Option<(VerifyState, &'a str)> {
    let runner = runner?;
    let running = runner.current.as_ref();
    if let Some(run) = running.filter(|run| run.task == task.name && run.verb == verb) {
        return Some((VerifyState::InCorso, run.sha.as_str()));
    }
    let (index, request) = runner
        .queue
        .iter()
        .enumerate()
        .find(|(_, request)| request.task == task.name && request.verb == verb)?;
    let state = VerifyState::InCoda {
        position: index + 1,
    };
    Some((state, request.sha.as_str()))
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{cancelled_result, failed_result, passed_result, readable, HEAD};
    use super::*;
    use crate::models::{QueuedRequest, RunnerPhase, RunningRequest};

    fn runner(current: Option<(&str, &str)>, queue: &[(&str, &str)]) -> RunnerState {
        RunnerState {
            state: RunnerPhase::Running,
            current: current.map(|(task, verb)| RunningRequest {
                task: task.into(),
                verb: verb.into(),
                sha: HEAD.into(),
                started_at: "2026-11-10T06:41:12Z".into(),
            }),
            queue: queue
                .iter()
                .map(|(task, verb)| QueuedRequest {
                    task: (*task).into(),
                    verb: (*verb).into(),
                    sha: HEAD.into(),
                    queued_at: "2026-11-10T06:43:50Z".into(),
                })
                .collect(),
        }
    }

    fn states(rows: &[VerifyRow]) -> Vec<(&str, VerifyState)> {
        rows.iter()
            .map(|row| (row.verb.as_str(), row.state))
            .collect()
    }

    #[test]
    fn verify_rows_a_result_that_ran_on_the_mac_says_so_and_one_from_a_box_does_not() {
        let mut task = readable("m1");
        passed_result(&mut task, "test");
        passed_result(&mut task, "rules");
        let on_mac = task.results.get_mut("test").expect("the result");
        on_mac.place = crate::models::Place::Mac;
        let rows = verify_rows(&task, None);
        let ran_on_mac: Vec<_> = rows
            .iter()
            .map(|row| (row.verb.as_str(), row.ran_on_mac))
            .collect();
        assert_eq!(ran_on_mac, vec![("test", true), ("rules", false)]);
    }

    #[test]
    fn verify_rows_a_result_read_from_box_keeps_where_it_ran_and_none_means_a_box() {
        let read = |place: &str| -> crate::models::VerifyResult {
            let json = format!(
                r#"{{"sha":"a","exitCode":0,"seconds":3,"finishedAt":"t","isHead":true{place}}}"#
            );
            serde_json::from_str(&json).expect("a result")
        };
        assert_eq!(read(r#","where":"mac""#).place, crate::models::Place::Mac);
        assert_eq!(read(r#","where":"box""#).place, crate::models::Place::Box);
        assert_eq!(read("").place, crate::models::Place::Box, "an older box");
    }

    #[test]
    fn required_verbs_are_listed_even_when_never_run() {
        let rows = verify_rows(&readable("m1"), None);
        let never = VerifyState::Result(ResultView::MaiEseguito);
        assert_eq!(states(&rows), vec![("test", never), ("rules", never)]);
        assert_eq!(rows[0].sha, None);
    }

    #[test]
    fn other_verbs_follow_from_results_and_the_runner_in_order() {
        let mut task = readable("m1");
        passed_result(&mut task, "lint");
        let rows = verify_rows(
            &task,
            Some(&runner(None, &[("m1", "build"), ("m2", "e2e")])),
        );
        let verbs: Vec<&str> = rows.iter().map(|row| row.verb.as_str()).collect();
        assert_eq!(verbs, vec!["test", "rules", "build", "lint"]);
    }

    #[test]
    fn running_and_queued_requests_win_over_the_last_result() {
        let mut task = readable("m1");
        passed_result(&mut task, "test");
        failed_result(&mut task, "rules");
        let runner = runner(Some(("m1", "test")), &[("m2", "test"), ("m1", "rules")]);
        let rows = verify_rows(&task, Some(&runner));
        assert_eq!(
            states(&rows),
            vec![
                ("test", VerifyState::InCorso),
                ("rules", VerifyState::InCoda { position: 2 }),
            ]
        );
    }

    #[test]
    fn a_running_or_queued_request_shows_its_own_sha_and_whether_it_is_head() {
        let mut task = readable("m1");
        passed_result(&mut task, "test");
        let older = "1111111111111111111111111111111111111111";
        let mut state = runner(Some(("m1", "test")), &[("m1", "rules")]);
        if let Some(run) = state.current.as_mut() {
            run.sha = older.into();
        }
        let rows = verify_rows(&task, Some(&state));
        assert_eq!(rows[0].state, VerifyState::InCorso);
        assert_eq!(rows[0].sha.as_deref(), Some(&older[..SHORT_SHA]));
        assert!(!rows[0].is_head);
        assert_eq!(rows[1].state, VerifyState::InCoda { position: 1 });
        assert_eq!(rows[1].sha.as_deref(), Some(&HEAD[..SHORT_SHA]));
        assert!(rows[1].is_head, "queued for HEAD, never run before");
    }

    #[test]
    fn another_tasks_run_does_not_change_this_task() {
        let mut task = readable("m1");
        passed_result(&mut task, "test");
        let rows = verify_rows(&task, Some(&runner(Some(("m2", "test")), &[])));
        assert_eq!(rows[0].state, VerifyState::Result(ResultView::Passato));
    }

    #[test]
    fn result_rows_carry_the_short_sha_duration_and_head_mark() {
        let mut task = readable("m1");
        passed_result(&mut task, "test");
        let row = &verify_rows(&task, None)[0];
        assert_eq!(row.sha.as_deref(), Some(&HEAD[..SHORT_SHA]));
        assert_eq!(row.seconds, Some(38.0));
        assert!(row.is_head);
    }

    #[test]
    fn a_result_the_service_set_aside_is_told_from_a_failure_and_carries_its_reason() {
        let reason = "the worktree changed while the check ran (the command ended with 0): this is not a result for 1234567890ab";
        let json = format!(
            r#"{{"sha":"{HEAD}","exitCode":2,"seconds":3,"finishedAt":"t","isHead":true,"discarded":"{reason}"}}"#
        );
        let mut task = readable("m1");
        let result = serde_json::from_str(&json).expect("a result");
        task.results.insert("test".into(), result);
        let row = &verify_rows(&task, None)[0];
        assert_ne!(row.state.word(), Key("result.failed"));
        assert_eq!(row.state.word(), Key("result.discarded"));
        assert_ne!(row.state.tone(), Tone::Failed);
        assert_eq!(
            row.discarded,
            Some(Said::new("verify.discarded.changed")),
            "the reason in the interface's words"
        );
        let other = "the worktree is at 1234, not at the commit that was requested";
        let said = discarded_said(other);
        assert_eq!(
            said,
            Said::new("verify.discarded.other").text("reason", other)
        );
    }

    #[test]
    fn a_cancelled_result_reads_cancelled_with_its_commit_muted_never_green() {
        let mut task = readable("m1");
        cancelled_result(&mut task, "test");
        let row = &verify_rows(&task, None)[0];
        assert_eq!(row.state.word(), Key("result.cancelled"));
        assert_eq!(row.state.tone(), Tone::Old);
        assert_eq!(row.sha.as_deref(), Some(&HEAD[..SHORT_SHA]));
        assert!(row.is_head);
    }

    #[test]
    fn a_green_result_that_is_not_plain_green_is_neutral_and_the_test_row_has_its_count() {
        let mut task = readable("m1");
        passed_result(&mut task, "test");
        if let Some(result) = task.results.get_mut("test") {
            result.changes_checks = Some(true);
            result.checks_touched = Some(vec!["vitest.config.ts".into()]);
            result.tests = Some(crate::models::TestCounts {
                ran: 7880,
                base: Some(7880),
                ..Default::default()
            });
        }
        let row = &verify_rows(&task, None)[0];
        assert_eq!(row.state.word(), Key("reserve.checks_changed"));
        assert_eq!(row.state.tone(), Tone::Neutral, "never the green tone");
        assert_eq!(row.tests, Some(Said::new("verify.tests").count(7880)));
    }

    #[test]
    fn a_row_says_what_its_commit_did_to_the_existing_tests() {
        let mut task = readable("m1");
        passed_result(&mut task, "test");
        if let Some(result) = task.results.get_mut("test") {
            result.tests_touched = Some(crate::models::TestsTouched {
                modified: 3,
                deleted: 1,
                files: Vec::new(),
            });
        }
        let row = &verify_rows(&task, None)[0];
        assert_eq!(
            row.tests_touched,
            Some(
                Said::new("trust.tests_modified_deleted")
                    .count(3)
                    .number("deleted", 1)
            )
        );
        assert_eq!(row.state.tone(), Tone::Neutral);
    }

    #[test]
    fn an_old_result_is_muted_never_green() {
        let mut task = readable("m1");
        passed_result(&mut task, "test");
        task.head = Some("2222222222222222222222222222222222222222".into());
        let row = &verify_rows(&task, None)[0];
        assert_eq!(row.state.word(), Key("result.old"));
        assert_eq!(row.state.tone(), Tone::Old);
        assert!(!row.is_head);
    }
}
