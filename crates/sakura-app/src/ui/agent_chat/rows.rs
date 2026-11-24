//! One agent's conversation as rows: what the person wrote to it, what it wrote, the tools it called with
//! the time they took, the engine's lines about this run and a role's result block. Pure: the entries of a
//! page in, the rows out.

use crate::l10n::{tr, Key};
use crate::theme::lane_page as sizes;
use crate::ui::event_words::event_words;
use crate::ui::lane_side::Location;
use sakura_core::engine::feed::FeedEntry;
use std::collections::HashMap;
use std::path::PathBuf;

const KIND_RESULT: &str = "result";

/// One row of the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A line the person sent to this agent.
    You(String),
    /// What the agent wrote; `is_cut` when the engine cut it at its limit.
    Words {
        text: String,
        is_cut: bool,
    },
    Tool(ToolRow),
    /// A line of the engine about this run, as the page says it.
    Engine(String),
    /// A role's result block: its verdict and what it says.
    Result {
        verdict: String,
        summary: String,
    },
    /// The start of a round of the agent after the first, counted from one: a quiet divider.
    Round(usize),
}

/// A tool the agent called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRow {
    pub name: String,
    pub path: String,
    pub line: Option<u64>,
    /// A shell call: the first line of what it ran.
    pub command: String,
    /// A search call: what it searched for.
    pub pattern: String,
    /// What the engine says the call acted on, for a call the fields above say nothing of.
    pub target: String,
    /// The folder the run worked in; the path is relative to it.
    pub workspace: String,
    pub started_at: Option<i64>,
    /// None while the call runs.
    pub ended_at: Option<i64>,
}

impl ToolRow {
    /// Whether the call has not ended.
    pub fn is_running(&self) -> bool {
        self.started_at.is_some() && self.ended_at.is_none()
    }

    /// How long the call has run at `now` (Unix seconds), or took when it ended, in words; none when the
    /// engine gave no times.
    pub fn elapsed_words(&self, now: i64) -> Option<String> {
        let started = self.started_at?;
        let until = self.ended_at.unwrap_or(now);
        let seconds = u64::try_from(until - started).unwrap_or(0);
        Some(crate::ui::thread::elapsed(std::time::Duration::from_secs(
            seconds,
        )))
    }

    /// What the line says beside the tool: the command a shell call ran, else the file and line it named, else
    /// what a search looked for, else what the engine says the call acted on; empty when the call named none.
    pub fn place(&self) -> String {
        match (self.command.is_empty(), self.path.is_empty(), self.line) {
            (false, _, _) => self.command.clone(),
            (true, true, _) if self.pattern.is_empty() => self.target.clone(),
            (true, true, _) => self.pattern.clone(),
            (true, false, Some(line)) => format!("{}:{line}", self.path),
            (true, false, None) => self.path.clone(),
        }
    }

    /// Whether the call named what it worked on, so that its verb has something to be followed by.
    #[cfg(test)]
    pub fn has_target(&self) -> bool {
        !self.place().is_empty()
    }
}

/// The entries of a page as rows, in order.
#[cfg(test)]
pub fn rows_of(entries: &[FeedEntry]) -> Vec<Row> {
    identified_rows_of(entries)
        .into_iter()
        .map(|(_, row)| row)
        .collect()
}

/// The rows of a page, in order, each with the id of the entry it came from. A read of the engine's own role
/// files is its business, not a step the person follows.
#[cfg(test)]
pub fn identified_rows_of(entries: &[FeedEntry]) -> Vec<(String, Row)> {
    parts_of(entries)
        .into_iter()
        .map(|(id, row, _)| (id, row))
        .collect()
}

/// The rows of a page as `identified_rows_of` gives them, each with whether its text continues in the next.
pub fn parts_of(entries: &[FeedEntry]) -> Vec<(String, Row, bool)> {
    let shown = entries
        .iter()
        .filter(|entry| !(entry.is_tool() && sakura_core::files::is_role_file(&entry.path)));
    shown
        .map(|entry| (entry.id.clone(), row_of(entry), entry.continued))
        .collect()
}

/// A page's rows with the parts of long texts joined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    /// Text that continues the last row already shown.
    pub tail: Option<String>,
    pub rows: Vec<(String, Row)>,
    /// Whether the last text still waits for its next part, in a later page.
    pub awaits_part: bool,
}

/// Joins the parts of the texts the engine sent in pieces into one message each. `awaits_part` says the last
/// row shown waits for its next part. A joined text keeps the id of its first part.
pub fn join_parts(awaits_part: bool, parts: Vec<(String, Row, bool)>) -> Joined {
    let mut joined = Joined {
        tail: None,
        rows: Vec::new(),
        awaits_part,
    };
    for (id, row, continued) in parts {
        let Row::Words { text, is_cut } = row else {
            joined.awaits_part = false;
            joined.rows.push((id, row));
            continue;
        };
        if joined.awaits_part {
            match joined.rows.last_mut() {
                Some((_, Row::Words { text: last, .. })) => last.push_str(&text),
                _ => joined.tail.get_or_insert_with(String::new).push_str(&text),
            }
        } else {
            joined.rows.push((id, Row::Words { text, is_cut }));
        }
        joined.awaits_part = continued;
    }
    joined
}

/// A page's rows split by the ids already shown: rows that replace the row at their index, and rows to add
/// in order, each with its id.
pub type ById = (Vec<(usize, Row)>, Vec<(String, Row)>);

/// Splits a page's rows by the ids already `shown`. An entry without an id is always new.
pub fn sort_by_id(shown: &HashMap<String, usize>, rows: Vec<(String, Row)>) -> ById {
    let mut replaced = Vec::new();
    let mut added = Vec::new();
    for (id, row) in rows {
        let at = shown.get(&id).copied();
        match at {
            Some(index) => replaced.push((index, row)),
            None => added.push((id, row)),
        }
    }
    (replaced, added)
}

fn row_of(entry: &FeedEntry) -> Row {
    match (entry.kind.as_str(), &entry.result) {
        (KIND_RESULT, Some(result)) => Row::Result {
            verdict: result.verdict.clone(),
            summary: result.summary.clone(),
        },
        _ if entry.is_you() => Row::You(entry.text.clone()),
        _ if entry.is_engine() => {
            Row::Engine(engine_words(&entry.text, !entry.technical.is_empty()))
        }
        _ if entry.is_tool() => Row::Tool(ToolRow {
            name: entry.tool.clone(),
            path: entry.path.clone(),
            line: entry.line,
            command: entry.command.clone(),
            pattern: entry.pattern.clone(),
            target: entry.target.clone(),
            workspace: entry.workspace.clone(),
            started_at: entry
                .started_at
                .as_deref()
                .and_then(sakura_core::engine::moment::iso_seconds),
            ended_at: entry
                .ended_at
                .as_deref()
                .and_then(sakura_core::engine::moment::iso_seconds),
        }),
        _ => Row::Words {
            text: entry.text.clone(),
            is_cut: entry.truncated,
        },
    }
}

/// An engine line as the person reads it: the words of a ledger line this page knows, any other as the engine wrote it.
///
/// An engine that gives the plain sentence apart from its ledger line (`is_plain`) is believed. A line of an
/// older engine is a ledger line when it carries an actor or a tag; one this page has no words for is then "an
/// engine event", never the line itself. Any other text is the engine's own words and stands as it is.
pub fn engine_words(raw: &str, is_plain: bool) -> String {
    if is_plain {
        return raw.to_string();
    }
    let words = event_words(raw);
    let is_ledger = raw.contains(" actor=") || raw.trim_start().starts_with('[');
    match words == tr(Key("events.unknown")) && !is_ledger {
        true => raw.to_string(),
        false => words.to_string(),
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ToolLocation {
    worktree: Option<String>,
    workspace: String,
    path: String,
    line: Option<u32>,
}

impl ToolLocation {
    pub(super) fn resolve(self) -> Location {
        let path = within_worktree(self.worktree.as_deref(), &self.workspace, &self.path);
        Location::new(path, self.line)
    }
}

pub(super) fn latest_place(rows: &[Row], worktree: Option<&str>) -> Option<ToolLocation> {
    rows.iter().rev().find_map(|row| match row {
        Row::Tool(tool) if !tool.path.is_empty() => Some(ToolLocation {
            worktree: worktree.map(str::to_string),
            workspace: tool.workspace.clone(),
            path: tool.path.clone(),
            line: tool.line.and_then(|line| u32::try_from(line).ok()),
        }),
        _ => None,
    })
}

/// `path`, which a tool call named inside `workspace`, as a path inside the lane's `worktree`, which is where the
/// Files view reads from. A run that worked in a folder of the worktree names its files from that folder, so the
/// path is joined to the folder's place in the worktree; any other path is kept as it came. Links are not
/// resolved here: the reader refuses any that leaves the worktree.
fn within_worktree(worktree: Option<&str>, workspace: &str, path: &str) -> String {
    let inside = worktree
        .filter(|_| !workspace.is_empty())
        .and_then(|worktree| {
            settled(workspace)
                .strip_prefix(settled(worktree))
                .ok()
                .map(PathBuf::from)
        });
    match inside {
        Some(folder) if !folder.as_os_str().is_empty() => {
            folder.join(path).to_string_lossy().into_owned()
        }
        _ => path.to_string(),
    }
}

/// `folder` as the system names it when it can, so that a folder reached through a link is the same folder.
fn settled(folder: &str) -> PathBuf {
    std::fs::canonicalize(folder).unwrap_or_else(|_| PathBuf::from(folder))
}

/// `text` cut to its first [`FOLD_LINES`] lines, a long line counting as the rows it wraps onto, when it runs
/// longer than that; none when it fits. The cut ends where a block of the markdown ends (a blank line outside a
/// code fence), so a paragraph that crosses the limit is left whole to "Show all" and a part never shows a raw
/// `**` or a stray backtick; only a first block longer than the limit is cut inside, at a line or at the limit,
/// and any marker that cut left open is dropped.
pub fn folded(text: &str) -> Option<String> {
    let mut left = sizes::FOLD_LINES;
    let mut kept = String::new();
    let mut boundary: Option<usize> = None;
    let mut is_fenced = false;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let is_blank = line.trim().is_empty() && !is_fenced;
        if is_blank && !kept.trim().is_empty() {
            boundary = Some(kept.trim_end().len());
        }
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            is_fenced = !is_fenced;
        }
        let rows = line.chars().count().div_ceil(sizes::FOLD_COLUMNS).max(1);
        if rows > left {
            let partial: String = line.chars().take(left * sizes::FOLD_COLUMNS).collect();
            return Some(cut_at(&kept, boundary, &partial, is_fenced));
        }
        left -= rows;
        kept.push_str(line);
        kept.push('\n');
        let next_is_blank = lines.peek().is_some_and(|next| next.trim().is_empty());
        if left == 0 && lines.peek().is_some() {
            let ends_a_block = next_is_blank && !is_fenced;
            let at = ends_a_block.then(|| kept.trim_end().len()).or(boundary);
            return Some(cut_at(&kept, at, "", is_fenced));
        }
    }
    None
}

/// The folded part: up to the last block boundary when there is one, else what was kept with `partial` (the
/// start of the line that overflowed), with no marker left open unless it is inside a code fence.
fn cut_at(kept: &str, boundary: Option<usize>, partial: &str, is_fenced: bool) -> String {
    if let Some(at) = boundary.filter(|at| *at > 0) {
        return kept[..at].trim_end().to_string();
    }
    let whole = format!("{kept}{partial}");
    match is_fenced {
        true => whole.trim_end().to_string(),
        false => without_open_markers(whole.trim_end()),
    }
}

/// `text` with the lone backtick or `**` that was never closed taken out, so the markdown reads without it.
fn without_open_markers(text: &str) -> String {
    let mut text = text.to_string();
    for marker in ["`", "**"] {
        if !text.matches(marker).count().is_multiple_of(2) {
            if let Some(at) = text.rfind(marker) {
                text.replace_range(at..at + marker.len(), "");
            }
        }
    }
    text.trim_end().to_string()
}

/// What a role's result block says, read in any case: an approval is not a failure, and a verdict nobody
/// here knows is shown as it came, with no mark of good or bad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Approved,
    Passed,
    Failed,
    Rejected,
    /// The reviewer asks for changes: not an approval and not a failure of the run.
    ChangesRequested,
    /// The builder found nothing to change: a result, with no mark of good or bad.
    NoChange,
    /// The run ended with an error of its own.
    Errored,
    /// The run finished what it was asked.
    Done,
    Other(String),
}

/// The verdict `raw` names.
pub fn verdict_of(raw: &str) -> Verdict {
    match raw.trim().to_lowercase().as_str() {
        "approve" | "approved" | "lgtm" => Verdict::Approved,
        "pass" | "passed" | "ok" => Verdict::Passed,
        "fail" | "failed" | "ko" => Verdict::Failed,
        "reject" | "rejected" => Verdict::Rejected,
        "no_change" => Verdict::NoChange,
        "error" => Verdict::Errored,
        "done" => Verdict::Done,
        "revise" | "revised" | "request_changes" | "changes_requested" => Verdict::ChangesRequested,
        _ => Verdict::Other(raw.trim().to_string()),
    }
}

/// The heading and the body of a result card, in words. The engine summarises a result block that lists
/// things as "<count> <key>" (`1 ruling_requests`): a count of something is said in words and one this build
/// has no words for is left out, never shown with its field name. A builder whose only result is a request for
/// a ruling did not finish: it asked.
pub fn result_words(verdict: &str, summary: &str) -> (String, Option<String>) {
    let counted = counted_list(summary);
    let heading = match (verdict_of(verdict), &counted) {
        (Verdict::Done, Some((_, "ruling_requests"))) => {
            tr(Key("lane_page.verdict.asked_ruling")).to_string()
        }
        (verdict, _) => verdict.words(),
    };
    let body = match counted {
        Some((count, key)) => counted_words(count, key),
        None => Some(summary.trim().to_string()).filter(|summary| !summary.is_empty()),
    };
    (heading, body)
}

/// "3 findings" as the count and the key, when `summary` is exactly that.
fn counted_list(summary: &str) -> Option<(u64, &str)> {
    let (count, key) = summary.trim().split_once(' ')?;
    let is_key = !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    is_key.then_some((count.parse().ok()?, key))
}

/// The words of `count` of the list `key`; none for a list this build has no words for.
fn counted_words(count: u64, key: &str) -> Option<String> {
    let words = match key {
        "findings" => Key("agent_chat.result.findings"),
        "rulings" => Key("agent_chat.result.rulings"),
        "ruling_requests" => Key("agent_chat.result.ruling_requests"),
        "rows" => Key("agent_chat.result.rows"),
        _ => return None,
    };
    Some(crate::l10n::tr_count(words, count, &[]).to_string())
}

impl Verdict {
    /// Whether the verdict is a success: it gets the check.
    pub fn is_good(&self) -> bool {
        matches!(self, Verdict::Approved | Verdict::Passed | Verdict::Done)
    }

    /// Whether the verdict is a failure: it gets the cross, and nothing else does.
    pub fn is_bad(&self) -> bool {
        matches!(self, Verdict::Failed | Verdict::Rejected | Verdict::Errored)
    }

    /// The verdict in words.
    pub fn words(&self) -> String {
        let key = match self {
            Verdict::Approved => Key("lane_page.verdict.approved"),
            Verdict::Passed => Key("lane_page.verdict.pass"),
            Verdict::Failed => Key("lane_page.verdict.fail"),
            Verdict::Rejected => Key("lane_page.verdict.rejected"),
            Verdict::ChangesRequested => Key("lane_page.verdict.revise"),
            Verdict::NoChange => Key("lane_page.verdict.no_change"),
            Verdict::Errored => Key("lane_page.verdict.error"),
            Verdict::Done => Key("lane_page.verdict.done"),
            Verdict::Other(raw) => return raw.clone(),
        };
        tr(key).to_string()
    }
}

#[cfg(test)]
mod location_tests {
    use super::*;

    #[test]
    fn a_workspace_symlink_resolves_relative_to_the_real_worktree() {
        let root = tempfile::tempdir().unwrap();
        let subdir = root.path().join("src");
        let alias = root.path().join("alias");
        std::fs::create_dir(&subdir).unwrap();
        std::os::unix::fs::symlink(&subdir, &alias).unwrap();
        let request = ToolLocation {
            worktree: Some(root.path().to_string_lossy().into_owned()),
            workspace: alias.to_string_lossy().into_owned(),
            path: "main.rs".into(),
            line: Some(8),
        };
        assert_eq!(request.resolve(), Location::new("src/main.rs", Some(8)));
    }
}
