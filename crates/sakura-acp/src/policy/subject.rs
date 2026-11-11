//! What a permission request is about, as the policy reads it. The policy decides on this and nothing
//! else, and the window shows this and nothing else: there is one reading, so the person is never shown
//! one thing while another is judged. The agent's title for the call is not part of it.

use crate::model::{ToolCallFields, ToolContent, ToolKind};
use serde_json::Value;

/// The most of a raw input shown as JSON; the rest is cut and the subject says so.
pub const MAX_RAW_INPUT_BYTES: usize = 4096;
/// The keys of a mode switch's raw input that name the mode.
const MODE_KEYS: [&str; 3] = ["mode", "modeId", "mode_id"];
/// The only mode a switch to is approved without the person.
const PLAN_MODE: &str = "plan";
/// The raw-input fields that name a file or a folder the call reaches, by the names the known tools use.
/// Their values are judged with the call's locations: a request cannot show one place and reach another.
const TARGET_KEYS: [&str; 8] = [
    "path",
    "file_path",
    "filePath",
    "notebook_path",
    "old_path",
    "new_path",
    "source",
    "destination",
];
/// The raw-input field that holds a command.
const COMMAND_KEY: &str = "command";
/// The most fields listed as unread, and the most of one field's value shown.
const MAX_UNREAD_FIELDS: usize = 32;
const MAX_FIELD_BYTES: usize = 512;

/// The raw-input fields that reach nothing and run nothing, for the kind of call that may carry them.
/// `None`: the kind has no such list, because it reaches nothing itself (a thought) or always goes to the
/// person (every field is then shown).
fn inert_keys(kind: Option<ToolKind>) -> Option<&'static [&'static str]> {
    Some(match kind? {
        ToolKind::Read => &["offset", "limit", "line", "pages"],
        ToolKind::Edit => &[
            "content",
            "old_string",
            "new_string",
            "replace_all",
            "edits",
            "oldText",
            "newText",
            "new_source",
            "cell_id",
            "cell_type",
            "edit_mode",
        ],
        ToolKind::Delete | ToolKind::Move => &[],
        ToolKind::Search => &[
            "pattern",
            "query",
            "glob",
            "type",
            "output_mode",
            "head_limit",
            "offset",
            "multiline",
            "context",
            "-i",
            "-n",
            "-A",
            "-B",
            "-C",
        ],
        ToolKind::Execute => &[COMMAND_KEY, "description", "timeout"],
        ToolKind::SwitchMode => &MODE_KEYS,
        ToolKind::Think => return None,
        ToolKind::Fetch | ToolKind::Other => &[],
    })
}

/// What a permission request is about.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Subject {
    /// The call's kind; none when no message stated it.
    pub kind: Option<ToolKind>,
    /// The command: `rawInput.command` as it is when it is a string, its words joined by one space when it
    /// is a list of strings. This exact text is what the policy lexes.
    pub command: Option<String>,
    /// Every path the call names, once each: its locations, the files of its diffs, then the targets its
    /// raw input names.
    pub paths: Vec<String>,
    /// The modes a mode switch names, by every key that can name one.
    pub modes: Vec<String>,
    /// The raw input's fields the policy does not read for a call of this kind, as `name: value` with the
    /// value as JSON: a working directory, an environment, a second target in a shape it cannot check,
    /// anything it has no rule for. A request that has one goes to the person, who is shown it.
    pub unread: Vec<String>,
    /// Whether `unread` leaves fields out.
    pub is_unread_cut: bool,
    /// The call's raw input as JSON, for a request whose command, paths and modes say nothing.
    pub raw_input: Option<String>,
    /// Whether `raw_input` is only the start of the JSON.
    pub is_raw_input_cut: bool,
}

impl Subject {
    /// Reads the subject of the request for `tool`.
    pub fn of(tool: &ToolCallFields) -> Self {
        let input = tool.raw_input.as_ref();
        let (raw_input, is_raw_input_cut) = match input.map(json) {
            Some((text, is_cut)) => (Some(text), is_cut),
            None => (None, false),
        };
        let (unread, is_unread_cut) = input
            .map(|input| unread(tool.kind, input))
            .unwrap_or_default();
        Self {
            kind: tool.kind,
            command: input.and_then(command_text),
            paths: paths(tool),
            modes: match tool.kind {
                Some(ToolKind::SwitchMode) => input.map(modes).unwrap_or_default(),
                _ => Vec::new(),
            },
            unread,
            is_unread_cut,
            raw_input,
            is_raw_input_cut,
        }
    }

    /// Whether the request names no command, no path and no mode: only its raw input says what it is.
    pub fn is_unread(&self) -> bool {
        self.command.is_none() && self.paths.is_empty() && self.modes.is_empty()
    }

    /// Whether a mode switch goes to plan mode and nowhere else: every key that names a mode says so.
    pub(super) fn is_plan_mode(&self) -> bool {
        !self.modes.is_empty() && self.modes.iter().all(|mode| mode == PLAN_MODE)
    }
}

/// `rawInput.command` as a string, or as a list of strings joined by one space.
fn command_text(input: &Value) -> Option<String> {
    match input.get("command")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(words) => {
            let words: Option<Vec<&str>> = words.iter().map(Value::as_str).collect();
            Some(words?.join(" "))
        }
        _ => None,
    }
}

/// The fields of `input` the policy has no rule for in a call of `kind`, and whether the list was cut. A
/// field is read when it is a target given as a string (judged as a path), the command of a command, a
/// mode of a mode switch, or on the kind's list of fields that reach nothing.
fn unread(kind: Option<ToolKind>, input: &Value) -> (Vec<String>, bool) {
    let (Some(fields), Some(inert)) = (input.as_object(), inert_keys(kind)) else {
        return (Vec::new(), false);
    };
    let is_read = |key: &str, value: &Value| {
        let is_target = TARGET_KEYS.contains(&key) && value.is_string();
        is_target || inert.contains(&key)
    };
    let unread = fields.iter().filter(|(key, value)| !is_read(key, value));
    let mut listed: Vec<String> = unread.map(|(key, value)| field_text(key, value)).collect();
    let is_cut = listed.len() > MAX_UNREAD_FIELDS;
    listed.truncate(MAX_UNREAD_FIELDS);
    (listed, is_cut)
}

/// `name: value`, the value as compact JSON cut at [`MAX_FIELD_BYTES`] on a character boundary.
fn field_text(key: &str, value: &Value) -> String {
    let mut text = format!("{key}: {value}");
    let mut end = text.len().min(MAX_FIELD_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

/// The targets the raw input names as strings, by every key that can name one.
fn raw_targets(input: &Value) -> impl Iterator<Item = &str> {
    TARGET_KEYS
        .iter()
        .filter_map(|key| input.get(key)?.as_str())
}

/// Every path the call names: its locations, the files of its diffs and the targets of its raw input, in
/// that order, once each.
fn paths(tool: &ToolCallFields) -> Vec<String> {
    let locations = tool
        .locations
        .iter()
        .flatten()
        .map(|location| &location.path);
    let diffs = tool
        .content
        .iter()
        .flatten()
        .filter_map(|content| match content {
            ToolContent::Diff(diff) => Some(&diff.path),
            _ => None,
        });
    let raw = tool.raw_input.iter().flat_map(raw_targets);
    let named = locations.chain(diffs).map(String::as_str).chain(raw);
    let mut paths: Vec<String> = Vec::new();
    for path in named {
        if !paths.iter().any(|known| known == path) {
            paths.push(path.to_string());
        }
    }
    paths
}

fn modes(input: &Value) -> Vec<String> {
    let named = MODE_KEYS.iter().filter_map(|key| input.get(key));
    named
        .map(|mode| match mode.as_str() {
            Some(mode) => mode.to_string(),
            None => mode.to_string(),
        })
        .collect()
}

/// The raw input as compact JSON, cut at [`MAX_RAW_INPUT_BYTES`] on a character boundary.
fn json(input: &Value) -> (String, bool) {
    let mut text = input.to_string();
    let mut end = text.len().min(MAX_RAW_INPUT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let is_cut = end < text.len();
    text.truncate(end);
    (text, is_cut)
}
