//! Naming a place in a lavoro's files: the lines the person selected, as text an agent can read, and
//! the file an agent's tool call points at, as a path inside the lavoro.

/// The most lines and bytes of selected text a reference carries; above either, only the place is named.
const TEXT_MAX_LINES: usize = 20;
const TEXT_MAX_BYTES: usize = 2000;
/// The fence around the selected text.
const FENCE: &str = "```";

/// Lines of a file, picked by the person to ask the agent about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The path inside the lavoro.
    pub path: String,
    /// The first and the last line, numbered from 1.
    pub first_line: u32,
    pub last_line: u32,
    /// The selected text, when it is short.
    pub text: Option<String>,
}

impl Reference {
    /// The reference to lines `first_line..=last_line` of `path`; `selected` is kept only when short.
    pub fn new(path: &str, first_line: u32, last_line: u32, selected: &str) -> Self {
        let is_short = selected.len() <= TEXT_MAX_BYTES
            && selected.lines().count() <= TEXT_MAX_LINES
            && !selected.trim().is_empty();
        Self {
            path: path.to_string(),
            first_line,
            last_line: last_line.max(first_line),
            text: is_short.then(|| selected.to_string()),
        }
    }

    /// The place as an agent reads it: `path:12` or `path:12-18`.
    pub fn place(&self) -> String {
        match self.first_line == self.last_line {
            true => format!("{}:{}", self.path, self.first_line),
            false => format!("{}:{}-{}", self.path, self.first_line, self.last_line),
        }
    }

    /// What goes into the composer: the place, and the text under it in a fence when it is carried.
    pub fn for_composer(&self) -> String {
        match &self.text {
            Some(text) => {
                let body = text.trim_end_matches('\n');
                format!("{}\n{FENCE}\n{body}\n{FENCE}\n", self.place())
            }
            None => format!("{}\n", self.place()),
        }
    }
}

/// The path inside the lavoro of `location`, a path an agent named in a tool call, given the folder the
/// agent works in. `None` for a place outside that folder, which the file view never opens.
pub fn workspace_relative(workspace: &str, location: &str) -> Option<String> {
    let root = workspace.trim_end_matches('/');
    let inside = match location.strip_prefix(root) {
        Some(rest) if rest.starts_with('/') => rest.trim_start_matches('/'),
        Some(_) => return None,
        None if location.starts_with('/') => return None,
        None => location,
    };
    let parts: Vec<&str> = inside
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let stays_inside = !parts.is_empty() && !parts.contains(&"..");
    stays_inside.then(|| parts.join("/"))
}
