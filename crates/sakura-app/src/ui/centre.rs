//! The centre of a lavoro's page: the view chosen, right under the header, whose row carries the switch
//! between the centre's views. The agent's view is always there; the lavoro's own terminal is a second
//! view beside it once one was opened. Opening the terminal never replaces, hides for good or ends the agent: each has its own
//! session and its own place, and the way back is the switch itself. When the terminal's shell ends (an
//! `exit`), only the terminal goes: the centre returns to the agent's view and one quiet line says so,
//! with the way to open it again. Which view a lavoro shows is kept per lavoro by the desk; the one place
//! where a view of the centre becomes its content is [`Desk::centre`].

use super::chrome::keep_click;
use super::console::rules::ConsoleKind;
use super::console::{self, TerminalEnded};
use super::desk::Desk;
use super::status_bar::TerminalItem;
use super::view_row::{self, ViewTab};
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette, Theme};
use gpui::{
    div, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement,
    StatefulInteractiveElement as _, Styled, Window,
};
use sakura_core::models::Task;

/// The views of the centre, in the order of its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CentreView {
    /// The lavoro's agent.
    #[default]
    Agent,
    /// The lavoro's own terminal.
    Terminal,
}

impl CentreView {
    /// The name the view's elements are found by, in any language.
    pub fn id(self) -> &'static str {
        match self {
            CentreView::Agent => "agent",
            CentreView::Terminal => "terminal",
        }
    }

    /// The label on the row.
    pub fn label(self) -> &'static str {
        match self {
            CentreView::Agent => tr(Key("view.agent")),
            CentreView::Terminal => tr(Key("view.terminal")),
        }
    }
}

impl Desk {
    /// The view the centre of `task` shows: its terminal only while it has one.
    pub fn centre_view(&self, task: &str, cx: &gpui::App) -> CentreView {
        let asked = self.centre_views.get(task).copied().unwrap_or_default();
        match self.has_terminal(task, cx) {
            true => asked,
            false => CentreView::Agent,
        }
    }

    /// Shows `view` in the centre of `task`.
    pub fn show_centre_view(&mut self, task: &str, view: CentreView, cx: &mut Context<Self>) {
        self.centre_views.insert(task.to_string(), view);
        cx.notify();
    }

    /// Whether `task` has a terminal of its own, open or opening.
    pub fn has_terminal(&self, task: &str, cx: &gpui::App) -> bool {
        self.consoles.read(cx).terminal(task).is_some()
    }

    /// "Open a terminal" opens the lavoro's own terminal and shows it; "Close the terminal" ends it and
    /// goes back to the agent. Neither touches the agent's session.
    pub fn toggle_terminal(&mut self, task: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_terminal(task, cx) {
            self.consoles
                .update(cx, |consoles, cx| consoles.close_terminal(task, cx));
            self.show_centre_view(task, CentreView::Agent, cx);
        } else {
            self.ended_terminals.remove(task);
            self.open_console(task, ConsoleKind::Shell, window, cx);
            self.show_centre_view(task, CentreView::Terminal, cx);
        }
    }

    /// The status bar's terminal item: shows the terminal of the lavoro on screen, opening one when it
    /// has none, and returns to the agent when the terminal is what is shown.
    pub fn switch_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.shown(cx) else {
            return;
        };
        match (self.has_terminal(&task, cx), self.centre_view(&task, cx)) {
            (false, _) => self.toggle_terminal(&task, window, cx),
            (true, CentreView::Terminal) => self.show_centre_view(&task, CentreView::Agent, cx),
            (true, CentreView::Agent) => self.show_centre_view(&task, CentreView::Terminal, cx),
        }
    }

    /// Where the lavoro on screen stands with its terminal, for the status bar; none while no lavoro
    /// is shown.
    pub fn terminal_item(&self, cx: &gpui::App) -> Option<TerminalItem> {
        let task = self.shown(cx)?;
        let item = match (self.has_terminal(&task, cx), self.centre_view(&task, cx)) {
            (false, _) => TerminalItem::None,
            (true, CentreView::Agent) => TerminalItem::Open,
            (true, CentreView::Terminal) => TerminalItem::Shown,
        };
        Some(item)
    }

    /// The shell of a lavoro's terminal ended: the centre goes back to the agent's view and says once
    /// that the terminal ended. Nothing else closes.
    pub(super) fn terminal_ended(&mut self, ended: &TerminalEnded, cx: &mut Context<Self>) {
        self.ended_terminals.insert(ended.task.clone(), ended.code);
        self.show_centre_view(&ended.task, CentreView::Agent, cx);
    }

    /// Puts away the line that says `task`'s terminal ended.
    pub fn dismiss_terminal_ended(&mut self, task: &str, cx: &mut Context<Self>) {
        self.ended_terminals.remove(task);
        cx.notify();
    }

    /// The centre of `task`'s page: the view this lavoro shows. This is the one
    /// place where a view of the centre becomes its content: a new view is a variant of [`CentreView`]
    /// and an arm here.
    pub(super) fn centre(
        &mut self,
        task: &Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = match self.centre_view(&task.name, cx) {
            CentreView::Agent => self.agent_side(task, window, cx),
            CentreView::Terminal => self.terminal_side(task, cx),
        };
        let palette = Theme::palette(cx);
        let ended = self.ended_terminals.get(&task.name).copied();
        let notice =
            ended.map(|code| console::view::terminal_ended(&task.name, code, &palette, cx));
        div()
            .debug_selector(|| "centre".into())
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .debug_selector(|| "centre-body".into())
                    .flex_1()
                    .min_h_0()
                    .child(body),
            )
            .children(notice)
            .into_any_element()
    }

    /// The lavoro's own terminal as the centre shows it.
    fn terminal_side(&self, task: &Task, cx: &mut Context<Self>) -> AnyElement {
        let consoles = self.consoles.read(cx);
        let row = self.facts.read(cx).row(&task.name);
        match (consoles.terminal(&task.name), row) {
            (Some(console), Some(row)) => {
                let palette = Theme::palette(cx);
                console::view::view(row, console, consoles.can_open(), &palette, cx)
            }
            _ => div().into_any_element(),
        }
    }

    /// The switch between the centre's views, for the header's row: the agent always, the terminal while
    /// the lavoro has one. It is there with one view too, so that opening a terminal moves nothing.
    pub(super) fn centre_switch(
        &self,
        task: &str,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let shown = self.centre_view(task, cx);
        let tabs = self.centre_views_of(task, cx).map(|view| {
            let tab = ViewTab {
                id: view.id(),
                selector: format!("centre-view-{}", view.id()),
                label: view.label(),
                is_current: view == shown,
                counted: None,
            };
            let task = task.to_string();
            let tab = view_row::tab(tab, palette).on_click(
                cx.listener(move |desk, _, _, cx| desk.show_centre_view(&task, view, cx)),
            );
            keep_click(tab)
        });
        div()
            .debug_selector(|| "centre-views".into())
            .flex_none()
            .flex()
            .items_center()
            .gap(theme::TAB_GAP)
            .children(tabs)
            .into_any_element()
    }

    /// The views the centre of `task` has, in the switch's order.
    pub(super) fn centre_views_of(
        &self,
        task: &str,
        cx: &gpui::App,
    ) -> impl Iterator<Item = CentreView> {
        let terminal = self.has_terminal(task, cx).then_some(CentreView::Terminal);
        std::iter::once(CentreView::Agent).chain(terminal)
    }
}
