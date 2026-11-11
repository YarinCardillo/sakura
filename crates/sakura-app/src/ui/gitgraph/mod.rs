//! The "Git" view of the right pane: the commits of a project as a graph, the base branch and every
//! task's branch as lanes. One row a commit: its place in the lanes, its subject, the task whose branch
//! ends there with the task's state, and how long ago it was written. The rows are virtualised and
//! nothing is drawn at rest; the graph is read from `box graph` when the view is made and when
//! [`GitGraph::refresh`] is called, never on a timer.
//!
//! Entry point, one [`GitGraph`] per project shown:
//! - [`GitGraph::new`] takes how the graph is read ([`box_graph`] over the app's `box`) and how a task's
//!   state is drawn on its lane ([`LaneMark`]: the desk hands over its own state mark, so a state looks
//!   here as it does in the sidebar).
//! - [`GitGraph::refresh`] reads the graph again: a task committed, was made or was archived.
//! - It emits [`GitGraphEvent::CommitPicked`] with the commit's sha when a row is pressed; the desk shows
//!   that commit's diff in "Changes".

mod lanes;
mod rows;

#[cfg(test)]
mod tests;

use crate::ui::files::{on_runtime, Reading};
use gpui::{AnyElement, App, Context, EventEmitter, Task, UniformListScrollHandle};
use sakura_core::box_cli::BoxCli;
use sakura_core::error::BridgeResult;
use sakura_core::graph::{self, Age, Graph, Layout};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;

/// Reads the graph of the project shown.
pub type ReadGraph = Rc<dyn Fn() -> Reading<Graph>>;

/// Draws the state of the task named, beside its name on its lane; nothing for a task the desk does
/// not know.
pub type LaneMark = Rc<dyn Fn(&str, &App) -> Option<AnyElement>>;

/// The graph of `project`, read with `box graph` on `runtime`.
pub fn box_graph(cli: BoxCli, project: impl Into<String>, runtime: Arc<Runtime>) -> ReadGraph {
    let project = project.into();
    Rc::new(move || {
        let (cli, project) = (cli.clone(), project.clone());
        on_runtime(&runtime, async move {
            graph::read_graph(&cli, &project, None).await
        })
    })
}

/// What the graph tells the desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitGraphEvent {
    /// The commit with this sha was pressed.
    CommitPicked(String),
}

/// A graph ready to draw: where each commit stands and how old it was when the graph was read.
struct Drawn {
    graph: Graph,
    layout: Layout,
    ages: Vec<Option<Age>>,
}

impl Drawn {
    /// `graph` laid out, or a refusal when it is not one that can be drawn.
    fn of(graph: Graph) -> BridgeResult<Self> {
        let graph = graph.checked()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH);
        let now = now.map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        });
        let age = |commit: &graph::Commit| {
            graph::parse_time(&commit.time).map(|then| Age::between(then, now))
        };
        Ok(Self {
            layout: Layout::of(&graph)?,
            ages: graph.commits.iter().map(age).collect(),
            graph,
        })
    }
}

/// What stands in the view.
enum Shown {
    Reading,
    Failed,
    Graph(Rc<Drawn>),
}

/// The commit graph of one project.
pub struct GitGraph {
    read: ReadGraph,
    mark: LaneMark,
    shown: Shown,
    /// The commit pressed last: its row keeps the selected fill.
    picked: Option<String>,
    scroll: UniformListScrollHandle,
    reading: Option<Task<()>>,
}

impl EventEmitter<GitGraphEvent> for GitGraph {}

impl GitGraph {
    /// The graph `read` gives, with tasks' states drawn by `mark`. It is read at once.
    pub fn new(read: ReadGraph, mark: LaneMark, cx: &mut Context<Self>) -> Self {
        let mut graph = Self {
            read,
            mark,
            shown: Shown::Reading,
            picked: None,
            scroll: UniformListScrollHandle::default(),
            reading: None,
        };
        graph.refresh(cx);
        graph
    }

    /// Reads the graph again. What is shown stays until the answer comes.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let reading = (self.read)();
        self.reading = Some(cx.spawn(async move |graph, cx| {
            let answer = reading.await;
            drop(graph.update(cx, |graph, cx| graph.answered(answer, cx)));
        }));
    }

    fn answered(&mut self, answer: BridgeResult<Graph>, cx: &mut Context<Self>) {
        self.shown = match (answer.and_then(Drawn::of), &self.shown) {
            (Ok(drawn), _) => Shown::Graph(Rc::new(drawn)),
            // A graph already shown stays when reading it again fails.
            (Err(_), Shown::Graph(drawn)) => Shown::Graph(drawn.clone()),
            (Err(_), _) => Shown::Failed,
        };
        cx.notify();
    }

    /// The lavoro the commit `sha` belongs to; none for a commit of the base branch or one not shown.
    pub fn lavoro_of(&self, sha: &str) -> Option<String> {
        let Shown::Graph(drawn) = &self.shown else {
            return None;
        };
        let commit = drawn
            .graph
            .commits
            .iter()
            .find(|commit| commit.sha == sha)?;
        commit.lavoro.clone()
    }

    fn pick(&mut self, sha: String, cx: &mut Context<Self>) {
        self.picked = Some(sha.clone());
        cx.emit(GitGraphEvent::CommitPicked(sha));
        cx.notify();
    }
}
