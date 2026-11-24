//! The projects of the sidebar: a row for each, that opens or closes the lanes under it, and a row for each
//! open lane with its state mark, its title and a thin line of its progress.

use super::kit::progress_line;
use super::lane_sidebar::LaneSidebar;
use super::lanes::{self, ProjectGroup};
use super::sidebar_pages::{frame, go, go_aux, marked};
use crate::icons;
use crate::l10n::{tr, Key};
use crate::theme::{self, sidebar as tokens, Palette};
use gpui::prelude::FluentBuilder as _;
use gpui::Context;
use gpui::{
    div, AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;
use sakura_core::engine::snapshot::{Item, Project};
use sakura_core::tabs::Page;

/// What the controls of a project's row do.
const NEW_LANE: Key = Key("sidebar.new_lane");
const SHOW_LANES: Key = Key("sidebar.show_lanes");

impl LaneSidebar {
    /// A project's row and, while it is open, a row for each of its open lanes.
    pub(super) fn project(
        &self,
        group: &ProjectGroup<'_>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let is_open = !self.collapsed.contains(&group.folder);
        let mut rows = vec![self.project_row(group, is_open, palette, cx)];
        if is_open {
            let open = group.lanes.iter().filter(|item| lanes::is_open(item));
            rows.extend(open.map(|item| self.lane_row(item, palette, cx)));
        }
        rows
    }

    /// The name opens the project page. The rest of the row folds its lanes when it has any to show.
    fn project_row(
        &self,
        group: &ProjectGroup<'_>,
        is_open: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_lanes = group.lanes.iter().any(|item| lanes::is_open(item));
        let chevron_selector = format!("project-chevron-{}", group.name);
        let disclosure = div().flex_none().size(tokens::CHEVRON_SIZE);
        let disclosure = disclosure.when_some(project_chevron(is_open, has_lanes), |slot, icon| {
            slot.debug_selector(move || chevron_selector)
                .text_color(palette.text_tertiary)
                .child(Icon::new(icon).with_size(tokens::CHEVRON_SIZE))
        });
        let name_selector = format!("group-{}", group.name);
        let folder = group.folder.clone();
        let open_folder = group.folder.clone();
        let hover_group = SharedString::from(format!("project-group-{}", group.folder));
        let row = frame(
            &format!("project-{}", group.name),
            tokens::ROW_HEIGHT,
            false,
            palette,
        )
        .group(hover_group.clone())
        .px(tokens::PADDING_X)
        .gap(tokens::ROW_GAP)
        .text_size(tokens::ROW_TEXT)
        .when(has_lanes, |row| {
            row.on_click(cx.listener(move |sidebar, _, _, cx| sidebar.toggle_project(&folder, cx)))
        })
        .child(disclosure)
        .child(
            div()
                .id(SharedString::from(format!("project-name-{}", group.folder)))
                .debug_selector(move || name_selector)
                .flex_1()
                .min_w_0()
                .truncate()
                .on_click(cx.listener(move |sidebar, _, _, cx| {
                    cx.stop_propagation();
                    drop(sidebar.desk.update(cx, |desk, cx| {
                        desk.navigate(Page::Project(open_folder.clone()), false, cx)
                    }));
                }))
                .child(group.name.clone()),
        );
        let missing = group
            .project
            .filter(|project| project.folder_missing)
            .map(|project| {
                let selector = format!("sidebar-folder-missing-{}", project.name);
                div()
                    .debug_selector(move || selector)
                    .flex_none()
                    .text_size(tokens::COUNT_TEXT)
                    .text_color(palette.text_secondary)
                    .child(tr(lanes::FOLDER_MISSING))
            });
        let controls = div()
            .opacity(0.)
            .group_hover(hover_group, |style| style.opacity(1.))
            .flex()
            .child(self.project_lanes(&group.folder, &group.name, cx))
            .children(
                group
                    .project
                    .filter(|project| project.start_lane().is_some())
                    .map(|project| self.project_new_lane(project.clone(), cx)),
            );
        row.children(missing).child(controls).into_any_element()
    }

    /// The control of a project's row that shows the Lanes page kept to its lanes.
    fn project_lanes(&self, folder: &str, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let (desk, folder) = (self.desk.clone(), folder.to_string());
        let id = SharedString::from(format!("lanes-of-{name}"));
        super::pane_toggle::press_button(
            (id, "lanes-of-icon"),
            (icons::LANES_PAGE, theme::STATUS_CONTROL_HEIGHT),
            tr(SHOW_LANES),
            move |_, cx| {
                let folder = folder.clone();
                drop(desk.update(cx, |desk, cx| desk.open_lanes_of(&folder, false, cx)));
            },
            cx,
        )
    }

    /// The control of a project's row that starts a lane in it.
    fn project_new_lane(&self, project: Project, cx: &mut Context<Self>) -> AnyElement {
        let desk = self.desk.clone();
        let id = SharedString::from(format!("new-lane-{}", project.name));
        super::pane_toggle::press_button(
            (id, "new-lane-icon"),
            (IconName::Plus, theme::STATUS_CONTROL_HEIGHT),
            tr(NEW_LANE),
            move |window, cx| {
                let project = project.clone();
                drop(desk.update(cx, |desk, cx| desk.open_new_lane(project, window, cx)));
            },
            cx,
        )
    }

    /// The lanes no project owns, under a heading of their own: an outside agent's lanes are not a project, and
    /// no lane can be started there.
    pub(super) fn outside(
        &self,
        found: &[&Item],
        palette: &Palette,
        cx: &gpui::App,
    ) -> Vec<AnyElement> {
        let open: Vec<&&Item> = found.iter().filter(|item| lanes::is_open(item)).collect();
        if open.is_empty() {
            return Vec::new();
        }
        let heading = div()
            .debug_selector(|| "sidebar-outside".into())
            .h(tokens::HEADING_HEIGHT)
            .flex_none()
            .mt(tokens::HEADING_MARGIN_TOP)
            .mb(tokens::HEADING_MARGIN_BOTTOM)
            .px(tokens::PADDING_X)
            .flex()
            .items_center()
            .text_size(theme::TEXT_LABEL)
            .text_color(palette.text_tertiary)
            .child(tr(Key("sidebar.outside")))
            .into_any_element();
        let rows = open
            .into_iter()
            .map(|item| self.lane_row(item, palette, cx));
        std::iter::once(heading).chain(rows).collect()
    }

    /// One open lane: its mark, its title and, when the engine has a plan for it, the line of its progress.
    pub(super) fn lane_row(&self, item: &Item, palette: &Palette, cx: &gpui::App) -> AnyElement {
        let task = item.task().to_string();
        let lane = item.lane_key();
        let is_current = self.current == Page::Lane(lane.clone());
        let (mark_selector, name_selector) = (format!("glyph-{task}"), format!("name-{task}"));
        let mark = div()
            .debug_selector(move || mark_selector)
            .flex_none()
            .child(lanes::glyph_of(item).paint(tokens::LANE_MARK_SIZE, palette));
        let words: SharedString = lanes::lane_words(item, self.is_identity)
            .replace(['\n', '\r'], " ")
            .into();
        let title = div()
            .id(SharedString::from(name_selector.clone()))
            .debug_selector(move || name_selector)
            .relative()
            .flex_1()
            .min_w_0()
            .truncate()
            .child(words.clone());
        let title = super::tip::sidebar_label(title, format!("tip-lane-{task}").into(), words, cx);
        let row = frame(
            &format!("row-{task}"),
            tokens::LANE_HEIGHT,
            is_current,
            palette,
        )
        .pl(tokens::LANE_PADDING_START)
        .pr(tokens::PADDING_X)
        .gap(tokens::ROW_GAP)
        .text_size(tokens::LANE_TEXT)
        .on_click(go(self.desk.clone(), Page::Lane(lane.clone())))
        .on_aux_click(go_aux(self.desk.clone(), Page::Lane(lane)))
        .child(mark)
        .child(title)
        .children(line(item, &task));
        marked(row, is_current)
    }
}

/// The disclosure icon for a project with lanes to show.
pub(super) fn project_chevron(is_open: bool, has_lanes: bool) -> Option<IconName> {
    has_lanes.then_some(match is_open {
        true => IconName::ChevronDown,
        false => IconName::ChevronRight,
    })
}

/// The thin line of a lane's progress; none for a lane the engine has no plan for, and none while nothing is
/// done, when an empty line reads as a divider.
fn line(item: &Item, task: &str) -> Option<AnyElement> {
    let progress = item
        .progress
        .as_ref()
        .filter(|progress| progress.total > 0 && progress.done > 0)?;
    let selector = format!("row-{task}-bar");
    let element = div()
        .debug_selector(move || selector)
        .flex_none()
        .child(progress_line(
            progress.done,
            progress.total,
            tokens::LANE_LINE_WIDTH,
        ));
    Some(element.into_any_element())
}
