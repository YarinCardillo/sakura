//! "New lane": one sheet, as Linear's new issue is, opened by Cmd-N from anywhere, by the "+" beside Projects
//! and by a project's row. It holds the project the lane starts in, a title, what the lane should do, and the
//! preset and pace chips when the engine takes them. Start (Cmd-Return from either field) sends the engine's
//! action `L` with the title, a blank line and the description; what the engine refuses is said in the sheet in
//! its own words. When the lane is there the sheet goes away and the lane opens in the tab on screen.
//!
//! The sheet never has the caret by itself: a field takes it on a press. The sheet is drawn in
//! `new_lane_view`.

use super::engine_desk::Desk;
use super::project_add::Place;
use crate::data::engine_facts::{EngineFacts, EngineRead as _};
use crate::data::source::Source;
use crate::l10n::{say, tr, Key};
use crate::theme::new_lane as tokens;
use crate::ui::enter::is_composing;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, ParentElement as _, SharedString, Subscription,
    Window,
};
use gpui_component::input::{InputEvent, InputState, TextareaState};
use gpui_component::WindowExt as _;
use sakura_core::engine::cli::{Act, ActResult};
use sakura_core::engine::presets::PresetChoice;
use sakura_core::engine::snapshot::{Action, Project, Snapshot};
use sakura_core::engine::tracks::TrackRef;
use sakura_core::tabs::{Page, TabId};
use std::collections::BTreeMap;

/// What the title field asks while empty, and what the description asks.
pub(super) const TITLE_LABEL: Key = Key("new_lane.title_label");
pub(super) const DESCRIPTION_LABEL: Key = Key("new_lane.description_label");
const QUESTION: Key = Key("new_lane.question");
/// What the sheet says when the engine did not answer in words.
const NOT_STARTED: Key = Key("new_lane.not_started");
/// The inputs of the engine's action that starts a lane.
const TEXT_INPUT: &str = "text";
pub(super) const PRESET_INPUT: &str = "preset";
pub(super) const PACE_INPUT: &str = "level";
pub(super) const TRACK_INPUT: &str = "track";

/// The sentence a lane is started with: its title, then a blank line and the description when there is one;
/// empty while there is no title.
pub fn lane_text(title: &str, description: &str) -> String {
    let (title, description) = (title.trim(), description.trim());
    match (title.is_empty(), description.is_empty()) {
        (true, _) => String::new(),
        (false, true) => title.to_string(),
        (false, false) => format!("{title}\n\n{description}"),
    }
}

/// Whether `action` takes the input `name`, needed or not.
fn takes(action: &Action, name: &str) -> bool {
    action
        .needs
        .iter()
        .chain(&action.optional)
        .any(|input| input == name)
}

/// The inputs of the engine's `action` for a lane: its text, and the preset and pace chosen when the action
/// takes them; an engine that takes only the text is sent only that.
pub(super) fn action_inputs(
    action: &Action,
    text: String,
    (preset, pace): (Option<&str>, Option<&str>),
) -> BTreeMap<String, String> {
    let mut inputs = BTreeMap::from([(TEXT_INPUT.to_string(), text)]);
    for (name, value) in [(PRESET_INPUT, preset), (PACE_INPUT, pace)] {
        if let (Some(value), true) = (value, takes(action, name)) {
            inputs.insert(name.to_string(), value.to_string());
        }
    }
    inputs
}

/// What the sheet tells the desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewLaneEvent {
    /// The engine started a lane in `folder` and said its id: the task id, which with the folder makes the lane's
    /// key. Empty when it named none.
    Started { folder: String, task: String },
}

/// Where the sheet stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Step {
    Asking,
    Starting,
    /// The engine did not start it: its words.
    Told(SharedString),
}

/// The content of the sheet: a project, a title and a description, and the choices that go with them.
pub struct NewLaneSheet {
    pub(super) title: Entity<InputState>,
    pub(super) description: Entity<TextareaState>,
    /// The projects the engine lets a lane start in, in the sidebar's order, and the one chosen.
    pub(super) projects: Vec<Project>,
    pub(super) project: usize,
    /// No project was named when the sheet opened and there are several: the person is asked, and the lane
    /// cannot start before one is chosen.
    pub(super) is_asking_project: bool,
    /// The presets lanes run as now, and what the person chose over them.
    pub(super) presets: Vec<PresetChoice>,
    /// The preset the engine has in force, in words: what a lane runs as while none is chosen.
    pub(super) engine_preset: Option<String>,
    pub(super) preset: Option<String>,
    pub(super) pace: Option<String>,
    pub(super) track: Option<String>,
    preferred_track: Option<String>,
    operation: super::track_page::Operation,
    facts: Option<Entity<EngineFacts>>,
    source: Source,
    pub(super) step: Step,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<NewLaneEvent> for NewLaneSheet {}

impl NewLaneSheet {
    /// An empty sheet that starts lanes through `source`, in `projects[project]` at first.
    pub fn new(
        (projects, chosen): (Vec<Project>, Option<usize>),
        snapshot: &Snapshot,
        source: Source,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = cx.new(|cx| InputState::new(window, cx));
        let description = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(tokens::DESCRIPTION_MIN_ROWS, tokens::DESCRIPTION_MAX_ROWS)
                .placeholder(tr(QUESTION))
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &title,
                window,
                |sheet, title, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } if is_composing(title, window, cx) => {}
                    InputEvent::PressEnter {
                        secondary: true, ..
                    } => sheet.submit(cx),
                    InputEvent::PressEnter { .. } => sheet
                        .description
                        .update(cx, |description, cx| description.focus(window, cx)),
                    _ => cx.notify(),
                },
            ),
            cx.subscribe_in(
                &description,
                window,
                |sheet, description, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter {
                        secondary: true, ..
                    } if !is_composing(description, window, cx) => sheet.submit(cx),
                    _ => cx.notify(),
                },
            ),
        ];
        let is_asking_project = chosen.is_none() && projects.len() > 1;
        Self {
            title,
            description,
            projects,
            project: chosen.unwrap_or(0),
            is_asking_project,
            presets: super::lanes::presets_to_pick(snapshot),
            engine_preset: super::lanes::engine_preset_words(snapshot),
            preset: None,
            pace: None,
            track: None,
            preferred_track: None,
            operation: Default::default(),
            facts: None,
            source,
            step: Step::Asking,
            _subscriptions: subscriptions,
        }
    }

    fn watch_projects(
        &mut self,
        facts: Entity<EngineFacts>,
        preferred: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.preferred_track = preferred.map(str::to_owned);
        self.track = self
            .preferred_track
            .clone()
            .filter(|track| self.offers_track(track));
        self._subscriptions
            .push(cx.observe(&facts, |sheet, facts, cx| {
                if let Some(snapshot) = facts.read(cx).snapshot() {
                    for project in &mut sheet.projects {
                        if let Some(current) = snapshot
                            .projects
                            .iter()
                            .find(|current| current.id == project.id)
                        {
                            project.clone_from(current);
                        } else {
                            project.actions.clear();
                            project.track_choices.clear();
                        }
                    }
                    cx.notify();
                }
            }));
        self.facts = Some(facts);
    }

    pub(super) fn track_choices(&self) -> Option<&[TrackRef]> {
        let project = self.current();
        project.start_lane().filter(|action| takes_track(action))?;
        Some(&project.track_choices)
    }

    fn offers_track(&self, id: &str) -> bool {
        self.track_choices()
            .is_some_and(|choices| choices.iter().any(|choice| choice.id == id))
    }

    /// Writes `words` as the title, for the tests.
    #[cfg(test)]
    pub(super) fn set_title(&mut self, words: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.title.update(cx, |title, cx| {
            title.set_value(words.to_string(), window, cx)
        });
    }

    /// The project the lane starts in.
    pub(super) fn current(&self) -> &Project {
        &self.projects[self.project]
    }

    /// The engine's action that starts a lane in the project chosen.
    fn start_action(&self) -> Option<&Action> {
        self.current().start_lane()
    }

    /// The preset `name` in words: the engine's label when a list of presets gave it, else the name with a capital.
    pub(super) fn preset_words(&self, name: &str) -> String {
        let known = self.presets.iter().find(|preset| preset.name == name);
        known.map_or_else(|| super::lanes::capitalised(name), PresetChoice::words)
    }

    /// Whether the engine's action takes the input `name`.
    pub(super) fn accepts(&self, name: &str) -> bool {
        self.start_action()
            .is_some_and(|action| takes(action, name))
    }

    /// The text of the lane and the choices the engine takes, as the action's inputs.
    fn inputs(&self, action: &Action, cx: &gpui::App) -> BTreeMap<String, String> {
        let text = lane_text(
            &self.title.read(cx).value(),
            &self.description.read(cx).value(),
        );
        action_inputs(action, text, (self.preset.as_deref(), self.pace.as_deref()))
    }

    /// Whether Start would start a lane: there is a title, none is being started, and the engine offers it.
    pub fn can_submit(&self, cx: &gpui::App) -> bool {
        let has_title = !self.title.read(cx).value().trim().is_empty();
        has_title
            && !self.is_asking_project
            && self.step != Step::Starting
            && self.start_action().is_some()
            && self.track.as_ref().is_none_or(|id| self.offers_track(id))
    }

    /// Starts the lane with what the fields say; nothing happens while there is no title.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(action) = self.start_action().cloned().filter(|_| self.can_submit(cx)) else {
            return;
        };
        let folder = self.current().folder.clone();
        let mut act = Act {
            target: self.current().id.clone(),
            inputs: self.inputs(&action, cx),
            action,
            confirm: None,
        };
        if let Some(track) = &self.track {
            act.inputs.insert(TRACK_INPUT.into(), track.clone());
            act = self.operation.prepare(act);
        } else {
            self.operation = Default::default();
        }
        let is_tracked = act.is_track();
        self.step = Step::Starting;
        let pending = self.source.act(act);
        cx.spawn(async move |sheet, cx| {
            let result = pending.await;
            drop(sheet.update(cx, |sheet, cx| {
                sheet.answered(result, folder, is_tracked, cx)
            }));
        })
        .detach();
        cx.notify();
    }

    /// The engine's answer to the lane requested in `folder`, the project the request was made for.
    fn answered(
        &mut self,
        result: sakura_core::error::BridgeResult<ActResult>,
        folder: String,
        is_tracked: bool,
        cx: &mut Context<Self>,
    ) {
        if is_tracked && !matches!(result, Ok(ActResult::Done { exit_code: 0, .. })) {
            if let Some(facts) = &self.facts {
                facts.update(cx, |facts, cx| facts.read(cx));
            }
        }
        self.step = match result {
            Ok(ActResult::Done {
                exit_code: 0,
                output,
                ..
            }) => {
                cx.emit(NewLaneEvent::Started {
                    folder,
                    task: output.trim().to_string(),
                });
                Step::Asking
            }
            Ok(ActResult::Refused {
                reason,
                reason_code,
                reason_args,
            }) => {
                if is_tracked {
                    let words = sakura_core::engine::tracks::refusal(
                        reason_code.as_deref(),
                        reason_args.as_ref(),
                        &reason,
                    );
                    Step::Told(say(&words))
                } else {
                    Step::Told(reason.into())
                }
            }
            Ok(ActResult::Done { .. } | ActResult::Confirm { .. }) => {
                Step::Told(tr(NOT_STARTED).into())
            }
            Err(error) => Step::Told(say(&error.said).to_string().into()),
        };
        cx.notify();
    }

    /// Takes the choice of `input` (a preset or a pace) to `value`; the lane's own pace or no value is the
    /// choice of none, and nothing is sent for it.
    pub(super) fn choose(&mut self, input: &str, value: Option<String>, cx: &mut Context<Self>) {
        match input {
            PRESET_INPUT => self.preset = value,
            TRACK_INPUT => self.track = value.filter(|id| self.offers_track(id)),
            _ => self.pace = value.filter(|pace| pace != super::new_lane_view::LANE_DEFAULT_PACE),
        }
        cx.notify();
    }

    /// Starts the lane in the project at `index` of the list from now on; not while a lane is being started.
    pub(super) fn choose_project(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.step == Step::Starting {
            return;
        }
        self.project = index.min(self.projects.len().saturating_sub(1));
        if self.track.as_ref().is_none_or(|id| !self.offers_track(id)) {
            self.track = self
                .preferred_track
                .clone()
                .filter(|id| self.offers_track(id));
        }
        self.is_asking_project = false;
        cx.notify();
    }
}

/// The projects of `snapshot` the engine lets a lane start in, in the sidebar's order.
pub(super) fn takes_track(action: &Action) -> bool {
    [TRACK_INPUT, "op"]
        .iter()
        .all(|key| action.optional.iter().any(|input| input == key))
}

pub(super) fn can_start_on_track(project: &Project, id: &str) -> bool {
    project.start_lane().is_some_and(takes_track)
        && project.track_choices.iter().any(|choice| choice.id == id)
}

fn starting_projects(snapshot: &Snapshot) -> Vec<Project> {
    let groups = super::lanes::by_project(snapshot);
    let starting = groups.into_iter().filter_map(|group| group.project);
    starting
        .filter(|project| project.start_lane().is_some())
        .cloned()
        .collect()
}

impl Desk {
    /// "New lane" from where no project's row was pressed (Cmd-N, the Lanes page, the palette): in the
    /// project the Lanes page is kept to, else in the first project the engine lets a lane start in.
    pub fn open_new_lane_default(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kept = self
            .active_lanes(cx)
            .and_then(|page| page.read(cx).project())
            .map(str::to_string);
        self.open_new_lane_in((kept.as_deref(), None), window, cx);
    }

    /// "New lane" on `project`'s row: the sheet that asks what the lane should do.
    pub fn open_new_lane(&mut self, project: Project, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_lane_in((Some(&project.folder), None), window, cx);
    }

    /// The sheet over the page, with the project in `folder` chosen when the engine lets a lane start
    /// there, else the first that it does. Nothing before the engine's first snapshot.
    pub(super) fn open_track_lane(
        &mut self,
        track: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_new_lane_in((None, Some(track)), window, cx);
    }

    fn open_new_lane_in(
        &mut self,
        (folder, track): (Option<&str>, Option<&str>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.facts.read(cx).shared_snapshot() else {
            return;
        };
        if folder.is_some_and(|folder| snapshot.is_project_folder_missing(folder)) {
            return;
        }
        let projects = starting_projects(&snapshot);
        if track.is_some_and(|id| {
            !projects
                .iter()
                .any(|project| can_start_on_track(project, id))
        }) {
            return;
        }
        if projects.is_empty() {
            return self.show_add_project_first(window, cx);
        }
        // The sheet opens over the Lanes page, where the new lane will be listed.
        if track.is_none() && *self.bar.read(cx).tabs().active().page() != Page::Lanes {
            self.navigate(Page::Lanes, false, cx);
        }
        let active = self.bar.read(cx).tabs().active();
        let (source_tab, source_page) = (active.id(), active.page().clone());
        let at = projects
            .iter()
            .position(|project| Some(project.folder.as_str()) == folder);
        let source = self.source.clone();
        let sheet = cx.new(|cx| {
            let chosen = (projects, at);
            let mut sheet = NewLaneSheet::new(chosen, &snapshot, source, window, cx);
            sheet.watch_projects(self.facts.clone(), track, cx);
            sheet
        });
        cx.subscribe_in(
            &sheet,
            window,
            move |desk, _, event: &NewLaneEvent, window, cx| {
                let NewLaneEvent::Started { folder, task } = event;
                window.close_dialog(cx);
                desk.open_started_lane(source_tab, source_page.clone(), folder, task, cx);
            },
        )
        .detach();
        self.show_new_lane(sheet, window, cx);
    }

    /// "New lane" with no project to start it in: says to add one, and its button opens the Mac's folder
    /// picker in place. The composer comes back for the project once the engine lists it.
    fn show_add_project_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let adds = self.adds.clone();
        super::dialog_frame::open_guarded(window, cx, move |dialog, window, cx| {
            let palette = crate::theme::Theme::palette(cx);
            let refusal = adds.read(cx).refusal_in(Place::NewLane).cloned();
            let adds = adds.clone();
            let sheet =
                super::new_lane_view::no_project(window, &palette, refusal, move |window, cx| {
                    adds.update(cx, |adds, cx| adds.choose(Place::NewLane, window, cx))
                });
            super::dialog_frame::placed(dialog, super::dialog_frame::Width::Small, window)
                .on_ok(|_, _, _| false)
                .child(sheet)
        });
        self.is_waiting_for_project = true;
        cx.notify();
    }

    /// A project was added while the composer waited for one: the composer opens for it.
    pub(super) fn open_composer_for_the_project_added(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_waiting_for_project {
            return;
        }
        let Some(snapshot) = self.facts.read(cx).shared_snapshot() else {
            return;
        };
        if starting_projects(&snapshot).is_empty() {
            return;
        }
        self.is_waiting_for_project = false;
        window.close_dialog(cx);
        self.open_new_lane_in((None, None), window, cx);
    }

    /// Puts `sheet` over the page, near its top as the prototype has it, and centred across the window.
    fn show_new_lane(
        &mut self,
        sheet: Entity<NewLaneSheet>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        super::dialog_frame::open_guarded(window, cx, move |dialog, window, _| {
            super::dialog_frame::placed(dialog, super::dialog_frame::Width::Medium, window)
                // Enter is the fields': the sheet goes away when a lane was started, or at Esc.
                .on_ok(|_, _, _| false)
                .child(sheet.clone())
        });
        cx.notify();
    }

    /// The engine started the lane `task` in `folder`: reads the engine now and shows exactly that lane in its tab,
    /// once it is listed. No other lane is ever opened in its place, and none when the engine named none.
    pub(super) fn open_started_lane(
        &mut self,
        tab: TabId,
        origin: Page,
        folder: &str,
        task: &str,
        cx: &mut Context<Self>,
    ) {
        let pending = self.source.snapshot();
        self.facts.update(cx, |facts, cx| facts.read(cx));
        if task.is_empty() {
            return;
        }
        let key = serde_json::json!(["flow", folder, task]).to_string();
        cx.spawn(async move |desk, cx| {
            let Ok(snapshot) = pending.await else {
                return;
            };
            if snapshot.items.iter().any(|item| item.lane_key() == key) {
                drop(desk.update(cx, |desk, cx| {
                    desk.follow_page_link(tab, &origin, Page::Lane(key), false, cx);
                }));
            }
        })
        .detach();
    }
}
