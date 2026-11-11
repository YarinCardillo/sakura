//! The root view: the sidebar on the canvas, the tab strip in the window strip and, fused with the tab shown,
//! the inset panel with that lavoro's page. Selection and the view switch apply in the same frame as the
//! click; nothing here waits on `box`.

use super::breath::{BreathLayer, BreathMarks, Owner};
use super::centre::CentreView;
use super::changes::list::ChangesList;
use super::changes::picker::ReviewToolbar;
use super::console::rules::ConsoleKind;
use super::console::{self, Consoles, LoginDone, TerminalEnded};
use super::conversations::{self, Conversation};
use super::desk_play::Plays;
use super::files::{self, QuickOpen, QuickOpenEvent};
use super::files_side::{FilesOpen, GraphOpen, Place};
use super::header_view::HeaderView;
use super::hold::HoldButton;
use super::new_task::NewTaskOpen;
use super::notifying::Notifying;
use super::opening::{opening, Opening};
use super::palette::Palette;
use super::project_setup::{ProjectSetup, SetupEvent};
use super::publishing::Publishing;
use super::right_pane::{self, PaneFacts, PaneView};
use super::sidebar::SidebarView;
use super::slide::Slide;
use super::tabs::haptics::{self, Haptics};
use super::tabs::{self, Strip, StripEvent};
use super::verify::{self, VerifyInputs};
use super::{blocked, page, panes, status_bar};
use crate::data::actions::Actions;
use crate::data::changes::Changes;
use crate::data::facts::{Facts, Phase, StatusRead};
use crate::data::projects::Projects;
use crate::data::retain_tasks;
use crate::data::sessions::Sessions;
use crate::data::source::Source;
use crate::l10n::{tr, Key};
use crate::menus;
use crate::prefs::Preferences;
use crate::theme::{self, Theme};
use gpui::{
    div, AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement, Render,
    ScrollHandle, StyleRefinement, Styled, Subscription, WeakEntity, Window,
};
use gpui_base::ResizableState;
use sakura_core::models::Task;
use sakura_core::projects::NewLavoro;
use sakura_core::tabs::{Placement, Tabs};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

/// What the panel says while the names are listed and the window waits for their state to choose one.
const READING_ALL: Key = Key("desk.reading_all");

/// The window's content.
pub struct Desk {
    pub(super) facts: Entity<Facts>,
    pub(super) actions: Entity<Actions>,
    /// The tabs in the window strip: one per open lavoro. The lavoro on screen is the one of the tab shown.
    pub(super) strip: Entity<Strip>,
    /// Nothing was chosen yet, by the person or by the window: it opens on a task as soon as one can be
    /// chosen rightly (see [`super::opening`]).
    is_opening: bool,
    /// The view of the right pane each lavoro shows; "Changes" until another was chosen.
    pub(super) pane_views: HashMap<String, PaneView>,
    /// The views of the right pane as the preferences hold them.
    pub(super) pane_views_kept: HashMap<String, PaneView>,
    /// Where the right pane's row of views is scrolled to.
    views_scroll: right_pane::ViewsScroll,
    /// The view of the centre each lavoro shows; the agent until its terminal was opened.
    pub(super) centre_views: HashMap<String, CentreView>,
    /// The lavori whose terminal ended by itself, with its exit code, until the line that says so is
    /// put away or the terminal opened again.
    pub(super) ended_terminals: HashMap<String, Option<i32>>,
    /// The Checks row whose log is open, as (task, verb).
    open_log: Option<(String, String)>,
    log_scroll: ScrollHandle,
    /// One hold button per hold id ("Take" and "Archive" of each task), kept so a hold survives re-renders.
    pub(super) holds: HashMap<String, Hold>,
    /// The task whose removal waits for a hold, after "Archive" found work that would be lost.
    pub(super) removing: Option<String>,
    /// "New task" while its sheet is open.
    pub(super) new_task: Option<NewTaskOpen>,
    /// The projects a lavoro can be made in.
    pub(super) projects: Entity<Projects>,
    /// The Play of each open lavoro.
    pub(super) plays: Plays,
    /// The page that adds a project, and whether the person requested it while lavori are listed.
    pub(super) setup: Entity<ProjectSetup>,
    pub(super) is_adding_project: bool,
    /// The lavori requested from the sheet whose agent has not started yet, with what each is made
    /// from and the account its project is told to use: what "Try again" asks for again.
    pub(super) creating: HashMap<String, (NewLavoro, Option<String>)>,
    pub(super) changes: Entity<Changes>,
    /// The diffs of each lavoro looked at and the commit picker over them, kept so open files and scroll
    /// survive a change of tab or of view.
    panes: HashMap<String, Entity<ChangesList>>,
    pickers: HashMap<String, Entity<ReviewToolbar>>,
    pub(super) split: Entity<ResizableState>,
    pub(super) changes_share: f32,
    /// How much of the sidebar and of the changes pane shows: each hides and comes back.
    pub(super) sidebar_slide: Slide,
    pub(super) changes_slide: Slide,
    /// Paints every breathing half on its own tick, so a breath draws neither the sidebar nor the header.
    breath: Entity<BreathLayer>,
    /// How many times the window's frame was built: a test of the app at rest reads it.
    #[cfg(test)]
    draws: usize,
    pub(super) sidebar: Entity<SidebarView>,
    header: Entity<HeaderView>,
    /// The terminal of each task's Agente view, kept while the app runs.
    pub(super) consoles: Entity<Consoles>,
    /// The conversation of each lavoro whose page was drawn, kept while the app runs.
    pub(super) conversations: HashMap<String, Conversation>,
    /// The files of each lavoro whose "Files" view was shown or whose agent was followed, and the
    /// commits of each project whose "Git" view was shown.
    pub(super) files: HashMap<String, FilesOpen>,
    pub(super) graphs: HashMap<String, GraphOpen>,
    /// Where each lavoro's agent is, from its conversation, and the lavori whose pane the person took
    /// before their files were ever read.
    pub(super) agent_places: HashMap<String, Place>,
    pub(super) not_following: HashSet<String>,
    /// Quick open (Cmd+P), one per window.
    quick_open: Entity<QuickOpen>,
    /// The trackpad's feedback when the hold that takes a lavoro's work completes.
    pub(super) haptics: Rc<dyn Haptics>,
    /// The command palette of this window, drawn over everything.
    pub(super) palette: Entity<Palette>,
    /// Where agents are started from, and the sessions kept for the conversations.
    pub(super) source: Source,
    pub(super) sessions: Sessions,
    /// What is known of the publishing of each lavoro: the request sent, the pull request named.
    pub(super) publishing: HashMap<String, Publishing>,
    /// What the system is told of the lavori, and what the Dock's badge counts.
    pub(super) notifying: Notifying,
    /// The desk itself, for what a view asks of it later.
    pub(super) me: WeakEntity<Desk>,
    _observers: Vec<Subscription>,
}

/// The entities a desk shows.
pub struct DeskData {
    pub facts: Entity<Facts>,
    pub actions: Entity<Actions>,
    pub projects: Entity<Projects>,
    pub changes: Entity<Changes>,
    pub consoles: Entity<Consoles>,
    pub source: Source,
    pub sessions: Sessions,
}

/// A hold button of one task, and its confirmation.
pub(super) struct Hold {
    pub(super) task: String,
    pub(super) button: Entity<HoldButton>,
    pub(super) _confirmed: Subscription,
}

impl Desk {
    /// Where the panel starts now, from the window's left edge (the sidebar in view, or the inset).
    pub(super) fn panel_left_now(&self) -> gpui::Pixels {
        let share = self.sidebar_slide.share(Instant::now());
        theme::SIDEBAR_WIDTH * share + theme::PANEL_INSET * (1.0 - share)
    }

    /// A desk over `facts` that opens on the task `preferences` remember, with the changes pane at their
    /// share (they were read before the window was built). An agent that `box` started is attached at once.
    pub fn new(
        data: DeskData,
        preferences: &Preferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut observers = Self::watch_data(&data, cx);
        observers.extend(Self::watch_events(&data, window, cx));
        let strip = Self::strip_from(preferences, cx);
        observers.push(
            cx.subscribe_in(&strip, window, |desk, _, _: &StripEvent, window, cx| {
                // Quick open lists the files of the lavoro it was opened for: it goes with that page.
                desk.quick_open
                    .update(cx, |quick_open, cx| quick_open.close(window, cx));
                desk.tab_changed(cx);
                desk.tell_states(cx);
            }),
        );
        let quick_open = Self::quick_open_in(window, &mut observers, cx);
        let palette = Self::palette_in(window, &mut observers, cx);
        observers.extend(Self::keep_window(window, cx));
        let pane_views = Self::pane_views_from(preferences);
        let shown = strip.read(cx).tabs().active().map(|tab| tab.task.clone());
        let (sidebar, header) = Self::child_views(&data, shown.clone(), cx);
        let (sidebar_slide, changes_slide) = Self::slides_from(preferences);
        let breath = cx.new(|cx| BreathLayer::new(data.facts.clone(), cx));
        let setup = cx.new(|_| ProjectSetup::new(data.source.clone(), data.actions.clone()));
        observers.push(cx.subscribe(&setup, |desk, _, event: &SetupEvent, cx| {
            desk.project_setup_ended(event, cx)
        }));
        let mut desk = Self {
            plays: Plays::over(&data.source),
            facts: data.facts,
            actions: data.actions,
            is_opening: shown.is_none(),
            strip,
            pane_views: pane_views.clone(),
            pane_views_kept: pane_views,
            views_scroll: right_pane::ViewsScroll::default(),
            centre_views: HashMap::new(),
            ended_terminals: HashMap::new(),
            open_log: None,
            log_scroll: ScrollHandle::new(),
            holds: HashMap::new(),
            removing: None,
            new_task: None,
            projects: data.projects,
            setup,
            is_adding_project: false,
            creating: HashMap::new(),
            changes: data.changes,
            panes: HashMap::new(),
            pickers: HashMap::new(),
            split: cx.new(|_| ResizableState::default()),
            changes_share: preferences.changes_share,
            sidebar_slide,
            changes_slide,
            breath,
            #[cfg(test)]
            draws: 0,
            sidebar,
            header,
            consoles: data.consoles,
            conversations: HashMap::new(),
            files: HashMap::new(),
            graphs: HashMap::new(),
            agent_places: HashMap::new(),
            not_following: HashSet::new(),
            quick_open,
            palette,
            haptics: haptics::system(),
            source: data.source,
            sessions: data.sessions,
            publishing: HashMap::new(),
            notifying: Notifying::new(),
            me: cx.weak_entity(),
            _observers: observers,
        };
        desk.start(window, cx);
        desk
    }

    /// Quick open for this window: Cmd+P lists the files of the lavoro on screen, and a file picked opens
    /// in that lavoro's "Files" view.
    fn quick_open_in(
        window: &mut Window,
        observers: &mut Vec<Subscription>,
        cx: &mut Context<Self>,
    ) -> Entity<QuickOpen> {
        let quick_open = cx.new(|cx| QuickOpen::new(window, cx));
        let desk = cx.weak_entity();
        let target = move |cx: &gpui::App| desk.upgrade()?.read(cx).quick_open_target(cx);
        files::quick_open::install(&quick_open, target, window, cx);
        observers.push(cx.subscribe_in(
            &quick_open,
            window,
            |desk, _, event: &QuickOpenEvent, window, cx| desk.file_picked(event, window, cx),
        ));
        quick_open
    }

    /// The strip with the tabs the preferences kept (those of lavori that are gone close after the first
    /// full read), or with the one lavoro a snapshot requested; its keys are bound here.
    fn strip_from(preferences: &Preferences, cx: &mut Context<Self>) -> Entity<Strip> {
        let stored = preferences.tabs.as_ref().and_then(Tabs::from_json);
        let mut tabs = stored.unwrap_or_default();
        if let Some(asked) = crate::snapshot::selection() {
            tabs.show(&asked);
        }
        let strip = cx.new(|cx| Strip::new(tabs, cx));
        tabs::keys::install(&strip, cx);
        strip
    }

    /// What follows the facts, the actions and the consoles: the window is drawn again when one of them
    /// changes, opens on a task once one can be chosen, and forgets tasks that are gone.
    fn watch_data(data: &DeskData, cx: &mut Context<Self>) -> Vec<Subscription> {
        vec![
            cx.observe(&data.facts, |desk, _, cx| {
                desk.open_on_a_task(cx);
                desk.advance_creations(cx);
                desk.files_follow_facts(cx);
                desk.tell_states(cx);
                desk.tell_notifications(cx);
                desk.publishing_follows(cx);
                cx.notify();
            }),
            cx.observe(&data.changes, |desk, _, cx| desk.patches_moved(cx)),
            cx.observe(&data.actions, |desk, _, cx| {
                desk.advance_creations(cx);
                cx.notify();
            }),
            cx.observe(&data.consoles, |_, _, cx| cx.notify()),
            cx.observe(&data.projects, |desk, _, cx| {
                desk.projects_moved(cx);
                cx.notify();
            }),
            cx.subscribe(&data.facts, |desk, facts, _: &StatusRead, cx| {
                // A lavoro requested a moment ago may not be listed yet: it is not one that is gone.
                let asked: Vec<String> = desk.creating.keys().cloned().collect();
                let listed = facts.read(cx);
                desk.forget_missing(|task| {
                    listed.has_task(task) || asked.iter().any(|a| a == task)
                });
                let facts = facts.clone();
                desk.strip.update(cx, |strip, cx| {
                    let listed = facts.read(cx);
                    let gone: Vec<String> = strip
                        .tabs()
                        .tabs()
                        .iter()
                        .map(|tab| tab.task.clone())
                        .filter(|task| !listed.has_task(task) && !asked.contains(task))
                        .collect();
                    strip.retain_tasks(|task| !gone.iter().any(|gone| gone == task), cx);
                });
                desk.reopen_if_gone(cx);
            }),
        ]
    }

    /// What answers an event: a sign-in that ended well starts the conversations that waited for it, a
    /// terminal that ended is put away, and the Mac's languages are read again when the window comes
    /// to the front.
    fn watch_events(
        data: &DeskData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Subscription> {
        vec![
            cx.subscribe(&data.consoles, |desk, _, _: &LoginDone, cx| {
                desk.signed_in(cx)
            }),
            cx.subscribe(&data.consoles, |desk, _, ended: &TerminalEnded, cx| {
                desk.terminal_ended(ended, cx)
            }),
            // The Mac's languages may have changed while the window was not the active one.
            cx.observe_window_activation(window, |desk, window, cx| {
                if window.is_window_active() {
                    crate::language::follow_the_system(cx);
                }
                // In front or behind: what is news is told only while behind.
                desk.tell_notifications(cx);
            }),
        ]
    }

    /// The sidebar and the header, each a view of its own so that it is drawn only when its facts change.
    fn child_views(
        data: &DeskData,
        selected: Option<String>,
        cx: &mut Context<Self>,
    ) -> (Entity<SidebarView>, Entity<HeaderView>) {
        let (desk, facts) = (cx.weak_entity(), data.facts.clone());
        let sidebar = cx.new(|cx| SidebarView::new(desk, facts, selected, cx));
        let desk = cx.weak_entity();
        let watched = (
            data.facts.clone(),
            data.actions.clone(),
            data.consoles.clone(),
        );
        let changes = data.changes.clone();
        let header = cx.new(|cx| HeaderView::new(desk, watched, changes, cx));
        (sidebar, header)
    }

    /// What a new desk does once: names the panes in the menu, answers the window's commands, applies
    /// what a snapshot requested and reads the changes of the task shown.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        menus::show_panes(self.panes_shown(), cx);
        Self::handle_window_actions(window, cx);
        Self::answer_notifications(window, cx);
        Self::handle_play_actions(window, cx);
        self.apply_snapshot_choice(cx);
        self.watch_changes(cx);
    }

    /// Drops the panes, reviews, holds, open log and pending removal of tasks that no longer exist.
    fn forget_missing(&mut self, exists: impl Fn(&str) -> bool) {
        retain_tasks(&mut self.panes, String::as_str, &exists);
        retain_tasks(&mut self.pickers, String::as_str, &exists);
        retain_tasks(&mut self.pane_views, String::as_str, &exists);
        retain_tasks(&mut self.centre_views, String::as_str, &exists);
        retain_tasks(&mut self.ended_terminals, String::as_str, &exists);
        retain_tasks(&mut self.conversations, String::as_str, &exists);
        retain_tasks(&mut self.files, String::as_str, &exists);
        retain_tasks(&mut self.agent_places, String::as_str, &exists);
        retain_tasks(&mut self.publishing, String::as_str, &exists);
        self.plays.retain(&exists);
        self.not_following.retain(|task| exists(task));
        self.sessions.retain(&exists);
        self.holds.retain(|_, hold| exists(&hold.task));
        if self
            .open_log
            .as_ref()
            .is_some_and(|(task, _)| !exists(task))
        {
            self.open_log = None;
        }
        if self.removing.as_deref().is_some_and(|task| !exists(task)) {
            self.removing = None;
        }
    }

    /// The view of the right pane and the open log a snapshot requested (nothing without the snapshot
    /// feature).
    fn apply_snapshot_choice(&mut self, cx: &mut Context<Self>) {
        let (view, log) = crate::snapshot::view_and_log();
        let asked = view.as_deref().and_then(PaneView::named);
        if let (Some(view), Some(task)) = (asked, self.shown(cx)) {
            self.pane_views.insert(task, view);
        }
        if let Some(verb) = log {
            self.toggle_log(verb, cx);
        }
    }

    /// The lavoro on screen: the one of the tab shown.
    pub fn shown(&self, cx: &gpui::App) -> Option<String> {
        let tabs = self.strip.read(cx).tabs();
        tabs.active().map(|tab| tab.task.clone())
    }

    /// The tabs of the window strip.
    #[cfg(test)]
    pub fn strip(&self) -> &Entity<Strip> {
        &self.strip
    }

    /// The view the right pane of `task` shows.
    pub fn pane_view(&self, task: &str) -> PaneView {
        self.pane_views.get(task).copied().unwrap_or_default()
    }

    /// How many rows `task`'s diffs list (files, and the lines of open ones).
    #[cfg(test)]
    pub fn pane_rows(&self, task: &str, cx: &gpui::App) -> usize {
        self.panes
            .get(task)
            .map_or(0, |pane| pane.read(cx).row_count())
    }

    /// The diffs of `task`, if its "Changes" was shown.
    #[cfg(test)]
    pub fn changes_list(&self, task: &str) -> Option<Entity<ChangesList>> {
        self.panes.get(task).cloned()
    }

    /// How many times the sidebar and the header were drawn.
    #[cfg(test)]
    pub fn renders(&self, cx: &gpui::App) -> (usize, usize) {
        (
            self.sidebar.read(cx).renders(),
            self.header.read(cx).renders(),
        )
    }

    /// How many times the window's frame was built.
    #[cfg(test)]
    pub fn draws(&self) -> usize {
        self.draws
    }

    /// Whether the breath layer ticks.
    #[cfg(test)]
    pub fn is_breathing(&self, cx: &gpui::App) -> bool {
        self.breath.read(cx).is_ticking()
    }

    /// The page that adds a project.
    #[cfg(any(test, feature = "snapshot"))]
    pub fn project_setup(&self) -> &Entity<ProjectSetup> {
        &self.setup
    }

    /// The consoles of the Agente views.
    #[cfg(any(test, feature = "snapshot"))]
    pub fn consoles(&self) -> &Entity<Consoles> {
        &self.consoles
    }

    /// Shows the lavoro `name` in the tab on screen, or in the tab that already holds it.
    pub fn select(&mut self, name: String, cx: &mut Context<Self>) {
        self.is_opening = false;
        self.strip.update(cx, |strip, cx| strip.show(&name, cx));
    }

    /// Opens the lavoro `name` in a tab of its own behind the one on screen (Cmd+click on its row).
    pub fn open_behind(&mut self, name: String, cx: &mut Context<Self>) {
        self.is_opening = false;
        self.strip
            .update(cx, |strip, cx| strip.open(&name, Placement::Background, cx));
    }

    /// The tabs changed (one was chosen, opened, closed or moved): the page, the sidebar's highlight and
    /// the changes follow the tab shown, and the tabs are kept for the next launch.
    fn tab_changed(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown(cx);
        if self.removing != shown {
            self.removing = None;
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_selected(shown.clone(), cx));
        let tabs = self.strip.read(cx).tabs().to_json();
        let last = shown.clone();
        Preferences::change(cx, move |preferences| {
            preferences.tabs = Some(tabs);
            preferences.last_task = last.or(preferences.last_task.take());
        });
        if let Some(name) = shown {
            self.is_opening = false;
            self.read_if_pending(name, cx);
        }
        self.watch_changes(cx);
        cx.notify();
    }

    /// While nothing was chosen: shows the task the window opens on, once it can be told.
    fn open_on_a_task(&mut self, cx: &mut Context<Self>) {
        if !self.is_opening || self.facts.read(cx).phase() != &Phase::Ready {
            return;
        }
        let last = Preferences::current(cx).last_task;
        let choice = opening(last.as_deref(), self.facts.read(cx).rows());
        if let Opening::Show(name) = choice {
            self.select(name, cx);
        }
    }

    /// After a full read closed the tabs of lavori that are gone: with no tab left, the window opens on
    /// another lavoro as it does at launch.
    fn reopen_if_gone(&mut self, cx: &mut Context<Self>) {
        if self.shown(cx).is_none() {
            self.is_opening = true;
            self.open_on_a_task(cx);
        }
    }

    /// Asks the facts of `name` now when only its name is known, ahead of the read of every task.
    fn read_if_pending(&mut self, name: String, cx: &mut Context<Self>) {
        self.facts
            .update(cx, |facts, cx| facts.read_if_pending(&name, cx));
    }

    /// The selected task's changes are read now and whenever its facts move on.
    fn watch_changes(&mut self, cx: &mut Context<Self>) {
        let selected = self.shown(cx);
        self.changes
            .update(cx, |changes, cx| changes.watch(selected, cx));
    }

    /// Shows `view` in the right pane of `task`.
    pub fn show_pane_view(&mut self, task: &str, view: PaneView, cx: &mut Context<Self>) {
        self.pane_taken(task, view, cx);
        if self.pane_view(task) == PaneView::Play && view != PaneView::Play {
            self.play_view_left(task, cx);
        }
        self.pane_views.insert(task.to_string(), view);
        cx.notify();
    }

    /// Opens the log of `verb` for the selected task, or closes it when it is the open one.
    pub fn toggle_log(&mut self, verb: String, cx: &mut Context<Self>) {
        let Some(task) = self.shown(cx) else {
            return;
        };
        let target = (task.clone(), verb.clone());
        if self.open_log.as_ref() == Some(&target) {
            self.open_log = None;
            self.actions
                .update(cx, |actions, _| actions.forget_log(&task, &verb));
        } else {
            self.open_log = Some(target);
            self.log_scroll.scroll_to_bottom();
            self.actions
                .update(cx, |actions, cx| actions.load_log(task, verb, cx));
        }
        cx.notify();
    }

    /// Takes the work of `task`.
    pub fn take(&mut self, task: String, cx: &mut Context<Self>) {
        self.actions
            .update(cx, |actions, cx| actions.take(task, cx));
    }

    /// Opens a console of `kind` in its place in `task`'s centre, replacing the one there.
    pub fn open_console(
        &mut self,
        task: &str,
        kind: ConsoleKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.consoles
            .update(cx, |consoles, cx| consoles.open(task, kind, window, cx));
    }

    /// Puts away the agent's console of `task`.
    pub fn close_console(&mut self, task: &str, cx: &mut Context<Self>) {
        self.consoles
            .update(cx, |consoles, cx| consoles.close(task, cx));
    }

    /// Queues `verb` for `task` with the runner.
    pub fn run_verify(&mut self, task: String, verb: String, cx: &mut Context<Self>) {
        self.actions
            .update(cx, |actions, cx| actions.verify(task, verb, cx));
    }

    /// Runs the checks that wait again, after a run that left them waiting.
    pub fn retry_queue(&mut self, cx: &mut Context<Self>) {
        self.actions
            .update(cx, |actions, cx| actions.retry_queue(cx));
    }

    /// Cancels the queued or running check of `verb` for `task`.
    pub fn cancel_verify(&mut self, task: String, verb: String, cx: &mut Context<Self>) {
        self.actions
            .update(cx, |actions, cx| actions.cancel(task, verb, cx));
    }

    /// The panel with the tab strip over it, from the window's top to the status bar and to the window's
    /// right edge; its left inset grows as the sidebar (`sidebar_share` of it in view) goes, and the first
    /// tab then starts after the traffic lights.
    fn panel(
        &mut self,
        sidebar_share: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let content = self.panel_content(window, cx);
        let left =
            theme::SIDEBAR_WIDTH * sidebar_share + theme::PANEL_INSET * (1.0 - sidebar_share);
        let lights = match window.is_fullscreen() {
            true => gpui::Pixels::ZERO,
            false => theme::TRAFFIC_LIGHTS_ROOM,
        };
        self.strip
            .update(cx, |strip, _| strip.set_leading_inset(lights - left));
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .pl(theme::PANEL_INSET * (1.0 - sidebar_share))
            .child(tabs::framed(&self.strip, content, &palette))
    }

    /// The selected task's page, or what the panel says when no task is shown: the page that adds a
    /// project while one is requested, the conversation of a lavoro requested a moment ago, or the
    /// first page.
    fn panel_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.is_adding_project {
            return page::view(None, None, self.setup.clone().into_any_element());
        }
        let shown = self.shown(cx);
        let facts = self.facts.read(cx);
        let row = shown.as_deref().and_then(|name| facts.row(name));
        let asked = shown.filter(|name| row.is_none() && self.creating.contains_key(name));
        let shown = row.map(|row| (row.task.clone(), row.is_pending));
        let is_empty = facts.phase() == &Phase::Ready && facts.rows().is_empty();
        let is_ready = facts.phase() == &Phase::Ready;
        let (shown_task, body) = match (shown, asked) {
            (Some((task, true)), _) => {
                let body = self.reading_body(&task, window, cx);
                (Some(task), body)
            }
            (Some((task, false)), _) => {
                let body = self.page_body(&task, window, cx);
                (Some(task), body)
            }
            // Requested in this frame: `box` lists it in a moment, and its page takes this place.
            (None, Some(name)) => {
                let conversation = self.conversation(&name, window, cx);
                (None, conversation.into_any_element())
            }
            (None, None) if is_empty => (None, self.first_page(window, cx)),
            (None, None) if is_ready => (None, panes::empty(tr(READING_ALL), cx)),
            (None, None) => (None, div().into_any_element()),
        };
        if shown_task.is_none() {
            // No header is drawn: what it left for the breath layer is gone with it.
            BreathMarks::clear(Owner::Header, cx);
        }
        let header = shown_task.as_ref().map(|_| {
            let style = StyleRefinement::default()
                .w_full()
                .h(theme::HEADER_BOX_HEIGHT)
                .flex_none();
            self.header.clone().cached(style).into_any_element()
        });
        let notice = shown_task.and_then(|task| self.page_notice(&task, cx));
        page::view(header, notice, body)
    }
}

impl Desk {
    /// The page of a lavoro under its header: the centre beside the right pane.
    fn page_body(
        &mut self,
        task: &Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let centre = self.centre(task, window, cx);
        self.beside_the_pane(centre, task, false, window, cx)
    }

    /// The page of a lavoro whose facts are being read: its changes are read on their own, so the right
    /// pane works; what needs the facts says they are being read.
    fn reading_body(
        &mut self,
        task: &Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let centre = panes::empty(tr(page::READING_SENTENCE), cx);
        self.beside_the_pane(centre, task, true, window, cx)
    }

    /// The right pane of `task`: its row of views over the view this lavoro shows. This is the one place
    /// where a view of the pane becomes its content: a new view is a variant of [`PaneView`] and an arm
    /// here.
    pub(super) fn right_pane(
        &mut self,
        task: &Task,
        is_pending: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.follow_into_view(&task.name, window, cx);
        let view = self.pane_view(&task.name);
        let body = match view {
            PaneView::Files | PaneView::Checks | PaneView::Git | PaneView::Play if is_pending => {
                panes::empty(tr(page::READING_SENTENCE), cx)
            }
            PaneView::Files => self.files_view(task, window, cx),
            PaneView::Changes => self.changes_view(&task.name, cx),
            PaneView::Checks => self.checks_view(task, cx),
            PaneView::Git => self.git_view(task, cx),
            PaneView::Play => self.play_view(task, window, cx),
        };
        let has_play = self.plays.is_offered(&task.name);
        let facts = PaneFacts {
            task,
            is_pending,
            view,
            listed_files: self.listed_files(&task.name, cx),
            has_play,
            scroll: self.views_scroll.following(&task.name, view, has_play),
        };
        right_pane::view(&facts, body, &Theme::palette(cx), window, cx)
    }

    /// "Changes": the commit picker over the diffs of `task`. With no commit ahead there is nothing to
    /// pick, so its band is not drawn and the files start under the row of views.
    fn changes_view(&mut self, task: &str, cx: &mut Context<Self>) -> AnyElement {
        let picker = self.picker_for(task, cx).cached(
            StyleRefinement::default()
                .w_full()
                .h(theme::TOOLBAR_HEIGHT)
                .flex_none(),
        );
        let read = self.changes.read(cx).task(task);
        let has_commits = read
            .and_then(|read| read.commits())
            .is_some_and(|commits| !commits.is_empty());
        let list = self.pane_for(task, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(has_commits.then_some(picker))
            .child(div().flex_1().min_h_0().child(list))
            .into_any_element()
    }

    /// "Checks": the checks of `task` and their results.
    fn checks_view(&mut self, task: &Task, cx: &mut Context<Self>) -> AnyElement {
        let open_verb = self.open_log.as_ref().filter(|(t, _)| *t == task.name);
        let inputs = VerifyInputs {
            task,
            facts: self.facts.read(cx),
            actions: self.actions.read(cx),
            open_verb: open_verb.map(|(_, verb)| verb.as_str()),
            log_scroll: &self.log_scroll,
        };
        verify::view(inputs, &Theme::palette(cx), cx)
    }

    /// The agent's view of a lavoro: its conversation. An agent that was started in a terminal keeps
    /// that terminal while it runs (one front end per lavoro at a time), and a sign-in asked from the
    /// conversation is shown here until it ends.
    pub(super) fn agent_side(
        &mut self,
        task: &Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_console = self.consoles.read(cx).get(&task.name).is_some();
        if !has_console && console::rules::is_agent_running(task) {
            self.open_console(&task.name, ConsoleKind::Agent, window, cx);
        }
        let row = self.facts.read(cx).row(&task.name);
        if let (Some(console), Some(row)) = (self.consoles.read(cx).get(&task.name), row) {
            let can_open = self.consoles.read(cx).can_open();
            let palette = Theme::palette(cx);
            let terminal = console::view::view(row, console, can_open, &palette, cx);
            return match console.kind() {
                ConsoleKind::Agent => console::view::in_its_terminal(terminal, &palette),
                ConsoleKind::Shell | ConsoleKind::Login(_) => terminal,
            };
        }
        // A lavoro requested with a first message shows that message while it is made.
        let sentence = conversations::unavailable(task);
        if let Some(sentence) = sentence.filter(|_| !self.creating.contains_key(&task.name)) {
            return conversations::unavailable_view(sentence, &Theme::palette(cx));
        }
        // The conversation shown is the one whose kept session is loaded: a lavoro only listed has no
        // conversation made, and starts nothing.
        let conversation = self.conversation(&task.name, window, cx);
        // On the Mac the lavoro's project names its agent's account, and with it the kept session.
        if task.place == sakura_core::models::Place::Mac {
            self.projects
                .update(cx, |projects, cx| projects.load_once(cx));
        }
        conversation.update(cx, |conversation, cx| conversation.load_kept(cx));
        conversation.into_any_element()
    }

    pub(super) fn pane_for(&mut self, task: &str, cx: &mut Context<Self>) -> Entity<ChangesList> {
        if let Some(pane) = self.panes.get(task) {
            return pane.clone();
        }
        let (changes, facts) = (self.changes.clone(), self.facts.clone());
        let pane = cx.new(|cx| ChangesList::new(task.to_string(), changes, facts, cx));
        self.panes.insert(task.to_string(), pane.clone());
        pane
    }

    fn picker_for(&mut self, task: &str, cx: &mut Context<Self>) -> Entity<ReviewToolbar> {
        if let Some(picker) = self.pickers.get(task) {
            return picker.clone();
        }
        let changes = self.changes.clone();
        let picker = cx.new(|cx| ReviewToolbar::new(task.to_string(), changes, cx));
        self.pickers.insert(task.to_string(), picker.clone());
        picker
    }
}

impl Render for Desk {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::snapshot::frame_started();
        crate::snapshot::count_render("desk");
        #[cfg(test)]
        {
            self.draws += 1;
        }
        BreathMarks::clear(Owner::Page, cx);
        let palette = Theme::palette(cx);
        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.canvas)
            .text_color(palette.text)
            .font_family(theme::FONT_UI)
            .text_size(theme::TEXT_BODY);
        let blocked = match self.facts.read(cx).phase() {
            Phase::Blocked(error) => Some(error.clone()),
            _ => None,
        };
        if let Some(error) = blocked {
            return root.child(blocked::view(&error, &palette));
        }
        let now = Instant::now();
        if self.is_sliding(now) {
            window.request_animation_frame();
        }
        let sidebar_share = self.sidebar_slide.share(now);
        if sidebar_share <= 0.0 {
            // A hidden sidebar draws no glyph: what it left for the breath layer is gone with it.
            BreathMarks::clear(Owner::Sidebar, cx);
        }
        let above = div()
            .flex_1()
            .min_h_0()
            .flex()
            .children(self.sidebar_slot(sidebar_share))
            .child(self.panel(sidebar_share, window, cx));
        let facts = self.facts.read(cx);
        let bar = status_bar::Bar {
            panes: self.panes_shown(),
            terminal: self.terminal_item(cx),
            follow: self.follow_item(cx),
            is_playing: self
                .shown(cx)
                .is_some_and(|task| self.plays.is_running(&task)),
        };
        let status = status_bar::view(facts, bar, &palette, cx);
        root.child(above)
            .child(status)
            .child(self.quick_open.clone())
            .child(self.palette.clone())
            .child(self.breath.clone())
            .children(self.grid_overlay(window, cx))
            .children(crate::snapshot::frame_marker())
    }
}
