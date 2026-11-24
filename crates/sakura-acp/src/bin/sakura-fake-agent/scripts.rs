//! The scripted sessions. The first word of the prompt picks one: `plain`, `think`, `tools`, `diff`,
//! `shell`, `terminal`, `plan`, `permission`, `permission-safe`, `permission-read`, `permission-read-outside`,
//! `permission-mcp`, `permission-grep`, `permission-command`, `permission-twins`, `permission-late`, `slow`,
//! `master` (what the engine's master does: starts a lane, reads files, proposes a note), `stream5000`, `crash`, `stall`, `stall-late`, `deaf`, `lines`, `fenced`, `refusal`; anything else plays the showcase, which strings
//! most of them together. The scripted replies use neutral English examples.

use serde_json::{json, Value};

/// The workspace the fake agent pretends to work in, unless `WORKSPACE_VARIABLE` names one (a folder on
/// disk, for a task on the Mac).
const WORKSPACE: &str = "/work/demo";
const WORKSPACE_VARIABLE: &str = "SAKURA_FAKE_WORKSPACE";
/// A file of the person's outside the workspace, which a read must not reach unasked.
const OUTSIDE_FILE: &str = "/home/person/.ssh/id_ed25519";
/// Chunks of the long stream fixture.
const STREAM_CHUNKS: usize = 5000;
/// Chunks of the slow turn a cancel is meant to interrupt.
const SLOW_CHUNKS: usize = 400;

/// One step of a scripted turn.
pub enum Step {
    /// A `session/update` payload.
    Update(Value),
    /// A pause of this many delays.
    Pause(u32),
    /// A permission request, and the update that follows when it is allowed or refused.
    Permission {
        request: Value,
        allowed: Value,
        refused: Value,
    },
    /// A permission request sent only once the turn was cancelled, as an agent that had it on its way
    /// would; then the update that says what it was answered, and the turn ends as cancelled.
    LatePermission {
        request: Value,
        allowed: Value,
        refused: Value,
    },
    /// The turn goes on until it is cancelled, then still sends these updates before it ends.
    Linger(Vec<Value>),
    /// The process exits in the middle of the turn.
    Crash,
    /// The turn ends here because the model refused the prompt.
    Refuse,
    /// The turn never ends by itself.
    Stall,
    /// The turn never ends, not even when it is cancelled.
    Deaf,
}

/// The steps for a prompt, picked by its first word.
pub fn for_prompt(text: &str) -> Vec<Step> {
    // The master's messages start with the project they are about, on a line of its own: the script is
    // picked by what follows.
    let text = match text.strip_prefix("Project: ") {
        Some(rest) => rest.split_once('\n').map_or(rest, |(_, after)| after),
        None => text,
    };
    let word = text.split_whitespace().next().unwrap_or("").to_lowercase();
    match word.as_str() {
        "plain" => plain(),
        "push" => pushed(),
        "master" => master(),
        "think" => think(),
        "tools" => tools(),
        "diff" => diff(),
        "shell" => shell(),
        "terminal" => terminal(),
        "plan" => plan(),
        "permission" => permission(),
        "permission-safe" => permission_safe(),
        "permission-read" => permission_read(&path("src/folders/sort.rs")),
        "permission-read-outside" => permission_read(OUTSIDE_FILE),
        "permission-mcp" => permission_mcp(),
        "permission-grep" => permission_grep(),
        "permission-command" => permission_command(),
        "permission-command-context" => {
            permission_command_with_input(json!({ "command": "git status", "cwd": "relative" }))
        }
        "permission-twins" => permission_twins(),
        "permission-late" => permission_late(),
        "slow" => stream(SLOW_CHUNKS),
        "stream5000" => stream(STREAM_CHUNKS),
        "crash" => crash(),
        "stall" => vec![message("Thinking."), Step::Stall],
        "stall-late" => vec![message("Thinking."), Step::Linger(late_words())],
        "deaf" => vec![message("I keep going."), Step::Deaf],
        "lines" => vec![message(LINES)],
        "fenced" => fenced(),
        "refusal" => vec![message(REFUSAL), Step::Refuse],
        _ if text.starts_with('/') => vec![message(&format!("Ran {text}."))],
        // A publishing request in any language names what it asks for.
        _ if text.to_lowercase().contains("pull request") => pushed(),
        _ => showcase(),
    }
}

/// An answer whose lines stand under each other, with a list and a code block.
const LINES: &str = "Here are the numbers, one per line:\n1\n2\n3\n\nAnd a list:\n- one\n- two\n\n```sh\nls -la\ncat hello.txt\n```\nDone.";
/// What a provider writes when its model refuses a prompt, as the message that ends the turn.
const REFUSAL: &str = "API Error: the model is unable to respond to this request, which appears to violate the Usage Policy (https://example.com/legal/aup). Request ID: req_011FakeAgentRefusal. Try rephrasing the request or attempting a different approach.";

/// A command whose output the agent wraps in a markdown fence.
fn fenced() -> Vec<Step> {
    let output = "```console\nREADME.md\nhello.txt\n```";
    vec![
        tool_call(
            "call-ls",
            "ls",
            "execute",
            "completed",
            json!({ "rawInput": { "command": "ls" }, "content": text_content(output) }),
        ),
        message("Two files."),
    ]
}

/// The stored session, as `session/load` replays it.
pub fn history() -> Vec<Value> {
    let turn = [think(), plan(), tools(), diff(), terminal(), plain()];
    let mut updates = vec![chunk(
        "user_message_chunk",
        "Fix folder sorting and add a test.",
    )];
    updates.extend(turn.into_iter().flatten().filter_map(update_of));
    updates.push(chunk("user_message_chunk", "Great, thanks."));
    updates.push(chunk("agent_message_chunk", "Done. The test passes."));
    updates
}

/// A stored session whose only prompt the model refused: an agent replays the provider's words as a
/// plain message, with nothing that says they were a refusal.
pub fn refused_history() -> Vec<Value> {
    vec![
        chunk("user_message_chunk", "Fai quello che non si può."),
        chunk("agent_message_chunk", REFUSAL),
    ]
}

fn update_of(step: Step) -> Option<Value> {
    match step {
        Step::Update(update) => Some(update),
        _ => None,
    }
}

fn chunk(kind: &str, text: &str) -> Value {
    json!({ "sessionUpdate": kind, "content": { "type": "text", "text": text } })
}

fn message(text: &str) -> Step {
    Step::Update(chunk("agent_message_chunk", text))
}

fn thought(text: &str) -> Step {
    Step::Update(chunk("agent_thought_chunk", text))
}

/// `text` as message steps of one word each, with a pause between them.
fn words(text: &str, kind: fn(&str) -> Step) -> Vec<Step> {
    text.split_inclusive(' ')
        .flat_map(|word| [kind(word), Step::Pause(1)])
        .collect()
}

fn tool_call(id: &str, title: &str, kind: &str, status: &str, more: Value) -> Step {
    let mut call = json!({
        "sessionUpdate": "tool_call",
        "toolCallId": id,
        "title": title,
        "kind": kind,
        "status": status,
    });
    merge(&mut call, more);
    Step::Update(call)
}

fn tool_update(id: &str, fields: Value) -> Step {
    let mut update = json!({ "sessionUpdate": "tool_call_update", "toolCallId": id });
    merge(&mut update, fields);
    Step::Update(update)
}

fn merge(into: &mut Value, from: Value) {
    if let (Some(into), Value::Object(from)) = (into.as_object_mut(), from) {
        into.extend(from);
    }
}

fn text_content(text: &str) -> Value {
    json!([{ "type": "content", "content": { "type": "text", "text": text } }])
}

fn path(relative: &str) -> String {
    let workspace = std::env::var(WORKSPACE_VARIABLE).unwrap_or_else(|_| WORKSPACE.to_string());
    format!("{workspace}/{relative}")
}

/// What an agent that was asked to publish answers: the pull request it opened, then a last word.
fn pushed() -> Vec<Step> {
    words(
        "I pushed and opened the draft pull request: \
         https://github.com/acme/desk/pull/12 with the summary.",
        message,
    )
}

fn plain() -> Vec<Step> {
    words(
        "I read the module: sorting compares names **without** normalization, \
         so `Élite` comes after `Zeta`. Compare the normalized form.",
        message,
    )
}

/// What the engine's master does in one turn: it starts a lane, reads files, and proposes a note to the
/// knowledge. The lane and the note are the ones of the engine's test fixtures (`demo`, `note-1`). Its words
/// are English, as the master's are.
fn master() -> Vec<Step> {
    let mut steps = words(
        "Started a lane for it. The reviewer gates every step.",
        message,
    );
    steps.push(tool_call(
        "master-lane",
        "mcp__sakura-master__lane_open",
        "other",
        "completed",
        json!({
            "rawInput": { "project": "repo", "text": "Make uploads retry with backoff" },
            "content": text_content(r#"{"lane": "demo"}"#),
        }),
    ));
    steps.push(tool_call(
        "master-read",
        "Read 6 files, searched the code twice",
        "read",
        "completed",
        json!({ "content": text_content("src/upload.rs") }),
    ));
    steps.extend(words(
        "This is the change I propose to the knowledge. It applies when you approve it, here or in the Inbox.",
        message,
    ));
    steps.push(tool_call(
        "master-note",
        "mcp__sakura-master__knowledge_propose",
        "other",
        "completed",
        json!({
            "rawInput": { "path": "notes/parser.md", "text": "ignored, the engine's wait is what is shown" },
            "content": text_content(r#"{"proposal": "note-1"}"#),
        }),
    ));
    steps
}

fn think() -> Vec<Step> {
    let mut steps = words(
        "The folders should be alphabetical. The comparison uses bytes instead of letters. \
         I will find where the list is built and change the sort key.",
        thought,
    );
    steps.extend(words(
        "The problem is the sort key. I will change it and add a test.",
        message,
    ));
    steps
}

/// Three calls that end in different states; the second one's first update arrives before its `tool_call`.
fn tools() -> Vec<Step> {
    let read = path("src/folders/sort.rs");
    vec![
        tool_call(
            "call-read",
            "Read src/folders/sort.rs",
            "read",
            "pending",
            json!({ "locations": [{ "path": read, "line": 12 }] }),
        ),
        Step::Pause(4),
        tool_update("call-read", json!({ "status": "in_progress" })),
        tool_update("call-search", json!({ "status": "in_progress" })),
        Step::Pause(4),
        tool_call(
            "call-search",
            "Search \"sort_by\" in src",
            "search",
            "in_progress",
            json!({ "locations": [{ "path": path("src") }] }),
        ),
        tool_update(
            "call-read",
            json!({ "status": "completed", "content": text_content("pub fn sort(folders: &mut [Folder]) {\n    folders.sort_by(|a, b| a.name.cmp(&b.name));\n}") }),
        ),
        Step::Pause(4),
        tool_update(
            "call-search",
            json!({ "status": "failed", "content": text_content("rg: src/legacy: Permission denied") }),
        ),
        tool_call(
            "call-test",
            "cargo test folders",
            "execute",
            "in_progress",
            json!({ "rawInput": { "command": "cargo test folders" } }),
        ),
        Step::Pause(4),
    ]
}

fn diff() -> Vec<Step> {
    let file = path("src/folders/sort.rs");
    let old = "use crate::Folder;\n\npub fn sort(folders: &mut [Folder]) {\n    folders.sort_by(|a, b| a.name.cmp(&b.name));\n}\n";
    let new = "use crate::Folder;\nuse crate::text::normalize;\n\npub fn sort(folders: &mut [Folder]) {\n    folders.sort_by_key(|folder| normalize(&folder.name));\n}\n";
    vec![
        tool_call(
            "call-edit",
            "Edit src/folders/sort.rs",
            "edit",
            "in_progress",
            json!({ "locations": [{ "path": file }] }),
        ),
        Step::Pause(4),
        tool_update(
            "call-edit",
            json!({
                "status": "completed",
                "content": [{ "type": "diff", "path": file, "oldText": old, "newText": new }],
            }),
        ),
    ]
}

/// A command that changes a file and names none, as `printf ... >> README.md` does; the turn then goes on
/// until it is cancelled, so what changed on disk is all that says where the agent is while it works.
fn shell() -> Vec<Step> {
    let command = "printf 'one line\\n' >> README.md";
    vec![
        tool_call(
            "call-shell",
            command,
            "execute",
            "completed",
            json!({ "rawInput": { "command": command } }),
        ),
        message("Ho aggiunto one line al README."),
        Step::Stall,
    ]
}

/// A command whose output grows: each update carries the whole output so far. It is the call `tools`
/// leaves running, so the showcase and the stored session see it through to its end.
fn terminal() -> Vec<Step> {
    let lines = [
        "   Compiling demo v0.1.0 (/work/demo)",
        "    Finished `test` profile in 2.31s",
        "     Running unittests src/lib.rs",
        "",
        "running 3 tests",
        "test folders::sort_accents ... ok",
        "test folders::sort_empty ... ok",
        "test folders::sort_stable ... ok",
        "",
        "test result: ok. 3 passed; 0 failed; 0 ignored",
    ];
    let mut steps = vec![tool_call(
        "call-test",
        "cargo test folders",
        "execute",
        "in_progress",
        json!({ "rawInput": { "command": "cargo test folders" } }),
    )];
    for shown in 1..=lines.len() {
        let output = lines[..shown].join("\n");
        steps.push(Step::Pause(2));
        steps.push(tool_update(
            "call-test",
            json!({ "content": text_content(&output) }),
        ));
    }
    steps.push(tool_update("call-test", json!({ "status": "completed" })));
    steps
}

/// A plan sent three times, whole each time.
fn plan() -> Vec<Step> {
    let entries = |states: [&str; 3]| {
        let entry = |content: &str, status: &str| json!({ "content": content, "priority": "medium", "status": status });
        json!({
            "sessionUpdate": "plan",
            "entries": [
                entry("Find where the list is sorted", states[0]),
                entry("Sort by the normalized form", states[1]),
                entry("Add an accent test", states[2]),
            ],
        })
    };
    vec![
        Step::Update(entries(["in_progress", "pending", "pending"])),
        Step::Pause(4),
        Step::Update(entries(["completed", "in_progress", "pending"])),
        Step::Pause(4),
        Step::Update(entries(["completed", "completed", "in_progress"])),
    ]
}

fn options() -> Value {
    json!([
        { "optionId": "allow-once", "name": "Allow", "kind": "allow_once" },
        { "optionId": "allow-always", "name": "Always allow", "kind": "allow_always" },
        { "optionId": "reject-once", "name": "Reject", "kind": "reject_once" },
    ])
}

/// A fetch outside the box: the person must decide.
fn permission() -> Vec<Step> {
    let call = json!({
        "toolCallId": "call-fetch",
        "title": "Download https://crates.io/api/v1/crates/unicode-normalization",
        "kind": "fetch",
        "status": "pending",
        "rawInput": { "url": "https://crates.io/api/v1/crates/unicode-normalization" },
        "_meta": claude_tool("WebFetch"),
    });
    vec![
        message("I need the current library version."),
        Step::Pause(2),
        Step::Permission {
            request: json!({ "toolCall": call, "options": options() }),
            allowed: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-fetch", "status": "completed", "content": text_content("unicode-normalization 0.1.25") }),
            refused: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-fetch", "status": "failed" }),
        },
        message(" Continuing."),
    ]
}

/// An edit inside the workspace: the client may allow it by itself.
fn permission_safe() -> Vec<Step> {
    let file = path("src/folders/sort.rs");
    let call = json!({
        "toolCallId": "call-safe-edit",
        "title": "Edit src/folders/sort.rs",
        "kind": "edit",
        "status": "pending",
        "locations": [{ "path": file }],
        "rawInput": { "file_path": file, "old_string": "before", "new_string": "after" },
        "_meta": claude_tool("Edit"),
    });
    vec![
        Step::Permission {
            request: json!({ "toolCall": call, "options": options() }),
            allowed: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-safe-edit", "status": "completed" }),
            refused: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-safe-edit", "status": "failed" }),
        },
        message("Edited."),
    ]
}

/// A read of `file` by the Claude agent's Read tool, as its adapter reports one.
fn permission_read(file: &str) -> Vec<Step> {
    let call = json!({
        "toolCallId": "call-read",
        "title": format!("Read {file}"),
        "kind": "read",
        "status": "pending",
        "locations": [{ "path": file }],
        "rawInput": { "file_path": file },
        "_meta": claude_tool("Read"),
    });
    vec![
        Step::Permission {
            request: json!({ "toolCall": call, "options": options() }),
            allowed: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-read", "status": "completed" }),
            refused: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-read", "status": "failed" }),
        },
        message("Letto."),
    ]
}

/// A search of the workspace by the Claude agent's Grep, with a regular expression that looks like a
/// path that climbs out.
fn permission_grep() -> Vec<Step> {
    let call = json!({
        "toolCallId": "call-grep",
        "name": "Grep",
        "title": "grep -n \"\\.\\./\\.\\.\\(\"",
        "kind": "search",
        "status": "pending",
        "rawInput": { "pattern": "\\.\\./\\.\\.\\(", "-n": true },
        "_meta": claude_tool("Grep"),
    });
    vec![
        Step::Permission {
            request: json!({ "toolCall": call, "options": options() }),
            allowed: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-grep", "status": "completed" }),
            refused: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-grep", "status": "failed" }),
        },
        message("Cercato."),
    ]
}

/// A known command by the Claude agent's Bash: a package script, whose behaviour the workspace decides.
fn permission_command() -> Vec<Step> {
    permission_command_with_input(json!({ "command": "npm run test" }))
}

fn permission_command_with_input(input: Value) -> Vec<Step> {
    let call = json!({
        "toolCallId": "call-command",
        "title": "npm run test",
        "kind": "execute",
        "status": "pending",
        "rawInput": input,
        "_meta": claude_tool("Bash"),
    });
    vec![
        Step::Permission {
            request: json!({ "toolCall": call, "options": options() }),
            allowed: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-command", "status": "completed" }),
            refused: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-command", "status": "failed" }),
        },
        message("Done."),
    ]
}

/// A tool of an MCP server the login configured, which the client knows nothing of.
fn permission_mcp() -> Vec<Step> {
    let call = json!({
        "toolCallId": "call-mcp",
        "title": "mcp__notes__export",
        "kind": "other",
        "status": "pending",
        "rawInput": { "all": true },
        "_meta": claude_tool("mcp__notes__export"),
    });
    vec![
        Step::Permission {
            request: json!({ "toolCall": call, "options": options() }),
            allowed: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-mcp", "status": "completed" }),
            refused: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-mcp", "status": "failed" }),
        },
        message("Esportato."),
    ]
}

/// The `_meta` the Claude agent's adapter puts on a tool call: the name of the tool.
fn claude_tool(name: &str) -> Value {
    json!({ "claudeCode": { "toolName": name } })
}

/// The same safe edit, offered with two options under one id: allowing it once and allowing it always.
/// An answer by id cannot say which is meant.
fn permission_twins() -> Vec<Step> {
    let call = json!({
        "toolCallId": "call-twin-edit",
        "title": "Edit src/folders/sort.rs",
        "kind": "edit",
        "status": "pending",
        "locations": [{ "path": path("src/folders/sort.rs") }],
    });
    let twins = json!([
        { "optionId": "allow", "name": "Always allow", "kind": "allow_always" },
        { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
        { "optionId": "reject-once", "name": "Reject", "kind": "reject_once" },
    ]);
    vec![
        Step::Permission {
            request: json!({ "toolCall": call, "options": twins }),
            allowed: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-twin-edit", "status": "completed" }),
            refused: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-twin-edit", "status": "failed" }),
        },
        message("Edited."),
    ]
}

/// A safe edit whose permission request leaves only after the turn was cancelled.
fn permission_late() -> Vec<Step> {
    let call = json!({
        "toolCallId": "call-late-edit",
        "title": "Edit src/folders/sort.rs",
        "kind": "edit",
        "status": "pending",
        "locations": [{ "path": path("src/folders/sort.rs") }],
    });
    vec![
        message("Starting."),
        tool_call(
            "call-late-edit",
            "Edit src/folders/sort.rs",
            "edit",
            "pending",
            json!({}),
        ),
        Step::LatePermission {
            request: json!({ "toolCall": call, "options": options() }),
            allowed: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-late-edit", "status": "completed" }),
            refused: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-late-edit", "status": "failed" }),
        },
    ]
}

/// What a turn still says after it was told to stop.
fn late_words() -> Vec<Value> {
    ["I arrive", " in", " late."]
        .iter()
        .map(|word| chunk("agent_message_chunk", word))
        .collect()
}

/// A long markdown answer in `count` chunks of a word or two.
fn stream(count: usize) -> Vec<Step> {
    const TEXT: &str = "## How sorting works\n\nThe folder list is built once and then \
        sorted by the **normalized form** of each name, placing accented letters beside their base letters. \
        The comparison stays stable: folders with the same name keep their original order.\n\n\
        - the key is computed once per folder\n- empty names go last\n- `normalize` does not allocate \
        for names already in ASCII\n\n```rust\npub fn sort(folders: &mut [Folder]) {\n    \
        folders.sort_by_key(|folder| normalize(&folder.name));\n}\n```\n\nThe test covers accents, \
        the empty list and stability. A language-specific ordering needs one change.\n\n";
    let pieces: Vec<&str> = TEXT.split_inclusive(' ').collect();
    (0..count)
        .flat_map(|at| [message(pieces[at % pieces.len()]), Step::Pause(1)])
        .collect()
}

fn crash() -> Vec<Step> {
    vec![message("Starting work"), Step::Pause(2), Step::Crash]
}

fn showcase() -> Vec<Step> {
    let mut steps = think();
    for part in [plan(), tools(), diff(), terminal(), permission(), plain()] {
        steps.push(Step::Pause(6));
        steps.extend(part);
    }
    steps
}
