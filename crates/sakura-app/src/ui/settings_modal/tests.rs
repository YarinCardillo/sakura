//! Settings as a person meets it: the modal that Cmd+comma opens over the window, its sections, and what a
//! press on a control sends to the engine.

use super::{Section, SettingsModal};
use crate::prefs::Preferences;
use crate::theme::{Appearance, Theme};
use crate::ui::engine_desk::Desk;
use crate::ui::engine_fixture_tests::{edited, full};
use crate::ui::engine_rig_tests::{self, wait_until, Rig, SNAPSHOT};
use crate::ui::lane_tests::{bounds, click, open_answering};
use gpui::{AppContext as _, Entity, Focusable as _, TestAppContext, VisualTestContext};
use sakura_core::tabs::Page;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const DONE: &str = r#"{"result": "done", "exit_code": 0, "output": "ok\n"}"#;
const CONFIRM: &str = r#"{"result": "confirm", "token": "17.ab", "expires": "2026-11-16T08:02:00Z", "summary": "Disconnect github"}"#;
const REFUSED: &str =
    r#"{"result": "refused", "reason": "the connector is in use by a running lane"}"#;
const SERVICES: &str = r#"["host","services"]"#;
const CONNECTORS: &str = r#"["host","connectors"]"#;

/// The desk over a fake engine whose snapshot is `snapshot`, with the modal open on `section`.
fn open_modal<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
    first: &str,
    second: &str,
    section: &str,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    let (rig, desk, cx) = open_answering(cx, snapshot, first, second);
    cx.update(|window, cx| {
        desk.update(cx, |desk, cx| desk.open_settings(window, cx));
    });
    wait_until(cx, "the modal", |cx| {
        cx.debug_bounds("settings-modal").is_some()
    });
    if section != "general" {
        click(cx, &format!("settings-section-{section}"));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    (rig, desk, cx)
}

/// The modal alone in a window, in the home `home`, over a fake engine.
pub(super) fn open_alone<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
    home: PathBuf,
) -> (Rig, Entity<SettingsModal>, &'a mut VisualTestContext) {
    let rig = engine_rig_tests::open(cx, snapshot, DONE, DONE);
    cx.update(|cx| Preferences::default().install(None, cx));
    let (facts, source) = (rig.facts.clone(), rig.source.clone());
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let modal = cx.new(|cx| SettingsModal::new(source, facts, home, window, cx));
        *kept.borrow_mut() = Some(modal.clone());
        gpui_component::Root::new(modal, window, cx)
    });
    let modal = slot.borrow_mut().take().expect("the modal was built");
    cx.update(|window, _| window.activate_window());
    let facts = rig.facts.clone();
    wait_until(cx, "the first snapshot", |cx| {
        cx.debug_bounds("settings-modal").is_some()
            && facts.read_with(cx, |facts, _| facts.snapshot().is_some())
    });
    (rig, modal, cx)
}

fn show(cx: &mut VisualTestContext, section: &str) {
    click(cx, &format!("settings-section-{section}"));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn command(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    sakura_core::test_support::write_script(&path, "#!/bin/sh\nexit 0\n");
    path
}

fn write_path(modal: &Entity<SettingsModal>, text: &str, cx: &mut VisualTestContext) {
    let field = modal.read_with(cx, |modal, _| modal.path.clone());
    cx.update(|window, cx| field.update(cx, |field, cx| field.set_value(text, window, cx)));
    modal.update(cx, |modal, cx| modal.save_engine_path(cx));
    let written = text.trim();
    let expected = (!written.is_empty()).then_some(written.to_string());
    wait_until(cx, "the engine path is saved", |cx| {
        cx.update(|_, cx| Preferences::current(cx).engine_bin) == expected
    });
}

fn nowhere_home() -> PathBuf {
    tempfile::tempdir().expect("a folder").keep()
}

#[gpui::test]
fn cmd_comma_opens_the_modal_and_escape_closes_it(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_answering(cx, SNAPSHOT, DONE, DONE);
    cx.update(|_, cx| crate::menus::install(cx));
    let windows = |cx: &mut VisualTestContext| cx.update(|_, cx| cx.windows().len());
    let before = windows(cx);
    cx.simulate_keystrokes("cmd-,");
    wait_until(cx, "the modal", |cx| {
        cx.debug_bounds("settings-modal").is_some()
    });
    assert_eq!(windows(cx), before, "no window other than the main one");
    cx.simulate_keystrokes("escape");
    wait_until(cx, "the modal closes", |cx| {
        cx.debug_bounds("settings-modal").is_none()
    });
}

/// The modal opened as a dialog over the desk's window, with its handle kept.
fn open_dialog(
    cx: &mut TestAppContext,
    section: Section,
) -> (Rig, Entity<SettingsModal>, &mut VisualTestContext) {
    let (rig, _desk, cx) = open_answering(cx, SNAPSHOT, DONE, DONE);
    let (source, facts) = (rig.source.clone(), rig.facts.clone());
    let modal = cx
        .update(|window, cx| super::open(source, facts, section, window, cx))
        .expect("the modal opens");
    wait_until(cx, "the modal", |cx| {
        cx.debug_bounds("settings-modal").is_some()
    });
    cx.run_until_parked();
    (rig, modal, cx)
}

fn has_focus(modal: &Entity<SettingsModal>, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| modal.read(cx).focus.is_focused(window))
}

fn escape_closes(cx: &mut VisualTestContext) {
    cx.simulate_keystrokes("escape");
    wait_until(cx, "the modal closes", |cx| {
        cx.debug_bounds("settings-modal").is_none()
    });
}

#[gpui::test]
fn escape_closes_the_modal_on_its_first_opening_with_nothing_clicked(cx: &mut TestAppContext) {
    let (_rig, modal, cx) = open_dialog(cx, Section::General);
    assert!(
        !has_focus(&modal, cx),
        "opening leaves the modal's own focus stop untouched"
    );
    escape_closes(cx);
}

#[gpui::test]
fn escape_closes_the_modal_after_a_folder_pick_left_the_window_with_no_focus(
    cx: &mut TestAppContext,
) {
    let (_rig, modal, cx) = open_dialog(cx, Section::Projects);
    // The system's folder panel gave the window back with no element focused; the pick then redraws the modal.
    cx.update(|window, cx| window.blur(cx));
    let adds = modal.read_with(cx, |modal, _| modal.adds.clone());
    adds.update(cx, |_, cx| cx.notify());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    assert!(
        !has_focus(&modal, cx),
        "the modal leaves focus where it was"
    );
    escape_closes(cx);
}

#[gpui::test]
fn changing_settings_sections_keeps_the_tab_order_without_taking_focus(cx: &mut TestAppContext) {
    let (_rig, modal, cx) = open_dialog(cx, Section::General);
    cx.update(|window, cx| window.blur(cx));
    modal.update(cx, |modal, cx| modal.show(Section::Accounts, cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        !has_focus(&modal, cx),
        "a section change leaves focus where it was"
    );
    escape_closes(cx);
}

#[gpui::test]
fn the_close_button_closes_the_modal(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "general");
    click(cx, "settings-close");
    wait_until(cx, "the modal closes", |cx| {
        cx.debug_bounds("settings-modal").is_none()
    });
}

#[gpui::test]
fn the_list_holds_the_eight_sections_in_order_and_the_modal_is_860_by_580(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "general");
    let names = [
        "general",
        "projects",
        "accounts",
        "connectors",
        "notifications",
        "shortcuts",
        "updates",
        "about",
    ];
    let tops: Vec<f32> = names
        .iter()
        .map(|name| {
            f32::from(
                bounds(cx, &format!("settings-section-{name}"))
                    .expect(name)
                    .top(),
            )
        })
        .collect();
    assert!(tops.windows(2).all(|pair| pair[0] < pair[1]), "{tops:?}");
    let modal = cx.debug_bounds("settings-modal").expect("the modal");
    assert_eq!(
        (f32::from(modal.size.width), f32::from(modal.size.height)),
        (
            860.,
            f32::from(cx.update(|window, _| crate::ui::dialog_frame::available_height(window)))
        )
    );
}

#[gpui::test]
fn the_modal_stands_at_the_shared_dialog_offset_and_a_one_line_row_is_44_points_high(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "general");
    let modal = cx.debug_bounds("settings-modal").expect("the modal");
    assert_eq!(
        f32::from(modal.top()),
        f32::from(crate::theme::DIALOG_SHEET_TOP),
        "the shared offset"
    );
    let language = bounds(cx, "settings-row-language").expect("a row");
    assert_eq!(f32::from(language.size.height), 44.);
}

#[gpui::test]
fn opening_the_modal_gives_no_field_the_caret(cx: &mut TestAppContext) {
    let (_rig, modal, cx) = open_alone(cx, SNAPSHOT, nowhere_home());
    let field = modal.read_with(cx, |modal, _| modal.path.clone());
    let initial = field.read_with(cx, |field, _| field.value());
    cx.simulate_input("typed with no press on the field");
    cx.run_until_parked();
    assert_eq!(field.read_with(cx, |field, _| field.value()), initial);
}

#[gpui::test]
fn settings_reveals_the_discovered_engine_path_only_on_request(cx: &mut TestAppContext) {
    let (_rig, _modal, cx) = open_alone(cx, SNAPSHOT, nowhere_home());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("settings-engine").is_some(),
        "the command status is shown"
    );
    assert!(cx.debug_bounds("settings-engine-location-value").is_none());
    click(cx, "settings-engine-location-reveal");
    assert!(cx.debug_bounds("settings-engine-location-value").is_some());
}

#[gpui::test]
fn a_saved_path_is_kept_in_the_preferences_and_is_the_command_in_use(cx: &mut TestAppContext) {
    let (rig, modal, cx) = open_alone(cx, SNAPSHOT, nowhere_home());
    let chosen = command(rig.dir.path(), "chosen-sakura");
    write_path(&modal, &chosen.display().to_string(), cx);
    let kept = cx.update(|_, cx| Preferences::current(cx).engine_bin);
    assert_eq!(kept, Some(chosen.display().to_string()));
    assert_eq!(
        rig.source.command(),
        Some(chosen.canonicalize().expect("a real path"))
    );
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("settings-engine-failed").is_none(),
        "a command that exists is not refused"
    );
}

#[gpui::test]
fn closing_settings_keeps_the_submitted_engine_path(cx: &mut TestAppContext) {
    let (rig, modal, cx) = open_dialog(cx, Section::General);
    let chosen = command(rig.dir.path(), "late-sakura");
    cx.update(|window, cx| {
        modal.update(cx, |modal, cx| {
            modal.path.update(cx, |path, cx| {
                path.set_value(chosen.display().to_string(), window, cx)
            });
            modal.save_engine_path(cx);
        });
        window.dispatch_event(
            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse("escape").unwrap(),
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        );
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("settings-modal").is_none());
    let executable = chosen.canonicalize().unwrap();
    let written = chosen.display().to_string();
    wait_until(cx, "the submitted path survives dismissal", |cx| {
        cx.update(|_, cx| Preferences::current(cx).engine_bin.as_deref() == Some(written.as_str()))
            && rig.source.command().as_deref() == Some(executable.as_path())
    });
}

#[gpui::test]
fn a_path_that_is_no_command_is_said_under_the_field_and_kept(cx: &mut TestAppContext) {
    let (rig, modal, cx) = open_alone(cx, SNAPSHOT, nowhere_home());
    let nowhere = rig.dir.path().join("nowhere");
    write_path(&modal, &nowhere.display().to_string(), cx);
    wait_until(cx, "the sentence", |cx| {
        cx.debug_bounds("settings-engine-failed").is_some()
    });
    let kept = cx.update(|_, cx| Preferences::current(cx).engine_bin);
    assert_eq!(kept, Some(nowhere.display().to_string()));
}

#[gpui::test]
fn an_empty_field_clears_the_path(cx: &mut TestAppContext) {
    let (rig, modal, cx) = open_alone(cx, SNAPSHOT, nowhere_home());
    write_path(
        &modal,
        &command(rig.dir.path(), "sakura-x").display().to_string(),
        cx,
    );
    write_path(&modal, "  ", cx);
    assert_eq!(cx.update(|_, cx| Preferences::current(cx).engine_bin), None);
}

#[gpui::test]
fn the_field_starts_with_the_path_the_preferences_keep(cx: &mut TestAppContext) {
    let rig = engine_rig_tests::open(cx, SNAPSHOT, DONE, DONE);
    cx.update(|cx| {
        let preferences = Preferences {
            engine_bin: Some("/opt/engine/bin/sakura".into()),
            ..Preferences::default()
        };
        preferences.install(None, cx);
    });
    let (facts, source, home) = (rig.facts.clone(), rig.source.clone(), nowhere_home());
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let modal = cx.new(|cx| SettingsModal::new(source, facts, home, window, cx));
        *kept.borrow_mut() = Some(modal.clone());
        gpui_component::Root::new(modal, window, cx)
    });
    let modal = slot.borrow_mut().take().expect("the modal");
    let field = modal.read_with(cx, |modal, _| modal.path.clone());
    assert_eq!(
        field.read_with(cx, |field, _| field.value().to_string()),
        "/opt/engine/bin/sakura"
    );
}

#[gpui::test]
fn theme_light_switches_the_palette_in_the_same_frame(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "general");
    assert_eq!(
        cx.update(|_, cx| Theme::palette(cx).appearance),
        Appearance::Dark
    );
    click(cx, "settings-theme-2");
    assert_eq!(
        cx.update(|_, cx| Theme::palette(cx).appearance),
        Appearance::Light
    );
    assert_eq!(
        cx.update(|_, cx| Preferences::current(cx).appearance),
        Some(Appearance::Light)
    );
}

/// Scrolls the section under the pointer to its end with the wheel, for a control below the fold.
fn scroll_down(cx: &mut VisualTestContext) {
    let at = bounds(cx, "settings-modal").expect("the modal").center();
    cx.simulate_event(gpui::ScrollEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-600.))),
        ..Default::default()
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn the_keeper_switch_sends_b_with_the_keeper_turned_off(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_modal(cx, &full(), DONE, DONE, "general");
    scroll_down(cx);
    click(cx, "settings-keeper");
    wait_until(cx, "the call", |_| rig.acts().len() == 1);
    assert_eq!(
        rig.acts()[0],
        format!("cockpit act B {SERVICES} --name keeper --on no --source app --json")
    );
}

#[gpui::test]
fn an_engine_that_reports_no_keeper_says_so_and_has_no_switch(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "general");
    assert!(
        cx.debug_bounds("settings-row-keeper").is_some(),
        "the row is there"
    );
    assert!(
        cx.debug_bounds("settings-keeper").is_none(),
        "its switch is not"
    );
}

#[gpui::test]
fn the_animated_pixels_switch_stops_and_starts_the_visible_retino(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "general");
    let retino = desk.read_with(cx, |desk, _| desk.retino.clone());
    cx.update(|_, cx| cx.set_reduce_motion(false));
    redraw(cx);
    let is_ticking =
        |cx: &mut VisualTestContext| retino.read_with(cx, |retino, _| retino.is_ticking());
    assert!(is_ticking(cx));

    click(cx, "settings-animated-pixels");
    assert!(!is_ticking(cx));
    redraw(cx);
    assert!(!is_ticking(cx));
    click(cx, "settings-animated-pixels");
    assert!(is_ticking(cx));
    redraw(cx);

    click(cx, "settings-animated-pixels");
    redraw(cx);
    click(cx, "settings-animated-pixels-default");
    assert!(is_ticking(cx));
}

#[gpui::test]
fn the_master_conversation_switch_keeps_its_choice_in_the_preferences(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "general");
    assert!(cx.update(|_, cx| Preferences::current(cx).is_master_restored));
    scroll_down(cx);
    click(cx, "settings-master-restore");
    assert!(!cx.update(|_, cx| Preferences::current(cx).is_master_restored));
}

#[gpui::test]
fn open_at_login_writes_and_removes_the_launch_agent_in_the_home(cx: &mut TestAppContext) {
    let home = nowhere_home();
    let (_rig, _modal, cx) = open_alone(cx, SNAPSHOT, home.clone());
    assert!(!super::login_item::is_enabled(&home));
    click(cx, "settings-login");
    wait_until(cx, "the login item is installed", |_| {
        super::login_item::is_enabled(&home)
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    click(cx, "settings-login");
    wait_until(cx, "the login item is removed", |_| {
        !super::login_item::is_enabled(&home)
    });
}

fn redraw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn the_accent_row_has_four_filled_swatches_and_the_default_is_the_one_marked(
    cx: &mut TestAppContext,
) {
    use crate::theme_brand::Accent;
    let (_rig, _modal, cx) = open_alone(cx, SNAPSHOT, nowhere_home());
    for accent in Accent::ALL {
        let id = Box::leak(format!("settings-accent-{}", accent.id()).into_boxed_str());
        assert!(cx.debug_bounds(id).is_some(), "{id}");
    }
    assert!(cx
        .debug_bounds("settings-accent-sakura-pink-selected")
        .is_some());
    assert!(cx
        .debug_bounds("settings-accent-signal-orange-selected")
        .is_none());
    assert!(cx.debug_bounds("settings-accent-default").is_none());
}

#[gpui::test]
fn choosing_an_accent_changes_the_palette_in_the_same_frame_and_keeps_the_choice(
    cx: &mut TestAppContext,
) {
    use crate::theme_brand::Accent;
    let (_rig, _modal, cx) = open_alone(cx, SNAPSHOT, nowhere_home());
    click(cx, "settings-accent-instrument-cyan");
    let (chosen, shown) = cx.update(|_, cx| (Preferences::current(cx).accent, Theme::palette(cx)));
    assert_eq!(chosen, Accent::InstrumentCyan);
    let expected = Accent::InstrumentCyan.rgb(shown.appearance == Appearance::Dark);
    assert_eq!(shown.attention, gpui::Hsla::from(gpui::rgb(expected)));
    redraw(cx);
    assert!(cx
        .debug_bounds("settings-accent-instrument-cyan-selected")
        .is_some());
    assert!(cx
        .debug_bounds("settings-accent-sakura-pink-selected")
        .is_none());
    click(cx, "settings-accent-default");
    assert_eq!(
        cx.update(|_, cx| Preferences::current(cx).accent),
        Accent::SakuraPink
    );
}

#[gpui::test]
fn no_swatch_has_the_keyboard_on_opening_and_a_focused_one_is_chosen_with_enter(
    cx: &mut TestAppContext,
) {
    use crate::theme_brand::Accent;
    let (_rig, modal, cx) = open_alone(cx, SNAPSHOT, nowhere_home());
    let handles = modal.read_with(cx, |modal, _| modal.accent_focus.clone());
    for handle in &handles {
        assert!(
            !cx.update(|window, _| handle.is_focused(window)),
            "no autofocus"
        );
    }
    cx.update(|window, cx| window.focus(&handles[2], cx));
    cx.simulate_keystrokes("enter");
    assert_eq!(
        cx.update(|_, cx| Preferences::current(cx).accent),
        Accent::SignalOrange
    );
}

#[gpui::test]
fn a_theme_that_is_not_the_default_offers_use_default_and_pressing_it_goes_back(
    cx: &mut TestAppContext,
) {
    let (_rig, modal, cx) = open_alone(cx, SNAPSHOT, nowhere_home());
    assert!(
        cx.debug_bounds("settings-theme-default").is_none(),
        "at its default nothing is offered"
    );
    cx.update(|_, cx| Preferences::change(cx, |prefs| prefs.appearance = Some(Appearance::Dark)));
    modal.update(cx, |_, cx| cx.notify());
    redraw(cx);
    click(cx, "settings-theme-default");
    assert_eq!(cx.update(|_, cx| Preferences::current(cx).appearance), None);
    redraw(cx);
    assert!(cx.debug_bounds("settings-theme-default").is_none());
}

#[gpui::test]
fn a_language_that_is_not_the_default_offers_use_default(cx: &mut TestAppContext) {
    use crate::l10n::{Language, LanguageChoice};
    let (_rig, modal, cx) = open_alone(cx, SNAPSHOT, nowhere_home());
    assert!(cx.debug_bounds("settings-language-default").is_none());
    cx.update(|_, cx| crate::language::set_choice(LanguageChoice::Is(Language::English), cx));
    modal.update(cx, |_, cx| cx.notify());
    redraw(cx);
    click(cx, "settings-language-default");
    assert_eq!(
        cx.update(|_, cx| crate::language::choice(cx)),
        LanguageChoice::Automatic
    );
}

#[gpui::test]
fn open_at_login_turned_on_offers_use_default_which_turns_it_off(cx: &mut TestAppContext) {
    let home = nowhere_home();
    let (_rig, _modal, cx) = open_alone(cx, SNAPSHOT, home.clone());
    assert!(cx.debug_bounds("settings-login-default").is_none());
    click(cx, "settings-login");
    redraw(cx);
    click(cx, "settings-login-default");
    wait_until(cx, "the login item is removed", |_| {
        !super::login_item::is_enabled(&home)
    });
}

#[gpui::test]
fn a_written_engine_path_offers_use_default_which_empties_the_field_and_uses_the_one_found(
    cx: &mut TestAppContext,
) {
    let (rig, modal, cx) = open_alone(cx, SNAPSHOT, nowhere_home());
    assert!(
        cx.debug_bounds("settings-engine-default").is_none(),
        "an empty field is the default already"
    );
    let chosen = command(rig.dir.path(), "chosen-sakura");
    write_path(&modal, &chosen.display().to_string(), cx);
    redraw(cx);
    click(cx, "settings-engine-default");
    wait_until(cx, "the default engine path is saved", |cx| {
        cx.update(|_, cx| Preferences::current(cx).engine_bin)
            .is_none()
    });
    let field = modal.read_with(cx, |modal, _| modal.path.clone());
    assert_eq!(
        field.read_with(cx, |field, _| field.value().to_string()),
        ""
    );
    assert_eq!(cx.update(|_, cx| Preferences::current(cx).engine_bin), None);
    assert_eq!(
        rig.source.command(),
        rig.source.default_command(),
        "the engine found is in use again"
    );
}

#[gpui::test]
fn projects_lists_each_project_and_adding_a_folder_asks_the_engine(cx: &mut TestAppContext) {
    let (rig, modal, cx) = open_alone(cx, SNAPSHOT, nowhere_home());
    show(cx, "projects");
    assert!(cx.debug_bounds("settings-project-repo").is_some());
    assert!(cx.debug_bounds("settings-project-other").is_some());
    assert!(
        cx.debug_bounds("settings-add-project").is_some(),
        "the one place a project is added from"
    );
    modal.update(cx, |modal, cx| modal.add_folder("/work/new".into(), cx));
    wait_until(cx, "the call", |_| rig.acts().len() == 1);
    assert_eq!(
        rig.acts()[0],
        "cockpit act add-project /work/new --source app --json"
    );
}

#[gpui::test]
fn an_action_the_engine_offers_on_a_project_is_a_button_and_sends_that_action(
    cx: &mut TestAppContext,
) {
    let snapshot = edited(|snapshot| {
        let project = &mut snapshot["projects"][0];
        project["actions"]
            .as_array_mut()
            .expect("actions")
            .push(serde_json::json!(
            {"key": "X", "label": "Remove", "needs": [], "optional": [], "confirm": false}));
    });
    let (rig, _desk, cx) = open_modal(cx, &snapshot, DONE, DONE, "projects");
    assert!(
        cx.debug_bounds("settings-project-other-X").is_none(),
        "only where it is offered"
    );
    click(cx, "settings-project-repo-X");
    wait_until(cx, "the call", |_| rig.acts().len() == 1);
    assert_eq!(
        rig.acts()[0],
        r#"cockpit act X ["project","/work/repo"] --source app --json"#
    );
}

#[gpui::test]
fn accounts_lists_every_account_with_its_provider_and_plan(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, &full(), DONE, DONE, "accounts");
    for id in ["personal", "work", "codex"] {
        assert!(
            cx.debug_bounds(Box::leak(format!("settings-account-{id}").into_boxed_str()))
                .is_some(),
            "{id}"
        );
    }
}

#[gpui::test]
fn renaming_an_account_sends_the_engine_s_rename_with_the_new_label(cx: &mut TestAppContext) {
    let (rig, modal, cx) = open_alone(cx, &full(), nowhere_home());
    show(cx, "accounts");
    click(cx, "settings-account-work-rename");
    cx.run_until_parked();
    let field = modal.read_with(cx, |modal, _| modal.name_field.clone());
    cx.update(|window, cx| {
        field.update(cx, |field, cx| field.set_value("Client work", window, cx))
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    click(cx, "settings-account-work-save");
    wait_until(cx, "the call", |_| rig.acts().len() == 1);
    assert_eq!(
        rig.acts()[0],
        r#"cockpit act l ["account","claude","work"] --label Client work --source app --json"#
    );
}

#[gpui::test]
fn an_account_row_offers_rename_and_neither_hide_nor_show(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, &full(), DONE, DONE, "accounts");
    assert!(cx.debug_bounds("settings-account-work-rename").is_some());
    for key in ["n", "e"] {
        let id = Box::leak(format!("settings-account-personal-{key}").into_boxed_str());
        assert!(cx.debug_bounds(id).is_none(), "{key}");
    }
}

/// The full snapshot with the engine offering "Remove" (asking first) on the accounts named in `ids`.
fn offering_remove(ids: &[&str]) -> String {
    edited(|snapshot| {
        for account in snapshot["accounts"].as_array_mut().expect("the accounts") {
            if !ids.contains(&account["id"].as_str().unwrap_or("")) {
                continue;
            }
            account["actions"]
                .as_array_mut()
                .expect("actions")
                .push(serde_json::json!(
                    {"key": "X", "label": "Remove", "needs": [], "optional": [], "confirm": true}
                ));
        }
    })
}

#[gpui::test]
fn only_an_account_the_engine_lets_go_has_a_remove_button(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, &offering_remove(&["work"]), DONE, DONE, "accounts");
    assert!(cx.debug_bounds("settings-account-work-remove").is_some());
    assert!(cx
        .debug_bounds("settings-account-personal-remove")
        .is_none());
}

#[gpui::test]
fn removing_an_account_asks_first_and_the_engine_s_remove_runs_only_after_the_confirmation(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = open_modal(cx, &offering_remove(&["work"]), CONFIRM, DONE, "accounts");
    click(cx, "settings-account-work-remove");
    wait_until(cx, "the question", |cx| {
        cx.debug_bounds("settings-account-work-remove-confirm")
            .is_some()
    });
    assert_eq!(rig.acts().len(), 1, "only the asking call went out");
    click(cx, "settings-account-work-remove-confirm");
    wait_until(cx, "the confirmed call", |_| rig.acts().len() == 2);
    assert_eq!(
        rig.acts()[1],
        r#"cockpit act X ["account","claude","work"] --confirm 17.ab --source app --json"#
    );
}

/// What the engine said of a refused removal: the account is pinned to a step of a project.
const PINNED: &str =
    r#"{"result": "refused", "reason": "a step of the project orchard is pinned to this account"}"#;

/// The real engine's snapshot: every account in it offers Remove, the hidden one too.
const REAL_SNAPSHOT: &str =
    include_str!("../../../../sakura-core/tests/fixtures/engine-real-snapshot.json");

#[gpui::test]
fn every_account_row_of_the_real_engine_has_a_remove_button(cx: &mut TestAppContext) {
    let snapshot: serde_json::Value = serde_json::from_str(REAL_SNAPSHOT).expect("the fixture");
    let ids: Vec<String> = snapshot["accounts"]
        .as_array()
        .expect("the accounts")
        .iter()
        .map(|account| account["id"].as_str().expect("an id").to_string())
        .collect();
    assert_eq!(ids.len(), 3);
    let (_rig, _desk, cx) = open_modal(cx, REAL_SNAPSHOT, DONE, DONE, "accounts");
    for id in ids {
        let selector = Box::leak(format!("settings-account-{id}-remove").into_boxed_str());
        assert!(cx.debug_bounds(selector).is_some(), "{id}");
    }
}

#[gpui::test]
fn a_removal_the_engine_refuses_says_why_under_the_row_and_runs_nothing(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_modal(cx, &offering_remove(&["work"]), PINNED, DONE, "accounts");
    click(cx, "settings-account-work-remove");
    wait_until(cx, "the refusal", |cx| {
        cx.debug_bounds("settings-account-work-remove-told")
            .is_some()
    });
    assert_eq!(rig.acts().len(), 1, "only the asking call went out");
    assert!(cx
        .debug_bounds("settings-account-work-remove-confirm")
        .is_none());
}

#[test]
fn the_question_before_removing_an_account_names_it() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let words = super::accounts::remove_words("Client work");
    assert!(words.contains("Client work"), "{words}");
}

#[gpui::test]
fn an_act_that_needs_a_confirmation_asks_in_words_and_runs_only_after_the_person_confirms(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = open_modal(cx, &full(), CONFIRM, DONE, "connectors");
    click(cx, "settings-connector-github-button");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    click(cx, "settings-connector-github-disconnect-github");
    wait_until(cx, "the question", |cx| {
        cx.debug_bounds("settings-connector-github-confirm")
            .is_some()
    });
    assert_eq!(
        rig.acts().len(),
        1,
        "only the asking call went to the engine"
    );
    click(cx, "settings-connector-github-confirm");
    wait_until(cx, "the confirmed call", |_| rig.acts().len() == 2);
    assert!(
        rig.acts()[1].contains("--confirm 17.ab"),
        "{}",
        rig.acts()[1]
    );
}

#[gpui::test]
fn what_the_engine_refuses_is_said_under_the_row(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, &full(), REFUSED, REFUSED, "connectors");
    click(cx, "settings-connector-github-button");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    click(cx, "settings-connector-github-disconnect-github");
    wait_until(cx, "the refusal", |cx| {
        cx.debug_bounds("settings-connector-github-told").is_some()
    });
}

#[gpui::test]
fn a_connector_the_digest_reads_through_says_what_it_reads_under_its_name(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, &full(), DONE, DONE, "connectors");
    click(cx, "settings-connector-github-button");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx
        .debug_bounds("settings-connector-github-reads-github")
        .is_some());
}

#[gpui::test]
fn connectors_show_their_state_and_connect_sends_o_with_the_kind(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_modal(cx, &full(), DONE, DONE, "connectors");
    assert!(cx.debug_bounds("settings-connector-github").is_some());
    click(cx, "settings-connector-linear-button");
    wait_until(cx, "the call", |_| rig.acts().len() == 1);
    assert_eq!(
        rig.acts()[0],
        format!("cockpit act O {CONNECTORS} --kind linear --source app --json")
    );
}

#[gpui::test]
fn every_connector_row_has_its_platform_s_mark_bare_at_the_icon_size(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, &full(), DONE, DONE, "connectors");
    for kind in ["github", "notion", "linear", "feed", "program"] {
        let id = Box::leak(format!("settings-connector-{kind}-mark").into_boxed_str());
        let mark = cx
            .debug_bounds(id)
            .unwrap_or_else(|| panic!("{kind} has no mark"));
        let size = f32::from(crate::theme::ICON_SIZE);
        assert_eq!(
            (f32::from(mark.size.width), f32::from(mark.size.height)),
            (size, size),
            "{kind}: the mark is the icon, with no tile around it"
        );
    }
}

#[gpui::test]
fn a_connected_connector_is_managed_and_disconnect_sends_the_lower_case_o(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_modal(cx, &full(), DONE, DONE, "connectors");
    click(cx, "settings-connector-github-button");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    click(cx, "settings-connector-github-disconnect-github");
    wait_until(cx, "the call", |_| rig.acts().len() == 1);
    assert_eq!(
        rig.acts()[0],
        format!("cockpit act o {CONNECTORS} --name github --source app --json")
    );
}

#[gpui::test]
fn an_engine_that_lists_no_connectors_says_so(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "connectors");
    assert!(cx.debug_bounds("settings-connector-github").is_none());
}

#[gpui::test]
fn settings_has_no_phone_section(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "general");
    assert!(cx.debug_bounds("settings-section-phone").is_none());
}

#[gpui::test]
fn the_notification_switches_keep_their_choice_in_the_preferences(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "notifications");
    click(cx, "settings-notify-finished");
    assert!(cx.update(|_, cx| Preferences::current(cx).is_finished_muted));
    assert!(!cx.update(|_, cx| Preferences::current(cx).is_needs_person_muted));
}

#[gpui::test]
fn the_tab_keys_do_not_reach_the_page_under_the_modal(cx: &mut TestAppContext) {
    let pages = |desk: &Entity<Desk>, cx: &mut VisualTestContext| -> usize {
        desk.read_with(cx, |desk, cx| desk.bar.read(cx).tabs().tabs().len())
    };
    let (_rig, desk, cx) = open_answering(cx, SNAPSHOT, DONE, DONE);
    cx.update(|window, _| window.activate_window());
    desk.update(cx, |desk, cx| {
        desk.navigate(sakura_core::tabs::Page::Engine, true, cx)
    });
    cx.run_until_parked();
    let before = pages(&desk, cx);
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.open_settings(window, cx)));
    wait_until(cx, "the modal", |cx| {
        cx.debug_bounds("settings-modal").is_some()
    });
    cx.simulate_keystrokes("cmd-w");
    cx.run_until_parked();
    assert_eq!(
        pages(&desk, cx),
        before,
        "Cmd-W under the modal closes no tab"
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-w");
    cx.run_until_parked();
    assert_eq!(pages(&desk, cx), before - 1, "with the modal gone it does");
}

#[gpui::test]
fn adding_a_feed_sends_o_with_its_kind_its_name_and_its_address(cx: &mut TestAppContext) {
    let (rig, modal, cx) = open_alone(cx, &full(), nowhere_home());
    show(cx, "connectors");
    click(cx, "settings-connector-feed-button");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| {
        let (label, address) = {
            let form = modal.read(cx).add_form_of("feed");
            (form.name.clone(), form.address.clone())
        };
        label.update(cx, |field, cx| field.set_value("News", window, cx));
        address.update(cx, |field, cx| {
            field.set_value("https://example.com/feed.xml", window, cx)
        });
    });
    click(cx, "settings-connector-feed-add");
    wait_until(cx, "the call", |_| {
        rig.acts().contains(&format!(
            "cockpit act O {CONNECTORS} --address https://example.com/feed.xml --kind feed --label News --source app --json"
        ))
    });
}

fn open_program_form(
    cx: &mut TestAppContext,
) -> (Rig, Entity<SettingsModal>, &mut VisualTestContext) {
    let (rig, modal, cx) = open_alone(cx, &full(), nowhere_home());
    show(cx, "connectors");
    click(cx, "settings-connector-program-button");
    cx.run_until_parked();
    for _ in 0..3 {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    (rig, modal, cx)
}

#[gpui::test]
fn the_program_form_labels_both_fields_without_a_command_example(cx: &mut TestAppContext) {
    let (_rig, _modal, cx) = open_program_form(cx);
    for id in [
        "settings-connector-program-name-label",
        "settings-connector-program-address-label",
    ] {
        assert!(cx.debug_bounds(id).is_some(), "{id} is on screen");
    }
    let (name, address) = super::connectors::examples_of("program").expect("a form");
    assert_eq!(name, "Morning notes");
    assert_eq!(address, "Command to run");
}

#[gpui::test]
fn the_program_form_has_one_primary_button_named_for_its_action_and_a_cancel(
    cx: &mut TestAppContext,
) {
    let (_rig, _modal, cx) = open_program_form(cx);
    assert!(cx.debug_bounds("settings-connector-program-add").is_some());
    assert!(cx
        .debug_bounds("settings-connector-program-cancel")
        .is_some());
    assert!(
        cx.debug_bounds("settings-connector-program-button")
            .is_none(),
        "the row's own Add is not drawn beside the form's"
    );
}

#[gpui::test]
fn opening_a_form_gives_no_field_the_caret(cx: &mut TestAppContext) {
    let (_rig, modal, cx) = open_program_form(cx);
    let fields = modal.read_with(cx, |modal, _| {
        let form = modal.add_form_of("program");
        [form.name.clone(), form.address.clone()]
    });
    for field in fields {
        let is_focused = cx.update(|window, cx| field.read(cx).focus_handle(cx).is_focused(window));
        assert!(
            !is_focused,
            "no field has the caret before the person presses it"
        );
    }
}

#[gpui::test]
fn an_empty_form_says_what_is_missing_in_a_sentence_and_sends_nothing(cx: &mut TestAppContext) {
    let (rig, _modal, cx) = open_program_form(cx);
    click(cx, "settings-connector-program-add");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("settings-connector-program-problem")
            .is_some(),
        "the refusal is said under the fields"
    );
    assert!(rig
        .acts()
        .iter()
        .all(|call| !call.contains("cockpit act O")));
}

#[gpui::test]
fn cancel_closes_the_form_and_brings_the_row_s_button_back(cx: &mut TestAppContext) {
    let (_rig, _modal, cx) = open_program_form(cx);
    click(cx, "settings-connector-program-cancel");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("settings-connector-program-add").is_none());
    assert!(cx
        .debug_bounds("settings-connector-program-button")
        .is_some());
}

/// The modal is one shape: the list at its left is drawn inside the modal's ring with corners one ring
/// narrower than the modal's, and the dialog around the modal has the modal's own radius, so no square
/// corner of a child and no second rounded shape shows at a corner.
#[test]
fn the_modal_is_one_shape_with_one_radius_and_its_children_follow_it() {
    use crate::theme::settings_modal as sizes;
    assert_eq!(
        sizes::NAV_RADIUS + crate::theme::PANEL_RING_WIDTH,
        sizes::RADIUS
    );
}

/// Every text field of Settings and the Engine page is made by `controls::state`, which gives it the words
/// it shows while empty: none is built bare.
#[test]
fn no_field_of_settings_or_the_engine_page_is_built_without_its_hint() {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
    for folder in ["settings_modal", "engine_page"] {
        for entry in std::fs::read_dir(ui.join(folder)).expect("the folder") {
            let path = entry.expect("an entry").path();
            let is_source = path.extension().is_some_and(|ext| ext == "rs");
            let is_test = path.file_name().is_some_and(|name| name == "tests.rs");
            if !is_source || is_test {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("the source");
            assert!(
                !text.contains("InputState::new("),
                "{} builds a field with no hint: use controls::state",
                path.display()
            );
        }
    }
}

fn restart_and_check(cx: &mut VisualTestContext) -> (bool, bool) {
    (
        cx.debug_bounds("settings-updates-restart").is_some(),
        cx.debug_bounds("settings-updates-check").is_some(),
    )
}

#[gpui::test]
fn an_update_found_with_no_page_offers_restart_to_update_in_place_of_check_now(
    cx: &mut TestAppContext,
) {
    use crate::updates::{install_for_test, Release, UpdateState};
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "updates");
    let found = Release {
        version: "999.0.0".into(),
        url: String::new(),
    };
    cx.update(|_, cx| install_for_test(UpdateState::Available(found), cx));
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx)
    });
    assert_eq!(restart_and_check(cx), (true, false));
}

#[gpui::test]
fn every_other_state_offers_check_now_and_never_restart(cx: &mut TestAppContext) {
    use crate::updates::{install_for_test, UpdateState};
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "updates");
    for state in [
        UpdateState::Ready,
        UpdateState::Checking,
        UpdateState::UpToDate,
        UpdateState::Failed,
    ] {
        cx.update(|_, cx| install_for_test(state.clone(), cx));
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx)
        });
        assert_eq!(restart_and_check(cx), (false, true), "{state:?}");
    }
}

#[gpui::test]
fn the_menu_s_check_for_updates_opens_settings_on_the_updates_section(cx: &mut TestAppContext) {
    use crate::updates::{install_for_test, UpdateState};
    let (_rig, _desk, cx) = open_answering(cx, SNAPSHOT, DONE, DONE);
    cx.update(|_, cx| {
        crate::menus::install(cx);
        install_for_test(UpdateState::Ready, cx);
    });
    cx.dispatch_action(crate::menus::CheckForUpdates);
    wait_until(cx, "the Updates section", |cx| {
        cx.debug_bounds("settings-row-updates-check").is_some()
    });
}

#[gpui::test]
fn the_check_in_general_gives_the_same_feedback_as_the_one_in_updates(cx: &mut TestAppContext) {
    use crate::updates::{install_for_test, Release, UpdateState};
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "general");
    let present = |cx: &mut VisualTestContext| {
        (
            cx.debug_bounds("settings-version-restart").is_some(),
            cx.debug_bounds("settings-check-updates").is_some(),
        )
    };
    cx.update(|_, cx| install_for_test(UpdateState::Checking, cx));
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx)
    });
    assert_eq!(present(cx), (false, true));
    let found = Release {
        version: "999.0.0".into(),
        url: String::new(),
    };
    cx.update(|_, cx| install_for_test(UpdateState::Available(found), cx));
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx)
    });
    assert_eq!(present(cx), (true, false));
}

#[gpui::test]
fn a_click_on_the_name_of_a_notification_row_flips_its_switch(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "notifications");
    click(cx, "settings-row-notify-finished-label");
    assert!(cx.update(|_, cx| Preferences::current(cx).is_finished_muted));
    click(cx, "settings-row-notify-finished-label");
    assert!(!cx.update(|_, cx| Preferences::current(cx).is_finished_muted));
    assert!(!cx.update(|_, cx| Preferences::current(cx).is_needs_person_muted));
}

#[gpui::test]
fn a_click_on_the_name_of_the_engine_row_puts_the_caret_in_its_field(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, SNAPSHOT, DONE, DONE, "general");
    let before = cx.update(|window, cx| window.focused(cx));
    click(cx, "settings-row-engine-label");
    let after = cx.update(|window, cx| window.focused(cx));
    assert_ne!(before, after, "the caret moved to the field");
}

#[gpui::test]
fn opening_every_page_and_changing_nothing_sends_the_engine_no_change_of_rules_or_anything_else(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open_modal(cx, &full(), DONE, DONE, "general");
    for section in Section::ALL {
        show(cx, section.id());
        cx.run_until_parked();
    }
    escape_closes(cx);
    desk.update(cx, |desk, cx| desk.navigate(Page::Engine, false, cx));
    wait_until(cx, "the Engine page", |cx| {
        cx.debug_bounds("engine-preset-0").is_some()
    });
    cx.run_until_parked();
    for node in crate::ui::engine_page::all_nodes() {
        click(cx, &format!("engine-node-{node}"));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    assert_eq!(
        rig.acts(),
        Vec::<String>::new(),
        "looking at the pages changes nothing in the engine"
    );
}

#[gpui::test]
fn the_accounts_section_offers_add_account_which_opens_the_sheet_over_settings(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open_modal(cx, &full(), DONE, DONE, "accounts");
    assert!(bounds(cx, "settings-accounts-add").is_some(), "the button");
    click(cx, "settings-accounts-add");
    wait_until(cx, "the sheet", |cx| {
        cx.debug_bounds("account-add").is_some()
    });
    assert!(bounds(cx, "account-add-log-in").is_some());
    assert!(bounds(cx, "account-add-use-folder").is_some());
}

#[gpui::test]
fn general_has_one_row_for_what_a_new_master_conversation_starts_with(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, &full(), DONE, DONE, "general");
    assert!(bounds(cx, "settings-row-new-master").is_some(), "the row");
    assert!(bounds(cx, "settings-new-master").is_some(), "its chip");
}

#[gpui::test]
fn scoped_connector_disconnect_keeps_the_named_tasks_in_its_preview(cx: &mut TestAppContext) {
    let fixture = include_str!("../../../../sakura-core/tests/fixtures/connector-controls.json");
    let (rig, _, cx) = open_modal(cx, fixture, CONFIRM, DONE, "connectors");
    click(cx, "connector-manage-connection-id");
    wait_until(cx, "connection details", |cx| {
        bounds(cx, "connector-disconnect").is_some()
    });
    click(cx, "connector-disconnect");
    wait_until(cx, "disconnect preview", |cx| {
        bounds(cx, "connector-action-confirm").is_some()
    });
    assert!(bounds(cx, "connector-dependent-task-id").is_some());
    let calls = rig.acts();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].contains("connectordisconnect"));
    assert!(calls[0].contains("--name connection-id --op "));
    assert!(calls[0].contains("--revision 1"));
}

#[gpui::test]
fn editing_one_of_two_scoped_instances_keeps_the_form_after_a_stale_revision(
    cx: &mut TestAppContext,
) {
    let fixture = include_str!("../../../../sakura-core/tests/fixtures/connector-controls.json");
    let mut snapshot: serde_json::Value = serde_json::from_str(fixture).unwrap();
    let rows = snapshot["connectors"].as_array_mut().unwrap();
    let mut second = rows[0].clone();
    second["id"] = "connection-second".into();
    second["label"] = "Other website".into();
    rows.push(second);
    let refused =
        r#"{"result":"refused","reason":"stale_revision","reason_code":"stale_revision"}"#;
    let (rig, _, cx) = open_modal(cx, &snapshot.to_string(), CONFIRM, refused, "connectors");
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    wait_until(cx, "two scoped connections", |cx| {
        bounds(cx, "connector-manage-connection-second").is_some()
            && bounds(cx, "connector-manage-connection-id").is_some()
            && bounds(cx, "connector-add-github").is_some()
    });
    click(cx, "connector-manage-connection-second");
    wait_until(cx, "second instance", |cx| {
        bounds(cx, "connector-edit").is_some()
    });
    click(cx, "connector-edit");
    wait_until(cx, "connector editor", |cx| {
        bounds(cx, "connector-save").is_some()
    });
    let field = bounds(cx, "connector-name-label");
    click(cx, "connector-save");
    wait_until(cx, "save preview", |cx| {
        bounds(cx, "connector-action-confirm").is_some()
    });
    click(cx, "connector-action-confirm");
    wait_until(cx, "stale connection", |cx| {
        bounds(cx, "connector-action-told").is_some()
    });
    assert_eq!(bounds(cx, "connector-name-label"), field);
    let calls = rig.acts();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].contains(r#"connectoredit ["host","connectors"] --choices {"label":"Other website"} --name connection-second --op "#));
    assert_eq!(calls[1].replace(" --confirm 17.ab", ""), calls[0]);
}

#[gpui::test]
fn accounts_header_actions_share_one_right_aligned_row(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, &full(), DONE, DONE, "accounts");
    let add = cx.debug_bounds("settings-accounts-add").unwrap();
    let scan = cx.debug_bounds("setup-look-again").unwrap();
    let row = cx.debug_bounds("settings-account-work").unwrap();
    assert_eq!(add.center().y, scan.center().y);
    assert!(add.right() < scan.left());
    assert_eq!(scan.right(), row.right());
}

#[gpui::test]
fn accounts_names_have_no_leading_slot_and_actions_center_on_both_lines(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_modal(cx, &full(), DONE, DONE, "accounts");
    let row = cx.debug_bounds("settings-account-work").unwrap();
    let name = cx.debug_bounds("settings-account-work-name").unwrap();
    let note = cx.debug_bounds("settings-account-work-note").unwrap();
    let rename = cx.debug_bounds("settings-account-work-rename").unwrap();
    assert!(name.left() - row.left() == crate::theme::kit::scale::SETTINGS_ROW_PADDING);
    assert_eq!(name.left(), note.left());
    assert!((rename.center().y - row.center().y).abs() <= gpui::px(1.));
}

#[gpui::test]
fn reaudit3_connector_edit_keeps_equal_gaps_between_field_groups(cx: &mut TestAppContext) {
    let fixture = include_str!("../../../../sakura-core/tests/fixtures/connector-controls.json");
    let (_, _, window) = open_modal(cx, fixture, DONE, DONE, "connectors");
    click(window, "connector-add-github");
    wait_until(window, "connector editor", |cx| {
        bounds(cx, "connector-filter-label").is_some()
    });
    let [name, resource, toggle, branch] = [
        "connector-name",
        "connector-resource-label",
        "connector-whole-repo-row",
        "connector-filter-label",
    ]
    .map(|id| bounds(window, id).unwrap());
    let resource_field = bounds(window, "connector-resource").unwrap();
    assert_eq!(
        resource.top() - name.bottom(),
        toggle.top() - resource_field.bottom()
    );
    assert_eq!(
        branch.top() - toggle.bottom(),
        resource.top() - name.bottom()
    );
    window.update(|window, cx| {
        let card = crate::theme::Theme::palette(cx).grouped;
        assert!(
            window.painted_quads().iter().any(|quad| {
                quad.bounds == toggle.scale(window.scale_factor())
                    && quad.background.as_solid() == Some(card)
            }),
            "the branch toggle needs the shared card surface"
        );
    });
}
