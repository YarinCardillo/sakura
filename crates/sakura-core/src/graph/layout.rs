//! Where each commit of a graph is drawn: one row a commit, children above their parents. Columns come
//! from ancestry alone, as `git log --graph` and GitKraken place them: a commit stands in the column
//! that was coming down to it from a child, its first parent goes on in that column, every other parent
//! gets a column of its own, and a column is free again once the commit it led to is drawn. So a branch
//! leaves and a merge comes back as two lines, whoever the commits are said to belong to: the task a
//! commit belongs to only names the lane. The first column is held for the base branch's last commit.

use super::{not_a_graph, Graph};
use crate::error::BridgeResult;
use std::collections::{BTreeSet, HashMap};

/// The most pieces of line a layout holds over all its rows, and the most columns it is wide. A graph
/// that would need more is refused whole.
const STROKES_MAX: usize = 1_000_000;
const COLUMNS_MAX: usize = 128;

/// One piece of line in a row. The commit's dot is in the row's own column, at mid height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stroke {
    /// The whole height of the row, in a column: an edge passing by.
    Through(usize),
    /// From the row's top edge to the dot: a child above, in the same column.
    FromAbove,
    /// From the dot to the row's bottom edge: a parent below, in the same column.
    ToBelow,
    /// From the dot to the top edge of another column: a child there, above (a branch leaving).
    BendUp(usize),
    /// From the dot to the bottom edge of another column: a parent there, below (a merge).
    BendDown(usize),
}

/// One row of the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The commit's index in [`Graph::commits`].
    pub commit: usize,
    /// The column of its dot.
    pub column: usize,
    pub strokes: Vec<Stroke>,
    /// The lanes ([`Graph::lanes`]) whose last commit this is.
    pub heads: Vec<usize>,
}

/// The rows of a graph and how many columns they use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    pub rows: Vec<Row>,
    pub columns: usize,
}

/// A column on its way down to a commit.
#[derive(Clone, Copy)]
struct Waiting {
    /// The commit the column leads to, by its index in the graph.
    commit: usize,
    /// Whether a line comes down the column: a column only held for a commit has none yet.
    has_line: bool,
}

/// The columns of the graph at the row being placed.
#[derive(Default)]
struct Columns {
    waiting: Vec<Option<Waiting>>,
}

impl Columns {
    /// Holds a column for `commit` before any row is placed: no line comes down it yet.
    fn hold(&mut self, commit: usize) {
        self.waiting.push(Some(Waiting {
            commit,
            has_line: false,
        }));
    }

    /// The first free column, taken for `waiting`.
    fn take(&mut self, waiting: Waiting) -> usize {
        let free = self.waiting.iter().position(Option::is_none);
        let column = free.unwrap_or(self.waiting.len());
        match free {
            Some(column) => self.waiting[column] = Some(waiting),
            None => self.waiting.push(Some(waiting)),
        }
        column
    }

    /// The columns that lead to `commit`, leftmost first.
    fn leading_to(&self, commit: usize) -> Vec<usize> {
        let leads = |waiting: &Option<Waiting>| waiting.is_some_and(|w| w.commit == commit);
        let columns = self.waiting.iter().enumerate();
        columns
            .filter(|(_, w)| leads(w))
            .map(|(column, _)| column)
            .collect()
    }

    /// Places `commit`, whose listed parents are `parents` (the first one first): its column and the
    /// pieces of line of its row. The columns are left as the next row finds them.
    fn place(&mut self, commit: usize, parents: &[usize]) -> (usize, Vec<Stroke>) {
        let arriving = self.leading_to(commit);
        let held = Waiting {
            commit,
            has_line: false,
        };
        let column = arriving.first().copied().unwrap_or_else(|| self.take(held));
        let mut strokes = self.passing(commit);
        for &other in &arriving {
            let has_line = self.waiting[other].is_some_and(|waiting| waiting.has_line);
            match (other == column, has_line) {
                (true, true) => strokes.push(Stroke::FromAbove),
                (false, true) => strokes.push(Stroke::BendUp(other)),
                (_, false) => {}
            }
        }
        // The columns that arrived stay taken while the parents are placed: a line that leaves this
        // row in a column where another just arrived would read as that one going on.
        for (nth, &parent) in parents.iter().enumerate() {
            strokes.push(self.lead_to(parent, (nth == 0).then_some(column)));
        }
        let is_led_on = self.waiting[column].is_some_and(|waiting| waiting.commit != commit);
        for &other in arriving
            .iter()
            .filter(|&&other| other != column || !is_led_on)
        {
            self.waiting[other] = None;
        }
        if !is_led_on {
            self.waiting[column] = None;
        }
        strokes.sort();
        strokes.dedup();
        (column, strokes)
    }

    /// The lines that pass the row of `commit` by, on their way to other commits.
    fn passing(&self, commit: usize) -> Vec<Stroke> {
        let passes = |waiting: &Option<Waiting>| {
            waiting.is_some_and(|waiting| waiting.commit != commit && waiting.has_line)
        };
        let columns = self.waiting.iter().enumerate();
        let passing = columns.filter(|(_, waiting)| passes(waiting));
        passing.map(|(column, _)| Stroke::Through(column)).collect()
    }

    /// Starts the line from the row's commit to `parent`. The first parent goes on in the commit's
    /// own column (`own`); another parent joins a column that already leads to it, or takes a free one.
    fn lead_to(&mut self, parent: usize, own: Option<usize>) -> Stroke {
        let line = Waiting {
            commit: parent,
            has_line: true,
        };
        if let Some(column) = own {
            self.waiting[column] = Some(line);
            return Stroke::ToBelow;
        }
        let joined = self.leading_to(parent).first().copied();
        let column = joined.unwrap_or_else(|| self.take(line));
        self.waiting[column] = Some(line);
        Stroke::BendDown(column)
    }
}

impl Layout {
    /// The layout of `graph`, or a refusal when drawing it would take more than a layout holds.
    pub fn of(graph: &Graph) -> BridgeResult<Self> {
        let index: HashMap<&str, usize> = shas(graph).collect();
        let order = children_first(graph, &index);
        // A commit listed twice, or one that is among its own ancestors, is no history: the order
        // would leave commits out, and a graph is never drawn in part.
        if index.len() != graph.commits.len() || order.len() != graph.commits.len() {
            return Err(not_a_graph());
        }
        let heads = heads(graph, &index);
        let mut columns = Columns::default();
        let base_head = graph.lanes.first().and_then(|lane| lane.head.as_deref());
        if let Some(&commit) = base_head.and_then(|sha| index.get(sha)) {
            columns.hold(commit);
        }
        let mut rows = Vec::with_capacity(graph.commits.len());
        let mut strokes_made = 0;
        for commit in order {
            let (column, strokes) = columns.place(commit, &parents_of(graph, commit, &index));
            strokes_made += strokes.len();
            if strokes_made > STROKES_MAX || columns.waiting.len() > COLUMNS_MAX {
                return Err(not_a_graph());
            }
            rows.push(Row {
                commit,
                column,
                strokes,
                heads: heads[commit].clone(),
            });
        }
        let used = match rows.is_empty() {
            true => 0,
            false => columns.waiting.len(),
        };
        Ok(Self {
            rows,
            columns: used,
        })
    }
}

/// The listed parents of `commit`, each once, the first one first; a parent older than the list is
/// not among them.
fn parents_of(graph: &Graph, commit: usize, index: &HashMap<&str, usize>) -> Vec<usize> {
    let mut parents: Vec<usize> = Vec::new();
    for sha in &graph.commits[commit].parents {
        match index.get(sha.as_str()) {
            Some(&parent) if !parents.contains(&parent) => parents.push(parent),
            _ => {}
        }
    }
    parents
}

/// Whether the commits of `graph` are a history: each listed once, none among its own ancestors.
pub(super) fn is_history(graph: &Graph) -> bool {
    let index: HashMap<&str, usize> = shas(graph).collect();
    let count = graph.commits.len();
    index.len() == count && children_first(graph, &index).len() == count
}

/// The commits in the order `box` gave them, moved only as far as needed for every commit to stand
/// above its parents (two commits of the same second can come either way round). A commit that is
/// among its own ancestors, and every commit above it, is left out: the order is then shorter than
/// the list.
fn children_first(graph: &Graph, index: &HashMap<&str, usize>) -> Vec<usize> {
    let count = graph.commits.len();
    let parents: Vec<Vec<usize>> = (0..count)
        .map(|commit| parents_of(graph, commit, index))
        .collect();
    let mut waiting = vec![0usize; count];
    for parent in parents.iter().flatten() {
        waiting[*parent] += 1;
    }
    let mut ready: BTreeSet<usize> = (0..count).filter(|&commit| waiting[commit] == 0).collect();
    let mut order = Vec::with_capacity(count);
    while let Some(commit) = ready.pop_first() {
        order.push(commit);
        for &parent in &parents[commit] {
            waiting[parent] -= 1;
            if waiting[parent] == 0 {
                ready.insert(parent);
            }
        }
    }
    order
}

fn shas(graph: &Graph) -> impl Iterator<Item = (&str, usize)> {
    let commits = graph.commits.iter().enumerate();
    commits.map(|(index, commit)| (commit.sha.as_str(), index))
}

/// For every commit, the lanes whose head it is.
fn heads(graph: &Graph, index: &HashMap<&str, usize>) -> Vec<Vec<usize>> {
    let mut heads = vec![Vec::new(); graph.commits.len()];
    for (lane, head) in graph.lanes.iter().enumerate() {
        let commit = head.head.as_deref().and_then(|sha| index.get(sha));
        if let Some(&commit) = commit {
            heads[commit].push(lane);
        }
    }
    heads
}
