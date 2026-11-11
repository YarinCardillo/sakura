//! Publishing the work of a task on the Mac. `box` has no verb for it: the app sends the task's
//! agent a request in its conversation (push the branch, open a draft pull request against the
//! base), and reads the link of the pull request from what the agent writes back. Here are the
//! request (the project's own text, or the default one) and the reading of the link. Pure: no
//! process, no network.

use crate::error::{BridgeError, BridgeResult};
use crate::said::Said;

/// The longest request a project keeps, in characters.
pub const REQUEST_MAX_CHARS: usize = 2000;
/// How much of the end of a text is looked at for a link, in bytes.
pub const SCAN_MAX_BYTES: usize = 256 * 1024;
/// The longest part of a link's path (an owner, a repository, a group), in characters.
pub const SEGMENT_MAX_CHARS: usize = 100;
/// The longest link looked at, in bytes.
const URL_MAX_BYTES: usize = 2048;
/// The longest host name.
const HOST_MAX_CHARS: usize = 253;
/// The most parts before a merge request's number (groups, subgroups and the project).
const GROUPS_MAX: usize = 20;
/// The most digits of a pull request's number, and of a port.
const NUMBER_MAX_DIGITS: usize = 10;
const PORT_MAX_DIGITS: usize = 5;
const SCHEME: &str = "https://";
/// The most links tried, newest first.
const CANDIDATES_MAX: usize = 64;

/// What the agent is asked to do to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// The project's own text, sent as it is.
    Own(String),
    /// The default one, said in the interface's language with the task's base branch.
    Default(Said),
}

/// The default request: push the branch and open a draft pull request against `base` with a
/// summary of the changes.
pub fn default_request(base: &str) -> Said {
    Said::new("publish.request.default").text("base", base)
}

/// The request for a task whose base branch is `base`: the project's `own` text when it has one.
pub fn request_for(own: Option<&str>, base: &str) -> Request {
    match own.map(str::trim).filter(|text| !text.is_empty()) {
        Some(own) => Request::Own(own.chars().take(REQUEST_MAX_CHARS).collect()),
        None => Request::Default(default_request(base)),
    }
}

/// What is kept of a request typed in Settings: the text, or `None` (the default one) when nothing
/// was typed. Lines are kept; any other control character and a text over the bound are refused.
pub fn own_request(typed: &str) -> BridgeResult<Option<String>> {
    let text = typed.trim();
    let is_text = text.chars().count() <= REQUEST_MAX_CHARS
        && !text.chars().any(|c| c.is_control() && c != '\n');
    if !is_text {
        return Err(BridgeError::invalid(Said::new("publish.invalid.request")));
    }
    Ok((!text.is_empty()).then(|| text.to_string()))
}

/// Which kind of host a link is in the shape of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// `https://<host>/<owner>/<repository>/pull/<number>`.
    GitHub,
    /// `https://<host>/<group>/.../<project>/-/merge_requests/<number>`.
    GitLab,
}

/// The link of a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    /// The link of the pull request itself, without what followed its number.
    pub url: String,
    pub number: u64,
    pub host: Host,
}

/// The newest link of a pull request in `text` (what the agent wrote), looking at its end only.
pub fn pull_request(text: &str) -> Option<PullRequest> {
    scan(text).0
}

/// The link, and how many bytes were looked at to find it or to find none. The work is bounded
/// whatever the text holds: the end of the text only, the newest [`CANDIDATES_MAX`] link starts
/// only, and of each no more than the longest link.
fn scan(text: &str) -> (Option<PullRequest>, usize) {
    let tail = tail(text);
    let mut examined = tail.len();
    for (at, _) in tail.rmatch_indices(SCHEME).take(CANDIDATES_MAX) {
        let start = start_of(&tail[at + SCHEME.len()..]);
        examined += start.len();
        if let Some(found) = whole_link(start).and_then(link) {
            return (Some(found), examined);
        }
    }
    (None, examined)
}

/// The last [`SCAN_MAX_BYTES`] of `text`, from a character's start.
fn tail(text: &str) -> &str {
    let from = text.len().saturating_sub(SCAN_MAX_BYTES);
    let from = (from..=text.len()).find(|at| text.is_char_boundary(*at));
    &text[from.unwrap_or(text.len())..]
}

/// The start of `rest`: one byte more than the longest link, cut back to a character's start.
fn start_of(rest: &str) -> &str {
    let most = rest.len().min(URL_MAX_BYTES + 1);
    let end = (0..=most).rev().find(|at| rest.is_char_boundary(*at));
    &rest[..end.unwrap_or(0)]
}

/// The link `start` begins with, up to the character that ends it; none when it goes on past the
/// longest link.
fn whole_link(start: &str) -> Option<&str> {
    let is_end = |c: char| c.is_whitespace() || "<>\"'()[]{}".contains(c);
    let whole = start.split(is_end).next()?;
    (whole.len() <= URL_MAX_BYTES - SCHEME.len()).then_some(whole)
}

/// The pull request `whole` (what follows `https://`, up to its end) links to, if it is one.
fn link(whole: &str) -> Option<PullRequest> {
    let path = whole.split(['?', '#']).next()?;
    let mut parts = path.split('/');
    let host = parts.next().filter(|host| is_host(host))?;
    let parts: Vec<&str> = parts.collect();
    let (kind, before_number) = shape(&parts)?;
    let names = &parts[..before_number];
    let number = number(parts.get(before_number)?)?;
    let are_names = names.iter().all(|name| is_segment(name));
    let url = format!("{SCHEME}{host}/{}/{number}", names.join("/"));
    are_names.then_some(PullRequest {
        url,
        number,
        host: kind,
    })
}

/// The kind of link the path's parts are in the shape of, and where its number stands.
fn shape(parts: &[&str]) -> Option<(Host, usize)> {
    if parts.len() >= 4 && parts[2] == "pull" {
        return Some((Host::GitHub, 3));
    }
    let dash = parts.iter().position(|part| *part == "-")?;
    let is_merge_request = parts.get(dash + 1) == Some(&"merge_requests");
    ((2..=GROUPS_MAX).contains(&dash) && is_merge_request).then_some((Host::GitLab, dash + 2))
}

/// A host name, with a port or without: letters, digits, dots and hyphens, never a login.
fn is_host(host: &str) -> bool {
    let (name, port) = host
        .split_once(':')
        .map_or((host, None), |(name, port)| (name, Some(port)));
    let is_letter = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-');
    let is_name = !name.is_empty()
        && name.len() <= HOST_MAX_CHARS
        && name.chars().all(is_letter)
        && name
            .split('.')
            .all(|label| !label.is_empty() && !label.starts_with('-'));
    let is_digits = |port: &str| {
        !port.is_empty()
            && port.len() <= PORT_MAX_DIGITS
            && port.chars().all(|c| c.is_ascii_digit())
    };
    is_name && port.map_or(true, is_digits)
}

/// One part of a path: an owner, a repository, a group, or a word of the shape itself.
fn is_segment(part: &str) -> bool {
    let is_letter = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_');
    !part.is_empty() && part.len() <= SEGMENT_MAX_CHARS && part.chars().all(is_letter)
}

/// The number a part starts with, when only the end of a sentence follows it.
fn number(part: &str) -> Option<u64> {
    let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
    let is_sentence_end = part[digits.len()..].chars().all(|c| ".,;:!".contains(c));
    let is_number = !digits.is_empty() && digits.len() <= NUMBER_MAX_DIGITS && is_sentence_end;
    digits
        .parse()
        .ok()
        .filter(|number| is_number && *number > 0)
}

#[cfg(test)]
mod tests;
