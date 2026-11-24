use super::*;
use crate::ui::lane_page::LanePage;
use crate::ui::split;
use crate::ui::track_page::TrackPage;
use gpui::prelude::FluentBuilder as _;
use gpui::{BoxShadow, EntityId, FocusHandle, MouseButton, Pixels, Stateful};
use gpui_base::ResizableState;

#[derive(Clone)]
pub(in crate::ui) enum TabPage {
    Inbox(Entity<InboxPage>),
    Lanes(Entity<LanesPage>),
    Scheduled(Entity<ScheduledPage>),
    Engine(Entity<EnginePage>),
    Lane(Entity<LanePage>),
    Project(Entity<ProjectPage>),
    Track(Entity<TrackPage>),
}

impl TabPage {
    fn element(self) -> AnyElement {
        match self {
            Self::Inbox(page) => page.into_any_element(),
            Self::Lanes(page) => page.into_any_element(),
            Self::Scheduled(page) => page.into_any_element(),
            Self::Engine(page) => page.into_any_element(),
            Self::Lane(page) => page.into_any_element(),
            Self::Project(page) => page.into_any_element(),
            Self::Track(page) => page.into_any_element(),
        }
    }

    fn controls(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        match self {
            Self::Inbox(page) => page.update(cx, |page, cx| page.controls(window, cx)),
            Self::Lanes(page) => page.update(cx, |page, cx| page.controls(window, cx)),
            Self::Scheduled(page) => page.update(cx, |page, cx| page.controls(cx)),
            Self::Engine(page) => page.update(cx, |page, cx| page.controls(window, cx)),
            Self::Lane(page) => page.update(cx, |page, cx| page.controls(window, cx)),
            Self::Project(_) | Self::Track(_) => gpui::Empty.into_any_element(),
        }
    }

    fn age_view(&self, snapshot: &sakura_core::engine::snapshot::Snapshot) -> Option<EntityId> {
        match self {
            Self::Inbox(page)
                if !snapshot.waits.is_empty()
                    || snapshot
                        .decided
                        .as_ref()
                        .is_some_and(|decided| !decided.is_empty()) =>
            {
                Some(page.entity_id())
            }
            Self::Lanes(page) if !snapshot.items.is_empty() => Some(page.entity_id()),
            Self::Lane(page) => Some(page.entity_id()),
            _ => None,
        }
    }
}

type PageKey = (TabId, Page);

#[derive(Default)]
pub(super) struct TabPages {
    views: HashMap<PageKey, TabPage>,
    subscriptions: HashMap<PageKey, Subscription>,
    has_claimed_inbox: bool,
    focus: HashMap<TabId, PaneFocus>,
}

struct PaneFocus {
    handle: FocusHandle,
    _on_enter: Subscription,
}

impl TabPages {
    fn retain(&mut self, keep: impl Fn(&PageKey) -> bool) {
        self.views.retain(|key, _| keep(key));
        self.subscriptions.retain(|key, _| keep(key));
    }

    pub(super) fn retain_lanes(&mut self, listed: &[String]) {
        self.retain(|(_, page)| match page {
            Page::Lane(lane) => listed.contains(lane),
            _ => true,
        });
    }
}

#[derive(Clone)]
pub(super) struct TabSplit {
    ids: (TabId, TabId),
    initial_share: u16,
    state: Entity<ResizableState>,
}

impl TabSplit {
    fn share(&self, cx: &App) -> u16 {
        split::measured_share(self.state.read(cx)).unwrap_or(self.initial_share)
    }
}

impl Desk {
    pub(super) fn scroll_active_page(&self, key: &gpui::Keystroke, window: &mut Window, cx: &App) {
        if crate::ui::dialog_guard::is_open(cx) {
            return;
        }
        let tab = self.bar.read(cx).tabs().active();
        if window.focused(cx).is_some()
            && self
                .pages
                .focus
                .get(&tab.id())
                .is_none_or(|pane| !pane.handle.is_focused(window))
        {
            return;
        }
        let scrolled = match self.pages.views.get(&(tab.id(), tab.page().clone())) {
            Some(TabPage::Engine(page)) => page.read(cx).scroll_for(key),
            Some(TabPage::Lanes(page)) => page.read(cx).scroll_for(key),
            _ => false,
        };
        if scrolled {
            window.refresh();
        }
    }

    /// Moves keyboard focus to the visible peer's container, leaving its editor untouched.
    pub(in crate::ui) fn focus_other_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if crate::ui::dialog_guard::is_open(cx) {
            return false;
        }
        let Some(other) = self.bar.read(cx).tabs().other_in_visible_split() else {
            return false;
        };
        let handle = self.pane_focus(other, window, cx);
        window.focus(&handle, cx);
        self.activate_pane(other, cx)
    }

    fn pane_focus(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        self.pages
            .focus
            .entry(id)
            .or_insert_with(|| {
                let handle = cx.focus_handle();
                let entered = cx.on_focus_in(&handle, window, move |desk, _, cx| {
                    desk.activate_pane(id, cx);
                });
                PaneFocus {
                    handle,
                    _on_enter: entered,
                }
            })
            .handle
            .clone()
    }

    fn activate_pane(&mut self, id: TabId, cx: &mut Context<Self>) -> bool {
        if crate::ui::dialog_guard::is_open(cx) {
            return false;
        }
        let (left, right) = self.bar.read(cx).tabs().visible();
        if right.is_some() && (id == left || Some(id) == right) {
            return self.bar.update(cx, |bar, cx| bar.activate(id, cx));
        }
        false
    }

    fn click_pane(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let old = self.bar.read(cx).tabs().active().id();
        let previous = self
            .pages
            .focus
            .get(&old)
            .filter(|pane| pane.handle.contains_focused(window, cx))
            .and_then(|_| window.focused(cx));
        if self.activate_pane(id, cx) {
            if let Some(previous) = previous {
                window.defer(cx, move |window, cx| {
                    if !crate::ui::dialog_guard::is_open(cx) && previous.is_focused(window) {
                        window.blur(cx);
                    }
                });
            }
        }
    }

    fn track_pane_focus(
        &mut self,
        pane: Stateful<gpui::Div>,
        id: TabId,
        palette: &theme::Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let handle = self.pane_focus(id, window, cx);
        let ring = BoxShadow::new(Pixels::ZERO, Pixels::ZERO, palette.ring)
            .spread_radius(theme::PANEL_RING_WIDTH * 2.)
            .inset();
        pane.track_focus(&handle)
            .tab_index(0)
            .focus_visible(move |style| style.shadow(vec![ring.clone()]))
            .capture_any_mouse_down(cx.listener(
                move |desk, event: &gpui::MouseDownEvent, window, cx| {
                    if event.button == MouseButton::Left {
                        desk.click_pane(id, window, cx);
                    }
                },
            ))
            // Child controls receive the mouse event before the pane's default focus handler.
            .on_any_mouse_down(|_, window, _| window.prevent_default())
    }

    #[cfg(test)]
    pub(in crate::ui) fn pane_focus_for_test(&self, id: TabId) -> Option<FocusHandle> {
        self.pages.focus.get(&id).map(|pane| pane.handle.clone())
    }

    pub(super) fn sync_tab_split(&mut self, cx: &mut Context<Self>) {
        let tabs = self.bar.read(cx).tabs();
        let (left, right) = tabs.visible();
        let split = tabs
            .split()
            .filter(|pair| (left, right) == (pair.left, Some(pair.right)));
        if split.is_some_and(|pair| {
            self.tab_split
                .as_ref()
                .is_some_and(|at| at.ids == (pair.left, pair.right))
        }) {
            return;
        }
        let previous = self.tab_split.take();
        self.tab_split = split.map(|pair| TabSplit {
            ids: (pair.left, pair.right),
            initial_share: previous
                .filter(|at| at.ids == (pair.right, pair.left))
                .map_or(pair.left_share_bps, |at| at.share(cx)),
            state: cx.new(|_| ResizableState::default()),
        });
    }

    pub(super) fn panel_body(
        &mut self,
        width: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(split) = self.tab_split.clone() else {
            let id = self.bar.read(cx).tabs().active().id();
            self.set_compact_pane(id, false, cx);
            return self
                .pane_for_tab(id, window, cx)
                .expect("the active tab is open");
        };
        let (left, right) = split.ids;
        let ratio = split::share_fraction(split.share(cx));
        let compact_limit = theme::page_split::NARROW_PANE_WIDTH;
        self.set_compact_pane(left, width * ratio < compact_limit, cx);
        self.set_compact_pane(right, width * (1. - ratio) < compact_limit, cx);
        let (Some(left_pane), Some(right_pane)) = (
            self.pane_for_tab(left, window, cx),
            self.pane_for_tab(right, window, cx),
        ) else {
            return gpui::Empty.into_any_element();
        };
        let desk = cx.weak_entity();
        split::page_tabs_view(
            left_pane,
            right_pane,
            &split.state,
            width,
            split.initial_share,
            move |state, _, cx| {
                drop(desk.update(cx, |desk, cx| {
                    desk.commit_tab_resize((left, right), state, cx)
                }));
            },
        )
    }

    fn commit_tab_resize(
        &mut self,
        ids: (TabId, TabId),
        state: &Entity<ResizableState>,
        cx: &mut Context<Self>,
    ) {
        if self
            .tab_split
            .as_ref()
            .is_none_or(|split| split.ids != ids || split.state != *state)
            || self.bar.read(cx).tabs().visible() != (ids.0, Some(ids.1))
        {
            return;
        }
        if let Some(share) = split::measured_share(state.read(cx)) {
            self.bar
                .update(cx, |bar, cx| bar.set_split_share(ids.0, ids.1, share, cx));
        }
    }

    /// Renders one pane with its own header, controls and page view.
    pub(super) fn pane_for_tab(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tab = self.bar.read(cx).tabs().tab(id)?.clone();
        let page = self.page_for_tab(&tab, window, cx);
        #[cfg(feature = "snapshot")]
        let page = self
            .catalogue
            .as_ref()
            .filter(|_| self.tab_split.is_none())
            .map_or(page, |view| view.clone().into_any_element());
        let palette = Theme::palette(cx);
        let controls = self.controls_for_tab(&tab, window, cx);
        let header = self.bar.read(cx).header_row(&tab, controls, &palette);
        let pane = div()
            .id(("page-pane", id.as_u64()))
            .debug_selector(move || format!("page-pane-{}", id.as_u64()))
            .size_full()
            .min_w_0()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .child(div().flex_1().min_h_0().child(page));
        let is_split = self.tab_split.is_some();
        Some(
            pane.when(is_split, |pane| {
                self.track_pane_focus(pane, id, &palette, window, cx)
            })
            .into_any_element(),
        )
    }

    fn set_compact_pane(&self, id: TabId, is_compact: bool, cx: &mut App) {
        let Some(tab) = self.bar.read(cx).tabs().tab(id) else {
            return;
        };
        match self.pages.views.get(&(id, tab.page().clone())) {
            Some(TabPage::Inbox(page)) if page.read(cx).is_compact() != is_compact => {
                page.update(cx, |page, cx| page.set_compact(is_compact, cx));
            }
            Some(TabPage::Lane(page)) if page.read(cx).is_compact() != is_compact => {
                page.update(cx, |page, cx| page.set_compact(is_compact, cx));
            }
            Some(TabPage::Lanes(page)) if page.read(cx).is_compact() != is_compact => {
                page.update(cx, |page, cx| page.set_compact(is_compact, cx));
            }
            _ => {}
        }
    }

    /// Routes navigation from a visible tab without changing the selected side.
    pub fn navigate_from_tab(&mut self, id: TabId, page: Page, cx: &mut Context<Self>) -> bool {
        self.bar
            .update(cx, |bar, cx| bar.navigate_from(id, page, cx))
    }

    pub(in crate::ui) fn follow_page_link(
        &mut self,
        id: TabId,
        originating_page: &Page,
        destination: Page,
        is_new_tab: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.is_visible_page(id, originating_page, cx) {
            return false;
        }
        if is_new_tab {
            self.navigate(destination, true, cx);
            return true;
        }
        self.navigate_from_tab(id, destination, cx)
    }

    pub(in crate::ui) fn is_visible_page(&self, id: TabId, page: &Page, cx: &App) -> bool {
        let tabs = self.bar.read(cx).tabs();
        let (left, right) = tabs.visible();
        (id == left || Some(id) == right) && tabs.tab(id).is_some_and(|tab| tab.page() == page)
    }

    pub(super) fn visible_tabs(&self, cx: &App) -> Vec<Tab> {
        let tabs = self.bar.read(cx).tabs();
        let (left, right) = tabs.visible();
        [Some(left), right]
            .into_iter()
            .flatten()
            .filter_map(|id| tabs.tab(id).cloned())
            .collect()
    }

    pub(super) fn sync_tab_pages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tabs = self.bar.read(cx).tabs();
        self.pages.retain(|(id, _)| tabs.tab(*id).is_some());
        self.pages.focus.retain(|id, _| tabs.tab(*id).is_some());
        let owners: Vec<_> = tabs
            .tabs()
            .iter()
            .map(|tab| tab.conversation().to_string())
            .collect();
        self.drafts
            .update(cx, |drafts, _| drafts.retain_editors(&owners));
        self.archive_hidden_master_pages(cx);
        let visible = self.visible_tabs(cx);
        for tab in &visible {
            self.ensure_tab_page(tab, window, cx);
        }
        for ((id, where_at), page) in &self.pages.views {
            let is_shown = visible
                .iter()
                .any(|tab| tab.id() == *id && tab.page() == where_at);
            match page {
                TabPage::Lane(page) => page.update(cx, |page, cx| page.set_shown(is_shown, cx)),
                TabPage::Scheduled(page) => {
                    page.update(cx, |page, cx| page.set_shown(is_shown, cx))
                }
                _ => {}
            }
        }
    }

    pub(super) fn open_scheduled_run(
        &mut self,
        id: TabId,
        run: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.navigate_from_tab(id, Page::Scheduled, cx) {
            return;
        }
        let tabs = self.bar.read(cx).tabs();
        let tab = tabs
            .tab(id)
            .filter(|tab| *tab.page() == Page::Scheduled)
            .unwrap_or_else(|| tabs.active())
            .clone();
        self.ensure_tab_page(&tab, window, cx);
        if let Some(TabPage::Scheduled(page)) = self.pages.views.get(&(tab.id(), Page::Scheduled)) {
            page.update(cx, |page, cx| page.open_run(run.to_owned(), cx));
        }
    }

    pub(super) fn update_inbox_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        let visible = self.visible_tabs(cx);
        for ((id, _), page) in &self.pages.views {
            if let TabPage::Inbox(page) = page {
                let is_shown = visible
                    .iter()
                    .any(|tab| tab.id() == *id && *tab.page() == Page::Inbox);
                page.update(cx, |page, cx| {
                    page.keep_focus_only_while_shown(is_shown, window, cx)
                });
            }
        }
    }

    pub(super) fn repaint_ages(&self, cx: &mut App) {
        let Some(snapshot) = self.facts.read(cx).snapshot() else {
            return;
        };
        let views: Vec<_> = self
            .visible_tabs(cx)
            .iter()
            .filter_map(|tab| {
                self.pages
                    .views
                    .get(&(tab.id(), tab.page().clone()))?
                    .age_view(snapshot)
            })
            .collect();
        for view in views {
            cx.notify(view);
        }
    }

    /// Renders the view that belongs to this tab and page.
    pub(super) fn page_for_tab(
        &mut self,
        tab: &Tab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_tab_page(tab, window, cx);
        match tab.page() {
            Page::Master => self.masters[tab.conversation()].clone().into_any_element(),
            page => self.pages.views[&(tab.id(), page.clone())]
                .clone()
                .element(),
        }
    }

    pub(super) fn controls_for_tab(
        &self,
        tab: &Tab,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        if *tab.page() == Page::Master {
            return self.masters[tab.conversation()]
                .update(cx, |page, cx| page.controls(window, cx));
        }
        self.pages.views[&(tab.id(), tab.page().clone())].controls(window, cx)
    }

    pub(super) fn ensure_tab_page(
        &mut self,
        tab: &Tab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = tab.page();
        if *page == Page::Master {
            self.claim_master_page(tab, window, cx);
            return;
        }
        let key = (tab.id(), page.clone());
        if self.pages.views.contains_key(&key) {
            return;
        }
        let entity = match page {
            Page::Inbox => TabPage::Inbox(self.new_tab_inbox(tab.id(), window, cx)),
            Page::Lanes => TabPage::Lanes(self.lanes_view(tab.id(), cx)),
            Page::Scheduled => TabPage::Scheduled(
                cx.new(|cx| ScheduledPage::new(self.facts.clone(), self.source.clone(), cx)),
            ),
            Page::Engine => TabPage::Engine(self.new_tab_engine(tab.id(), window, cx)),
            Page::Lane(lane) => TabPage::Lane(self.new_tab_lane(tab, lane, cx)),
            Page::Project(folder) => {
                let desk = cx.weak_entity();
                TabPage::Project(
                    cx.new(|cx| ProjectPage::new(desk, tab.id(), self.facts.clone(), folder, cx)),
                )
            }
            Page::Track(id) => {
                let desk = cx.weak_entity();
                TabPage::Track(cx.new(|cx| {
                    TrackPage::new(
                        desk,
                        tab.id(),
                        self.facts.clone(),
                        self.source.clone(),
                        id.clone(),
                        cx,
                    )
                }))
            }
            Page::Master => unreachable!(),
        };
        self.pages.views.insert(key, entity);
    }

    fn new_tab_inbox(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InboxPage> {
        let page = if self.pages.has_claimed_inbox {
            let (facts, answers, source) = (
                self.facts.clone(),
                self.answers.clone(),
                self.source.clone(),
            );
            let page = cx.new(|cx| InboxPage::new(facts, answers, source, cx));
            page.update(cx, |inbox, cx| inbox.offer_adding(self.adds.clone(), cx));
            page
        } else {
            self.pages.has_claimed_inbox = true;
            self.inbox.clone()
        };
        let subscription = cx.subscribe_in(&page, window, move |desk, _, event, window, cx| {
            if desk.is_visible_page(id, &Page::Inbox, cx) {
                desk.inbox_asked(id, event, window, cx);
            }
        });
        self.pages
            .subscriptions
            .insert((id, Page::Inbox), subscription);
        page
    }

    pub(in crate::ui) fn active_inbox(&self, cx: &App) -> Option<&Entity<InboxPage>> {
        let tab = self.bar.read(cx).tabs().active();
        if *tab.page() != Page::Inbox {
            return None;
        }
        match self.pages.views.get(&(tab.id(), Page::Inbox)) {
            Some(TabPage::Inbox(page)) => Some(page),
            _ => None,
        }
    }

    pub(in crate::ui) fn lanes_view(
        &mut self,
        id: TabId,
        cx: &mut Context<Self>,
    ) -> Entity<LanesPage> {
        let key = (id, Page::Lanes);
        if let Some(TabPage::Lanes(page)) = self.pages.views.get(&key) {
            return page.clone();
        }
        let desk = cx.weak_entity();
        let groups = Preferences::try_current(cx)
            .map(|prefs| prefs.open_lane_groups)
            .unwrap_or_default();
        let page = cx.new(|cx| LanesPage::new(desk, id, self.facts.clone(), groups, cx));
        self.pages.views.insert(key, TabPage::Lanes(page.clone()));
        page
    }

    fn new_tab_engine(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<EnginePage> {
        let page =
            cx.new(|cx| EnginePage::new(self.facts.clone(), self.source.clone(), window, cx));
        let subscription = cx.subscribe_in(&page, window, move |desk, _, event, window, cx| {
            if desk.is_visible_page(id, &Page::Engine, cx) {
                desk.engine_asked(id, event, window, cx);
            }
        });
        self.pages
            .subscriptions
            .insert((id, Page::Engine), subscription);
        page
    }

    fn new_tab_lane(&mut self, tab: &Tab, lane: &str, cx: &mut Context<Self>) -> Entity<LanePage> {
        let data = (self.answers.clone(), self.inbox.downgrade());
        let page = cx.new(|cx| {
            LanePage::new(
                lane.to_string(),
                self.facts.clone(),
                data,
                self.source.clone(),
                cx,
            )
        });
        let key = tab.page().clone();
        let owner = tab.conversation().to_string();
        self.drafts
            .update(cx, |drafts, cx| drafts.open_editor(&key, &owner, cx));
        let drafts = self.drafts.read(cx);
        let restored = drafts.restored(&key, &owner);
        let agents = drafts.restored_agents(&key, &owner);
        page.update(cx, |page, _| {
            if let Some(text) = restored {
                page.restore_draft(text);
            }
            page.restore_agent_drafts(agents);
        });
        let id = tab.id();
        let subscription = cx.subscribe(&page, {
            let key = key.clone();
            move |desk, page, event: &LanePageEvent, cx| match event {
                LanePageEvent::OpenInbox => {
                    desk.follow_page_link(id, &key, Page::Inbox, false, cx);
                }
                LanePageEvent::OpenTrack(track) => {
                    desk.follow_page_link(id, &key, Page::Track(track.clone()), false, cx);
                }
                LanePageEvent::DraftChanged => {
                    for (agent, text) in page.read(cx).drafts(cx) {
                        desk.drafts.update(cx, |drafts, cx| {
                            drafts.note_for(&key, agent.as_deref(), &owner, &text, cx)
                        });
                    }
                }
            }
        });
        self.pages.subscriptions.insert((id, key), subscription);
        page
    }

    pub(super) fn active_lane(&self, cx: &App) -> Option<&Entity<LanePage>> {
        let tab = self.bar.read(cx).tabs().active();
        self.lane_view(tab.id(), tab.page())
    }

    pub(in crate::ui) fn lane_view(&self, id: TabId, page: &Page) -> Option<&Entity<LanePage>> {
        match self.pages.views.get(&(id, page.clone())) {
            Some(TabPage::Lane(page)) => Some(page),
            _ => None,
        }
    }

    fn engine_view(&self, id: TabId) -> Option<&Entity<EnginePage>> {
        match self.pages.views.get(&(id, Page::Engine)) {
            Some(TabPage::Engine(page)) => Some(page),
            _ => None,
        }
    }

    pub(super) fn active_engine(&self, cx: &App) -> Option<&Entity<EnginePage>> {
        self.engine_view(self.bar.read(cx).tabs().active().id())
    }

    #[cfg(test)]
    pub(in crate::ui) fn inbox_for_test(&self, id: TabId) -> Option<&Entity<InboxPage>> {
        match self.pages.views.get(&(id, Page::Inbox)) {
            Some(TabPage::Inbox(page)) => Some(page),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(in crate::ui) fn project_for_test(
        &self,
        id: TabId,
        folder: &str,
    ) -> Option<&Entity<ProjectPage>> {
        match self
            .pages
            .views
            .get(&(id, Page::Project(folder.to_string())))
        {
            Some(TabPage::Project(page)) => Some(page),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(in crate::ui) fn master_for_test(
        &self,
        id: TabId,
        cx: &App,
    ) -> Option<&Entity<MasterPage>> {
        let tab = self.bar.read(cx).tabs().tab(id)?;
        self.masters.get(tab.conversation())
    }

    #[cfg(test)]
    pub(in crate::ui) fn lane_for_test(&self, lane: &str, cx: &App) -> Option<&Entity<LanePage>> {
        let tabs = self.bar.read(cx).tabs();
        let page = Page::Lane(lane.to_string());
        self.lane_view(tabs.active().id(), &page).or_else(|| {
            tabs.tabs()
                .iter()
                .find_map(|tab| self.lane_view(tab.id(), &page))
        })
    }

    pub(in crate::ui) fn active_lanes(&self, cx: &App) -> Option<&Entity<LanesPage>> {
        let id = self.bar.read(cx).tabs().active().id();
        match self.pages.views.get(&(id, Page::Lanes)) {
            Some(TabPage::Lanes(page)) => Some(page),
            _ => None,
        }
    }
}
