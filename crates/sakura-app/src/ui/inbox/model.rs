//! What the Inbox shows of the engine's snapshot, decided here and drawn elsewhere: the asks grouped by lane,
//! the words of an ask's kind, the verbs an ask offers in the interface's own vocabulary and the words of an
//! answer already given. It reads the engine's actions and invents none: a verb the engine does not offer on
//! an ask is not on the ask.

use crate::l10n::{tr, tr_with, Key};
use crate::ui::answers::lane_key;
use gpui::SharedString;
use sakura_core::engine::snapshot::{Action, Decided, Item, Snapshot, Wait};
use std::collections::BTreeMap;

const KIND_APPROVAL: &str = "approval";
const KIND_OWNER_ROW: &str = "owner_row";
const KIND_QUESTION: &str = "question";
pub(in crate::ui) const KIND_BLOCKED: &str = "blocked";

const GITHUB_DISCONNECTED: Key = Key("inbox.github_disconnected");

pub(super) fn needs_github(wait: &Wait) -> bool {
    [&wait.title, &wait.detail, &wait.technical]
        .into_iter()
        .any(|words| {
            let tokens: Vec<_> = words
                .split_whitespace()
                .map(|word| word.trim_matches(|c: char| c.is_ascii_punctuation()))
                .collect();
            tokens.windows(3).any(|tokens| {
                tokens[0].eq_ignore_ascii_case("gh")
                    && tokens[1].eq_ignore_ascii_case("auth")
                    && tokens[2].eq_ignore_ascii_case("login")
            })
        })
}

fn asks_for_terminal(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower
        .lines()
        .any(|line| line.trim_start().starts_with("$ "))
        || ["run ", "run: ", "type ", "execute "]
            .into_iter()
            .any(|verb| {
                lower.match_indices(verb).any(|(at, _)| {
                    (at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric())
                        && crate::ui::plain_error::shows_command_or_path(
                            lower[at + verb.len()..].trim_start_matches([' ', '`', '"', '\'']),
                        )
                })
            })
}

/// The error copy shown both in the Inbox and on the lane card.
pub fn plain_reason(wait: &Wait) -> crate::ui::plain_error::Plain {
    use crate::ui::plain_error::{plain_of, Plain};

    if needs_github(wait) {
        return Plain {
            words: tr(GITHUB_DISCONNECTED).to_string(),
            raw: None,
        };
    }
    let mut plain = match wait.kind == KIND_BLOCKED {
        true if !wait.technical.is_empty() && !asks_for_terminal(&wait.detail) => {
            plain_of(&wait.technical, &wait.detail)
        }
        true => plain_of(&wait.detail, ""),
        false => Plain {
            words: wait.detail.clone(),
            raw: None,
        },
    };
    if wait.kind != KIND_APPROVAL && asks_for_terminal(&plain.words) {
        plain = plain_of(&plain.words, "");
        if asks_for_terminal(&plain.words) {
            plain.words = tr(Key("error.plain.generic")).to_string();
        }
    }
    plain.raw = plain.raw.filter(|raw| !asks_for_terminal(raw));
    if wait.kind == KIND_BLOCKED
        && !wait.detail.trim().is_empty()
        && plain.raw.is_none()
        && !plain.words.contains(['\n', ':', '/'])
    {
        plain.words = super::super::lanes::capitalised(plain.words.trim());
        if !plain.words.is_empty() && !plain.words.ends_with(['.', '!', '?', '…']) {
            plain.words.push('.');
        }
    }
    plain
}
/// Whether the engine wrote the text of `wait` itself (a stuck lane, the approval of a gate, an automation): it
/// is shown as it is, never read as Markdown. What an agent or the master wrote (the spec to approve, a check,
/// a question, a proposal) is Markdown, made inert.
pub fn is_engine_text(wait: &Wait) -> bool {
    match wait.original_kind() {
        KIND_BLOCKED | KIND_AUTOMATION => true,
        KIND_APPROVAL => !super::is_spec_approval(wait),
        _ => false,
    }
}

pub(in crate::ui) fn is_folder_action(wait: &Wait, action: &str) -> bool {
    !wait.is_outside()
        && matches!(
            action,
            ACTION_APPROVE | ACTION_MARK | ACTION_RESUME | ACTION_STOP | "C" | "H" | "start"
        )
}

pub(super) fn checks_commands<'a>(wait: &Wait, words: &'a str) -> Option<(&'a str, Vec<&'a str>)> {
    if wait.original_kind() != KIND_APPROVAL || wait.key_name() != CHECKS_GATE {
        return None;
    }
    let (intro, lines) = words
        .trim_end_matches('\n')
        .rsplit_once("\n\n")
        .unwrap_or(("", words));
    let commands = lines
        .lines()
        .map(|line| {
            line.strip_prefix("- ")
                .filter(|command| !command.is_empty())
        })
        .collect::<Option<Vec<_>>>()?;
    (!commands.is_empty()).then_some((intro, commands))
}

/// The gate of an approval to deliver a lane that changes files another lane changed.
const OVERLAP_GATE: &str = "overlap";

/// The title of `wait` as the window says it: the engine's, but for the overlap ask, whose title the engine
/// writes in the singular while its sentence names the lanes that delivered first: with several, the title says
/// so too.
pub fn ask_title_words(wait: &Wait) -> String {
    if asks_for_terminal(&wait.title) {
        return plain_reason(wait).words;
    }
    let names_several = || {
        let lanes = wait.detail.split(": ").next().unwrap_or_default();
        lanes.matches('"').count() / 2 > 1
    };
    match wait.kind == KIND_APPROVAL && wait.key_name() == OVERLAP_GATE && names_several() {
        true => tr(Key("inbox.overlap_several")).to_string(),
        false => ellipsised(&wait.title, &wait.detail),
    }
}

/// `title`, ending with an ellipsis when it is the start of `detail` and not all of it: the engine cuts the
/// title it makes of the first line of an agent's words (`Try it yourself: <first line>`) at a length, with
/// nothing to say so.
fn ellipsised(title: &str, detail: &str) -> String {
    let said = title
        .split_once(": ")
        .map_or(title, |(_, said)| said)
        .trim();
    let is_cut = !said.is_empty()
        && detail.trim().len() > said.len()
        && detail.trim().starts_with(said)
        && !title.ends_with(['.', '?', '!', '…']);
    match is_cut {
        true => format!("{title}…"),
        false => title.to_string(),
    }
}

const KIND_PROPOSAL: &str = "proposal";
const KIND_AUTOMATION: &str = "automation";
/// The gate whose approval may be remembered.
const CHECKS_GATE: &str = "checks";
const ACTION_APPROVE: &str = "a";
const ACTION_ACCEPT: &str = "y";
const ACTION_REJECT: &str = "N";
const ACTION_MARK: &str = "k";
const ACTION_REPLY: &str = "r";
const ACTION_RESUME: &str = "R";
const ACTION_STOP: &str = "s";
const INPUT_REMEMBER: &str = "remember";
const INPUT_VERDICT: &str = "verdict";
const INPUT_NOTE: &str = "note";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Section {
    NeedsYou,
    Triage,
}

impl Section {
    fn of(wait: &Wait) -> Self {
        if wait.section.as_deref() == Some("triage") {
            Self::Triage
        } else {
            Self::NeedsYou
        }
    }

    pub(super) fn label(self) -> Key {
        match self {
            Self::NeedsYou => Key("inbox.section.needs_you"),
            Self::Triage => Key("inbox.section.triage"),
        }
    }
}

/// The asks of one lane, as the list shows them under one band.
#[derive(Debug)]
pub struct Group<'a> {
    pub(super) section: Section,
    pub title: SharedString,
    /// The day of a morning digest, as the engine dated it; none for any other lane.
    pub date: Option<String>,
    /// When the newest ask of the lane was raised, in seconds; none when no ask says.
    pub newest: Option<i64>,
    pub asks: Vec<&'a Wait>,
}

#[cfg(test)]
thread_local! {
    static GROUP_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The groups of `snapshot`, the lane with the newest ask first and each lane's asks newest first.
pub fn groups(snapshot: &Snapshot) -> Vec<Group<'_>> {
    #[cfg(test)]
    GROUP_CALLS.with(|calls| calls.set(calls.get() + 1));
    let mut groups: Vec<Group> = Vec::new();
    let mut indices = BTreeMap::new();
    for wait in &snapshot.waits {
        let section = Section::of(wait);
        let id = wait
            .group
            .as_ref()
            .map_or_else(|| wait.lane_key(), |group| group.id.clone());
        let index = *indices.entry((section, id)).or_insert_with(|| {
            let index = groups.len();
            groups.push(Group {
                section,
                title: group_title(wait, snapshot),
                date: None,
                newest: None,
                asks: Vec::new(),
            });
            index
        });
        groups[index].asks.push(wait);
    }
    for group in &mut groups {
        group
            .asks
            .sort_by_key(|wait| std::cmp::Reverse(raised(wait)));
        group.newest = group.asks.first().and_then(|wait| raised(wait));
        group.date = group.asks.first().and_then(|wait| wait.digest.clone());
    }
    groups.sort_by_key(|group| (group.section, std::cmp::Reverse(group.newest)));
    groups
}

fn group_title(wait: &Wait, snapshot: &Snapshot) -> SharedString {
    if wait.is_digest() {
        return SharedString::from(tr(Key("inbox.digest")));
    }
    if let Some(origin) = wait
        .proposal
        .as_ref()
        .and_then(|proposal| proposal.origin.as_ref())
    {
        return origin.title.clone().into();
    }
    if let Some(group) = &wait.group {
        return group.label.clone().into();
    }
    if wait.kind == KIND_PROPOSAL {
        return SharedString::from(tr(Key("page.master")));
    }
    lane_of(wait, snapshot)
        .map_or_else(
            || wait.lane_name().to_string(),
            |lane| super::super::lanes::lane_words(lane, false),
        )
        .into()
}

/// When `wait` was raised, in seconds; none when the engine gave no time. An ask with no time is the oldest.
pub fn raised(wait: &Wait) -> Option<i64> {
    wait.since
        .as_deref()
        .and_then(sakura_core::graph::parse_time)
}

/// What narrows the list: one project, one kind of ask, or neither.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub project: Option<String>,
    pub kind: Option<String>,
    pub snoozed: bool,
}

impl Filter {
    /// Whether nothing is narrowed.
    pub fn is_empty(&self) -> bool {
        self.project.is_none() && self.kind.is_none() && !self.snoozed
    }

    #[cfg(test)]
    pub(super) fn allows_wait(&self, wait: &Wait, now: i64) -> bool {
        self.snoozed == wait.is_snoozed_at(now) && self.allows(&wait.project, wait_kind(wait))
    }

    pub(super) fn allows(&self, project: &str, kind: &str) -> bool {
        let is_project = self
            .project
            .as_deref()
            .is_none_or(|wanted| wanted == project);
        let is_kind = self.kind.as_deref().is_none_or(|wanted| wanted == kind);
        is_project && is_kind
    }
}

fn wake_time(wait: &Wait) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::{DateTime, Local};
    Some(DateTime::from_timestamp(wait.wake_at()?, 0)?.with_timezone(&Local))
}

pub(super) fn wake_words(wait: &Wait) -> Option<SharedString> {
    wake_words_for(
        wait,
        Key("inbox.snooze.wakes_today"),
        Key("inbox.snooze.wakes_date"),
    )
}

pub(super) fn snoozed_status(wait: &Wait) -> SharedString {
    wake_words_for(
        wait,
        Key("inbox.state.snoozed_until_today"),
        Key("inbox.state.snoozed_until_date"),
    )
    .unwrap_or_else(|| tr(Key("inbox.state.snoozed")).into())
}

fn wake_words_for(wait: &Wait, today: Key, date_key: Key) -> Option<SharedString> {
    let wake = wake_time(wait)?;
    let clock = wake.format("%H:%M").to_string();
    if wake.date_naive() == chrono::Local::now().date_naive() {
        return Some(tr_with(today, &[("clock", &clock)]));
    }
    let date = wake.format("%Y-%m-%d").to_string();
    Some(tr_with(date_key, &[("date", &date), ("clock", &clock)]))
}

/// The asks that wait for the person, in the order the list shows them, one after the other.
#[cfg(test)]
pub fn ordered<'a>(snapshot: &'a Snapshot, filter: &Filter) -> Vec<&'a Wait> {
    let now = super::list::now();
    groups(snapshot)
        .into_iter()
        .flat_map(|group| group.asks)
        .filter(|wait| filter.allows_wait(wait, now))
        .collect()
}

/// The answers the person gave lately that `filter` lets through, newest first.
pub fn decided_list<'a>(snapshot: &'a Snapshot, filter: &Filter) -> Vec<&'a Decided> {
    let mut list: Vec<&Decided> = snapshot
        .decided
        .iter()
        .flatten()
        .filter(|decided| filter.allows(&decided.project, history_kind(decided)))
        .collect();
    list.sort_by_key(|decided| std::cmp::Reverse(decided_at(decided)));
    list
}

/// When an answer was given, in seconds.
pub fn decided_at(decided: &Decided) -> Option<i64> {
    decided
        .at
        .as_deref()
        .and_then(sakura_core::graph::parse_time)
}

/// What names an answer in the list: the ask can be answered again after a resume, so its id is not enough.
pub fn decided_key(decided: &Decided) -> String {
    format!(
        "{}@{}",
        decided.id,
        decided.at.as_deref().unwrap_or_default()
    )
}

pub(super) fn decided_gate(decided: &Decided) -> Option<String> {
    let from_key = |key: &[String]| {
        key.get(4)
            .map(|gate| gate.split('@').next().unwrap_or_default().to_string())
    };
    from_key(&decided.key).or_else(|| {
        let key: Vec<String> = serde_json::from_str(&decided.id).ok()?;
        from_key(&key)
    })
}

/// The lane an answered ask was about, when the engine still lists it.
pub fn decided_lane<'a>(decided: &Decided, snapshot: &'a Snapshot) -> Option<&'a Item> {
    let is_about = |item: &&Item| match decided.lane.as_deref() {
        Some(lane) => item.task() == lane || item.id == lane,
        None => item.task() == decided.task && item.project() == decided.project,
    };
    snapshot.items.iter().find(is_about)
}

/// The lane an ask is about.
pub fn lane_of<'a>(wait: &Wait, snapshot: &'a Snapshot) -> Option<&'a Item> {
    let key = wait.lane_key();
    snapshot.items.iter().find(|item| item.lane_key() == key)
}

/// "From claude in work": whom an outside agent's ask is from, and where it works; none for the engine's own.
pub fn from_words(wait: &Wait) -> Option<SharedString> {
    if let Some(origin) = wait
        .proposal
        .as_ref()
        .and_then(|proposal| proposal.origin.as_ref())
    {
        let title = tr_with(Key("inbox.scheduled_origin"), &[("task", &origin.title)]);
        return Some(
            [
                Some(title.as_str()),
                origin.source.as_deref(),
                origin.at.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ")
            .into(),
        );
    }
    let is_known = wait.is_outside() && !wait.agent().is_empty();
    is_known.then(|| crate::ui::lanes::from_words(wait.agent(), &wait.project))
}

/// What an answer says it did, in the words of the interface: the toast's sentence and the Done list's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Say {
    Approve,
    AlwaysApprove,
    Decline,
    Resume,
    Stop,
    Close,
    Restore,
    Review,
    Start,
    Dismiss,
}

/// What a person answers an ask with: one of its verbs, one of the choices a question offers, or words of
/// their own. Every answer goes through the Inbox's held path, whichever page it is given on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Verb(Say),
    Resume(Option<String>),
    Choice(String),
    Reply(String),
}

/// One button the engine's actions give an ask: its word, the call it sends and what that call carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verb {
    pub say: Say,
    pub action: Action,
    /// What the answer is kept under in the window's answers.
    pub key: String,
    /// What the engine's action applies to.
    pub target: String,
    pub inputs: BTreeMap<String, String>,
    /// Whether the person may add a note to this answer: the engine's action takes one.
    pub takes_note: bool,
    /// Whether this is a verdict on a check, which the person looked at ("Done", "Not now"), not an approval.
    pub is_mark: bool,
}

/// The buttons of an ask.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Verbs {
    pub primary: Option<Verb>,
    pub secondary: Vec<Verb>,
    /// The answers an outside agent's question offers, one button each.
    pub choices: Vec<String>,
    /// The engine's action that answers in words, when the ask takes a reply.
    pub reply: Option<Action>,
}

impl Verbs {
    pub fn missing_blocked_actions(&self, wait: &Wait) -> impl Iterator<Item = (Say, Key)> + '_ {
        let is_blocked = wait.kind == KIND_BLOCKED && !wait.is_outside();
        [
            (Say::Resume, Key("waiting.resume")),
            (Say::Stop, Key("waiting.stop")),
        ]
        .into_iter()
        .filter(move |(say, _)| {
            is_blocked
                && !self
                    .primary
                    .iter()
                    .chain(&self.secondary)
                    .any(|verb| verb.say == *say)
        })
    }
}

/// What the engine lets the person do on `wait`; the lane's own actions serve a blocked lane's ask.
pub fn verbs(wait: &Wait, lane: Option<&Item>) -> Verbs {
    let mut verbs = Verbs::default();
    for action in &wait.actions {
        match action.key.as_str() {
            ACTION_APPROVE => approve(&mut verbs, wait, action),
            ACTION_ACCEPT => verbs.primary = Some(on_wait(Say::Approve, wait, action)),
            ACTION_REJECT => verbs.secondary.push(on_wait(Say::Decline, wait, action)),
            ACTION_MARK => mark(&mut verbs, wait, action),
            "markreviewed" => verbs.primary = Some(on_wait(Say::Review, wait, action)),
            "start" if wait.proposal.is_some() => {
                verbs.primary = Some(on_wait(Say::Start, wait, action))
            }
            "dismiss" => verbs.secondary.push(on_wait(Say::Dismiss, wait, action)),
            "H" => verbs.secondary.push(on_wait(Say::Restore, wait, action)),
            ACTION_RESUME | ACTION_STOP | "C" if wait.kind == KIND_BLOCKED => {
                let say = match action.key.as_str() {
                    ACTION_RESUME => Say::Resume,
                    ACTION_STOP => Say::Stop,
                    _ => Say::Close,
                };
                verbs.secondary.push(on_wait(say, wait, action));
            }
            "tracksettle" if wait.original_kind() == "track_decision" => {
                verbs.reply = Some(action.clone());
            }
            ACTION_REPLY => {
                verbs.choices = wait.choices.clone();
                verbs.reply = Some(action.clone());
            }
            _ => {}
        }
    }
    if wait.kind == KIND_BLOCKED {
        if wait.section.is_none() && verbs.secondary.is_empty() {
            lane_verbs(&mut verbs, lane);
        } else if let Some(index) = [Say::Resume, Say::Stop, Say::Close]
            .into_iter()
            .find_map(|say| verbs.secondary.iter().position(|verb| verb.say == say))
        {
            verbs.primary = Some(verbs.secondary.remove(index));
        }
    }
    if let Some(resume) = verbs
        .primary
        .as_mut()
        .filter(|verb| verb.say == Say::Resume)
    {
        resume.takes_note =
            wait.wants_note && resume.action.optional.iter().any(|name| name == "note");
    }
    verbs
}

fn on_wait(say: Say, wait: &Wait, action: &Action) -> Verb {
    Verb {
        say,
        action: action.clone(),
        key: wait.id.clone(),
        target: wait.id.clone(),
        inputs: BTreeMap::new(),
        takes_note: false,
        is_mark: false,
    }
}

/// "Approve", and on the checks gate "Always approve".
fn approve(verbs: &mut Verbs, wait: &Wait, action: &Action) {
    verbs.primary = Some(on_wait(Say::Approve, wait, action));
    let can_remember =
        wait.key_name() == CHECKS_GATE && action.optional.iter().any(|name| name == INPUT_REMEMBER);
    if can_remember {
        let mut always = on_wait(Say::AlwaysApprove, wait, action);
        always.inputs.insert(INPUT_REMEMBER.into(), "yes".into());
        verbs.secondary.push(always);
    }
}

/// A mark is Approve with the verdict ok, and Decline with the verdict ko, which may carry a note.
fn mark(verbs: &mut Verbs, wait: &Wait, action: &Action) {
    let verdict = |say, value: &str| {
        let mut verb = on_wait(say, wait, action);
        verb.inputs.insert(INPUT_VERDICT.into(), value.into());
        verb.is_mark = true;
        verb
    };
    verbs.primary = Some(verdict(Say::Approve, "ok"));
    let mut decline = verdict(Say::Decline, "ko");
    decline.takes_note = action.optional.iter().any(|name| name == INPUT_NOTE);
    verbs.secondary.push(decline);
}

/// "Resume" and "Stop" of a blocked ask are the lane's own actions, answered under the lane's id.
fn lane_verbs(verbs: &mut Verbs, lane: Option<&Item>) {
    let Some(lane) = lane else {
        return;
    };
    for (key, say) in [(ACTION_RESUME, Say::Resume), (ACTION_STOP, Say::Stop)] {
        let Some(action) = lane.actions.iter().find(|action| action.key == key) else {
            continue;
        };
        let verb = Verb {
            say,
            action: action.clone(),
            key: lane_key(&lane.id, key),
            target: lane.id.clone(),
            inputs: BTreeMap::new(),
            takes_note: false,
            is_mark: false,
        };
        match say {
            Say::Resume => verbs.primary = Some(verb),
            _ => verbs.secondary.push(verb),
        }
    }
}

impl Verb {
    /// The word on the verb's button: a verdict on a check says "Done" or "Not now", since "Approve" and
    /// "Decline" mean nothing for a thing the person only looked at.
    pub fn label(&self) -> SharedString {
        let key = match (self.is_mark, self.say) {
            (true, Say::Approve) => Key("inbox.mark.done"),
            (true, Say::Decline) => Key("inbox.mark.not_now"),
            (false, Say::Approve) => Key("waiting.approve"),
            (false, Say::AlwaysApprove) => Key("inbox.always_approve"),
            (false, Say::Decline) => Key("inbox.decline"),
            (_, Say::Resume) => Key("waiting.resume"),
            (_, Say::Stop) => Key("waiting.stop"),
            (_, Say::Close) => Key("inbox.close"),
            (_, Say::Restore) => Key("inbox.restore"),
            (_, Say::Review) => Key("inbox.mark_reviewed"),
            (_, Say::Start) => Key("inbox.start_lane"),
            (_, Say::Dismiss) => Key("inbox.dismiss"),
            (true, Say::AlwaysApprove) => Key("inbox.always_approve"),
        };
        SharedString::from(tr(key))
    }

    /// What the answer was, past tense: the toast's sentence and the Done list's.
    pub fn done_words(&self) -> SharedString {
        match (self.is_mark, self.say) {
            (true, Say::Approve) => SharedString::from(tr(Key("inbox.answer.marked_done"))),
            (true, Say::Decline) => SharedString::from(tr(Key("inbox.answer.not_now"))),
            _ => self.say.done_words(),
        }
    }
}

impl Say {
    /// What the answer was, past tense: the toast's sentence and the Done list's.
    pub fn done_words(self) -> SharedString {
        let key = match self {
            Say::Approve => Key("inbox.answer.approved"),
            Say::AlwaysApprove => Key("inbox.answer.always"),
            Say::Decline => Key("inbox.answer.declined"),
            Say::Resume => Key("inbox.answer.resumed"),
            Say::Stop => Key("inbox.answer.stopped"),
            Say::Close => Key("inbox.answer.closed"),
            Say::Restore => Key("inbox.answer.restored"),
            Say::Review => Key("inbox.answer.reviewed"),
            Say::Start => Key("inbox.answer.started"),
            Say::Dismiss => Key("inbox.answer.dismissed"),
        };
        SharedString::from(tr(key))
    }
}

pub(super) fn inbox_kind(kind: &str, is_triage: bool) -> &str {
    match kind {
        KIND_OWNER_ROW | KIND_AUTOMATION => KIND_APPROVAL,
        KIND_PROPOSAL if !is_triage => KIND_APPROVAL,
        other => other,
    }
}

pub(super) fn wait_kind(wait: &Wait) -> &str {
    inbox_kind(&wait.kind, Section::of(wait) == Section::Triage)
}

pub(super) fn history_kind(decided: &Decided) -> &str {
    if matches!(decided.answer.as_str(), "started" | "dismissed") {
        KIND_PROPOSAL
    } else {
        inbox_kind(&decided.kind, false)
    }
}

pub(super) fn inbox_kind_words(kind: &str) -> SharedString {
    match kind {
        KIND_PROPOSAL => tr(Key("inbox.kind.proposal")).into(),
        KIND_BLOCKED => tr(Key("inbox.kind.blocked")).into(),
        _ => kind_words(kind),
    }
}

pub(super) fn row_context(wait: &Wait, snapshot: &Snapshot) -> SharedString {
    if snapshot.is_project_folder_missing(&wait.repo) {
        return tr(Key("project.folder_missing")).into();
    }
    inbox_kind_words(wait_kind(wait))
}

pub(super) fn relative_age(since: Option<&str>, now: i64) -> Option<SharedString> {
    super::super::relative_age_words(since, now)
}

/// The localized label for an ask's kind.
pub fn kind_words(kind: &str) -> SharedString {
    let key = match kind {
        KIND_APPROVAL => Key("inbox.kind.approval"),
        KIND_OWNER_ROW => Key("inbox.kind.check"),
        KIND_QUESTION => Key("inbox.kind.question"),
        KIND_BLOCKED => Key("inbox.kind.stuck"),
        KIND_PROPOSAL => Key("inbox.kind.knowledge"),
        KIND_AUTOMATION => Key("inbox.kind.automation"),
        "review" => Key("inbox.kind.review"),
        _ => Key("inbox.kind.other"),
    };
    SharedString::from(tr(key))
}

/// The localized label for an answer the engine kept.
pub fn answer_words(decided: &Decided) -> SharedString {
    let key = match decided.answer.as_str() {
        "approve" | "accept" => Key("inbox.answer.approved"),
        "decline" | "reject" => Key("inbox.answer.declined"),
        "mark_ok" => Key("inbox.answer.marked_done"),
        "mark_ko" => Key("inbox.answer.not_now"),
        "reply" => Key("inbox.answer.replied"),
        "resume" => Key("inbox.answer.resumed"),
        "stop" => Key("inbox.answer.stopped"),
        "close" | "closed" => Key("inbox.answer.closed"),
        "reviewed" => Key("inbox.answer.reviewed"),
        "merged" => Key("inbox.answer.merged"),
        "started" => Key("inbox.answer.started"),
        "dismissed" => Key("inbox.answer.dismissed"),
        "choice" => {
            let choice = decided.choice.as_deref().unwrap_or_default();
            return tr_with(Key("inbox.answer.choice"), &[("choice", choice)]);
        }
        _ => Key("inbox.answer.other"),
    };
    SharedString::from(tr(key))
}

pub(super) fn resolved_by(decided: &Decided) -> Option<SharedString> {
    let namespaced = decided.actor.split_once(':');
    let (source, actor) = namespaced.unwrap_or((&decided.actor, &decided.actor));
    let place = decided.place.as_deref().unwrap_or(match source {
        "app" => "desktop",
        "overlay" => "phone",
        "github" => "github",
        "cli" => "cli",
        _ if namespaced.is_none() => "cli",
        _ => "",
    });
    let actor = if source == "overlay" {
        actor.split(':').next().unwrap_or(actor)
    } else {
        actor
    };
    let is_github = source == "github" && place == "github";
    let place = match place {
        "desktop" => tr(Key("inbox.place.desktop")),
        "phone" => tr(Key("inbox.place.phone")),
        "cli" => tr(Key("inbox.place.cli")),
        "github" => tr(Key("inbox.place.github")),
        _ => return Some(tr(Key("inbox.place.unknown")).into()),
    };
    if actor.is_empty() && decided.place.is_none() {
        None
    } else if actor.is_empty() || is_github {
        Some(place.to_string().into())
    } else {
        Some(format!("{actor}, {place}").into())
    }
}

/// When `since` was, by the person's clock ("21:40", "Yesterday 21:40", "Tue 21:40" or a date); none when the
/// engine gave no time.
pub fn clock_words(since: Option<&str>, now: i64) -> Option<SharedString> {
    let at = sakura_core::graph::parse_time(since?)?;
    let offset = crate::ui::engine_page::reset::local_offset(at);
    Some(clock_words_in(at, now, offset))
}

/// The same for a moment `now` and a zone `offset` seconds ahead of UTC: the day before is "Yesterday", which
/// is how a person says it, where the Engine page's clock names the weekday.
pub fn clock_words_in(at: i64, now: i64, offset: i64) -> SharedString {
    const SECONDS_PER_DAY: i64 = 86_400;
    let day = |moment: i64| (moment + offset).div_euclid(SECONDS_PER_DAY);
    match day(now) - day(at) {
        1 => {
            let clock = crate::ui::engine_page::reset::clock_words_in(at, at, offset);
            tr_with(Key("inbox.yesterday"), &[("clock", &clock)])
        }
        _ => SharedString::from(crate::ui::engine_page::reset::clock_words_in(
            at, now, offset,
        )),
    }
}

/// An ask's text without the numbering of a list of one item: "1. Words" is a paragraph, and a list of
/// several items stays a list.
pub fn plain_text(text: &str) -> String {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed.strip_prefix("1. ") else {
        return text.to_string();
    };
    let is_numbered = |line: &str| {
        let digits = line
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .count();
        digits > 0 && line.trim_start()[digits..].starts_with(". ")
    };
    if rest.lines().any(is_numbered) {
        return text.to_string();
    }
    rest.lines()
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a band says at its right: for the morning digest how many of its asks are decided; for any other lane
/// how many asks it holds, but only beside other bands, since the sidebar says the total.
pub fn band_trailing(
    group: &Group,
    asks: &[&Wait],
    snapshot: &Snapshot,
    has_several: bool,
) -> Option<SharedString> {
    let progress = lane_of(asks[0], snapshot).and_then(|lane| lane.progress.as_ref());
    match (group.date.is_some(), progress) {
        (true, Some(progress)) if progress.total > 0 => {
            let (done, total) = (progress.done.to_string(), progress.total.to_string());
            Some(tr_with(
                Key("inbox.decided"),
                &[("done", &done), ("total", &total)],
            ))
        }
        _ if has_several => Some(SharedString::from(asks.len().to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod eligibility_tests {
    use super::GROUP_CALLS;

    #[gpui::test]
    fn review_inbox_redraw_reuses_cached_eligibility(cx: &mut gpui::TestAppContext) {
        let (_rig, page, cx) =
            super::super::tests::open_on(cx, crate::ui::engine_rig_tests::SNAPSHOT, "{}", "{}");
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        GROUP_CALLS.with(|calls| calls.set(0));
        page.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(GROUP_CALLS.with(std::cell::Cell::get), 0);
    }
}
