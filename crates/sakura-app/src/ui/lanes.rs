//! How a lane, an engine flow task, looks wherever it is shown: its state mark, its progress bar and the
//! words under it. Everything here is read from the engine's snapshot; the app computes nothing of a lane's
//! progress and decides nothing of its state.

use crate::l10n::{tr, tr_with, Key};
use crate::theme::{self, Palette};
use crate::ui::kit::StateGlyph;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _,
};
use gpui_component::progress::Progress;
use gpui_component::Sizable as _;
use sakura_core::engine::presets::PresetChoice;
use sakura_core::engine::snapshot::{Item, Progress as LaneProgress, Project, Snapshot};
use sakura_core::model::{self, StateId, TaskState};
use std::collections::BTreeMap;

pub(super) const FOLDER_MISSING: Key = Key("project.folder_missing");

/// The state the window shows for a lane: what the engine says of its agent, in the words every state of
/// the window has. A lane at a gate waits for the person; a blocked one keeps the tone of a failure and the
/// engine's word, blocked; one that delivered, was cleaned up or was stopped has a state of its own; a lane
/// that is none of these nor at work stands by. An outside lane is `waiting` exactly while it has an open row or
/// question, and that is what waits for the person.
pub fn state_of(item: &Item) -> TaskState {
    let id = match item.agent_state.as_str() {
        "gate" => StateId::NeedsYou,
        "waiting" if item.is_outside() => StateId::NeedsYou,
        "blocked" => StateId::Failed,
        "working" => StateId::Working,
        // A lane that ended is never "idle": it delivered, was cleaned up or was stopped.
        "delivered" => StateId::Verified,
        "cleaned" | "stopped" => StateId::NotStarted,
        _ => StateId::Idle,
    };
    let state = model::state_for(id);
    match ended_word(item) {
        Some(word) => TaskState { word, ..state },
        None => state,
    }
}

/// The lane title, or its branch while the person holds Option.
pub fn lane_words(item: &Item, is_identity: bool) -> String {
    if let Some(branch) = branch(item).filter(|_| is_identity) {
        return branch.to_string();
    }
    if item.title.trim().is_empty() {
        let task = item.task();
        return if task.is_empty() {
            tr(Key("lane.untitled")).to_string()
        } else {
            task.to_string()
        };
    }
    item.title.clone()
}

pub(super) fn branch(item: &Item) -> Option<&str> {
    (item.agent_state != "cleaned" && !item.branch.is_empty()).then_some(item.branch.as_str())
}

/// The mark of a lane's state on a tab: its own shape for a lane that needs the person, runs, finished,
/// stopped or has not started.
pub fn glyph_of(item: &Item) -> StateGlyph {
    match state_of(item).id {
        StateId::NotStarted if item.agent_state == "cleaned" => StateGlyph::NotStarted,
        StateId::NeedsYou => StateGlyph::NeedsYou,
        StateId::Working => StateGlyph::Running,
        StateId::Verified | StateId::Taken => StateGlyph::Finished,
        StateId::Failed | StateId::NotStarted => StateGlyph::Stopped,
        _ => StateGlyph::NotStarted,
    }
}

/// How many of `items` are open lanes: those that neither delivered, nor were cleaned up, nor were stopped. A
/// morning digest is no lane.
pub fn open_count(items: &[Item]) -> usize {
    open_lanes(items).count()
}

/// The open lanes of `items`: the set the Engine page's table lists and its header counts.
pub fn open_lanes(items: &[Item]) -> impl Iterator<Item = &Item> {
    items
        .iter()
        .filter(|item| !item.is_digest() && is_open(item))
}

/// How many of `items` are lanes at work now. A stuck lane, one that waits at a gate and one with nothing
/// running are open, and none of them runs. A morning digest is no lane.
pub fn running_count(items: &[Item]) -> usize {
    open_lanes(items)
        .filter(|item| state_of(item).id == StateId::Working)
        .count()
}

/// Whether `item` is an open lane: one that neither delivered, nor was cleaned up, nor was stopped.
pub fn is_open(item: &Item) -> bool {
    !matches!(
        item.agent_state.as_str(),
        "delivered" | "cleaned" | "stopped"
    )
}

/// The word of the state a lane ended in, in the interface's words: `blocked`, the engine's word for a lane
/// that cannot continue, `delivered`, `cleaned` or `stopped`; none for a lane that has not ended.
pub fn ended_word(item: &Item) -> Option<sakura_core::said::Key> {
    let word = match item.agent_state.as_str() {
        "blocked" => "lane.state.blocked",
        "delivered" => "lane.state.delivered",
        "cleaned" => "lane.state.cleaned",
        "stopped" => "lane.state.stopped",
        _ => return None,
    };
    Some(sakura_core::said::Key(word))
}

/// "From claude in work": whom an outside agent's lane or wait is from, and where it works; "From digest" when the
/// project has the agent's own name, so the name is not said twice.
pub fn from_words(agent: &str, project: &str) -> SharedString {
    if agent == project {
        return tr_with(Key("waiting.from_alone"), &[("agent", agent)]);
    }
    tr_with(
        Key("waiting.from"),
        &[("agent", agent), ("project", project)],
    )
}

/// "3 of 7 done": what the bar says in words, and the bar's accessible label.
pub fn done_words(progress: &LaneProgress) -> SharedString {
    let (done, total) = (progress.done.to_string(), progress.total.to_string());
    tr_with(Key("lane.progress"), &[("done", &done), ("total", &total)])
}

/// The localized label for an engine phase.
pub fn phase_word(phase: &str) -> SharedString {
    let key = match phase {
        "draft" => Key("lane.phase.draft"),
        "ready" => Key("lane.phase.ready"),
        "intake" => Key("lane.phase.intake"),
        "contract" => Key("lane.phase.contract"),
        "plan" => Key("lane.phase.plan"),
        "build" => Key("lane.phase.build"),
        "evaluate" => Key("lane.phase.evaluate"),
        "pr" => Key("lane.phase.pr"),
        "report" => Key("lane.phase.report"),
        "cleanup" => Key("lane.phase.cleanup"),
        _ => Key("lane.phase.unknown"),
    };
    SharedString::from(tr(key))
}

/// The localized label for an engine role.
pub fn role_words(role: &str) -> SharedString {
    let key = match role {
        "planner" => Key("lane.role.planner"),
        "plan_reviewer" => Key("lane.role.plan_reviewer"),
        "arbiter" => Key("lane.role.arbiter"),
        "builder" => Key("lane.role.builder"),
        "code_reviewer" => Key("lane.role.code_reviewer"),
        "evaluator" => Key("lane.role.evaluator"),
        "reporter" => Key("lane.role.reporter"),
        _ => Key("lane.role.unknown"),
    };
    SharedString::from(tr(key))
}

/// Where a lane stands for the Lanes page: the groups of its list, in the order it shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LaneGroup {
    /// The lane has an open ask, or cannot go on without the person.
    NeedsYou,
    Running,
    Finished,
    Stopped,
    Closed,
}

impl LaneGroup {
    /// The groups in the order the page lists them.
    pub const ALL: [LaneGroup; 5] = [
        LaneGroup::NeedsYou,
        LaneGroup::Running,
        LaneGroup::Finished,
        LaneGroup::Stopped,
        LaneGroup::Closed,
    ];
}

/// What the Lanes page keeps of the lanes: those of one project (by its folder) and those of one preset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaneFilter {
    pub project: Option<String>,
    pub preset: Option<String>,
}

impl LaneFilter {
    /// Whether no filter is set.
    pub fn is_empty(&self) -> bool {
        self.project.is_none() && self.preset.is_none()
    }

    fn keeps(&self, item: &Item) -> bool {
        let is_project = self
            .project
            .as_ref()
            .is_none_or(|folder| *folder == item.repo);
        let is_preset = self
            .preset
            .as_ref()
            .is_none_or(|preset| item.preset.as_ref() == Some(preset));
        is_project && is_preset
    }
}

/// The lanes of a snapshot by group, each group newest activity first.
#[derive(Debug, Default)]
pub struct LaneGroups<'a> {
    groups: [Vec<&'a Item>; 5],
}

impl<'a> LaneGroups<'a> {
    /// The lanes of `group`.
    pub fn of(&self, group: LaneGroup) -> &[&'a Item] {
        &self.groups[group as usize]
    }
}

/// A lane can stay at its gate while every ask for it is snoozed.
pub(super) fn is_waiting_on_snoozed_asks(item: &Item, snapshot: &Snapshot, now: i64) -> bool {
    let mut waits = snapshot.waits_for(item);
    waits.next().is_some_and(|wait| wait.is_snoozed_at(now))
        && waits.all(|wait| wait.is_snoozed_at(now))
}

/// Outside progress counts owner rows. Questions do not contribute to its total.
pub(super) fn snoozed_owner_rows(item: &Item, snapshot: &Snapshot, now: i64) -> u32 {
    let count = snapshot
        .waits_for(item)
        .filter(|wait| wait.original_kind() == "owner_row" && wait.is_snoozed_at(now))
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

pub(super) fn shown_state(item: &Item, snapshot: Option<&Snapshot>) -> TaskState {
    let state = state_of(item);
    if state.id == StateId::NeedsYou
        && snapshot
            .is_some_and(|snapshot| is_waiting_on_snoozed_asks(item, snapshot, super::unix_now()))
    {
        return model::state_for(StateId::Idle);
    }
    state
}

/// The group `item` is in: a lane with an open ask, or that cannot go on, needs the person whatever else it
/// is; the others are running, finished (delivered or cleaned up) or stopped.
pub fn group_of(item: &Item, snapshot: &Snapshot) -> LaneGroup {
    if item.agent_state == "cleaned" {
        return LaneGroup::Closed;
    }
    let now = super::unix_now();
    let has_ask = snapshot
        .waits_for(item)
        .any(|wait| !wait.is_snoozed_at(now));
    let needs_person = state_of(item).id == StateId::NeedsYou || item.agent_state == "blocked";
    if has_ask || (needs_person && !is_waiting_on_snoozed_asks(item, snapshot, now)) {
        return LaneGroup::NeedsYou;
    }
    match item.agent_state.as_str() {
        "delivered" => LaneGroup::Finished,
        "stopped" => LaneGroup::Stopped,
        _ => LaneGroup::Running,
    }
}

/// The lanes of `snapshot` that `filter` keeps, by group.
pub fn grouped<'a>(snapshot: &'a Snapshot, filter: &LaneFilter) -> LaneGroups<'a> {
    let mut found = LaneGroups::default();
    let lanes = snapshot.items.iter().filter(|item| !item.is_digest());
    for item in lanes.filter(|item| filter.keeps(item)) {
        found.groups[group_of(item, snapshot) as usize].push(item);
    }
    for group in &mut found.groups {
        group.sort_by(|a, b| {
            let (a_at, b_at) = (&a.last_activity_at, &b.last_activity_at);
            b_at.cmp(a_at).then_with(|| a.name().cmp(b.name()))
        });
    }
    found
}

/// The distinct presets the lanes of `snapshot` run as, in name order.
pub fn presets_of(snapshot: &Snapshot) -> Vec<String> {
    let found: std::collections::BTreeSet<&String> = snapshot
        .items
        .iter()
        .filter_map(|item| item.preset.as_ref())
        .collect();
    found.into_iter().cloned().collect()
}

/// The preset `name` in words: the label the engine gave a lane that runs as it, or the engine itself when it is the
/// preset in force, else the name with a capital. An id is never shown while the engine said a label for it.
pub fn preset_words(snapshot: &Snapshot, name: &str) -> String {
    let of_lane = snapshot
        .items
        .iter()
        .find(|item| item.preset.as_deref() == Some(name))
        .and_then(|item| item.preset_label.clone());
    let of_engine = snapshot
        .engine
        .iter()
        .find(|engine| engine.preset.as_deref() == Some(name))
        .and_then(|engine| engine.preset_label.clone());
    of_lane.or(of_engine).unwrap_or_else(|| capitalised(name))
}

/// The preset the engine has in force, in words: what a lane starts under when none is chosen; none from an
/// engine that does not say.
pub fn engine_preset_words(snapshot: &Snapshot) -> Option<String> {
    let engine = snapshot.engine.as_ref()?;
    let preset = engine.preset.as_deref()?;
    Some(
        engine
            .preset_label
            .clone()
            .unwrap_or_else(|| capitalised(preset)),
    )
}

/// The distinct presets the lanes of `snapshot` run as, each with its words.
pub fn preset_choices_of(snapshot: &Snapshot) -> Vec<PresetChoice> {
    let named = |name: String| {
        let label = preset_words(snapshot, &name);
        PresetChoice::named(&name, Some(&label))
    };
    presets_of(snapshot).into_iter().map(named).collect()
}

/// The presets a person may pick: those the engine lists, in its order, with its words for each (the legacy
/// names are left out by the engine); for a snapshot that lists none, those the lanes run as.
pub fn presets_to_pick(snapshot: &Snapshot) -> Vec<PresetChoice> {
    match snapshot.engine.as_ref().map(|engine| &engine.presets) {
        Some(listed) if !listed.is_empty() => listed.clone(),
        _ => preset_choices_of(snapshot),
    }
}

/// Where a lane stands in its steps: the step it is at, and how many of the steps are done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepWords {
    pub name: SharedString,
    /// Done and total; none while the lane has no plan or steps to count. The thin line of progress is drawn
    /// from it.
    pub counts: Option<(u32, u32)>,
    /// Which of the steps the lane's header draws it is at, and how many that is: a step turned off is not drawn
    /// there, so it is not counted here. None when the engine reports no steps.
    pub place: Option<(u32, u32)>,
}

/// Where `item` stands: from its steps when the engine reports them, else from its progress and phase.
pub fn step_words(item: &Item) -> StepWords {
    if !item.pipeline.is_empty() {
        return from_pipeline(item);
    }
    if !item.steps.is_empty() {
        return from_steps(item);
    }
    let phase = item
        .progress
        .as_ref()
        .map_or("", |progress| progress.phase_id());
    let counts = item
        .progress
        .as_ref()
        .filter(|progress| progress.total > 0)
        .map(|progress| (progress.done, progress.total));
    StepWords {
        name: phase_word(phase),
        counts,
        place: None,
    }
}

/// From the whole pipeline: the steps the lane runs are the ones counted, the one it is at is the first that runs now,
/// else the first still to come.
fn from_pipeline(item: &Item) -> StepWords {
    let pipeline = item.current_pipeline();
    let running: Vec<_> = pipeline.iter().filter(|step| step.is_on).collect();
    let current = running
        .iter()
        .find(|step| matches!(step.state.as_str(), "running" | "waiting"))
        .or_else(|| running.iter().find(|step| step.state == "next"))
        .or_else(|| running.last())
        .copied();
    let at = current.and_then(|current| running.iter().position(|step| step.id == current.id));
    let done = running.iter().filter(|step| step.state == "done").count();
    StepWords {
        name: current.map_or_else(SharedString::default, super::step_track::pipeline_words),
        counts: Some((done as u32, running.len() as u32)),
        place: at.map(|at| (at as u32 + 1, running.len() as u32)),
    }
}

fn from_steps(item: &Item) -> StepWords {
    let steps: Vec<&sakura_core::engine::snapshot::LaneStep> = item
        .steps
        .iter()
        .filter(|step| step.state != "skipped")
        .collect();
    let current = steps
        .iter()
        .find(|step| step.state == "now")
        .or_else(|| steps.iter().find(|step| step.state == "next"))
        .or_else(|| steps.last())
        .copied();
    let at = current.and_then(|current| steps.iter().position(|step| step.id == current.id));
    StepWords {
        name: current.map_or_else(SharedString::default, step_name),
        counts: Some(super::step_track::counted(&item.steps)),
        place: at.map(|at| (at as u32 + 1, steps.len() as u32)),
    }
}

/// The word of a step by its id; a step this build has no word for is shown as the engine names it.
fn step_name(step: &sakura_core::engine::snapshot::LaneStep) -> SharedString {
    let key = match step.id.as_str() {
        "plan" => Key("lane.step.plan"),
        "build" => Key("lane.step.build"),
        "checks" => Key("lane.step.checks"),
        "deliver" => Key("lane.step.deliver"),
        _ => return SharedString::from(step.name.clone()),
    };
    SharedString::from(tr(key))
}

/// "Build, step 2 of 4": the step a lane is at among the steps its header draws; with no steps, "Build, 2 of 4"
/// from its progress, or the step alone while there is nothing to count.
pub fn step_line(words: &StepWords) -> SharedString {
    if let Some((place, total)) = words.place {
        let (place, total) = (place.to_string(), total.to_string());
        let values = [
            ("step", words.name.as_ref()),
            ("place", &place),
            ("total", &total),
        ];
        return tr_with(Key("lanes_page.step_at"), &values);
    }
    match words.counts {
        Some((done, total)) => {
            let (done, total) = (done.to_string(), total.to_string());
            let values = [
                ("step", words.name.as_ref()),
                ("done", &done),
                ("total", &total),
            ];
            tr_with(Key("lanes_page.step_counted"), &values)
        }
        None => words.name.clone(),
    }
}

/// "careful" as "Careful": the engine names presets in lower case.
pub fn capitalised(name: &str) -> String {
    let mut letters = name.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

/// A project of the sidebar and the lanes under it.
pub struct ProjectGroup<'a> {
    /// The folder's name.
    pub name: String,
    /// The project's folder, which names it for good: two folders may share a name.
    pub folder: String,
    /// The registered project.
    pub project: Option<&'a Project>,
    /// Its lanes, in name order.
    pub lanes: Vec<&'a Item>,
}

/// The projects of `snapshot` in name order, each with its lanes in name order: the order a sidebar lists them
/// in, the same at every read, whatever state a lane is in. Every registered project is
/// listed, also one with no lane. An outside agent's lane in a folder no project owns is not a project's: it
/// is one of [`outside_lanes`], and its folder is never listed as a project.
pub fn by_project(snapshot: &Snapshot) -> Vec<ProjectGroup<'_>> {
    let mut groups: BTreeMap<&str, ProjectGroup<'_>> = BTreeMap::new();
    for project in &snapshot.projects {
        let group = ProjectGroup {
            name: project.name.clone(),
            folder: project.folder.clone(),
            project: Some(project),
            lanes: Vec::new(),
        };
        groups.insert(project.folder.as_str(), group);
    }
    for item in snapshot
        .items
        .iter()
        .filter(|item| !item.is_digest() && !is_outside_of_projects(item, snapshot))
    {
        groups
            .entry(item.repo.as_str())
            .or_insert_with(|| ProjectGroup {
                name: item.project().to_string(),
                folder: item.repo.clone(),
                project: None,
                lanes: Vec::new(),
            })
            .lanes
            .push(item);
    }
    let mut listed: Vec<ProjectGroup<'_>> = groups.into_values().collect();
    for group in &mut listed {
        group.lanes.sort_by(by_name);
    }
    listed.sort_by(|a, b| a.name.cmp(&b.name));
    listed
}

/// The lanes an outside agent opened in a folder no registered project owns, the newest first: where no
/// lane can be started, and which is no project.
pub fn outside_lanes(snapshot: &Snapshot) -> Vec<&Item> {
    let mut found: Vec<&Item> = snapshot
        .items
        .iter()
        .filter(|item| is_outside_of_projects(item, snapshot))
        .collect();
    found.sort_by(newest_first);
    found
}

/// Whether `item` is an outside agent's lane in a folder that is no registered project.
fn is_outside_of_projects(item: &Item, snapshot: &Snapshot) -> bool {
    item.is_outside()
        && !item.is_digest()
        && !snapshot
            .projects
            .iter()
            .any(|project| project.folder == item.repo)
}

/// The order a project's lanes are listed in: by name, which no state change moves. The time a lane entered its
/// state, and when it was last active, change as it works, and a row that moved under the pointer was pressed
/// as another lane.
fn by_name(a: &&Item, b: &&Item) -> std::cmp::Ordering {
    a.name()
        .cmp(b.name())
        .then_with(|| a.lane_key().cmp(&b.lane_key()))
}

fn newest_first(a: &&Item, b: &&Item) -> std::cmp::Ordering {
    let (a_task, b_task) = (a.task(), b.task());
    b.since
        .cmp(&a.since)
        .then_with(|| a_task.cmp(b_task))
        .then_with(|| a.lane_key().cmp(&b.lane_key()))
}

/// What a lane's row says under its name: the bar of its progress, or, while it has no plan yet, the word of
/// its phase. `id` names the bar's element; a `wide` bar fills the room it is given.
pub fn progress(item: &Item, id: &str, is_wide: bool, palette: &Palette) -> AnyElement {
    match item.progress.as_ref().filter(|progress| progress.total > 0) {
        Some(progress) => bar(progress, id, is_wide, palette),
        None => {
            let phase = item.progress.as_ref().map(|progress| progress.phase_id());
            let word = phase.map_or_else(SharedString::default, phase_word);
            super::chrome::text(theme::TEXT_LABEL, palette.text_tertiary)
                .debug_selector({
                    let id = format!("{id}-phase");
                    move || id
                })
                .child(word)
                .into_any_element()
        }
    }
}

/// The kit's progress bar at `progress`, its words as its accessible label.
fn bar(progress: &LaneProgress, id: &str, is_wide: bool, palette: &Palette) -> AnyElement {
    let share = f32::from(u16::try_from(progress.done.min(progress.total)).unwrap_or(u16::MAX))
        / f32::from(u16::try_from(progress.total).unwrap_or(u16::MAX))
        * 100.;
    let selector = format!("{id}-bar");
    div()
        .debug_selector(move || selector)
        .when(is_wide, |bar| bar.w_full())
        .when(!is_wide, |bar| bar.w(theme::LANE_BAR_WIDTH).flex_none())
        .child(
            Progress::new(SharedString::from(format!("{id}-progress")))
                .value(share)
                .color(palette.primary)
                .accessibility_label(done_words(progress))
                .xsmall(),
        )
        .into_any_element()
}
