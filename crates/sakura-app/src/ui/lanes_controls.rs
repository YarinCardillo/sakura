//! The Lanes page's header controls: Filter (by project or by preset), the filter in force with a way to take
//! it off, and "New lane".

use super::engine_desk::Desk;
use super::kit::{self, chip, filter_button, ButtonKind};
use super::kit::{ScrollMenu, ScrollMenuItem};
use super::lanes::{self, LaneFilter};
use super::lanes_page::LanesPage;
use super::page_bar::PageControls;
use super::project_page::folder_name;
use crate::l10n::{tr, Key};
use crate::theme::{self, Theme};
use gpui::{
    div, AnyElement, Context, InteractiveElement as _, IntoElement, Keystroke, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, WeakEntity, Window,
};
use gpui_component::kbd::Kbd;
use gpui_kit_assets::IconName;
use sakura_core::engine::snapshot::Snapshot;

impl PageControls for LanesPage {
    /// Filter (by project or preset), the filter in force with a way to take it off, and "New lane".
    fn controls(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = self.facts.read(cx).shared_snapshot();
        let is_missing = self
            .project()
            .zip(snapshot.as_ref())
            .is_some_and(|(folder, snapshot)| snapshot.is_project_folder_missing(folder));
        let page = cx.weak_entity();
        let menu = snapshot.map(|snapshot| filter_menu(page, snapshot, self.filter.clone()));
        let palette = Theme::palette(cx);
        let filter = filter_button(
            "lanes-filter",
            tr(Key("lanes_page.filter")),
            menu,
            &palette,
            cx,
        );
        div()
            .flex()
            .items_center()
            .gap(theme::GAP)
            .children(self.clear_filter(cx))
            .child(filter)
            .child(new_lane_button(self.desk.clone(), is_missing, cx))
            .into_any_element()
    }
}

impl LanesPage {
    /// The filter in force, as a chip that takes it off when pressed.
    fn clear_filter(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.filter.is_empty() {
            return None;
        }
        let name = self.filter.project.as_deref().map(|folder| {
            let snapshot = self.facts.read(cx).snapshot();
            let project = snapshot
                .into_iter()
                .flat_map(|snapshot| &snapshot.projects)
                .find(|project| project.folder == folder);
            match project {
                Some(project) if project.folder_missing => {
                    format!("{}, {}", project.name, tr(lanes::FOLDER_MISSING))
                }
                Some(project) => project.name.clone(),
                None => folder_name(folder),
            }
        });
        let preset = self.filter.preset.as_deref().map(lanes::capitalised);
        let words: Vec<String> = name.into_iter().chain(preset).collect();
        let chip = chip(
            "lanes-clear-filter",
            words.join(", "),
            Some(IconName::Close),
            cx.listener(|page, _, _, cx| {
                page.filter = LaneFilter::default();
                cx.notify();
            }),
            cx,
        );
        Some(
            div()
                .debug_selector(|| "lanes-clear-filter".into())
                .child(chip)
                .into_any_element(),
        )
    }
}

/// The Filter button's menu: every project, then, when the lanes run as presets, every preset; the one in
/// force is checked, and the first choice of each takes the filter off.
fn filter_menu(
    page: WeakEntity<LanesPage>,
    snapshot: std::rc::Rc<Snapshot>,
    filter: LaneFilter,
) -> impl Fn(ScrollMenu, &mut Window, &mut gpui::App) -> ScrollMenu + 'static {
    move |menu, _, _| {
        let all_projects = SharedString::from(tr(Key("lanes_page.all_projects")));
        let mut menu = menu.item(choice(
            &page,
            all_projects,
            filter.project.is_none(),
            None::<String>,
            true,
        ));
        for project in &snapshot.projects {
            let is_on = filter.project.as_deref() == Some(project.folder.as_str());
            menu = menu.item(choice(
                &page,
                if project.folder_missing {
                    format!("{}, {}", project.name, tr(lanes::FOLDER_MISSING)).into()
                } else {
                    project.name.clone().into()
                },
                is_on,
                Some(project.folder.clone()),
                true,
            ));
        }
        let presets = lanes::preset_choices_of(&snapshot);
        if presets.is_empty() {
            return menu;
        }
        let all_presets = SharedString::from(tr(Key("lanes_page.all_presets")));
        let mut menu = menu.separator().item(choice(
            &page,
            all_presets,
            filter.preset.is_none(),
            None::<String>,
            false,
        ));
        for preset in presets {
            let is_on = filter.preset.as_deref() == Some(preset.name.as_str());
            menu = menu.item(choice(
                &page,
                preset.words().into(),
                is_on,
                Some(preset.name),
                false,
            ));
        }
        menu
    }
}

/// One choice of the filter menu: it sets the project (or, when not `is_project`, the preset) to `value`.
fn choice(
    page: &WeakEntity<LanesPage>,
    label: SharedString,
    is_on: bool,
    value: Option<String>,
    is_project: bool,
) -> ScrollMenuItem {
    let page = page.clone();
    ScrollMenuItem::new(label)
        .checked(is_on)
        .on_click(move |_, _, cx| {
            let value = value.clone();
            drop(page.update(cx, |page, cx| match is_project {
                true => page.show_project(value, cx),
                false => page.show_preset(value, cx),
            }));
        })
}

/// The secondary New lane action and its shortcut.
fn new_lane_button(
    desk: WeakEntity<Desk>,
    is_missing: bool,
    cx: &mut Context<LanesPage>,
) -> AnyElement {
    let palette = theme::Theme::palette(cx);
    let key = Keystroke::parse("cmd-n").map(Kbd::new).ok();
    let id = "lanes-new-lane";
    let label = tr(Key("lanes_page.new_lane"));
    let kind = ButtonKind::Secondary;
    if is_missing {
        let button = kit::button_icon_disabled(id, label, IconName::Plus, kind, &palette);
        let button = super::tip::bind(
            button.children(key),
            "tip-lanes-new-lane",
            super::tip::Words::plain(tr(lanes::FOLDER_MISSING)),
            cx,
        )
        .into_any_element();
        return super::tour::track(super::tour::Step::Lanes, theme::RADIUS_CONTROL, button);
    }
    let button = kit::button_icon(id, label, IconName::Plus, kind, &palette)
        .children(key)
        .on_click(move |_, window, cx| {
            drop(desk.update(cx, |desk, cx| desk.open_new_lane_default(window, cx)));
        });
    super::tour::track(super::tour::Step::Lanes, theme::RADIUS_CONTROL, button)
}
