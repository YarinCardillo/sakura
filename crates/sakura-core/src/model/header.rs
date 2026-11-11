//! What the header of a task's page shows beside its name: whether there is something to take, how many
//! files changed, and how many checks failed. Pure rules, so the header never decides them itself.

use super::result::{result_view, ResultView};
use super::take::{take_rule, TakeRule};
use crate::models::Task;

/// Whether "Take" is offered at all: only when there is something to take. It is never shown disabled.
pub fn has_something_to_take(task: &Task) -> bool {
    !matches!(take_rule(task), TakeRule::Disabled { .. })
}

/// The figure beside "Changes", when there is a file to count: the files the view lists, once the whole
/// patch of every change was read (`listed`; it counts the files not committed yet, which `box` leaves
/// out of its own figure), else the files `box` counted in the commits.
pub fn changed_files(task: &Task, listed: Option<usize>) -> Option<u32> {
    let files = match listed {
        Some(listed) => u32::try_from(listed).ok(),
        None => task.files_changed,
    };
    files.filter(|files| *files > 0)
}

/// How many checks failed on HEAD, when at least one did: the figure beside "Checks". The task's state
/// is said once, beside its name; the tab only counts.
pub fn failed_checks(task: &Task) -> Option<u32> {
    let failed = task
        .results
        .keys()
        .filter(|verb| result_view(task, verb) == ResultView::Fallito)
        .count();
    u32::try_from(failed).ok().filter(|failed| *failed > 0)
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{failed_result, passed_result, preparing, readable};
    use super::*;
    use crate::models::TakenState;

    #[test]
    fn take_is_offered_only_when_there_are_commits_to_take() {
        assert!(
            has_something_to_take(&readable("m1")),
            "three commits ahead"
        );
        let mut nothing = readable("m1");
        nothing.ahead = Some(0);
        assert!(!has_something_to_take(&nothing));
        let mut taken = readable("m1");
        taken.taken = Some(TakenState {
            sha: taken.head.clone().unwrap(),
            at: "2026-11-10T07:00:00Z".into(),
        });
        assert!(!has_something_to_take(&taken), "already taken at HEAD");
        let mut unreadable = readable("m1");
        unreadable.readable = false;
        assert!(!has_something_to_take(&unreadable));
        assert!(!has_something_to_take(&preparing("new", false)));
    }

    #[test]
    fn a_take_that_needs_a_hold_is_still_something_to_take() {
        let mut task = readable("m1");
        task.dirty_files = Some(2);
        assert!(has_something_to_take(&task));
    }

    #[test]
    fn the_changed_files_are_counted_only_above_zero() {
        assert_eq!(changed_files(&readable("m1"), None), Some(9));
        let mut none = readable("m1");
        none.files_changed = Some(0);
        assert_eq!(changed_files(&none, None), None);
        none.files_changed = None;
        assert_eq!(changed_files(&none, None), None, "not read is not a figure");
    }

    #[test]
    fn the_files_the_view_lists_are_the_figure_once_the_whole_patch_was_read() {
        let mut task = readable("m1");
        task.files_changed = Some(0);
        assert_eq!(
            changed_files(&task, Some(2)),
            Some(2),
            "two new files not committed yet: box counts none, the view lists two"
        );
        assert_eq!(changed_files(&readable("m1"), Some(10)), Some(10));
        assert_eq!(changed_files(&readable("m1"), Some(0)), None);
    }

    #[test]
    fn the_checks_that_failed_on_head_are_counted_and_passed_or_old_ones_are_not() {
        let mut task = readable("m1");
        passed_result(&mut task, "test");
        assert_eq!(failed_checks(&task), None);
        failed_result(&mut task, "rules");
        assert_eq!(failed_checks(&task), Some(1));
        failed_result(&mut task, "test");
        assert_eq!(failed_checks(&task), Some(2));
        task.head = Some("2222222222222222222222222222222222222222".into());
        assert_eq!(failed_checks(&task), None, "an old failure is not counted");
    }
}
