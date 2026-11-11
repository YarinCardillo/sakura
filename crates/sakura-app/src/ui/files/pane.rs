//! The files of one task: the tree over the open file, and whether the pane follows the agent. The tree
//! is the kit's (virtualised, with its keys); the levels it shows are read one at a time, as folders are
//! opened, and read again only when the task changes.

mod render;

use super::file_view::{FileView, FileViewEvent};
use super::follow::{Follow, Location};
use super::source::Source;
use super::tree_rows::{self, RowKind, Rows};
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, ScrollStrategy, SharedString, Subscription,
    Window,
};
use gpui_base::{TreeEvent, TreeState};
use sakura_core::error::BridgeResult;
use sakura_core::files::{workspace_relative, Level, Reference, Tree};
use sakura_core::patch::Patch;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// What the pane tells the desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesEvent {
    /// Following the agent turned on or off: the status bar shows it.
    FollowChanged(bool),
    /// "Ask the agent" was pressed with these lines selected: the composer takes them, nothing is sent.
    AskAgent(Reference),
}

/// The "Files" view of one task.
pub struct FilesPane {
    source: Rc<dyn Source>,
    /// The folder the task's agent works in: the places it names are under it.
    workspace: String,
    tree: Tree,
    tree_state: Entity<TreeState>,
    /// What each row of the kit's tree shows, by the row's id.
    rows: Rc<Rows>,
    /// The levels being read, each with the number of its latest read: an answer to an earlier read
    /// of the same folder is old and is dropped.
    reading: HashMap<String, u64>,
    /// How many reads of levels were started.
    reads: u64,
    file: Entity<FileView>,
    follow: Follow,
    /// A file to bring into view in the tree once the folders above it were read.
    to_reveal: Option<SharedString>,
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<FilesEvent> for FilesPane {}

impl FilesPane {
    /// The files of the task `source` reads, whose agent works in `workspace`. Its root is read at once.
    pub fn new(
        source: Rc<dyn Source>,
        workspace: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree_state = cx.new(|cx| TreeState::new(cx));
        let file = cx.new(|cx| FileView::new(source.clone(), window, cx));
        let on_tree = cx.subscribe(&tree_state, Self::on_tree_event);
        let on_file = cx.subscribe(&file, Self::on_file_event);
        let mut pane = Self {
            source,
            workspace: workspace.into(),
            tree: Tree::new(),
            tree_state,
            rows: Rc::default(),
            reading: HashMap::new(),
            reads: 0,
            file,
            follow: Follow::default(),
            to_reveal: None,
            _subscriptions: [on_tree, on_file],
        };
        pane.read_unread(cx);
        pane
    }

    /// Whether the pane follows the agent.
    pub fn is_following(&self) -> bool {
        self.follow.is_on()
    }

    /// The path of the open file.
    pub fn open_path<'a>(&self, cx: &'a gpui::App) -> Option<&'a str> {
        self.file.read(cx).path()
    }

    /// The agent is at `location` (a path as its tool call names it) and `line`. While following is on
    /// the file opens there; a place outside the task is refused.
    pub fn show_location(
        &mut self,
        location: &str,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = workspace_relative(&self.workspace, location) else {
            return;
        };
        let shown = self.follow.agent_at(Location::new(path, line)).cloned();
        if let Some(place) = shown {
            self.show(&place, window, cx);
        }
    }

    /// Follows the agent again and goes to the last place it was at.
    pub fn follow_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (was_off, last) = self.follow.turn_on();
        let last = last.cloned();
        if was_off {
            cx.emit(FilesEvent::FollowChanged(true));
            cx.notify();
        }
        if let Some(place) = last {
            self.show(&place, window, cx);
        }
    }

    /// Opens the file the person picked, in the tree or in quick open: the view is theirs from here.
    pub fn open(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.person_acted(cx);
        self.show(&Location::new(path, None), window, cx);
    }

    /// The task changed: what is on screen is read again, the levels shown and the open file.
    pub fn task_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for dir in self.tree.shown() {
            self.read_level(dir, cx);
        }
        self.file.update(cx, |file, cx| file.reread(window, cx));
    }

    /// Takes the task's patch, for the marks in the open file's gutter.
    pub fn set_patch(&mut self, patch: Option<Arc<Patch>>, cx: &mut Context<Self>) {
        self.file.update(cx, |file, cx| file.set_patch(patch, cx));
    }

    /// Shows everything, or only what differs from the base.
    pub fn set_only_touched(&mut self, only_touched: bool, cx: &mut Context<Self>) {
        self.tree.set_only_touched(only_touched);
        self.sync_tree(cx);
        self.read_unread(cx);
    }

    /// Stops following: the person took the pane for another of its views.
    pub fn stop_following(&mut self, cx: &mut Context<Self>) {
        self.person_acted(cx);
    }

    /// Follows the agent again at its last place, or stops: the status bar's control.
    pub fn set_following(&mut self, is_on: bool, window: &mut Window, cx: &mut Context<Self>) {
        match is_on {
            true => self.follow_agent(window, cx),
            false => self.person_acted(cx),
        }
    }

    /// The person took the view: following stops.
    fn person_acted(&mut self, cx: &mut Context<Self>) {
        if self.follow.person_acted() {
            cx.emit(FilesEvent::FollowChanged(false));
            cx.notify();
        }
    }

    /// Opens the file of `place` at its line and brings its row into view in the tree.
    fn show(&mut self, place: &Location, window: &mut Window, cx: &mut Context<Self>) {
        self.file.update(cx, |file, cx| {
            file.open(&place.path, place.line, window, cx)
        });
        self.tree.reveal(&place.path);
        self.to_reveal = Some(place.path.clone().into());
        self.sync_tree(cx);
        self.read_unread(cx);
    }

    /// Asks for every level that is shown and was never read.
    fn read_unread(&mut self, cx: &mut Context<Self>) {
        for dir in self.tree.unread() {
            if !self.reading.contains_key(&dir) {
                self.read_level(dir, cx);
            }
        }
    }

    fn read_level(&mut self, dir: String, cx: &mut Context<Self>) {
        let reading = self.source.level(&dir);
        self.reads += 1;
        let read = self.reads;
        self.reading.insert(dir.clone(), read);
        cx.spawn(async move |pane, cx| {
            let level = reading.await;
            drop(pane.update(cx, |pane, cx| pane.level_read((&dir, read), level, cx)));
        })
        .detach();
        cx.notify();
    }

    /// The answer to read number `read` of the level of `dir`. When the folder was requested again
    /// since, this answer is older than what is on its way: it is dropped.
    fn level_read(
        &mut self,
        (dir, read): (&str, u64),
        level: BridgeResult<Level>,
        cx: &mut Context<Self>,
    ) {
        if self.reading.get(dir) != Some(&read) {
            return;
        }
        self.reading.remove(dir);
        // A level that could not be read, or that is not one the tree can show, is said so in its
        // place and never asked again by itself; one read before stays as it was.
        if level
            .and_then(|level| self.tree.set_level(dir, level))
            .is_err()
        {
            self.tree.set_failed(dir);
        }
        self.sync_tree(cx);
        self.read_unread(cx);
    }

    /// Asks for the root again: the person pressed "Try again" where it could not be read.
    fn retry(&mut self, cx: &mut Context<Self>) {
        self.tree.retry("");
        self.read_unread(cx);
        cx.notify();
    }

    /// Hands the tree as it is now to the kit's, keeping the row the keyboard was on, and brings the
    /// open file's row into view when it was requested and is there.
    fn sync_tree(&mut self, cx: &mut Context<Self>) {
        let (items, rows) = tree_rows::items(&self.tree.nodes(), self.tree.is_truncated());
        self.rows = Rc::new(rows);
        let to_reveal = self.to_reveal.clone();
        let revealed = self.tree_state.update(cx, |state, cx| {
            let selected = state.selected_item().map(|item| item.id.clone());
            state.set_items(items, cx);
            let wanted = to_reveal.clone().or(selected);
            let index = wanted.and_then(|id| state.index_of(&id));
            state.set_selected_index(index, cx);
            let row = to_reveal.and_then(|id| state.index_of(&id))?;
            state.scroll_to_item(row, ScrollStrategy::Center);
            Some(row)
        });
        if revealed.is_some() {
            self.to_reveal = None;
        }
        cx.notify();
    }

    fn on_tree_event(&mut self, _: Entity<TreeState>, event: &TreeEvent, cx: &mut Context<Self>) {
        let (id, is_open) = match event {
            TreeEvent::Expanded(id) => (id, true),
            TreeEvent::Collapsed(id) => (id, false),
        };
        if matches!(self.rows.get(id), Some(RowKind::Entry(_))) {
            self.tree.set_open(id, is_open);
            self.read_unread(cx);
        }
    }

    fn on_file_event(
        &mut self,
        _: Entity<FileView>,
        event: &FileViewEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            FileViewEvent::PersonActed => self.person_acted(cx),
            FileViewEvent::AskAgent(reference) => {
                cx.emit(FilesEvent::AskAgent(reference.clone()));
            }
        }
    }

    /// The path of the row the keyboard is on, when it is a file that can be opened.
    fn selected_file(&self, cx: &gpui::App) -> Option<SharedString> {
        let id = self.tree_state.read(cx).selected_item()?.id.clone();
        let is_file = self.rows.get(&id).is_some_and(render::is_file);
        is_file.then_some(id)
    }
}

#[cfg(test)]
impl FilesPane {
    /// The open file's view, for the tests.
    pub(in crate::ui) fn file_view(&self) -> Entity<FileView> {
        self.file.clone()
    }
}
