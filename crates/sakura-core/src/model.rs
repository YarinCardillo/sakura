//! The task model the window shows, as pure functions over [`Task`] facts: which sidebar group a task belongs
//! to, its state word and tone, whether "Take" is a plain press, a hold or disabled, how a verification
//! result reads, and the figures for durations and ages. No I/O, no clock: the caller passes
//! the time when one is needed.

mod format;
mod header;
mod remove;
mod result;
mod take;
mod trust;
mod verify;

pub use format::{
    duration, facts_age, is_load_high, is_memory_critical, load, memory, runner_status,
    runner_waiting, FRESH_FACTS_SECONDS,
};
pub use header::{changed_files, failed_checks, has_something_to_take};
pub use remove::{remove_rule, RemoveRule};
pub use result::{result_view, ResultView};
pub use take::{take_details, take_rule, TakeRule};
pub use trust::{distrust, tests_word, touched_sentence, Distrust, Missing, Reserve};
pub use verify::{short_sha, verify_rows, VerifyRow, VerifyState, SHORT_SHA};

use crate::models::{AgentPhase, Task};
use crate::said::Key;

/// The verbs whose results must be green on HEAD for a plain "Take" and for the "Verified" state: the
/// checks of the task's own project, as `box` lists them. No verb is known to the app by itself.
pub fn required_verbs(task: &Task) -> &[String] {
    task.checks.as_deref().unwrap_or_default()
}

/// The sidebar's groups, in the order the sidebar shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// The agent waits for the person: a question, a permission.
    ServeTe,
    /// An agent is running.
    AlLavoro,
    /// The workspace is being created, or the agent is starting.
    InPreparazione,
    /// To look at: the agent exited, nothing is running, the work changed after a take, or the workspace
    /// cannot be read.
    DaGuardare,
    /// Nothing runs and nothing is left to look at: the agent answered and stands by.
    InAttesa,
    /// Nothing was ever started on it.
    DaAvviare,
    /// Taken, with nothing new since.
    Presi,
}

impl Group {
    /// All groups in sidebar order.
    pub const ALL: [Group; 7] = [
        Group::ServeTe,
        Group::AlLavoro,
        Group::InPreparazione,
        Group::DaGuardare,
        Group::InAttesa,
        Group::DaAvviare,
        Group::Presi,
    ];

    /// The key of the group's label.
    pub fn label(self) -> Key {
        Key(match self {
            Group::ServeTe => "group.needs_you",
            Group::AlLavoro => "group.working",
            Group::InPreparazione => "group.preparing",
            Group::DaGuardare => "group.review",
            Group::InAttesa => "group.idle",
            Group::DaAvviare => "group.not_started",
            Group::Presi => "group.taken",
        })
    }

    /// What names the group in the window whatever the language: for its elements, never shown.
    pub fn id(self) -> &'static str {
        match self {
            Group::ServeTe => "needs-you",
            Group::AlLavoro => "working",
            Group::InPreparazione => "preparing",
            Group::DaGuardare => "review",
            Group::InAttesa => "idle",
            Group::DaAvviare => "not-started",
            Group::Presi => "taken",
        }
    }
}

/// Which group a task belongs to by the facts alone (the window lists it by the state it shows, see
/// `group_tasks`). The rules, first match wins:
///
/// 1. Being prepared (`preparing` set and not failed): Al lavoro.
/// 2. Preparation failed, or the workspace is unreadable: Da guardare.
/// 3. An agent container is running: Al lavoro (even if the task was taken before).
/// 4. Taken, HEAD is the taken commit and nothing is uncommitted: Taken.
/// 5. Everything else (agent exited, no agent and never taken, taken with new commits or uncommitted files
///    since): Da guardare.
pub fn group_of(task: &Task) -> Group {
    if is_preparing(task) || (is_agent_running(task) && is_settled(task)) {
        return Group::AlLavoro;
    }
    if !is_settled(task) {
        return Group::DaGuardare;
    }
    if is_taken_unchanged(task) {
        Group::Presi
    } else {
        Group::DaGuardare
    }
}

/// Splits tasks into the three groups, in sidebar order. `waits` says whether a task's agent waits for the
/// person: those are listed with the ones to look at, before the others; the input order is kept otherwise.
pub fn group_tasks<'a>(
    tasks: &'a [Task],
    seen: &dyn Fn(&Task) -> (bool, Live),
) -> Vec<(Group, Vec<&'a Task>)> {
    let group = |task: &Task| {
        let (waits, live) = seen(task);
        shown_group(task, waits, live)
    };
    Group::ALL
        .iter()
        .map(|listed| {
            let is_member = |task: &&Task| group(task) == *listed;
            let mut members: Vec<&Task> = tasks.iter().filter(is_member).collect();
            members.sort_by_key(|task| !seen(task).0);
            (*listed, members)
        })
        .collect()
}

/// What a state looks like and means, independent of any colour value: the window maps a tone to its theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    /// Quiet, nothing to do (preparing, or nothing started yet).
    Neutral,
    /// An agent works.
    Running,
    /// Needs attention.
    Attention,
    /// Checks passed on HEAD.
    Passed,
    /// A check failed, or creation failed.
    Failed,
    /// Done or out of date: shown muted.
    Old,
    /// Something is wrong with the workspace itself.
    Warning,
}

/// The state of a task as the window names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateId {
    /// The lavoro's agent asked the person something and stands still until it is answered.
    NeedsYou,
    Preparing,
    /// Nothing to look at yet: no commit ahead, nothing uncommitted, no agent at work.
    NotStarted,
    CreationFailed,
    Unreadable,
    Working,
    /// The agent answered and nothing is left to look at: it waits for the next message.
    Idle,
    ToReview,
    Verified,
    Failed,
    Taken,
}

impl StateId {
    /// What names the state in the window whatever the language: for its elements, never shown.
    pub fn id(self) -> &'static str {
        match self {
            StateId::NeedsYou => "needs-you",
            StateId::Preparing => "preparing",
            StateId::NotStarted => "not-started",
            StateId::CreationFailed => "creation-failed",
            StateId::Unreadable => "unreadable",
            StateId::Working => "working",
            StateId::Idle => "idle",
            StateId::ToReview => "to-review",
            StateId::Verified => "verified",
            StateId::Failed => "failed",
            StateId::Taken => "taken",
        }
    }
}

/// What the lavoro's conversation in the window says of its agent. For a lavoro on the Mac the service
/// knows nothing of the agent (it runs under the window), so this is the only source of "working".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Live {
    /// No conversation was held in this window.
    #[default]
    Unknown,
    /// The lavoro is being prepared: its agent cannot start yet.
    Starting,
    /// A turn is running.
    Working,
    /// The agent answered and stands by.
    Answered,
    /// The agent ended with a problem.
    Stopped,
}

/// The state the window shows for `task`, with what its conversation says: waiting for the person
/// first, then a turn at work; after an answer, what the facts say, and "idle" where they say nothing
/// was started.
pub fn shown_state_live(task: &Task, waits_for_person: bool, live: Live) -> TaskState {
    if waits_for_person {
        return state_for(StateId::NeedsYou);
    }
    let said = task_state(task);
    match live {
        Live::Starting => state_for(StateId::Preparing),
        Live::Stopped => state_for(StateId::Failed),
        Live::Working => state_for(StateId::Working),
        Live::Answered if said.id == StateId::NotStarted => state_for(StateId::Idle),
        Live::Answered | Live::Unknown => said,
    }
}

/// A state with the key of its word and its tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskState {
    pub id: StateId,
    pub word: Key,
    pub tone: Tone,
}

/// The state of a task. Verified needs every required verb passed on HEAD with commits ahead and nothing
/// uncommitted; Failed means one of them failed on HEAD. A result for an older commit is never green.
pub fn task_state(task: &Task) -> TaskState {
    let id = match group_of(task) {
        _ if is_preparing(task) => StateId::Preparing,
        _ if is_creation_failed(task) => StateId::CreationFailed,
        _ if !task.readable => StateId::Unreadable,
        Group::AlLavoro => StateId::Working,
        Group::Presi => StateId::Taken,
        Group::DaGuardare
        | Group::InAttesa
        | Group::DaAvviare
        | Group::ServeTe
        | Group::InPreparazione => review_state(task),
    };
    state_for(id)
}

/// The group the window lists `task` under, from the state it shows: what needs the person, what is at
/// work, what stands by with nothing to look at, what was taken. A lavoro whose agent answered and left
/// nothing to review is not among the ones to review.
fn shown_group(task: &Task, waits_for_person: bool, live: Live) -> Group {
    match shown_state_live(task, waits_for_person, live).id {
        StateId::NeedsYou => Group::ServeTe,
        StateId::Preparing => Group::InPreparazione,
        StateId::ToReview
        | StateId::Verified
        | StateId::Failed
        | StateId::CreationFailed
        | StateId::Unreadable => Group::DaGuardare,
        StateId::Working => Group::AlLavoro,
        StateId::Idle => Group::InAttesa,
        StateId::NotStarted => Group::DaAvviare,
        StateId::Taken => Group::Presi,
    }
}

/// The word and tone of a state id.
pub fn state_for(id: StateId) -> TaskState {
    let (word, tone) = match id {
        StateId::NeedsYou => ("state.needs_you", Tone::Attention),
        StateId::Preparing => ("state.preparing", Tone::Neutral),
        StateId::NotStarted => ("state.not_started", Tone::Neutral),
        StateId::CreationFailed => ("state.creation_failed", Tone::Failed),
        StateId::Unreadable => ("state.unreadable", Tone::Warning),
        StateId::Working => ("state.working", Tone::Running),
        StateId::Idle => ("state.idle", Tone::Neutral),
        StateId::ToReview => ("state.to_review", Tone::Attention),
        StateId::Verified => ("state.verified", Tone::Passed),
        StateId::Failed => ("state.failed", Tone::Failed),
        StateId::Taken => ("state.taken", Tone::Old),
    };
    TaskState {
        id,
        word: Key(word),
        tone,
    }
}

/// Whether `task` needs attention (the only meaning of the accent), by the state the window
/// shows: its agent asked something or ended badly, it stopped with work to look at, or a result arrived
/// for HEAD. A lavoro that is empty, idle, unreadable, being prepared, at work or taken is not one.
pub fn is_attention(task: &Task, waits_for_person: bool, live: Live) -> bool {
    matches!(
        shown_state_live(task, waits_for_person, live).id,
        StateId::NeedsYou | StateId::ToReview | StateId::Verified | StateId::Failed
    )
}

/// Ready to start when there is no work at all; else Failed when a required verb failed on HEAD; Verified
/// exactly when "Take" is a plain press (commits ahead, nothing uncommitted, every required verb passed on
/// HEAD) and the project has a check at all, so green never covers unverified or absent work.
fn review_state(task: &Task) -> StateId {
    if is_empty(task) {
        return StateId::NotStarted;
    }
    let required = required_verbs(task);
    let is_failed = required
        .iter()
        .any(|verb| result_view(task, verb) == ResultView::Fallito);
    if is_failed {
        StateId::Failed
    } else if take_rule(task) == TakeRule::Plain && !required.is_empty() {
        StateId::Verified
    } else {
        StateId::ToReview
    }
}

/// Known to have no commit ahead and nothing uncommitted. Unknown figures are never called empty.
fn is_empty(task: &Task) -> bool {
    task.ahead == Some(0) && task.dirty_files == Some(0)
}

fn is_preparing(task: &Task) -> bool {
    task.preparing.as_ref().is_some_and(|p| !p.failed)
}

fn is_creation_failed(task: &Task) -> bool {
    task.preparing.as_ref().is_some_and(|p| p.failed)
}

/// Readable and not touched by a creation job: the facts describe a real workspace.
fn is_settled(task: &Task) -> bool {
    task.readable && task.preparing.is_none()
}

fn is_agent_running(task: &Task) -> bool {
    task.agent
        .as_ref()
        .is_some_and(|agent| agent.state == AgentPhase::Running)
}

fn is_taken_unchanged(task: &Task) -> bool {
    let is_head_taken = match (&task.taken, &task.head) {
        (Some(taken), Some(head)) => taken.sha == *head,
        _ => false,
    };
    is_head_taken && task.dirty_files.unwrap_or(0) == 0
}

#[cfg(test)]
pub(crate) mod fixtures;

#[cfg(test)]
mod tests {
    use super::fixtures::{
        cancelled_result, exited, failed_result, passed_result, preparing, readable, running,
    };
    use super::*;
    use crate::models::{BoxStatus, TakenState};

    const SHA_OLD: &str = "1111111111111111111111111111111111111111";

    fn taken_at(task: &mut Task, sha: &str) {
        task.taken = Some(TakenState {
            sha: sha.to_string(),
            at: "2026-11-10T07:00:00Z".into(),
        });
    }

    #[test]
    fn unreadable_workspace_is_da_guardare_and_illeggibile() {
        let mut task = readable("rotta");
        task.readable = false;
        assert_eq!(group_of(&task), Group::DaGuardare);
        assert_eq!(task_state(&task).id, StateId::Unreadable);
        assert_eq!(task_state(&task).tone, Tone::Warning);
    }

    #[test]
    fn task_being_prepared_is_al_lavoro_and_in_preparazione() {
        let task = preparing("new", false);
        assert_eq!(group_of(&task), Group::AlLavoro);
        assert_eq!(task_state(&task).word, Key("state.preparing"));
    }

    #[test]
    fn failed_preparation_is_da_guardare_with_a_failed_tone() {
        let task = preparing("new", true);
        assert_eq!(group_of(&task), Group::DaGuardare);
        assert_eq!(task_state(&task).id, StateId::CreationFailed);
        assert_eq!(task_state(&task).tone, Tone::Failed);
    }

    #[test]
    fn running_agent_is_al_lavoro_even_after_a_take() {
        let mut task = running("m1");
        let head = task.head.clone().unwrap();
        taken_at(&mut task, &head);
        assert_eq!(group_of(&task), Group::AlLavoro);
        assert_eq!(task_state(&task).word, Key("state.working"));
        assert_eq!(task_state(&task).tone, Tone::Running);
    }

    #[test]
    fn exited_agent_is_da_guardare() {
        let task = exited("m1");
        assert_eq!(group_of(&task), Group::DaGuardare);
        assert_eq!(task_state(&task).tone, Tone::Attention);
    }

    #[test]
    fn no_agent_and_never_taken_is_da_guardare() {
        let task = readable("m1");
        assert_eq!(group_of(&task), Group::DaGuardare);
        assert_eq!(task_state(&task).word, Key("state.to_review"));
    }

    #[test]
    fn taken_with_nothing_new_is_presi_and_preso() {
        let mut task = exited("m1");
        let head = task.head.clone().unwrap();
        taken_at(&mut task, &head);
        assert_eq!(group_of(&task), Group::Presi);
        assert_eq!(task_state(&task).word, Key("state.taken"));
        assert_eq!(task_state(&task).tone, Tone::Old);
    }

    #[test]
    fn taken_with_new_commits_since_is_da_guardare() {
        let mut task = readable("m1");
        taken_at(&mut task, SHA_OLD);
        assert_eq!(group_of(&task), Group::DaGuardare);
    }

    #[test]
    fn taken_with_uncommitted_files_since_is_da_guardare() {
        let mut task = readable("m1");
        let head = task.head.clone().unwrap();
        taken_at(&mut task, &head);
        task.dirty_files = Some(1);
        assert_eq!(group_of(&task), Group::DaGuardare);
    }

    #[test]
    fn required_verbs_passed_on_head_read_verificato() {
        let mut task = exited("m1");
        passed_result(&mut task, "test");
        passed_result(&mut task, "rules");
        assert_eq!(task_state(&task).id, StateId::Verified);
        assert_eq!(task_state(&task).tone, Tone::Passed);
    }

    #[test]
    fn green_results_with_uncommitted_files_do_not_read_verificato() {
        let mut task = exited("m1");
        passed_result(&mut task, "test");
        passed_result(&mut task, "rules");
        task.dirty_files = Some(2);
        assert_eq!(task_state(&task).id, StateId::ToReview);
    }

    #[test]
    fn green_results_with_nothing_ahead_do_not_read_verificato() {
        let mut task = exited("m1");
        passed_result(&mut task, "test");
        passed_result(&mut task, "rules");
        task.ahead = Some(0);
        assert_ne!(task_state(&task).id, StateId::Verified);
        assert!(!is_attention(&task, false, Live::Unknown));
    }

    #[test]
    fn the_checks_listed_and_required_are_the_project_s_own_in_its_order() {
        let mut task = readable("m1");
        task.checks = Some(vec!["lint".into(), "slow".into(), "test".into()]);
        let listed = |task: &Task| -> Vec<String> {
            let rows = verify_rows(task, None);
            rows.into_iter().map(|row| row.verb).collect()
        };
        assert_eq!(listed(&task), ["lint", "slow", "test"]);
        assert_eq!(required_verbs(&task), ["lint", "slow", "test"]);
        assert_ne!(take_rule(&task), TakeRule::Plain, "its checks never ran");
        for none in [Some(Vec::new()), None] {
            task.checks = none;
            assert!(listed(&task).is_empty(), "the app knows no verb by itself");
            assert_eq!(take_rule(&task), TakeRule::Plain, "nothing to wait for");
            assert_eq!(
                task_state(&task).id,
                StateId::ToReview,
                "nothing was checked: never verified"
            );
        }
    }

    #[test]
    fn empty_task_without_agent_reads_not_started_in_a_neutral_tone() {
        let mut task = readable("m1");
        task.ahead = Some(0);
        let state = task_state(&task);
        assert_eq!(state.id, StateId::NotStarted);
        assert_eq!(state.word, Key("state.not_started"));
        assert_eq!(state.tone, Tone::Neutral);
    }

    #[test]
    fn empty_task_stays_in_da_guardare_without_being_an_attention_item() {
        let mut task = readable("m1");
        task.ahead = Some(0);
        assert_eq!(group_of(&task), Group::DaGuardare);
        assert!(!is_attention(&task, false, Live::Unknown));
    }

    #[test]
    fn empty_task_with_a_result_on_head_is_still_ready_to_start() {
        let mut task = readable("m1");
        task.ahead = Some(0);
        failed_result(&mut task, "test");
        assert_eq!(task_state(&task).id, StateId::NotStarted);
    }

    #[test]
    fn uncommitted_files_alone_are_work_to_look_at() {
        let mut task = exited("m1");
        task.ahead = Some(0);
        task.dirty_files = Some(1);
        assert_eq!(task_state(&task).id, StateId::ToReview);
        assert!(is_attention(&task, false, Live::Unknown));
    }

    #[test]
    fn unknown_figures_are_not_called_empty() {
        let mut task = readable("m1");
        task.ahead = None;
        task.dirty_files = None;
        assert_ne!(task_state(&task).id, StateId::NotStarted);
    }

    #[test]
    fn stopped_agent_with_work_and_results_on_head_are_attention_items() {
        assert!(is_attention(&exited("m1"), false, Live::Unknown));
        let mut verified = exited("m2");
        passed_result(&mut verified, "test");
        passed_result(&mut verified, "rules");
        assert!(is_attention(&verified, false, Live::Unknown));
        let mut failed = exited("m3");
        failed_result(&mut failed, "test");
        assert!(is_attention(&failed, false, Live::Unknown));
    }

    #[test]
    fn running_unreadable_preparing_and_taken_tasks_are_not_attention_items() {
        let mut unreadable = readable("rotta");
        unreadable.readable = false;
        let mut taken = exited("preso");
        let head = taken.head.clone().unwrap();
        taken_at(&mut taken, &head);
        for task in [running("m1"), unreadable, preparing("new", false), taken] {
            assert!(!is_attention(&task, false, Live::Unknown), "{}", task.name);
        }
    }

    #[test]
    fn one_required_verb_failed_on_head_reads_fallito() {
        let mut task = exited("m1");
        passed_result(&mut task, "test");
        failed_result(&mut task, "rules");
        assert_eq!(task_state(&task).word, Key("state.failed"));
    }

    #[test]
    fn a_cancelled_required_verb_is_neither_verificato_nor_fallito() {
        let mut task = exited("m1");
        passed_result(&mut task, "test");
        cancelled_result(&mut task, "rules");
        if let Some(result) = task.results.get_mut("rules") {
            result.exit_code = 0;
        }
        assert_eq!(task_state(&task).id, StateId::ToReview);
    }

    #[test]
    fn green_results_of_a_commit_that_changes_the_checks_do_not_read_verificato() {
        let mut task = exited("m1");
        passed_result(&mut task, "test");
        passed_result(&mut task, "rules");
        if let Some(result) = task.results.get_mut("rules") {
            result.changes_checks = Some(true);
        }
        assert_eq!(task_state(&task).id, StateId::ToReview);
    }

    #[test]
    fn green_results_for_an_older_commit_do_not_read_verificato() {
        let mut task = exited("m1");
        passed_result(&mut task, "test");
        passed_result(&mut task, "rules");
        task.head = Some(SHA_OLD.into());
        assert_eq!(task_state(&task).id, StateId::ToReview);
    }

    #[test]
    fn grouping_keeps_sidebar_order_and_input_order() {
        let tasks = vec![readable("b"), running("c"), readable("a")];
        let groups = group_tasks(&tasks, &|_| (false, Live::Unknown));
        let names: Vec<(Group, Vec<&str>)> = groups
            .iter()
            .map(|(g, ts)| (*g, ts.iter().map(|t| t.name.as_str()).collect()))
            .collect();
        assert_eq!(
            names,
            vec![
                (Group::ServeTe, vec![]),
                (Group::AlLavoro, vec!["c"]),
                (Group::InPreparazione, vec![]),
                (Group::DaGuardare, vec!["b", "a"]),
                (Group::InAttesa, vec![]),
                (Group::DaAvviare, vec![]),
                (Group::Presi, vec![]),
            ]
        );
    }

    #[test]
    fn status_fixture_groups_as_the_window_expects() {
        let status: BoxStatus =
            serde_json::from_str(include_str!("../tests/fixtures/status.json")).unwrap();
        let groups = group_tasks(&status.tasks, &|_| (false, Live::Unknown));
        let names = |group: Group| -> Vec<&str> {
            let (_, members) = groups.iter().find(|(id, _)| *id == group).expect("group");
            members.iter().map(|t| t.name.as_str()).collect()
        };
        assert_eq!(
            names(Group::DaGuardare),
            vec!["sample-accent", "broken-check"]
        );
        assert_eq!(names(Group::AlLavoro), vec!["sample-folders"]);
        assert!(names(Group::InAttesa).is_empty());
        assert_eq!(names(Group::Presi), vec!["headings-26"]);
    }

    #[test]
    fn an_agent_that_waits_for_the_person_is_the_state_shown_whatever_the_facts_say() {
        let at_work = running("m1");
        assert_eq!(task_state(&at_work).id, StateId::Working);
        let waiting = shown_state_live(&at_work, true, Live::Unknown);
        assert_eq!(waiting.id, StateId::NeedsYou);
        assert_eq!(waiting.tone, Tone::Attention);
        assert_eq!(
            shown_state_live(&at_work, false, Live::Unknown),
            task_state(&at_work)
        );
    }

    #[test]
    fn a_task_whose_agent_waits_is_listed_in_a_group_of_its_own_before_every_other() {
        let tasks = vec![exited("done"), running("asks"), running("works")];
        let groups = group_tasks(&tasks, &|task| (task.name == "asks", Live::Unknown));
        let names = |group: Group| -> Vec<&str> {
            let (_, members) = groups.iter().find(|(id, _)| *id == group).expect("group");
            members.iter().map(|task| task.name.as_str()).collect()
        };
        assert_eq!(
            groups[0].0,
            Group::ServeTe,
            "what needs the person comes first"
        );
        assert_eq!(names(Group::ServeTe), ["asks"]);
        assert_eq!(names(Group::DaGuardare), ["done"]);
        assert_eq!(names(Group::AlLavoro), ["works"]);
    }

    /// A lavoro on the Mac as the service describes it: no agent it knows of, nothing changed yet.
    fn untouched(name: &str) -> Task {
        let mut task = readable(name);
        (task.ahead, task.dirty_files) = (Some(0), Some(0));
        task
    }

    fn group_seen(task: &Task, waits: bool, live: Live) -> Group {
        let tasks = [task.clone()];
        let groups = group_tasks(&tasks, &|_| (waits, live));
        let found = groups.iter().find(|(_, members)| !members.is_empty());
        found.map(|(group, _)| *group).expect("a group")
    }

    #[test]
    fn what_the_conversation_says_of_its_agent_is_the_state_of_a_lavoro_the_service_knows_no_agent_of(
    ) {
        let task = untouched("m1");
        let state = |waits, live| shown_state_live(&task, waits, live).id;
        assert_eq!(state(false, Live::Unknown), StateId::NotStarted);
        assert_eq!(state(false, Live::Starting), StateId::Preparing);
        assert_eq!(state(false, Live::Working), StateId::Working);
        assert_eq!(state(true, Live::Working), StateId::NeedsYou);
        assert_eq!(state(false, Live::Answered), StateId::Idle);
        assert_eq!(state(false, Live::Stopped), StateId::Failed);
    }

    #[test]
    fn a_lavoro_is_listed_under_what_its_shown_state_says_not_under_what_the_facts_alone_say() {
        let task = untouched("m1");
        assert_eq!(group_seen(&task, false, Live::Working), Group::AlLavoro);
        assert_eq!(
            group_seen(&task, false, Live::Starting),
            Group::InPreparazione,
            "being prepared is not at work yet: the group says what the state says"
        );
        assert_eq!(group_seen(&task, true, Live::Working), Group::ServeTe);
        assert_eq!(group_seen(&task, false, Live::Stopped), Group::DaGuardare);
        assert_eq!(
            group_seen(&task, false, Live::Answered),
            Group::InAttesa,
            "the agent answered and nothing is left to look at: not among the ones to review"
        );
        assert_eq!(
            group_seen(&task, false, Live::Unknown),
            Group::DaAvviare,
            "nothing was started: apart from the ones whose agent answered"
        );
        let mut changed = untouched("m2");
        changed.dirty_files = Some(2);
        assert_eq!(
            group_seen(&changed, false, Live::Answered),
            Group::DaGuardare
        );
        assert_eq!(
            group_seen(&changed, false, Live::Working),
            Group::AlLavoro,
            "its agent is still at it: the work is not to review yet"
        );
    }

    #[test]
    fn attention_follows_the_shown_state() {
        let mut changed = untouched("m1");
        changed.dirty_files = Some(2);
        assert!(is_attention(&changed, false, Live::Answered));
        assert!(
            !is_attention(&changed, false, Live::Working),
            "still at work"
        );
        assert!(
            is_attention(&untouched("m2"), true, Live::Working),
            "it asked the person"
        );
        assert!(
            is_attention(&untouched("m3"), false, Live::Stopped),
            "its agent ended badly"
        );
        assert!(
            !is_attention(&untouched("m4"), false, Live::Answered),
            "idle"
        );
    }
}
