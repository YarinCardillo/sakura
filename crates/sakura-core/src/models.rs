//! Types of the service interface. Field names are camelCase on the wire. A test
//! parses `tests/fixtures/status.json` into these and serialises it back, so the two sides cannot drift.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedRequest {
    pub task: String,
    pub verb: String,
    pub sha: String,
    pub queued_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningRequest {
    pub task: String,
    pub verb: String,
    pub sha: String,
    pub started_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunnerPhase {
    Running,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunnerState {
    pub state: RunnerPhase,
    pub current: Option<RunningRequest>,
    pub queue: Vec<QueuedRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResult {
    pub sha: String,
    pub exit_code: i32,
    pub seconds: f64,
    pub finished_at: String,
    pub is_head: bool,
    /// Where the check ran: in a box, or in the task's own folder on the Mac, where nothing keeps the
    /// agent from what the check reads. A `box` that says nothing ran it in a box.
    #[serde(default, rename = "where", skip_serializing_if = "Place::is_box")]
    pub place: Place,
    /// The check was cancelled (`box cancel`) before it ended: its exit code says nothing about the work.
    #[serde(
        default,
        deserialize_with = "null_as_false",
        skip_serializing_if = "is_false"
    )]
    pub cancelled: bool,
    /// Whether the commit that was checked changes the files that define the checks; `None` when `box` did
    /// not record it (an older `box`, or a result that ran nothing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes_checks: Option<bool>,
    /// Those files, at most twenty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks_touched: Option<Vec<String>>,
    /// Why `box` could not tell whether the checks changed, when it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks_reason: Option<String>,
    /// The test files that existed at the base and that the commit modified or deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests_touched: Option<TestsTouched>,
    /// For the test verb, how many tests ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<TestCounts>,
}

/// The existing test files a commit changed: how many it modified, how many it deleted, and which (at
/// most twenty).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestsTouched {
    pub modified: u32,
    pub deleted: u32,
    #[serde(default)]
    pub files: Vec<String>,
}

/// How many tests a test result ran, against the task's base.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestCounts {
    /// Tests that passed or failed.
    pub ran: u64,
    /// Tests that were skipped or left to do.
    #[serde(default)]
    pub skipped: Option<u64>,
    /// The same count in the last test result of the base commit; `None` when there is none.
    #[serde(default)]
    pub base: Option<u64>,
    /// How many the base skips; `None` when there is no result for the base.
    #[serde(default)]
    pub base_skipped: Option<u64>,
}

/// `null` where a flag is expected reads as false.
fn null_as_false<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(deserializer)?.unwrap_or(false))
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentPhase {
    Running,
    Exited,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentContainer {
    pub container: String,
    pub account: String,
    pub state: AgentPhase,
    pub cpu_percent: Option<f64>,
    pub memory_mb: Option<u64>,
    pub started_at: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakenState {
    pub sha: String,
    pub at: String,
}

/// Progress of a `task_new` job whose result is not final yet. An addition to the contract, see
/// `CONTRACT-ADDITIONS.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preparing {
    pub line: String,
    pub failed: bool,
    /// Whether `box` lists the task, so its workspace exists and may hold work; false for the synthetic entry.
    #[serde(rename = "hasWorkspace", default)]
    pub has_workspace: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub name: String,
    pub profile: String,
    /// Where the lavoro lives; a `box` from before places existed says nothing, and that is a box.
    #[serde(default, rename = "where", skip_serializing_if = "Place::is_box")]
    pub place: Place,
    /// The worktree of a lavoro on the Mac, as an absolute path: the working directory of its agent.
    /// `None` for a lavoro in a box, whose agent works in `/work/<task>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    pub base: String,
    pub base_sha: String,
    pub readable: bool,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub head_subject: Option<String>,
    pub head_time: Option<String>,
    pub ahead: Option<u32>,
    pub dirty_files: Option<u32>,
    pub files_changed: Option<u32>,
    pub insertions: Option<u32>,
    pub deletions: Option<u32>,
    pub taken: Option<TakenState>,
    pub agent: Option<AgentContainer>,
    pub other_containers: u32,
    pub results: BTreeMap<String, VerifyResult>,
    /// The checks of the task's project, by their verbs, as `box` lists them and in its order; `None`
    /// from a `box` that does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparing: Option<Preparing>,
    /// What a task in a box holds on disk against its budget, as the service measured it; `None` for a
    /// task on the Mac, when the measure failed, or from a service that does not measure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk: Option<DiskUse>,
}

/// The disk use of a task in a box against its budget (`disk` of `box status` and `box facts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUse {
    pub bytes: u64,
    pub entries: u64,
    pub limit_bytes: u64,
    pub entry_limit: u64,
    /// The task is over its budget of bytes or of entries: the service refuses to start work in it.
    pub over_budget: bool,
}

/// One element of `box tasks --json`: what is known of a task without reading its workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSummary {
    pub name: String,
    pub profile: String,
    #[serde(default, rename = "where")]
    pub place: Place,
    pub base: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoxStatus {
    pub runner: RunnerState,
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    pub time: String,
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JournalEventKind {
    New,
    AgentStart,
    AgentExit,
    VerifyQueued,
    VerifyDone,
    Take,
    Rm,
    Cancel,
    /// An event this version does not know: it still means something changed.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEvent {
    pub at: String,
    pub event: JournalEventKind,
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub name: String,
    /// Which lavori it serves: an account of the boxes, or one of the person's own logins on the Mac.
    #[serde(default, rename = "where")]
    pub place: Place,
    /// Whether it is signed in. `box` says nothing (`null`) for an account of the Mac: it never opens
    /// the files of a login.
    #[serde(default, deserialize_with = "null_as_false")]
    pub logged_in: bool,
    /// For an account of the Mac: whether its folder is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
}

impl Account {
    /// Whether an agent can be started with it, as far as `box` can tell: a box account that is signed
    /// in, or a login of the Mac whose folder exists.
    pub fn is_usable(&self) -> bool {
        match self.place {
            Place::Box => self.logged_in,
            Place::Mac => self.exists.unwrap_or(false),
        }
    }
}

/// Where a lavoro lives, and which lavori an account serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    /// On the Mac: a worktree of the person's repository, the agent running as the person.
    Mac,
    /// In a box: a container with limits.
    #[default]
    Box,
}

impl Place {
    /// Whether this is a box: what a `box` that says nothing of places means, and so not written out.
    pub fn is_box(&self) -> bool {
        *self == Place::Box
    }

    /// The word `box` knows the place by.
    pub fn word(self) -> &'static str {
        match self {
            Place::Mac => "mac",
            Place::Box => "box",
        }
    }

    /// The folder the agent of `task` works in, as the agent names it.
    pub fn workspace(self, task: &str, folder: Option<&str>) -> String {
        match (self, folder) {
            (Place::Mac, Some(folder)) => folder.to_string(),
            _ => format!("/work/{task}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryPressure {
    Normal,
    Warn,
    Critical,
}

/// Read by the core from macOS itself, not from `box`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub cpus: u32,
    pub load1: f64,
    pub memory_pressure: MemoryPressure,
    pub swap_used_mb: u64,
    pub swap_total_mb: u64,
    pub disk_free_gb: f64,
}

/// One plan's weekly use, from the Leeway service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsage {
    pub label: String,
    pub weekly_percent: f64,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalKind {
    Agent,
    Login,
    Shell,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalRequest {
    pub kind: TerminalKind,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

/// Payload of the `facts-changed` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FactsChanged {
    pub task: Option<String>,
}

/// Payload of the `terminal-exit` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalExit {
    pub id: String,
    pub exit_code: Option<i32>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_disk_use_of_a_task_in_a_box_is_read_as_the_service_writes_it() {
        let written = r#"{"bytes":123456,"entries":123,"limitBytes":21474836480,"entryLimit":1000000,"overBudget":false,"enforcement":"detection"}"#;
        let disk: DiskUse = serde_json::from_str(written).unwrap();
        assert_eq!(disk.bytes, 123_456);
        assert_eq!(disk.entries, 123);
        assert_eq!(disk.limit_bytes, 21_474_836_480);
        assert_eq!(disk.entry_limit, 1_000_000);
        assert!(!disk.over_budget);
        let none: Option<DiskUse> = serde_json::from_str("null").unwrap();
        assert_eq!(none, None, "a task on the Mac, or a measure that failed");
    }

    use super::*;
    use serde_json::Value;

    const FIXTURE: &str = include_str!("../tests/fixtures/status.json");

    /// Equality that treats `412` and `412.0` as the same number.
    fn same_json(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_json(x, y))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(key, x)| b.get(key).is_some_and(|y| same_json(x, y)))
            }
            _ => left == right,
        }
    }

    #[test]
    fn status_fixture_round_trips_to_equal_json() {
        let original: Value = serde_json::from_str(FIXTURE).unwrap();
        let status: BoxStatus = serde_json::from_str(FIXTURE).unwrap();
        let written = serde_json::to_value(&status).unwrap();
        assert!(same_json(&original, &written), "{written:#}");
    }

    #[test]
    fn status_fixture_has_the_shapes_the_window_relies_on() {
        let status: BoxStatus = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(status.runner.state, RunnerPhase::Running);
        assert!(status.tasks.iter().any(|task| !task.readable));
        assert!(status.tasks.iter().any(|task| task.taken.is_some()));
        assert!(status.tasks.iter().any(|task| task.agent.is_none()));
    }

    #[test]
    fn task_summary_reads_the_inventory_and_ignores_fields_it_does_not_use() {
        let json = r#"[{"name":"m1","profile":"example-app","base":"main","taken":null},
            {"name":"m2","profile":"p","base":"dev","taken":{"sha":"abc1234","at":"2026-11-10T07:00:00Z"}}]"#;
        let tasks: Vec<TaskSummary> = serde_json::from_str(json).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[1].name, "m2");
        assert_eq!(tasks[1].base, "dev");
    }

    #[test]
    fn a_result_says_whether_it_was_cancelled_and_older_results_were_not() {
        let cancelled = r#"{"sha":"abc1234","exitCode":130,"seconds":4.0,"finishedAt":"x","isHead":true,"cancelled":true}"#;
        let result: VerifyResult = serde_json::from_str(cancelled).unwrap();
        assert!(result.cancelled);
        let older =
            r#"{"sha":"abc1234","exitCode":0,"seconds":4.0,"finishedAt":"x","isHead":true}"#;
        let result: VerifyResult = serde_json::from_str(older).unwrap();
        assert!(!result.cancelled);
        assert_eq!(serde_json::to_string(&result).unwrap(), older);
    }

    #[test]
    fn a_result_reads_what_box_recorded_about_the_checks_and_the_tests() {
        let json = r#"{"sha":"abc1234","exitCode":0,"seconds":412,"finishedAt":"x","cancelled":false,
            "changesChecks":true,"checksTouched":["vitest.config.ts"],"checksReason":null,
            "testsTouched":{"modified":0,"deleted":0,"files":[]},
            "tests":{"ran":7880,"skipped":0,"base":null,"baseSkipped":0},"isHead":true}"#;
        let result: VerifyResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.changes_checks, Some(true));
        assert_eq!(
            result.checks_touched.as_deref(),
            Some(&["vitest.config.ts".to_string()][..])
        );
        let tests = result.tests.unwrap();
        assert_eq!((tests.ran, tests.base), (7880, None));
        assert_eq!((tests.skipped, tests.base_skipped), (Some(0), Some(0)));
        let touched = result.tests_touched.unwrap();
        assert_eq!((touched.modified, touched.deleted), (0, 0));
    }

    #[test]
    fn a_result_from_before_the_fields_existed_has_none_of_them() {
        let json = r#"{"sha":"abc1234","exitCode":0,"seconds":4,"finishedAt":"x","isHead":true,
            "cancelled":null,"changesChecks":null,"checksTouched":null,"tests":null}"#;
        let result: VerifyResult = serde_json::from_str(json).unwrap();
        assert!(!result.cancelled);
        assert_eq!(result.changes_checks, None);
        assert_eq!(result.checks_touched, None);
        assert_eq!(result.tests, None);
    }

    #[test]
    fn the_journal_reads_a_cancel_and_events_it_does_not_know() {
        let line = r#"[{"at":"2026-11-10T07:02:00Z","event":"cancel","task":"m1","verb":"test"},
            {"at":"2026-11-10T07:03:00Z","event":"something-new","task":"m1"}]"#;
        let events: Vec<JournalEvent> = serde_json::from_str(line).unwrap();
        assert_eq!(events[0].event, JournalEventKind::Cancel);
        assert_eq!(events[1].event, JournalEventKind::Unknown);
    }

    #[test]
    fn journal_event_omits_absent_fields() {
        let line = r#"{"at":"2026-11-10T07:02:00Z","event":"verify-done","task":"m1","verb":"test","exitCode":0}"#;
        let event: JournalEvent = serde_json::from_str(line).unwrap();
        assert_eq!(event.event, JournalEventKind::VerifyDone);
        assert_eq!(serde_json::to_string(&event).unwrap(), line);
    }

    #[test]
    fn terminal_request_accepts_the_window_shape() {
        let json = r#"{"kind":"agent","task":"m1","cols":120,"rows":32}"#;
        let request: TerminalRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.kind, TerminalKind::Agent);
        assert_eq!(request.account, None);
    }

    #[test]
    fn machine_serialises_camel_case_with_lowercase_pressure() {
        let machine = Machine {
            cpus: 8,
            load1: 1.5,
            memory_pressure: MemoryPressure::Warn,
            swap_used_mb: 1,
            swap_total_mb: 2,
            disk_free_gb: 3.5,
        };
        let value = serde_json::to_value(&machine).unwrap();
        assert_eq!(value["memoryPressure"], "warn");
        assert_eq!(value["swapUsedMb"], 1);
        assert_eq!(value["diskFreeGb"], 3.5);
    }

    #[test]
    fn a_task_says_where_it_lives_and_one_from_before_places_existed_is_in_a_box() {
        let on_the_mac = r#"{"name":"m1","profile":"app","where":"mac","base":"main"}"#;
        let summary: TaskSummary = serde_json::from_str(on_the_mac).unwrap();
        assert_eq!(summary.place, Place::Mac);
        let older = r#"{"name":"m1","profile":"app","base":"main"}"#;
        let summary: TaskSummary = serde_json::from_str(older).unwrap();
        assert_eq!(summary.place, Place::Box);
    }

    #[test]
    fn the_agent_works_in_the_worktree_on_the_mac_and_under_work_in_a_box() {
        let folder = Some("/home/x/agentbox-home/work/m1");
        assert_eq!(
            Place::Mac.workspace("m1", folder),
            "/home/x/agentbox-home/work/m1"
        );
        assert_eq!(Place::Box.workspace("m1", None), "/work/m1");
        assert_eq!(Place::Mac.workspace("m1", None), "/work/m1", "not read yet");
    }

    #[test]
    fn an_account_of_the_mac_is_usable_when_its_folder_exists_and_one_of_the_boxes_when_signed_in()
    {
        let listed = r#"[
            {"name":"personal","where":"box","loggedIn":true,"authMethod":"claude.ai","checkedAt":"x"},
            {"name":"signed-out","where":"box","loggedIn":false},
            {"name":"personal","where":"mac","path":"/home/x/.claude","exists":true,"loggedIn":null},
            {"name":"old","loggedIn":true}
        ]"#;
        let accounts: Vec<Account> = serde_json::from_str(listed).unwrap();
        let usable: Vec<bool> = accounts.iter().map(Account::is_usable).collect();
        assert_eq!(usable, [true, false, true, true]);
        assert_eq!(accounts[2].place, Place::Mac);
        assert_eq!(
            accounts[3].place,
            Place::Box,
            "an older box lists box accounts only"
        );
    }
}
