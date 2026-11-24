//! Commits and trees read from their bytes. What they hold was written by an agent for a task in a box:
//! every field is checked and bounded, and a malformed object is an error, never a panic.

use super::objects::corrupt;
use super::oid::Oid;
use crate::error::BridgeResult;

/// The most parents a commit is read with.
const PARENTS_MAX: usize = 64;
/// The longest subject kept, in characters.
const SUBJECT_MAX_CHARS: usize = 200;

/// What the views need of a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub tree: Oid,
    pub parents: Vec<Oid>,
    /// Seconds since the epoch, as the committer wrote them.
    pub time: i64,
    /// The first paragraph of the message on one line, control characters as spaces.
    pub subject: String,
}

/// The mode of a tree entry, as git writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    File,
    Executable,
    Link,
    Tree,
    /// A submodule's commit.
    Gitlink,
}

impl Mode {
    /// The octal form git prints in a patch.
    pub fn octal(self) -> &'static str {
        match self {
            Mode::File => "100644",
            Mode::Executable => "100755",
            Mode::Link => "120000",
            Mode::Tree => "40000",
            Mode::Gitlink => "160000",
        }
    }

    /// The mode of an index entry's raw value.
    pub fn of_index(raw: u32) -> Option<Self> {
        match raw >> 12 {
            0o10 if raw & 0o111 != 0 => Some(Mode::Executable),
            0o10 => Some(Mode::File),
            0o12 => Some(Mode::Link),
            0o16 => Some(Mode::Gitlink),
            _ => None,
        }
    }

    fn of_tree(text: &[u8]) -> Option<Self> {
        match text {
            b"100644" | b"100664" | b"100640" => Some(Mode::File),
            b"100755" => Some(Mode::Executable),
            b"120000" => Some(Mode::Link),
            b"40000" | b"040000" => Some(Mode::Tree),
            b"160000" => Some(Mode::Gitlink),
            _ => None,
        }
    }
}

/// One entry of a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: Mode,
    pub name: String,
    pub oid: Oid,
}

/// The commit in `data`.
pub fn commit(data: &[u8]) -> BridgeResult<Commit> {
    let (headers, message) = match find(data, b"\n\n") {
        Some(at) => (&data[..at], &data[at + 2..]),
        None => (data, &[][..]),
    };
    let mut tree = None;
    let mut parents = Vec::new();
    let mut time = None;
    for line in headers.split(|byte| *byte == b'\n') {
        if let Some(hex) = line.strip_prefix(b"tree ") {
            tree = tree.or(hex_oid(hex));
        } else if let Some(hex) = line.strip_prefix(b"parent ") {
            let parent = hex_oid(hex).ok_or_else(|| corrupt("a commit's parent is not an id"))?;
            if parents.len() == PARENTS_MAX {
                return Err(corrupt("a commit has too many parents"));
            }
            parents.push(parent);
        } else if let Some(person) = line.strip_prefix(b"committer ") {
            time = person_time(person);
        }
    }
    Ok(Commit {
        tree: tree.ok_or_else(|| corrupt("a commit has no tree"))?,
        parents,
        time: time.unwrap_or(0),
        subject: subject(message),
    })
}

/// The entries of the tree in `data`, in its order.
pub fn tree(data: &[u8]) -> BridgeResult<Vec<TreeEntry>> {
    let mut entries = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let space = rest
            .iter()
            .position(|b| *b == b' ')
            .ok_or_else(|| corrupt("a tree entry has no mode"))?;
        let mode = Mode::of_tree(&rest[..space])
            .ok_or_else(|| corrupt("a tree entry has an unknown mode"))?;
        rest = &rest[space + 1..];
        let nul = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| corrupt("a tree entry has no name"))?;
        let name = String::from_utf8_lossy(&rest[..nul]).into_owned();
        let oid = rest
            .get(nul + 1..nul + 21)
            .and_then(Oid::from_bytes)
            .ok_or_else(|| corrupt("a tree entry has no id"))?;
        rest = &rest[nul + 21..];
        if is_plain_name(&name) {
            entries.push(TreeEntry { mode, name, oid });
        }
    }
    Ok(entries)
}

/// Whether a tree entry's name names one entry inside the work tree: not a way out, not git's own.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\0'])
        && !name.eq_ignore_ascii_case(".git")
}

fn hex_oid(hex: &[u8]) -> Option<Oid> {
    Oid::from_hex(std::str::from_utf8(hex).ok()?)
}

/// The seconds of `Name <email> 1700000000 +0100`.
fn person_time(person: &[u8]) -> Option<i64> {
    let close = person.iter().rposition(|b| *b == b'>')?;
    let rest = std::str::from_utf8(&person[close + 1..]).ok()?;
    rest.split_whitespace().next()?.parse().ok()
}

fn subject(message: &[u8]) -> String {
    let text = String::from_utf8_lossy(message);
    let paragraph = text
        .trim_start_matches('\n')
        .split("\n\n")
        .next()
        .unwrap_or_default();
    let joined = paragraph
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");
    joined
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(SUBJECT_MAX_CHARS)
        .collect()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
