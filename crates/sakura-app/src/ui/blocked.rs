//! The one full-window state shown when `box` cannot run at all: what is wrong and the exact thing to do.
//! The facts entity retries on every tick (5 s) and the window returns to the tasks by itself.

use super::centred::centred;
use super::chrome::{drag_region, mono, text};
use crate::l10n::{say, tr, Key};
use crate::theme::{self, Palette};
use gpui::{div, px, IntoElement, ParentElement, Styled};
use sakura_core::box_cli::BOX_BIN_VAR;
use sakura_core::error::{BridgeError, ErrorCode};

/// The blocked state for `error`, over the whole window.
pub fn view(error: &BridgeError, palette: &Palette) -> impl IntoElement {
    let message = div()
        .flex_1()
        .min_h_0()
        .child(centred(message(error, palette)));
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            drag_region("blocked-drag")
                .h(theme::WINDOW_STRIP_HEIGHT)
                .flex_none(),
        )
        .child(message)
}

/// Title, what failed, what to do, and that it retries.
fn message(error: &BridgeError, palette: &Palette) -> impl IntoElement {
    let (title, remedy) = advice(error);
    let heading = text(theme::TEXT_BODY, palette.text).font_weight(theme::WEIGHT_MEDIUM);
    let retry = text(theme::TEXT_LABEL, palette.text_tertiary).pt(theme::GAP);
    div()
        .w(px(theme::BLOCKED_COLUMN_WIDTH))
        .flex()
        .flex_col()
        .gap(theme::GAP)
        .child(heading.child(title))
        .child(text(theme::TEXT_SMALL, palette.text_secondary).child(say(&error.said)))
        .child(remedy_line(remedy, palette))
        .child(retry.child(tr(Key("blocked.retrying"))))
}

/// The remedy, with the variable name in monospace when it names one.
fn remedy_line(remedy: Remedy, palette: &Palette) -> impl IntoElement {
    let line = text(theme::TEXT_SMALL, palette.text_secondary)
        .flex()
        .flex_wrap()
        .gap_x_1();
    match remedy {
        Remedy::Sentence(text) => line.child(text),
        Remedy::BoxPath => line
            .child(tr(Key("blocked.install_before")))
            .child(mono().child(BOX_BIN_VAR))
            .child(tr(Key("blocked.install_after"))),
    }
}

enum Remedy {
    Sentence(&'static str),
    BoxPath,
}

fn advice(error: &BridgeError) -> (&'static str, Remedy) {
    match error.code {
        ErrorCode::BoxMissing => (tr(Key("blocked.box_missing")), Remedy::BoxPath),
        _ => (
            tr(Key("blocked.docker_down")),
            Remedy::Sentence(tr(Key("blocked.docker_hint"))),
        ),
    }
}
