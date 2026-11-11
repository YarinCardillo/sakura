//! What the desk holds of a lavoro's files and of a project's commits: the "Files" and "Git" views of
//! the right pane, quick open, "Follow the agent" and "Ask the agent". Everything is read through `box`
//! (`box tree`, `file`, `search`, `graph`) and only when a view is on screen: a view that is not shown is
//! marked as out of date when the lavoro changes and read again when it is shown, never on a timer.

use super::centre::CentreView;
use super::conversations::Conversation;
use super::desk::Desk;
use super::files::{
    BoxSource, FilesEvent, FilesPane, QuickOpenEvent, Source as FilesSource, Target,
};
use super::gitgraph::{self, GitGraph, GitGraphEvent, LaneMark};
use super::glyph::state_glyph;
use super::panes;
use super::right_pane::PaneView;
use crate::data::changes::Scope;
use crate::data::facts::Facts;
use crate::l10n::{tr, Key};
use crate::theme::Theme;
use crate::ui::breath::Breath;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement as _, Subscription, Window,
};
use sakura_acp::model::ToolKind;
use sakura_acp::thread::{EntryKind, Thread};
use sakura_core::model;
use sakura_core::models::Task;
use sakura_core::patch::Patch;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

/// Where an agent is: the file of its latest tool call, as the call names it, and the line.
pub(super) type Place = (String, Option<u32>);

/// The files of one lavoro, once its "Files" view was shown or its agent was followed.
pub(super) struct FilesOpen {
    pub(super) pane: Entity<FilesPane>,
    /// The patch its gutter was given: the lavoro's whole patch against its base.
    patch: Option<Arc<Patch>>,
    /// When the facts last said the lavoro's workspace may have changed, as of the last read here.
    work_read_at: Option<Instant>,
    /// The lavoro changed while the view was not on screen: it is read again when it is shown.
    is_stale: bool,
    /// The agent's place the pane was last told of.
    told: Option<Place>,
    _watch: Subscription,
}

/// The commits of one project, once a "Git" view of one of its lavori was shown.
pub(super) struct GraphOpen {
    view: Entity<GitGraph>,
    /// The project's lavori and when each one's workspace was last said to change, as of the last read.
    seen: Vec<(String, Instant)>,
    is_stale: bool,
    _watch: Subscription,
}

/// The file and line of the agent's latest tool call that names a place. A search names where it looks
/// (often a folder), not a file the agent is at: it moves nothing. Neither does a call of a conversation
/// shown again from before: it says where the agent had been, not where it goes now.
fn agent_place(thread: &Thread) -> Option<Place> {
    let calls = thread
        .entries()
        .iter()
        .rev()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Tool(call) if call.kind != ToolKind::Search && !entry.is_replayed => {
                call.locations.last()
            }
            _ => None,
        });
    let at = calls.into_iter().next()?;
    Some((at.path.clone(), at.line))
}

/// The lavori of `project` and when each one's workspace was last said to change.
fn project_work(facts: &Facts, project: &str) -> Vec<(String, Instant)> {
    let rows = facts
        .rows()
        .iter()
        .filter(|row| row.task.profile == project);
    rows.map(|row| (row.task.name.clone(), row.work_read_at))
        .collect()
}

impl Desk {
    /// "Files": the tree of `task` and its open file. With the sample data, which has no `box`, one
    /// sentence says so.
    pub(super) fn files_view(
        &mut self,
        task: &Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(pane) = self.files_of(&task.name, window, cx) else {
            return panes::empty(tr(Key("fixture.no_box")), cx);
        };
        if let Some(open) = self.files.get_mut(&task.name) {
            if std::mem::take(&mut open.is_stale) {
                pane.update(cx, |pane, cx| pane.task_changed(window, cx));
            }
        }
        pane.into_any_element()
    }

    /// The files of the lavoro `name`, made now when they are first needed: their root is read then.
    pub(super) fn files_of(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<FilesPane>> {
        if let Some(open) = self.files.get(name) {
            return Some(open.pane.clone());
        }
        let source = self.files_source(name)?;
        let row = self.facts.read(cx).row(name)?;
        let work_read_at = (!row.is_pending).then_some(row.work_read_at);
        let workspace = row.task.place.workspace(name, row.task.folder.as_deref());
        let pane = cx.new(|cx| FilesPane::new(source, workspace, window, cx));
        if self.not_following.remove(name) {
            pane.update(cx, |pane, cx| pane.set_following(false, window, cx));
        }
        let task = name.to_string();
        let watch = cx.subscribe_in(
            &pane,
            window,
            move |desk, _, event: &FilesEvent, window, cx| {
                desk.on_files_event(&task, event, window, cx)
            },
        );
        let open = FilesOpen {
            pane: pane.clone(),
            patch: None,
            work_read_at,
            is_stale: false,
            told: None,
            _watch: watch,
        };
        self.files.insert(name.to_string(), open);
        self.patches_moved(cx);
        Some(pane)
    }

    /// Where the files of the lavoro `name` are read from; nothing on the sample data.
    fn files_source(&self, name: &str) -> Option<Rc<dyn FilesSource>> {
        let cli = self.source.cli()?.clone();
        let runtime = self.source.runtime().clone();
        Some(Rc::new(BoxSource::new(cli, name, runtime)))
    }

    fn on_files_event(
        &mut self,
        task: &str,
        event: &FilesEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // The status bar draws its control again.
            FilesEvent::FollowChanged(_) => cx.notify(),
            FilesEvent::AskAgent(reference) => {
                let conversation = self.conversation(task, window, cx);
                let lines = reference.for_composer();
                conversation.update(cx, |conversation, cx| {
                    conversation.quote(&lines, window, cx)
                });
                self.show_centre_view(task, CentreView::Agent, cx);
            }
        }
    }

    /// The facts changed: the files and the graphs whose lavori changed are out of date. They are read
    /// again when their view is drawn; the window is drawn again by whoever watches the facts.
    pub(super) fn files_follow_facts(&mut self, cx: &mut Context<Self>) {
        let facts = self.facts.read(cx);
        for (name, open) in &mut self.files {
            let now = facts.row(name).filter(|row| !row.is_pending);
            let now = now.map(|row| row.work_read_at);
            if now.is_some() && open.work_read_at.is_some() && now != open.work_read_at {
                open.is_stale = true;
            }
            open.work_read_at = now.or(open.work_read_at);
        }
        for (project, open) in &mut self.graphs {
            let now = project_work(facts, project);
            if now != open.seen {
                open.is_stale = true;
                open.seen = now;
            }
        }
    }

    /// The changes of a lavoro were read: the gutter of its open file takes the lavoro's whole patch.
    pub(super) fn patches_moved(&mut self, cx: &mut Context<Self>) {
        let changes = self.changes.read(cx);
        let mut moved = Vec::new();
        for (name, open) in &mut self.files {
            let read = changes
                .task(name)
                .filter(|read| *read.scope() == Scope::All);
            let Some(patch) = read.and_then(|read| read.patch()) else {
                continue;
            };
            let is_same = open
                .patch
                .as_ref()
                .is_some_and(|held| Arc::ptr_eq(held, patch));
            if !is_same {
                open.patch = Some(patch.clone());
                moved.push((open.pane.clone(), patch.clone()));
            }
        }
        for (pane, patch) in moved {
            pane.update(cx, |pane, cx| pane.set_patch(Some(patch), cx));
        }
    }

    /// The conversation of `task` was drawn again: when its agent is at another place, the page is
    /// drawn again so the right pane can follow.
    pub(super) fn agent_moved(&mut self, task: &str, cx: &mut Context<Self>) {
        let place = self
            .conversations
            .get(task)
            .and_then(|kept: &Conversation| agent_place(kept.view.read(cx).thread()));
        let Some(place) = place else {
            return;
        };
        if self.agent_places.get(task) != Some(&place) {
            self.agent_places.insert(task.to_string(), place);
            cx.notify();
        }
    }

    /// Before the right pane of `task` is drawn: while it follows the agent and the agent is at a place
    /// the pane was not told of, the pane shows "Files" and the file opens at the line. Nothing animates.
    pub(super) fn follow_into_view(
        &mut self,
        task: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(place) = self.agent_places.get(task).cloned() else {
            return;
        };
        let told = self.files.get(task).and_then(|open| open.told.clone());
        if told.as_ref() == Some(&place) || !self.is_following(task, cx) {
            return;
        }
        let Some(pane) = self.files_of(task, window, cx) else {
            return;
        };
        if let Some(open) = self.files.get_mut(task) {
            open.told = Some(place.clone());
        }
        self.pane_views.insert(task.to_string(), PaneView::Files);
        pane.update(cx, |pane, cx| {
            pane.show_location(&place.0, place.1, window, cx)
        });
    }

    /// Whether the right pane of `task` follows its agent: it does until the person takes the pane.
    pub(super) fn is_following(&self, task: &str, cx: &App) -> bool {
        match self.files.get(task) {
            Some(open) => open.pane.read(cx).is_following(),
            None => !self.not_following.contains(task),
        }
    }

    /// Whether the lavoro on screen is followed, for the status bar; none while no lavoro is shown or
    /// its files cannot be read (the sample data).
    pub(super) fn follow_item(&self, cx: &App) -> Option<bool> {
        let task = self.shown(cx)?;
        self.source.cli()?;
        Some(self.is_following(&task, cx))
    }

    /// The status bar's "Follow the agent": stops following, or follows again, which shows "Files" at
    /// the agent's last place.
    pub fn toggle_follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.shown(cx) else {
            return;
        };
        let is_on = !self.is_following(&task, cx);
        let Some(pane) = self.files_of(&task, window, cx) else {
            return;
        };
        pane.update(cx, |pane, cx| pane.set_following(is_on, window, cx));
        if is_on {
            self.pane_views.insert(task, PaneView::Files);
        }
        cx.notify();
    }

    /// The person chose `view` in the right pane of `task`: any view but "Files" takes the pane from
    /// the agent, so following stops.
    pub(super) fn pane_taken(&mut self, task: &str, view: PaneView, cx: &mut Context<Self>) {
        if view == PaneView::Files || !self.is_following(task, cx) {
            return;
        }
        match self.files.get(task) {
            // Stopping needs no window: only following again jumps.
            Some(open) => open.pane.update(cx, |pane, cx| pane.stop_following(cx)),
            None => {
                self.not_following.insert(task.to_string());
            }
        }
    }

    /// The lavoro on screen and its files, for quick open; none with no lavoro or on the sample data.
    pub(super) fn quick_open_target(&self, cx: &App) -> Option<Target> {
        let task = self.shown(cx)?;
        let source = self.files_source(&task)?;
        Some(Target {
            task,
            source,
            panel_left: self.panel_left_now(),
        })
    }

    /// A file was picked in quick open: it opens in the "Files" view of the lavoro quick open was opened
    /// for.
    pub(super) fn file_picked(
        &mut self,
        event: &QuickOpenEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let QuickOpenEvent::Picked { task, path } = event;
        let Some(pane) = self.files_of(task, window, cx) else {
            return;
        };
        self.pane_views.insert(task.clone(), PaneView::Files);
        pane.update(cx, |pane, cx| pane.open(path, window, cx));
        cx.notify();
    }

    /// "Git": the commits of the project of `task`. With the sample data one sentence says so.
    pub(super) fn git_view(&mut self, task: &Task, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.graph_of(&task.profile, cx) else {
            return panes::empty(tr(Key("fixture.no_box")), cx);
        };
        if let Some(open) = self.graphs.get_mut(&task.profile) {
            if std::mem::take(&mut open.is_stale) {
                view.update(cx, |graph, cx| graph.refresh(cx));
            }
        }
        view.into_any_element()
    }

    /// The graph of `project`, read when it is first shown.
    fn graph_of(&mut self, project: &str, cx: &mut Context<Self>) -> Option<Entity<GitGraph>> {
        if let Some(open) = self.graphs.get(project) {
            return Some(open.view.clone());
        }
        let cli = self.source.cli()?.clone();
        let read = gitgraph::box_graph(cli, project, self.source.runtime().clone());
        let mark = self.lane_mark();
        let view = cx.new(|cx| GitGraph::new(read, mark, cx));
        let watch = cx.subscribe(&view, |desk, graph, event: &GitGraphEvent, cx| {
            let GitGraphEvent::CommitPicked(sha) = event;
            let lavoro = graph.read(cx).lavoro_of(sha);
            desk.show_commit(sha, lavoro, cx);
        });
        let open = GraphOpen {
            view: view.clone(),
            seen: project_work(self.facts.read(cx), project),
            is_stale: false,
            _watch: watch,
        };
        self.graphs.insert(project.to_string(), open);
        Some(view)
    }

    /// Draws a lavoro's state on its lane, with the mark the sidebar gives it.
    fn lane_mark(&self) -> LaneMark {
        let facts = self.facts.clone();
        Rc::new(move |task, cx| {
            let facts = facts.read(cx);
            let row = facts.row(task).filter(|row| !row.is_pending)?;
            let (waits, live) = (facts.is_waiting(task), facts.live(task));
            let state = model::shown_state_live(&row.task, waits, live);
            let palette = Theme::palette(cx);
            Some(state_glyph(state.id, state.tone, &palette, Breath::Still))
        })
    }

    /// A commit of the graph was pressed: its diff is shown in "Changes" of the lavoro it belongs to.
    /// A commit of the base branch belongs to none and `box` gives no diff of it: nothing happens.
    fn show_commit(&mut self, sha: &str, lavoro: Option<String>, cx: &mut Context<Self>) {
        let Some(task) = lavoro else {
            return;
        };
        if self.shown(cx).as_deref() != Some(task.as_str()) {
            self.select(task.clone(), cx);
        }
        let scope = Scope::Commit(sha.to_string());
        self.changes
            .update(cx, |changes, cx| changes.set_scope(&task, scope, cx));
        self.pane_taken(&task, PaneView::Modifiche, cx);
        self.pane_views.insert(task, PaneView::Modifiche);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakura_acp::model::{Location, ToolCallFields, Update};
    use sakura_acp::thread::ThreadEvent;

    /// A tool call of the agent at `path`.
    fn call_at(path: &str) -> ThreadEvent {
        let at = Location {
            path: path.to_string(),
            line: Some(3),
        };
        ThreadEvent::Update(Update::ToolCall(ToolCallFields {
            locations: Some(vec![at]),
            ..ToolCallFields::new(path)
        }))
    }

    #[test]
    fn agent_place_is_where_the_agent_went_now_not_where_a_replayed_conversation_had_been() {
        let mut thread = Thread::default();
        thread.apply(ThreadEvent::ReplayStarted);
        thread.apply(call_at("src/prima.rs"));
        thread.apply(ThreadEvent::ReplayFinished);
        assert_eq!(
            agent_place(&thread),
            None,
            "a conversation shown again moves nothing"
        );
        thread.apply(ThreadEvent::Prompted("ancora".into()));
        thread.apply(call_at("src/adesso.rs"));
        assert_eq!(
            agent_place(&thread),
            Some(("src/adesso.rs".to_string(), Some(3)))
        );
    }
}
