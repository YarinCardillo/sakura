//! A file path in a row: its root folder and its file name always show whole; the folders between them give
//! way first, cut with an ellipsis, as the row narrows. A path with one folder or none has no middle.

use gpui::{
    div, Div, Hsla, InteractiveElement as _, ParentElement as _, SharedString, Stateful,
    Styled as _,
};
#[cfg(not(feature = "boxes"))]
use gpui::{AnyElement, App, IntoElement as _, StatefulInteractiveElement as _};
#[cfg(not(feature = "boxes"))]
use gpui_component::button::{Button, ButtonVariants as _};
#[cfg(not(feature = "boxes"))]
use gpui_component::popover::Popover;
#[cfg(not(feature = "boxes"))]
use gpui_component::IconName;

#[cfg(not(feature = "boxes"))]
pub(in crate::ui) fn reveal_path(id: &str, path: String, cx: &App) -> AnyElement {
    reveal(id, path, false, cx)
}

#[cfg(not(feature = "boxes"))]
pub(in crate::ui) fn reveal_path_icon(id: &str, path: String, cx: &App) -> AnyElement {
    reveal(id, path, true, cx)
}

#[cfg(not(feature = "boxes"))]
fn reveal(id: &str, path: String, is_icon: bool, cx: &App) -> AnyElement {
    let trigger = format!("{id}-reveal");
    let selector = format!("{id}-value");
    let palette = crate::theme::Theme::palette(cx);
    let label = crate::l10n::tr(crate::l10n::Key("common.reveal_path"));
    let button = Button::new(trigger.clone())
        .debug_selector(move || trigger)
        .custom(crate::ui::chrome::quiet_variant(&palette, cx));
    let button = if is_icon {
        button.icon(IconName::Folder).tooltip(label)
    } else {
        button.label(label)
    };
    let popover = Popover::new(id.to_string())
        .trigger(button)
        .content(move |_, window, _| {
            let value = selector.clone();
            crate::ui::chrome::mono()
                .id(value.clone())
                .debug_selector(move || value)
                .max_w(window.viewport_size().width * 0.5)
                .overflow_x_scroll()
                .child(path.clone())
        });
    if is_icon {
        let icon = format!("{id}-icon");
        div()
            .debug_selector(move || icon.clone())
            .child(popover)
            .into_any_element()
    } else {
        popover.into_any_element()
    }
}

/// A path split into the three places the row keeps or cuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathParts<'a> {
    /// The first folder with its separator, empty for a file at the top.
    pub root: &'a str,
    /// The folders between the root and the name, with their separators.
    pub middle: &'a str,
    /// The file name with its extension.
    pub name: &'a str,
}

/// Splits `path` at its first and its last separator.
pub fn split_path(path: &str) -> PathParts<'_> {
    let Some(last) = path.rfind('/') else {
        return PathParts {
            root: "",
            middle: "",
            name: path,
        };
    };
    let (folders, name) = path.split_at(last + 1);
    let root_end = folders.find('/').map_or(0, |slash| slash + 1);
    let (root, middle) = folders.split_at(root_end);
    PathParts { root, middle, name }
}

/// `path` as one line in the code font: the root and the name in `ink` and `quiet`, the middle shrinking
/// first. `id` names it in the window (`{id}-root`, `{id}-middle`, `{id}-name` its parts) and lets a tooltip
/// bind to it.
pub fn path_label(
    id: impl Into<SharedString>,
    path: &str,
    (ink, quiet): (Hsla, Hsla),
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let parts = split_path(path);
    let selectors = [
        format!("{id}-root"),
        format!("{id}-middle"),
        format!("{id}-name"),
    ];
    let [root, middle, name] = selectors;
    crate::ui::chrome::mono()
        .id(id)
        .min_w_0()
        .flex()
        .whitespace_nowrap()
        .child(
            div()
                .debug_selector(move || root.clone())
                .flex_none()
                .text_color(quiet)
                .child(parts.root.to_string()),
        )
        .child(
            div()
                .debug_selector(move || middle.clone())
                .min_w_0()
                .truncate()
                .text_color(quiet)
                .child(parts.middle.to_string()),
        )
        .child(
            div()
                .debug_selector(move || name.clone())
                .flex_none()
                .max_w_full()
                .truncate()
                .text_color(ink)
                .child(parts.name.to_string()),
        )
}
