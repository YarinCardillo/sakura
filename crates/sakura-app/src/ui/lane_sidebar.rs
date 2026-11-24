//! The sidebar of the engine's window. It starts right under the title bar and has no footer. From the top:
//! the pages (Inbox with the count of open asks, Master, Lanes with the count of open lanes, Engine with its
//! pace word), then the "Projects" heading with the button that starts a lane, then each project the engine
//! knows, expandable to its open lanes: the lane's state mark, its title and a thin progress line.
//!
//! A press on a row shows its page in the tab on screen, with Cmd in a tab of its own. The rows of the
//! pages are in `sidebar_pages`, the projects' in `sidebar_projects`.

use super::engine_desk::Desk;
use super::inbox::InboxPage;
use super::lanes;
use super::project_add::{Place, ProjectAdds};
use crate::data::engine_facts::EngineFacts;
use crate::l10n::{tr, Key};
use crate::prefs::Preferences;
use crate::theme::{self, sidebar as tokens, Palette, Theme};
use gpui::{
    div, AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, WeakEntity, Window,
};
use sakura_core::engine::snapshot::{Item, Snapshot};
use sakura_core::tabs::Page;
use std::collections::BTreeSet;

/// What the heading says: the list below it is of projects, each with its lanes.
const PROJECTS: Key = Key("sidebar.projects");
/// The line of an empty list.
const NO_PROJECTS: Key = Key("sidebar.no_projects");
/// What the heading's button does.
const ADD_PROJECT: Key = Key("sidebar.add_project");

/// What the page rows count and say, read from one snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Counts {
    /// The open asks: what waits for the person, the Inbox's own number.
    pub asks: usize,
    /// The lanes that are open.
    pub lanes: usize,
    /// The engine's pace in words; none when it reports none.
    pub pace: Option<SharedString>,
}

/// The paces an engine reports, slowest first, with the word of each.
const PACES: [(&str, Key); 5] = [
    ("slowest", Key("pace.slowest")),
    ("slow", Key("pace.slow")),
    ("normal", Key("pace.normal")),
    ("fast", Key("pace.fast")),
    ("fastest", Key("pace.fastest")),
];

/// The counts of `snapshot`; all empty before the first one is read.
pub(super) fn counts(snapshot: Option<&Snapshot>) -> Counts {
    let Some(snapshot) = snapshot else {
        return Counts::default();
    };
    Counts {
        asks: snapshot.active_waits(super::unix_now()).count(),
        lanes: lanes::open_count(&snapshot.items),
        pace: snapshot.pace.as_deref().and_then(pace_words),
    }
}

/// The word of a pace the engine reported; none for one this app has no word for.
fn pace_words(pace: &str) -> Option<SharedString> {
    let (_, key) = PACES.iter().find(|(name, _)| *name == pace)?;
    Some(tr(*key).into())
}

/// The sidebar as its own view, drawn again only when the engine's facts, the open page or the closed
/// projects change.
pub struct LaneSidebar {
    pub(super) desk: WeakEntity<Desk>,
    facts: Entity<EngineFacts>,
    /// The Inbox page: its answers held or just sent are no longer asks that wait.
    inbox: Entity<InboxPage>,
    /// Adding a project: the folder panel, the engine's answer and the way to make a folder a repository.
    adds: Entity<ProjectAdds>,
    retino_mask: super::retino::SidebarMask,
    pub(super) current: Page,
    /// Whether Option is held: the rows name their lanes by branch.
    pub(super) is_identity: bool,
    pub(super) collapsed: BTreeSet<String>,
    pub(super) has_expanded_closed_tracks: bool,
    _observers: Vec<Subscription>,
}

impl LaneSidebar {
    /// The sidebar of `desk` over `facts` and the Inbox's answers, with the row of the page `current` filled
    /// and the projects in `collapsed` (by folder) closed.
    pub fn new(
        desk: WeakEntity<Desk>,
        (facts, inbox, adds): (Entity<EngineFacts>, Entity<InboxPage>, Entity<ProjectAdds>),
        (current, collapsed): (Page, BTreeSet<String>),
        retino_mask: super::retino::SidebarMask,
        cx: &mut Context<Self>,
    ) -> Self {
        let observers = vec![
            cx.observe(&facts, |_, _, cx| cx.notify()),
            cx.observe(&inbox, |_, _, cx| cx.notify()),
            cx.observe(&adds, |_, _, cx| cx.notify()),
        ];
        Self {
            desk,
            facts,
            inbox,
            adds,
            retino_mask,
            current,
            is_identity: false,
            collapsed,
            has_expanded_closed_tracks: false,
            _observers: observers,
        }
    }

    /// Names the lanes by branch while Option is held, and by name otherwise.
    pub fn set_identity(&mut self, is_identity: bool, cx: &mut Context<Self>) {
        if self.is_identity != is_identity {
            self.is_identity = is_identity;
            cx.notify();
        }
    }

    /// Fills the row of `page` from the next frame.
    pub fn set_current(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.current != page {
            self.current = page;
            cx.notify();
        }
    }

    /// Closes the project in `folder` if it is open, and opens it if it is closed; the choice is kept for
    /// the next launch.
    pub(super) fn toggle_project(&mut self, folder: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(folder) {
            self.collapsed.insert(folder.to_string());
        }
        let collapsed = self.collapsed.clone();
        Preferences::change(cx, move |preferences| {
            preferences.collapsed_projects = collapsed
        });
        cx.notify();
    }
}

impl Render for LaneSidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::snapshot::count_render("sidebar");
        let palette = Theme::palette(cx);
        let snapshot = self.facts.read(cx).shared_snapshot();
        let mut counted = counts(snapshot.as_deref());
        if let Some(snapshot) = &snapshot {
            counted.asks = self.inbox.read(cx).open_asks(snapshot, cx);
        }
        div()
            .w(theme::SIDEBAR_WIDTH)
            .h_full()
            .flex_none()
            .px(tokens::PADDING_X)
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .bg(palette.canvas)
                    .child(self.pages(&counted, &palette))
                    .child(self.heading(&palette, cx)),
            )
            .child(self.projects(snapshot.as_deref(), &palette, cx))
    }
}

impl LaneSidebar {
    /// The "Projects" heading, and at its end the one control that adds a project: a plain plus that shows while
    /// the pointer is on the heading. A lane is started from a project's own row, never from here.
    fn heading(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let hover_group = SharedString::from("sidebar-projects-heading-group");
        div()
            .debug_selector(|| "sidebar-projects-heading".into())
            .group(hover_group.clone())
            .h(tokens::HEADING_HEIGHT)
            .flex_none()
            .mt(tokens::HEADING_MARGIN_TOP)
            .mb(tokens::HEADING_MARGIN_BOTTOM)
            .px(tokens::PADDING_X)
            .flex()
            .items_center()
            .justify_between()
            .text_size(theme::TEXT_LABEL)
            .text_color(palette.text_tertiary)
            .child(tr(PROJECTS))
            .child(
                div()
                    .opacity(0.)
                    .group_hover(hover_group, |style| style.opacity(1.))
                    .child(self.add_project_button(cx)),
            )
            .into_any_element()
    }

    /// The heading's button that adds a project: the Mac's folder panel, then the engine.
    fn add_project_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let adds = self.adds.clone();
        super::pane_toggle::press_button(
            ("sidebar-add-project".into(), "sidebar-add-project-icon"),
            (gpui_kit_assets::IconName::Plus, tokens::HEADING_BUTTON),
            tr(ADD_PROJECT),
            move |window, cx| adds.update(cx, |adds, cx| adds.choose(Place::Sidebar, window, cx)),
            cx,
        )
    }

    /// What stands where the projects will be while there is none: one line on what a project is, the button
    /// that adds one, and under it the engine's reason when the folder was refused, with the offer to make it a
    /// Git repository, which is only an offer.
    fn no_project(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let pressed = self.adds.clone();
        let add = super::settings_modal::controls::button(
            "sidebar-add-project-empty",
            tr(ADD_PROJECT),
            std::rc::Rc::new(move |window: &mut Window, cx: &mut gpui::App| {
                pressed.update(cx, |adds, cx| adds.choose(Place::Sidebar, window, cx))
            }),
            cx,
        );
        div()
            .px(tokens::PADDING_X)
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(
                div()
                    .debug_selector(|| "sidebar-no-projects".into())
                    .text_size(theme::TEXT_SMALL)
                    .text_color(palette.text_tertiary)
                    .child(tr(NO_PROJECTS)),
            )
            .child(div().flex().child(add))
            .children(self.adding_notice(palette, cx))
            .into_any_element()
    }

    /// The engine's reason when the folder was refused, with the offer to make it a Git repository when it is
    /// none (an offer: nothing is made until it is pressed); none while nothing was refused.
    fn adding_notice(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let adds = self.adds.clone();
        let (refusal, init) = {
            let adds = adds.read(cx);
            (
                adds.refusal_in(Place::Sidebar).cloned()?,
                adds.could_be_made_a_repository().is_some(),
            )
        };
        let make = init.then(|| {
            super::settings_modal::controls::button(
                "sidebar-init-project",
                tr(Key("project_add.init")),
                std::rc::Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                    adds.update(cx, |adds, cx| adds.make_a_repository_and_add(cx))
                }),
                cx,
            )
        });
        let notice = div()
            .px(tokens::PADDING_X)
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(
                div()
                    .debug_selector(|| "sidebar-add-project-refusal".into())
                    .text_size(theme::TEXT_SMALL)
                    .text_color(palette.danger)
                    .child(refusal),
            )
            .children(make.map(|make| div().flex().child(make)));
        Some(notice.into_any_element())
    }

    /// The projects in order, each a row over its open lanes; a line says so when there is none.
    fn projects(
        &self,
        snapshot: Option<&Snapshot>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let list = div()
            .id("sidebar-projects")
            .flex_1()
            .min_h_0()
            .pb(tokens::HEADING_MARGIN_TOP)
            .overflow_y_scroll();
        let Some(snapshot) = snapshot else {
            return list.into_any_element();
        };
        let content = div()
            .debug_selector(|| "sidebar-projects-content".into())
            .relative()
            .child(self.retino_mask.measure())
            .bg(palette.canvas)
            .flex()
            .flex_col()
            .gap(tokens::ROW_PITCH_GAP);
        let groups = lanes::by_project(snapshot);
        let outside = lanes::outside_lanes(snapshot);
        if groups.is_empty() && outside.is_empty() {
            return list
                .child(
                    content
                        .child(self.no_project(palette, cx))
                        .children(self.track_section(&snapshot.tracks, palette, cx)),
                )
                .into_any_element();
        }
        let (registered, unregistered): (Vec<_>, Vec<_>) =
            groups.iter().partition(|group| group.project.is_some());
        // A folder only agents outside the engine work in is no project: its lanes stand under "From outside".
        let outside_items: Vec<&Item> = unregistered
            .iter()
            .flat_map(|group| group.lanes.iter().copied())
            .chain(outside)
            .collect();
        // Under "Projects" there is a project or the line that says there is none, never nothing.
        // With no project the line, the button and the reason are one block; with some, the reason stands alone.
        let none = match registered.is_empty() {
            true => Some(self.no_project(palette, cx)),
            false => self.adding_notice(palette, cx),
        };
        let outside = self.outside(&outside_items, palette, cx);
        let rows: Vec<AnyElement> = none
            .into_iter()
            .chain(
                registered
                    .iter()
                    .flat_map(|group| self.project(group, palette, cx)),
            )
            .chain(outside)
            .collect();
        list.child(content.children(rows).children(self.track_section(
            &snapshot.tracks,
            palette,
            cx,
        )))
        .into_any_element()
    }
}
