//! The line reader behind [`Patch::parse`]: a file starts at `diff --git`, its extended headers say what
//! changed, and each `@@` hunk is read by its line counts, so a removed line that looks like a header
//! (`--- x`) stays a removed line.

use super::path::{self, Side};
use super::{Body, Change, FilePatch, Hunk, Line, LineKind, ModeChange, Patch};

const GIT_LINE: &str = "diff --git ";
const OMITTED_START: &str = "[patch omitted: ";
const OMITTED_END: &str = " kB]";
const NO_NEWLINE: char = '\\';
/// Extended headers that say nothing the window shows.
const IGNORED_HEADERS: [&str; 3] = ["similarity index ", "dissimilarity index ", "index "];
/// Extended headers with a value, longest first where one is a prefix of another.
const KEYED_HEADERS: [&str; 8] = [
    "new file mode ",
    "deleted file mode ",
    "old mode ",
    "new mode ",
    "rename from ",
    "rename to ",
    "copy from ",
    "copy to ",
];

/// Reads every line of `text`; see [`Patch::parse`].
pub(super) fn parse(text: &str) -> Patch {
    let mut reader = Reader {
        is_complete: true,
        ..Reader::default()
    };
    // `lines` drops the final empty piece and a carriage return before each line feed.
    for line in text.lines() {
        reader.line(line);
    }
    reader.finish()
}

/// A file being read: the paths seen so far, kept until the file ends.
struct Draft {
    file: FilePatch,
    old: Option<String>,
    new: Option<String>,
    old_mode: Option<String>,
    is_skipping_binary: bool,
}

/// Where the current hunk stands: the next line numbers and how many lines each side still expects.
struct Cursor {
    old_line: u32,
    new_line: u32,
    old_left: u32,
    new_left: u32,
}

#[derive(Default)]
struct Reader {
    files: Vec<FilePatch>,
    draft: Option<Draft>,
    cursor: Option<Cursor>,
    is_complete: bool,
}

impl Reader {
    fn line(&mut self, line: &str) {
        if self.is_in_hunk() {
            if self.hunk_line(line) {
                return;
            }
            self.cut_hunk();
            self.cursor = None;
        } else if self.cursor.take().is_some() && line.starts_with(NO_NEWLINE) {
            return self.mark_no_newline();
        }
        self.header_line(line);
    }

    fn finish(mut self) -> Patch {
        if self.is_in_hunk() {
            self.cut_hunk();
        }
        self.close_file();
        Patch {
            files: self.files,
            is_complete: self.is_complete,
        }
    }

    /// The current hunk ended before its counts said: the patch and its file are both partial.
    fn cut_hunk(&mut self) {
        self.is_complete = false;
        if let Some(draft) = self.draft.as_mut() {
            draft.file.is_complete = false;
        }
    }

    fn is_in_hunk(&self) -> bool {
        self.cursor
            .as_ref()
            .is_some_and(|cursor| cursor.old_left > 0 || cursor.new_left > 0)
    }

    fn header_line(&mut self, line: &str) {
        if let Some(rest) = line.strip_prefix(GIT_LINE) {
            return self.open_file(rest);
        }
        let is_read = match self.draft.as_mut() {
            Some(draft) if draft.is_skipping_binary => true,
            Some(_) if line.starts_with("@@ ") => self.open_hunk(line),
            Some(draft) => draft.header(line),
            None => line.is_empty(),
        };
        if !is_read && !line.is_empty() {
            self.is_complete = false;
        }
    }

    fn open_file(&mut self, rest: &str) {
        self.close_file();
        let paths = path::git_line_paths(rest);
        if paths.is_none() {
            self.is_complete = false;
        }
        let (old, new) = paths.unzip();
        self.draft = Some(Draft {
            file: FilePatch::named(""),
            old,
            new,
            old_mode: None,
            is_skipping_binary: false,
        });
    }

    fn close_file(&mut self) {
        let Some(draft) = self.draft.take() else {
            return;
        };
        match draft.finish() {
            Some(file) => self.files.push(file),
            None => self.is_complete = false,
        }
    }
}

impl Reader {
    fn open_hunk(&mut self, line: &str) -> bool {
        let Some(((old_line, old_left), (new_line, new_left))) = hunk_ranges(line) else {
            return false;
        };
        let Some(draft) = self.draft.as_mut() else {
            return false;
        };
        draft.file.hunks.push(Hunk {
            header: line.to_string(),
            lines: Vec::new(),
        });
        self.cursor = Some(Cursor {
            old_line,
            new_line,
            old_left,
            new_left,
        });
        true
    }

    /// Reads one line inside a hunk; false when it does not belong there.
    fn hunk_line(&mut self, line: &str) -> bool {
        let (kind, text) = match line.chars().next() {
            Some(' ') => (LineKind::Context, &line[1..]),
            Some('+') => (LineKind::Added, &line[1..]),
            Some('-') => (LineKind::Removed, &line[1..]),
            Some(NO_NEWLINE) => {
                self.mark_no_newline();
                return true;
            }
            // Some tools strip the space of an empty context line.
            None => (LineKind::Context, ""),
            Some(_) => return false,
        };
        self.push_line(kind, text)
    }

    fn push_line(&mut self, kind: LineKind, text: &str) -> bool {
        let (Some(cursor), Some(draft)) = (self.cursor.as_mut(), self.draft.as_mut()) else {
            return false;
        };
        let Some((old, new)) = cursor.advance(kind) else {
            return false;
        };
        let file = &mut draft.file;
        match kind {
            LineKind::Added => file.added = file.added.saturating_add(1),
            LineKind::Removed => file.removed = file.removed.saturating_add(1),
            LineKind::Context => {}
        }
        if let Some(hunk) = file.hunks.last_mut() {
            hunk.lines.push(Line {
                kind,
                old,
                new,
                text: text.to_string(),
                has_no_newline: false,
            });
        }
        true
    }

    fn mark_no_newline(&mut self) {
        let last = self
            .draft
            .as_mut()
            .and_then(|draft| draft.file.hunks.last_mut())
            .and_then(|hunk| hunk.lines.last_mut());
        match last {
            Some(line) => line.has_no_newline = true,
            None => self.is_complete = false,
        }
    }
}

impl Cursor {
    /// Numbers a line of `kind` and moves on; `None` when the hunk expects no such line or a number
    /// would overflow.
    fn advance(&mut self, kind: LineKind) -> Option<(Option<u32>, Option<u32>)> {
        let takes_old = kind != LineKind::Added;
        let takes_new = kind != LineKind::Removed;
        if (takes_old && self.old_left == 0) || (takes_new && self.new_left == 0) {
            return None;
        }
        let old = takes_old.then_some(self.old_line);
        let new = takes_new.then_some(self.new_line);
        if takes_old {
            self.old_line = self.old_line.checked_add(1)?;
            self.old_left -= 1;
        }
        if takes_new {
            self.new_line = self.new_line.checked_add(1)?;
            self.new_left -= 1;
        }
        Some((old, new))
    }
}

impl Draft {
    /// Reads one extended header line; false when it is not one.
    fn header(&mut self, line: &str) -> bool {
        if let Some(rest) = line.strip_prefix("--- ") {
            return self.side(rest, false);
        }
        if let Some(rest) = line.strip_prefix("+++ ") {
            return self.side(rest, true);
        }
        if let Some(rest) = line.strip_prefix(OMITTED_START) {
            return self.omitted(rest);
        }
        if line.starts_with("Binary files ") && line.ends_with(" differ") {
            self.file.body = Body::Binary;
            return true;
        }
        if line == "GIT binary patch" {
            self.file.body = Body::Binary;
            self.is_skipping_binary = true;
            return true;
        }
        self.metadata(line)
    }

    /// The `diff --git` extended headers: creation, deletion, modes, renames and the ignored ones.
    fn metadata(&mut self, line: &str) -> bool {
        if IGNORED_HEADERS
            .iter()
            .any(|header| line.starts_with(header))
        {
            return true;
        }
        let Some((key, value)) = KEYED_HEADERS
            .iter()
            .find_map(|key| line.strip_prefix(key).map(|value| (*key, value)))
        else {
            return false;
        };
        match key {
            "new file mode " => self.file.change = Change::New,
            "deleted file mode " => self.file.change = Change::Deleted,
            "old mode " => self.old_mode = Some(value.to_string()),
            "new mode " => {
                let old = self.old_mode.take().unwrap_or_default();
                let new = value.to_string();
                self.file.mode = Some(ModeChange { old, new });
            }
            "rename from " | "copy from " => return self.renamed(value, false),
            _ => return self.renamed(value, true),
        }
        true
    }

    fn renamed(&mut self, value: &str, is_target: bool) -> bool {
        let Some(path) = path::plain_path(value) else {
            return false;
        };
        self.file.change = Change::Renamed;
        if is_target {
            self.new = Some(path);
        } else {
            self.old = Some(path);
        }
        true
    }

    fn side(&mut self, rest: &str, is_new: bool) -> bool {
        match path::side_path(rest) {
            Some(Side::Path(path)) if is_new => self.new = Some(path),
            Some(Side::Path(path)) => self.old = Some(path),
            Some(Side::Absent) => {}
            None => return false,
        }
        true
    }

    fn omitted(&mut self, rest: &str) -> bool {
        let size = rest
            .strip_suffix(OMITTED_END)
            .and_then(|size| size.parse().ok());
        match size {
            Some(kilobytes) => {
                self.file.body = Body::Omitted { kilobytes };
                true
            }
            None => false,
        }
    }

    /// The finished file, or `None` when no path could be read for it.
    fn finish(self) -> Option<FilePatch> {
        let Draft {
            mut file, old, new, ..
        } = self;
        let path = match file.change {
            Change::Deleted => old.clone().or(new),
            _ => new.or(old.clone()),
        }?;
        if file.change == Change::Renamed {
            file.previous_path = old;
        }
        file.path = path;
        Some(file)
    }
}

/// `@@ -a[,b] +c[,d] @@...` as ((a, b), (c, d)); a count without a comma is one line.
fn hunk_ranges(line: &str) -> Option<((u32, u32), (u32, u32))> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, rest) = rest.split_once(' ')?;
    if !rest.starts_with("@@") {
        return None;
    }
    Some((range(old)?, range(new)?))
}

fn range(text: &str) -> Option<(u32, u32)> {
    match text.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((text.parse().ok()?, 1)),
    }
}
