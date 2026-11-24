//! The page of one project: where it is on this Mac (its folder and its branch), the lanes working in it and
//! the way to start one. The branch comes from the project's engine snapshot.

use super::chrome::text;
use super::engine_desk::Desk;
use super::lanes;
use super::sidebar_pages::go_from;
use crate::data::engine_facts::EngineFacts;
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette, Theme};
use gpui::{
    div, AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, WeakEntity, Window,
};
use sakura_core::engine::snapshot::{Item, Project};
use sakura_core::tabs::{Page, TabId};
use std::path::Path;

/// The page of the project in one folder.
pub struct ProjectPage {
    desk: WeakEntity<Desk>,
    facts: Entity<EngineFacts>,
    folder: String,
    tab: TabId,
    _observer: Subscription,
}

impl ProjectPage {
    /// Shows one project using the engine snapshot.
    pub fn new(
        desk: WeakEntity<Desk>,
        tab: TabId,
        facts: Entity<EngineFacts>,
        folder: &str,
        cx: &mut Context<Self>,
    ) -> Self {
        let observer = cx.observe(&facts, |_, _, cx| cx.notify());
        Self {
            desk,
            tab,
            facts,
            folder: folder.to_string(),
            _observer: observer,
        }
    }

    #[cfg(test)]
    pub(super) fn folder_for_test(&self) -> &str {
        &self.folder
    }
}

impl Render for ProjectPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let snapshot = self.facts.read(cx).shared_snapshot();
        let project: Option<Project> = snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .projects
                .iter()
                .find(|project| project.folder == self.folder)
                .cloned()
        });
        let lanes_of: Vec<Item> = snapshot
            .iter()
            .flat_map(|snapshot| snapshot.items.iter())
            .filter(|item| item.repo == self.folder && lanes::is_open(item))
            .cloned()
            .collect();
        let is_missing = project
            .as_ref()
            .is_some_and(|project| project.folder_missing);
        let branch = project
            .as_ref()
            .and_then(|project| project.branch.as_deref());
        let facts = self.facts_block(branch, &palette, cx);
        let new_lane = project.filter(|project| project.start_lane().is_some());
        div()
            .debug_selector(|| "project-page".into())
            .size_full()
            .pl(theme::page_bar::HEADER_PADDING_START)
            .pr(theme::page_bar::BAR_PADDING_END)
            .pt(theme::GAP_WIDE)
            .flex()
            .flex_col()
            .gap(theme::GAP_WIDE)
            .child(facts)
            .children(is_missing.then(|| {
                div()
                    .text_color(palette.text_secondary)
                    .child(tr(super::lanes::FOLDER_MISSING))
            }))
            .children(new_lane.map(|project| self.new_lane_button(project, cx)))
            .child(self.lanes_block(&lanes_of, &palette, cx))
    }
}

impl ProjectPage {
    /// Its folder and its branch.
    fn facts_block(&self, branch: Option<&str>, palette: &Palette, cx: &gpui::App) -> AnyElement {
        let line = |selector: &'static str, label: Key, value: AnyElement| {
            div()
                .debug_selector(move || selector.into())
                .flex()
                .items_center()
                .gap(theme::GAP_WIDE)
                .text_size(theme::TEXT_SMALL)
                .child(
                    div()
                        .w(crate::theme::project_page::LABEL_COLUMN)
                        .text_color(palette.text_tertiary)
                        .child(tr(label)),
                )
                .child(div().min_w_0().text_color(palette.text).child(value))
        };
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(line(
                "project-page-folder",
                Key("project_page.folder"),
                crate::ui::kit::reveal_path("project-page-folder", self.folder.clone(), cx),
            ))
            .child(line(
                "project-page-branch",
                Key("project_page.branch"),
                div()
                    .child(
                        branch
                            .filter(|branch| !branch.is_empty())
                            .unwrap_or_else(|| tr(Key("project_page.branch_unavailable")))
                            .to_string(),
                    )
                    .into_any_element(),
            ))
            .into_any_element()
    }

    /// "New lane", which starts one in this project.
    fn new_lane_button(&self, project: Project, cx: &mut Context<Self>) -> AnyElement {
        let desk = self.desk.clone();
        let tab = self.tab;
        let source = Page::Project(self.folder.clone());
        let press = std::rc::Rc::new(move |window: &mut Window, cx: &mut gpui::App| {
            let project = project.clone();
            drop(desk.update(cx, |desk, cx| {
                if desk.is_visible_page(tab, &source, cx) {
                    desk.open_new_lane(project, window, cx);
                }
            }));
        });
        div()
            .flex()
            .child(crate::ui::settings_modal::controls::primary_button(
                "project-page-new-lane",
                tr(Key("sidebar.new_lane")),
                press,
                cx,
            ))
            .into_any_element()
    }

    /// The lanes working in the project, each a row that opens it; a line says so when there is none.
    fn lanes_block(&self, found: &[Item], palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = match found.is_empty() {
            true => vec![text(theme::TEXT_SMALL, palette.text_tertiary)
                .child(tr(Key("project_page.no_lanes")))
                .into_any_element()],
            false => found
                .iter()
                .enumerate()
                .map(|(at, item)| self.lane_row(at, item, palette))
                .collect(),
        };
        let _ = cx;
        div()
            .debug_selector(|| "project-page-lanes".into())
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(
                text(theme::TEXT_LABEL, palette.text_tertiary).child(tr(Key("project_page.lanes"))),
            )
            .children(rows)
            .into_any_element()
    }

    fn lane_row(&self, at: usize, item: &Item, palette: &Palette) -> AnyElement {
        let selector = format!("project-page-lane-{at}");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .h(theme::CONTROL_HEIGHT)
            .flex()
            .items_center()
            .gap(theme::GAP)
            .text_size(theme::TEXT_SMALL)
            .text_color(palette.text)
            .on_click(go_from(
                self.desk.clone(),
                self.tab,
                Page::Project(self.folder.clone()),
                Page::Lane(item.lane_key()),
                false,
            ))
            .on_aux_click(go_from(
                self.desk.clone(),
                self.tab,
                Page::Project(self.folder.clone()),
                Page::Lane(item.lane_key()),
                true,
            ))
            .child(lanes::glyph_of(item).paint(crate::theme::kit::scale::ICON_REGULAR, palette))
            .child(SharedString::from(lanes::lane_words(item, false)))
            .into_any_element()
    }
}

/// The name of the folder: its last part.
pub(super) fn folder_name(folder: &str) -> String {
    Path::new(folder)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder.to_string())
}
