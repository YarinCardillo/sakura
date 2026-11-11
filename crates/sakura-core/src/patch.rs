//! A unified git patch (`box diff`) read into files, hunks and numbered lines for display. The text is
//! untrusted workspace data: it is only ever shown, never run or used to build a command. Parsing never
//! panics; what cannot be read is skipped, and [`Patch::is_complete`] says whether anything was.

mod parse;
mod path;

/// A parsed patch: its files in patch order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Patch {
    pub files: Vec<FilePatch>,
    /// False when some input could not be read and was skipped; the files hold what could be.
    pub is_complete: bool,
}

impl Patch {
    /// Reads `text`; never fails, see [`Patch::is_complete`].
    pub fn parse(text: &str) -> Patch {
        parse::parse(text)
    }

    /// Added lines over every file.
    pub fn added(&self) -> u64 {
        self.files.iter().map(|file| u64::from(file.added)).sum()
    }

    /// Removed lines over every file.
    pub fn removed(&self) -> u64 {
        self.files.iter().map(|file| u64::from(file.removed)).sum()
    }
}

/// What happened to a file as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Modified,
    New,
    Deleted,
    /// Moved from [`FilePatch::previous_path`], with or without changes.
    Renamed,
}

/// What the patch carries for a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// Text hunks (possibly none: a pure rename, a mode change or an empty file).
    Text,
    /// Git said "Binary files differ".
    Binary,
    /// `box` left the patch out because it was too large (`[patch omitted: N kB]`).
    Omitted { kilobytes: u64 },
}

/// A permission change, as git's octal modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeChange {
    pub old: String,
    pub new: String,
}

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePatch {
    /// The path after the change (the old path for a deleted file).
    pub path: String,
    /// The path before a rename.
    pub previous_path: Option<String>,
    pub change: Change,
    pub body: Body,
    pub mode: Option<ModeChange>,
    pub added: u32,
    pub removed: u32,
    pub hunks: Vec<Hunk>,
    /// False when a hunk of this file was cut short: its lines and figures are only the part that was read.
    pub is_complete: bool,
}

impl FilePatch {
    /// The directory part of the path with its trailing slash, empty at the top level.
    pub fn directory(&self) -> &str {
        match self.path.rfind('/') {
            Some(slash) => &self.path[..=slash],
            None => "",
        }
    }

    /// The file name part of the path.
    pub fn file_name(&self) -> &str {
        &self.path[self.directory().len()..]
    }

    /// Lines over every hunk.
    pub fn line_count(&self) -> usize {
        self.hunks.iter().map(|hunk| hunk.lines.len()).sum()
    }

    /// A modified text file at `path` with nothing in it yet.
    pub(crate) fn named(path: &str) -> FilePatch {
        FilePatch {
            path: path.to_string(),
            previous_path: None,
            change: Change::Modified,
            body: Body::Text,
            mode: None,
            added: 0,
            removed: 0,
            hunks: Vec::new(),
            is_complete: true,
        }
    }
}

/// One `@@` hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// The `@@ -a,b +c,d @@` line, with the scope git reports after it.
    pub header: String,
    pub lines: Vec<Line>,
}

/// Whether a line is unchanged, added or removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One line of a hunk, without its prefix character or line ending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    /// Number on the old side; none for an added line.
    pub old: Option<u32>,
    /// Number on the new side; none for a removed line.
    pub new: Option<u32>,
    pub text: String,
    /// Git's "\ No newline at end of file" followed this line.
    pub has_no_newline: bool,
}

#[cfg(test)]
mod tests;
