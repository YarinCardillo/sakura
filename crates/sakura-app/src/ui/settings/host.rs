//! Settings in the app: what opens the window (Cmd+comma, the app menu, a link to a project's page),
//! what the window is given, and what the app does with what it says. Each preference is kept in the
//! app's own preferences and in force at once; what belongs to the service goes through the service
//! and the app's list of projects is read again after it.

use super::{
    system_folder_picker, BoxSettings, Deps, LanguageControl, Notify, Settings, SettingsEvent,
    TerminalText, ThemeChoice,
};
use crate::data::projects::Projects;
use crate::data::source::Source;
use crate::prefs::Preferences;
use crate::theme::{Appearance, Theme};
use crate::ui::centre::CentreView;
use crate::ui::console::rules::ConsoleKind;
use crate::ui::desk::Desk;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Context, Entity, Global, Subscription, WeakEntity,
    Window,
};
use std::rc::Rc;

/// What Settings needs of the app while its window can be opened.
pub struct Host {
    source: Source,
    projects: Entity<Projects>,
    desk: WeakEntity<Desk>,
    /// The main window: a sign-in is shown in it.
    window: AnyWindowHandle,
    /// The app listens to the Settings window that is open.
    listening: Option<Subscription>,
}

impl Global for Host {}

impl Host {
    /// The host of the main `window`, whose `desk` shows the lavori of `source`.
    pub fn new(
        source: Source,
        projects: Entity<Projects>,
        desk: WeakEntity<Desk>,
        window: AnyWindowHandle,
    ) -> Self {
        Self {
            source,
            projects,
            desk,
            window,
            listening: None,
        }
    }
}

/// Opens Settings, or brings its window forward. Nothing opens on the sample data, which has no
/// service to read the projects and the accounts from.
pub fn show(cx: &mut App) -> Option<Entity<Settings>> {
    let host = cx.try_global::<Host>()?;
    let (cli, runtime) = (host.source.cli()?.clone(), host.source.runtime().clone());
    let deps = deps(Rc::new(BoxSettings::new(cli, runtime)), cx);
    // A launch that only renders the window to a file never shows it.
    let view = super::open_in(deps, !crate::snapshot::is_asked(), cx)?;
    let listening = cx.subscribe(&view, |_, event: &SettingsEvent, cx| said(event, cx));
    cx.global_mut::<Host>().listening = Some(listening);
    Some(view)
}

/// Opens Settings on the page of the project named.
pub fn show_project(name: &str, cx: &mut App) {
    let Some(view) = show(cx) else {
        return;
    };
    let (window, name) = (view.read(cx).window, name.to_string());
    drop(window.update(cx, |_, window, cx| {
        view.update(cx, |settings, cx| settings.show_project(&name, window, cx));
    }));
}

/// One step of a snapshot's script on the Settings window: a section by its name (`agent`),
/// `project=<name>`, `add=<folder>` (the folder the Mac's picker would have given), `scroll=<points>`,
/// `font=<family>` and `size=<points>` (the terminal's text, as the person would choose it; no family
/// is Sakura's own), `smallest` (the window at the smallest it gets) and `snap=<file>`, which saves the
/// window as it is now.
#[cfg(feature = "snapshot")]
pub fn drive(step: &str, cx: &mut App) {
    let Some(view) = show(cx) else {
        return eprintln!("sakura: Settings does not open on the sample data");
    };
    let window = view.read(cx).window;
    let (verb, argument) = step.split_once('=').unwrap_or((step, ""));
    let argument = argument.to_string();
    let done = window.update(cx, |_, window, cx| match verb {
        "project" => view.update(cx, |settings, cx| {
            settings.show_project(&argument, window, cx);
        }),
        "add" => view.update(cx, |settings, cx| {
            let folder = std::path::PathBuf::from(&argument);
            settings.pick_folder = Rc::new(move |_| gpui::Task::ready(Some(folder.clone())));
            settings.add_project(window, cx);
        }),
        "scroll" => view.update(cx, |settings, _| {
            let down = gpui::px(argument.parse().unwrap_or(0.0));
            let mut offset = gpui::Point::default();
            offset.y = -down;
            settings.scroll.set_offset(offset);
        }),
        "font" => view.update(cx, |settings, cx| {
            let family = (!argument.is_empty()).then(|| argument.clone());
            settings.set_terminal_family(family, cx);
        }),
        "size" => view.update(cx, |settings, cx| match argument.parse() {
            Ok(size) => settings.set_terminal_size(size, cx),
            Err(_) => eprintln!("sakura: {argument:?} is not a size"),
        }),
        "smallest" => {
            use crate::theme::settings::{SETTINGS_MIN_HEIGHT, SETTINGS_MIN_WIDTH};
            window.resize(gpui::size(SETTINGS_MIN_WIDTH, SETTINGS_MIN_HEIGHT));
        }
        "snap" => {
            window.draw(cx).clear(cx);
            crate::snapshot::save(window, std::path::Path::new(&argument));
        }
        section => {
            let named = |listed: &super::Section| {
                listed.id().strip_prefix("settings-section-") == Some(section)
            };
            match super::Section::ALL.into_iter().find(named) {
                Some(section) => view.update(cx, |settings, cx| settings.show(section, cx)),
                None => eprintln!("sakura: unknown Settings step {step:?}"),
            }
        }
    });
    if let Err(error) = done {
        eprintln!("sakura: Settings step {step:?} failed: {error}");
    }
}

/// What the window is given: the preferences as they are now.
fn deps(source: Rc<BoxSettings>, cx: &App) -> Deps {
    let preferences = Preferences::current(cx);
    let theme = match preferences.appearance {
        None => ThemeChoice::System,
        Some(Appearance::Light) => ThemeChoice::Light,
        Some(Appearance::Dark) => ThemeChoice::Dark,
    };
    Deps {
        source,
        shortcuts: Rc::new(crate::menus::shortcuts),
        theme,
        language: LanguageControl {
            current: Rc::new(crate::language::choice),
            set: Rc::new(crate::language::set_choice),
        },
        publish_requests: preferences.publish_requests.into_iter().collect(),
        publish_default: preferences.publish_request,
        notify: Notify {
            needs_you: !preferences.is_needs_person_muted,
            finished: !preferences.is_finished_muted,
        },
        terminal: TerminalText {
            family: preferences.terminal_font,
            size: preferences.terminal_font_size,
        },
        families: Rc::new(|cx: &mut App| {
            let text = cx.text_system().clone();
            cx.background_spawn(async move { crate::fonts::monospaced_families(&text) })
        }),
        data_folder: data_folder().into(),
        pick_folder: system_folder_picker(),
    }
}

/// The folder the service keeps its data in, the person's home written as `~`.
fn data_folder() -> String {
    let folder = sakura_core::watcher::agentbox_home();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match home.and_then(|home| folder.strip_prefix(home).ok().map(|rest| rest.to_owned())) {
        Some(rest) => format!("~/{}", rest.display()),
        None => folder.display().to_string(),
    }
}

/// The terminal's text is kept and every open terminal is styled again with it at once.
fn choose_terminal_text(text: TerminalText, cx: &mut App) {
    Preferences::change(cx, |preferences| {
        preferences.terminal_font = text.family.clone();
        preferences.terminal_font_size = text.size;
    });
    crate::fonts::choose_terminal(text.family, text.size);
    // The terminals are styled again whenever the theme is set.
    Theme::set(Theme::palette(cx).appearance, cx);
    cx.refresh_windows();
}

/// Keeps what Settings changed and puts it in force.
fn said(event: &SettingsEvent, cx: &mut App) {
    match event.clone() {
        SettingsEvent::SignInAsked { account } => sign_in(account, cx),
        SettingsEvent::ThemeChosen(choice) => choose_theme(choice, cx),
        SettingsEvent::PublishRequestChanged { project, request } => {
            Preferences::change(cx, |preferences| match request {
                Some(request) => drop(preferences.publish_requests.insert(project, request)),
                None => drop(preferences.publish_requests.remove(&project)),
            });
        }
        SettingsEvent::PublishDefaultChanged(request) => {
            Preferences::change(cx, |preferences| preferences.publish_request = request);
        }
        SettingsEvent::NotifyChanged(notify) => {
            Preferences::change(cx, |preferences| {
                preferences.is_needs_person_muted = !notify.needs_you;
                preferences.is_finished_muted = !notify.finished;
            });
        }
        SettingsEvent::TerminalTextChosen(text) => choose_terminal_text(text, cx),
        SettingsEvent::ProjectsChanged => {
            let host = cx.global::<Host>();
            let (projects, desk) = (host.projects.clone(), host.desk.clone());
            projects.update(cx, |projects, cx| projects.load(cx));
            // What a lavoro can check and play is its project's: the lavori are read again with it.
            if let Some(desk) = desk.upgrade() {
                let facts = desk.read(cx).facts.clone();
                facts.update(cx, |facts, cx| facts.refresh_all(cx));
            }
        }
    }
}

/// The appearance chosen is kept and in force in every window at once; "System" takes the Mac's.
fn choose_theme(choice: ThemeChoice, cx: &mut App) {
    let appearance = match choice {
        ThemeChoice::System => None,
        ThemeChoice::Light => Some(Appearance::Light),
        ThemeChoice::Dark => Some(Appearance::Dark),
    };
    Preferences::change(cx, |preferences| preferences.appearance = appearance);
    let shown = appearance.unwrap_or_else(|| Appearance::from_window(cx.window_appearance()));
    Theme::set(shown, cx);
    cx.refresh_windows();
}

/// The sign-in of an account of the boxes is the app's own, in the terminal of the main window, which
/// comes forward.
fn sign_in(account: String, cx: &mut App) {
    let host = cx.global::<Host>();
    let (desk, window) = (host.desk.clone(), host.window);
    drop(window.update(cx, |_, window, cx| {
        drop(desk.update(cx, |desk, cx| desk.sign_in_to(account, window, cx)));
        window.activate_window();
    }));
}

impl Desk {
    /// Opens the terminal sign-in of `account` in the lavoro on screen. With no lavoro on screen
    /// there is no terminal to sign in from, and nothing opens.
    pub(in crate::ui) fn sign_in_to(
        &mut self,
        account: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(task) = self.shown(cx) else {
            return;
        };
        self.open_console(&task, ConsoleKind::Login(account), window, cx);
        self.show_centre_view(&task, CentreView::Agent, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;
    use gpui::TestAppContext;
    use std::cell::Cell;

    #[gpui::test]
    fn a_project_changed_in_settings_has_the_lavori_read_again(cx: &mut TestAppContext) {
        use crate::ui::interaction_tests::wait_for;
        use crate::ui::launch_tests::{
            calls, fake_agentbox, open_live, settled_calls, Names, TASK,
        };
        let (dir, script) = fake_agentbox(Names::Listed, "0", "");
        let (cx, desk, _) = open_live(cx, script, Some(TASK), false);
        wait_for(cx, "the page", |cx| {
            cx.debug_bounds("page-header").is_some()
        });
        let statuses =
            |calls: Vec<String>| calls.iter().filter(|call| *call == "status --json").count();
        let before = statuses(settled_calls(cx, &dir));
        cx.update(|window, cx| {
            let (source, projects) = {
                let desk = desk.read(cx);
                (desk.source.clone(), desk.projects.clone())
            };
            let host = Host::new(source, projects, desk.downgrade(), window.window_handle());
            cx.set_global(host);
            said(&SettingsEvent::ProjectsChanged, cx);
        });
        wait_for(cx, "the lavori to be read again", |_| {
            statuses(calls(&dir)) > before
        });
    }

    #[gpui::test]
    fn the_terminal_text_chosen_is_kept_and_styles_the_terminals_at_once(cx: &mut TestAppContext) {
        let restyled = Rc::new(Cell::new(0));
        let seen = restyled.clone();
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx);
            Preferences::default().install(None, cx);
            Theme::set(Appearance::Light, cx);
            // What the open terminals hang on: they are styled again when the theme is set.
            cx.observe_global::<Theme>(move |_| seen.set(seen.get() + 1))
                .detach();
        });
        cx.run_until_parked();
        let before = restyled.get();
        let chosen = TerminalText {
            family: Some("Menlo".into()),
            size: Some(15),
        };
        cx.update(|cx| choose_terminal_text(chosen, cx));
        cx.run_until_parked();
        assert_eq!(restyled.get(), before + 1, "the terminals are told at once");
        cx.update(|cx| {
            let style = theme::terminal_style(Theme::palette(cx).appearance);
            assert_eq!(
                (style.font_family.as_ref(), style.font_size),
                ("Menlo", 15.)
            );
            assert_eq!(Theme::palette(cx).appearance, Appearance::Light);
            let kept = Preferences::current(cx);
            assert_eq!(kept.terminal_font.as_deref(), Some("Menlo"));
            assert_eq!(kept.terminal_font_size, Some(15));
            choose_terminal_text(TerminalText::default(), cx);
            let style = theme::terminal_style(Appearance::Light);
            assert_eq!(style.font_family.as_ref(), theme::FONT_MONO);
            assert_eq!(style.font_size, theme::TERMINAL_TEXT);
        });
    }
}
