//! "Projects": the repositories the engine knows, with what the engine offers on each, and the one place a
//! project is added from.

use super::group::{button, group, row, sentence};
use super::SettingsModal;
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette};
use crate::ui::chrome::text;
use crate::ui::project_add::Place;
use gpui::{
    div, AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, Styled as _,
};
use sakura_core::engine::cli::Act;
use sakura_core::engine::snapshot::Project;
use std::collections::BTreeMap;
use std::rc::Rc;

/// The action that starts a lane: the New lane sheet's, not a control of this list.
const START_LANE_KEY: &str = "L";

impl SettingsModal {
    /// The groups of "Projects".
    pub(super) fn projects(
        &mut self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let projects: Vec<Project> = self
            .snapshot(cx)
            .map(|snapshot| snapshot.projects.clone())
            .unwrap_or_default();
        let listed = if projects.is_empty() {
            sentence(tr(Key("settings.projects.none")), palette)
        } else {
            let rows = projects
                .iter()
                .map(|project| self.project_row(project, palette, cx));
            group(rows.collect(), palette)
        };
        vec![listed, self.add_group(palette, cx)]
    }

    /// Adds the repository of `folder`, as the folder panel's answer does.
    #[cfg(test)]
    pub fn add_folder(&mut self, folder: String, cx: &mut Context<Self>) {
        self.adds
            .update(cx, |adds, cx| adds.add_from(Place::Settings, folder, cx));
    }

    fn project_row(
        &self,
        project: &Project,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let offered = project
            .actions
            .iter()
            .filter(|action| action.key != START_LANE_KEY && action.needs.is_empty());
        let buttons: Vec<_> = offered
            .map(|action| {
                let press = format!("settings-project-{}-{}", project.name, action.key);
                let act = Act {
                    action: action.clone(),
                    target: project.id.clone(),
                    inputs: BTreeMap::new(),
                    confirm: None,
                };
                let acts = self.acts.clone();
                let key = press.clone();
                let on_press = Rc::new(move |_: &mut gpui::Window, cx: &mut gpui::App| {
                    let act = act.clone();
                    acts.update(cx, |acts, cx| acts.press(&key, act, "", cx));
                });
                button(
                    press,
                    SharedString::from(action.label.clone()),
                    on_press,
                    cx,
                )
            })
            .collect();
        let id = format!("settings-project-{}", project.name);
        let location = crate::ui::kit::reveal_path_icon(&id, project.folder.clone(), cx);
        let controls = div()
            .flex()
            .items_center()
            .gap(theme::GAP)
            .child(location)
            .children(buttons);
        row(
            id,
            SharedString::from(project.name.clone()),
            None,
            controls,
            None,
            palette,
        )
    }

    fn add_group(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let adds = self.adds.clone();
        let choose = Rc::new(move |window: &mut gpui::Window, cx: &mut gpui::App| {
            adds.update(cx, |adds, cx| adds.choose(Place::Settings, window, cx));
        });
        let add = button(
            "settings-add-project",
            tr(Key("settings.projects.add")),
            choose,
            cx,
        );
        let refusal = self
            .adds
            .read(cx)
            .refusal_in(Place::Settings)
            .map(|refusal| {
                text(theme::TEXT_LABEL, palette.danger)
                    .debug_selector(|| "settings-add-project-refused".into())
                    .child(refusal.clone())
                    .into_any_element()
            });
        let note = tr(Key("settings_modal.projects.add_note")).into();
        let name = tr(Key("settings_modal.projects.add_title"));
        let rows = vec![row(
            "settings-row-add-project",
            name,
            Some(note),
            add,
            refusal,
            palette,
        )];
        group(rows, palette)
    }
}
