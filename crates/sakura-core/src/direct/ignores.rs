//! Which files git ignores in a work tree, by its own rules: every `.gitignore` from the root down to
//! the file's folder (a deeper one decides first) and `.git/info/exclude`, a folder that is ignored
//! hiding everything below it. The files are read through the confined root and only matched, as
//! globs. The person's global excludes file is not read.

use super::root::Root;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;
use std::collections::HashMap;
use std::sync::Mutex;

/// The largest `.gitignore` read.
pub const IGNORE_FILE_MAX_BYTES: u64 = 1024 * 1024;
/// The name of a folder's ignore file.
pub const IGNORE_FILE: &str = ".gitignore";

/// The ignore rules of one work tree, read as they are needed.
pub struct Ignores<'a> {
    root: &'a Root,
    exclude: Option<Gitignore>,
    by_folder: Mutex<HashMap<String, Option<Gitignore>>>,
}

impl<'a> Ignores<'a> {
    /// The rules of the work tree at `root`, with the repository's own excludes, `exclude` (the text
    /// of `info/exclude`, when there is one).
    pub fn new(root: &'a Root, exclude: Option<&[u8]>) -> Self {
        Self {
            root,
            exclude: exclude.and_then(matcher),
            by_folder: Mutex::default(),
        }
    }

    /// Takes the rules of `folder` (relative to the root) from a caller that has the folder open:
    /// `text` is its ignore file, none when it has none.
    pub fn learn(&self, folder: &str, text: Option<&[u8]>) {
        let mut known = self
            .by_folder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        known
            .entry(folder.to_string())
            .or_insert_with(|| text.and_then(matcher));
    }

    /// Whether git ignores `path` (relative to the root), a folder when `is_dir`.
    pub fn is_ignored(&self, path: &str, is_dir: bool) -> bool {
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        for depth in 1..parts.len() {
            if self.decides(&parts[..depth], true) == Some(true) {
                return true;
            }
        }
        self.decides(&parts, is_dir) == Some(true)
    }

    /// What the deepest rule that speaks of the path says: ignored, kept, or nothing.
    fn decides(&self, parts: &[&str], is_dir: bool) -> Option<bool> {
        for folders in (0..parts.len()).rev() {
            let folder = parts[..folders].join("/");
            let relative = parts[folders..].join("/");
            if let Some(decided) =
                self.with_folder(&folder, |rules| verdict(rules.matched(&relative, is_dir)))
            {
                return Some(decided);
            }
        }
        let rules = self.exclude.as_ref()?;
        verdict(rules.matched(parts.join("/"), is_dir))
    }

    fn with_folder(
        &self,
        folder: &str,
        decide: impl Fn(&Gitignore) -> Option<bool>,
    ) -> Option<bool> {
        let mut known = self
            .by_folder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let rules = known.entry(folder.to_string()).or_insert_with(|| {
            let path = match folder.is_empty() {
                true => IGNORE_FILE.to_string(),
                false => format!("{folder}/{IGNORE_FILE}"),
            };
            self.root
                .read(&path, IGNORE_FILE_MAX_BYTES)
                .ok()
                .as_deref()
                .and_then(matcher)
        });
        rules.as_ref().and_then(decide)
    }
}

fn verdict<T>(found: Match<T>) -> Option<bool> {
    match found {
        Match::None => None,
        Match::Ignore(_) => Some(true),
        Match::Whitelist(_) => Some(false),
    }
}

/// The rules of one ignore file; a line that is not a valid pattern is skipped, as git skips it.
fn matcher(text: &[u8]) -> Option<Gitignore> {
    let text = String::from_utf8_lossy(text);
    let mut builder = GitignoreBuilder::new("/");
    for line in text.lines() {
        drop(builder.add_line(None, line));
    }
    builder.build().ok().filter(|rules| !rules.is_empty())
}
