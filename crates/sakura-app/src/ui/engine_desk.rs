//! The root view of the default build: the engine's cockpit. The sidebar on the canvas, right under the
//! title bar, and the inset panel: the page bar over the page of the tab shown.
//! It reads the engine through [`EngineFacts`] and never waits on it. The desk of the `boxes` feature
//! (`desk.rs`) is the one that shows `box` tasks.

mod tab_pages;

use self::tab_pages::{TabPages, TabSplit};
use super::answers::Answers;
use super::breath::{BreathLayer, BreathMarks, Owner};
use super::draft_keeper::DraftKeeper;
use super::engine_notifying::Notifying;
use super::engine_page::{EnginePage, EnginePageEvent};
use super::engine_status_bar;
use super::files::{quick_open, QuickOpen, QuickOpenEvent, Target};
use super::inbox::{InboxEvent, InboxPage};
use super::lane_page::LanePageEvent;
use super::lane_sidebar::LaneSidebar;
use super::lanes;
use super::lanes_page::LanesPage;
use super::master_page::{MasterEvent, MasterPage};
use super::page_bar::{self, LaneTab, PageBar, PageBarEvent, PageControls, SplitSide};
use super::palette::Palette;
use super::project_add::ProjectAdds;
use super::project_page::ProjectPage;
use super::retino::Retino;
use super::scheduled::ScheduledPage;
use super::settings_modal::Section;
use super::slide::Slide;
use super::tour;
use super::{master_accounts, master_agent::MasterAgent};
use crate::data::engine_facts::{AgesChanged, EngineFacts, WaitsChanged};
use crate::data::master_history::{MasterHistory, Past};
use crate::data::sessions::Sessions;
use crate::data::source::Source;
use crate::menus::{
    self, CheckForUpdates, Minimize, NewLane, PanesShown, QuickOpenFile, ShowSettings, ShowTour,
    ToggleChanges, ToggleSidebar, Zoom,
};
use crate::prefs::Preferences;
use crate::theme::{self, Theme};
use gpui::{
    div, AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement, Render, SharedString, StyleRefinement, Styled, Subscription, Task, Window,
};
use sakura_core::engine::snapshot::Item;
use sakura_core::engine::tracks::ConversationKey;
use sakura_core::tabs::{Page, Tab, TabId, Tabs};
use std::collections::HashMap;
use std::time::Instant;

/// The window's content.
pub struct Desk {
    pub(super) facts: Entity<EngineFacts>,
    pub(super) source: Source,
    /// The page bar: the open tabs, and the controls beside them.
    pub(super) bar: Entity<PageBar>,
    pub(super) sidebar_slide: Slide,
    /// The window's one tooltip, over everything.
    tip_layer: Entity<super::tip::TipLayer>,
    /// Paints every breathing half on its own tick, so a breath draws neither the sidebar nor the page.
    breath: Entity<BreathLayer>,
    /// The dither behind the sidebar and along the bottom row, under every other view.
    pub(super) retino: Entity<Retino>,
    #[cfg(test)]
    pub(super) renders: usize,
    /// The lanes under their projects.
    sidebar: Entity<LaneSidebar>,
    /// The person's answers to the engine, shown by the pages that send them.
    pub(super) answers: Entity<Answers>,
    /// The Inbox page: the asks that wait for the person.
    pub(super) inbox: Entity<InboxPage>,
    /// The Master page of each tab that showed one: every tab keeps a conversation of its own.
    masters: HashMap<String, Entity<MasterPage>>,
    master_subscriptions: HashMap<String, [Subscription; 2]>,
    /// The Master page of the conversation kept before tabs had their own, made with the desk, until a tab that
    /// holds that conversation takes it.
    spare_master: Option<Entity<MasterPage>>,
    /// The kept sessions, which each tab's conversation is kept in.
    sessions: Sessions,
    master_histories: [MasterHistory; MasterAgent::ALL.len()],
    /// The command palette, drawn over everything.
    pub(super) palette: Entity<Palette>,
    pages: TabPages,
    tab_split: Option<TabSplit>,
    /// Adding a project, shared by the sidebar, the Inbox and the New lane composer.
    pub(super) adds: Entity<ProjectAdds>,
    /// The New lane composer requested a project to be added and comes back when the engine lists one.
    pub(super) is_waiting_for_project: bool,
    /// What the person typed and has not sent, kept across tabs and launches.
    drafts: Entity<DraftKeeper>,
    /// What the system is told of the waits, and what the Dock's badge counts.
    pub(super) notifying: Notifying,
    /// Whether the column of the lane page last toggled is shown, for the menu's word.
    is_column_shown: bool,
    /// Quick open (Cmd-P): any file of the lane shown, by name.
    pub(super) quick_open: Entity<QuickOpen>,
    /// The catalogue page, when a scripted check requested it: it takes the page's place.
    #[cfg(feature = "snapshot")]
    catalogue: Option<Entity<super::catalogue::Catalogue>>,
    _modifiers: Task<()>,
    _observers: Vec<Subscription>,
}

/// The entities a desk shows.
pub struct DeskData {
    pub facts: Entity<EngineFacts>,
    pub source: Source,
    /// Where the master's conversation is kept across launches.
    pub sessions: Sessions,
}

impl Desk {
    pub(crate) fn root(
        desk: Entity<Self>,
        window: &mut Window,
        cx: &mut Context<gpui_component::Root>,
    ) -> gpui_component::Root {
        let retino = desk.read(cx).retino.clone();
        let surface = cx.new(|_| DeskSurface { desk, retino });
        gpui_component::Root::new(surface, window, cx)
    }

    /// A desk over `data`, with the panes and tabs the preferences kept.
    pub fn new(
        data: DeskData,
        preferences: &Preferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tips = cx.new(|_| super::tip::Tips::new());
        cx.set_global(super::tip::TipsHandle(tips.clone()));
        let tip_layer = cx.new(|cx| super::tip::TipLayer::new(tips, cx));
        let tabs = Tabs::restore(preferences.tabs.as_ref());
        let bar = cx.new(|cx| PageBar::new(tabs, cx));
        page_bar::keys::install(&bar, cx);
        let facts = data.facts.clone();
        let breath =
            cx.new(|cx| BreathLayer::new(Box::new(move |cx| is_working(facts.read(cx))), cx));
        let retino = cx.new(|cx| Retino::new(window, cx));
        let retino_mask = retino.read(cx).sidebar_mask();
        let answers = cx.new(|cx| Answers::new(data.facts.clone(), data.source.clone(), cx));
        let adds = cx.new(|_| ProjectAdds::new(data.source.clone(), data.facts.clone()));
        let inbox = {
            let (facts, answers, source) =
                (data.facts.clone(), answers.clone(), data.source.clone());
            cx.new(|cx| InboxPage::new(facts, answers, source, cx))
        };
        inbox.update(cx, |inbox, cx| inbox.offer_adding(adds.clone(), cx));
        let master = {
            let (source, sessions) = (data.source.clone(), data.sessions.clone());
            let (facts, answers) = (data.facts.clone(), answers.clone());
            let inbox = inbox.clone();
            cx.new(|cx| MasterPage::new(source, sessions, (facts, answers, inbox), window, cx))
        };
        let drafts = cx.new(|_| DraftKeeper::new(preferences));
        let master_histories = master.read(cx).histories();
        let conversation = master.read(cx).conversation();
        // The text only: the files that were attached are named by the page of the tab that shows the composer.
        let kept_text = drafts.read(cx).restored(&Page::Master, "");
        if let Some(text) = kept_text {
            conversation.update(cx, |view, cx| view.write(&text, window, cx));
        }
        let (desk, facts) = (cx.weak_entity(), data.facts.clone());
        let shown = shown_page(&bar, cx);
        let collapsed = preferences.collapsed_projects.clone();
        let sidebar = {
            let inbox = inbox.clone();
            cx.new(|cx| {
                LaneSidebar::new(
                    desk,
                    (facts, inbox, adds.clone()),
                    (shown, collapsed),
                    retino_mask,
                    cx,
                )
            })
        };
        let (palette, palette_asked) = Self::palette_in(window, cx);
        let quick_open = cx.new(|cx| QuickOpen::new(window, cx));
        let target_desk = cx.weak_entity();
        quick_open::install(
            &quick_open,
            move |cx| target_desk.upgrade()?.read(cx).quick_open_target(cx),
            window,
            cx,
        );
        let inner_back_desk = cx.weak_entity();
        bar.update(cx, |bar, _| {
            bar.set_inner_back(std::rc::Rc::new(move |cx| {
                let closed = inner_back_desk.update(cx, |desk, cx| desk.close_open_agent(cx));
                closed.unwrap_or(false)
            }))
        });
        tour::reset(cx);
        let window_id = window.window_handle().window_id();
        let scroll_keys = cx.observe_keystrokes(move |desk, event, window, cx| {
            if window.window_handle().window_id() == window_id {
                desk.scroll_active_page(&event.keystroke, window, cx);
            }
        });
        let observers =
            vec![
                scroll_keys,
                palette_asked,
                tour::install(cx),
                cx.observe(&data.facts, |desk, _, cx| desk.facts_changed(cx)),
                cx.observe_in(&data.facts, window, |desk, _, window, cx| {
                    desk.open_composer_for_the_project_added(window, cx)
                }),
                cx.observe(&adds, |_, _, cx| cx.refresh_windows()),
                cx.subscribe(&data.facts, |desk, _, _: &WaitsChanged, cx| {
                    desk.tell_notifications(cx)
                }),
                cx.subscribe(&data.facts, |desk, _, _: &AgesChanged, cx| {
                    desk.repaint_ages(cx)
                }),
                cx.subscribe_in(&bar, window, |desk, _, event: &PageBarEvent, window, cx| {
                    match event {
                        PageBarEvent::Changed => desk.tab_changed(window, cx),
                        PageBarEvent::SplitDropPreviewChanged => cx.notify(),
                        PageBarEvent::NewConversation => desk.start_new_conversation(window, cx),
                    }
                }),
                cx.subscribe(&quick_open, |desk, _, event: &QuickOpenEvent, cx| {
                    let QuickOpenEvent::Picked { task, path, tab } = event;
                    desk.open_picked_file(*tab, task, path, cx);
                }),
            ];
        let sidebar_slide = Slide::at_rest(!preferences.is_sidebar_hidden, Instant::now());
        let mut desk = Self {
            _modifiers: super::held_keys::watch(window, cx),
            facts: data.facts,
            source: data.source,
            bar,
            sidebar_slide,
            tip_layer,
            breath,
            retino,
            #[cfg(test)]
            renders: 0,
            sidebar,
            answers,
            inbox,
            masters: HashMap::new(),
            master_subscriptions: HashMap::new(),
            spare_master: Some(master),
            sessions: data.sessions,
            master_histories,
            palette,
            pages: TabPages::default(),
            tab_split: None,
            adds: adds.clone(),
            is_waiting_for_project: false,
            drafts,
            notifying: Notifying::new(),
            is_column_shown: true,
            quick_open,
            #[cfg(feature = "snapshot")]
            catalogue: None,
            _observers: observers,
        };
        Self::handle_window_actions(window, cx);
        Self::handle_pane_focus(window, cx);
        Self::handle_tour_actions(window, cx);
        Self::answer_notifications(window, cx);
        menus::show_panes(desk.panes_shown(), cx);
        desk.facts_changed(cx);
        desk.tell_notifications(cx);
        desk.sync_tab_pages(window, cx);
        desk.sync_tab_split(cx);
        desk
    }

    fn inbox_asked(
        &mut self,
        id: TabId,
        event: &InboxEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InboxEvent::OpenLane(lane) => {
                self.navigate_from_tab(id, Page::Lane(lane.clone()), cx);
            }
            InboxEvent::OpenTrack(id_of_track, is_new_tab) => {
                self.follow_page_link(
                    id,
                    &Page::Inbox,
                    Page::Track(id_of_track.clone()),
                    *is_new_tab,
                    cx,
                );
            }
            InboxEvent::OpenLanes => {
                self.navigate_from_tab(id, Page::Lanes, cx);
            }
            InboxEvent::OpenScheduledRun(run) => {
                self.open_scheduled_run(id, run, window, cx);
            }
            InboxEvent::StartLane => self.open_new_lane_default(window, cx),
        }
    }

    fn resume_master(
        &self,
        page: &Entity<MasterPage>,
        agent: MasterAgent,
        past: &Past,
        cx: &mut Context<Self>,
    ) {
        let key = master_accounts::conversation_key(agent, &past.account, past.session.clone());
        if let Some(id) = self.conversation_tab(&key, cx) {
            page.read(cx).take_history(agent, past);
            self.bar.update(cx, |bar, cx| bar.show_master_in(id, cx));
        } else {
            page.update(cx, |page, cx| page.resume_here(agent, past, cx));
        }
    }

    fn conversation_tab(&self, key: &ConversationKey, cx: &App) -> Option<TabId> {
        let agent = master_provider(key)?;
        let account = master_accounts::stored_account(key);
        self.bar.read(cx).tabs().tabs().iter().find_map(|tab| {
            let slot = tab.conversation();
            let matches = if let Some(page) = self.masters.get(slot) {
                page.read(cx).kept_identity().as_ref() == Some(key)
            } else {
                let slot = master_accounts::session_key(account, slot);
                self.sessions.stored(agent.session_name(), &slot).as_deref()
                    == Some(key.session.as_str())
            };
            matches.then(|| tab.id())
        })
    }

    fn track_history(&self, key: &ConversationKey) -> Option<(MasterAgent, Past)> {
        let (index, agent) = MasterAgent::ALL
            .into_iter()
            .enumerate()
            .find(|(_, agent)| agent.code() == key.provider)?;
        let account = master_accounts::stored_account(key);
        self.master_histories[index]
            .list()
            .into_iter()
            .find(|past| past.account == account && past.session == key.session)
            .map(|past| (agent, past))
    }

    fn track_conversation_tab(&self, key: &ConversationKey, cx: &App) -> Option<TabId> {
        let id = self.conversation_tab(key, cx)?;
        let tab = self.bar.read(cx).tabs().tab(id)?;
        (self.masters.contains_key(tab.conversation()) || self.can_resume_account(key, cx))
            .then_some(id)
    }

    pub(super) fn can_open_track_conversation(&self, key: &ConversationKey, cx: &App) -> bool {
        self.track_conversation_tab(key, cx).is_some()
            || (self.can_resume_account(key, cx) && self.track_history(key).is_some())
    }

    fn can_resume_account(&self, key: &ConversationKey, cx: &App) -> bool {
        master_accounts::stored_account(key).is_empty()
            || self.facts.read(cx).snapshot().is_some_and(|snapshot| {
                snapshot.accounts.iter().any(|account| {
                    account.id == key.account
                        && account.provider == key.provider
                        && !account.is_hidden()
                })
            })
    }

    pub(super) fn open_track_conversation(
        &mut self,
        tab: TabId,
        key: &ConversationKey,
        is_new_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(owner) = self.track_conversation_tab(key, cx) {
            self.bar.update(cx, |bar, cx| bar.show_master_in(owner, cx));
            return;
        }
        if !self.can_resume_account(key, cx) {
            return;
        }
        let Some((agent, past)) = self.track_history(key) else {
            return;
        };
        let id = self.bar.update(cx, |bar, cx| {
            if is_new_tab {
                bar.open_new(Page::Master, cx)
            } else {
                bar.show_master_in(tab, cx);
                tab
            }
        });
        let holder = self.bar.read(cx).tabs().tab(id).cloned();
        if let Some(holder) = holder {
            self.claim_master_page(&holder, window, cx);
            if let Some(page) = self.masters.get(holder.conversation()) {
                page.update(cx, |page, cx| page.resume_here(agent, &past, cx));
            }
        }
    }

    /// The Master page of the tab shown; before one was made for it, the first page, which a tab takes up when
    /// it shows the Master.
    pub(super) fn shown_master(&self, cx: &App) -> Entity<MasterPage> {
        let talk = self.bar.read(cx).tabs().active().conversation();
        self.masters
            .get(talk)
            .or(self.spare_master.as_ref())
            .or_else(|| self.masters.values().next())
            .cloned()
            .expect("a Master page")
    }

    fn claim_master_page(&mut self, tab: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        let talk = tab.conversation().to_string();
        if self.masters.contains_key(&talk) {
            return;
        }
        let page = if talk.is_empty() {
            self.spare_master.take()
        } else {
            None
        };
        let page = page.unwrap_or_else(|| self.new_master_page(&talk, window, cx));
        self.drafts.update(cx, |drafts, cx| {
            drafts.open_editor(&Page::Master, &talk, cx)
        });
        let conversation = page.read(cx).conversation();
        restore_master_draft(&self.drafts, &talk, &conversation, window, cx);
        let subscriptions = self.watch_master_page(tab.id(), &talk, &page, cx);
        self.master_subscriptions
            .insert(talk.clone(), subscriptions);
        self.masters.insert(talk, page);
    }

    fn watch_master_page(
        &self,
        id: TabId,
        owner: &str,
        page: &Entity<MasterPage>,
        cx: &mut Context<Self>,
    ) -> [Subscription; 2] {
        let events = cx.subscribe(page, move |desk, page, event, cx| match event {
            MasterEvent::OpenLane(lane) => {
                desk.follow_page_link(id, &Page::Master, Page::Lane(lane.clone()), false, cx);
            }
            MasterEvent::OpenInbox => {
                desk.follow_page_link(id, &Page::Master, Page::Inbox, false, cx);
            }
            MasterEvent::ResumeHistory(agent, past) => {
                if desk.is_visible_page(id, &Page::Master, cx) {
                    desk.resume_master(&page, *agent, past, cx);
                }
            }
            MasterEvent::OpenTrack(track) => {
                desk.follow_page_link(id, &Page::Master, Page::Track(track.clone()), false, cx);
            }
        });
        let owner = owner.to_string();
        let view = page.read(cx).conversation();
        let had_files = std::cell::Cell::new(false);
        let page = page.downgrade();
        let track_key = std::cell::RefCell::new(None);
        let drafts = cx.observe(&view, move |desk, view, cx| {
            let current = page
                .upgrade()
                .and_then(|page| page.read(cx).kept_identity());
            if *track_key.borrow() != current {
                track_key.replace(current);
                cx.notify();
            }
            let (text, files) = {
                let view = view.read(cx);
                (view.draft(cx), view.attachment_names())
            };
            desk.drafts.update(cx, |drafts, cx| {
                drafts.note_for(&Page::Master, None, &owner, &text, cx);
                let has_files = !files.is_empty();
                if had_files.replace(has_files) || has_files {
                    drafts.note_files(&Page::Master, &owner, &files, cx);
                }
            });
        });
        [events, drafts]
    }

    fn new_master_page(
        &self,
        talk: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MasterPage> {
        let histories = self.master_histories.clone();
        let (source, sessions) = (self.source.clone(), self.sessions.clone());
        let deps = (self.facts.clone(), self.answers.clone(), self.inbox.clone());
        let slot = (histories, talk.to_string());
        cx.new(|cx| MasterPage::in_slot(source, sessions, deps, slot, window, cx))
    }

    fn archive_hidden_master_pages(&mut self, cx: &App) {
        let tabs = self.bar.read(cx).tabs().tabs();
        self.masters.retain(|talk, page| {
            match tabs.iter().find(|tab| tab.conversation() == talk) {
                Some(tab) if *tab.page() == Page::Master => return true,
                Some(_) => page.read(cx).archive_open(cx),
                None => page.read(cx).leave(cx),
            }
            self.master_subscriptions.remove(talk);
            false
        });
    }

    /// What the bar shows follows what the engine says: each lane's name and mark, and what is wrong with
    /// the engine. A lane the engine no longer lists keeps its tab, which then says so on its page.
    fn facts_changed(&mut self, cx: &mut Context<Self>) {
        let facts = self.facts.read(cx);
        let items = facts.snapshot().map(|snapshot| snapshot.items.as_slice());
        let items = items.unwrap_or_default();
        let lanes: HashMap<String, LaneTab> = items.iter().map(lane_tab).collect();
        let listed: Option<Vec<String>> = facts
            .snapshot()
            .map(|_| items.iter().map(Item::lane_key).collect());
        if let Some(listed) = &listed {
            self.pages.retain_lanes(listed);
            self.drafts
                .update(cx, |drafts, cx| drafts.forget_lanes_not_in(listed, cx));
        }
        let track_names =
            self.facts
                .read(cx)
                .snapshot()
                .and_then(|snapshot| match &snapshot.tracks {
                    sakura_core::engine::tracks::Tracks::Available { rows, .. } => Some(
                        rows.iter()
                            .map(|track| (track.id.clone(), super::track_page::name(track).into()))
                            .collect(),
                    ),
                    sakura_core::engine::tracks::Tracks::Unsupported => Some(HashMap::new()),
                    sakura_core::engine::tracks::Tracks::Unavailable(_) => None,
                });
        self.bar.update(cx, |bar, cx| {
            bar.set_lanes(lanes, cx);
            if let Some(names) = track_names {
                bar.set_track_names(names, cx);
            }
        });
        cx.notify();
    }

    /// The tabs changed: the page follows the tab shown, and the tabs are kept for the next launch.
    fn tab_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_tab_pages(window, cx);
        self.sync_tab_split(cx);
        let shown = shown_page(&self.bar, cx);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_current(shown, cx));
        let tabs = self.bar.read(cx).tabs().to_json();
        Preferences::change(cx, move |preferences| preferences.tabs = Some(tabs));
        self.drafts.update(cx, |drafts, cx| drafts.flush(cx));
        cx.notify();
    }

    fn start_new_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.bar.read(cx).tabs().active().clone();
        if *tab.page() != Page::Master {
            return;
        }
        self.claim_master_page(&tab, window, cx);
        let view = self.masters[tab.conversation()].read(cx).conversation();
        view.update(cx, |view, cx| view.set_project(None, cx));
    }

    /// Goes back one page in the tab shown, for the tests.
    #[cfg(test)]
    pub(super) fn go_back_for_test(&mut self, cx: &mut Context<Self>) {
        self.bar.update(cx, |bar, cx| {
            bar.back(cx);
        });
    }

    /// Shows `page`: in the tab shown, or, with `new_tab`, in a tab of its own (Cmd+click on a link). A page
    /// open in another tab is shown there instead, unless a new tab was requested.
    pub fn navigate(&mut self, page: Page, new_tab: bool, cx: &mut Context<Self>) {
        self.bar.update(cx, |bar, cx| match new_tab {
            true => drop(bar.open_new(page, cx)),
            false => bar.navigate(page, cx),
        });
    }

    /// Presses the action `action` (Approve `a`, Accept `y`) on the wait `name` names, as its button does, for
    /// the scripted checks: `name` is its task, or part of its id or title. The engine asks for a
    /// confirmation, which [`Desk::drive_confirm`] gives.
    #[cfg(any(test, feature = "snapshot"))]
    pub fn drive_press(&mut self, action: &str, name: &str, cx: &mut Context<Self>) {
        self.drive_press_gate(action, None, name, cx)
    }

    /// Like [`Desk::drive_press`], on the wait of the gate `gate` when one is named (`overlap`: the approval
    /// that lets a delivery that overlaps another lane go on).
    #[cfg(any(test, feature = "snapshot"))]
    pub fn drive_press_gate(
        &mut self,
        action: &str,
        gate: Option<&str>,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        let found = self.wait_named(name, gate, Some(action), cx);
        let Some((id, action)) = found.and_then(|(id, action)| Some((id, action?))) else {
            return eprintln!("sakura: no wait named {name:?} offers {action:?}");
        };
        let inputs = std::collections::BTreeMap::new();
        self.answers.update(cx, |answers, cx| {
            answers.press(id.clone(), action, id, inputs, cx)
        });
    }

    /// Confirms what the engine summarised for the action of the lane `name` names that waits for it, or else
    /// for the wait `name` names, for the scripted checks.
    #[cfg(any(test, feature = "snapshot"))]
    pub fn drive_confirm(&mut self, name: &str, cx: &mut Context<Self>) {
        let lane_keys = self.lane_keys_named(name, cx);
        let keys = match (lane_keys.is_empty(), self.wait_named(name, None, None, cx)) {
            (false, _) => lane_keys,
            (true, Some((id, _))) => vec![id],
            (true, None) => Vec::new(),
        };
        if keys.is_empty() {
            return eprintln!("sakura: nothing named {name:?} to confirm");
        }
        for key in keys {
            self.answers
                .update(cx, |answers, cx| answers.confirm(&key, cx));
        }
    }

    /// Presses the lane action `action` (Resume `R`, Stop `s`, Clean up `C`) of the lane of the task
    /// `task`, as its button does, for the scripted checks. Only an action the snapshot lists for the lane
    /// can be pressed.
    #[cfg(any(test, feature = "snapshot"))]
    pub fn drive_lane_action(&mut self, action: &str, task: &str, cx: &mut Context<Self>) {
        let snapshot = self.facts.read(cx).snapshot();
        let items = snapshot.map(|snapshot| snapshot.items.as_slice());
        let found = items.unwrap_or_default().iter().find_map(|item| {
            let offered = item.actions.iter().find(|offered| offered.key == action)?;
            (item.task() == task).then(|| (item.id.clone(), offered.clone()))
        });
        let Some((id, action)) = found else {
            return eprintln!("sakura: the lane {task:?} does not offer {action:?}");
        };
        let key = super::answers::lane_key(&id, &action.key);
        let inputs = std::collections::BTreeMap::new();
        self.answers
            .update(cx, |answers, cx| answers.press(key, action, id, inputs, cx));
    }

    /// The keys under which the lane `name` names holds answers that wait for a confirmation.
    #[cfg(any(test, feature = "snapshot"))]
    pub(super) fn lane_keys_named(&self, name: &str, cx: &App) -> Vec<String> {
        let snapshot = self.facts.read(cx).snapshot();
        let items = snapshot.map(|snapshot| snapshot.items.as_slice());
        let item = items
            .unwrap_or_default()
            .iter()
            .find(|item| item.task() == name);
        let Some(item) = item else {
            return Vec::new();
        };
        let answers = self.answers.read(cx);
        let keys = item
            .actions
            .iter()
            .map(|action| super::answers::lane_key(&item.id, &action.key));
        keys.filter(|key| {
            matches!(
                answers.step(key),
                Some(super::answers::Step::Confirming { .. })
            )
        })
        .collect()
    }

    /// The first wait `name` names (its task, or part of its id or title), with its action `action` when one
    /// is requested.
    #[cfg(any(test, feature = "snapshot"))]
    fn wait_named(
        &self,
        name: &str,
        gate: Option<&str>,
        action: Option<&str>,
        cx: &Context<Self>,
    ) -> Option<(String, Option<sakura_core::engine::snapshot::Action>)> {
        let snapshot = self.facts.read(cx).snapshot()?;
        snapshot.waits.iter().find_map(|wait| {
            let is_named = wait.task == name || wait.id.contains(name) || wait.title.contains(name);
            let offered = match action {
                Some(key) => Some(
                    wait.actions
                        .iter()
                        .find(|offered| offered.key == key)?
                        .clone(),
                ),
                None => None,
            };
            let at_gate = gate.is_none_or(|gate| wait.key_name() == gate);
            (is_named && at_gate).then(|| (wait.id.clone(), offered))
        })
    }

    /// Takes the Master's conversation away and makes it again over the sessions kept in `folder` (the
    /// file there is read again, as at a launch), so that it takes up the session kept before: what a
    /// relaunch does, for the scripted checks.
    #[cfg(any(test, feature = "snapshot"))]
    pub fn drive_reopen_master(
        &mut self,
        folder: Option<&std::path::Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = self.source.clone();
        let sessions = Sessions::new(folder, source.runtime().clone());
        let (facts, answers) = (self.facts.clone(), self.answers.clone());
        let inbox = self.inbox.clone();
        let bar = self.bar.read(cx);
        let tabs = bar.tabs();
        let holder = match *tabs.active().page() == Page::Master {
            true => Some(tabs.active()),
            false => tabs.tabs().iter().find(|tab| *tab.page() == Page::Master),
        };
        let holder = holder.unwrap_or(tabs.active());
        let talk = holder.conversation().to_string();
        let id = holder.id();
        let histories = self.master_histories.clone();
        let slot = (histories, talk.clone());
        self.sessions = sessions.clone();
        let page = cx.new(|cx| {
            MasterPage::in_slot(source, sessions, (facts, answers, inbox), slot, window, cx)
        });
        if talk.is_empty() {
            self.spare_master = None;
        }
        let subscriptions = self.watch_master_page(id, &talk, &page, cx);
        self.master_subscriptions
            .insert(talk.clone(), subscriptions);
        self.masters.insert(talk, page);
        cx.notify();
    }

    /// Shows the Inbox's answered asks, for the scripted checks.
    #[cfg(feature = "snapshot")]
    pub fn drive_inbox_done(&mut self, cx: &mut Context<Self>) {
        self.navigate(Page::Inbox, false, cx);
        if let Some(inbox) = self.active_inbox(cx) {
            inbox.update(cx, |inbox, cx| inbox.drive_done(cx));
        }
    }

    /// Shows the catalogue page in place of the page of the tab, and the toast it shows, for the scripted checks.
    #[cfg(feature = "snapshot")]
    pub fn drive_catalogue(
        &mut self,
        part: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = super::catalogue::Catalogue::new(part, window, cx);
        let has_toast = page.shows_toast();
        self.catalogue = Some(cx.new(|_| page));
        if has_toast {
            super::kit::Toast::show(
                window,
                cx,
                crate::l10n::tr(super::catalogue::TOAST_WORDS),
                None,
            );
        }
        cx.notify();
    }

    /// Shows the lane of the task `task`, for the scripted checks.
    #[cfg(feature = "snapshot")]
    pub fn drive_lane(&mut self, task: &str, cx: &mut Context<Self>) {
        let snapshot = self.facts.read(cx).snapshot();
        let items = snapshot.map(|snapshot| snapshot.items.as_slice());
        let found = items
            .unwrap_or_default()
            .iter()
            .find(|item| item.task() == task);
        match found.map(Item::lane_key) {
            Some(key) => self.open_lane(&key, cx),
            None => eprintln!("sakura: no lane for {task:?}"),
        }
    }

    /// Opens the conversation of the agent `run` on the lane on screen, as a press on its row does, for the
    /// scripted checks.
    #[cfg(feature = "snapshot")]
    pub fn drive_open_agent(&mut self, run: &str, cx: &mut Context<Self>) {
        match self.active_lane(cx) {
            Some(page) => page.update(cx, |page, cx| page.drive_open_agent(run, cx)),
            None => eprintln!("sakura: the lane's page was not drawn"),
        }
    }

    /// How many times the text being written in the open agent's conversation of the lane on screen grew; none
    /// when no conversation is open, for the scripted checks.
    #[cfg(feature = "snapshot")]
    pub fn drive_chat_growths(&self, cx: &gpui::App) -> Option<usize> {
        let page = self.active_lane(cx)?;
        page.read(cx).drive_chat_growths(cx)
    }

    /// Sends `text` to the lane on screen, as its composer's button does, for the scripted checks.
    #[cfg(feature = "snapshot")]
    pub fn drive_write(&mut self, text: &str, cx: &mut Context<Self>) {
        match self.active_lane(cx) {
            Some(page) => page.update(cx, |page, cx| page.drive_write(text, cx)),
            None => eprintln!("sakura: the lane's page was not drawn"),
        }
    }

    /// The Master's page, for the tests and the scripted checks.
    #[cfg(any(test, feature = "snapshot"))]
    pub fn master_page(&self, cx: &App) -> Entity<MasterPage> {
        self.shown_master(cx)
    }

    /// The Master's conversation, for the tests and the scripted checks.
    #[cfg(any(test, feature = "snapshot"))]
    pub fn master_conversation(&self, cx: &App) -> Entity<super::thread::ThreadView> {
        self.shown_master(cx).read(cx).conversation()
    }

    /// Shows the lane with key `lane` (see [`Item::lane_key`]) in the tab shown.
    #[cfg(any(test, feature = "snapshot"))]
    pub fn open_lane(&mut self, lane: &str, cx: &mut Context<Self>) {
        self.navigate(Page::Lane(lane.to_string()), false, cx);
    }

    /// The Engine page's view, for the tests.
    #[cfg(test)]
    pub(super) fn engine_page(&self, cx: &App) -> Entity<EnginePage> {
        self.active_engine(cx)
            .expect("the active Engine page")
            .clone()
    }

    /// Opens Settings as a modal over the window (Cmd+comma, the bottom row's gear).
    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings_at(Section::General, window, cx);
    }

    /// Opens Settings on `section`.
    pub fn open_settings_at(
        &mut self,
        section: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // One overlay at a time: the palette never stays over Settings.
        self.palette
            .update(cx, |palette, cx| palette.close(window, cx));
        let (source, facts) = (self.source.clone(), self.facts.clone());
        drop(super::settings_modal::open(
            source, facts, section, window, cx,
        ));
    }

    /// The Engine page requested something outside itself: Settings on a section, or the Master with a
    /// sentence started in its composer.
    fn engine_asked(
        &mut self,
        id: TabId,
        event: &EnginePageEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EnginePageEvent::OpenSettings(section) => self.open_settings_at(*section, window, cx),
            EnginePageEvent::AskMaster(text) => {
                self.navigate_from_tab(id, Page::Master, cx);
                let tabs = self.bar.read(cx).tabs();
                let source = tabs.tab(id).filter(|tab| *tab.page() == Page::Master);
                let holder = source.unwrap_or(tabs.active()).clone();
                self.claim_master_page(&holder, window, cx);
                let conversation = self.masters[holder.conversation()].read(cx).conversation();
                conversation.update(cx, |conversation, cx| conversation.write(text, window, cx));
            }
        }
    }

    fn close_open_agent(&mut self, cx: &mut Context<Self>) -> bool {
        self.active_lane(cx)
            .cloned()
            .is_some_and(|page| page.update(cx, |page, cx| page.close_agent(cx)))
    }

    /// Hides the sidebar, or brings it back.
    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_slide = self
            .sidebar_slide
            .toggled(Instant::now(), cx.reduce_motion());
        let shown = self.panes_shown();
        Preferences::change(cx, |preferences| {
            preferences.is_sidebar_hidden = !shown.sidebar
        });
        menus::show_panes(shown, cx);
        cx.notify();
    }

    /// Which panes are shown, or on their way to being shown.
    pub fn panes_shown(&self) -> PanesShown {
        PanesShown {
            sidebar: self.sidebar_slide.is_shown(),
            changes: self.is_column_shown,
        }
    }

    /// Whether the page shown has a column at its right, and if so whether it is open.
    fn column_state(&self, cx: &App) -> Option<bool> {
        let page = self.active_lane(cx)?.read(cx);
        page.has_column(cx).then(|| page.is_column_open())
    }

    /// What quick open searches: the folder of the lane shown, when it has one this Mac can read.
    pub(super) fn quick_open_target(&self, cx: &App) -> Option<Target> {
        let lane = self.quick_open_lane(cx)?;
        let tab = self.bar.read(cx).tabs().active().id();
        let source = self.active_lane(cx)?.read(cx).files_source(cx)?;
        Some(Target {
            task: lane,
            tab: Some(tab),
            source,
            panel_left: self.panel_left_now(),
        })
    }

    fn open_picked_file(
        &mut self,
        tab: Option<TabId>,
        lane: &str,
        path: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = tab else {
            return;
        };
        let page = Page::Lane(lane.to_string());
        if self.is_visible_page(tab, &page, cx) {
            if let Some(page) = self.lane_view(tab, &page).cloned() {
                page.update(cx, |page, cx| page.open_file(path, cx));
            }
        }
    }

    /// Hides the column of the lane page shown, or brings it back.
    pub fn toggle_column(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.active_lane(cx).cloned() else {
            return;
        };
        page.update(cx, |page, cx| page.toggle_column(cx));
        self.is_column_shown = page.read(cx).is_column_open();
        menus::show_panes(self.panes_shown(), cx);
        cx.notify();
    }

    fn handle_pane_focus(window: &Window, cx: &mut Context<Self>) {
        let id = window.window_handle().window_id();
        let desk = cx.weak_entity();
        let app: &mut App = cx;
        app.on_action(move |_: &page_bar::keys::FocusOtherPane, cx| {
            if crate::ui::dialog_guard::is_open(cx) {
                return;
            }
            let Some(window) = cx.active_window().filter(|handle| handle.window_id() == id) else {
                return;
            };
            let desk = desk.clone();
            in_window(window, cx, move |window, cx| {
                if window.is_window_active() && !crate::ui::dialog_guard::is_open(cx) {
                    drop(desk.update(cx, |desk, cx| desk.focus_other_pane(window, cx)));
                }
            });
        });
    }

    /// Answers the window's menu commands wherever the keyboard is.
    fn handle_window_actions(window: &Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        let desk = cx.weak_entity();
        let desk_for_settings = desk.clone();
        let app: &mut App = cx;
        app.on_action(move |_: &Minimize, cx| {
            in_window(handle, cx, |window, _| window.minimize_window())
        });
        app.on_action(move |_: &Zoom, cx| in_window(handle, cx, |window, _| window.zoom_window()));
        let new_lane_desk = desk.clone();
        let sidebar_desk = desk.clone();
        app.on_action(move |_: &ToggleSidebar, cx| {
            drop(sidebar_desk.update(cx, |desk, cx| desk.toggle_sidebar(cx)));
        });
        app.on_action(move |_: &ToggleChanges, cx| {
            drop(desk.update(cx, |desk, cx| desk.toggle_column(cx)));
        });
        let quick_open_desk = new_lane_desk.clone();
        app.on_action(move |_: &QuickOpenFile, cx| {
            let desk = quick_open_desk.clone();
            in_window(handle, cx, move |window, cx| {
                drop(desk.update(cx, |desk, cx| desk.quick_open_file(window, cx)));
            })
        });
        app.on_action(move |_: &NewLane, cx| {
            let desk = new_lane_desk.clone();
            in_window(handle, cx, move |window, cx| {
                drop(desk.update(cx, |desk, cx| desk.open_new_lane_default(window, cx)));
            })
        });
        let desk = desk_for_settings;
        let for_updates = desk.clone();
        app.on_action(move |_: &ShowSettings, cx| {
            let desk = desk.clone();
            in_window(handle, cx, move |window, cx| {
                drop(desk.update(cx, |desk, cx| desk.open_settings(window, cx)));
            });
        });
        // The menu's check has nowhere to say what it found: Settings does, on its Updates section. The
        // check starts here too: of the app's handlers for one action only the last registered answers.
        app.on_action(move |_: &CheckForUpdates, cx| {
            crate::updates::check_now(cx);
            let desk = for_updates.clone();
            in_window(handle, cx, move |window, cx| {
                drop(desk.update(cx, |desk, cx| {
                    desk.open_settings_at(Section::Updates, window, cx)
                }));
            });
        });
    }

    pub(super) fn reveal_engine_rules(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.bar.read(cx).tabs().active().clone();
        self.ensure_tab_page(&tab, window, cx);
        if let Some(engine) = self.active_engine(cx) {
            engine.read(cx).scroll_to_rules();
        }
    }

    fn handle_tour_actions(window: &Window, cx: &mut Context<Self>) {
        let (handle, desk) = (window.window_handle(), cx.weak_entity());
        let app: &mut App = cx;
        let show = desk.clone();
        app.on_action(move |_: &ShowTour, cx| {
            let show = show.clone();
            in_window(handle, cx, move |window, cx| {
                drop(show.update(cx, |desk, cx| desk.show_tour(window, cx)));
            });
        });
        let advance = desk.clone();
        app.on_action(move |_: &tour::Advance, cx| {
            let advance = advance.clone();
            in_window(handle, cx, move |window, cx| {
                drop(advance.update(cx, |desk, cx| desk.advance_tour(window, cx)));
            });
        });
        app.on_action(move |_: &tour::Skip, cx| {
            drop(desk.update(cx, |desk, cx| desk.skip_tour(cx)));
        });
    }

    /// The sidebar in the room `share` of its width leaves it, sliding out to the left; nothing when hidden.
    fn sidebar_slot(&self, share: f32, window: &Window, cx: &mut App) -> Option<AnyElement> {
        if share <= 0.0 {
            return None;
        }
        let sidebar = if tour::active_step(cx).is_some() {
            self.sidebar.clone().into_any_element()
        } else {
            self.sidebar
                .clone()
                .cached(
                    StyleRefinement::default()
                        .w(theme::SIDEBAR_WIDTH)
                        .flex_1()
                        .min_h_0(),
                )
                .into_any_element()
        };
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
                    .child(sidebar)
                    .children((share > 0.5).then(|| super::kit::Toast::gutter(window, cx))),
            );
        Some(slot.into_any_element())
    }

    fn tell_retino(&self, share: f32, cx: &mut Context<Self>) {
        let page_left = f32::from(Self::panel_left(share));
        let shift = super::retino::strip_shift(share);
        self.retino.read(cx).set_inputs((page_left, shift));
    }

    /// How far the panel's left edge is from the window's now, the sidebar sliding in or out.
    pub(super) fn panel_left_now(&self) -> gpui::Pixels {
        Self::panel_left(self.sidebar_slide.share(Instant::now()))
    }

    fn panel_left(share: f32) -> gpui::Pixels {
        theme::SIDEBAR_WIDTH * share + theme::PANEL_INSET * (1.0 - share)
    }

    /// The panel under the title bar: its header row, then the page of the tab shown. It has rounded
    /// corners and a hairline ring all around, and ends just above the bottom row. Its left inset grows as the sidebar (`sidebar_share` of it in view) goes.
    fn panel(
        &mut self,
        sidebar_share: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = Theme::palette(cx);
        self.update_inbox_focus(window, cx);
        let viewport = window.viewport_size().width;
        let inner_width = super::split::body_width(viewport, sidebar_share) - theme::PANEL_INSET;
        let body = self.panel_body(inner_width, window, cx);
        let preview = self.bar.read(cx).split_drop_side().map(|side| {
            let layer = div()
                .absolute()
                .top_0()
                .bottom_0()
                .w(gpui::relative(0.5))
                .bg(palette.hover)
                .opacity(theme::page_split::DROP_PREVIEW_OPACITY);
            match side {
                SplitSide::Left => layer.left_0(),
                SplitSide::Right => layer.right_0(),
            }
        });
        let card = div()
            .debug_selector(|| "page-panel".into())
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(palette.panel)
            .rounded(theme::RADIUS_SURFACE)
            .border(theme::PANEL_RING_WIDTH)
            .border_color(palette.panel_ring)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .rounded(theme::RADIUS_SURFACE)
                    .overflow_hidden()
                    .child(body),
            )
            .children(preview);
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .pr(theme::PANEL_INSET)
            .pl(theme::PANEL_INSET * (1.0 - sidebar_share))
            .child(card)
    }
}

/// What a lane's tab says: its project, its name and the mark of its state.
fn master_provider(key: &ConversationKey) -> Option<MasterAgent> {
    MasterAgent::ALL
        .into_iter()
        .find(|agent| agent.code() == key.provider)
}

fn lane_tab(item: &Item) -> (String, LaneTab) {
    // An outside agent's folder is not a project: no project is named in its breadcrumb.
    let project = match item.is_outside() && item.agent() == item.project() {
        true => String::new(),
        false => item.project().to_string(),
    };
    let tab = LaneTab {
        project: SharedString::from(project),
        name: SharedString::from(lanes::lane_words(item, false)),
        identity: SharedString::from(lanes::lane_words(item, true)),
        glyph: lanes::glyph_of(item),
    };
    (item.lane_key(), tab)
}

fn shown_page(bar: &Entity<PageBar>, cx: &App) -> Page {
    bar.read(cx).tabs().active().page().clone()
}

/// Whether an agent is at work: what the breath layer ticks for.
fn is_working(facts: &EngineFacts) -> bool {
    let items = facts.snapshot().map(|snapshot| snapshot.items.as_slice());
    items
        .unwrap_or_default()
        .iter()
        .any(|item| item.agent_state == "working")
}

struct DeskSurface {
    desk: Entity<Desk>,
    retino: Entity<Retino>,
}

impl Render for DeskSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let desk = if super::held_keys::follow(window, cx) || tour::active_step(cx).is_some() {
            self.desk.clone().into_any_element()
        } else {
            self.desk
                .clone()
                .cached(StyleRefinement::default().size_full())
                .into_any_element()
        };
        div()
            .size_full()
            .relative()
            .bg(Theme::palette(cx).canvas)
            .child(self.retino.clone())
            .child(desk)
    }
}

impl Render for Desk {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::snapshot::frame_started();
        crate::snapshot::count_render("desk");
        #[cfg(test)]
        {
            self.renders += 1;
        }
        BreathMarks::clear(Owner::Page, cx);
        let is_identity = super::held_keys::HeldKeys::current(cx).is_identity_shown();
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_identity(is_identity, cx));
        let palette = Theme::palette(cx);
        let now = Instant::now();
        if !self.sidebar_slide.is_at_rest(now) {
            window.request_animation_frame();
        }
        let share = self.sidebar_slide.share(now);
        if share <= 0.0 {
            // A hidden sidebar draws no glyph: what it left for the breath layer is gone with it.
            BreathMarks::clear(Owner::Sidebar, cx);
        }
        let viewport = window.viewport_size();
        let top = theme::page_bar::BAR_HEIGHT;
        let content_height =
            (viewport.height - top - theme::STATUS_BAR_HEIGHT).max(gpui::Pixels::ZERO);
        let drop_bounds = gpui::Bounds::new(
            gpui::point(gpui::Pixels::ZERO, top),
            gpui::size(viewport.width, content_height),
        );
        self.bar.update(cx, |bar, _| {
            bar.set_sidebar_share(share);
            bar.set_split_drop_bounds(drop_bounds);
        });
        self.tell_retino(share, cx);
        let shown = shown_page(&self.bar, cx);
        self.palette
            .update(cx, |palette, cx| palette.follow_page(&shown, window, cx));
        let above = div()
            .flex_1()
            .min_h_0()
            .flex()
            .children(self.sidebar_slot(share, window, cx))
            .child(self.panel(share, window, cx));
        let column = self.column_state(cx);
        let status = engine_status_bar::view(self.sidebar_slide.is_shown(), column, cx);
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .text_color(palette.text)
            .font_family(theme::FONT_UI)
            .text_size(theme::TEXT_BODY)
            .child(self.bar.clone())
            .child(above)
            .children((share <= 0.5).then(|| {
                div()
                    .h(theme::kit::TOAST_DOCK_HEIGHT)
                    .flex_none()
                    .flex()
                    .items_end()
                    .child(
                        div()
                            .w(theme::SIDEBAR_WIDTH)
                            .child(super::kit::Toast::gutter(window, cx)),
                    )
            }))
            .child(status)
            .child(self.tip_layer.clone())
            .child(self.breath.clone())
            .child(self.palette.clone())
            .child(self.quick_open.clone())
            .children(self.tour_layer(&palette, cx))
            .children(crate::snapshot::frame_marker())
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

/// Puts back what the last launch left in the Master's composer: its text, and a notice naming the files that were
/// attached to it and are not kept.
fn restore_master_draft(
    drafts: &Entity<DraftKeeper>,
    owner: &str,
    conversation: &Entity<super::thread::ThreadView>,
    window: &mut Window,
    cx: &mut App,
) {
    let (text, files) = {
        let drafts = drafts.read(cx);
        (
            drafts.restored(&Page::Master, owner),
            drafts.restored_files(&Page::Master, owner),
        )
    };
    conversation.update(cx, |view, cx| {
        if let Some(text) = text {
            view.write(&text, window, cx);
        }
        view.show_files_not_kept(&files, cx);
    });
}
