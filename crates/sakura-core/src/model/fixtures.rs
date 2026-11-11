//! Task builders for the model's tests.

use crate::models::{
    AgentContainer, AgentPhase, Place, Preparing, Task, TestCounts, TestsTouched, VerifyResult,
};
use std::collections::BTreeMap;

pub const HEAD: &str = "9b1e0c3a5d7f2468ace013579bdf02468ace1357";

/// A readable task with three commits ahead, nothing uncommitted, no agent, never taken, no results.
pub fn readable(name: &str) -> Task {
    Task {
        name: name.to_string(),
        profile: "example-app".into(),
        place: Place::Box,
        folder: None,
        base: "lab/example-app".into(),
        base_sha: "536ea1c688aa01b2c3d4e5f60718293a4b5c6d7e".into(),
        readable: true,
        branch: Some(format!("box/{name}")),
        head: Some(HEAD.into()),
        head_subject: Some("Subject".into()),
        head_time: Some("2026-11-10T06:43:31Z".into()),
        ahead: Some(3),
        dirty_files: Some(0),
        files_changed: Some(9),
        insertions: Some(141),
        deletions: Some(133),
        taken: None,
        agent: None,
        other_containers: 0,
        results: BTreeMap::new(),
        preparing: None,
        disk: None,
    }
}

/// A readable task whose agent container runs.
pub fn running(name: &str) -> Task {
    with_agent(readable(name), AgentPhase::Running)
}

/// A readable task whose agent container has exited.
pub fn exited(name: &str) -> Task {
    with_agent(readable(name), AgentPhase::Exited)
}

/// A synthetic task still being created, or whose creation failed.
pub fn preparing(name: &str, failed: bool) -> Task {
    let mut task = readable(name);
    task.readable = false;
    task.preparing = Some(Preparing {
        line: "Installing".into(),
        failed,
        has_workspace: false,
    });
    task
}

/// Records a passed result for HEAD that `box` vouches for: the commit leaves the checks and the existing
/// tests alone, and the test verb ran as many tests as the base, none skipped.
pub fn passed_result(task: &mut Task, verb: &str) {
    record(task, verb, 0);
    if let Some(result) = task.results.get_mut(verb) {
        vouch(verb, result);
    }
}

/// Fills in what `box` records with a result it found nothing against.
pub fn vouch(verb: &str, result: &mut VerifyResult) {
    result.changes_checks = Some(false);
    result.checks_touched = Some(Vec::new());
    result.tests_touched = Some(TestsTouched::default());
    if verb == "test" {
        result.tests = Some(TestCounts {
            ran: 7880,
            skipped: Some(0),
            base: Some(7880),
            base_skipped: Some(0),
        });
    }
}

/// Records a passed result for HEAD as an older `box` writes it: nothing about the checks or the tests.
pub fn unrecorded_result(task: &mut Task, verb: &str) {
    record(task, verb, 0);
}

/// Records a result for HEAD of a check that was cancelled while it ran.
pub fn cancelled_result(task: &mut Task, verb: &str) {
    record(task, verb, 130);
    if let Some(result) = task.results.get_mut(verb) {
        result.cancelled = true;
    }
}

/// Records a failed result for HEAD.
pub fn failed_result(task: &mut Task, verb: &str) {
    record(task, verb, 1);
}

fn record(task: &mut Task, verb: &str, exit_code: i32) {
    let result = VerifyResult {
        sha: HEAD.into(),
        exit_code,
        seconds: 38.0,
        finished_at: "2026-11-10T06:44:40Z".into(),
        is_head: true,
        place: Place::Box,
        cancelled: false,
        changes_checks: None,
        checks_touched: None,
        checks_reason: None,
        tests_touched: None,
        tests: None,
    };
    task.results.insert(verb.to_string(), result);
}

fn with_agent(mut task: Task, state: AgentPhase) -> Task {
    task.agent = Some(AgentContainer {
        container: format!("agentbox-agent-{}", task.name),
        account: "work".into(),
        state,
        cpu_percent: None,
        memory_mb: None,
        started_at: "2026-11-10T05:12:40Z".into(),
        exit_code: None,
    });
    task
}
