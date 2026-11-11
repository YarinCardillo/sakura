//! How one verification result reads for a task.

use super::trust::{distrust, Reserve};
use crate::models::{Task, VerifyResult};
use crate::said::Key;

/// The ways a verb's last result can read. A result for another commit than HEAD is [`Vecchio`],
/// whatever its exit code: an old green is never shown as green. A cancelled one is never green either.
///
/// [`Vecchio`]: ResultView::Vecchio
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResultView {
    Passato,
    Fallito,
    Vecchio,
    MaiEseguito,
    /// Cancelled on HEAD before it ended: neither passed nor failed.
    Annullata,
    /// Passed on HEAD, but what `box` recorded with it keeps it from being plain green.
    VerdeConRiserva(Reserve),
}

impl ResultView {
    /// The key of the word shown beside the glyph.
    pub fn word(self) -> Key {
        match self {
            ResultView::Passato => Key("result.passed"),
            ResultView::Fallito => Key("result.failed"),
            ResultView::Vecchio => Key("result.old"),
            ResultView::MaiEseguito => Key("result.never_run"),
            ResultView::Annullata => Key("result.cancelled"),
            ResultView::VerdeConRiserva(reserve) => reserve.word(),
        }
    }
}

/// How the last result of `verb` reads for `task`.
pub fn result_view(task: &Task, verb: &str) -> ResultView {
    match task.results.get(verb) {
        None => ResultView::MaiEseguito,
        Some(result) if !is_for_head(task, result) => ResultView::Vecchio,
        Some(result) if result.cancelled => ResultView::Annullata,
        Some(result) if result.exit_code == 0 => match distrust(verb, result) {
            Some(distrust) => ResultView::VerdeConRiserva(distrust.reserve()),
            None => ResultView::Passato,
        },
        Some(_) => ResultView::Fallito,
    }
}

/// `box` marks the result with `isHead`; the sha is compared as well so that facts read at different times
/// cannot make an old result look current.
pub(crate) fn is_for_head(task: &Task, result: &VerifyResult) -> bool {
    result.is_head && task.head.as_deref() == Some(result.sha.as_str())
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{cancelled_result, failed_result, passed_result, readable};
    use super::*;

    #[test]
    fn missing_result_reads_mai_eseguito() {
        assert_eq!(
            result_view(&readable("m1"), "test"),
            ResultView::MaiEseguito
        );
    }

    #[test]
    fn passed_result_on_head_reads_passato() {
        let mut task = readable("m1");
        passed_result(&mut task, "test");
        assert_eq!(result_view(&task, "test"), ResultView::Passato);
    }

    #[test]
    fn failed_result_on_head_reads_fallito() {
        let mut task = readable("m1");
        failed_result(&mut task, "test");
        assert_eq!(result_view(&task, "test"), ResultView::Fallito);
    }

    #[test]
    fn result_marked_not_head_reads_vecchio_even_when_green() {
        let mut task = readable("m1");
        passed_result(&mut task, "test");
        if let Some(result) = task.results.get_mut("test") {
            result.is_head = false;
        }
        assert_eq!(result_view(&task, "test"), ResultView::Vecchio);
    }

    #[test]
    fn result_for_another_sha_reads_vecchio_even_if_marked_head() {
        let mut task = readable("m1");
        failed_result(&mut task, "test");
        task.head = Some("2222222222222222222222222222222222222222".into());
        assert_eq!(result_view(&task, "test"), ResultView::Vecchio);
    }

    #[test]
    fn a_cancelled_result_on_head_reads_annullata_whatever_its_exit_code() {
        let mut task = readable("m1");
        cancelled_result(&mut task, "test");
        assert_eq!(result_view(&task, "test"), ResultView::Annullata);
        if let Some(result) = task.results.get_mut("test") {
            result.exit_code = 0;
        }
        assert_eq!(
            result_view(&task, "test"),
            ResultView::Annullata,
            "never green"
        );
        assert_eq!(ResultView::Annullata.word(), Key("result.cancelled"));
    }

    #[test]
    fn a_cancelled_result_for_an_older_commit_reads_vecchio() {
        let mut task = readable("m1");
        cancelled_result(&mut task, "test");
        task.head = Some("2222222222222222222222222222222222222222".into());
        assert_eq!(result_view(&task, "test"), ResultView::Vecchio);
    }

    #[test]
    fn a_green_result_whose_commit_changes_the_checks_never_reads_passato() {
        let mut task = readable("m1");
        passed_result(&mut task, "rules");
        if let Some(result) = task.results.get_mut("rules") {
            result.changes_checks = Some(true);
            result.checks_touched = Some(vec!["eslint.config.js".into()]);
        }
        let view = result_view(&task, "rules");
        assert_eq!(view, ResultView::VerdeConRiserva(Reserve::ChecksChanged));
        assert_eq!(view.word(), Key("reserve.checks_changed"));
    }

    #[test]
    fn a_failed_result_is_fallito_whatever_else_was_recorded() {
        let mut task = readable("m1");
        failed_result(&mut task, "test");
        if let Some(result) = task.results.get_mut("test") {
            result.changes_checks = Some(true);
        }
        assert_eq!(result_view(&task, "test"), ResultView::Fallito);
    }

    #[test]
    fn each_view_has_its_own_word() {
        assert_eq!(ResultView::MaiEseguito.word(), Key("result.never_run"));
        assert_eq!(ResultView::Vecchio.word(), Key("result.old"));
    }
}
