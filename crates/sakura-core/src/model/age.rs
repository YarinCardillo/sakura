//! A task's own age, independent of when its facts were fetched.

use super::{StateId, TaskState};
use crate::models::Task;
use crate::said::Said;
use std::time::Duration;

const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 3600;

/// The words of an age and when its next whole unit changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAge {
    pub at: i64,
    pub said: Said,
    pub refresh_in: Duration,
}

/// The time since the last completed turn, or creation for an unstarted task.
/// Missing timestamps and working tasks have no age.
pub fn task_age(task: &Task, shown: TaskState, now: i64) -> Option<TaskAge> {
    if shown.id == StateId::Working {
        return None;
    }
    let at = crate::graph::parse_time(task.age_at.as_deref()?)?;
    Some(TaskAge::at(at, now))
}

impl TaskAge {
    /// An age from persistent UTC seconds, clamping a clock in the future to zero.
    pub fn at(at: i64, now: i64) -> Self {
        let seconds = now.saturating_sub(at).max(0) as u64;
        let unit = if seconds < SECONDS_PER_HOUR {
            SECONDS_PER_MINUTE
        } else {
            SECONDS_PER_HOUR
        };
        let (key, count) = match seconds {
            _ if seconds < SECONDS_PER_MINUTE => ("age.seconds", seconds),
            _ if seconds < SECONDS_PER_HOUR => ("age.minutes", seconds / SECONDS_PER_MINUTE),
            _ => ("age.hours", seconds / SECONDS_PER_HOUR),
        };
        Self {
            at,
            said: Said::new(key).number("n", count),
            refresh_in: Duration::from_secs(unit - seconds % unit),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::parse_time;
    use crate::model::{fixtures, shown_state_live, Live};

    fn age(at: &str, now: i64) -> Option<TaskAge> {
        let mut task = fixtures::readable("task");
        task.age_at = Some(at.into());
        task_age(&task, shown_state_live(&task, false, Live::Unknown), now)
    }

    #[test]
    fn row_ages_are_the_tasks_own_and_survive_a_relaunch() {
        let now = parse_time("2026-11-11T12:00:00Z").unwrap();
        let old = age("2026-11-11T10:00:00Z", now).unwrap();
        let recent = age("2026-11-11T11:58:00Z", now).unwrap();
        assert_eq!(old.said, Said::new("age.hours").number("n", 2));
        assert_eq!(recent.said, Said::new("age.minutes").number("n", 2));
        let mut task = fixtures::readable("task");
        task.age_at = Some("2026-11-11T11:58:00Z".into());
        let relaunched: Task =
            serde_json::from_str(&serde_json::to_string(&task).unwrap()).unwrap();
        assert_eq!(
            task_age(
                &relaunched,
                shown_state_live(&relaunched, false, Live::Unknown),
                now
            ),
            Some(recent)
        );
    }

    #[test]
    fn row_age_includes_a_turn_that_just_ended() {
        let now = parse_time("2026-11-11T12:00:00Z").unwrap();
        assert_eq!(
            age("2026-11-11T12:00:00Z", now).unwrap().said,
            Said::new("age.seconds").number("n", 0)
        );
        assert_eq!(
            age("2026-11-11T11:59:55Z", now).unwrap().said,
            Said::new("age.seconds").number("n", 5)
        );
        assert_eq!(
            age("2026-11-11T12:01:00Z", now).unwrap().said,
            Said::new("age.seconds").number("n", 0)
        );
    }

    #[test]
    fn row_age_is_absent_while_an_agent_works_in_either_place() {
        let mut task = fixtures::running("task");
        task.age_at = Some("2026-11-11T10:00:00Z".into());
        assert!(task_age(
            &task,
            shown_state_live(&task, false, Live::Unknown),
            i64::MAX
        )
        .is_none());
        task.agent = None;
        assert!(task_age(
            &task,
            shown_state_live(&task, false, Live::Working),
            i64::MAX
        )
        .is_none());
    }

    #[test]
    fn row_age_never_falls_back_to_read_or_check_times() {
        let mut task = fixtures::readable("task");
        fixtures::passed_result(&mut task, "test");
        let shown = shown_state_live(&task, false, Live::Unknown);
        assert!(task_age(&task, shown, 100).is_none());
        task.age_at = Some("bad time".into());
        assert!(task_age(&task, shown, 100).is_none());
    }

    #[test]
    fn row_age_wakes_at_minute_then_hour_boundaries() {
        let at = "1970-01-01T00:00:00Z";
        for (now, delay) in [
            (0, 60),
            (5, 55),
            (59, 1),
            (60, 60),
            (3599, 1),
            (3600, 3600),
            (7201, 3599),
        ] {
            assert_eq!(age(at, now).unwrap().refresh_in, Duration::from_secs(delay));
        }
    }
}
