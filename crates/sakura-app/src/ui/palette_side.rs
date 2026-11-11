//! The command palette in the desk's window: where the window is when Cmd+K is pressed, and what the desk
//! does for a command only it can carry out. A command on a lavoro names its lavoro; the palette checks
//! it against the window as it is when it is chosen, and the desk finds the lavoro again by name.

use super::desk::Desk;
use super::palette::{self, Ask, Palette, PaletteEvent, PlayState, ShownTask, Situation};
use super::publishing::{offer, Offer};
use super::task_menu::can_remove;
use gpui::{App, AppContext as _, Context, Entity, Subscription, Window};

impl Desk {
    /// The palette of this window: Cmd+K lists the commands for where the window is at that moment.
    pub(super) fn palette_in(
        window: &mut Window,
        observers: &mut Vec<Subscription>,
        cx: &mut Context<Self>,
    ) -> Entity<Palette> {
        let palette = cx.new(|cx| Palette::new(window, cx));
        let desk = cx.weak_entity();
        let situation = move |cx: &App| {
            let desk = desk.upgrade().map(|desk| desk.read(cx).situation(cx));
            desk.unwrap_or_else(|| nowhere(cx))
        };
        palette::install(&palette, situation, window, cx);
        observers.push(cx.subscribe_in(
            &palette,
            window,
            |desk, _, event: &PaletteEvent, window, cx| {
                let PaletteEvent::Asked(ask) = event;
                desk.palette_asked(ask, window, cx);
            },
        ));
        palette
    }

    /// Where the window is: its lavori in the sidebar's order, the one shown and what can be done with
    /// it now, the panes, the language.
    pub fn situation(&self, cx: &App) -> Situation {
        let facts = self.facts.read(cx);
        let listed = facts.rows().iter().filter(|row| !row.is_pending);
        let shown = self.shown(cx).and_then(|name| facts.row(&name));
        let shown = shown.filter(|row| !row.is_pending).map(|row| ShownTask {
            name: row.task.name.clone(),
            view: self.pane_view(&row.task.name),
            can_publish: match offer(&row.task) {
                Offer::Nothing => false,
                Offer::AfterTake => true,
                Offer::ByItsAgent => self.can_ask_agent(&row.task.name, cx),
            },
            can_archive: can_remove(&row.task),
            play: self.play_state(&row.task.name),
        });
        Situation {
            tasks: listed.map(|row| row.task.name.clone()).collect(),
            shown,
            panes: self.panes_shown(),
            language: crate::language::choice(cx),
            can_close_tab: self.strip.read(cx).tabs().len() > 1,
            panel_left: self.panel_left_now(),
        }
    }

    /// Where the Play of `task` stands, as the palette offers it.
    fn play_state(&self, task: &str) -> PlayState {
        let plays = &self.plays;
        match (plays.is_offered(task), plays.is_running(task)) {
            (false, _) => PlayState::Absent,
            (true, true) => PlayState::Running,
            (true, false) if plays.can_start(task) => PlayState::AtRest,
            (true, false) => PlayState::Unable,
        }
    }

    /// Carries out what the palette asked, on the lavoro it names.
    fn palette_asked(&mut self, ask: &Ask, window: &mut Window, cx: &mut Context<Self>) {
        match ask {
            Ask::ShowTask(task) => self.select(task.clone(), cx),
            Ask::ShowView { task, view } => {
                if !self.panes_shown().changes {
                    self.toggle_changes(cx);
                }
                self.show_pane_view(task, *view, cx);
            }
            Ask::Publish(task) => self.publish(task, window, cx),
            Ask::Archive(task) => self.ask_remove(task, cx),
        }
    }
}

/// The window with no desk left: nothing to go to and nothing shown.
fn nowhere(cx: &App) -> Situation {
    Situation {
        tasks: Vec::new(),
        shown: None,
        panes: crate::menus::PanesShown {
            sidebar: true,
            changes: true,
        },
        language: crate::language::choice(cx),
        can_close_tab: false,
        panel_left: gpui::Pixels::ZERO,
    }
}
