//! The quiet sentence a pane shows when it has nothing else to show. The Agente view's terminal is
//! `ui::console`, its changes `ui::changes`.

use super::brand::{mark, Ink, MarkSize, Phase};
use super::centred::centred;
use crate::theme::{self, Theme};
use gpui::{div, AnyElement, App, InteractiveElement as _, IntoElement, ParentElement, Styled};

/// A centred, quiet sentence filling the pane.
pub fn empty(sentence: &'static str, cx: &App) -> AnyElement {
    let palette = Theme::palette(cx);
    centred(sentence)
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_tertiary)
        .into_any_element()
}

/// The page when there is no task at all and none can be made: `sentence` says why.
pub fn no_task(sentence: impl Into<gpui::SharedString>, cx: &App) -> AnyElement {
    let palette = Theme::palette(cx);
    // The brand's mark, bare and still, over the sentence: the desk has nothing else to show.
    let said = div()
        .flex()
        .flex_col()
        .items_center()
        .gap(theme::GAP_WIDE)
        .child(
            div()
                .debug_selector(|| "brand-mark-empty".into())
                .child(mark(MarkSize::MEDIUM, Ink::Fuchsia, Phase::Whole)),
        )
        .child(
            div()
                .debug_selector(|| "no-task".into())
                .child(sentence.into()),
        );
    centred(said)
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_tertiary)
        .into_any_element()
}
