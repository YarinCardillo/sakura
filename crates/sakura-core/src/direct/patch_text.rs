//! Unified patches written as `git diff` writes them, so the one patch reader of the views reads them:
//! a header per file, the modes, `Binary files ... differ` for binary content, `[patch omitted: N kB]`
//! for a file whose patch is over [`FILE_PATCH_MAX_BYTES`], and the whole cut at
//! [`PATCH_MAX_BYTES`]. The limits are the ones of the service's `diff.py`.

use super::git::Mode;
use similar::{Algorithm, ChangeTag, TextDiff};
use std::fmt::Write;
use std::time::Duration;

/// The longest patch of one file kept; a longer one is said to be omitted.
pub const FILE_PATCH_MAX_BYTES: usize = 400 * 1024;
/// The longest whole patch.
pub const PATCH_MAX_BYTES: usize = 8 * 1024 * 1024;
/// The largest file compared line by line.
pub const BLOB_MAX_BYTES: usize = 2 * 1024 * 1024;
/// How much of a file is looked at for a NUL to tell binary content, as git does.
const SNIFF_BYTES: usize = 8000;
/// How long the comparison of one file may take before a coarser one is used.
const DIFF_TIMEOUT: Duration = Duration::from_millis(200);
const CONTEXT_LINES: usize = 3;
const TRUNCATED: &str = "[output truncated at 8 MB]\n";

/// One side of a file in a patch.
pub enum Content {
    Bytes(std::sync::Arc<Vec<u8>>),
    /// Over [`BLOB_MAX_BYTES`]: not read.
    TooLarge,
}

/// A patch being written.
#[derive(Default)]
pub struct PatchText {
    text: String,
    is_full: bool,
}

/// Lines added and removed in a file; nothing for a binary or unread one.
pub fn line_counts(old: &Content, new: &Content) -> Option<(u32, u32)> {
    let (Content::Bytes(old), Content::Bytes(new)) = (old, new) else {
        return None;
    };
    if is_binary(old) || is_binary(new) {
        return None;
    }
    let (old, new) = (String::from_utf8_lossy(old), String::from_utf8_lossy(new));
    let diff = configured().diff_lines(old.as_ref(), new.as_ref());
    let (mut added, mut removed) = (0u32, 0u32);
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added = added.saturating_add(1),
            ChangeTag::Delete => removed = removed.saturating_add(1),
            ChangeTag::Equal => {}
        }
    }
    Some((added, removed))
}

impl PatchText {
    /// Whether the patch reached its limit and nothing more is written.
    pub fn is_full(&self) -> bool {
        self.is_full
    }

    /// The patch.
    pub fn finish(self) -> String {
        self.text
    }

    /// Adds the patch of `path`: `old` and `new` are the file on each side, absent where there is none.
    pub fn file(&mut self, path: &str, old: Option<(Mode, Content)>, new: Option<(Mode, Content)>) {
        if self.is_full {
            return;
        }
        let quoted = (quote(&format!("a/{path}")), quote(&format!("b/{path}")));
        let mut header = format!("diff --git {} {}\n", quoted.0, quoted.1);
        match (&old, &new) {
            (None, Some((mode, _))) => writeln!(header, "new file mode {}", mode.octal()),
            (Some((mode, _)), None) => writeln!(header, "deleted file mode {}", mode.octal()),
            (Some((a, _)), Some((b, _))) if a != b => {
                writeln!(header, "old mode {}\nnew mode {}", a.octal(), b.octal())
            }
            _ => Ok(()),
        }
        .expect("writing to a string");
        let empty = Content::Bytes(Default::default());
        let old_content = old.as_ref().map_or(&empty, |(_, content)| content);
        let new_content = new.as_ref().map_or(&empty, |(_, content)| content);
        let (a, b) = (
            old.as_ref()
                .map_or("/dev/null".to_string(), |_| quoted.0.clone()),
            new.as_ref()
                .map_or("/dev/null".to_string(), |_| quoted.1.clone()),
        );
        let body = body(old_content, new_content, &a, &b);
        let file = match body {
            Body::Omitted(size) => format!("{header}[patch omitted: {} kB]\n", size / 1024),
            Body::Text(hunks) if header.len() + hunks.len() > FILE_PATCH_MAX_BYTES => {
                format!(
                    "{header}[patch omitted: {} kB]\n",
                    (header.len() + hunks.len()) / 1024
                )
            }
            Body::Text(hunks) => header + &hunks,
        };
        self.push(&file);
    }

    fn push(&mut self, file: &str) {
        if self.text.len() + file.len() > PATCH_MAX_BYTES {
            self.text.push_str(TRUNCATED);
            self.is_full = true;
            return;
        }
        self.text.push_str(file);
    }
}

enum Body {
    Text(String),
    Omitted(usize),
}

fn body(old: &Content, new: &Content, a: &str, b: &str) -> Body {
    let (Content::Bytes(old), Content::Bytes(new)) = (old, new) else {
        return Body::Omitted(BLOB_MAX_BYTES + 1);
    };
    if old == new {
        return Body::Text(String::new());
    }
    if is_binary(old) || is_binary(new) {
        return Body::Text(format!("Binary files {a} and {b} differ\n"));
    }
    let (old, new) = (String::from_utf8_lossy(old), String::from_utf8_lossy(new));
    let diff = configured().diff_lines(old.as_ref(), new.as_ref());
    let mut text = format!("--- {}\n+++ {}\n", side(a), side(b));
    let mut unified = diff.unified_diff();
    unified
        .context_radius(CONTEXT_LINES)
        .missing_newline_hint(true);
    for hunk in unified.iter_hunks() {
        write!(text, "{hunk}").expect("writing to a string");
        if text.len() > FILE_PATCH_MAX_BYTES {
            return Body::Omitted(text.len());
        }
    }
    Body::Text(text)
}

fn configured() -> similar::TextDiffConfig {
    let mut config = TextDiff::configure();
    config.algorithm(Algorithm::Myers).timeout(DIFF_TIMEOUT);
    config
}

/// Whether git would call this content binary: a NUL in its first bytes.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0)
}

/// A `---`/`+++` path: a path with a space is followed by a tab, as git writes it.
fn side(path: &str) -> String {
    match path.contains(' ') && !path.starts_with('"') {
        true => format!("{path}\t"),
        false => path.to_string(),
    }
}

/// A path quoted as git quotes it when it holds a quote, a backslash, a control character or a byte
/// outside ASCII; as it is otherwise.
pub fn quote(path: &str) -> String {
    let needs = path
        .bytes()
        .any(|byte| !(0x20..0x7f).contains(&byte) || byte == b'"' || byte == b'\\');
    if !needs {
        return path.to_string();
    }
    let mut out = String::from("\"");
    for byte in path.bytes() {
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(byte as char),
            _ => write!(out, "\\{byte:03o}").expect("writing to a string"),
        }
    }
    out.push('"');
    out
}
