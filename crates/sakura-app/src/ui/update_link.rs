//! A release page link, shared by Settings and the status bar; never shown for a failed check.

use crate::l10n::{tr, Key};
use crate::theme::{self, Theme};
use crate::updates;
use gpui::{
    div, AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::FocusableExt as _;

/// Shows a published release in the browser without downloading or replacing any files.
pub fn button(id: &'static str, cx: &App) -> Option<AnyElement> {
    let release = updates::available_release(cx)?;
    let palette = Theme::palette(cx);
    let sentence = updates::sentence(&updates::UpdateState::Available(release.clone()));
    let (label, height, look) = match id {
        "status-update-release" => (
            sentence,
            theme::STATUS_CONTROL_HEIGHT,
            super::chrome::quiet_variant(&palette, cx),
        ),
        _ => (
            tr(Key("updates.view_release")).to_string(),
            theme::CONTROL_HEIGHT,
            super::chrome::quiet_variant(&palette, cx),
        ),
    };
    let button = Button::new(id)
        .debug_selector(move || id.into())
        .custom(look)
        .h(height)
        .px(theme::CONTROL_PADDING_X)
        .focus_ring(false)
        .tab_stop(false)
        .child(div().text_size(theme::TEXT_LABEL).child(label))
        .on_click(move |_, _, cx| cx.open_url(&release.url));
    Some(button.into_any_element())
}
