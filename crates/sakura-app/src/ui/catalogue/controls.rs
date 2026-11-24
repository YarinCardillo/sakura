//! The controls in every state: buttons, chips, tags, the choice between a few words, the toggle, the stepper,
//! fields with their labels, and an open menu.

use super::{group, row, specimen};
use crate::l10n::{tr, tr_with, Key};
use crate::theme::kit::catalogue as room;
use crate::theme::kit::scale::*;
use crate::theme::{self, Palette};
use crate::ui::kit::{
    self, button, button_hovered, button_icon, field, menu_chip, stepper, tag, toggle, ButtonKind,
};
use crate::ui::settings_modal::controls as settings_controls;
use gpui::{
    div, AnyElement, App, AppContext as _, Entity, IntoElement as _, ParentElement as _,
    Styled as _, Window,
};
use gpui_component::input::InputState;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit_assets::IconName;
use std::rc::Rc;

/// The two fields the page shows: one empty with its example, one filled.
pub(super) fn fields_made(window: &mut Window, cx: &mut App) -> [Entity<InputState>; 2] {
    let empty = cx.new(|cx| InputState::new(window, cx).placeholder("my-project"));
    let filled = cx.new(|cx| {
        let mut state = InputState::new(window, cx);
        state.set_value("sakura-core", window, cx);
        state
    });
    [empty, filled]
}

/// The open menu the page shows.
pub(super) fn menu_made(window: &mut Window, cx: &mut App) -> Entity<PopupMenu> {
    PopupMenu::build(window, cx, |menu, _, _| {
        menu.item(PopupMenuItem::new(tr(Key("waiting.approve"))).icon(IconName::FolderPlus))
            .item(PopupMenuItem::new(tr(Key("inbox.always_approve"))).checked(true))
            .item(PopupMenuItem::new(tr(Key("catalogue.long_menu_label"))))
            .item(PopupMenuItem::new(tr(Key("inbox.decline"))).disabled(true))
    })
}

/// A ring drawn the way the kit draws keyboard focus, to show that state without a keyboard.
fn focused(element: impl gpui::IntoElement, palette: &Palette) -> AnyElement {
    div()
        .p(SPACE_XS)
        .rounded(RADIUS_SURFACE)
        .border(theme::PANEL_RING_WIDTH * 2.)
        .border_color(palette.ring)
        .child(element)
        .into_any_element()
}

/// Four kinds of button, each at rest, hovered, pressed, disabled, focused, with a long label and an icon.
pub(super) fn buttons(palette: &Palette) -> AnyElement {
    let kinds = [
        (
            "primary",
            Key("catalogue.button_primary"),
            ButtonKind::Primary,
        ),
        (
            "secondary",
            Key("catalogue.button_secondary"),
            ButtonKind::Secondary,
        ),
        ("quiet", Key("catalogue.button_quiet"), ButtonKind::Quiet),
        (
            "destructive",
            Key("catalogue.button_destructive"),
            ButtonKind::Destructive,
        ),
    ];
    let rows = kinds.map(|(name, kind_name, kind)| button_row(name, kind_name, kind, palette));
    let large = kit::button_large(
        "cat-large",
        tr(Key("new_lane.start")),
        ButtonKind::Primary,
        palette,
    );
    let mut all = Vec::from(rows);
    all.push(specimen(tr(Key("catalogue.large_button")), large, palette));
    group(
        "catalogue-buttons",
        tr(Key("catalogue.buttons")),
        all,
        palette,
    )
}

fn button_row(name: &str, kind_name: Key, kind: ButtonKind, palette: &Palette) -> AnyElement {
    let id = |state: &str| format!("cat-{name}-{state}");
    let disabled = kit::button_disabled(id("disabled"), tr(Key("waiting.approve")), kind, palette);
    let long = div().w(room::BUTTON_LONG_ROOM).flex().child(button(
        id("long"),
        tr(Key("catalogue.long_action")),
        kind,
        palette,
    ));
    let states = vec![
        specimen(
            &tr_with(Key("catalogue.resting_button"), &[("kind", tr(kind_name))]),
            button(id("rest"), tr(Key("waiting.approve")), kind, palette),
            palette,
        ),
        specimen(
            tr(Key("catalogue.hover")),
            button_hovered(id("hover"), tr(Key("waiting.approve")), kind, palette),
            palette,
        ),
        specimen(
            tr(Key("catalogue.pressed")),
            button_hovered(id("pressed"), tr(Key("waiting.approve")), kind, palette),
            palette,
        ),
        specimen(tr(Key("catalogue.disabled_opacity")), disabled, palette),
        specimen(
            tr(Key("catalogue.keyboard_focus")),
            focused(
                button(id("focus"), tr(Key("waiting.approve")), kind, palette),
                palette,
            ),
            palette,
        ),
        specimen(tr(Key("catalogue.truncated_label")), long, palette),
        specimen(
            tr(Key("catalogue.with_icon")),
            button_icon(
                id("icon"),
                tr(Key("settings.projects.open")),
                IconName::FolderPlus,
                kind,
                palette,
            ),
            palette,
        ),
    ];
    row(states).into_any_element()
}

/// Chips and tags: the same height, padding and label.
pub(super) fn chips_and_tags(palette: &Palette, cx: &App) -> AnyElement {
    let chip = kit::chip(
        "cat-chip",
        tr(Key("catalogue.standard")),
        Some(IconName::FolderPlus),
        |_, _, _| {},
        cx,
    );
    let menu = menu_chip(
        "cat-menu-chip",
        tr(Key("catalogue.careful")),
        Some(IconName::FolderPlus),
        cx,
    );
    let long = div().w(room::CHIP_LONG_ROOM).flex().child(menu_chip(
        "cat-long-chip",
        tr(Key("catalogue.long_action")),
        None,
        cx,
    ));
    let hovered = kit::chip(
        "cat-chip-hover",
        tr(Key("catalogue.hover_action")),
        None,
        |_, _, _| {},
        cx,
    );
    let all = vec![
        specimen(tr(Key("catalogue.chip_height")), chip, palette),
        specimen(tr(Key("catalogue.chip_menu")), menu, palette),
        specimen(tr(Key("catalogue.truncated_label")), long, palette),
        specimen(tr(Key("catalogue.resting_chip")), hovered, palette),
        specimen(
            tr(Key("catalogue.tag_without_dot")),
            tag("cat-tag", "sakura-core", None, palette),
            palette,
        ),
        specimen(
            tr(Key("catalogue.tag_with_dot")),
            tag(
                "cat-tag-dot",
                "sakura-app",
                Some(palette.text_tertiary),
                palette,
            ),
            palette,
        ),
        specimen(
            tr(Key("catalogue.truncated_tag")),
            div().w(room::TAG_LONG_ROOM).flex().child(tag(
                "cat-tag-long",
                tr(Key("catalogue.long_action")),
                None,
                palette,
            )),
            palette,
        ),
    ];
    group("catalogue-chips", tr(Key("catalogue.chips")), all, palette)
}

/// The choice between a few words (inline and full width), the toggle and the stepper.
pub(super) fn choices(palette: &Palette, _cx: &App) -> AnyElement {
    let words = |on: usize| {
        [
            tr(Key("common.details")),
            tr(Key("view.changes")),
            tr(Key("view.files")),
        ]
        .iter()
        .enumerate()
        .map(|(index, word)| ((*word).into(), index == on))
        .collect::<Vec<_>>()
    };
    let inline = kit::segments("cat-seg", &words(0), |_, _, _| {});
    let full = div()
        .w(room::FIELD_WIDTH)
        .child(kit::segments("cat-seg-full", &words(1), |_, _, _| {}).full_width());
    let step = |value, id: &str| {
        stepper(
            id,
            (value, value),
            Some(1),
            Some(9),
            Rc::new(|_, _, _| {}),
            palette,
        )
    };
    let all = vec![
        specimen(tr(Key("catalogue.inline_segments")), inline, palette),
        specimen(tr(Key("catalogue.full_segments")), full, palette),
        specimen(
            tr(Key("catalogue.toggle_on")),
            toggle("cat-toggle-on", true, |_, _, _| {}),
            palette,
        ),
        specimen(
            tr(Key("catalogue.toggle_off")),
            toggle("cat-toggle-off", false, |_, _, _| {}),
            palette,
        ),
        specimen(tr(Key("catalogue.stepper")), step(3, "cat-step"), palette),
        specimen(
            tr(Key("catalogue.stepper_minimum")),
            step(1, "cat-step-min"),
            palette,
        ),
    ];
    group(
        "catalogue-choices",
        tr(Key("catalogue.choices")),
        all,
        palette,
    )
}

/// Fields: always under a visible label, with a hint, empty and filled.
pub(super) fn fields(inputs: &[Entity<InputState>; 2], palette: &Palette, _cx: &App) -> AnyElement {
    let width = room::FIELD_WIDTH;
    let make =
        |id: &str, name: &str, label: &str, input: &Entity<InputState>, hint: Option<&str>| {
            let control = settings_controls::field(input, width, palette);
            let hint = hint.map(|hint| hint.to_string().into());
            specimen(
                name,
                div().w(width).child(field(
                    id.to_string(),
                    label.to_string(),
                    control,
                    hint,
                    palette,
                )),
                palette,
            )
        };
    let all = vec![
        make(
            "cat-empty",
            tr(Key("catalogue.empty_field")),
            tr(Key("catalogue.project_name")),
            &inputs[0],
            Some(tr(Key("catalogue.project_hint"))),
        ),
        make(
            "cat-filled",
            tr(Key("catalogue.filled_field")),
            tr(Key("projects.repository")),
            &inputs[1],
            None,
        ),
    ];
    group(
        "catalogue-fields",
        tr(Key("catalogue.fields")),
        all,
        palette,
    )
}

/// An open menu: an item with an icon, a checked one, a long one, a disabled one.
pub(super) fn menu(menu: &Entity<PopupMenu>, palette: &Palette) -> AnyElement {
    let shown = div().child(menu.clone());
    group(
        "catalogue-menu",
        tr(Key("catalogue.menu")),
        vec![specimen(tr(Key("catalogue.menu_spacing")), shown, palette)],
        palette,
    )
}
