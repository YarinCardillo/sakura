//! The page bar: the window's title bar, one row on the canvas over both the sidebar and the page panel.
//! At its left a group as wide as the sidebar holds the room of the traffic lights and the back and
//! forward arrows, the forward arrow ending where the sidebar's rows end; the tabs start where the page
//! panel does, and a button opens a new one. With the sidebar hidden the group shrinks and the tabs follow.
//! A tab is a pill with the page's icon and name, the one shown filled; the pills shrink to a minimum width
//! and then the row scrolls. A tab is dragged along the row to reorder it ([`drag`]), and tabs opened,
//! closed or moved glide to their places.
//!
//! The panel's own header row ([`PageBar::header_row`]) holds the page's breadcrumb and its controls.
//!
//! Entry point: [`PageBar`], an entity that owns the tabs ([`sakura_core::tabs::Tabs`]) and draws them.
//! - It needs: the tabs to start from ([`PageBar::new`]); the lanes' names and marks, and how much of the
//!   sidebar is in view, set by the desk as they change.
//! - It emits [`PageBarEvent::Changed`] whenever the tabs, their order, the one shown or a tab's history
//!   changed: the desk then shows the page of `bar.tabs().active()` and stores `bar.tabs().to_json()`.
//! - [`keys::install`] binds Cmd+1 to Cmd+9, Cmd+W, Cmd+T and Cmd+[ and Cmd+].

mod buttons;
mod cursor;
mod drag;
mod glide;
mod header;
pub mod keys;
mod pill;
mod render;

#[cfg(test)]
mod close_tests;
#[cfg(test)]
mod cursor_tests;
#[cfg(test)]
mod drag_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tip_tests;

use crate::ui::kit::StateGlyph;
use crate::ui::tab_motion::haptics::{self, Haptics};
use gpui::{
    AnyElement, Bounds, Context, EventEmitter, IntoElement as _, Pixels, ScrollHandle,
    SharedString, Window,
};
use sakura_core::tabs::{Page, SplitTabs, TabId, Tabs};
use std::collections::HashMap;
use std::rc::Rc;

/// The key that puts a dragged tab back.
const ESCAPE: &str = "escape";

#[derive(Debug, PartialEq, Eq)]
enum TabMenu {
    Disabled,
    Add { active: TabId, peer: TabId },
    Peers { active: TabId, peers: Vec<TabId> },
    Pair(SplitTabs),
}

/// The window edge that will receive the dragged tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitSide {
    Left,
    Right,
}

/// The tabs, their order, the one shown or a tab's history changed: read [`PageBar::tabs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageBarEvent {
    Changed,
    SplitDropPreviewChanged,
    /// A tab was opened for a new, empty conversation of the Master.
    NewConversation,
}

/// What a lane's tab says: the project and the name that make its breadcrumb, and its state's mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneTab {
    pub project: SharedString,
    pub name: SharedString,
    /// What the lane is called by the engine and git: shown while Option is held.
    pub identity: SharedString,
    pub glyph: StateGlyph,
}

/// What the page shown puts at the right end of its header row: its buttons, filters and the like. A page with none
/// keeps the default, which draws nothing.
pub trait PageControls: Sized + 'static {
    /// The controls, drawn once per frame; `cx` is the page's own, for the listeners they carry.
    fn controls(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> AnyElement {
        gpui::Empty.into_any_element()
    }
}

/// What Cmd+[ asks of the page shown before the tab goes back: true when the page took it.
pub type InnerBack = Rc<dyn Fn(&mut gpui::App) -> bool>;

/// The page bar's state: the open tabs and what the bar says of them.
pub struct PageBar {
    tabs: Tabs,
    lanes: HashMap<String, LaneTab>,
    track_names: HashMap<String, SharedString>,
    /// How much of the sidebar is in view, from none to all of it.
    sidebar_share: f32,
    scroll: ScrollHandle,
    /// The tab the row of pills was last scrolled to, so a tab that is shown is brought into view once and
    /// the person may scroll away from it afterwards.
    scrolled_to: Option<(TabId, Option<TabId>)>,
    /// Where the tabs rest, and the tab in the hand with the tabs that give way to it.
    motion: drag::Motion,
    split_drop_bounds: Option<Bounds<Pixels>>,
    split_drop_side: Option<SplitSide>,
    strip_width: Pixels,
    /// The trackpad's taps while a tab is dragged.
    haptics: Rc<dyn Haptics>,
    /// What Cmd+[ asks of the page shown before it goes back in the tab's history: true when the page took it.
    pub(super) inner_back: Option<InnerBack>,
    /// Esc puts a tab in the hand back, wherever the keyboard is.
    _escape: gpui::Subscription,
    _dialog: gpui::Subscription,
    /// A window that stops being the active one puts a tab in the hand back; watched from the first frame.
    activation: Option<gpui::Subscription>,
}

impl EventEmitter<PageBarEvent> for PageBar {}

impl PageBar {
    /// A bar over `tabs`.
    pub fn new(tabs: Tabs, cx: &mut Context<Self>) -> Self {
        let escape = cx.observe_keystrokes(|bar, event, _, cx| {
            if event.keystroke.key == ESCAPE {
                bar.cancel_drag(cx);
            }
        });
        let dialog = cx.observe_global::<crate::ui::dialog_guard::OpenDialogs>(|bar, cx| {
            if crate::ui::dialog_guard::is_open(cx) {
                bar.cancel_drag(cx);
            }
        });
        let order = tabs.tabs().iter().map(|tab| tab.id()).collect();
        Self {
            tabs,
            lanes: HashMap::new(),
            track_names: HashMap::new(),
            sidebar_share: 1.0,
            scroll: ScrollHandle::new(),
            scrolled_to: None,
            motion: drag::Motion::new(order),
            split_drop_bounds: None,
            split_drop_side: None,
            strip_width: Pixels::ZERO,
            haptics: haptics::system(),
            inner_back: None,
            _escape: escape,
            _dialog: dialog,
            activation: None,
        }
    }

    /// Tells the bar what Cmd+[ asks of the page shown first: the page's own way back, true when it took it.
    pub fn set_inner_back(&mut self, inner: InnerBack) {
        self.inner_back = Some(inner);
    }

    /// The open tabs.
    pub fn tabs(&self) -> &Tabs {
        &self.tabs
    }

    fn tab_menu(&self, id: TabId) -> TabMenu {
        if let Some(pair) = self.tabs.split() {
            return if pair.other(id).is_some() {
                TabMenu::Pair(pair)
            } else {
                TabMenu::Disabled
            };
        }
        if self.tabs.len() < 2 || self.tabs.tab(id).is_none() {
            return TabMenu::Disabled;
        }
        let active = self.tabs.active().id();
        if id != active {
            return TabMenu::Add { active, peer: id };
        }
        let peers = self
            .tabs
            .tabs()
            .iter()
            .map(|tab| tab.id())
            .filter(|id| *id != active)
            .collect();
        TabMenu::Peers { active, peers }
    }

    fn create_menu_split(&mut self, active: TabId, peer: TabId, cx: &mut Context<Self>) -> bool {
        self.tabs.active().id() == active && self.create_split(active, peer, cx)
    }

    /// Joins two tabs and publishes the new strip when creation succeeds.
    pub fn create_split(&mut self, left: TabId, right: TabId, cx: &mut Context<Self>) -> bool {
        let created = self.tabs.create_split(left, right);
        self.tell_if(created, cx)
    }

    /// Swaps the sides of the pair containing `member`.
    pub fn swap_split(&mut self, member: TabId, cx: &mut Context<Self>) -> bool {
        let swapped = self.tabs.swap_split(member);
        self.tell_if(swapped, cx)
    }

    /// Publishes a completed resize when its pair is still current.
    pub fn set_split_share(
        &mut self,
        left: TabId,
        right: TabId,
        left_share_bps: u16,
        cx: &mut Context<Self>,
    ) -> bool {
        let resized = self.tabs.set_split_share(left, right, left_share_bps);
        self.tell_if(resized, cx)
    }

    /// Shows `page` in the tab shown, as a link inside the app does.
    pub fn navigate(&mut self, page: Page, cx: &mut Context<Self>) {
        let before = self.tabs.clone();
        self.tabs.navigate(page);
        self.tell_if(self.tabs != before, cx);
    }

    pub(super) fn show_master_in(&mut self, id: TabId, cx: &mut Context<Self>) {
        let moved = self.tabs.activate(id);
        let navigated = self.tabs.active().id() == id && self.tabs.navigate_here(Page::Master);
        self.tell_if(moved || navigated, cx);
    }

    /// Navigates from a visible tab and publishes the change.
    pub fn navigate_from(&mut self, id: TabId, page: Page, cx: &mut Context<Self>) -> bool {
        let moved = self.tabs.navigate_from(id, page);
        self.tell_if(moved, cx)
    }

    /// Opens `page` in a new tab and shows it, as Cmd+click on a link does.
    pub fn open_new(&mut self, page: Page, cx: &mut Context<Self>) -> TabId {
        let id = self.tabs.open_new(page);
        self.tell_if(true, cx);
        id
    }

    /// Opens a new tab on the Master for a new, empty conversation, as the + and Cmd+T do: the window starts
    /// that conversation when it hears of it.
    pub fn open_new_conversation(&mut self, cx: &mut Context<Self>) -> TabId {
        let id = self.open_new(Page::Master, cx);
        cx.emit(PageBarEvent::NewConversation);
        id
    }

    /// Goes back one page in the tab shown. False when it has no history behind it.
    pub fn back(&mut self, cx: &mut Context<Self>) -> bool {
        let moved = self.tabs.back();
        self.tell_if(moved, cx)
    }

    /// Goes forward one page in the tab shown. False when it has no history ahead.
    pub fn forward(&mut self, cx: &mut Context<Self>) -> bool {
        let moved = self.tabs.forward();
        self.tell_if(moved, cx)
    }

    /// Shows the tab `id`. False when it is not open or already shown.
    pub fn activate(&mut self, id: TabId, cx: &mut Context<Self>) -> bool {
        let moved = self.tabs.activate(id);
        self.tell_if(moved, cx)
    }

    /// Cmd+`digit`: 1 to 8 show the tab at that place, 9 the last one.
    pub fn activate_digit(&mut self, digit: usize, cx: &mut Context<Self>) -> bool {
        let moved = self.tabs.activate_digit(digit);
        self.tell_if(moved, cx)
    }

    /// Closes the tab `id` while another remains. False when nothing closed.
    pub fn close(&mut self, id: TabId, cx: &mut Context<Self>) -> bool {
        let closed = self.tabs.close(id);
        self.tell_if(closed, cx)
    }

    /// Closes the tab shown while another remains.
    pub fn close_active(&mut self, cx: &mut Context<Self>) -> bool {
        let id = self.tabs.active().id();
        self.close(id, cx)
    }

    /// The mark each lane of `keys` has on its tab, for the tests.
    #[cfg(test)]
    pub(crate) fn glyphs_of(&self, keys: &[String]) -> Vec<Option<StateGlyph>> {
        let glyph = |key: &String| self.lanes.get(key).map(|lane| lane.glyph);
        keys.iter().map(glyph).collect()
    }

    /// The names and marks of the lanes the tabs may show, by lane key. A lane that is not in it keeps its
    /// tab, named by its page, until the engine lists it again.
    pub fn set_lanes(&mut self, lanes: HashMap<String, LaneTab>, cx: &mut Context<Self>) {
        if self.lanes != lanes {
            self.lanes = lanes;
            cx.notify();
        }
    }

    pub(super) fn set_track_names(
        &mut self,
        names: HashMap<String, SharedString>,
        cx: &mut Context<Self>,
    ) {
        if self.track_names != names {
            self.track_names = names;
            cx.notify();
        }
    }

    /// How much of the sidebar is in view (from 0 to 1): the group at the bar's left is as wide as it.
    pub fn set_sidebar_share(&mut self, share: f32) {
        self.sidebar_share = share;
    }

    /// Replaces the trackpad feedback, so a test records the taps instead of making them.
    #[cfg(test)]
    pub(crate) fn set_haptics(&mut self, haptics: Rc<dyn Haptics>) {
        self.haptics = haptics;
    }

    /// Tells the window's tooltips what happened to the tabs under them: `act` is done to the controller, when
    /// the window has one.
    fn tell_tips(
        cx: &mut gpui::App,
        act: impl FnOnce(&mut crate::ui::tip::Tips, &mut Context<crate::ui::tip::Tips>),
    ) {
        if let Some(crate::ui::tip::TipsHandle(tips)) =
            cx.try_global::<crate::ui::tip::TipsHandle>().cloned()
        {
            tips.update(cx, act);
        }
    }

    fn tell_if(&mut self, has_changed: bool, cx: &mut Context<Self>) -> bool {
        if has_changed {
            Self::tell_tips(cx, |tips, cx| tips.close(cx));
            self.note_change(cx);
            cx.emit(PageBarEvent::Changed);
            cx.notify();
        }
        has_changed
    }
}
