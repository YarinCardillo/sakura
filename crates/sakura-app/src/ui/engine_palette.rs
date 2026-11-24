//! The command palette in the engine's window: where the window is when Cmd+K is pressed, and what the desk
//! does for a command only it can carry out. A command on a lane or an ask names it; the palette checks it
//! against the window as it is when it is chosen. The verbs of an ask are sent through the same [`Answers`]
//! runner the Inbox's buttons use.
//!
//! [`Answers`]: super::answers::Answers

use super::engine_desk::Desk;
use super::lanes::{self, LaneFilter};
use super::palette::engine::{LaneRef, ProjectRef, SelectedAsk};
use super::palette::{self, Ask, Palette, PaletteEvent, Situation};
use gpui::{App, AppContext as _, Context, Entity, Subscription, Window};
use sakura_core::engine::snapshot::Snapshot;
use sakura_core::tabs::Page;
use std::collections::BTreeMap;

/// The key of the engine action that stops a lane.
const STOP_KEY: &str = "s";

impl Desk {
    /// The palette of this window: Cmd+K lists the commands for where the window is at that moment. The
    /// subscription carries what it asks of the desk.
    pub(super) fn palette_in(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<Palette>, Subscription) {
        let palette = cx.new(|cx| Palette::new(window, cx));
        let desk = cx.weak_entity();
        let situation = move |cx: &App| {
            let desk = desk.upgrade().map(|desk| desk.read(cx).situation(cx));
            desk.unwrap_or_else(|| nowhere(cx))
        };
        palette::install(&palette, situation, window, cx);
        let asked = cx.subscribe_in(
            &palette,
            window,
            |desk, _, event: &PaletteEvent, window, cx| {
                let PaletteEvent::Asked(ask) = event;
                desk.palette_asked(ask, window, cx);
            },
        );
        (palette, asked)
    }

    /// Where the window is: the page shown, every lane and project, what the page offers.
    pub fn situation(&self, cx: &App) -> Situation {
        let page = self.bar.read(cx).tabs().active().page().clone();
        let snapshot = self.facts.read(cx).snapshot();
        Situation {
            has_scheduled: snapshot.is_some_and(|snapshot| snapshot.scheduled.is_some()),
            can_create_track: snapshot
                .is_some_and(|snapshot| snapshot.tracks.action("tracknew").is_some()),
            lanes: snapshot.map(palette_lanes).unwrap_or_default(),
            projects: snapshot.map(palette_projects).unwrap_or_default(),
            tracks: snapshot.map_or_else(Vec::new, |snapshot| {
                snapshot
                    .tracks
                    .rows()
                    .iter()
                    .map(|track| (track.id.clone(), super::track_page::name(track).to_string()))
                    .collect()
            }),
            stoppable: snapshot.and_then(|snapshot| stoppable(&page, snapshot)),
            selected_ask: self.inbox_selected_ask(cx),
            can_close_tab: self.bar.read(cx).tabs().tabs().len() > 1,
            panel_left: self.panel_left_now(),
            page,
        }
    }

    /// The ask the Inbox has selected, with the verbs that answer it: only while the Inbox is the page shown.
    pub(super) fn inbox_selected_ask(&self, cx: &App) -> Option<SelectedAsk> {
        self.active_inbox(cx)?.read(cx).selected_ask_for_palette(cx)
    }

    /// Carries out what the palette asked, on the page, lane or ask it names.
    pub(super) fn palette_asked(&mut self, ask: &Ask, window: &mut Window, cx: &mut Context<Self>) {
        match ask {
            Ask::Go { page, is_new_tab } => self.navigate(page.clone(), *is_new_tab, cx),
            Ask::OpenProject { folder, is_new_tab } => self.open_lanes_of(folder, *is_new_tab, cx),
            Ask::StopLane(lane) => self.stop_lane(lane, cx),
            Ask::NewTrack => self.open_new_track(window, cx),
            Ask::Answer { wait, verb } => self.answer_ask(wait, verb, window, cx),
        }
    }

    /// Cmd+P: the Files quick open of the lane on screen. Nothing on any other page.
    pub(super) fn quick_open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.quick_open_target(cx);
        self.quick_open
            .update(cx, |quick_open, cx| quick_open.toggle(target, window, cx));
    }

    /// The lane Cmd+P would search the files of: the one on screen, none on another page.
    pub(super) fn quick_open_lane(&self, cx: &App) -> Option<String> {
        match self.bar.read(cx).tabs().active().page() {
            Page::Lane(lane) => Some(lane.clone()),
            _ => None,
        }
    }

    fn stop_lane(&mut self, lane: &str, cx: &mut Context<Self>) {
        let snapshot = self.facts.read(cx).snapshot();
        let found = snapshot.and_then(|snapshot| {
            let item = snapshot.items.iter().find(|item| item.lane_key() == lane)?;
            let stop = item.actions.iter().find(|action| action.key == STOP_KEY)?;
            Some((item.id.clone(), stop.clone()))
        });
        let Some((id, action)) = found else {
            return eprintln!("sakura: the lane {lane:?} no longer offers to stop");
        };
        let key = super::answers::lane_key(&id, &action.key);
        self.answers.update(cx, |answers, cx| {
            answers.press(key, action, id, BTreeMap::new(), cx)
        });
    }

    /// A verb of the selected ask goes through the Inbox, so it is held under the Undo toast like its buttons.
    fn answer_ask(&mut self, wait: &str, verb: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(inbox) = self.active_inbox(cx) {
            inbox.update(cx, |page, cx| page.answer_with_verb(wait, verb, window, cx));
        }
    }
}

/// Every lane of `snapshot`, those that need the person first.
pub(super) fn palette_lanes(snapshot: &Snapshot) -> Vec<LaneRef> {
    let found = lanes::grouped(snapshot, &LaneFilter::default());
    let ordered = lanes::LaneGroup::ALL
        .into_iter()
        .flat_map(|group| found.of(group).iter().map(move |item| (group, *item)));
    ordered
        .map(|(_, item)| LaneRef {
            key: item.lane_key(),
            name: item.name().to_string(),
            glyph: lanes::glyph_of(item),
        })
        .collect()
}

fn palette_projects(snapshot: &Snapshot) -> Vec<ProjectRef> {
    let groups = lanes::by_project(snapshot);
    let named = |group: lanes::ProjectGroup<'_>| ProjectRef {
        folder: group.folder,
        name: group.name,
    };
    groups.into_iter().map(named).collect()
}

/// The lane on screen, when the engine offers to stop it.
fn stoppable(page: &Page, snapshot: &Snapshot) -> Option<String> {
    let Page::Lane(lane) = page else {
        return None;
    };
    let item = snapshot
        .items
        .iter()
        .find(|item| item.lane_key() == *lane)?;
    item.actions
        .iter()
        .any(|action| action.key == STOP_KEY)
        .then(|| lane.clone())
}

/// The window with no desk left: nothing to go to and nothing shown.
fn nowhere(_: &App) -> Situation {
    Situation {
        has_scheduled: false,
        can_create_track: false,
        page: Page::Inbox,
        lanes: Vec::new(),
        projects: Vec::new(),
        tracks: Vec::new(),
        stoppable: None,
        selected_ask: None,
        can_close_tab: false,
        panel_left: gpui::Pixels::ZERO,
    }
}
