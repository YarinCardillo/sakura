//! The bottom row of the engine's window, in the manner of Zed's: a thin row of quiet icon buttons along the
//! foot of the window. It has no background of its own (what is behind the sidebar shows through under it)
//! and says nothing in words: no status and no warning. At the left the sidebar's toggle and Settings; at
//! the right the tour, the update when one is ready, and the lane column's toggle when available.
//!
//! An engine that cannot be reached is said inside the page that needs it, not here.

use super::tip;
use crate::icons::PaneIcon;
use crate::l10n::{tr, tr_with, Key};
use crate::menus::{self, InstallUpdate, ShowSettings, ShowTour, ToggleChanges, ToggleSidebar};
use crate::theme::{engine_bar as bar, Palette, Theme};
use crate::updates;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, Action, AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

/// What a button of the row does: the action a press dispatches, or the release page a press opens.
enum Press {
    Action(Box<dyn Action>),
    Open(String),
}

/// One button of the row: its name for the tests, its icon, what a press would do in words and what it
/// does.
struct Spec {
    id: &'static str,
    icon: Icon,
    label: SharedString,
    press: Press,
}

/// The row. `is_sidebar_open` is how the sidebar's toggle is drawn; the lane column's toggle is there only
/// when the page shown has a column, and is drawn open or closed as `column` says (none: no column, no
/// toggle).
pub fn view(is_sidebar_open: bool, column: Option<bool>, cx: &App) -> AnyElement {
    let palette = Theme::palette(cx);
    let left = div()
        .flex()
        .items_center()
        .gap(bar::BAR_GAP)
        .child(button(sidebar_toggle(is_sidebar_open), &palette, cx))
        .child(button(settings(), &palette, cx));
    let right = std::iter::once(tour())
        .chain(update(cx))
        .chain(column.map(column_toggle))
        .map(|spec| button(spec, &palette, cx));
    div()
        .debug_selector(|| "status-bar".into())
        .h(bar::BAR_HEIGHT)
        .w_full()
        .flex_none()
        .px(bar::BAR_PADDING_X)
        .flex()
        .items_center()
        .gap(bar::BAR_GAP)
        .child(left)
        .child(div().flex_1())
        .children(right)
        .into_any_element()
}

fn sidebar_toggle(is_open: bool) -> Spec {
    Spec {
        id: "toggle-sidebar",
        icon: Icon::new(PaneIcon::Left { is_open }),
        label: tr(menus::sidebar_label(is_open)).into(),
        press: Press::Action(Box::new(ToggleSidebar)),
    }
}

fn settings() -> Spec {
    Spec {
        id: "status-settings",
        icon: Icon::new(crate::icons::SETTINGS),
        label: tr(Key("status.settings")).into(),
        press: Press::Action(Box::new(ShowSettings)),
    }
}

fn tour() -> Spec {
    Spec {
        id: "status-tour",
        icon: Icon::new(IconName::Info),
        label: tr(Key("tour.show")).into(),
        press: Press::Action(Box::new(ShowTour)),
    }
}

/// The update button, when a release is ready: a press shows its page, or, for an update the platform
/// updater found, which has no page, installs it and restarts.
fn update(cx: &App) -> Option<Spec> {
    let release = updates::ready_release(cx)?;
    let press = match release.url.is_empty() {
        true => Press::Action(Box::new(InstallUpdate)),
        false => Press::Open(release.url),
    };
    Some(Spec {
        id: "status-update",
        icon: Icon::new(IconName::CircleArrowDown),
        label: tr_with(Key("status.update_ready"), &[("version", &release.version)]),
        press,
    })
}

fn column_toggle(is_open: bool) -> Spec {
    Spec {
        id: "toggle-column",
        icon: Icon::new(PaneIcon::Right { is_open }),
        label: tr(Key("status.lane_column")).into(),
        press: Press::Action(Box::new(ToggleChanges)),
    }
}

/// A quiet icon button: secondary ink, no fill at rest, the hover fill under the pointer.
fn button(spec: Spec, palette: &Palette, cx: &App) -> AnyElement {
    let (id, label) = (spec.id, spec.label);
    let icon_id = format!("{id}-icon");
    let mark = div()
        .debug_selector(move || icon_id)
        .size(bar::ICON_SIZE)
        .flex_none()
        .child(spec.icon.with_size(bar::ICON_SIZE));
    let (ring, hover, active) = (palette.ring, palette.hover, palette.fill);
    let button = div()
        .id(id)
        .debug_selector(move || id.into())
        .role(gpui::accesskit::Role::Button)
        .aria_label(label.clone())
        .size(bar::ICON_BUTTON)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(crate::theme::kit::scale::RADIUS_CONTROL)
        .text_color(palette.text_secondary)
        .border_1()
        .border_color(gpui::transparent_black())
        .focusable()
        .tab_stop(true)
        .when(id == "status-settings", |button| {
            button.capture_any_mouse_down(|_, window, _| window.prevent_default())
        })
        .focus_visible(move |style| style.border_color(ring))
        .hover(move |style| style.bg(hover))
        .active(move |style| style.bg(active))
        .child(mark);
    let (button, words) = match spec.press {
        Press::Action(action) => {
            let pressed = action.boxed_clone();
            let words = tip::words_for(label, action.as_ref(), cx);
            let button = button
                .on_click(move |_, window, cx| window.dispatch_action(pressed.boxed_clone(), cx));
            (button, words)
        }
        Press::Open(url) => {
            let button = button.on_click(move |_, _, cx| cx.open_url(&url));
            (button, tip::Words::plain(label))
        }
    };
    tip::bind(button, format!("tip-{id}"), words, cx).into_any_element()
}
