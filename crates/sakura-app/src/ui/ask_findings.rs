//! What the reviewer of a lane requested, as the ask to deliver although the review still wants changes
//! carries it: one plain line each, under the ask, in the Inbox and on the lane's card.

use crate::l10n::{tr, Key};
use crate::theme::{self, Palette};
use crate::ui::chrome::text;
use crate::ui::path_text::plain_with_paths;
use gpui::{
    div, AnyElement, InteractiveElement as _, IntoElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _,
};

/// The words over the findings.
const HEADING: Key = Key("ask.findings");

/// The findings of the ask `id` as a block, or none when the engine gave none. `stem` and `id` name the block
/// (`{stem}-{id}`) and its lines (`{stem}-line-{id}-{n}`) in the window.
pub fn findings_view(
    stem: &'static str,
    id: &str,
    findings: &[String],
    palette: &Palette,
) -> Option<AnyElement> {
    if findings.is_empty() {
        return None;
    }
    let block = format!("{stem}-{id}");
    let lines = findings.iter().enumerate().map(|(at, finding)| {
        let line = format!("{stem}-line-{id}-{at}");
        text(theme::TEXT_SMALL, palette.text)
            .id(gpui::SharedString::from(line.clone()))
            .debug_selector(move || line)
            .overflow_x_scroll()
            .child(plain_with_paths(
                &crate::ui::thread::shown(finding),
                palette.text,
                palette.text_secondary,
            ))
    });
    Some(
        div()
            .debug_selector(move || block)
            .flex()
            .flex_col()
            .gap(theme::GAP_SMALL)
            .child(text(theme::TEXT_SMALL, palette.text_tertiary).child(tr(HEADING)))
            .children(lines)
            .into_any_element(),
    )
}
