//! "Run checks with the Restate engine", in "Machine": the runner of the checks the service keeps in
//! its config (`box config set RUNNER`), off by default. The switch shows what `box status` says; a
//! press writes the other runner and reads the machine again. With the engine on and a program it
//! needs missing, the row's sentence is what to install; a refusal of `box` is said there instead.

use super::sections::{keywords, marked};
use super::{Read, Settings};
use crate::l10n::{tr, Key};
use crate::ui::settings::rows::Row;
use gpui::{div, Context, InteractiveElement as _, ParentElement as _, SharedString, Window};
use gpui_component::switch::Switch;
use gpui_component::{Disableable as _, FocusableExt as _};
use sakura_core::error::BridgeError;
use sakura_core::models::{CheckRunner, EnginePrerequisites, RunnerKind};

/// The switch, by its name in the window.
const ENGINE_SWITCH: &str = "settings-machine-engine";

impl Settings {
    /// The row of the switch, for the choice `box status` said.
    pub(super) fn engine_row(&self, choice: &CheckRunner, cx: &Context<Self>) -> Row {
        let runner = self.runner_asked.unwrap_or(choice.runner);
        let is_on = runner == RunnerKind::Engine;
        let description = description(self.runner_refusal.as_ref(), is_on, choice.engine.as_ref());
        let is_changing = self.runner_asked.is_some();
        let settings = cx.weak_entity();
        let control = move |window: &mut Window, _: &mut gpui::App| {
            let settings = settings.clone();
            // Pressed with the pointer it keeps the focus but shows no ring; reached with the keyboard
            // it shows it.
            let switch = Switch::new(ENGINE_SWITCH)
                .focus_ring(window.last_input_was_keyboard())
                .checked(is_on)
                .disabled(is_changing)
                .on_click(move |is_on: &bool, _, cx| {
                    let runner = if *is_on {
                        RunnerKind::Engine
                    } else {
                        RunnerKind::Bash
                    };
                    drop(settings.update(cx, |settings, cx| settings.choose_runner(runner, cx)));
                });
            let switch = div()
                .debug_selector(move || ENGINE_SWITCH.into())
                .child(switch);
            marked(format!("{ENGINE_SWITCH}-row"), switch)
        };
        Row::setting(
            tr(Key("settings.machine.engine")),
            description,
            keywords(Key("settings.machine.engine.keywords")),
            control,
        )
    }

    /// Writes the runner the person chose, then reads the machine again. Until `box` answered the
    /// switch shows the choice and takes no other press.
    pub(super) fn choose_runner(&mut self, runner: RunnerKind, cx: &mut Context<Self>) {
        if self.runner_asked.is_some() {
            return;
        }
        self.runner_asked = Some(runner);
        self.runner_refusal = None;
        let answer = self.source.set_runner(runner);
        cx.spawn(async move |settings, cx| {
            let answer = answer.await;
            drop(settings.update(cx, |settings, cx| {
                settings.runner_asked = None;
                settings.runner_refusal = answer.err();
                settings.read(Read::Machine, cx);
            }));
        })
        .detach();
        cx.notify();
    }
}

/// The row's sentence: why `box` did not change the runner, else what to install while the engine is
/// on and lacks a program, else what the switch changes.
fn description(
    refusal: Option<&BridgeError>,
    is_on: bool,
    engine: Option<&EnginePrerequisites>,
) -> SharedString {
    if let Some(refusal) = refusal {
        let why = refusal
            .details
            .as_deref()
            .map(|line| line.trim_start_matches("box: ").to_string())
            .unwrap_or_else(|| crate::l10n::say(&refusal.said).to_string());
        let said = sakura_core::said::Said::new("settings.machine.engine.refused").text("why", why);
        return crate::l10n::say(&said);
    }
    match engine.filter(|engine| is_on && !engine.is_complete()) {
        Some(engine) => tr(install_key(engine)).into(),
        None => tr(Key("settings.machine.engine.description")).into(),
    }
}

/// The one line that says what to install, for the programs that are missing.
fn install_key(engine: &EnginePrerequisites) -> Key {
    match (engine.restate_server, engine.node) {
        (false, false) => Key("settings.machine.engine.install.both"),
        (false, true) => Key("settings.machine.engine.install.restate"),
        _ => Key("settings.machine.engine.install.node"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l10n::{self, Language};
    use sakura_core::said::Said;

    fn engine(restate_server: bool, node: bool) -> EnginePrerequisites {
        EnginePrerequisites {
            restate_server,
            node,
            setup: None,
        }
    }

    #[test]
    fn the_missing_programs_are_named_in_one_line_only_while_the_engine_is_on() {
        l10n::set_for_test(Language::English);
        let plain = description(None, false, Some(&engine(false, false)));
        assert_eq!(plain, tr(Key("settings.machine.engine.description")));
        let both = description(None, true, Some(&engine(false, false)));
        assert!(
            both.contains("brew install restatedev/tap/restate-server node"),
            "{both}"
        );
        let restate = description(None, true, Some(&engine(false, true)));
        assert!(
            restate.contains("restate-server") && !restate.contains("node"),
            "{restate}"
        );
        let node = description(None, true, Some(&engine(true, false)));
        assert!(
            node.contains("brew install node") && !node.contains("restate"),
            "{node}"
        );
        let ready = description(None, true, Some(&engine(true, true)));
        assert_eq!(ready, plain);
        assert_eq!(description(None, true, None), plain);
        for line in [both, restate, node] {
            assert!(!line.contains('\n'), "one line: {line}");
        }
    }

    #[test]
    fn a_refusal_is_said_with_box_s_line_in_both_languages() {
        let refusal = BridgeError::failed(Said::new("error.internal")).with_details(Some(
            "box: a check runs or waits in the engine: let it end or cancel it".into(),
        ));
        l10n::set_for_test(Language::English);
        let said = description(Some(&refusal), true, Some(&engine(false, false)));
        assert!(
            said.ends_with("a check runs or waits in the engine: let it end or cancel it"),
            "{said}"
        );
        assert!(!said.contains("box: "), "{said}");
        l10n::set_for_test(Language::Italian);
        let detto = description(Some(&refusal), true, None);
        assert_ne!(detto, said);
    }
}
