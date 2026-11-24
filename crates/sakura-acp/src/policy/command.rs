//! Whether a command may run without the person. By default it may not: a command goes to the person
//! unless every one of these holds.
//!
//! 1. It is one simple command that the strict lexer reads with certainty (`lexer`): no expansion, no
//!    substitution, no escape, no control operator, no redirection, no glob.
//! 2. No word is a URL, and no word is `cd`, `pushd` or `popd` (a changed directory would move every
//!    relative path).
//! 3. Its first word, whole, is a program on [`KNOWN_PROGRAMS`] (a path whose last part is a known name is
//!    not that name), and where the list names subcommands, its second word is one of them.
//! 4. Its options and operands match the program's supported grammar.
//! 5. No argument has a `..` part, and every absolute path is inside the workspace.
//!
//! Container isolation applies at execution, including symlink resolution and child processes. The Mac
//! policy always asks before running a command.

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

/// The short options each program takes with no value: the only letters that may be written together
/// after one dash.
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
    let leaves = arguments
        .iter()
        .any(|argument| leaves_workspace(argument, workspace));
    if leaves {
        return Some(Reason::OutsideWorkspace);
    }
    (!has_known_arguments(program, arguments)).then_some(Reason::UnknownCommand)
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

fn has_known_arguments(program: &str, arguments: &[String]) -> bool {
    let (verb, arguments) = match program {
        "cargo" | "git" | "npm" | "pnpm" | "yarn" | "bun" => {
            (arguments[0].as_str(), &arguments[1..])
        }
        _ => ("", arguments),
    };
    let switches = SWITCHES.iter().find(|(name, _)| *name == program);
    let mut words = arguments.iter().map(String::as_str);
    let mut operands = 0;
    while let Some(word) = words.next() {
        match word {
            "--" if matches!(program, "npm" | "pnpm" | "yarn" | "bun") && verb == "run" => {
                return operands == 1 && words.eq(["--fix"]);
            }
            "-m" if (program, verb) == ("git", "commit") => {
                if words.next().is_none() {
                    return false;
                }
            }
            "-n" if matches!(program, "head" | "tail") => {
                if !words
                    .next()
                    .is_some_and(|value| value.parse::<u64>().is_ok())
                {
                    return false;
                }
            }
            flag if flag.starts_with("--") && accepts_long_option(program, verb, flag) => {}
            flag if is_short_option(flag, switches.map(|(_, letters)| *letters)) => {}
            value if is_operand(program, value) => operands += 1,
            _ => return false,
        }
    }
    accepts_operands(program, verb, operands)
}

fn accepts_long_option(program: &str, verb: &str, flag: &str) -> bool {
    match (program, verb) {
        ("cargo", "build" | "check" | "test" | "clippy" | "doc" | "run" | "bench") => {
            matches!(
                flag,
                "--release" | "--workspace" | "--all-targets" | "--locked" | "--offline"
            )
        }
        ("cargo", "fmt") => matches!(flag, "--all" | "--check"),
        ("git", "status") => matches!(flag, "--short" | "--porcelain" | "--branch"),
        ("git", "diff" | "show") => matches!(
            flag,
            "--stat" | "--name-only" | "--name-status" | "--no-index"
        ),
        ("git", "log") => matches!(flag, "--oneline" | "--graph" | "--decorate"),
        ("rg" | "grep", "") => {
            matches!(flag, "--line-number" | "--ignore-case" | "--fixed-strings")
        }
        _ => false,
    }
}

fn is_short_option(argument: &str, switches: Option<&str>) -> bool {
    let Some(cluster) = argument.strip_prefix('-').filter(|value| !value.is_empty()) else {
        return false;
    };
    switches.is_some_and(|known| cluster.chars().all(|c| known.contains(c)))
}

fn is_operand(program: &str, word: &str) -> bool {
    if word.starts_with('-') {
        return false;
    }
    match program {
        "make" | "npm" | "pnpm" | "yarn" | "bun" => {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte))
        }
        "git" => !word.is_empty() && !word.starts_with(":("),
        _ => true,
    }
}

fn accepts_operands(program: &str, verb: &str, count: usize) -> bool {
    match (program, verb) {
        ("pwd", _) | ("cargo", "build" | "check" | "clippy" | "fmt" | "doc" | "tree") => count == 0,
        ("npm" | "pnpm" | "yarn" | "bun", "run") => count == 1,
        ("npm" | "pnpm" | "yarn" | "bun", "test") => count == 0,
        ("cp" | "mv", _) | ("git", "mv") => count >= 2,
        ("diff", _) => count == 2,
        ("uniq", _) => count <= 2,
        ("rg" | "grep" | "mkdir" | "rm" | "touch", _) | ("git", "add" | "rm") => count >= 1,
        _ => true,
    }
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
