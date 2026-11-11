//! What the window opens on: the task it showed last when it still exists, else the first row of "Da
//! guardare", else the first row. The choice is made once, as soon as it can be made rightly: the last task
//! is known from its name alone; the groups only once every row's facts were read.

use crate::data::facts::Row;
use sakura_core::model::{self, Group};

/// What to show when nothing is selected yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opening {
    /// Show this task.
    Show(String),
    /// Some rows are still being read, so the groups are not known: choose when they are.
    Wait,
    /// There is no task at all.
    Nothing,
}

/// The opening for `rows`, given the task shown `last`.
pub fn opening(last: Option<&str>, rows: &[Row]) -> Opening {
    if rows.is_empty() {
        return Opening::Nothing;
    }
    let is_listed = |name: &str| rows.iter().any(|row| row.task.name == name);
    if let Some(last) = last.filter(|name| is_listed(name)) {
        return Opening::Show(last.to_string());
    }
    if rows.iter().any(|row| row.is_pending) {
        return Opening::Wait;
    }
    let to_review = rows
        .iter()
        .find(|row| model::group_of(&row.task) == Group::DaGuardare);
    let first = to_review.unwrap_or(&rows[0]);
    Opening::Show(first.task.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakura_core::models::BoxStatus;
    use std::time::Instant;

    /// The fixture's rows: "sample-folders" (at work) first, then "sample-accent" (to look at).
    fn rows() -> Vec<Row> {
        let status: BoxStatus = serde_json::from_str(include_str!(
            "../../../sakura-core/tests/fixtures/status.json"
        ))
        .unwrap();
        let at = Instant::now();
        let row = |task| Row {
            task,
            fetched_at: at,
            work_read_at: at,
            requested_at: at,
            is_refreshing: false,
            is_pending: false,
        };
        status.tasks.into_iter().map(row).collect()
    }

    fn show(name: &str) -> Opening {
        Opening::Show(name.into())
    }

    #[test]
    fn the_task_shown_last_wins_even_while_its_state_is_being_read() {
        let mut rows = rows();
        rows.iter_mut().for_each(|row| row.is_pending = true);
        assert_eq!(opening(Some("headings-26"), &rows), show("headings-26"));
    }

    #[test]
    fn without_a_last_task_the_first_row_of_da_guardare_opens() {
        let rows = rows();
        assert_eq!(rows[0].task.name, "sample-folders", "not the first row");
        assert_eq!(opening(None, &rows), show("sample-accent"));
    }

    #[test]
    fn a_last_task_that_no_longer_exists_is_ignored() {
        assert_eq!(opening(Some("gone"), &rows()), show("sample-accent"));
    }

    #[test]
    fn with_nothing_to_look_at_the_first_row_opens() {
        let rows: Vec<Row> = rows()
            .into_iter()
            .filter(|row| model::group_of(&row.task) != Group::DaGuardare)
            .collect();
        assert_eq!(opening(None, &rows), show(&rows[0].task.name));
    }

    #[test]
    fn the_groups_are_not_guessed_while_a_row_is_being_read() {
        let mut rows = rows();
        rows[2].is_pending = true;
        assert_eq!(opening(None, &rows), Opening::Wait);
        assert_eq!(opening(Some("gone"), &rows), Opening::Wait);
    }

    #[test]
    fn no_task_at_all_opens_on_nothing() {
        assert_eq!(opening(Some("m1"), &[]), Opening::Nothing);
    }
}
