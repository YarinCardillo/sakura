//! The master's composer: the field over a row of tools, the project the message is about at its left and
//! the send button at its right. The other conversations keep the field and the button side by side.

use super::agent_menu::{MenuChoice, OnPick};
use super::{copy, ThreadView};
use crate::l10n::tr;
use crate::theme::thread as tokens;
use crate::theme::{self, Palette};
use crate::ui::kit::menu_chip_on_fill;
use gpui::{
    div, App, Context, InteractiveElement as _, IntoElement, ParentElement, SharedString, Styled,
    Window,
};
use gpui_component::menu::DropdownMenu as _;
use std::rc::Rc;

/// A chip with `label` that opens `choices` above it.
pub(super) fn choice_chip(
    id: &'static str,
    label: impl Into<SharedString>,
    title: Option<&'static str>,
    choices: Vec<MenuChoice>,
    cx: &App,
) -> impl IntoElement {
    menu_chip_on_fill(id, label, cx)
        .debug_selector(move || id.to_string())
        .dropdown_menu_with_anchor(gpui::Anchor::BottomLeft, move |menu, _, _| {
            let menu = match title {
                Some(title) => menu.label(title),
                None => menu,
            };
            let items = choices.iter().map(|choice| {
                let on_pick = choice.on_pick.clone();
                crate::ui::kit::checked_item(
                    choice.label.clone(),
                    choice.is_on,
                    move |_, window, cx| on_pick(window, cx),
                )
            });
            items.fold(menu.min_w(tokens::CHIP_MENU_WIDTH), |menu, item| {
                menu.item(item)
            })
        })
}

/// `chip` in a box that gives way when the row is short of room, so the buttons at the row's end never
/// leave the composer: the chip's word is cut, but not below [`tokens::CHIP_LEAST_WIDTH`].
fn shrinking(chip: impl IntoElement) -> gpui::Div {
    div()
        .min_w(tokens::CHIP_LEAST_WIDTH)
        .flex_shrink(1.)
        .overflow_hidden()
        .child(chip)
}

impl ThreadView {
    /// The master's composer: its field, then its tools.
    pub(super) fn master_composer(
        &self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let field = self.composer_field().capture_action(cx.listener(
            |view, _: &gpui_component::input::Paste, _, cx| {
                let pasted = cx.read_from_clipboard();
                if pasted.is_some_and(|item| view.attach_pasted(&item, cx)) {
                    cx.stop_propagation();
                }
            },
        ));
        let field = field
            .capture_action(
                cx.listener(|view, _: &gpui_component::input::MoveUp, _, cx| {
                    if view.slash_is_open(cx) {
                        view.slash_move(-1, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .capture_action(
                cx.listener(|view, _: &gpui_component::input::MoveDown, _, cx| {
                    if view.slash_is_open(cx) {
                        view.slash_move(1, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .capture_action(
                cx.listener(|view, _: &gpui_component::input::Escape, _, cx| {
                    if view.close_help(cx) {
                        cx.stop_propagation();
                    }
                }),
            );
        let tools = div()
            .w_full()
            .flex()
            .items_center()
            .gap(theme::GAP_SMALL)
            .child(self.attach_button(palette, cx))
            .children(self.project_chip(cx).map(shrinking))
            .children(self.agent_chip(cx).map(shrinking))
            .child(div().flex_1())
            .child(self.dictate_button(palette, cx))
            .child(self.send_button(palette, cx));
        div()
            .debug_selector(|| "thread-composer-box".to_string())
            .on_children_prepainted(|_children, _, _cx| {
                #[cfg(not(feature = "boxes"))]
                if let Some(first) = _children.first() {
                    crate::ui::tour::track_master_composer(
                        first.top() - tokens::COMPOSER_PADDING,
                        _cx,
                    );
                }
            })
            .w_full()
            .p(tokens::COMPOSER_PADDING)
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .rounded(tokens::COMPOSER_RADIUS)
            .bg(palette.fill)
            .children(self.attachment_chips(palette, cx))
            .child(field)
            .child(tools)
            .into_any_element()
    }

    /// "Any project", or the project the person's messages are about: a menu of the projects.
    fn project_chip(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let kit = self.master.as_ref()?;
        let names = kit.world.projects(cx);
        if names.is_empty() {
            return None;
        }
        let view = cx.entity().downgrade();
        let pick = |name: Option<String>| -> OnPick {
            let view = view.clone();
            Rc::new(move |_, cx| {
                let name = name.clone();
                view.update(cx, |view, cx| view.set_project(name, cx)).ok();
            })
        };
        let any = MenuChoice {
            label: tr(copy::PROJECT_ANY).into(),
            is_on: kit.project.is_none(),
            on_pick: pick(None),
        };
        let named = names.iter().map(|name| MenuChoice {
            label: name.clone().into(),
            is_on: kit.project.as_deref() == Some(name.as_str()),
            on_pick: pick(Some(name.clone())),
        });
        let label = match &kit.project {
            Some(name) => name.clone(),
            None => tr(copy::PROJECT_ANY).to_string(),
        };
        let choices = std::iter::once(any).chain(named).collect();
        Some(choice_chip("master-project-chip", label, None, choices, cx))
    }

    /// The project the person's next messages are about; none for any.
    pub fn set_project(&mut self, project: Option<String>, cx: &mut Context<Self>) {
        if let Some(kit) = self.master.as_mut() {
            kit.project = project;
            cx.emit(super::ThreadViewEvent::ProjectChosen);
            cx.notify();
        }
    }
}
