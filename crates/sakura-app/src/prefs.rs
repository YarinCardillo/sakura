//! Window preferences, the only state the app stores: the share of the Agente view the changes pane takes,
//! the task shown last and the open tabs. One small JSON file in Sakura's data folder, read before the
//! window is built and written on the background executor, one write after the other, never on the UI
//! thread. A missing or unreadable file means the defaults; a failed write is reported on stderr and changes
//! nothing else.

use crate::l10n::LanguageChoice;
use crate::theme::{self, Appearance};
use gpui::{App, AppContext as _, Global, Task};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "preferences.json";
/// The app's own folder inside the data folder, beside what `box` keeps there.
const APP_FOLDER: &str = "app";
const SHARE_KEY: &str = "changesShare";
const LAST_TASK_KEY: &str = "lastTask";
const SIDEBAR_HIDDEN_KEY: &str = "sidebarHidden";
const CHANGES_HIDDEN_KEY: &str = "changesHidden";
const TABS_KEY: &str = "tabs";
const LANGUAGE_KEY: &str = "language";
const LAST_PROJECT_KEY: &str = "lastProject";
const BOX_ACCOUNT_KEY: &str = "boxAccount";
const APPEARANCE_KEY: &str = "appearance";
const PUBLISH_REQUEST_KEY: &str = "publishRequest";
const PUBLISH_REQUESTS_KEY: &str = "publishRequests";
const NEEDS_PERSON_MUTED_KEY: &str = "muteNeedsPerson";
const FINISHED_MUTED_KEY: &str = "muteFinished";
const WINDOW_KEY: &str = "window";
const PANE_VIEWS_KEY: &str = "paneViews";
const TERMINAL_FONT_KEY: &str = "terminalFont";
const TERMINAL_FONT_SIZE_KEY: &str = "terminalFontSize";

/// Where the window stood on the screen and how large it was, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowPlace {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// What the window remembers across launches.
#[derive(Debug, Clone, PartialEq)]
pub struct Preferences {
    /// The changes pane's share of the Agente view, within the theme's limits.
    pub changes_share: f32,
    /// The task the window showed last: the one it opens on.
    pub last_task: Option<String>,
    /// Whether the sidebar and the changes pane were hidden.
    pub is_sidebar_hidden: bool,
    pub is_changes_hidden: bool,
    /// The open tabs, as the tab model writes them (`sakura_core::tabs::Tabs::to_json`); none before the
    /// first tab was opened.
    pub tabs: Option<Value>,
    /// The language chosen: automatic (the Mac's own) unless one was chosen.
    pub language: LanguageChoice,
    /// The project the last lavoro was made in: the one "New task" offers first.
    pub last_project: Option<String>,
    /// The account of the boxes chosen last in "New task": the one the agents in a box start with while
    /// it is signed in.
    pub box_account: Option<String>,
    /// The appearance chosen in Settings; none follows the Mac's own.
    pub appearance: Option<Appearance>,
    /// What "Publish" asks the agent when the lavoro's project has no request of its own; none is
    /// Sakura's own request.
    pub publish_request: Option<String>,
    /// The publishing request of each project that has one of its own, by the project's name.
    pub publish_requests: BTreeMap<String, String>,
    /// The news the person does not want the system to tell: a lavoro that needs them, an agent that
    /// finished. Both are told unless muted.
    pub is_needs_person_muted: bool,
    pub is_finished_muted: bool,
    /// Where the window was when it was last moved or sized; none before that, and the window then
    /// opens centred at its first size.
    pub window: Option<WindowPlace>,
    /// The view of the right pane each open lavoro showed, by the view's name.
    pub pane_views: BTreeMap<String, String>,
    /// The family and the size chosen for the terminal in Settings; none of either is Sakura's own.
    pub terminal_font: Option<String>,
    pub terminal_font_size: Option<u8>,
}

/// The appearance as the file writes it.
fn appearance_code(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Dark => "dark",
        Appearance::Light => "light",
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            changes_share: theme::CHANGES_SHARE_DEFAULT,
            last_task: None,
            is_sidebar_hidden: false,
            is_changes_hidden: false,
            tabs: None,
            language: LanguageChoice::Automatic,
            last_project: None,
            box_account: None,
            appearance: None,
            publish_request: None,
            publish_requests: BTreeMap::new(),
            is_needs_person_muted: false,
            is_finished_muted: false,
            window: None,
            pane_views: BTreeMap::new(),
            terminal_font: None,
            terminal_font_size: None,
        }
    }
}

/// The preferences of the running window, and the file they are saved to (none in a test window).
struct Stored {
    preferences: Preferences,
    path: Option<PathBuf>,
    /// The write in flight: the next one starts when it ends, so the file always holds the latest.
    saving: Option<Task<()>>,
}

impl Global for Stored {}

impl Preferences {
    /// The preferences in the default file, or the defaults.
    pub fn load() -> Self {
        default_path().map_or_else(Self::default, |path| Self::load_from(&path))
    }

    /// Makes these the window's preferences, saved to `path` on every change; `None` saves nowhere.
    pub fn install(self, path: Option<PathBuf>, cx: &mut App) {
        cx.set_global(Stored {
            preferences: self,
            path,
            saving: None,
        });
    }

    /// The window's preferences now.
    pub fn current(cx: &App) -> Preferences {
        cx.global::<Stored>().preferences.clone()
    }

    /// The window's preferences now, or none before they are installed.
    pub fn try_current(cx: &App) -> Option<Preferences> {
        let stored = cx.try_global::<Stored>()?;
        Some(stored.preferences.clone())
    }

    /// Applies `change` to the window's preferences and, when it changed something, saves them on the
    /// background executor after any write still running.
    pub fn change(cx: &mut App, change: impl FnOnce(&mut Preferences)) {
        let stored = cx.global_mut::<Stored>();
        let before = stored.preferences.clone();
        change(&mut stored.preferences);
        if stored.preferences == before {
            return;
        }
        let Some(path) = stored.path.clone() else {
            return;
        };
        let (preferences, earlier) = (stored.preferences.clone(), stored.saving.take());
        let saving = cx.background_spawn(async move {
            if let Some(earlier) = earlier {
                earlier.await;
            }
            preferences.save_to(&path);
        });
        cx.global_mut::<Stored>().saving = Some(saving);
    }

    /// The preferences in `path`, or the defaults when it is missing or unreadable.
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map_or_else(Self::default, |value| Self::from_json(&value))
    }

    /// Writes the preferences to `path`, creating its folder.
    pub fn save_to(&self, path: &Path) {
        let text = json!({
            SHARE_KEY: self.changes_share,
            LAST_TASK_KEY: self.last_task,
            SIDEBAR_HIDDEN_KEY: self.is_sidebar_hidden,
            CHANGES_HIDDEN_KEY: self.is_changes_hidden,
            TABS_KEY: self.tabs,
            LANGUAGE_KEY: self.language.code(),
            LAST_PROJECT_KEY: self.last_project,
            BOX_ACCOUNT_KEY: self.box_account,
            APPEARANCE_KEY: self.appearance.map(appearance_code),
            PUBLISH_REQUEST_KEY: self.publish_request,
            PUBLISH_REQUESTS_KEY: self.publish_requests,
            NEEDS_PERSON_MUTED_KEY: self.is_needs_person_muted,
            FINISHED_MUTED_KEY: self.is_finished_muted,
            WINDOW_KEY: self.window.map(|at| [at.x, at.y, at.width, at.height]),
            PANE_VIEWS_KEY: self.pane_views,
            TERMINAL_FONT_KEY: self.terminal_font,
            TERMINAL_FONT_SIZE_KEY: self.terminal_font_size,
        })
        .to_string();
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(path, text));
        if let Err(error) = written {
            eprintln!(
                "sakura: cannot save the preferences to {}: {error}",
                path.display()
            );
        }
    }

    fn from_json(value: &Value) -> Self {
        let share = value.get(SHARE_KEY).and_then(Value::as_f64);
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
        let flag = |key: &str| value.get(key).and_then(Value::as_bool).unwrap_or(false);
        Self {
            changes_share: share.map_or(theme::CHANGES_SHARE_DEFAULT, |share| {
                clamp_share(share as f32)
            }),
            last_task: text(LAST_TASK_KEY),
            is_sidebar_hidden: flag(SIDEBAR_HIDDEN_KEY),
            is_changes_hidden: flag(CHANGES_HIDDEN_KEY),
            tabs: value.get(TABS_KEY).filter(|tabs| !tabs.is_null()).cloned(),
            language: LanguageChoice::from_code(value.get(LANGUAGE_KEY).and_then(Value::as_str)),
            last_project: text(LAST_PROJECT_KEY),
            box_account: text(BOX_ACCOUNT_KEY),
            appearance: Appearance::forced(text(APPEARANCE_KEY).as_deref()),
            publish_request: text(PUBLISH_REQUEST_KEY),
            publish_requests: requests(value.get(PUBLISH_REQUESTS_KEY)),
            is_needs_person_muted: flag(NEEDS_PERSON_MUTED_KEY),
            is_finished_muted: flag(FINISHED_MUTED_KEY),
            window: value.get(WINDOW_KEY).and_then(window_place),
            pane_views: value.get(PANE_VIEWS_KEY).map(names).unwrap_or_default(),
            terminal_font: text(TERMINAL_FONT_KEY),
            terminal_font_size: value
                .get(TERMINAL_FONT_SIZE_KEY)
                .and_then(Value::as_u64)
                .and_then(|size| u8::try_from(size).ok()),
        }
    }
}

/// A window's place from the four numbers it is kept as; none when they are not four finite numbers
/// with a size.
fn window_place(value: &Value) -> Option<WindowPlace> {
    let numbers: Option<Vec<f32>> = value
        .as_array()?
        .iter()
        .map(|number| number.as_f64().map(|number| number as f32))
        .collect();
    let [x, y, width, height] = numbers?[..] else {
        return None;
    };
    let is_sound = [x, y, width, height].iter().all(|n| n.is_finite());
    (is_sound && width > 0.0 && height > 0.0).then_some(WindowPlace {
        x,
        y,
        width,
        height,
    })
}

/// The pairs of names in a JSON object whose values are text; anything else is left out.
fn names(value: &Value) -> BTreeMap<String, String> {
    let pairs = value.as_object().into_iter().flatten();
    pairs
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
        .collect()
}

/// The publishing requests the file holds: a project's name and its text; anything else is left out.
fn requests(value: Option<&Value>) -> BTreeMap<String, String> {
    let Some(Value::Object(kept)) = value else {
        return BTreeMap::new();
    };
    let texts = kept.iter().filter_map(|(project, request)| {
        let request = request.as_str()?;
        Some((project.clone(), request.to_string()))
    });
    texts.collect()
}

/// A share within the changes pane's limits; anything not a number is the default.
pub fn clamp_share(share: f32) -> f32 {
    if share.is_finite() {
        share.clamp(theme::CHANGES_SHARE_MIN, theme::CHANGES_SHARE_MAX)
    } else {
        theme::CHANGES_SHARE_DEFAULT
    }
}

/// The preferences file of this user, when the home folder is known.
pub fn default_path() -> Option<PathBuf> {
    let home = sakura_core::platform::data_home();
    (!home.as_os_str().is_empty()).then(|| home.join(APP_FOLDER).join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sakura-prefs-{}-{name}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        dir.join(FILE_NAME)
    }

    #[test]
    fn missing_file_gives_the_default_share() {
        let prefs = Preferences::load_from(&scratch("missing"));
        assert_eq!(prefs.changes_share, theme::CHANGES_SHARE_DEFAULT);
    }

    #[test]
    fn a_window_place_that_is_not_four_sound_numbers_is_none() {
        assert_eq!(window_place(&json!([1, 2, 3])), None);
        assert_eq!(window_place(&json!([1, 2, 0, 4])), None, "no width");
        assert_eq!(window_place(&json!(["a", 2, 3, 4])), None);
        assert_eq!(names(&json!({"m1": "files", "m2": 3})).len(), 1);
    }

    #[test]
    fn saved_share_comes_back() {
        let path = scratch("round-trip");
        let saved = Preferences {
            changes_share: 0.5,
            last_task: Some("m2".into()),
            is_sidebar_hidden: true,
            is_changes_hidden: false,
            tabs: Some(json!({ "version": 1, "tabs": [], "active": null })),
            language: LanguageChoice::Is(crate::l10n::Language::Italian),
            last_project: Some("demo-site".into()),
            box_account: Some("lavoro".into()),
            is_needs_person_muted: true,
            is_finished_muted: false,
            window: Some(WindowPlace {
                x: 40.0,
                y: 60.0,
                width: 1200.0,
                height: 800.0,
            }),
            pane_views: [("m2".to_string(), "checks".to_string())].into(),
            terminal_font: Some("Menlo".into()),
            terminal_font_size: Some(15),
            ..Preferences::default()
        };
        saved.save_to(&path);
        assert_eq!(Preferences::load_from(&path), saved);
    }

    #[test]
    fn what_settings_keeps_comes_back_and_a_file_without_it_means_the_defaults() {
        let path = scratch("settings");
        let requests = [("demo-site".to_string(), "Open a pull request".to_string())];
        let saved = Preferences {
            appearance: Some(Appearance::Light),
            publish_request: Some("Push and open a draft".into()),
            publish_requests: requests.into_iter().collect(),
            is_needs_person_muted: false,
            is_finished_muted: true,
            ..Preferences::default()
        };
        saved.save_to(&path);
        assert_eq!(Preferences::load_from(&path), saved);
        std::fs::write(&path, r#"{"changesShare":0.5,"publishRequests":{"a":3}}"#).unwrap();
        let older = Preferences::load_from(&path);
        assert_eq!(older.appearance, None, "the Mac's own appearance");
        assert!(
            !older.is_needs_person_muted && !older.is_finished_muted,
            "everything is told"
        );
        assert!(
            older.publish_requests.is_empty(),
            "a request that is not text is left out"
        );
    }

    #[test]
    fn the_language_is_automatic_until_one_is_chosen_and_the_choice_comes_back() {
        let path = scratch("language");
        assert_eq!(Preferences::default().language, LanguageChoice::Automatic);
        Preferences::default().save_to(&path);
        assert_eq!(
            Preferences::load_from(&path).language,
            LanguageChoice::Automatic,
            "nothing written means the Mac's own language"
        );
        let english = LanguageChoice::Is(crate::l10n::Language::English);
        let chosen = Preferences {
            language: english,
            ..Preferences::default()
        };
        chosen.save_to(&path);
        assert_eq!(Preferences::load_from(&path).language, english);
    }

    #[test]
    fn a_file_from_before_the_last_task_was_kept_has_none() {
        let path = scratch("older");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"changesShare":0.5}"#).unwrap();
        let prefs = Preferences::load_from(&path);
        assert_eq!(prefs.changes_share, 0.5);
        assert_eq!(prefs.last_task, None);
        assert!(!prefs.is_sidebar_hidden && !prefs.is_changes_hidden);
    }

    #[gpui::test]
    fn a_change_is_saved_and_the_file_holds_the_latest_of_several(cx: &mut gpui::TestAppContext) {
        cx.executor().allow_parking();
        let path = scratch("changes");
        cx.update(|cx| Preferences::default().install(Some(path.clone()), cx));
        for name in ["m1", "m2", "m3"] {
            cx.update(|cx| Preferences::change(cx, |prefs| prefs.last_task = Some(name.into())));
        }
        cx.run_until_parked();
        assert_eq!(
            Preferences::load_from(&path).last_task.as_deref(),
            Some("m3")
        );
        assert_eq!(
            cx.update(|cx| Preferences::current(cx))
                .last_task
                .as_deref(),
            Some("m3")
        );
    }

    #[gpui::test]
    fn a_change_that_changes_nothing_writes_nothing(cx: &mut gpui::TestAppContext) {
        let path = scratch("unchanged");
        cx.update(|cx| Preferences::default().install(Some(path.clone()), cx));
        cx.update(|cx| Preferences::change(cx, |prefs| prefs.last_task = None));
        cx.run_until_parked();
        assert!(!path.exists());
    }

    #[test]
    fn share_outside_the_limits_is_clamped() {
        assert_eq!(clamp_share(0.9), theme::CHANGES_SHARE_MAX);
        assert_eq!(clamp_share(0.1), theme::CHANGES_SHARE_MIN);
        assert_eq!(clamp_share(f32::NAN), theme::CHANGES_SHARE_DEFAULT);
    }

    #[test]
    fn unreadable_file_gives_the_defaults() {
        let path = scratch("garbage");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Preferences::load_from(&path), Preferences::default());
    }
}
