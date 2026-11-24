//! Updates as searchable settings rows; the updater owns all scheduling and check state.

use super::{
    controls,
    rows::{Group, Row},
    Section,
};
use crate::l10n::{tr, Key};
use crate::theme;
use crate::updates;
use gpui::{
    div, AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
    Window,
};
use gpui_component::{switch::Switch, Disableable as _, FocusableExt as _};
use std::rc::Rc;

pub(super) fn group(cx: &App) -> Group {
    let automatic = if cfg!(target_os = "macos") {
        "updates.automatic_downloads"
    } else {
        "updates.automatic_checks"
    };
    let rows = vec![
        Row::setting(
            tr(Key("updates.current_version")),
            tr(Key("updates.version_description")),
            vec![updates::CURRENT_VERSION.into()],
            version,
        ),
        Row::setting(
            tr(Key("updates.automatic")),
            tr(Key(automatic)),
            Vec::new(),
            automatic_control,
        ),
        Row::setting(
            tr(Key("updates.check_now")),
            updates::sentence(&updates::status(cx).state),
            Vec::new(),
            check_control,
        ),
    ];
    Group::new(Some(tr(Section::Updates.title()).into()), rows)
}

fn version(_: &mut Window, _: &mut App) -> AnyElement {
    div()
        .debug_selector(|| "settings-updates-version".into())
        .child(updates::CURRENT_VERSION)
        .into_any_element()
}

fn automatic_control(_: &mut Window, cx: &mut App) -> AnyElement {
    let automatic = Switch::new("updates-automatic-switch")
        .checked(updates::is_automatic(cx))
        .focus_ring(false)
        .tab_stop(false)
        .on_click(|enabled, _, cx| updates::set_automatic(*enabled, cx));
    div()
        .id("settings-updates-automatic")
        .debug_selector(|| "settings-updates-automatic".into())
        .capture_any_mouse_down(|_, window, _| window.prevent_default())
        .child(automatic)
        .into_any_element()
}

fn check_control(_: &mut Window, cx: &mut App) -> AnyElement {
    let check = controls::button(
        "settings-updates-check",
        tr(Key("updates.check_now")),
        Rc::new(|_, cx| updates::check_now(cx)),
        cx,
    )
    .disabled(!updates::status(cx).can_check)
    .focus_ring(false)
    .tab_stop(false);
    div()
        .flex()
        .items_center()
        .gap(theme::GAP)
        .child(check)
        .children(super::super::update_link::button(
            "settings-update-release",
            cx,
        ))
        .into_any_element()
}
