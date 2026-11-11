//! The two panes of the desk that hide and come back: the sidebar (cmd-b) and the right pane of a lavoro's
//! page (cmd-alt-b). Each has a button in the status bar, a key and a menu item under "Finestra", all one
//! action; the state is kept in the preferences. Hiding changes only the layout: the terminal, its session and its scrollback
//! are the same entities before and after.

use super::chrome::drag_region;
use super::desk::Desk;
use super::slide::Slide;
use super::split;
use crate::menus::{
    self, Minimize, NewTask, PanesShown, SwitchTerminal, ToggleChanges, ToggleFollow,
    ToggleSidebar, Zoom,
};
use crate::prefs::Preferences;
use crate::theme;
use gpui::{
    div, AnyElement, App, Context, IntoElement, ParentElement, StyleRefinement, Styled, Window,
};
use sakura_core::models::Task;
use std::time::Instant;

impl Desk {
    /// Hides the sidebar, or brings it back.
    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_slide = self
            .sidebar_slide
            .toggled(Instant::now(), cx.reduce_motion());
        self.panes_changed(cx);
    }

    /// Hides the changes pane of the Agente view, or brings it back.
    pub fn toggle_changes(&mut self, cx: &mut Context<Self>) {
        self.changes_slide = self
            .changes_slide
            .toggled(Instant::now(), cx.reduce_motion());
        self.panes_changed(cx);
    }

    /// Which panes are shown, or on their way to being shown.
    pub fn panes_shown(&self) -> PanesShown {
        PanesShown {
            sidebar: self.sidebar_slide.is_shown(),
            changes: self.changes_slide.is_shown(),
        }
    }

    /// The panes as the preferences left them, at rest.
    pub(super) fn slides_from(preferences: &Preferences) -> (Slide, Slide) {
        let now = Instant::now();
        (
            Slide::at_rest(!preferences.is_sidebar_hidden, now),
            Slide::at_rest(!preferences.is_changes_hidden, now),
        )
    }

    /// Answers the window's menu commands wherever the keyboard is: the window opens with nothing focused,
    /// and an action then reaches only the handlers of the app, never an element's.
    pub(super) fn handle_window_actions(window: &Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        let desk = cx.weak_entity();
        let app: &mut App = cx;
        app.on_action(move |_: &Minimize, cx| {
            in_window(handle, cx, |window, _| window.minimize_window())
        });
        app.on_action(move |_: &Zoom, cx| in_window(handle, cx, |window, _| window.zoom_window()));
        let new_task = desk.clone();
        app.on_action(move |_: &NewTask, cx| {
            let desk = new_task.clone();
            in_window(handle, cx, move |window, cx| {
                drop(desk.update(cx, |desk, cx| desk.open_new_task(window, cx)));
            });
        });
        let terminal = desk.clone();
        app.on_action(move |_: &SwitchTerminal, cx| {
            let desk = terminal.clone();
            in_window(handle, cx, move |window, cx| {
                drop(desk.update(cx, |desk, cx| desk.switch_terminal(window, cx)));
            });
        });
        let follow = desk.clone();
        app.on_action(move |_: &ToggleFollow, cx| {
            let desk = follow.clone();
            in_window(handle, cx, move |window, cx| {
                drop(desk.update(cx, |desk, cx| desk.toggle_follow(window, cx)));
            });
        });
        let sidebar = desk.clone();
        app.on_action(move |_: &ToggleSidebar, cx| {
            drop(sidebar.update(cx, |desk, cx| desk.toggle_sidebar(cx)));
        });
        app.on_action(move |_: &ToggleChanges, cx| {
            drop(desk.update(cx, |desk, cx| desk.toggle_changes(cx)));
        });
    }

    /// Keeps the new state for the next launch and names it in the menu.
    fn panes_changed(&mut self, cx: &mut Context<Self>) {
        let shown = self.panes_shown();
        Preferences::change(cx, |preferences| {
            preferences.is_sidebar_hidden = !shown.sidebar;
            preferences.is_changes_hidden = !shown.changes;
        });
        menus::show_panes(shown, cx);
        cx.notify();
    }

    /// Whether a pane is moving at `now`, so the next frame must be drawn.
    pub(super) fn is_sliding(&self, now: Instant) -> bool {
        !self.sidebar_slide.is_at_rest(now) || !self.changes_slide.is_at_rest(now)
    }

    /// The sidebar in the room `share` of its width leaves it, sliding out to the left; nothing when hidden.
    pub(super) fn sidebar_slot(&self, share: f32) -> Option<AnyElement> {
        if share <= 0.0 {
            return None;
        }
        let sidebar = self.sidebar.clone().cached(
            StyleRefinement::default()
                .w(theme::SIDEBAR_WIDTH)
                .flex_1()
                .min_h_0(),
        );
        // Over the sidebar the window strip holds the traffic lights and moves the window; the tabs are
        // the rest of it, over the panel.
        let lights = drag_region("window-strip")
            .h(theme::WINDOW_STRIP_HEIGHT)
            .w_full()
            .flex_none();
        let slot = div()
            .w(theme::SIDEBAR_WIDTH * share)
            .h_full()
            .flex_none()
            .overflow_hidden()
            .child(
                div()
                    .w(theme::SIDEBAR_WIDTH)
                    .h_full()
                    .ml(-(theme::SIDEBAR_WIDTH * (1.0 - share)))
                    .flex()
                    .flex_col()
                    .child(lights)
                    .child(sidebar),
            );
        Some(slot.into_any_element())
    }

    /// The lines of the layout grid over the window, when a snapshot asks for them.
    #[cfg(feature = "snapshot")]
    pub(super) fn grid_overlay(&self, window: &Window, cx: &Context<Self>) -> Option<AnyElement> {
        if !crate::snapshot::wants_grid() {
            return None;
        }
        let share = self.sidebar_slide.share(Instant::now());
        let is_split = self.changes_slide.is_shown();
        let pane = self.split.read(cx).sizes().get(1).copied();
        let panel_right = window.viewport_size().width;
        let marks = crate::snapshot::GridMarks {
            has_sidebar: share >= 1.0,
            panel_left: theme::SIDEBAR_WIDTH * share + theme::PANEL_INSET * (1.0 - share),
            pane_left: pane
                .filter(|width| is_split && *width > gpui::Pixels::ZERO)
                .map(|width| panel_right - theme::PANEL_RING_WIDTH - width),
        };
        Some(crate::snapshot::grid_overlay(marks))
    }

    /// Without the snapshot feature no grid is drawn.
    #[cfg(not(feature = "snapshot"))]
    pub(super) fn grid_overlay(&self, _: &Window, _: &Context<Self>) -> Option<AnyElement> {
        None
    }

    /// The page under the header: `centre` beside the right pane of `task`, or alone when the pane is
    /// hidden.
    pub(super) fn beside_the_pane(
        &mut self,
        centre: AnyElement,
        task: &Task,
        is_pending: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Instant::now();
        let shown = self.changes_slide.share(now);
        if shown <= 0.0 {
            return centre;
        }
        let pane = self.right_pane(task, is_pending, window, cx);
        let viewport = window.viewport_size().width;
        let split = split::Split {
            state: &self.split,
            share: self.changes_share,
            shown,
            width: split::body_width(viewport, self.sidebar_slide.share(now)),
        };
        split::view(centre, pane, split, cx)
    }
}

/// Runs `act` on the window right after the action that requested it: the window is being updated while
/// its action dispatches.
fn in_window(
    handle: gpui::AnyWindowHandle,
    cx: &mut App,
    act: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    cx.defer(move |cx| {
        // The handlers of a window that was closed stay with the app: such a window has nothing to do,
        // and the window opened after it answers with its own.
        drop(handle.update(cx, |_, window, cx| act(window, cx)));
    });
}
