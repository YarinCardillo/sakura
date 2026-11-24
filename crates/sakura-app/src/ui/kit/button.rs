//! The one button: four kinds (primary, secondary, quiet, destructive), one label (the control label of the
//! scale), one height (the regular control, or the large one for the single primary action of a sheet).
//! Every kind is a solid fill drawn here, never the kit's button: the kit draws a custom button's fill at a
//! fifth of its strength, which made secondary and quiet buttons invisible at rest. The primary action wears
//! the palette's primary color.

use crate::theme::kit::scale::{
    Typeset as _, CONTROL_LABEL, CONTROL_REGULAR, ICON_REGULAR, ICON_TEXT_GAP, RADIUS_CONTROL,
    SPACE_M,
};
#[cfg(any(test, feature = "snapshot"))]
use crate::theme::kit::scale::{CONTROL_LARGE, SPACE_L};
use crate::theme::{self, kit::DISABLED_OPACITY, Palette};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, point, px, transparent_black, BoxShadow, Div, ElementId, Hsla, InteractiveElement as _,
    ParentElement as _, Pixels, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled as _,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

/// What a button is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// The one action a view or a sheet exists for.
    Primary,
    /// Any other action of the view, filled one step above its surface.
    Secondary,
    #[cfg(not(feature = "boxes"))]
    Card,
    /// An action that stays out of the way until the pointer is on it.
    Quiet,
    /// An action that removes or ends something: the danger tone on the secondary fill.
    #[cfg(any(test, feature = "snapshot"))]
    Destructive,
}

/// A button of the regular control height with `label`. `id` names it in the window (the label is
/// `{id}-label`); the caller adds `on_click`.
pub fn button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    palette: &Palette,
) -> Stateful<Div> {
    frame(
        id.into(),
        label.into(),
        None,
        kind,
        (CONTROL_REGULAR, SPACE_M),
        true,
        palette,
    )
}

/// The regular button face without pointer or keyboard interaction.
pub fn button_disabled(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    palette: &Palette,
) -> Stateful<Div> {
    frame(
        id.into(),
        label.into(),
        None,
        kind,
        (CONTROL_REGULAR, SPACE_M),
        false,
        palette,
    )
}

/// A [`button`] with `icon` before its label: the regular icon and one icon and text gap (`{id}-icon`).
pub fn button_icon(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    icon: IconName,
    kind: ButtonKind,
    palette: &Palette,
) -> Stateful<Div> {
    frame(
        id.into(),
        label.into(),
        Some(icon),
        kind,
        (CONTROL_REGULAR, SPACE_M),
        true,
        palette,
    )
}

#[cfg(not(feature = "boxes"))]
pub fn button_icon_disabled(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    icon: IconName,
    kind: ButtonKind,
    palette: &Palette,
) -> Stateful<Div> {
    frame(
        id.into(),
        label.into(),
        Some(icon),
        kind,
        (CONTROL_REGULAR, SPACE_M),
        false,
        palette,
    )
}

/// A button of the large control height, for the primary action of a sheet or a card.
#[cfg(any(test, feature = "snapshot"))]
pub fn button_large(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    palette: &Palette,
) -> Stateful<Div> {
    frame(
        id.into(),
        label.into(),
        None,
        kind,
        (CONTROL_LARGE, SPACE_L),
        true,
        palette,
    )
}

/// A [`button`] drawn as it is under the pointer, to show that state without a pointer.
#[cfg(feature = "snapshot")]
pub fn button_hovered(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    palette: &Palette,
) -> Stateful<Div> {
    let button = button(id, label, kind, palette);
    match kind {
        ButtonKind::Primary => button.opacity(theme::PRIMARY_HOVER_OPACITY),
        _ => button.bg(faces(kind, palette).hover),
    }
}

/// The colours of a kind: at rest, under the pointer, and its ink (at rest and under the pointer).
struct Faces {
    rest: Hsla,
    hover: Hsla,
    ink: Hsla,
    hover_ink: Hsla,
}

fn faces(kind: ButtonKind, palette: &Palette) -> Faces {
    let (rest, hover, ink, hover_ink) = match kind {
        ButtonKind::Primary => (
            palette.primary,
            palette.primary,
            palette.action_ink,
            palette.action_ink,
        ),
        ButtonKind::Secondary => (palette.fill, palette.fill_hover, palette.text, palette.text),
        #[cfg(not(feature = "boxes"))]
        ButtonKind::Card => (palette.fill_hover, palette.fill, palette.text, palette.text),
        ButtonKind::Quiet => (
            transparent_black(),
            palette.fill,
            palette.text_secondary,
            palette.text,
        ),
        #[cfg(any(test, feature = "snapshot"))]
        ButtonKind::Destructive => (
            palette.fill,
            palette.fill_hover,
            palette.danger,
            palette.danger,
        ),
    };
    Faces {
        rest,
        hover,
        ink,
        hover_ink,
    }
}

fn frame(
    id: SharedString,
    label: SharedString,
    icon: Option<IconName>,
    kind: ButtonKind,
    (height, padding): (Pixels, Pixels),
    is_enabled: bool,
    palette: &Palette,
) -> Stateful<Div> {
    let faces = faces(kind, palette);
    let (selector, label_selector) = (id.clone(), format!("{id}-label"));
    let base = div()
        .id(ElementId::from(id.clone()))
        .debug_selector(move || selector.to_string())
        .min_h(height)
        .px(padding)
        .min_w_0()
        .flex()
        .items_center()
        .justify_center()
        .gap(ICON_TEXT_GAP)
        .rounded(RADIUS_CONTROL)
        .text_style(CONTROL_LABEL)
        .text_color(if is_enabled || kind == ButtonKind::Primary {
            faces.ink
        } else {
            palette.text_tertiary
        })
        .whitespace_normal()
        .when(is_enabled, |base| {
            let ring = keyboard_ring(palette);
            base.active(|style| style.opacity(theme::TAKE_PRESSED_OPACITY))
                .focusable()
                .tab_stop(true)
                .focus_visible(move |style| style.shadow(ring.clone()))
        })
        .children(icon.map(|name| mark(&id, name)))
        .child(
            div()
                .debug_selector(move || label_selector.clone())
                .min_w_0()
                .child(label),
        );
    painted(base, kind, faces, is_enabled)
}

fn mark(id: &SharedString, name: IconName) -> Div {
    let selector = format!("{id}-icon");
    div()
        .debug_selector(move || selector.clone())
        .flex_none()
        .child(Icon::new(name).with_size(ICON_REGULAR))
}

fn painted(base: Stateful<Div>, kind: ButtonKind, faces: Faces, is_enabled: bool) -> Stateful<Div> {
    if !is_enabled {
        return base
            .bg(faces.rest.opacity(DISABLED_OPACITY))
            .cursor_default();
    }
    if kind == ButtonKind::Primary {
        return base
            .bg(faces.rest)
            .hover(|style| style.opacity(theme::PRIMARY_HOVER_OPACITY));
    }
    let (hover, hover_ink) = (faces.hover, faces.hover_ink);
    base.bg(faces.rest).when(is_enabled, |base| {
        base.hover(move |style| style.bg(hover).text_color(hover_ink))
    })
}

/// The ring a button wears while the keyboard has it: the palette's ring, two ring widths out.
pub(in crate::ui) fn keyboard_ring(palette: &Palette) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: palette.ring,
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: theme::PANEL_RING_WIDTH * 2.,
        inset: false,
    }]
}
