//! The New lane sheet on screen: the project it starts in and a title over the description, then a line of
//! chips (preset and pace, when the engine takes them) with Start at its right end.

use super::dialog_frame;
use super::kit::{self, menu_chip};
use super::lane_paces::LANE_PACES;
use super::new_lane::{
    NewLaneSheet, Step, DESCRIPTION_LABEL, PACE_INPUT, PRESET_INPUT, TITLE_LABEL, TRACK_INPUT,
};
use crate::l10n::{tr, tr_with, Key};
use crate::theme::new_lane as tokens;
use crate::theme::{self, Palette, Theme};
use gpui::prelude::FluentBuilder as _;
use gpui::StatefulInteractiveElement as _;
use gpui::{
    div, AnyElement, Context, InteractiveElement as _, IntoElement, Keystroke, ParentElement,
    Render, SharedString, Styled, Window,
};
use gpui_component::button::Button;
use gpui_component::input::Textarea;
use gpui_component::kbd::Kbd;
use gpui_component::menu::DropdownMenu as _;
use gpui_component::{Disableable as _, Sizable as _, Size, WindowExt as _};

/// The sheet's name, and Start.
const HEADING: Key = Key("new_lane.heading");
const START: Key = Key("new_lane.start");
/// A chip's name before its choice: "Preset: Careful".
const PRESET: Key = Key("new_lane.preset");
const PACE: Key = Key("new_lane.pace");
/// What a preset chip says while no preset was chosen: the engine picks.
const PRESET_DEFAULT: Key = Key("new_lane.preset_default");
/// The paces an engine reports, slowest first, with the word of each.
/// The pace a lane starts at when none is chosen: the lane's own, whatever pace the engine as a whole runs at.
pub(super) const LANE_DEFAULT_PACE: &str = "normal";

/// The pace a new lane will start with, given the one chosen (none when nothing is).
pub(super) fn pace_in_force(chosen: Option<&str>) -> &str {
    chosen.unwrap_or(LANE_DEFAULT_PACE)
}

impl Render for NewLaneSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        dialog_frame::surface("new-lane-sheet", window, &palette)
            .child(dialog_frame::heading(
                "new-lane-heading",
                tr(HEADING),
                &palette,
            ))
            .child(self.project_control(cx))
            .child(self.fields(&palette))
            .child(self.footer(&palette, cx))
    }
}

/// The project the menu checks: the one the chip names, none while the chip asks for one.
fn checked_project(is_asking_project: bool, project: usize) -> Option<usize> {
    (!is_asking_project).then_some(project)
}

impl NewLaneSheet {
    fn project_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let name = match self.is_asking_project {
            true => SharedString::from(tr(Key("new_lane.choose_project"))),
            false => SharedString::from(self.current().name.clone()),
        };
        let button = kit::menu_chip_at_width(
            "new-lane-project",
            name,
            theme::settings::CHOICE_MENU_WIDTH,
            cx,
        );
        let project = self.project_menu(button, cx);
        let palette = Theme::palette(cx);
        let row = super::settings_modal::group::row(
            "new-lane-project-row",
            tr(Key("inbox.filter.project")),
            None,
            project,
            None,
            &palette,
        );
        div()
            .flex_none()
            .when(self.is_asking_project, |project| {
                project.debug_selector(|| "new-lane-asking-project".into())
            })
            .child(super::settings_modal::group::group(vec![row], &palette))
            .into_any_element()
    }

    fn project_menu(&self, button: Button, cx: &mut Context<Self>) -> AnyElement {
        let sheet = cx.weak_entity();
        let choices: Vec<(usize, String)> = self
            .projects
            .iter()
            .enumerate()
            .map(|(at, project)| (at, project.name.clone()))
            .collect();
        let current = checked_project(self.is_asking_project, self.project);
        button
            .disabled(self.step == Step::Starting)
            .dropdown_menu(move |menu, _, _| {
                choices.iter().fold(menu, |menu, (at, name)| {
                    let (sheet, at) = (sheet.clone(), *at);
                    let item =
                        kit::checked_item(name.clone(), Some(at) == current, move |_, _, cx| {
                            drop(sheet.update(cx, |sheet, cx| sheet.choose_project(at, cx)));
                        });
                    menu.item(item)
                })
            })
            .into_any_element()
    }

    fn fields(&self, palette: &Palette) -> AnyElement {
        let title = super::settings_modal::controls::styled_input(
            &self.title,
            theme::kit::scale::TEXT_BODY,
        )
        .appearance(false)
        .bordered(false)
        .focus_bordered(false)
        .min_h(theme::CONTROL_HEIGHT)
        .aria_label(tr(TITLE_LABEL));
        let title = dialog_frame::filled_field(
            ("new-lane-title", "new-lane-title-label"),
            tr(TITLE_LABEL),
            title,
            palette,
        );
        let description = dialog_frame::filled_field(
            ("new-lane-description", "new-lane-description-label"),
            tr(DESCRIPTION_LABEL),
            Textarea::new(&self.description)
                .appearance(false)
                .bordered(false)
                .with_size(Size::Medium)
                .text_size(tokens::DESCRIPTION_TEXT),
            palette,
        );
        dialog_frame::scrolling_body("new-lane-fields")
            .gap(theme::GAP_SMALL)
            .child(title)
            .child(description)
            .into_any_element()
    }

    fn track_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.is_asking_project {
            return None;
        }
        let offered = self.track_choices()?;
        let label = self
            .track
            .as_ref()
            .and_then(|id| offered.iter().find(|track| &track.id == id))
            .map_or_else(|| tr(Key("track.none")), |track| track.name.as_str());
        let choices = std::iter::once((tr(Key("track.none")).to_string(), None))
            .chain(
                offered
                    .iter()
                    .map(|track| (track.name.clone(), Some(track.id.clone()))),
            )
            .collect();
        let words = tr_with(Key("track.lane_picker"), &[("track", label)]);
        Some(self.choice_chip("new-lane-track", words, choices, TRACK_INPUT, cx))
    }

    /// The chips, what the engine said when it refused, and Start.
    fn footer(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let told = match &self.step {
            _ if self.current().folder_missing => Some(tr(super::lanes::FOLDER_MISSING).into()),
            Step::Told(told) => Some(told.clone()),
            _ => None,
        };
        let told = told.map(|told: SharedString| {
            super::chrome::text(theme::TEXT_SMALL, palette.danger)
                .debug_selector(|| "new-lane-note".into())
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .child(told)
        });
        let chips = [
            self.track_picker(cx),
            self.preset_chip(cx),
            self.pace_chip(cx),
        ]
        .into_iter()
        .flatten();
        dialog_frame::footer(palette)
            .children(chips)
            .children(told)
            .child(div().flex_1())
            .child(cancel_button(palette))
            .child(self.start_button(cx))
            .into_any_element()
    }

    /// "Preset: Careful": offered when the engine takes a preset and lists any.
    fn preset_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.accepts(PRESET_INPUT) || self.presets.is_empty() {
            return None;
        }
        let shown = match &self.preset {
            Some(preset) => self.preset_words(preset),
            None => self
                .engine_preset
                .clone()
                .unwrap_or_else(|| tr(PRESET_DEFAULT).to_string()),
        };
        let choices = preset_menu(
            self.engine_preset.as_deref(),
            &self.presets,
            &self.engine_preset_choice(),
        );
        let label = tr_with(PRESET, &[("choice", &shown)]);
        Some(self.choice_chip("new-lane-preset", label, choices, PRESET_INPUT, cx))
    }

    /// The word of the choice that takes the person's own preset off: the engine's preset in force.
    fn engine_preset_choice(&self) -> String {
        match &self.engine_preset {
            Some(preset) => preset.clone(),
            None => tr(PRESET_DEFAULT).to_string(),
        }
    }

    /// "Pace: Normal": offered when the engine takes a pace.
    fn pace_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.accepts(PACE_INPUT) {
            return None;
        }
        let level = Some(pace_in_force(self.pace.as_deref()));
        let word = |key: &str| {
            LANE_PACES
                .iter()
                .find(|(level, _)| *level == key)
                .map(|(_, key)| tr(*key))
        };
        let shown = level
            .and_then(word)
            .unwrap_or_else(|| tr(Key("pace.normal")));
        let choices = LANE_PACES
            .iter()
            .map(|(level, key)| (tr(*key).to_string(), Some(level.to_string())));
        let label = tr_with(PACE, &[("choice", shown)]);
        Some(self.choice_chip("new-lane-pace", label, choices.collect(), PACE_INPUT, cx))
    }

    /// A chip that opens the list of `choices` (word, value; no value takes the choice off) for `input`.
    fn choice_chip(
        &self,
        id: &'static str,
        label: SharedString,
        choices: Vec<(String, Option<String>)>,
        input: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sheet = cx.weak_entity();
        let current = match input {
            PRESET_INPUT => self.preset.clone(),
            TRACK_INPUT => self.track.clone(),
            _ => Some(pace_in_force(self.pace.as_deref()).to_string()),
        };
        let button = menu_chip(id, label, None, cx).debug_selector(move || id.into());
        let menu = button.dropdown_menu(move |menu, _, _| {
            choices.iter().fold(menu, |menu, (word, value)| {
                let (sheet, value) = (sheet.clone(), value.clone());
                let is_on = value == current;
                let item = kit::checked_item(word.clone(), is_on, move |_, _, cx| {
                    let value = value.clone();
                    drop(sheet.update(cx, |sheet, cx| sheet.choose(input, value, cx)));
                });
                menu.item(item)
            })
        });
        menu.into_any_element()
    }

    /// Start, with its key: the one filled button of the sheet.
    fn start_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let key = Keystroke::parse("cmd-enter").map(Kbd::new).ok();
        let palette = Theme::palette(cx);
        let label = tr(START);
        let kind = kit::ButtonKind::Primary;
        let button = if self.can_submit(cx) {
            kit::button("new-lane-start", label, kind, &palette)
                .on_click(cx.listener(|sheet, _, _, cx| sheet.submit(cx)))
        } else {
            kit::button_disabled("new-lane-start", label, kind, &palette)
        };
        button.flex_none().children(key).into_any_element()
    }
}

/// The sheet of a "New lane" with no project to start it in: what to do, and the button that does it. The
/// sheet shares the New lane sheet's surface.
pub(super) fn no_project(
    window: &Window,
    palette: &Palette,
    refusal: Option<SharedString>,
    on_add: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let add = kit::button(
        "new-lane-add-project",
        tr(Key("new_lane.add_project")),
        kit::ButtonKind::Primary,
        palette,
    )
    .on_click(move |_, window, cx| on_add(window, cx));
    dialog_frame::surface("new-lane-no-project", window, palette)
        .child(dialog_frame::heading(
            "new-lane-no-project-heading",
            tr(Key("new_lane.no_project")),
            palette,
        ))
        .child(
            super::chrome::text(theme::TEXT_SMALL, palette.text_secondary)
                .child(tr(Key("new_lane.no_project_detail"))),
        )
        .children(refusal.map(|words| {
            super::chrome::text(theme::TEXT_SMALL, palette.danger)
                .debug_selector(|| "new-lane-add-project-refusal".into())
                .child(words)
        }))
        .child(
            dialog_frame::footer(palette)
                .child(cancel_button(palette))
                .child(add),
        )
        .into_any_element()
}

/// The words of the preset menu and the value each sets (none takes the person's own choice off). The preset the
/// engine has in force is one of the listed presets, so it stands once, in its place, as the choice that sets
/// nothing; when it is not listed (or there is none) a first entry says what the engine picks.
pub(super) fn preset_menu(
    in_force: Option<&str>,
    presets: &[sakura_core::engine::presets::PresetChoice],
    engine_choice: &str,
) -> Vec<(String, Option<String>)> {
    let is_listed =
        in_force.is_some_and(|words| presets.iter().any(|preset| preset.words() == words));
    let mut menu: Vec<(String, Option<String>)> = Vec::new();
    if !is_listed {
        menu.push((engine_choice.to_string(), None));
    }
    for preset in presets {
        let is_in_force = is_listed && in_force == Some(preset.words().as_str());
        let value = (!is_in_force).then(|| preset.name.clone());
        menu.push((preset.words(), value));
    }
    menu
}

fn cancel_button(palette: &Palette) -> gpui::Stateful<gpui::Div> {
    kit::button(
        "new-lane-cancel",
        tr(Key("common.cancel")),
        kit::ButtonKind::Secondary,
        palette,
    )
    .flex_none()
    .on_click(|_, window, cx| window.close_dialog(cx))
}

#[cfg(test)]
mod tests {
    use super::checked_project;

    #[test]
    fn the_project_menu_checks_no_project_while_the_chip_asks_for_one() {
        assert_eq!(
            checked_project(true, 0),
            None,
            "the chip reads Choose a project"
        );
        assert_eq!(
            checked_project(false, 2),
            Some(2),
            "the chip names the project checked"
        );
    }
}
