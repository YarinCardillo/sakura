//! The menu at the right of the task header: "Stop agent", "Open a terminal" (or "Close the terminal" while one
//! is open) and "Archive". An item whose action runs is disabled, so a second press does nothing; a
//! failure, in the sentence `box` printed, is written under the header.

use super::chrome::keep_click;
use super::desk::Desk;
use crate::data::actions::{ActionKey, Actions};
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette, Theme};
use gpui::InteractiveElement as _;
use gpui::{div, px, AnyElement, Context, Entity, IntoElement, ParentElement, Styled};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use sakura_core::model::{self, RemoveRule};
use sakura_core::models::Task;

/// What the menu acts on, read when the header is drawn.
#[derive(Debug, Clone, Copy)]
pub struct MenuFacts {
    pub is_agent_running: bool,
    pub is_terminal_open: bool,
    pub can_remove: bool,
}

/// The menu for `task`.
pub fn view(task: &Task, facts: MenuFacts, actions: &Actions, cx: &Context<Desk>) -> AnyElement {
    let palette = Theme::palette(cx);
    let is_stopping = actions.is_running(&ActionKey::stop_agent(&task.name));
    let is_removing = actions.is_running(&ActionKey::remove(&task.name));
    let (desk, name) = (cx.entity(), task.name.clone());
    let trigger = Button::new("task-menu")
        .debug_selector(|| "task-menu".into())
        .custom(quiet_variant(&palette, cx))
        .w(theme::CONTROL_HEIGHT)
        .h(theme::CONTROL_HEIGHT)
        .p(gpui::Pixels::ZERO)
        .child(dots(palette.text_secondary))
        .dropdown_menu_with_anchor(gpui::Anchor::TopRight, move |menu, _, _| {
            items(menu, &desk, &name, &facts, is_stopping, is_removing)
        });
    // The header moves the window; a press on the menu must not. The dots end on the content inset; the
    // button's fill bleeds past it.
    keep_click(
        div()
            .id("task-menu-area")
            .flex_none()
            .mr(-theme::CONTROL_BLEED)
            .child(trigger),
    )
    .into_any_element()
}

/// Whether "Archive" can be pressed for `task` at all.
pub fn can_remove(task: &Task) -> bool {
    !matches!(model::remove_rule(task), RemoveRule::Disabled { .. })
}

fn items(
    menu: PopupMenu,
    desk: &Entity<Desk>,
    task: &str,
    facts: &MenuFacts,
    is_stopping: bool,
    is_removing: bool,
) -> PopupMenu {
    let on = |action: fn(&mut Desk, &str, &mut gpui::Window, &mut Context<Desk>)| {
        let (desk, task) = (desk.clone(), task.to_string());
        move |_: &gpui::ClickEvent, window: &mut gpui::Window, cx: &mut gpui::App| {
            desk.update(cx, |desk, cx| action(desk, &task, window, cx));
        }
    };
    let terminal_label = if facts.is_terminal_open {
        tr(Key("task_menu.close_terminal"))
    } else {
        tr(Key("task_menu.open_terminal"))
    };
    menu.min_w(px(theme::TASK_MENU_WIDTH))
        .item(
            PopupMenuItem::new(tr(Key("agent.stop_agent")))
                .disabled(!facts.is_agent_running || is_stopping)
                .on_click(on(|desk, task, _, cx| {
                    desk.stop_agent(task.to_string(), cx)
                })),
        )
        .item(
            // The terminal has its own place beside the agent: it opens whether an agent works or not.
            PopupMenuItem::new(terminal_label).on_click(on(Desk::toggle_terminal)),
        )
        .separator()
        .item(
            PopupMenuItem::new(tr(Key("task_menu.delete")))
                .disabled(!facts.can_remove || is_removing)
                .on_click(on(|desk, task, _, cx| desk.ask_remove(task, cx))),
        )
}

/// Three dots in a row, one icon wide: the trigger's only content.
fn dots(colour: gpui::Hsla) -> impl IntoElement {
    div()
        .debug_selector(|| "task-menu-icon".into())
        .flex_none()
        .w(theme::ICON_SIZE)
        .child(crate::ui::glyph::overflow(colour))
}

/// No fill at rest, the secondary fill under the pointer and while the menu is open.
pub fn quiet_variant(palette: &Palette, cx: &gpui::App) -> ButtonCustomVariant {
    ButtonCustomVariant::new(cx)
        .color(gpui::transparent_black())
        .foreground(palette.text_secondary)
        .hover(palette.fill)
        .active(palette.fill_hover)
}
