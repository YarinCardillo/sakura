//! Window preferences, the only state the app stores: the share of the Agente view the changes pane takes,
//! the task shown last and the open tabs. One small JSON file in Sakura's data folder, read before the
//! window is built and written on the background executor, one write after the other, never on the UI
//! thread. A missing or unreadable file uses default values. Only a missing file offers initial setup.

use crate::data::latest_write::Completion;
use crate::data::latest_write::LatestWrite;
use crate::l10n::LanguageChoice;
use crate::theme::{self, Appearance};
use crate::theme_brand::Accent;
use gpui::{App, Global};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "preferences.json";
/// The app's own folder inside the data folder, beside what `box` keeps there.
const APP_FOLDER: &str = "app";
const SHARE_KEY: &str = "changesShare";
const LAST_TASK_KEY: &str = "lastTask";
const SIDEBAR_HIDDEN_KEY: &str = "sidebarHidden";
const CHANGES_HIDDEN_KEY: &str = "changesHidden";
const TABS_KEY: &str = "tabs";
const DRAFTS_KEY: &str = "drafts";
const DECIDED_WAITS_KEY: &str = "decidedWaits";
const LANGUAGE_KEY: &str = "language";
const LAST_PROJECT_KEY: &str = "lastProject";
const BOX_ACCOUNT_KEY: &str = "boxAccount";
const APPEARANCE_KEY: &str = "appearance";
const ACCENT_KEY: &str = "accent";
const RETINO_ANIMATED_KEY: &str = "retinoAnimated";
const PUBLISH_REQUEST_KEY: &str = "publishRequest";
const PUBLISH_REQUESTS_KEY: &str = "publishRequests";
const NEEDS_PERSON_MUTED_KEY: &str = "muteNeedsPerson";
const FINISHED_MUTED_KEY: &str = "muteFinished";
const WINDOW_KEY: &str = "window";
const PANE_VIEWS_KEY: &str = "paneViews";
const PLACES_KEY: &str = "places";
const MASTER_AGENTS_KEY: &str = "masterAgents";
const MASTER_MODEL_KEY: &str = "masterModel";
const MASTER_ACCOUNT_KEY: &str = "masterAccount";
const MASTER_ACCOUNT_NAMES_KEY: &str = "masterAccountNames";
const NEW_MASTER_ACCOUNT_KEY: &str = "newMasterAccount";
const NEW_MASTER_MODEL_KEY: &str = "newMasterModel";
const NEW_MASTER_EFFORT_KEY: &str = "newMasterEffort";
const MASTER_MODELS_KEY: &str = "masterModels";
const MASTER_PICKS_KEY: &str = "masterPicks";
const TERMINAL_FONT_KEY: &str = "terminalFont";
const TERMINAL_FONT_SIZE_KEY: &str = "terminalFontSize";
const AUTOMATIC_UPDATES_KEY: &str = "updatesAutomatically";
const ENGINE_BIN_KEY: &str = "engineBin";
const OPEN_LANE_GROUPS_KEY: &str = "openLaneGroups";
const COLLAPSED_PROJECTS_KEY: &str = "collapsedProjects";
const MASTER_RESTORED_KEY: &str = "masterRestored";
const SETUP_DONE_KEY: &str = "setupDone";
const PENDING_SETUP_KEY: &str = "pendingSetup";

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
    /// The open tabs, as the tab model writes them (`sakura_core::tabs::Tabs::to_json` in the engine's window); none before the
    /// first tab was opened.
    pub tabs: Option<Value>,
    /// What was typed in the composers and not sent, as `sakura_core::drafts::Drafts::to_json` writes it.
    pub drafts: Option<Value>,
    pub(crate) decided_waits: Option<Value>,
    /// The language chosen: automatic (the Mac's own) unless one was chosen.
    pub language: LanguageChoice,
    /// The project the last task was made in: the one "New task" offers first.
    pub last_project: Option<String>,
    /// The account of the boxes chosen last in "New task": the one the agents in a box start with while
    /// it is signed in.
    pub box_account: Option<String>,
    /// The appearance chosen in Settings; none follows the Mac's own.
    pub appearance: Option<Appearance>,
    /// The accent chosen in Settings: Sakura Pink unless another was chosen.
    pub accent: Accent,
    pub is_retino_animated: bool,
    /// What "Publish" asks the agent when the task's project has no request of its own; none is
    /// Sakura's own request.
    pub publish_request: Option<String>,
    /// The publishing request of each project that has one of its own, by the project's name.
    pub publish_requests: BTreeMap<String, String>,
    /// The news the person does not want the system to tell: a task that needs them, an agent that
    /// finished. Both are told unless muted.
    pub is_needs_person_muted: bool,
    pub is_finished_muted: bool,
    /// Where the window was when it was last moved or sized; none before that, and the window then
    /// opens centred at its first size.
    pub window: Option<WindowPlace>,
    /// The view of the right pane each open task showed, by the view's name.
    pub pane_views: BTreeMap<String, String>,
    /// The place the person chose last for a task of each project ("mac" or "box"), by the project's
    /// name: it wins over the place "New task" would choose by itself.
    pub places: BTreeMap<String, String>,
    /// The agent the master was last switched to for each project ("claude" or "codex"), by the project's
    /// name; the empty name stands for "Any project".
    pub master_agents: BTreeMap<String, String>,
    /// The model the engine's spec ran a Claude master on at the last start ("opus"): the agent chip of an empty
    /// conversation names it until the next start says it again.
    pub master_model: Option<String>,
    /// The Leeway account the master's conversation runs on; empty for the host's own.
    pub master_account: String,
    /// The name the engine last gave each Master account, by the account's id: the chip names the conversation's
    /// account with it while the engine's accounts are still being read.
    pub master_account_names: BTreeMap<String, String>,
    /// What a new Master conversation starts with (Settings, "New Master conversations"): the account, the
    /// model and the effort. None leaves it to the app: the first Claude account, else the first account, with the
    /// program's own model and effort.
    pub new_master_account: Option<String>,
    pub new_master_model: Option<String>,
    pub new_master_effort: Option<String>,
    /// The last list of models each agent gave ("claude", "codex"), as the menu of an empty conversation shows
    /// it until the agent of a new session says it again; the shape is the menu's own (see `master_agent`).
    pub master_models: BTreeMap<String, Value>,
    /// What each master conversation runs on, by the agent's session: its model, effort and mode as the agent
    /// last said them, so a reopened conversation resumes on them (the shape is `master_picks`'s own).
    pub master_picks: BTreeMap<String, Value>,
    /// The family and the size chosen for the terminal in Settings; none of either is Sakura's own.
    pub terminal_font: Option<String>,
    pub terminal_font_size: Option<u8>,
    /// Whether platform update checks and, on macOS, downloads happen automatically.
    pub is_automatic_updates_enabled: bool,
    /// Where the engine's `sakura` command is, when the person set it once: `SAKURA_BIN` comes before it.
    pub engine_bin: Option<String>,
    /// The folders of the projects the sidebar shows closed, their lanes hidden.
    pub collapsed_projects: BTreeSet<String>,
    pub open_lane_groups: BTreeSet<String>,
    /// Whether the master's conversation comes back after a restart of the app or the Mac.
    pub is_master_restored: bool,
    pub is_setup_done: bool,
    pub pending_setup: Option<Value>,
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
            drafts: None,
            decided_waits: None,
            language: LanguageChoice::Automatic,
            last_project: None,
            box_account: None,
            appearance: None,
            accent: Accent::default(),
            is_retino_animated: true,
            publish_request: None,
            publish_requests: BTreeMap::new(),
            is_needs_person_muted: false,
            is_finished_muted: false,
            window: None,
            pane_views: BTreeMap::new(),
            places: BTreeMap::new(),
            master_agents: BTreeMap::new(),
            master_model: None,
            master_account: String::new(),
            master_account_names: BTreeMap::new(),
            new_master_account: None,
            new_master_model: None,
            new_master_effort: None,
            master_models: BTreeMap::new(),
            master_picks: BTreeMap::new(),
            terminal_font: None,
            terminal_font_size: None,
            is_automatic_updates_enabled: true,
            engine_bin: None,
            collapsed_projects: BTreeSet::new(),
            open_lane_groups: BTreeSet::new(),
            is_master_restored: true,
            is_setup_done: false,
            pending_setup: None,
        }
    }
}

/// The preferences of the running window, and the file they are saved to (none in a test window).
struct Stored {
    preferences: Preferences,
    path: Option<PathBuf>,
    saving: LatestWrite<(PathBuf, Preferences)>,
}

impl Global for Stored {}

impl Preferences {
    /// The preferences in the default file, or the defaults.
    pub fn load() -> Self {
        default_path().map_or_else(Self::default, |path| Self::load_from(&path))
    }

    /// Makes these the window's preferences, saved to `path` on every change; `None` saves nowhere.
    pub fn install(self, path: Option<PathBuf>, cx: &mut App) {
        let saving = cx
            .try_global::<Stored>()
            .filter(|stored| stored.path == path)
            .map(|stored| stored.saving.clone())
            .unwrap_or_default();
        cx.set_global(Stored {
            preferences: self,
            path,
            saving,
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

    #[cfg(not(feature = "boxes"))]
    pub(crate) fn decided_waits(cx: &App) -> Option<&Value> {
        cx.try_global::<Stored>()?
            .preferences
            .decided_waits
            .as_ref()
    }

    #[cfg(not(feature = "boxes"))]
    pub(crate) fn keep_decided_waits(cx: &mut App, waits: Value) {
        if cx.try_global::<Stored>().is_some() {
            Self::change(cx, |prefs| prefs.decided_waits = Some(waits));
        }
    }

    /// Whether decorative pixels should move, including before preferences are installed.
    #[cfg(not(feature = "boxes"))]
    pub fn is_retino_animated(cx: &App) -> bool {
        cx.try_global::<Stored>()
            .is_none_or(|stored| stored.preferences.is_retino_animated)
    }

    /// Subscribes a view to changes in the installed preferences.
    #[cfg(not(feature = "boxes"))]
    pub fn observe<V: 'static>(
        cx: &mut gpui::Context<V>,
        on_change: impl FnMut(&mut V, &mut gpui::Context<V>) + 'static,
    ) -> gpui::Subscription {
        cx.observe_global::<Stored>(on_change)
    }

    /// Coalesces pending saves so a burst of edits writes its latest state.
    pub fn change(cx: &mut App, change: impl FnOnce(&mut Preferences)) {
        drop(Self::queue_save(cx, change, false));
    }

    #[cfg(not(feature = "boxes"))]
    pub fn change_and_save<F: FnOnce(&mut Preferences)>(
        cx: &mut App,
        change: F,
    ) -> impl std::future::Future<Output = bool> + use<F> {
        let saved = Self::queue_save(cx, change, true);
        async move {
            match saved {
                Some(mut saved) => saved
                    .wait_for(Option::is_some)
                    .await
                    .ok()
                    .and_then(|completed| *completed)
                    .unwrap_or(false),
                None => true,
            }
        }
    }

    /// Queues a final snapshot behind an outstanding write so Quit can wait for it.
    #[cfg(not(feature = "boxes"))]
    pub fn save_before_quit(cx: &mut App) -> Option<impl std::future::Future<Output = bool>> {
        if !cx.try_global::<Stored>()?.saving.is_writing() {
            return None;
        }
        Some(Self::change_and_save(cx, |_| {}))
    }

    fn queue_save(
        cx: &mut App,
        change: impl FnOnce(&mut Preferences),
        must_save: bool,
    ) -> Option<Completion> {
        let stored = cx.global_mut::<Stored>();
        let before = stored.preferences.clone();
        change(&mut stored.preferences);
        if !must_save && stored.preferences == before {
            return None;
        }
        let path = stored.path.clone()?;
        let (preferences, saving) = (stored.preferences.clone(), stored.saving.clone());
        let write = |(path, preferences): (PathBuf, Preferences)| preferences.save_to(&path);
        let executor = cx.background_executor();
        #[cfg(not(feature = "boxes"))]
        if must_save {
            return Some(saving.submit_with_receipt((path, preferences), write, executor));
        }
        saving.submit((path, preferences), write, executor);
        None
    }

    /// Loads preferences and records whether a preferences file already exists.
    pub fn load_from(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            result => result.ok(),
        };
        text.and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map_or_else(
                || Self {
                    is_setup_done: true,
                    ..Self::default()
                },
                |value| Self::from_json(&value),
            )
    }

    /// Writes the preferences to `path`, creating its folder.
    pub fn save_to(&self, path: &Path) -> bool {
        let text = json!({
            SHARE_KEY: self.changes_share,
            LAST_TASK_KEY: self.last_task,
            SIDEBAR_HIDDEN_KEY: self.is_sidebar_hidden,
            CHANGES_HIDDEN_KEY: self.is_changes_hidden,
            TABS_KEY: self.tabs,
            DRAFTS_KEY: self.drafts,
            DECIDED_WAITS_KEY: self.decided_waits,
            LANGUAGE_KEY: self.language.code(),
            LAST_PROJECT_KEY: self.last_project,
            BOX_ACCOUNT_KEY: self.box_account,
            APPEARANCE_KEY: self.appearance.map(appearance_code),
            ACCENT_KEY: self.accent.id(),
            RETINO_ANIMATED_KEY: self.is_retino_animated,
            PUBLISH_REQUEST_KEY: self.publish_request,
            PUBLISH_REQUESTS_KEY: self.publish_requests,
            NEEDS_PERSON_MUTED_KEY: self.is_needs_person_muted,
            FINISHED_MUTED_KEY: self.is_finished_muted,
            WINDOW_KEY: self.window.map(|at| [at.x, at.y, at.width, at.height]),
            PANE_VIEWS_KEY: self.pane_views,
            PLACES_KEY: self.places,
            MASTER_AGENTS_KEY: self.master_agents,
            MASTER_MODEL_KEY: self.master_model,
            MASTER_ACCOUNT_KEY: self.master_account,
            MASTER_ACCOUNT_NAMES_KEY: self.master_account_names,
            NEW_MASTER_ACCOUNT_KEY: self.new_master_account,
            NEW_MASTER_MODEL_KEY: self.new_master_model,
            NEW_MASTER_EFFORT_KEY: self.new_master_effort,
            MASTER_MODELS_KEY: self.master_models,
            MASTER_PICKS_KEY: self.master_picks,
            TERMINAL_FONT_KEY: self.terminal_font,
            TERMINAL_FONT_SIZE_KEY: self.terminal_font_size,
            AUTOMATIC_UPDATES_KEY: self.is_automatic_updates_enabled,
            ENGINE_BIN_KEY: self.engine_bin,
            COLLAPSED_PROJECTS_KEY: self.collapsed_projects,
            OPEN_LANE_GROUPS_KEY: self.open_lane_groups,
            MASTER_RESTORED_KEY: self.is_master_restored,
            SETUP_DONE_KEY: self.is_setup_done,
            PENDING_SETUP_KEY: self.pending_setup,
        })
        .to_string();
        if let Err(error) = write_preferences(path, &text) {
            eprintln!(
                "sakura: cannot save the preferences to {}: {error}",
                path.display()
            );
            return false;
        }
        true
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
            drafts: value
                .get(DRAFTS_KEY)
                .filter(|drafts| !drafts.is_null())
                .cloned(),
            decided_waits: value
                .get(DECIDED_WAITS_KEY)
                .filter(|waits| !waits.is_null())
                .cloned(),
            language: LanguageChoice::from_code(value.get(LANGUAGE_KEY).and_then(Value::as_str)),
            last_project: text(LAST_PROJECT_KEY),
            box_account: text(BOX_ACCOUNT_KEY),
            appearance: Appearance::forced(text(APPEARANCE_KEY).as_deref()),
            accent: Accent::from_id(text(ACCENT_KEY).as_deref()),
            is_retino_animated: value
                .get(RETINO_ANIMATED_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(true),
            publish_request: text(PUBLISH_REQUEST_KEY),
            publish_requests: requests(value.get(PUBLISH_REQUESTS_KEY)),
            is_needs_person_muted: flag(NEEDS_PERSON_MUTED_KEY),
            is_finished_muted: flag(FINISHED_MUTED_KEY),
            window: value.get(WINDOW_KEY).and_then(window_place),
            pane_views: value.get(PANE_VIEWS_KEY).map(names).unwrap_or_default(),
            places: value.get(PLACES_KEY).map(names).unwrap_or_default(),
            master_agents: value.get(MASTER_AGENTS_KEY).map(names).unwrap_or_default(),
            master_model: text(MASTER_MODEL_KEY),
            master_account: text(MASTER_ACCOUNT_KEY).unwrap_or_default(),
            master_account_names: value
                .get(MASTER_ACCOUNT_NAMES_KEY)
                .map(names)
                .unwrap_or_default(),
            new_master_account: text(NEW_MASTER_ACCOUNT_KEY),
            new_master_model: text(NEW_MASTER_MODEL_KEY),
            new_master_effort: text(NEW_MASTER_EFFORT_KEY),
            master_models: value
                .get(MASTER_MODELS_KEY)
                .and_then(Value::as_object)
                .map(|kept| {
                    kept.iter()
                        .map(|(agent, list)| (agent.clone(), list.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            master_picks: value
                .get(MASTER_PICKS_KEY)
                .and_then(Value::as_object)
                .map(|kept| {
                    kept.iter()
                        .map(|(at, pick)| (at.clone(), pick.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            terminal_font: text(TERMINAL_FONT_KEY),
            terminal_font_size: value
                .get(TERMINAL_FONT_SIZE_KEY)
                .and_then(Value::as_u64)
                .and_then(|size| u8::try_from(size).ok()),
            is_automatic_updates_enabled: value
                .get(AUTOMATIC_UPDATES_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(true),
            engine_bin: text(ENGINE_BIN_KEY),
            collapsed_projects: folders(value.get(COLLAPSED_PROJECTS_KEY)),
            open_lane_groups: folders(value.get(OPEN_LANE_GROUPS_KEY)),
            is_master_restored: value
                .get(MASTER_RESTORED_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(true),
            is_setup_done: value
                .get(SETUP_DONE_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(true),
            pending_setup: value
                .get(PENDING_SETUP_KEY)
                .filter(|v| !v.is_null())
                .cloned(),
        }
    }
}

fn write_preferences(path: &Path, text: &str) -> std::io::Result<()> {
    let folder = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(folder)?;
    let mut file = tempfile::NamedTempFile::new_in(folder)?;
    file.write_all(text.as_bytes())?;
    file.persist(path)?;
    Ok(())
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

/// The project folders in a JSON list of text; anything else is left out.
fn folders(value: Option<&Value>) -> BTreeSet<String> {
    let listed = value.and_then(Value::as_array).into_iter().flatten();
    listed
        .filter_map(|folder| Some(folder.as_str()?.to_string()))
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

/// The preferences file of this user, when the home folder is known. A scripted run has a private data folder of
/// its own (`SAKURA_SNAPSHOT_DATA`) and keeps its preferences there, so a relaunch of it finds what the run left,
/// and the person's own file is never read or written.
pub fn default_path() -> Option<PathBuf> {
    #[cfg(feature = "snapshot")]
    let private = crate::snapshot::data_folder();
    #[cfg(not(feature = "snapshot"))]
    let private = None;
    let home = sakura_core::platform::data_home();
    path_in(
        private,
        (!home.as_os_str().is_empty()).then(|| home.into_os_string()),
        crate::window::can_activate(),
    )
}

/// Where the preferences are kept: in the private data folder of a scripted run when it has one, else under
/// the data home `home` unless this launch cannot be activated (a snapshot), which keeps none.
fn path_in(
    private: Option<PathBuf>,
    home: Option<std::ffi::OsString>,
    can_activate: bool,
) -> Option<PathBuf> {
    if let Some(folder) = private {
        return Some(folder.join(FILE_NAME));
    }
    if !can_activate {
        return None;
    }
    Some(PathBuf::from(home?).join(APP_FOLDER).join(FILE_NAME))
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
    fn a_scripted_run_keeps_its_preferences_in_its_private_data_folder() {
        let kept = path_in(
            Some("/run/data".into()),
            Some("/home/someone".into()),
            false,
        );
        assert_eq!(kept, Some(PathBuf::from("/run/data").join(FILE_NAME)));
        let kept = path_in(Some("/run/data".into()), Some("/home/someone".into()), true);
        assert_eq!(kept, Some(PathBuf::from("/run/data").join(FILE_NAME)));
    }

    #[test]
    fn a_launch_that_cannot_be_activated_keeps_no_preferences_of_the_persons() {
        assert_eq!(path_in(None, Some("/home/someone".into()), false), None);
    }

    #[test]
    fn an_ordinary_launch_keeps_them_in_the_persons_application_folder() {
        let kept = path_in(None, Some("/home/someone".into()), true);
        assert_eq!(
            kept,
            Some(
                PathBuf::from("/home/someone")
                    .join(APP_FOLDER)
                    .join(FILE_NAME)
            )
        );
        assert_eq!(path_in(None, None, true), None);
    }

    #[test]
    fn animated_pixels_are_on_by_default() {
        let path = scratch("retino-default");
        assert!(Preferences::default().save_to(&path));
        let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["retinoAnimated"], true);
    }

    #[test]
    fn animated_pixels_stay_off_after_a_round_trip() {
        let path = scratch("retino-off");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"retinoAnimated":false}"#).unwrap();
        assert!(Preferences::load_from(&path).save_to(&path));
        let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["retinoAnimated"], false);
    }

    #[test]
    fn an_old_preferences_file_keeps_animated_pixels_on() {
        let path = scratch("retino-older");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"setupDone":true,"sidebarHidden":true}"#).unwrap();
        let preferences = Preferences::load_from(&path);
        assert!(preferences.is_sidebar_hidden);
        assert!(preferences.save_to(&path));
        let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["retinoAnimated"], true);
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
            drafts: Some(json!({ "version": 1, "drafts": [] })),
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
            places: [("demo-site".to_string(), "box".to_string())].into(),
            master_agents: [
                (String::new(), "claude".into()),
                ("demo-site".into(), "codex".into()),
            ]
            .into(),
            master_model: Some("opus".into()),
            terminal_font: Some("Menlo".into()),
            terminal_font_size: Some(15),
            is_setup_done: true,
            ..Preferences::default()
        };
        saved.save_to(&path);
        assert_eq!(Preferences::load_from(&path), saved);
    }

    #[test]
    fn the_names_of_the_master_accounts_come_back_and_are_none_in_an_older_file() {
        let path = scratch("account-names");
        let saved = Preferences {
            master_account_names: [("claude-default".to_string(), "Personal".to_string())].into(),
            is_setup_done: true,
            ..Preferences::default()
        };
        saved.save_to(&path);
        assert_eq!(Preferences::load_from(&path), saved);
        std::fs::write(&path, r#"{"masterAccount":"a"}"#).unwrap();
        assert!(Preferences::load_from(&path)
            .master_account_names
            .is_empty());
    }

    #[test]
    fn what_a_new_master_conversation_starts_with_comes_back_and_is_none_in_an_older_file() {
        let path = scratch("new-master");
        let saved = Preferences {
            new_master_account: Some("work".into()),
            new_master_model: Some("sonnet".into()),
            new_master_effort: Some("low".into()),
            is_setup_done: true,
            ..Preferences::default()
        };
        saved.save_to(&path);
        assert_eq!(Preferences::load_from(&path), saved);
        std::fs::write(&path, r#"{"masterAccount":"a"}"#).unwrap();
        let older = Preferences::load_from(&path);
        assert_eq!(older.new_master_account, None);
        assert_eq!(
            (older.new_master_model, older.new_master_effort),
            (None, None)
        );
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
            is_setup_done: true,
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
    fn the_projects_the_sidebar_shows_closed_come_back_and_a_file_without_them_has_none() {
        let path = scratch("collapsed");
        let saved = Preferences {
            collapsed_projects: ["/work/repo".to_string()].into(),
            is_setup_done: true,
            ..Preferences::default()
        };
        saved.save_to(&path);
        assert_eq!(Preferences::load_from(&path), saved);
        std::fs::write(&path, r#"{"collapsedProjects":["/a",3]}"#).unwrap();
        let kept = Preferences::load_from(&path).collapsed_projects;
        assert_eq!(
            kept,
            ["/a".to_string()].into(),
            "a folder that is not text is left out"
        );
        std::fs::write(&path, r#"{"changesShare":0.5}"#).unwrap();
        assert!(Preferences::load_from(&path).collapsed_projects.is_empty());
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
    fn unreadable_file_gives_the_defaults_and_skips_setup() {
        let path = scratch("garbage");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(
            Preferences::load_from(&path),
            Preferences {
                is_setup_done: true,
                ..Preferences::default()
            }
        );
    }

    #[test]
    fn automatic_update_checks_default_to_on_and_the_preference_is_stored() {
        let path = scratch("updates");
        Preferences::default().save_to(&path);
        let json: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["updatesAutomatically"], true);
        std::fs::write(&path, r#"{"updatesAutomatically":false}"#).unwrap();
        let chosen = Preferences::load_from(&path);
        chosen.save_to(&path);
        let json: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["updatesAutomatically"], false);
    }
}
