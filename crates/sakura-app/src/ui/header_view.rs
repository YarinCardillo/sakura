//! The task header as its own view, so scrolling a pane below it does not draw it again. It is drawn again
//! when what it shows can change: facts, actions, consoles (the shell's menu item), the changes read (the
//! count of files on "Changes"), the desk's own changes (selection, view, the removal's confirmation) and
//! the appearance. The breath's tick does not draw it: the
//! breath layer paints its glyph's breathing half.

use super::breath::{BreathMarks, Owner};
use super::console::Consoles;
use super::desk::Desk;
use crate::data::actions::Actions;
use crate::data::changes::Changes;
use crate::data::facts::Facts;
use crate::theme::Theme;
use gpui::{
    div, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Subscription,
    WeakEntity, Window,
};

/// The header of the selected task.
pub struct HeaderView {
    desk: WeakEntity<Desk>,
    #[cfg(test)]
    renders: usize,
    _subscriptions: Vec<Subscription>,
}

impl HeaderView {
    /// The header of `desk`, drawn again when anything in `watched` (facts, actions, consoles) or the
    /// `changes` read change.
    pub fn new(
        desk: WeakEntity<Desk>,
        watched: (Entity<Facts>, Entity<Actions>, Entity<Consoles>),
        changes: Entity<Changes>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (facts, actions, consoles) = watched;
        let mut subscriptions = vec![
            cx.observe(&facts, |_, _, cx| cx.notify()),
            cx.observe(&actions, |_, _, cx| cx.notify()),
            cx.observe(&consoles, |_, _, cx| cx.notify()),
            cx.observe(&changes, |_, _, cx| cx.notify()),
            cx.observe_global::<Theme>(|_, cx| cx.notify()),
        ];
        if let Some(desk) = desk.upgrade() {
            subscriptions.push(cx.observe(&desk, |_, _, cx| cx.notify()));
        }
        Self {
            desk,
            #[cfg(test)]
            renders: 0,
            _subscriptions: subscriptions,
        }
    }

    /// How many times the header was drawn.
    #[cfg(test)]
    pub fn renders(&self) -> usize {
        self.renders
    }
}

impl Render for HeaderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        crate::snapshot::count_render("header");
        BreathMarks::clear(Owner::Header, cx);
        let header = self
            .desk
            .update(cx, |desk, cx| desk.header(window, cx))
            .ok()
            .flatten();
        div().size_full().children(header)
    }
}
