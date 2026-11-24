//! What `sakura cockpit snapshot --json` says, read for the app. Only the fields the app shows are kept;
//! a document that is not what the engine documents is refused whole, never shown in part.

use super::accounts::{Account, Connector, Usage};
use super::automations::{Automation, Digest};
use super::state::EngineState;
use super::tracks::{ConversationKey, Track, TrackRef, Tracks};
use super::unreadable_answer;
use crate::error::{BridgeError, BridgeResult, ErrorCode};
use crate::said::Said;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::BTreeMap;

/// The version of the cockpit's document this reader knows.
pub const API_VERSION: u64 = 1;

/// The kind of item that is an engine flow task, which the person knows as a lane.
const FLOW_KIND: &str = "flow";
/// The kind of item that is a lane an outside agent opened.
const OUTSIDE_KIND: &str = "outside";
/// The kind of item that is a day's morning digest: a report, not a project and not a lane the person started.
const DIGEST_KIND: &str = "digest";
/// The kinds of item the app shows.
const SHOWN_KINDS: [&str; 3] = [FLOW_KIND, OUTSIDE_KIND, DIGEST_KIND];
/// What the engine puts before an outside agent's name in an author.
const OUTSIDE_AUTHOR: &str = "outside:";
/// Where an outside agent's wait comes from.
const OUTSIDE_SOURCE: &str = "outside";

/// One control the engine offers on a wait or an item, in the engine's own shape.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Action {
    pub key: String,
    pub label: String,
    /// The inputs that must be sent.
    #[serde(default)]
    pub needs: Vec<String>,
    /// The inputs that may be sent.
    #[serde(default)]
    pub optional: Vec<String>,
    /// Whether the first call only asks, and a second one with its token runs it.
    #[serde(default)]
    pub confirm: bool,
}

/// Engine-owned grouping for an Inbox wait.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WaitGroup {
    pub id: String,
    pub label: String,
}

/// Defaults shared by the desktop and phone when starting a proposal.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WaitProposal {
    pub op: String,
    pub project: Option<String>,
    pub preset: Option<String>,
    pub account: Option<String>,
    pub title: String,
    pub origin: Option<ScheduledOrigin>,
    pub run_id: Option<String>,
}

/// Copied provenance that remains available after the run expires.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ScheduledOrigin {
    pub task_id: String,
    pub title: String,
    pub at: Option<String>,
    pub source: Option<String>,
}

/// One point a lane waits on from the person.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Wait {
    pub track: Option<TrackRef>,
    /// Stable across snapshots; the target of an action.
    pub id: String,
    /// `["wait", repo, task, kind, name]`.
    #[serde(default)]
    pub key: Vec<String>,
    /// The Inbox kind. Older engines send the original wait kind.
    pub kind: String,
    #[serde(default)]
    pub subkind: Option<String>,
    #[serde(default)]
    pub group: Option<WaitGroup>,
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default)]
    pub is_read: bool,
    #[serde(default)]
    pub is_snoozed: Option<bool>,
    #[serde(default)]
    pub snoozed_until: Option<String>,
    #[serde(default)]
    pub proposal: Option<WaitProposal>,
    pub repo: String,
    pub project: String,
    pub task: String,
    /// The title of the lane the ask belongs to; empty when the lane has none, the ask is not a lane's or
    /// the engine does not print it.
    #[serde(default)]
    pub lane_title: String,
    pub title: String,
    #[serde(default)]
    pub detail: String,
    /// The engine's own line when `detail` is a plain sentence made from it (a failed lookup, a command, a path
    /// of this Mac), shown on request; empty when `detail` is the line itself.
    #[serde(default)]
    pub technical: String,
    /// What the reviewer of a lane requested, one plain line each, on the ask to deliver although the review
    /// still wants changes; empty from an engine that does not give them.
    #[serde(default)]
    pub findings: Vec<String>,
    #[serde(default)]
    pub wants_note: bool,
    pub since: Option<String>,
    /// `flow` for the engine's own waits, `outside` for an outside agent's.
    #[serde(default)]
    pub source: String,
    /// An outside wait: `outside:<agent name>`; empty for the engine's own.
    #[serde(default)]
    pub author: String,
    /// An outside question: the answers offered to press; empty otherwise.
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
    /// What the ask points at, each with a title: an issue, a pull request. Empty from an engine that gives
    /// none (its addresses are then only in the ask's text).
    #[serde(default)]
    pub links: Vec<Link>,
    /// An ask of a morning digest: the day of the digest (`YYYY-MM-DD`); none for every other ask.
    #[serde(default)]
    pub digest: Option<String>,
}

impl Wait {
    /// The wake deadline as Unix seconds, if the engine supplied a valid timestamp.
    pub fn wake_at(&self) -> Option<i64> {
        super::moment::iso_seconds(self.snoozed_until.as_deref()?)
    }

    /// An explicit false wakes the wait early. Older engines report only a deadline.
    pub fn is_snoozed_at(&self, now: i64) -> bool {
        match (self.is_snoozed, self.wake_at()) {
            (Some(false), _) => false,
            (_, Some(wake)) => now < wake,
            (flag, None) => flag == Some(true),
        }
    }

    /// The engine kind that owns this wait's original actions.
    pub fn original_kind(&self) -> &str {
        self.subkind.as_deref().unwrap_or(&self.kind)
    }

    /// Whether an outside agent wrote this wait.
    pub fn is_outside(&self) -> bool {
        self.source == OUTSIDE_SOURCE
    }

    /// Whether this ask belongs to a morning digest, which is no project's.
    pub fn is_digest(&self) -> bool {
        self.digest.is_some()
    }

    /// The outside agent's name, as the "From" line says it; empty for an engine wait.
    pub fn agent(&self) -> &str {
        agent_of(&self.author)
    }

    /// What the lane of this wait is called: its title when the engine prints one, else its id.
    pub fn lane_name(&self) -> &str {
        match !self.lane_title.is_empty() {
            true => &self.lane_title,
            false => &self.task,
        }
    }

    /// The key of the lane this wait is about: the same string as that lane's [`Item::lane_key`].
    pub fn lane_key(&self) -> String {
        let repo = if self.is_digest() { "" } else { &self.repo };
        lane_key_of(self.is_outside(), repo, &self.task)
    }

    /// What the wait is about inside its task: the gate of an approval, the row of an owner row. The engine
    /// binds some of them to a revision, `overlap@<rev>`; the name is what is before the `@`.
    pub fn key_name(&self) -> &str {
        let named = self.key.get(4).map_or("", String::as_str);
        named.split_once('@').map_or(named, |(name, _)| name)
    }

    /// The revision the engine bound the wait to, `<rev>` of `overlap@<rev>`; empty when it bound none. The
    /// app never reads one: the wait's id carries it back to the engine whole.
    pub fn key_revision(&self) -> &str {
        let named = self.key.get(4).map_or("", String::as_str);
        named.split_once('@').map_or("", |(_, revision)| revision)
    }
}

/// What the engine computed of a lane's plan: done over total, and the phase it is in. A total of zero
/// means there is no plan or contract yet.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Progress {
    pub done: u32,
    pub total: u32,
    /// The phase for a flow lane is the step the lane is at in words (`Build`), counted within that step; the
    /// engine's own phase is `engine_phase`. An outside lane's progress has no `engine_phase`.
    pub phase: String,
    #[serde(default)]
    pub engine_phase: Option<String>,
}

impl Progress {
    /// The engine's own name of the phase (`build`, `evaluate`), which the app says in its own words.
    pub fn phase_id(&self) -> &str {
        self.engine_phase.as_deref().unwrap_or(&self.phase)
    }
}

/// A link an outside agent gave its lane, opened in the default browser.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Link {
    pub label: String,
    pub url: String,
}

/// What a morning digest's page reads: its news and its asks, as the engine reads them from the day's file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Reading {
    /// The digest's day, `YYYY-MM-DD`.
    pub date: String,
    /// The digest's text.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub news: Vec<ReadingNews>,
    #[serde(default)]
    pub asks: Vec<ReadingAsk>,
}

/// One news entry of a morning digest. It has no read state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReadingNews {
    pub title: String,
    #[serde(default)]
    pub text: String,
    /// The id of the digest source it came from.
    #[serde(default)]
    pub source: String,
    /// Titled links; none once the day's file is gone.
    #[serde(default)]
    pub links: Vec<Link>,
}

/// One check or decision a morning digest raised.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReadingAsk {
    /// `owner_row` or `question`.
    pub kind: String,
    pub title: String,
    /// What it says, without its links.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub links: Vec<Link>,
    /// The answers offered to press; empty for a check.
    #[serde(default)]
    pub choices: Vec<String>,
    /// `open` or `decided`.
    pub state: String,
    /// While it is open, the id of its wait in `waits`; none once decided.
    #[serde(rename = "id", default)]
    pub wait: Option<String>,
    /// Once decided: the verdict (`ok` or `ko`), the choice or the text the person gave.
    #[serde(default)]
    pub answer: Option<String>,
}

impl ReadingAsk {
    /// Whether the ask still waits for the person.
    pub fn is_open(&self) -> bool {
        self.state == "open"
    }
}

/// What a lane delivered: a pull request, a pushed branch or a local result, and the head it delivered.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Delivery {
    /// `pull_request`, `push` or `local`.
    pub kind: String,
    pub head: String,
    #[serde(default)]
    pub branch: String,
    /// The pull request's address; empty for the others.
    #[serde(default)]
    pub url: String,
}

/// One step of a lane's pipeline, as the engine reports it: its id (`plan`, `build`, `checks`, `deliver`), its
/// name, and where the lane stands on it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LaneStep {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// `done`, `now`, `next` or `skipped`.
    pub state: String,
    /// The model the step runs on, and the account when the person has several.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    /// The account's name in words, when the engine gives it.
    #[serde(default)]
    pub account_label: Option<String>,
}

/// One step of a lane's whole pipeline, as the engine lists it: where the lane stands on it, whether the lane
/// runs it, and whether the person may turn it on or off now, with the reason when not.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PipelineStep {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// `done`, `running`, `next` or `off`.
    pub state: String,
    /// Whether the lane runs the step.
    #[serde(rename = "on", default)]
    pub is_on: bool,
    #[serde(default)]
    pub can_toggle: bool,
    /// Why the step cannot be turned on or off now, in plain words; empty when it can.
    #[serde(default)]
    pub reason: String,
    /// For a step in the state `waiting` (the lane needs the person there), what it waits for, in words.
    #[serde(default)]
    pub note: String,
}

/// The step a lane is in now: the one source for a header and a step bar.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CurrentStep {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// The lane waits for the person in this step.
    #[serde(default)]
    pub waiting: bool,
    #[serde(default)]
    pub note: String,
}

/// One review of a lane's pipeline: `plan_review`, `task_review` or `final_review`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LaneReview {
    pub id: String,
    #[serde(rename = "on", default)]
    pub is_on: bool,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub account_label: Option<String>,
    #[serde(default)]
    pub state: String,
}

/// One lane: an engine flow task, or a lane an outside agent opened.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Item {
    pub track: Option<TrackRef>,
    pub id: String,
    pub key: Vec<String>,
    /// `flow`, `outside` or `digest`.
    #[serde(default)]
    pub kind: String,
    /// The folder of the lane; empty for a morning digest, which has none.
    #[serde(deserialize_with = "none_when_null")]
    pub repo: String,
    #[serde(default)]
    pub branch: String,
    /// `gate`, `blocked`, `working` or `waiting`.
    pub agent_state: String,
    #[serde(default)]
    pub pr: String,
    /// Role to model.
    #[serde(default)]
    pub models: BTreeMap<String, String>,
    #[serde(default)]
    pub pending: Vec<String>,
    pub since: Option<String>,
    pub last_activity_at: Option<String>,
    #[serde(default)]
    pub next_command: String,
    pub progress: Option<Progress>,
    /// What the lane is called: an outside agent's own words for it, a flow task's title when the engine
    /// prints one; empty when it prints none.
    #[serde(default)]
    pub title: String,
    /// An outside lane: `outside:<agent name>`; empty for a flow task.
    #[serde(default)]
    pub author: String,
    /// An outside lane: up to four links, each an `https://` address.
    #[serde(default)]
    pub links: Vec<Link>,
    /// An outside lane: how many of the person's answers its agent has not read yet.
    #[serde(default)]
    pub unread: u32,
    /// What a flow lane delivered, when it did: how, and where.
    #[serde(default)]
    pub delivery: Option<Delivery>,
    /// What the lane was asked to do, as its opener wrote it; none from an engine that does not report it.
    #[serde(default)]
    pub goal: Option<String>,
    /// Who opened the lane: `master`, `person` or `outside`.
    #[serde(default)]
    pub opened_by: Option<String>,
    /// The folder the lane's agents work in.
    #[serde(default)]
    pub worktree: Option<String>,
    /// The lane's steps in order; empty from an engine that does not report them, and for an outside lane.
    #[serde(default)]
    pub steps: Vec<LaneStep>,
    #[serde(default)]
    pub reviews: Vec<LaneReview>,
    /// Every step of the lane's pipeline in the order it runs; empty from an engine that does not list them, and
    /// for an outside lane.
    #[serde(default)]
    pub pipeline: Vec<PipelineStep>,
    /// The step the lane is in now; none from an engine that does not say, and for an outside lane.
    #[serde(default)]
    pub current_step: Option<CurrentStep>,
    /// Whether a reviewer of the lane runs now.
    #[serde(default)]
    pub in_review: bool,
    #[serde(default)]
    pub actions: Vec<Action>,
    /// A morning digest: its day, `YYYY-MM-DD`; none for any other item.
    #[serde(default)]
    pub date: Option<String>,
    /// A morning digest: what its page reads; none for any other item.
    #[serde(default)]
    pub reading: Option<Reading>,
    /// The preset the lane runs under, when the engine reports it.
    #[serde(default)]
    pub preset: Option<String>,
    /// The preset in words (`Careful`), when the engine gives them.
    #[serde(default)]
    pub preset_label: Option<String>,
    #[serde(default)]
    pub pace: Option<String>,
    /// Whether the pace the person chose waits for the lane's next step: `pace` already says the chosen one.
    /// False from an engine that does not say.
    #[serde(default)]
    pub pace_pending: bool,
}

/// `name` with a capital: the engine names presets, paces and plans in lower case.
pub fn capitalised(name: &str) -> String {
    let mut letters = name.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

impl Item {
    /// Use the current step to reconcile pipeline entries left behind by a phase change.
    pub fn current_pipeline(&self) -> Cow<'_, [PipelineStep]> {
        let reported = self.current_pipeline_index();
        let active = self.blocked_pipeline_index().or(reported).or_else(|| {
            self.pipeline.iter().rposition(|step| {
                step.is_on && matches!(step.state.as_str(), "running" | "waiting")
            })
        });
        let mut displayed = Cow::Borrowed(self.pipeline.as_slice());
        for (index, step) in self.pipeline.iter().enumerate() {
            let current = self
                .current_step
                .as_ref()
                .filter(|_| reported == Some(index));
            let state = match (active == Some(index), step.state.as_str()) {
                (true, _) if current.is_some_and(|current| current.waiting) => "waiting",
                (true, "waiting") => "waiting",
                (true, _) => "running",
                (false, "running" | "waiting") if active.is_some_and(|active| index < active) => {
                    "done"
                }
                (false, "running" | "waiting") if active.is_some() => "next",
                _ => &step.state,
            };
            let note = current.filter(|current| {
                state == "waiting" && step.note.is_empty() && !current.note.is_empty()
            });
            if state != step.state || note.is_some() {
                let shown = &mut displayed.to_mut()[index];
                state.clone_into(&mut shown.state);
                if let Some(current) = note {
                    shown.note.clone_from(&current.note);
                }
            }
        }
        displayed
    }

    fn blocked_pipeline_index(&self) -> Option<usize> {
        let reported_waiting = self.current_step.as_ref().is_some_and(|step| step.waiting);
        let is_blocked = matches!(
            self.agent_state.as_str(),
            "waiting" | "stuck" | "stopped" | "failed"
        );
        if reported_waiting || !is_blocked {
            return None;
        }
        // A blocked lane can still report the previously running step as current.
        self.pipeline
            .iter()
            .rposition(|step| step.is_on && step.state == "waiting" && !step.note.is_empty())
    }

    fn current_pipeline_index(&self) -> Option<usize> {
        let find = |id| {
            self.pipeline
                .iter()
                .position(|step| step.is_on && step.id == id)
        };
        if let Some(step) = &self.current_step {
            return find(step.id.as_str())
                .or_else(|| (step.id == "contract").then(|| find("plan")).flatten());
        }
        let phase = self.progress.as_ref().map(Progress::phase_id);
        if self.agent_state == "working" && phase == Some("contract") {
            return find("plan");
        }
        None
    }

    /// The lane as it is when its plan review was never run: the plan review step and review are off. The engine
    /// lists a plan review as done when only the arbiter ran under the plan reviewer's role (see
    /// [`super::agents::is_plan_review_only_the_arbiters`]).
    pub fn without_plan_review(&self) -> Cow<'_, Item> {
        let has_plan_review = self
            .pipeline
            .iter()
            .any(|step| step.id == "plan_review" && (step.is_on || step.state != "off"))
            || self.reviews.iter().any(|review| {
                review.id == "plan_review" && (review.is_on || review.state != "skipped")
            });
        if !has_plan_review {
            return Cow::Borrowed(self);
        }
        let mut corrected = self.clone();
        for step in corrected
            .pipeline
            .iter_mut()
            .filter(|step| step.id == "plan_review")
        {
            step.is_on = false;
            if step.state != "off" {
                step.reason.clear();
                step.note.clear();
                step.state = "off".to_string();
            }
        }
        for review in corrected
            .reviews
            .iter_mut()
            .filter(|review| review.id == "plan_review")
        {
            review.is_on = false;
            review.state = "skipped".to_string();
        }
        Cow::Owned(corrected)
    }

    /// The preset the lane runs under in words: the engine's label for it, else its id with a capital.
    pub fn preset_name(&self) -> Option<String> {
        let preset = self.preset.as_deref()?;
        Some(
            self.preset_label
                .clone()
                .unwrap_or_else(|| capitalised(preset)),
        )
    }

    /// Whether an outside agent opened this lane: a morning digest is one, written by the agent `digest`.
    pub fn is_outside(&self) -> bool {
        self.kind == OUTSIDE_KIND || self.is_digest()
    }

    /// Whether this is a day's morning digest: not a project and not a lane the person started, so the lists of
    /// projects and lanes leave it out and its own reading page shows it.
    pub fn is_digest(&self) -> bool {
        self.kind == DIGEST_KIND
    }

    /// The lane's id: a flow task's id inside its repository, an outside lane's (or a digest's) own id.
    pub fn task(&self) -> &str {
        let at = if self.is_outside() { 1 } else { 2 };
        self.key.get(at).map_or("", String::as_str)
    }

    /// What names the lane in the window: its kind, its repository and its id together, so that two lanes
    /// with one id (in two projects, or one a flow task and one an outside lane) are never the same lane.
    pub fn lane_key(&self) -> String {
        lane_key_of(self.is_outside(), &self.repo, self.task())
    }

    /// What the lane is called: its title when the engine prints one, else its id.
    pub fn name(&self) -> &str {
        match !self.title.is_empty() {
            true => &self.title,
            false => self.task(),
        }
    }

    /// The outside agent's name, as the "From" line says it; empty for a flow task.
    pub fn agent(&self) -> &str {
        agent_of(&self.author)
    }

    /// The project: the name of the repository's folder.
    pub fn project(&self) -> &str {
        self.repo
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or("")
    }
}

/// The key of the action that starts a lane in a project.
const START_LANE_KEY: &str = "L";

/// One registered project: a Git repository the engine knows, with or without lanes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Project {
    #[serde(default)]
    pub track_choices: Vec<TrackRef>,
    /// Stable across snapshots; the target of an action.
    pub id: String,
    pub name: String,
    /// The repository's main worktree: the `repo` of every lane of the project.
    pub folder: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub folder_missing: bool,
    /// How many of its lanes are open.
    #[serde(default)]
    pub open_lanes: u32,
    #[serde(default)]
    pub actions: Vec<Action>,
}

impl Project {
    /// The action that starts a lane in the project, when the engine offers it.
    pub fn start_lane(&self) -> Option<&Action> {
        if self.folder_missing {
            return None;
        }
        self.actions
            .iter()
            .find(|action| action.key == START_LANE_KEY)
    }
}

/// One line of the engine's recent events.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Event {
    pub source: String,
    pub scope: String,
    pub at: Option<String>,
    pub text: String,
}

/// One answer the person gave to an ask, as the engine keeps it for seven days.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Decided {
    /// The id the ask had as a wait.
    pub id: String,
    /// `["wait", repo, task, kind, name]`, so the row can be matched to its lane.
    #[serde(default)]
    pub key: Vec<String>,
    /// The kind of the ask: `approval`, `owner_row`, `question`, `proposal` or `blocked`.
    #[serde(default)]
    pub kind: String,
    pub project: String,
    pub task: String,
    /// What the ask said; empty when the engine could no longer read it.
    #[serde(default)]
    pub title: String,
    /// `approve`, `decline`, `reply`, `choice`, `resume`, `stop`, `accept`, `reject`, `mark_ok` or `mark_ko`.
    pub answer: String,
    pub choice: Option<String>,
    pub text: Option<String>,
    /// Who answered, as the action log writes it.
    #[serde(default)]
    pub actor: String,
    #[serde(default)]
    pub place: Option<String>,
    #[serde(default)]
    pub lane: Option<String>,
    pub at: Option<String>,
}

/// What the engine shows now: the waits across projects, oldest first, the lanes and the latest events.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub tracks: Tracks,
    pub waits: Vec<Wait>,
    pub items: Vec<Item>,
    pub events: Vec<Event>,
    /// Every registered project, also one with no lane, in the order the engine lists them.
    pub projects: Vec<Project>,
    /// Why part of the picture is missing, as the engine says it.
    pub problems: Vec<String>,
    /// The engine's pace (`slowest`, `slow`, `normal`, `fast` or `fastest`); none from an engine that does
    /// not report it.
    pub pace: Option<String>,
    /// The asks the person answered lately, newest first; none from an engine that does not report them,
    /// which is not the same as an engine that reports that nothing was answered.
    pub decided: Option<Vec<Decided>>,
    /// The accounts the engine spends; none from an engine that lists none.
    pub accounts: Vec<Account>,
    /// The places the engine reads for the person; none from an engine that does not report them.
    pub connectors: Option<Vec<Connector>>,
    pub connector_controls: Option<super::connectors::ConnectorControls>,
    pub connectors_problem: Option<String>,
    /// What the engine says of itself; none from an engine that says nothing.
    pub engine: Option<EngineState>,
    /// The services and scheduled automations the engine lists; none from an engine that lists none.
    pub automations: Option<Vec<Automation>>,
    /// When the usage readings were taken; none from an engine that does not say.
    pub usage: Option<Usage>,
    /// The morning digest's settings and sources; none from an engine that says nothing of them.
    pub digest: Option<Digest>,
    pub scheduled: Option<super::scheduled::Scheduled>,
}

impl Snapshot {
    pub fn track(&self, id: &str) -> Option<&Track> {
        self.tracks.rows().iter().find(|track| track.id == id)
    }

    pub fn track_of_conversation(&self, key: &ConversationKey) -> Option<&Track> {
        self.tracks
            .rows()
            .iter()
            .find(|track| track.conversations.iter().any(|member| &member.key == key))
    }

    /// Whether the engine reports that the registered repository at `folder` is missing.
    pub fn is_project_folder_missing(&self, folder: &str) -> bool {
        self.projects
            .iter()
            .any(|project| project.folder == folder && project.folder_missing)
    }

    /// Waits currently asking for the person's attention.
    pub fn active_waits(&self, now: i64) -> impl Iterator<Item = &Wait> {
        self.waits
            .iter()
            .filter(move |wait| !wait.is_snoozed_at(now))
    }

    /// Waits belonging to the lane, including snoozed waits.
    pub fn waits_for(&self, item: &Item) -> impl Iterator<Item = &Wait> {
        let key = item.lane_key();
        self.waits.iter().filter(move |wait| wait.lane_key() == key)
    }

    /// Reads the engine's document. It refuses one that is not JSON, that names no version or another
    /// version than [`API_VERSION`], or whose parts are not what is documented.
    pub fn parse(bytes: &[u8]) -> BridgeResult<Snapshot> {
        let document: Value = serde_json::from_slice(bytes).map_err(|_| unreadable_answer())?;
        let version = document.get("api").and_then(Value::as_u64);
        match version {
            Some(API_VERSION) => {}
            Some(found) => return Err(other_version(found)),
            None => return Err(unreadable_answer()),
        }
        Ok(Snapshot {
            tracks: Tracks::parse(&document)?,
            waits: part(&document, "waits")?,
            items: items(&document)?,
            events: part(&document, "events")?,
            projects: part(&document, "projects")?,
            problems: part(&document, "problems")?,
            pace: pace(&document),
            decided: decided(&document)?,
            accounts: part(&document, "accounts")?,
            connectors: optional_part(&document, "connectors")?,
            connector_controls: optional_object(&document, "connector_controls")?,
            connectors_problem: optional_object(&document, "connectors_problem")?,
            engine: engine_state(&document)?,
            automations: automations(&document)?,
            usage: optional_object(&document, "usage")?,
            digest: optional_object(&document, "digest")?,
            scheduled: optional_object(&document, "scheduled")?,
        })
    }
}

/// The decided asks of the document, none when the engine has no such part.
fn decided(document: &Value) -> BridgeResult<Option<Vec<Decided>>> {
    match document.get("decided") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => Vec::<Decided>::deserialize(value)
            .map(Some)
            .map_err(|_| unreadable_answer()),
    }
}

/// The pace in the document's `engine` part, when it has one.
fn pace(document: &Value) -> Option<String> {
    let pace = document.get("engine")?.get("pace")?.as_str()?;
    Some(pace.to_string())
}

/// The lanes of the document: its items of kind `flow` or `outside`. Any other kind (a manual lane) is not
/// shown and is not read, so its shape cannot break the page.
fn items(document: &Value) -> BridgeResult<Vec<Item>> {
    let listed = document.get("items").and_then(Value::as_array);
    let shown = listed.into_iter().flatten().filter(|item| {
        SHOWN_KINDS.contains(&item.get("kind").and_then(Value::as_str).unwrap_or(""))
    });
    shown
        .map(|item| Item::deserialize(item).map_err(|_| unreadable_answer()))
        .collect()
}

/// The key of a lane from its kind, repository and id.
fn lane_key_of(is_outside: bool, repo: &str, task: &str) -> String {
    let kind = if is_outside { OUTSIDE_KIND } else { FLOW_KIND };
    serde_json::json!([kind, repo, task]).to_string()
}

/// The agent's name in an author such as `outside:claude`.
fn agent_of(author: &str) -> &str {
    author.strip_prefix(OUTSIDE_AUTHOR).unwrap_or("")
}

/// A list of the document's, empty when the engine does not have it (the fields are new ones).
fn part<T: for<'de> Deserialize<'de>>(document: &Value, name: &str) -> BridgeResult<Vec<T>> {
    match document.get(name) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => Vec::<T>::deserialize(value).map_err(|_| unreadable_answer()),
    }
}

/// The document's `engine` part, none when the engine does not say.
fn engine_state(document: &Value) -> BridgeResult<Option<EngineState>> {
    match document.get("engine") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => EngineState::deserialize(value)
            .map(Some)
            .map_err(|_| unreadable_answer()),
    }
}

/// The `items` of the document's `automations` envelope (`{items, target, actions}`), none when the engine does
/// not have it.
fn automations(document: &Value) -> BridgeResult<Option<Vec<Automation>>> {
    match document.get("automations") {
        None | Some(Value::Null) => Ok(None),
        Some(envelope) => optional_part(envelope, "items"),
    }
}

/// An object of the document's, none when the engine does not have it.
fn optional_object<T: for<'de> Deserialize<'de>>(
    document: &Value,
    name: &str,
) -> BridgeResult<Option<T>> {
    match document.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => T::deserialize(value)
            .map(Some)
            .map_err(|_| unreadable_answer()),
    }
}

/// A list of the document's, none when the engine does not have it.
fn optional_part<T: for<'de> Deserialize<'de>>(
    document: &Value,
    name: &str,
) -> BridgeResult<Option<Vec<T>>> {
    match document.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => Vec::<T>::deserialize(value)
            .map(Some)
            .map_err(|_| unreadable_answer()),
    }
}

/// Reads null as empty text while preserving the engine's strings.
pub(super) fn none_when_null<'de, D: Deserializer<'de>>(reader: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(reader)?.unwrap_or_default())
}

fn other_version(found: u64) -> BridgeError {
    let said = Said::new("error.engine.api")
        .number("found", found)
        .number("expected", API_VERSION);
    BridgeError::new(ErrorCode::EngineFailed, said)
}
