//! Whether a command may run without the person. By default it may not: a command goes to the person
//! unless every one of these holds.
//!
//! 1. It is one simple command that the strict lexer reads with certainty (`lexer`): no expansion, no
//!    substitution, no escape, no control operator, no redirection, no glob.
//! 2. No word is a URL, and no word is `cd`, `pushd` or `popd` (a changed directory would move every
//!    relative path).
//! 3. Its first word, whole, is a program on [`KNOWN_PROGRAMS`] (a path whose last part is a known name is
//!    not that name), and where the list names subcommands, its second word is one of them.
//! 4. No argument is a flag that makes a program run or evaluate something ([`EVALUATING_FLAGS`]).
//! 5. A short option stands alone (`-n`, its value in the next word). One written together with anything
//!    (`-C..`, `-n5`, `-rf`) passes only when every letter after the dash is a switch that program is
//!    known to take with no value ([`SWITCHES`]): an attached value hides an evaluating flag from rule 4
//!    and a path from rule 6, and only the program's own grammar says where a run of letters ends.
//! 6. No argument has a `..` part, and every absolute path in an argument is inside the workspace.
//!
//! What this cannot see: a symbolic link inside the workspace that points outside it, and what the known
//! programs go on to run (a test suite, a Makefile, an npm script, a git hook). The container is the
//! boundary for those; this is the second layer, and it only decides when to interrupt the person.

use super::{is_inside, lexer, Reason};
use std::path::Path;

/// The programs approved automatically, matched by their whole first word. An empty list of subcommands
/// means the program takes none; otherwise the second word must be one of them.
const KNOWN_PROGRAMS: [(&str, &[&str]); 24] = [
    (
        "cargo",
        &[
            "build", "check", "test", "clippy", "fmt", "doc", "tree", "run", "bench",
        ],
    ),
    (
        "git",
        &[
            "status",
            "diff",
            "log",
            "show",
            "add",
            "commit",
            "branch",
            "switch",
            "checkout",
            "restore",
            "stash",
            "rev-parse",
            "blame",
            "ls-files",
            "mv",
            "rm",
            "tag",
        ],
    ),
    ("npm", &["test", "run"]),
    ("pnpm", &["test", "run"]),
    ("yarn", &["test", "run"]),
    ("bun", &["test", "run"]),
    ("make", &[]),
    ("rg", &[]),
    ("grep", &[]),
    ("ls", &[]),
    ("cat", &[]),
    ("head", &[]),
    ("tail", &[]),
    ("wc", &[]),
    ("pwd", &[]),
    ("echo", &[]),
    ("mkdir", &[]),
    ("touch", &[]),
    ("cp", &[]),
    ("mv", &[]),
    ("rm", &[]),
    ("diff", &[]),
    ("sort", &[]),
    ("uniq", &[]),
];

/// Flags that make a program evaluate text or run another program, with or without `=value`. They are
/// refused on every known program, so a harmless use (`rg -e pattern`) also goes to the person.
const EVALUATING_FLAGS: [&str; 15] = [
    "-c",
    "-e",
    "-E",
    "--eval",
    "--exec",
    "--command",
    "--config",
    "--pre",
    "--ext-diff",
    "--textconv",
    "--open-files-in-pager",
    "--compress-program",
    "--script-shell",
    "--upload-pack",
    "--receive-pack",
];

/// The short options each program takes with no value: the only letters that may be written together
/// after one dash. A program that is not here takes its short options one per word.
const SWITCHES: [(&str, &str); 12] = [
    ("ls", "lahRtr1dFSAGp"),
    ("rm", "rfRdv"),
    ("cp", "rRapfvn"),
    ("mv", "fnv"),
    ("mkdir", "pv"),
    ("grep", "rRinlvwcHhosqxF"),
    ("rg", "ilnvwsSFHNox"),
    ("cat", "nbs"),
    ("wc", "lwcm"),
    ("sort", "rnufbdh"),
    ("uniq", "cdui"),
    ("diff", "urNqwbBa"),
];

/// Words that change the directory relative paths are read from.
const DIRECTORY_CHANGES: [&str; 3] = ["cd", "pushd", "popd"];

/// Programs that reach the network, another machine or more rights. Only used to tell the person why a
/// command was not approved; every program off [`KNOWN_PROGRAMS`] asks anyway.
const REACHING_PROGRAMS: [&str; 22] = [
    "curl", "wget", "ssh", "scp", "sftp", "rsync", "nc", "ncat", "netcat", "telnet", "ftp", "gh",
    "docker", "podman", "kubectl", "npx", "pnpx", "bunx", "pip", "pip3", "claude", "codex",
];

/// Subcommands of known programs that reach the network or install things; again only for the reason.
const REACHING_SUBCOMMANDS: [&str; 14] = [
    "push",
    "pull",
    "fetch",
    "clone",
    "remote",
    "submodule",
    "ls-remote",
    "install",
    "add",
    "update",
    "publish",
    "login",
    "ci",
    "search",
];

/// Characters that separate one value from the next inside an argument (`--flag=value`, `a:b`, `a,b`).
const VALUE_BREAKS: [char; 3] = ['=', ':', ','];

/// Why the command `text` must reach the person; `None` only when it passes every rule above.
pub(super) fn reason_to_ask(text: &str, workspace: &Path) -> Option<Reason> {
    let Some(words) = lexer::words(text) else {
        return Some(Reason::UnknownCommand);
    };
    let Some((program, arguments)) = words.split_first() else {
        return Some(Reason::UnknownTarget);
    };
    if words.iter().any(|word| word.contains("://")) {
        return Some(Reason::Network);
    }
    if words
        .iter()
        .any(|word| DIRECTORY_CHANGES.contains(&word.as_str()))
    {
        return Some(Reason::UnknownCommand);
    }
    if !is_known(program, arguments) {
        return Some(unknown_reason(program, arguments));
    }
    if arguments
        .iter()
        .any(|argument| is_evaluating_flag(argument))
    {
        return Some(Reason::UnknownCommand);
    }
    let leaves = arguments
        .iter()
        .any(|argument| leaves_workspace(argument, workspace));
    if leaves {
        return Some(Reason::OutsideWorkspace);
    }
    let is_unread = arguments
        .iter()
        .any(|argument| is_unread_cluster(program, argument));
    is_unread.then_some(Reason::UnknownCommand)
}

/// Whether `program` is on the list and, where the list names subcommands, is followed by one.
fn is_known(program: &str, arguments: &[String]) -> bool {
    let Some((_, subcommands)) = KNOWN_PROGRAMS.iter().find(|(name, _)| *name == program) else {
        return false;
    };
    match (subcommands.is_empty(), arguments.first()) {
        (true, _) => true,
        (false, Some(subcommand)) => subcommands.contains(&subcommand.as_str()),
        (false, None) => false,
    }
}

/// What to tell the person about a command that is not known: that it reaches out, when that much is
/// plain, or just that it is not known.
fn unknown_reason(program: &str, arguments: &[String]) -> Reason {
    let subcommand = arguments.first().map(String::as_str);
    let reaches = REACHING_PROGRAMS.contains(&program)
        || subcommand.is_some_and(|subcommand| REACHING_SUBCOMMANDS.contains(&subcommand));
    if reaches {
        Reason::Network
    } else {
        Reason::UnknownCommand
    }
}

fn is_evaluating_flag(argument: &str) -> bool {
    let flag = argument.split('=').next().unwrap_or(argument);
    EVALUATING_FLAGS.contains(&flag)
}

/// Whether `argument` is a short option written together with something (more letters, or a value) that
/// `program`'s known switches do not account for letter by letter.
fn is_unread_cluster(program: &str, argument: &str) -> bool {
    let Some(cluster) = argument.strip_prefix('-') else {
        return false;
    };
    if cluster.starts_with('-') || cluster.chars().count() < 2 {
        return false;
    }
    let switches = SWITCHES.iter().find(|(name, _)| *name == program);
    let is_read = switches.is_some_and(|(_, known)| cluster.chars().all(|c| known.contains(c)));
    !is_read
}

/// Whether an argument names a place outside the workspace: any `..` part, or an absolute path that is
/// not under the workspace. Relative paths without `..` stay inside, because nothing changes directory.
fn leaves_workspace(argument: &str, workspace: &Path) -> bool {
    let has_parent_part = argument
        .split(|c| c == '/' || VALUE_BREAKS.contains(&c))
        .any(|part| part == "..");
    has_parent_part || absolute_paths(argument).any(|path| !is_inside(path, workspace))
}

/// The absolute paths inside an argument: a value that starts with `/`, and what follows the first `/` of
/// a flag's value (`-I/usr/include`).
fn absolute_paths(argument: &str) -> impl Iterator<Item = &str> {
    argument.split(VALUE_BREAKS).filter_map(|value| {
        let start = match value.starts_with('-') {
            true => value.find('/')?,
            false => value.starts_with('/').then_some(0)?,
        };
        Some(&value[start..])
    })
}
