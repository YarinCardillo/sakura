//! The strip's state and what the desk asks of it. Every change to the tabs goes through here, so the strip
//! can glide the tabs to their new places and tell the desk.

use super::drag::TabBox;
use super::haptics::{self, Haptics};
use super::motion::Sprung;
use super::paint::Prepared;
use super::pointer::{Drag, Press};
use gpui::{point, px, AppContext as _, Context, Entity, EventEmitter, Pixels, Point};
use gpui_base::TooltipOverlay;
use sakura_core::model::TaskState;
use sakura_core::tabs::{Placement, TabId, Tabs};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

/// The key that puts a dragged tab back.
const ESCAPE: &str = "escape";

/// The tabs, their order or the one shown changed: read [`Strip::tabs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripEvent {
    Changed,
}

/// The tab strip: the tabs and how they are drawn right now.
pub struct Strip {
    pub(super) tabs: Tabs,
    /// The state of each lavoro that has a tab, as the window shows it: its tab's mark.
    pub(super) states: HashMap<String, TaskState>,
    /// How far the first tab starts from the panel's left edge.
    pub(super) leading_inset: f32,
    /// How far each tab is drawn from its slot: stepping aside in a drag, gliding home after one.
    pub(super) offsets: HashMap<TabId, Sprung>,
    pub(super) press: Option<Press>,
    pub(super) drag: Option<Drag>,
    /// The corner morph's progress as last drawn: what the active tab's drawn distance gives, and fully
    /// round with no tab open.
    pub(super) corner: f32,
    pub(super) last_frame: Option<Instant>,
    /// Where each tab rests, from the strip's left edge, as last laid out.
    pub(super) slots: Vec<(TabId, TabBox)>,
    /// The strip's top-left corner in the window, as last laid out.
    pub(super) origin: Point<Pixels>,
    pub(super) scroll: f32,
    /// How far the strip can be scrolled, as last laid out: the room its tabs take past its end.
    pub(super) furthest: f32,
    /// The tab shown must be brought into sight at the next layout: it changed, or the tabs did. A wheel
    /// may then scroll it out of sight again, and it stays where the person put it.
    pub(super) is_showing_active: bool,
    /// Where the tabs rested before a change: at the next layout each glides from there.
    pub(super) glide_from: Option<HashMap<TabId, f32>>,
    /// The strip's own tooltips: one delay group.
    pub(super) tips: Entity<TooltipOverlay>,
    /// The fused shape's paths for the geometry last drawn, and how many times they were built.
    pub(super) prepared: Option<Rc<Prepared>>,
    pub(super) shapes_built: usize,
    /// The tab whose tooltip is requested or shown: the one the pointer came onto last.
    pub(super) tip_owner: Option<TabId>,
    /// The trackpad's taps while a tab is dragged.
    pub(super) haptics: Rc<dyn Haptics>,
    /// Esc puts a tab in the hand back, wherever the keyboard is.
    _escape: gpui::Subscription,
    /// A window that stops being the active one puts a tab in the hand back: the release that would
    /// land it may never reach this window. Watched from the first frame, when the window is known.
    pub(super) activation: Option<gpui::Subscription>,
}

impl EventEmitter<StripEvent> for Strip {}

impl Strip {
    /// A strip over `tabs` (a stored strip, or `Tabs::new()`, which has no tab).
    pub fn new(tabs: Tabs, cx: &mut Context<Self>) -> Self {
        let escape = cx.observe_keystrokes(|strip, event, _, cx| {
            if event.keystroke.key == ESCAPE {
                strip.cancel_drag(cx);
            }
        });
        Self {
            tabs,
            states: HashMap::new(),
            leading_inset: 0.,
            offsets: HashMap::new(),
            press: None,
            drag: None,
            corner: 0.,
            last_frame: None,
            slots: Vec::new(),
            origin: point(px(0.), px(0.)),
            scroll: 0.,
            furthest: 0.,
            is_showing_active: true,
            glide_from: None,
            tips: cx.new(|_| super::tips::overlay()),
            prepared: None,
            shapes_built: 0,
            tip_owner: None,
            haptics: haptics::system(),
            _escape: escape,
            activation: None,
        }
    }

    /// The tabs, their order and the one shown.
    pub fn tabs(&self) -> &Tabs {
        &self.tabs
    }

    /// Says the state of the lavori that have a tab, as the sidebar and the header show it: each tab's
    /// mark is its lavoro's state.
    pub fn set_states(&mut self, states: HashMap<String, TaskState>, cx: &mut Context<Self>) {
        if self.states != states {
            self.states = states;
            cx.notify();
        }
    }

    /// How many times the strip built the fused shape's paths since it was made.
    #[cfg(test)]
    pub(super) fn paths_built(&self) -> usize {
        self.shapes_built
    }

    /// Where the tab `id` is drawn in the window as of the last frame (in a drag: under the pointer).
    #[cfg(test)]
    pub fn tab_bounds(&self, id: TabId) -> Option<gpui::Bounds<Pixels>> {
        let (_, slot) = self.slots.iter().find(|(tab, _)| *tab == id)?;
        let aside = match self.drag {
            Some(drag) if drag.id == id => drag.dx,
            _ => self.offsets.get(&id).map_or(0., |offset| offset.value),
        };
        let origin = self.origin + point(px(slot.left + aside), px(super::metrics::TOP_INSET));
        let extent = gpui::size(px(slot.width), px(super::metrics::TAB_HEIGHT));
        Some(gpui::Bounds::new(origin, extent))
    }

    /// How far the first tab starts from the panel's left edge: nothing beside the sidebar, the traffic
    /// lights' room when it is hidden. A tab is flush on the panel's corner only when this is nothing.
    pub fn set_leading_inset(&mut self, inset: Pixels) {
        self.leading_inset = f32::from(inset).max(0.);
    }

    /// The wheel turned over the strip by `delta` px (sideways, or up and down on a plain wheel): tabs
    /// that do not fit come into sight. Nothing moves when every tab fits, or while one is in the hand.
    pub(super) fn scroll_by(&mut self, delta: f32, cx: &mut Context<Self>) {
        if self.press.is_some() {
            return;
        }
        let scrolled = (self.scroll - delta).clamp(0., self.furthest);
        if scrolled != self.scroll {
            self.scroll = scrolled;
            self.tip_owner = None;
            self.tips.update(cx, |tips, cx| tips.hide(cx));
            cx.notify();
        }
    }

    /// Replaces the trackpad feedback: [`haptics::Silent`] for a window that must stay out of the way.
    #[cfg(test)]
    pub fn set_haptics(&mut self, haptics: Rc<dyn Haptics>) {
        self.haptics = haptics;
    }

    /// Shows the lavoro `task` in the tab already on it, else in a new tab of its own (a click on a sidebar
    /// row, a lavoro just made): one rule, and the lavoro on screen never loses its tab to another.
    pub fn show(&mut self, task: &str, cx: &mut Context<Self>) {
        self.open(task, Placement::Foreground, cx);
    }

    /// Opens the lavoro `task` in a tab of its own (Cmd+click on a sidebar row).
    pub fn open(&mut self, task: &str, placement: Placement, cx: &mut Context<Self>) {
        self.change(cx, |tabs| {
            let before = tabs.clone();
            tabs.open(task, placement);
            *tabs != before
        });
    }

    /// Shows the tab `id`.
    pub fn activate(&mut self, id: TabId, cx: &mut Context<Self>) {
        self.change(cx, |tabs| tabs.activate(id));
    }

    /// Closes the tab `id` while another remains.
    pub fn close(&mut self, id: TabId, cx: &mut Context<Self>) {
        self.change(cx, |tabs| tabs.close(id));
    }

    /// Closes the tab shown (Cmd+W) while another remains; with no tab open there is nothing to close.
    pub fn close_active(&mut self, cx: &mut Context<Self>) {
        if let Some(shown) = self.tabs.active_id() {
            self.close(shown, cx);
        }
    }

    /// "Close other tabs": keeps the tab `id` only.
    pub fn close_others(&mut self, id: TabId, cx: &mut Context<Self>) {
        self.change(cx, |tabs| tabs.close_others(id));
    }

    /// Cmd+`digit`: 1 to 8 by place, 9 the last tab.
    pub fn select_position(&mut self, digit: usize, cx: &mut Context<Self>) {
        self.change(cx, |tabs| {
            let before = tabs.active_id();
            let now = tabs.select_position(digit);
            now.is_some() && now != before
        });
    }

    /// Closes the tabs of tasks that no longer exist.
    pub fn retain_tasks(&mut self, exists: impl Fn(&str) -> bool, cx: &mut Context<Self>) {
        self.change(cx, |tabs| tabs.retain_tasks(exists));
    }

    /// Applies `act` to the tabs; when it changed them, any drag ends, the tabs glide from where they
    /// stood, and the desk is told. A newly chosen tab is simply drawn as the one shown: nothing animates
    /// on a tab switch.
    pub(super) fn change(&mut self, cx: &mut Context<Self>, act: impl FnOnce(&mut Tabs) -> bool) {
        let stood = self.slot_lefts();
        if !act(&mut self.tabs) {
            return;
        }
        self.end_drag();
        self.is_showing_active = true;
        self.glide_from = Some(stood);
        self.offsets
            .retain(|id, _| self.tabs.index_of(*id).is_some());
        self.tip_owner = None;
        self.tips.update(cx, |tips, cx| tips.hide(cx));
        cx.emit(StripEvent::Changed);
        cx.notify();
    }

    /// Starts watching `window` for the loss of its active state, once.
    pub(super) fn watch_window(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if self.activation.is_some() {
            return;
        }
        let watch = cx.observe_window_activation(window, |strip, window, cx| {
            if !window.is_window_active() {
                strip.cancel_drag(cx);
            }
        });
        self.activation = Some(watch);
    }

    /// Where each tab rests now, by tab.
    pub(super) fn slot_lefts(&self) -> HashMap<TabId, f32> {
        self.slots
            .iter()
            .map(|(id, slot)| (*id, slot.left))
            .collect()
    }

    /// The tab in the hand, if any, goes back among the others: it and the tabs that stood aside head home.
    pub(super) fn end_drag(&mut self) {
        self.press = None;
        if let Some(drag) = self.drag.take() {
            let mut held = Sprung::at_rest(drag.dx);
            held.send_to(0.);
            self.offsets.insert(drag.id, held);
        }
        self.offsets
            .values_mut()
            .for_each(|offset| offset.send_to(0.));
    }
}
