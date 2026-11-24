//! The agents of a lane as the lane page lists them: one card per agent, from the engine's runs. A run that
//! resumes another is the same agent, so it folds into the first run of its chain; the rounds of one agent (a
//! builder's fix rounds, each a run of its own) fold into one card as well. The card's state and what it does
//! now are the newest round's, its conversation is every round in order.

use super::agent_words::{first_line, model_words, state_glyph, state_words, tool_label};
use super::kit::{AgentRowFacts, DoingLine};
use crate::data::engine_facts::EngineFacts;
use crate::data::source::Source;
use crate::l10n::{tr, tr_count, tr_with, Key};
use crate::theme::{self, lane_page as sizes, Palette, Theme};
use crate::ui::chrome::text;
use crate::ui::unix_now as now;
use gpui::{
    div, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
};
use sakura_core::engine::agents::{AgentRun, Doing};
use sakura_core::engine::cli::{LANE_FOLDER_MISSING, PROJECT_FOLDER_MISSING};
use sakura_core::engine::snapshot::Item;
use sakura_core::error::BridgeError;
use sakura_core::said::{Said, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// The role of the runs that write the spec, the contract and the plan.
const PLANNER: &str = "planner";

/// What the list says when the lane has no run yet.
const NONE_YET: Key = Key("agents.none");

/// One agent of a lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCard {
    /// The first run of the agent's first round: the agent is known by this name.
    pub origin: String,
    /// When the first run of the first round started.
    pub started_at: String,
    /// The run that is the agent now: the last of its newest round. It has the state, what is being done and
    /// how a message reaches the agent.
    pub tail: AgentRun,
    /// The rounds of the agent, in the order they started: its conversation is read under their names, one
    /// after the other.
    pub rounds: Vec<Round>,
    /// How many times the newest round was resumed after it finished.
    pub resumed: usize,
    /// The title of the task the agent works on, when another agent shares its role: two builders on two tasks
    /// are told apart by it.
    pub task: Option<String>,
}

/// One round of an agent: the first run of it, and the number the engine gave it (counted from one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Round {
    pub origin: String,
    pub number: usize,
}

/// The agents of `runs`: the roles in the order they first started, and within a role the newest agent first.
pub fn cards_of(runs: &[AgentRun]) -> Vec<AgentCard> {
    let mut cards: Vec<AgentCard> = agents_of(&chains_of(runs))
        .iter()
        .map(|rounds| card_of(rounds))
        .collect();
    let role_start: HashMap<String, String> =
        cards.iter().fold(HashMap::new(), |mut found, card| {
            let start = found
                .entry(role_of(&card.tail).to_string())
                .or_insert(card.started_at.clone());
            *start = (*start).clone().min(card.started_at.clone());
            found
        });
    cards.sort_by(|a, b| {
        let by_role = |card: &AgentCard| {
            (
                role_start[role_of(&card.tail)].clone(),
                role_of(&card.tail).to_string(),
            )
        };
        by_role(a)
            .cmp(&by_role(b))
            .then(b.started_at.cmp(&a.started_at))
    });
    name_tasks(&mut cards);
    cards
}

/// The runs grouped into chains: a run that resumes another joins that run's chain.
fn chains_of(runs: &[AgentRun]) -> Vec<Vec<&AgentRun>> {
    let mut chains: Vec<Vec<&AgentRun>> = Vec::new();
    let mut place: HashMap<&str, usize> = HashMap::new();
    for run in runs {
        let at = run.resumes.as_deref().and_then(|origin| place.get(origin));
        let at = match at {
            Some(at) => {
                chains[*at].push(run);
                *at
            }
            None => {
                chains.push(vec![run]);
                chains.len() - 1
            }
        };
        place.insert(run.name.as_str(), at);
    }
    chains
}

/// The chains grouped into agents: those of one `agent` the engine names are the rounds of one agent, in the
/// order they started. A run of an engine that names no agent is an agent of its own, with its resumes.
fn agents_of<'a>(chains: &'a [Vec<&'a AgentRun>]) -> Vec<Vec<&'a [&'a AgentRun]>> {
    let mut agents: Vec<Vec<&[&AgentRun]>> = Vec::new();
    let mut place: HashMap<&str, usize> = HashMap::new();
    for chain in chains {
        let key = chain[0].agent.as_str();
        let at = match place.get(key).filter(|_| !key.is_empty()) {
            Some(at) => *at,
            None => {
                agents.push(Vec::new());
                place.insert(key, agents.len() - 1);
                agents.len() - 1
            }
        };
        agents[at].push(chain);
    }
    for rounds in &mut agents {
        rounds.sort_by(|a, b| a[0].started_at.cmp(&b[0].started_at));
    }
    agents
}

fn card_of(rounds: &[&[&AgentRun]]) -> AgentCard {
    let (first, newest) = (rounds[0], rounds[rounds.len() - 1]);
    let tail = newest.last().expect("a chain has a run");
    let numbered = rounds.iter().enumerate().map(|(at, chain)| Round {
        origin: chain[0].name.clone(),
        number: match chain[0].round {
            0 => at + 1,
            number => number as usize,
        },
    });
    AgentCard {
        origin: first[0].name.clone(),
        started_at: first[0].started_at.clone(),
        tail: (*tail).clone(),
        rounds: numbered.collect(),
        resumed: newest.len() - 1,
        task: None,
    }
}

/// The role of `run` as a person names it, as Details names the steps: a plan review is a plan reviewer and
/// any other review a code reviewer, whatever word the engine gave the role.
pub(super) fn role_of(run: &AgentRun) -> &str {
    // The arbiter runs under the plan reviewer's role and the engine says it serves the plan review: it is an agent
    // of its own.
    if run.phase == "arbiter" {
        return "arbiter";
    }
    match (run.step_or_phase(), run.role.as_str()) {
        ("plan_review", _) => "plan_reviewer",
        (_, "reviewer") => "code_reviewer",
        (_, role) => role,
    }
}

/// The lane as the page says it: with no plan review when only the arbiter ran under the plan reviewer's role (see
/// [`sakura_core::engine::agents::is_plan_review_only_the_arbiters`]), `item` itself otherwise.
pub(super) fn as_run(item: &Item, is_plan_review_the_arbiters: bool) -> std::borrow::Cow<'_, Item> {
    match is_plan_review_the_arbiters {
        true => item.without_plan_review(),
        false => std::borrow::Cow::Borrowed(item),
    }
}

/// Gives the agents that share a role the title of the task each works on, so that two of them read as two.
fn name_tasks(cards: &mut [AgentCard]) {
    let sharing = |cards: &[AgentCard], role: &str| {
        cards
            .iter()
            .filter(|card| role_of(&card.tail) == role)
            .count()
    };
    let shared: Vec<bool> = cards
        .iter()
        .map(|card| sharing(cards, role_of(&card.tail)) > 1)
        .collect();
    for (card, is_shared) in cards.iter_mut().zip(shared) {
        let title = card.tail.task_title.trim();
        let titled = (is_shared && !title.is_empty()).then(|| title.to_string());
        card.task = fix_words(&card.tail.agent).or(titled);
    }
}

/// What a builder of a fix round fixes, from the engine's agent key (`build:build:final-1`: the step, the phase
/// and the task, which a fix round names after what it fixes and its round): the findings of the final review,
/// or what the checks found. A fix round serves no task of the plan, so it has no title of its own.
fn fix_words(agent: &str) -> Option<String> {
    let task = agent.rsplit(':').next()?;
    let (kind, round) = task.split_once('-')?;
    let key = match kind {
        "final" => Key("agents.fix.final"),
        "fix" => Key("agents.fix.checks"),
        _ => return None,
    };
    round.parse::<u32>().ok()?;
    Some(tr_with(key, &[("round", round)]).to_string())
}

/// What the row of `card` says at the moment `now` (Unix seconds): the elapsed time of a running tool is told
/// from it, so a clock that ticks shows the time move.
pub fn facts_of(card: &AgentCard, now: i64) -> AgentRowFacts {
    let run = &card.tail;
    AgentRowFacts {
        role: super::lanes::role_words(role_of(run)),
        phase: phase_words(run),
        detail: detail_words(run),
        task: card
            .task
            .as_deref()
            .map(|title| super::thread::shown(title).into()),
        rounds: rounds_words(
            card.rounds.last().map_or(1, |round| round.number),
            card.rounds.len(),
            card.resumed,
        ),
        state: state_words(run.state),
        glyph: state_glyph(run.state),
        doing: run.doing.as_ref().map(|doing| doing_line(doing, now)),
    }
}

/// Which step of the plan a planner's run is, in words (Intake, Contract, Plan): a lane has one planner row for
/// each and they read alike otherwise. No other role has a phase worth saying beside its name.
fn phase_words(run: &AgentRun) -> Option<SharedString> {
    (role_of(run) == PLANNER).then(|| planner_phase_words(&run.phase))
}

/// The words of a planner session's own phase: the spec (intake), the contract, the plan.
fn planner_phase_words(phase: &str) -> SharedString {
    let key = match phase {
        "intake" => Key("agents.phase.intake"),
        "contract" => Key("agents.phase.contract"),
        _ => Key("agents.phase.plan"),
    };
    tr(key).into()
}

fn detail_words(run: &AgentRun) -> SharedString {
    let model = model_words(&run.provider, &run.model);
    match run.account.as_deref().filter(|account| !account.is_empty()) {
        Some(account) => tr_with(
            Key("agents.detail"),
            &[
                ("model", &model),
                ("account", &super::thread::shown(account)),
            ],
        ),
        None => model,
    }
}

/// "Round 3": the round of an agent, counted from one.
pub(super) fn round_words(round: usize) -> SharedString {
    tr_with(Key("agents.round"), &[("round", &round.to_string())])
}

/// What the second line says of the agent's history: its round when it had several, and how often the newest
/// round was resumed, as in "Round 3, continued once".
fn rounds_words(newest: usize, rounds: usize, resumed: usize) -> Option<SharedString> {
    let round = (rounds > 1).then(|| round_words(newest));
    let resumed = (resumed > 0).then(|| tr_count(Key("agents.resumed"), resumed as u64, &[]));
    match (round, resumed) {
        (Some(round), Some(resumed)) => Some(format!("{round}, {}", lowered(&resumed)).into()),
        (round, resumed) => round.or(resumed),
    }
}

/// `words` with its first letter in lower case, to continue a line after a comma.
fn lowered(words: &str) -> String {
    let mut letters = words.chars();
    letters.next().map_or(String::new(), |first| {
        first.to_lowercase().chain(letters).collect()
    })
}

fn doing_line(doing: &Doing, now: i64) -> DoingLine {
    match doing {
        Doing::Text(words) => DoingLine::Text(super::thread::shown(first_line(words)).into()),
        Doing::Tool {
            tool,
            detail,
            since,
        } => {
            let started = sakura_core::engine::moment::iso_seconds(since).unwrap_or(now);
            let seconds = u64::try_from(now - started).unwrap_or(0);
            DoingLine::Tool {
                label: tool_label(tool),
                detail: super::thread::shown(first_line(detail)).into(),
                elapsed: super::thread::elapsed(Duration::from_secs(seconds)).into(),
            }
        }
    }
}

/// What the list asks of its page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneAgentsEvent {
    /// The person pressed the agent whose conversation is read under this run name.
    Open(String),
}

/// The agents of one lane, read now and every few seconds while the page is shown.
pub struct LaneAgents {
    source: Source,
    repo: PathBuf,
    task: String,
    facts: Entity<EngineFacts>,
    runs: Vec<AgentRun>,
    is_reading: bool,
    /// A change came while the list was being read: one more read follows it.
    is_again: bool,
    problem: Option<String>,
    _observer: Subscription,
    /// Whether the lane's page is shown: only then the list reads.
    is_active: bool,
    _poll: Option<gpui::Task<()>>,
    _clock: Option<gpui::Task<()>>,
}

impl EventEmitter<LaneAgentsEvent> for LaneAgents {}

impl LaneAgents {
    /// The list of the lane `task` of the repository `repo`, read at once and again when the engine's facts change.
    pub fn new(
        source: Source,
        repo: PathBuf,
        task: String,
        facts: &Entity<EngineFacts>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observer = cx.observe(facts, |list, _, cx| list.refresh(cx));
        let mut list = Self {
            source,
            repo,
            task,
            facts: facts.clone(),
            runs: Vec::new(),
            is_reading: false,
            is_again: false,
            problem: None,
            is_active: true,
            _observer: observer,
            _poll: None,
            _clock: None,
        };
        list.start_timers(cx);
        list.refresh(cx);
        list
    }

    /// Stops the reading and the clock while the lane's page is not shown, and starts them again, with a
    /// read at once, when it is. The runs read so far stay.
    pub fn set_active(&mut self, is_active: bool, cx: &mut Context<Self>) {
        if self.is_active == is_active {
            return;
        }
        self.is_active = is_active;
        match is_active {
            true => {
                self.start_timers(cx);
                self.refresh(cx);
            }
            false => {
                self._poll = None;
                self._clock = None;
            }
        }
    }

    fn start_timers(&mut self, cx: &mut Context<Self>) {
        self._poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(sizes::AGENTS_TICK).await;
            if this.update(cx, |list, cx| list.refresh(cx)).is_err() {
                return;
            }
        }));
        self._clock = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(sizes::AGENTS_CLOCK_TICK)
                .await;
            let ticked = this.update(cx, |list, cx| list.tick(cx));
            if ticked.is_err() {
                return;
            }
        }));
    }

    /// The runs read so far.
    pub fn runs(&self) -> &[AgentRun] {
        &self.runs
    }

    /// Reads the list again; one requested while one is read is made once that one is done.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.is_active {
            return;
        }
        if self.is_reading {
            self.is_again = true;
            return;
        }
        self.is_reading = true;
        let pending = self.source.agents(self.repo.clone(), self.task.clone());
        cx.spawn(async move |this, cx| {
            let read = pending.await;
            drop(this.update(cx, |list, cx| list.take(read, cx)));
        })
        .detach();
    }

    fn take(
        &mut self,
        read: sakura_core::error::BridgeResult<Vec<AgentRun>>,
        cx: &mut Context<Self>,
    ) {
        self.is_reading = false;
        match read {
            Ok(runs) => {
                self.problem = None;
                self.runs = runs;
            }
            Err(error) => self.problem = Some(self.problem_text(error, cx)),
        }
        cx.notify();
        if std::mem::take(&mut self.is_again) {
            self.refresh(cx);
        }
    }

    fn problem_text(&self, mut error: BridgeError, cx: &gpui::App) -> String {
        if error.said.key.0 == LANE_FOLDER_MISSING {
            let title = self
                .facts
                .read(cx)
                .snapshot()
                .and_then(|snapshot| {
                    snapshot.items.iter().find(|item| {
                        item.task() == self.task && std::path::Path::new(&item.repo) == self.repo
                    })
                })
                .map(|item| item.title.as_str())
                .filter(|title| !title.trim().is_empty());
            match title {
                Some(title) => {
                    if let Some((_, value)) =
                        error.said.args.iter_mut().find(|(key, _)| *key == "lane")
                    {
                        *value = Value::Text(title.to_string());
                    }
                }
                None => error.said = Said::new(PROJECT_FOLDER_MISSING),
            }
        }
        crate::l10n::told(&error)
    }

    /// Draws the list again each second while a tool runs, so its elapsed time moves.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let is_timing = self
            .runs
            .iter()
            .any(|run| run.state.is_live() && matches!(run.doing, Some(Doing::Tool { .. })));
        if is_timing {
            cx.notify();
        }
    }
}

impl Render for LaneAgents {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let cards = cards_of(&self.runs);
        let moment = now();
        let rows = cards.iter().map(|card| {
            let origin = card.origin.clone();
            super::kit::agent_row(
                format!("agent-{}", card.origin),
                &facts_of(card, moment),
                &palette,
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(LaneAgentsEvent::Open(origin.clone()));
            }))
        });
        let quiet = |words: SharedString, selector: &'static str, palette: &Palette| {
            text(theme::TEXT_SMALL, palette.text_tertiary)
                .px(crate::theme::kit::scale::ROW_PADDING)
                .debug_selector(move || selector.into())
                .child(words)
        };
        let empty = (cards.is_empty() && self.problem.is_none())
            .then(|| quiet(tr(NONE_YET).into(), "agents-none", &palette));
        let problem = self
            .problem
            .clone()
            .map(|words| quiet(words.into(), "agents-problem", &palette));
        div()
            .debug_selector(|| "lane-agents".into())
            .flex()
            .flex_col()
            .child(super::kit::section_header(
                "agents-header",
                tr(Key("agents.title")),
                None,
                &palette,
            ))
            .children(rows)
            .children(empty)
            .children(problem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::engine_rig_tests::{self, SNAPSHOT};
    use gpui::TestAppContext;

    #[gpui::test]
    fn fix37_the_missing_folder_message_names_the_lane_by_title(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("project");
        let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
        let lane = snapshot["items"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|item| item["key"][2] == "stuck")
            .unwrap();
        lane["repo"] = folder.display().to_string().into();
        lane["title"] = "Version option".into();
        let (rig, _, _) =
            crate::ui::lane_tests::open_answering(cx, &snapshot.to_string(), "{}", "{}");
        let (_, window) = cx.add_window_view(|_, cx| {
            LaneAgents::new(rig.source.clone(), folder, "stuck".into(), &rig.facts, cx)
        });
        engine_rig_tests::wait_until(window, "the missing folder message", |window| {
            window.debug_bounds("agents-problem").is_some()
        });
        let words = window.update(|window, cx| {
            window
                .root::<LaneAgents>()
                .unwrap()
                .unwrap()
                .read(cx)
                .problem
                .clone()
        });
        assert_eq!(
            words.as_deref(),
            Some(
                "The folder of project project is missing, so lane Version option cannot be read."
            )
        );
    }
}
