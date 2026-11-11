//! When to refetch what, as pure functions. A full `box status` starts a container per task, so it runs
//! only at start, when the journal shows that containers or the task set changed, or while `box` is
//! unusable; one task is refetched when its files change or the journal names it. The journal itself is
//! read when its file changes, never on a timer.

use sakura_core::models::{AgentPhase, JournalEvent, JournalEventKind, Task};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// A running agent that no terminal of the window is attached to is read again when its facts are older
/// than this, so an agent that exits on its own (which no file tells, only a `box facts` call) leaves "Al
/// lavoro" within about a minute. It is the only read requested by time.
pub const RUNNING_FACTS_MAX_AGE: Duration = Duration::from_secs(60);

/// What the journal events since the last look ask for.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RefreshPlan {
    /// Refetch everything (`box status`).
    pub is_full: bool,
    /// Refetch these tasks (`box facts`).
    pub tasks: BTreeSet<String>,
    /// The newest event time seen, the cursor for the next look.
    pub cursor: Option<String>,
}

/// The plan for `events` read after `cursor`. Events at or before the cursor were seen already (`--since`
/// includes the boundary). Agent start and exit, new and removed tasks, and verification requests (the runner
/// line) need a full refetch; a take changes one task.
pub fn plan(events: &[JournalEvent], cursor: Option<&str>) -> RefreshPlan {
    let fresh = events
        .iter()
        .filter(|event| cursor.is_none_or(|seen| event.at.as_str() > seen));
    let mut plan = RefreshPlan {
        cursor: cursor.map(str::to_string),
        ..RefreshPlan::default()
    };
    for event in fresh {
        match event.event {
            JournalEventKind::Take => {
                plan.tasks.insert(event.task.clone());
            }
            _ => plan.is_full = true,
        }
        if plan
            .cursor
            .as_deref()
            .is_none_or(|seen| event.at.as_str() > seen)
        {
            plan.cursor = Some(event.at.clone());
        }
    }
    plan
}

/// The newest event time, to start the cursor without acting on history.
pub fn newest(events: &[JournalEvent]) -> Option<String> {
    events.iter().map(|event| event.at.clone()).max()
}

/// The running task whose facts are oldest past [`RUNNING_FACTS_MAX_AGE`], if any, skipping those already
/// being refetched.
pub fn stale_running<'a>(
    rows: impl IntoIterator<Item = (&'a Task, Instant, bool)>,
    now: Instant,
) -> Option<String> {
    rows.into_iter()
        .filter(|(task, _, is_refreshing)| !is_refreshing && is_running(task))
        .filter(|(_, fetched_at, _)| now.duration_since(*fetched_at) > RUNNING_FACTS_MAX_AGE)
        .min_by_key(|(_, fetched_at, _)| *fetched_at)
        .map(|(task, _, _)| task.name.clone())
}

fn is_running(task: &Task) -> bool {
    task.agent
        .as_ref()
        .is_some_and(|agent| agent.state == AgentPhase::Running)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakura_core::models::BoxStatus;

    fn event(at: &str, kind: JournalEventKind, task: &str) -> JournalEvent {
        JournalEvent {
            at: at.into(),
            event: kind,
            task: task.into(),
            verb: None,
            sha: None,
            exit_code: None,
            account: None,
        }
    }

    fn tasks() -> Vec<Task> {
        let status: BoxStatus = serde_json::from_str(include_str!(
            "../../../sakura-core/tests/fixtures/status.json"
        ))
        .unwrap();
        status.tasks
    }

    #[test]
    fn agent_exit_after_the_cursor_asks_for_a_full_refresh() {
        let events = [event(
            "2026-11-10T07:00:01Z",
            JournalEventKind::AgentExit,
            "m1",
        )];
        let plan = plan(&events, Some("2026-11-10T07:00:00Z"));
        assert!(plan.is_full);
        assert_eq!(plan.cursor.as_deref(), Some("2026-11-10T07:00:01Z"));
    }

    #[test]
    fn events_at_or_before_the_cursor_are_ignored() {
        let events = [event(
            "2026-11-10T07:00:00Z",
            JournalEventKind::AgentStart,
            "m1",
        )];
        let plan = plan(&events, Some("2026-11-10T07:00:00Z"));
        assert_eq!(
            plan,
            RefreshPlan {
                cursor: Some("2026-11-10T07:00:00Z".into()),
                ..RefreshPlan::default()
            }
        );
    }

    #[test]
    fn a_take_refreshes_only_its_task() {
        let events = [event("2026-11-10T07:00:05Z", JournalEventKind::Take, "m1")];
        let plan = plan(&events, Some("2026-11-10T07:00:00Z"));
        assert!(!plan.is_full);
        assert_eq!(
            plan.tasks.into_iter().collect::<Vec<_>>(),
            vec!["m1".to_string()]
        );
    }

    #[test]
    fn verification_events_refresh_everything_for_the_runner_line() {
        let events = [event(
            "2026-11-10T07:00:05Z",
            JournalEventKind::VerifyQueued,
            "m1",
        )];
        assert!(plan(&events, Some("2026-11-10T07:00:00Z")).is_full);
    }

    #[test]
    fn newest_starts_the_cursor_at_the_latest_event() {
        let events = [
            event("2026-11-10T07:00:05Z", JournalEventKind::Take, "m1"),
            event("2026-11-10T06:00:00Z", JournalEventKind::New, "m2"),
        ];
        assert_eq!(newest(&events).as_deref(), Some("2026-11-10T07:00:05Z"));
        assert_eq!(newest(&[]), None);
    }

    #[test]
    fn stale_running_picks_the_oldest_running_task_past_the_limit() {
        let tasks = tasks();
        let now = Instant::now() + Duration::from_secs(600);
        let old = now - Duration::from_secs(120);
        let rows = tasks.iter().map(|task| (task, old, false));
        assert_eq!(stale_running(rows, now).as_deref(), Some("sample-folders"));
    }

    #[test]
    fn stale_running_skips_fresh_and_refreshing_rows() {
        let tasks = tasks();
        let now = Instant::now() + Duration::from_secs(600);
        let fresh = tasks.iter().map(|task| (task, now, false));
        assert_eq!(stale_running(fresh, now), None);
        let busy = tasks
            .iter()
            .map(|task| (task, now - Duration::from_secs(120), true));
        assert_eq!(stale_running(busy, now), None);
    }
}
