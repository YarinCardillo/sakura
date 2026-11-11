//! "Add project": the Mac's folder picker, then `box project detect` on the folder chosen, one
//! screen to confirm what was recognised (the name can be changed), then `box project add`. A
//! folder `box` refuses and an addition it refuses are said in place; nothing is asked again by
//! itself. Each step has a number, so an answer for a step the person left is dropped.

use super::controls::{self, OnPress};
use super::source::SettingsSource;
use super::FolderPicker;
use crate::l10n::{tr, Key};
use crate::theme::settings as sizes;
use crate::theme::{self, Palette, Theme};
use gpui::{
    div, AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window,
};
use gpui_component::input::InputState;
use sakura_core::error::{BridgeError, BridgeResult};
use sakura_core::settings::detect::Detected;
use std::path::PathBuf;
use std::rc::Rc;

/// What the flow tells Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddEvent {
    /// The project named was added.
    Added(String),
    /// The person left without adding.
    Cancelled,
}

/// Where the flow stands.
enum Stage {
    /// The folder picker is open.
    Picking,
    /// `box project detect` runs on the folder.
    Detecting,
    /// What was recognised is on screen.
    Confirming(Box<Detected>),
    /// `box project add` runs.
    Adding(Box<Detected>),
    /// The folder could not be used.
    Refused,
}

/// The flow that adds one project.
pub struct AddProject {
    source: Rc<dyn SettingsSource>,
    pick_folder: FolderPicker,
    stage: Stage,
    /// What the last step failed with, said in place.
    failure: Option<BridgeError>,
    name: Entity<InputState>,
    /// The step in flight.
    step: u64,
}

impl EventEmitter<AddEvent> for AddProject {}

impl AddProject {
    /// Opens the folder picker at once.
    pub fn new(
        source: Rc<dyn SettingsSource>,
        pick_folder: FolderPicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx));
        let mut flow = Self {
            source,
            pick_folder,
            stage: Stage::Picking,
            failure: None,
            name,
            step: 0,
        };
        flow.pick(window, cx);
        flow
    }

    /// Asks the person for a folder; none chosen ends the flow when nothing was recognised yet.
    fn pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step += 1;
        let step = self.step;
        self.stage = Stage::Picking;
        self.failure = None;
        let chosen = (self.pick_folder)(cx);
        cx.spawn_in(window, async move |flow, cx| {
            let folder = chosen.await;
            drop(flow.update_in(cx, |flow, window, cx| {
                if flow.step == step {
                    flow.picked(folder, window, cx);
                }
            }));
        })
        .detach();
        cx.notify();
    }

    fn picked(&mut self, folder: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = folder else {
            return cx.emit(AddEvent::Cancelled);
        };
        self.step += 1;
        let step = self.step;
        self.stage = Stage::Detecting;
        let answer = self.source.detect(&folder.to_string_lossy());
        cx.spawn_in(window, async move |flow, cx| {
            let answer = answer.await;
            drop(flow.update_in(cx, |flow, window, cx| {
                if flow.step == step {
                    flow.detected(answer, window, cx);
                }
            }));
        })
        .detach();
        cx.notify();
    }

    fn detected(
        &mut self,
        answer: BridgeResult<Detected>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match answer {
            Ok(detected) => {
                let name = detected.name.clone();
                self.name
                    .update(cx, |field, cx| field.set_value(name, window, cx));
                self.stage = Stage::Confirming(Box::new(detected));
            }
            Err(error) => {
                self.stage = Stage::Refused;
                self.failure = Some(error);
            }
        }
        cx.notify();
    }

    /// "Add project": what was recognised, under the name in the field, is sent to `box`.
    fn add(&mut self, cx: &mut Context<Self>) {
        let Stage::Confirming(detected) = &self.stage else {
            return;
        };
        let mut addition = detected.addition();
        addition.name = self.name.read(cx).value().trim().to_string();
        if let Err(error) = addition.args() {
            self.failure = Some(error);
            return cx.notify();
        }
        self.step += 1;
        let (step, name) = (self.step, addition.name.clone());
        self.failure = None;
        self.stage = Stage::Adding(detected.clone());
        let answer = self.source.add(addition);
        cx.spawn(async move |flow, cx| {
            let answer = answer.await;
            drop(flow.update(cx, |flow, cx| {
                if flow.step == step {
                    flow.added(name, answer, cx);
                }
            }));
        })
        .detach();
        cx.notify();
    }

    fn added(&mut self, name: String, answer: BridgeResult<()>, cx: &mut Context<Self>) {
        let Stage::Adding(detected) = std::mem::replace(&mut self.stage, Stage::Refused) else {
            return;
        };
        match answer {
            Ok(()) => cx.emit(AddEvent::Added(name)),
            Err(error) => self.failure = Some(error),
        }
        self.stage = Stage::Confirming(detected);
        cx.notify();
    }

    fn on(
        cx: &Context<Self>,
        act: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> OnPress {
        let flow = cx.weak_entity();
        Rc::new(move |window, cx| drop(flow.update(cx, |flow, cx| act(flow, window, cx))))
    }

    /// The buttons under every stage: leave, and what the stage offers.
    fn actions(&self, cx: &Context<Self>) -> AnyElement {
        let cancel = Self::on(cx, |_, _, cx| cx.emit(AddEvent::Cancelled));
        let other = Self::on(cx, |flow, window, cx| flow.pick(window, cx));
        let add = Self::on(cx, |flow, _, cx| flow.add(cx));
        let is_adding = matches!(self.stage, Stage::Adding(_));
        let has_proposal = matches!(self.stage, Stage::Confirming(_) | Stage::Adding(_));
        let can_choose = matches!(self.stage, Stage::Confirming(_) | Stage::Refused);
        let add_word = if is_adding {
            Key("settings.add.adding")
        } else {
            Key("settings.add.confirm")
        };
        div()
            .flex()
            .items_center()
            .justify_end()
            .gap(theme::GAP)
            .child(controls::button(
                "settings-add-cancel",
                tr(Key("common.cancel")),
                cancel,
                cx,
            ))
            .children(can_choose.then(|| {
                controls::button(
                    "settings-add-other",
                    tr(Key("settings.add.other_folder")),
                    other,
                    cx,
                )
            }))
            .children(has_proposal.then(|| {
                use gpui_component::Disableable as _;
                controls::button("settings-add-confirm", tr(add_word), add, cx).disabled(is_adding)
            }))
            .into_any_element()
    }

    /// What was recognised, as the rows of the project it would be.
    fn proposal(&self, detected: &Detected, palette: &Palette) -> Vec<AnyElement> {
        let said = |id: &'static str, name: Key, value: String| {
            controls::row(id, tr(name), controls::quiet(value, palette), None, palette)
        };
        let folder = match detected.subdir.as_str() {
            "" => detected.repo.clone(),
            subdir => format!("{}/{subdir}", detected.repo),
        };
        let name = controls::field(&self.name, sizes::FIELD_WIDTH, palette);
        let mut rows = vec![
            controls::row(
                "settings-add-name",
                tr(Key("settings.add.name")),
                name,
                None,
                palette,
            ),
            said(
                "settings-add-folder",
                Key("settings.project.folder"),
                folder,
            ),
            said(
                "settings-add-base",
                Key("settings.project.base"),
                detected.default_branch.clone(),
            ),
        ];
        if let Some(install) = &detected.install {
            rows.push(said(
                "settings-add-install",
                Key("settings.add.install"),
                install.command.clone(),
            ));
        }
        let mut groups = vec![controls::group(None, rows, palette)];
        let checks = detected.checks.iter().enumerate().map(|(nth, check)| {
            let id = SharedString::from(format!("settings-add-check-{nth}"));
            let command = controls::quiet(check.command.clone(), palette);
            controls::row(id, check.verb.clone(), command, None, palette)
        });
        let checks: Vec<AnyElement> = checks.collect();
        if checks.is_empty() {
            groups.push(controls::quiet(tr(Key("settings.add.no_checks")), palette));
        } else {
            groups.push(controls::group(
                Some(tr(Key("settings.project.checks"))),
                checks,
                palette,
            ));
        }
        // Where the proposal would not work as it is: said once, with what the service wrote as
        // its details.
        if !detected.notes.is_empty() {
            let notes = detected.notes.join("\n");
            let said = tr(Key("settings.add.notes"));
            let said =
                crate::ui::chrome::failure_sentence("settings-add-notes", said, Some(&notes))
                    .debug_selector(|| "settings-add-notes".into())
                    .text_size(theme::TEXT_SMALL)
                    .text_color(palette.text_tertiary);
            groups.push(said.into_any_element());
        }
        groups
    }
}

impl Render for AddProject {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let mut groups = match &self.stage {
            Stage::Picking => vec![controls::quiet(tr(Key("settings.add.picking")), &palette)],
            Stage::Detecting => vec![controls::quiet(tr(Key("settings.add.detecting")), &palette)],
            Stage::Confirming(detected) | Stage::Adding(detected) => {
                self.proposal(detected, &palette)
            }
            Stage::Refused => Vec::new(),
        };
        if let Some(failure) = &self.failure {
            groups.push(controls::failed("settings-add-failed", failure, &palette));
        }
        groups.push(self.actions(cx));
        div()
            .debug_selector(|| "settings-add".into())
            .w_full()
            .max_w(sizes::PAGE_MAX_WIDTH)
            .child(controls::page(
                tr(Key("settings.add.title")),
                groups,
                &palette,
            ))
    }
}
