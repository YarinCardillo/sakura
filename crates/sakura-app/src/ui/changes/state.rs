//! What the changes of a task show when there is no list to scroll: one quiet sentence. The list itself shows
//! the last patch read, with one line above it when `box` showed only part of it and one when the latest read
//! failed.

use crate::data::changes::Changes;
use crate::data::facts::Facts;
use crate::l10n::{say, tr, tr_with, Key, Said};
use crate::theme::{self, Palette};
use crate::ui::centred::centred;
use gpui::{div, AnyElement, InteractiveElement as _, IntoElement, ParentElement, Styled};
use sakura_core::patch::Patch;
use std::time::Instant;

/// The state of a task's changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pane {
    /// The workspace cannot be read, so neither can its changes.
    Unreadable,
    /// Nothing read yet.
    Loading,
    /// The first read failed.
    Failed(Said),
    /// Read, and nothing differs from the base.
    Empty { base: String },
    /// Files to list; `failure` when the latest read failed and the list is older, `is_partial` when `box`
    /// showed only part of the patch.
    Files {
        failure: Option<Said>,
        is_partial: bool,
    },
}

/// The line above a patch `box` cut short: what it lists is not every change, and its figures are not totals.
pub const PARTIAL_SENTENCE: Key = Key("changes.partial");

/// The state of `task`'s changes from what the facts and the reads know.
pub fn of(task: &str, facts: &Facts, changes: &Changes) -> Pane {
    let row = facts.row(task);
    if let Some(known) = row.and_then(|row| without_a_read(&row.task)) {
        return known;
    }
    let base = row.map(|row| row.task.base.clone()).unwrap_or_default();
    let Some(read) = changes.task(task) else {
        return Pane::Loading;
    };
    from_read(
        read.patch().map(|patch| &**patch),
        read.failure(Instant::now()),
        base,
    )
}

/// What is known of a task's changes without reading them: a lavoro that is being made has none yet (its
/// folder is not there to read, which is not a fault), and one whose folder cannot be read has none to
/// read.
fn without_a_read(task: &sakura_core::models::Task) -> Option<Pane> {
    if task.preparing.is_some() {
        return Some(Pane::Empty {
            base: String::new(),
        });
    }
    (!task.readable).then_some(Pane::Unreadable)
}

/// The state from the last patch read, the latest read's failure and the task's base.
fn from_read(patch: Option<&Patch>, failure: Option<Said>, base: String) -> Pane {
    match (patch, failure) {
        (None, Some(failure)) => Pane::Failed(failure),
        (None, None) => Pane::Loading,
        (Some(patch), None) if patch.files.is_empty() && patch.is_complete => Pane::Empty { base },
        (Some(patch), failure) => Pane::Files {
            failure,
            is_partial: !patch.is_complete,
        },
    }
}

/// The sentence for a state without a list.
pub fn sentence(pane: &Pane) -> String {
    match pane {
        Pane::Unreadable => tr(Key("changes.unreadable")).into(),
        Pane::Loading => tr(Key("changes.reading")).into(),
        Pane::Failed(failure) => say(failure).to_string(),
        Pane::Empty { base } if base.is_empty() => tr(Key("changes.none")).into(),
        Pane::Empty { base } => tr_with(Key("changes.none_since"), &[("base", base)]).to_string(),
        Pane::Files { .. } => String::new(),
    }
}

/// The sentence centred in the pane.
pub fn sentence_view(pane: Pane, palette: &Palette) -> AnyElement {
    let said = div()
        .debug_selector(|| "pane-sentence".into())
        .child(sentence(&pane));
    centred(said)
        .px(theme::SIDE_PANE_INSET)
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_tertiary)
        .into_any_element()
}

/// The line above an older list when the latest read failed.
pub fn failure_line(failure: &Said, palette: &Palette) -> AnyElement {
    notice_line(say(failure).to_string(), palette)
}

/// The line above a patch `box` cut short.
pub fn partial_line(palette: &Palette) -> AnyElement {
    notice_line(tr(PARTIAL_SENTENCE).to_string(), palette)
}

fn notice_line(text: String, palette: &Palette) -> AnyElement {
    div()
        .flex_none()
        .h(theme::TOOLBAR_HEIGHT)
        .pl(theme::SIDE_PANE_INSET)
        .pr(theme::CONTENT_PADDING)
        .flex()
        .items_center()
        .border_b_1()
        .border_color(palette.border)
        .text_size(theme::TEXT_LABEL)
        .text_color(palette.warning)
        .child(div().min_w_0().truncate().child(text))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_changes_name_the_base_in_one_sentence() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let pane = Pane::Empty {
            base: "lab/example-app".into(),
        };
        assert_eq!(
            sentence(&pane),
            "Nessuna modifica rispetto a lab/example-app"
        );
    }

    #[test]
    fn a_patch_box_cut_short_is_listed_as_partial() {
        let cut = Patch::parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,5 +1,5 @@\n x\n");
        let pane = from_read(Some(&cut), None, "main".into());
        assert_eq!(
            pane,
            Pane::Files {
                failure: None,
                is_partial: true
            }
        );
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        assert_eq!(
            tr(PARTIAL_SENTENCE),
            "Modifiche parziali: il servizio ne ha mostrato solo una parte"
        );
    }

    #[test]
    fn a_whole_patch_is_not_partial_and_an_empty_cut_one_is_not_empty() {
        let whole = Patch::parse(include_str!(
            "../../../../sakura-core/tests/fixtures/changes.patch"
        ));
        let pane = from_read(Some(&whole), None, "main".into());
        assert!(matches!(
            pane,
            Pane::Files {
                is_partial: false,
                ..
            }
        ));
        let nothing_read = Patch::parse("[output truncated at 8 MB]\n");
        let pane = from_read(Some(&nothing_read), None, "main".into());
        assert!(matches!(
            pane,
            Pane::Files {
                is_partial: true,
                ..
            }
        ));
    }

    #[test]
    fn a_lavoro_being_made_has_no_changes_yet_rather_than_a_folder_that_cannot_be_read() {
        let fixture = include_str!("../../../../sakura-core/tests/fixtures/status.json");
        let status: sakura_core::models::BoxStatus = serde_json::from_str(fixture).unwrap();
        let mut task = status.tasks[0].clone();
        assert_eq!(
            without_a_read(&task),
            None,
            "a lavoro that is there is read"
        );
        task.readable = false;
        assert_eq!(without_a_read(&task), Some(Pane::Unreadable));
        task.preparing = Some(sakura_core::models::Preparing {
            line: "cloning".into(),
            failed: false,
            has_workspace: false,
        });
        let none_yet = Pane::Empty {
            base: String::new(),
        };
        assert_eq!(without_a_read(&task), Some(none_yet));
    }

    #[test]
    fn every_state_without_a_list_has_a_sentence() {
        for pane in [
            Pane::Unreadable,
            Pane::Loading,
            Pane::Failed(Said::new("changes.failure").said("error", Said::new("error.internal"))),
            Pane::Empty {
                base: String::new(),
            },
        ] {
            assert!(!sentence(&pane).is_empty(), "{pane:?}");
        }
    }
}
