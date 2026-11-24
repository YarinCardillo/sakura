//! The buttons of the Inbox's row of actions: solid fills as the design draws them, not the kit's soft
//! tint. The one primary action wears the interface's action face (brushed silver on dark, graphite on
//! light); the others are the secondary fill or no fill at all.

use crate::theme::Palette;
use crate::ui::kit::{self, ButtonKind};
use crate::ui::tip::{self, Words};
use gpui::{App, Div, SharedString, Stateful};

/// How a button looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Face {
    /// The one filled action of the row.
    Primary,
    /// A filled secondary answer.
    Plain,
    /// No fill until the pointer is on it.
    Quiet,
}

/// A button named `id` that says `label`: the kit's button. `key`, the key that does the same, is not drawn
/// on it: it is in the button's tooltip. A button that is `is_off` is faded and the caller gives it no handler.
pub(super) fn action(
    id: &str,
    label: SharedString,
    key: Option<SharedString>,
    face: Face,
    is_off: bool,
    palette: &Palette,
    cx: &App,
) -> Stateful<Div> {
    let kind = match face {
        Face::Primary => ButtonKind::Primary,
        Face::Plain => ButtonKind::Secondary,
        Face::Quiet => ButtonKind::Quiet,
    };
    if is_off {
        return kit::button_disabled(id.to_string(), label, kind, palette);
    }
    let button = kit::button(id.to_string(), label.clone(), kind, palette);
    match key {
        Some(shortcut) => {
            let words = Words {
                label,
                shortcut: Some(shortcut),
            };
            tip::bind(button, format!("tip-{id}"), words, cx)
        }
        None => button,
    }
}

#[derive(Clone, Copy)]
pub(super) enum State {
    Open,
    Done,
}

impl State {
    pub(super) fn of_wait(wait: &sakura_core::engine::snapshot::Wait) -> Self {
        match wait.resolution.as_deref() {
            Some("done") => Self::Done,
            _ => Self::Open,
        }
    }

    pub(super) fn label(self) -> SharedString {
        use crate::l10n::{tr, Key};
        tr(match self {
            Self::Open => Key("inbox.state.open"),
            Self::Done => Key("inbox.view.done"),
        })
        .into()
    }

    pub(super) fn glyph(self, palette: &Palette) -> gpui::AnyElement {
        use crate::ui::kit::StateGlyph;
        let glyph = match self {
            Self::Open => StateGlyph::NotStarted,
            Self::Done => StateGlyph::Finished,
        };
        glyph.paint(crate::theme::HAND_MARK_SIZE, palette)
    }
}
