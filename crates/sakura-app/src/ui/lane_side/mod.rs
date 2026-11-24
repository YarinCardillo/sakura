//! The column at the right of a lane's page: a switch between Details, Changes and Files, and the view
//! picked. Details holds what the engine says of the lane and its controls (the preset menu, the pace),
//! Changes the files the lane changed with the patch of one, Files the file the agent is on. The column
//! shows what the engine reports and sends the person's choices through the window's answers; it decides
//! nothing.

mod changes;
mod details;
mod files;
mod worktree;

#[cfg(test)]
mod tests;

pub use files::Location;

use super::answers::{lane_key, Answers, Step};
use crate::data::engine_facts::EngineFacts;
use crate::data::source::Source;
use crate::l10n::{tr, Key};
use crate::theme::{self, lane_side as sizes, Theme};
use crate::ui::kit::segments;
use changes::LaneChanges;
use files::LaneFiles;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, ScrollHandle, SharedString, Styled as _, Subscription,
    Window,
};
use sakura_core::engine::snapshot::{Item, PipelineStep, Snapshot};
use std::collections::BTreeMap;

/// Which view of the column is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Details,
    Changes,
    Files,
}

const VIEWS: [View; 3] = [View::Details, View::Changes, View::Files];

impl View {
    fn words(self) -> SharedString {
        let key = match self {
            View::Details => Key("common.details"),
            View::Changes => Key("view.changes"),
            View::Files => Key("view.files"),
        };
        SharedString::from(tr(key))
    }
}

/// What the column asks of the page around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneSideEvent {
    /// The person wants to answer an ask in the Inbox.
    OpenInbox,
}

/// The column of the lane with key `lane`.
pub struct LaneSide {
    is_compact: bool,
    lane: String,
    facts: Entity<EngineFacts>,
    answers: Entity<Answers>,
    view: View,
    changes: Entity<LaneChanges>,
    files: Entity<LaneFiles>,
    /// What the person requested that the engine only queued, by the action's key: the preset or the pace
    /// the controls show until the engine's own facts say the same.
    requested: BTreeMap<String, String>,
    pipeline: Vec<PipelineStep>,
    /// How far Details is scrolled, kept while another view is shown.
    details_scroll: ScrollHandle,
    _observers: Vec<Subscription>,
}

impl EventEmitter<LaneSideEvent> for LaneSide {}

impl LaneSide {
    /// The same pipeline the lane header uses, including any pending toggle.
    pub fn set_pipeline(&mut self, pipeline: Vec<PipelineStep>, cx: &mut Context<Self>) {
        if self.pipeline != pipeline {
            self.pipeline = pipeline;
            cx.notify();
        }
    }

    /// The column of `lane`, whose folder and repository `item` names.
    pub fn new(
        item: &Item,
        facts: Entity<EngineFacts>,
        answers: Entity<Answers>,
        source: Source,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (repo, task) = (item.repo.clone().into(), item.task().to_string());
        let changes_source = source.clone();
        let changes = cx.new(|cx| LaneChanges::new(changes_source, repo, task, cx));
        let worktree = item.worktree.clone();
        let files = cx.new(|cx| LaneFiles::new(worktree, &source, window, cx));
        let observers = vec![
            cx.observe(&facts, |side, _, cx| side.facts_changed(cx)),
            cx.observe(&answers, |_, _, cx| cx.notify()),
            cx.observe(&changes, |_, _, cx| cx.notify()),
            cx.observe(&files, |_, _, cx| cx.notify()),
        ];
        Self {
            is_compact: false,
            lane: item.lane_key(),
            facts,
            answers,
            view: View::Details,
            changes,
            files,
            requested: BTreeMap::new(),
            pipeline: Vec::new(),
            details_scroll: ScrollHandle::new(),
            _observers: observers,
        }
    }

    /// The facts changed: what the shown view reads is read again.
    fn facts_changed(&mut self, cx: &mut Context<Self>) {
        self.drop_applied(cx);
        match self.view {
            View::Changes => self.changes.update(cx, |changes, cx| changes.refresh(cx)),
            View::Files => self.files.update(cx, |files, cx| files.reread(cx)),
            View::Details => {}
        }
        cx.notify();
    }

    /// What was queued and is now what the engine reports is forgotten: it applied.
    fn drop_applied(&mut self, cx: &Context<Self>) {
        let snapshot = self.facts.read(cx).shared_snapshot();
        let Some(item) = snapshot.as_ref().and_then(|snapshot| self.item(snapshot)) else {
            return;
        };
        self.requested.retain(|key, value| match key.as_str() {
            details::ACTION_PACE => item.pace.as_ref() != Some(value),
            details::ACTION_PRESET => item.preset.as_ref() != Some(value),
            _ => true,
        });
    }

    /// What the person asked of the action `key` that waits for the engine to apply it: none once the engine
    /// refused it or applied it.
    fn queued(&self, item: &Item, key: &str, cx: &gpui::App) -> Option<&str> {
        let step = self.answers.read(cx).step(&lane_key(&item.id, key));
        let is_refused = matches!(
            step,
            Some(Step::Told {
                is_problem: true,
                ..
            })
        );
        let asked = self.requested.get(key)?;
        (!is_refused).then_some(asked.as_str())
    }

    /// Shows `view`, reading what it needs.
    fn show(&mut self, view: View, cx: &mut Context<Self>) {
        self.view = view;
        if view == View::Changes {
            self.changes.update(cx, |changes, cx| changes.refresh(cx));
        }
        cx.notify();
    }

    /// The agent is at `place`: Files shows it while following.
    pub fn agent_at(&mut self, place: Option<Location>, cx: &mut Context<Self>) {
        self.files.update(cx, |files, cx| files.agent_at(place, cx));
    }

    /// Shows Files with `path` open, as quick open picked it.
    pub fn open_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.view = View::Files;
        self.files.update(cx, |files, cx| files.open(path, cx));
        cx.notify();
    }

    #[cfg(test)]
    pub fn open_file_path(&self, cx: &gpui::App) -> Option<String> {
        self.files.read(cx).open_path(cx)
    }

    /// What quick open reads files from: the lane's folder, when it has one.
    pub fn source(&self, cx: &gpui::App) -> Option<std::rc::Rc<dyn crate::ui::files::Source>> {
        self.files.read(cx).source()
    }

    /// Sends the engine's action `key` for this lane with `inputs`.
    fn send(&mut self, key: &str, inputs: BTreeMap<String, String>, cx: &mut Context<Self>) {
        let facts = self.facts.read(cx).shared_snapshot();
        let item = facts.as_ref().and_then(|snapshot| self.item(snapshot));
        let Some((item, action)) = item.and_then(|item| Some((item, offered(item, key)?))) else {
            return;
        };
        let (answer_key, target) = (lane_key(&item.id, key), item.id.clone());
        if let Some(asked) = inputs
            .values()
            .next()
            .filter(|_| details::is_queued_kind(key))
        {
            self.requested.insert(key.to_string(), asked.clone());
        }
        self.answers.update(cx, |answers, cx| {
            answers.press(answer_key, action, target, inputs, cx)
        });
        cx.notify();
    }

    fn item<'a>(&self, snapshot: &'a Snapshot) -> Option<&'a Item> {
        snapshot
            .items
            .iter()
            .find(|item| item.lane_key() == self.lane)
    }
}

/// The action `key` of `item`, when the engine offers it for the lane.
fn offered(item: &Item, key: &str) -> Option<sakura_core::engine::snapshot::Action> {
    item.actions
        .iter()
        .find(|action| action.key == key)
        .cloned()
}

impl LaneSide {
    pub(super) fn set_compact(&mut self, is_compact: bool, cx: &mut Context<Self>) {
        self.is_compact = is_compact;
        cx.notify();
    }
}

impl Render for LaneSide {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let snapshot = self.facts.read(cx).shared_snapshot();
        let item = snapshot.as_ref().and_then(|snapshot| self.item(snapshot));
        let body = match (item, self.view) {
            (Some(item), View::Details) => {
                self.details(item, snapshot.as_deref(), &palette, window, cx)
            }
            (Some(_), View::Changes) => self.changes.clone().into_any_element(),
            (Some(_), View::Files) => self.files.clone().into_any_element(),
            (None, _) => div().into_any_element(),
        };
        div()
            .id("lane-side")
            .debug_selector(|| "lane-side".into())
            .flex_none()
            .when(self.is_compact, |side| {
                side.w_full().h(gpui::relative(
                    crate::theme::page_split::NARROW_PANE_SECONDARY_FRACTION,
                ))
            })
            .when(!self.is_compact, |side| {
                side.w(sizes::WIDTH - theme::PANEL_RING_WIDTH)
            })
            .flex()
            .flex_col()
            .child(self.switch(cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}

impl LaneSide {
    /// The choice between Details, Changes and Files.
    fn switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let options: Vec<(SharedString, bool)> = VIEWS
            .iter()
            .map(|view| (view.words(), *view == self.view))
            .collect();
        let side = cx.weak_entity();
        let choice = segments("lane-side-view", &options, move |index, _, cx| {
            drop(side.update(cx, |side, cx| side.show(VIEWS[index], cx)));
        })
        .full_width();
        div()
            .mx(sizes::SWITCH_MARGIN_X)
            .mt(sizes::SWITCH_MARGIN_TOP)
            .mb(sizes::SWITCH_MARGIN_BOTTOM)
            .child(choice)
            .into_any_element()
    }
}
