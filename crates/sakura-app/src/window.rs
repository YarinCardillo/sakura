//! The one window: overlay title bar with inset traffic lights, created hidden and shown after its first
//! paint so it never flashes an empty frame, following the system appearance unless one is forced.

use crate::data::actions::Actions;
use crate::data::changes::Changes;
use crate::data::facts::Facts;
use crate::data::projects::Projects;
use crate::data::sessions::Sessions;
use crate::data::source::{Source, Started};
use crate::l10n::Language;
use crate::prefs::{Preferences, WindowPlace};
use crate::theme::{self, Appearance, Theme};
use crate::ui::console::Consoles;
use crate::ui::desk::{Desk, DeskData};
use crate::ui::settings::host::Host;
use gpui::{
    point, size, App, AppContext as _, Bounds, Entity, Pixels, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions,
};
use std::sync::Arc;
use std::time::Instant;
use tokio::runtime::Runtime;

/// Env var that keeps the app from activating at launch (for checks that must not steal focus).
const NO_ACTIVATE_VAR: &str = "SAKURA_NO_ACTIVATE";
/// Env var that forces the language (`en` or `it`), whatever the preference and the Mac say.
const LANGUAGE_VAR: &str = "SAKURA_LANGUAGE";
/// Env var that forces `dark` or `light`.
const APPEARANCE_VAR: &str = "SAKURA_APPEARANCE";

/// The window's title: the brand, the same in every language.
pub const APP_NAME: &str = "Sakura";
/// What the system knows the app by.
pub const APP_ID: &str = "dev.yarin.sakura";

/// Opens the window over the facts source and shows it after the first frame.
pub fn open(launched: Instant, runtime: Arc<Runtime>, cx: &mut App) {
    let started = match Source::from_environment(runtime) {
        Ok(started) => started,
        Err(error) => {
            eprintln!("sakura: cannot read the fixture: {error}");
            return cx.quit();
        }
    };
    // A window opened again while the app runs takes the preferences as they are now (its tabs and
    // panes); the first one reads them from their file.
    let preferences = Preferences::try_current(cx).unwrap_or_else(Preferences::load);
    let forced = Appearance::forced(std::env::var(APPEARANCE_VAR).ok().as_deref());
    // The appearance forced for this launch, else the one chosen in Settings, else the Mac's.
    let chosen = forced.or(preferences.appearance);
    let appearance = chosen.unwrap_or(Appearance::from_window(cx.window_appearance()));
    crate::fonts::choose_terminal(
        preferences.terminal_font.clone(),
        preferences.terminal_font_size,
    );
    Theme::set(appearance, cx);
    let asked = std::env::var(LANGUAGE_VAR).ok();
    let asked = language_asked(asked.as_deref(), crate::snapshot::is_asked());
    crate::language::start(preferences.language, asked, cx);
    let result = cx.open_window(options(preferences.window, cx), |window, cx| {
        let shell = Shell {
            is_following_system: forced.is_none(),
            launched,
            preferences,
        };
        root(started, shell, window, cx)
    });
    match result {
        Ok(_) if std::env::var_os(NO_ACTIVATE_VAR).is_none() => cx.activate(true),
        Ok(_) => {}
        Err(error) => {
            eprintln!("sakura: cannot open the window: {error}");
            cx.quit();
        }
    }
}

/// The app was opened again while it runs (a click on its Dock icon, a second launch) and shows no
/// window: Sakura stays running when its window is closed, as Mac apps with one main window do, and this
/// brings the window back, with the tabs and panes the preferences kept.
pub fn reopen(runtime: Arc<Runtime>, cx: &mut App) {
    ensure_window(cx, |cx| open(Instant::now(), runtime, cx));
}

/// Opens a window with `open` when the app has none, and says whether it did; a window that is there
/// (minimised or hidden) is brought forward instead.
fn ensure_window(cx: &mut App, open: impl FnOnce(&mut App)) -> bool {
    let Some(window) = cx.windows().first().copied() else {
        open(cx);
        return true;
    };
    cx.activate(true);
    if let Err(error) = window.update(cx, |_, window, _| window.activate_window()) {
        eprintln!("sakura: cannot bring the window forward: {error}");
    }
    false
}

/// The language requested this launch alone: the one `variable` names, and English for a snapshot that
/// names none, so that an image never depends on the language of the Mac it was taken on. Otherwise
/// none: the preference decides.
fn language_asked(variable: Option<&str>, is_snapshot: bool) -> Option<Language> {
    let named = variable.and_then(Language::from_code);
    named.or(is_snapshot.then_some(Language::English))
}

/// What the window is built with besides the facts.
struct Shell {
    is_following_system: bool,
    launched: Instant,
    preferences: Preferences,
}

/// The window's content, and the hooks that show it after the first frame and follow the system appearance.
fn root(
    started: Started,
    shell: Shell,
    window: &mut Window,
    cx: &mut App,
) -> Entity<gpui_component::Root> {
    show_after_first_frame(shell.launched, window);
    if shell.is_following_system {
        follow_system_appearance(window);
    }
    crate::snapshot::schedule(window, cx);
    let Started {
        source,
        events,
        watcher,
    } = started;
    let first = crate::snapshot::selection().or_else(|| shell.preferences.last_task.clone());
    let facts = cx.new(|cx| Facts::new(source.clone(), events, watcher, first, cx));
    let actions = cx.new(|cx| Actions::new(source.clone(), facts.clone(), cx));
    let projects = cx.new(|_| Projects::new(source.clone()));
    let consoles = cx.new(|cx| Consoles::new(&source, facts.clone(), actions.clone(), cx));
    let changes = cx.new(|cx| Changes::new(source.clone(), facts.clone(), cx));
    let kept = crate::prefs::default_path();
    let folder = kept.as_deref().and_then(std::path::Path::parent);
    let sessions = Sessions::new(folder, source.runtime().clone());
    let settings = (source.clone(), projects.clone());
    let data = DeskData {
        facts,
        actions,
        projects,
        changes,
        consoles,
        source,
        sessions,
    };
    let preferences = shell.preferences;
    let desk = cx.new(|cx| Desk::new(data, &preferences, window, cx));
    let host = Host::new(
        settings.0,
        settings.1,
        desk.downgrade(),
        window.window_handle(),
    );
    cx.set_global(host);
    preferences.install(crate::prefs::default_path(), cx);
    // The menu bar is built again now that the language and the choice it checks are known.
    crate::menus::rebuild(cx);
    #[cfg(feature = "snapshot")]
    crate::drive::schedule(desk.clone(), window, cx);
    cx.new(|cx| gpui_component::Root::new(desk, window, cx))
}

/// Where a window that was kept at `place` opens: there, no smaller than the window may be, when a
/// screen still shows part of it; else nowhere of its own (a screen was unplugged), and it opens centred.
fn kept_bounds(place: Option<WindowPlace>, screens: &[Bounds<Pixels>]) -> Option<Bounds<Pixels>> {
    let place = place?;
    let width = gpui::px(place.width).max(theme::WINDOW_MIN_WIDTH);
    let height = gpui::px(place.height).max(theme::WINDOW_MIN_HEIGHT);
    let origin = point(gpui::px(place.x), gpui::px(place.y));
    let bounds = Bounds::new(origin, size(width, height));
    let is_seen = screens.iter().any(|screen| screen.intersects(&bounds));
    is_seen.then_some(bounds)
}

/// The window opens where the person left it (`place`); the first time, and for a snapshot, centred at
/// its starting size.
fn options(place: Option<WindowPlace>, cx: &App) -> WindowOptions {
    let width = crate::snapshot::window_width().map_or(theme::WINDOW_START_WIDTH, |width| {
        gpui::px(width).max(theme::WINDOW_MIN_WIDTH)
    });
    let height = crate::snapshot::window_height().map_or(theme::WINDOW_START_HEIGHT, |height| {
        gpui::px(height).max(theme::WINDOW_MIN_HEIGHT)
    });
    let start = size(width, height);
    let screens: Vec<Bounds<Pixels>> = cx.displays().iter().map(|screen| screen.bounds()).collect();
    let place = place.filter(|_| !crate::snapshot::is_asked());
    let kept = kept_bounds(place, &screens);
    let bounds = kept.unwrap_or_else(|| Bounds::centered(None, start, cx));
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(APP_NAME.into()),
            appears_transparent: true,
            traffic_light_position: Some(point(theme::TRAFFIC_LIGHTS_X, theme::TRAFFIC_LIGHTS_Y)),
        }),
        focus: false,
        show: false,
        app_owns_titlebar_drag: true,
        window_background: WindowBackgroundAppearance::Opaque,
        window_min_size: Some(size(theme::WINDOW_MIN_WIDTH, theme::WINDOW_MIN_HEIGHT)),
        app_id: Some(APP_ID.into()),
        ..WindowOptions::default()
    }
}

fn show_after_first_frame(launched: Instant, window: &Window) {
    window.on_next_frame(move |window, _| {
        window.activate_window();
        let elapsed = launched.elapsed();
        eprintln!("sakura: first frame after {} ms", elapsed.as_millis());
    });
}

/// The windows follow the Mac's appearance while Settings names none of its own.
fn follow_system_appearance(window: &Window) {
    window
        .observe_window_appearance(|window, cx| {
            if Preferences::current(cx).appearance.is_none() {
                Theme::set(Appearance::from_window(window.appearance()), cx);
                cx.refresh_windows();
            }
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Empty, IntoElement, Render, TestAppContext};
    use std::cell::Cell;
    use std::rc::Rc;

    fn screen() -> Bounds<Pixels> {
        Bounds::new(
            point(gpui::px(0.), gpui::px(0.)),
            size(gpui::px(1512.), gpui::px(982.)),
        )
    }

    fn place(x: f32, y: f32, width: f32, height: f32) -> Option<WindowPlace> {
        Some(WindowPlace {
            x,
            y,
            width,
            height,
        })
    }

    #[test]
    fn a_window_opens_where_it_was_left_while_a_screen_still_shows_it() {
        let kept = kept_bounds(place(200., 120., 1100., 700.), &[screen()]);
        let expected = Bounds::new(
            point(gpui::px(200.), gpui::px(120.)),
            size(gpui::px(1100.), gpui::px(700.)),
        );
        assert_eq!(kept, Some(expected));
        assert_eq!(
            kept_bounds(None, &[screen()]),
            None,
            "the first time, centred"
        );
    }

    #[test]
    fn a_window_left_on_a_screen_that_is_gone_opens_centred() {
        let on_the_second_screen = place(2000., 100., 1100., 700.);
        assert_eq!(kept_bounds(on_the_second_screen, &[screen()]), None);
        assert_eq!(kept_bounds(on_the_second_screen, &[]), None);
    }

    #[test]
    fn a_kept_size_smaller_than_the_window_may_be_is_raised_to_it() {
        let kept = kept_bounds(place(10., 10., 20., 20.), &[screen()]).expect("on the screen");
        assert_eq!(
            kept.size,
            size(theme::WINDOW_MIN_WIDTH, theme::WINDOW_MIN_HEIGHT)
        );
    }

    struct Blank;

    impl Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            Empty
        }
    }

    #[gpui::test]
    fn opening_the_app_again_makes_a_window_only_when_there_is_none(cx: &mut TestAppContext) {
        let opened = Rc::new(Cell::new(0));
        let open = |opened: Rc<Cell<usize>>| {
            move |cx: &mut App| {
                opened.set(opened.get() + 1);
                let made = cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| Blank));
                made.expect("a window");
            }
        };
        let windows = |cx: &mut TestAppContext| cx.update(|cx| cx.windows().len());
        assert!(cx.update(|cx| ensure_window(cx, open(opened.clone()))));
        assert_eq!((windows(cx), opened.get()), (1, 1), "none was open");
        assert!(!cx.update(|cx| ensure_window(cx, open(opened.clone()))));
        assert_eq!(
            (windows(cx), opened.get()),
            (1, 1),
            "one is open: no second"
        );
        // The person closes the last window: the app stays running with none.
        let last = cx.update(|cx| cx.windows()[0]);
        cx.update(|cx| last.update(cx, |_, window, _| window.remove_window()))
            .expect("the window closes");
        cx.run_until_parked();
        assert_eq!(windows(cx), 0);
        assert!(cx.update(|cx| ensure_window(cx, open(opened.clone()))));
        assert_eq!((windows(cx), opened.get()), (1, 2), "the window is back");
    }

    #[test]
    fn a_launch_asks_for_the_language_its_variable_names_and_a_snapshot_for_english() {
        assert_eq!(language_asked(Some("it"), false), Some(Language::Italian));
        assert_eq!(language_asked(Some("it"), true), Some(Language::Italian));
        assert_eq!(language_asked(None, true), Some(Language::English));
        assert_eq!(language_asked(Some("fr"), true), Some(Language::English));
        assert_eq!(language_asked(None, false), None, "the preference decides");
        assert_eq!(language_asked(Some("fr"), false), None);
    }
}
