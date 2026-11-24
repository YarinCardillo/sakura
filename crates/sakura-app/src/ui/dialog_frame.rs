//! The one frame of every dialog, sheet and confirmation of the engine's window: top-anchored at
//! [`DIALOG_SHEET_TOP`], centred across the window, on a width of the shared scale, appearing in place with no slide.
//! The sheet inside draws its own surface; the kit's frame around it is taken away.

use crate::theme::{self, Palette, DIALOG_SHEET_TOP};
use gpui::Styled as _;
use gpui::{
    div, transparent_black, Div, FontWeight, InteractiveElement as _, ParentElement as _, Pixels,
    SharedString, Window,
};
#[cfg(not(feature = "boxes"))]
use gpui::{AnyElement, App, IntoElement, Stateful, StatefulInteractiveElement as _};
use gpui_component::dialog::{Dialog, DialogEntrance};
use gpui_component::Sizable as _;
#[cfg(not(feature = "boxes"))]
use gpui_component::WindowExt as _;

#[derive(Clone, Copy)]
pub enum Width {
    #[cfg(not(feature = "boxes"))]
    Small,
    Medium,
    #[cfg(not(feature = "boxes"))]
    Large,
}

impl Width {
    fn pixels(self) -> Pixels {
        match self {
            #[cfg(not(feature = "boxes"))]
            Self::Small => theme::DIALOG_WIDTH_SMALL,
            Self::Medium => theme::DIALOG_WIDTH_MEDIUM,
            #[cfg(not(feature = "boxes"))]
            Self::Large => theme::DIALOG_WIDTH_LARGE,
        }
    }
}

pub fn placed(dialog: Dialog, width: Width, window: &Window) -> Dialog {
    dialog
        .w(width.pixels())
        .margin_top(top(window))
        .entrance(DialogEntrance::None)
        .p_0()
        .border_0()
        .bg(transparent_black())
        .shadow_none()
        .close_button(false)
}

pub(super) fn top(window: &Window) -> Pixels {
    if window.viewport_size().height >= theme::WINDOW_MIN_HEIGHT {
        DIALOG_SHEET_TOP
    } else {
        theme::DIALOG_TOP
    }
}

pub fn available_height(window: &Window) -> Pixels {
    let height = window.viewport_size().height;
    let bottom = if height < theme::WINDOW_MIN_HEIGHT {
        theme::CONTENT_PADDING
    } else {
        let minimum = theme::STATUS_BAR_HEIGHT + theme::CONTENT_PADDING;
        let span = theme::WINDOW_START_HEIGHT - theme::WINDOW_MIN_HEIGHT;
        let expanded = ((height - theme::WINDOW_MIN_HEIGHT) / span).min(1.);
        minimum + (DIALOG_SHEET_TOP - minimum) * expanded
    };
    (height - top(window) - bottom).max(Pixels::ZERO)
}

pub fn surface(id: &'static str, window: &Window, palette: &Palette) -> Div {
    div()
        .debug_selector(move || id.into())
        .w_full()
        .max_h(available_height(window))
        .flex()
        .flex_col()
        .gap(theme::GAP)
        .p(theme::CONTENT_PADDING)
        .rounded(theme::RADIUS_SURFACE)
        .bg(palette.overlay_surface)
        .border(theme::PANEL_RING_WIDTH)
        .border_color(palette.overlay_edge)
        .shadow_lg()
}

#[cfg(not(feature = "boxes"))]
pub(super) fn filled_field(
    (id, label): (&'static str, &'static str),
    words: impl Into<SharedString>,
    editor: impl IntoElement,
    palette: &Palette,
) -> Div {
    use crate::theme::kit::scale::{Typeset as _, TEXT_FOOTNOTE};
    div()
        .flex()
        .flex_col()
        .gap(theme::GAP_TIGHT)
        .child(
            div()
                .debug_selector(move || label.into())
                .text_style(TEXT_FOOTNOTE)
                .font_weight(theme::WEIGHT_MEDIUM)
                .text_color(palette.text_secondary)
                .child(words.into()),
        )
        .child(
            div()
                .debug_selector(move || id.into())
                .rounded(theme::RADIUS_CONTROL)
                .bg(palette.fill)
                .min_h(theme::CONTROL_HEIGHT)
                .child(editor),
        )
}

#[cfg(not(feature = "boxes"))]
pub(super) fn title_field(
    ids: (&'static str, &'static str),
    label: &'static str,
    input: &gpui::Entity<gpui_component::input::InputState>,
    palette: &Palette,
) -> Div {
    let editor =
        super::settings_modal::controls::styled_input(input, theme::kit::scale::TEXT_TITLE)
            .appearance(false)
            .bordered(false)
            .focus_bordered(false)
            .min_h(theme::CONTROL_HEIGHT)
            .aria_label(label);
    filled_field(ids, label, editor, palette)
}

pub fn heading(id: &'static str, title: impl Into<SharedString>, palette: &Palette) -> Div {
    heading_with_close(id, title, palette, true)
}

pub fn heading_with_close(
    id: &'static str,
    title: impl Into<SharedString>,
    palette: &Palette,
    can_close: bool,
) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .gap(theme::GAP)
        .child(
            div()
                .debug_selector(move || id.into())
                .min_w_0()
                .text_size(theme::kit::scale::TEXT_TITLE.size)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.text)
                .child(title.into()),
        )
        .children(can_close.then(|| close_button("dialog-sheet-close")))
}

pub fn close_button(id: &'static str) -> Div {
    div().debug_selector(move || id.into()).flex_none().child(
        gpui_base::DialogClose::new().trigger(|base| {
            base.size(theme::STATUS_CONTROL_HEIGHT)
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_CONTROL)
                .tab_stop(false)
                .child(
                    gpui_component::Icon::new(gpui_kit_assets::IconName::Close)
                        .with_size(theme::kit::scale::ICON_REGULAR),
                )
                .accessibility_label(crate::l10n::tr(crate::l10n::Key("common.close")))
        }),
    )
}

pub fn footer(palette: &Palette) -> Div {
    div()
        .debug_selector(|| "dialog-footer".into())
        .flex_none()
        .border_t_1()
        .border_color(palette.border)
        .pt(theme::GAP)
        .flex()
        .items_center()
        .justify_end()
        .gap(theme::GAP_SMALL)
}

#[cfg(not(feature = "boxes"))]
pub fn group(
    id: &'static str,
    title: impl Into<SharedString>,
    rows: Vec<AnyElement>,
    palette: &Palette,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(theme::GAP_SMALL)
        .child(
            div()
                .debug_selector(move || id.into())
                .text_size(theme::TEXT_LABEL)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.text_secondary)
                .child(title.into()),
        )
        .child(super::settings_modal::group::group(rows, palette))
        .into_any_element()
}

#[cfg(not(feature = "boxes"))]
pub fn scrolling_body(id: &'static str) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .debug_selector(move || id.into())
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .pb(theme::GAP_SMALL)
        .flex()
        .flex_col()
}

/// Holds the dialog guard until the kit releases the dialog builder.
#[cfg(not(feature = "boxes"))]
pub fn open_guarded(
    window: &mut Window,
    cx: &mut App,
    build: impl Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static,
) {
    let guard = super::dialog_guard::DialogToken::take(cx);
    window.open_dialog(cx, move |dialog, window, cx| {
        let _held = &guard;
        build(dialog, window, cx)
    });
}
