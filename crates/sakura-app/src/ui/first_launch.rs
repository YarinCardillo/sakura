mod view;

use super::account_add::{self, AddAccountEvent};
use super::dialog_guard::DialogToken;
use super::master_agent::MasterAgent;
use crate::data::{engine_facts::EngineFacts, source::Source};
use crate::l10n::{tr, Key};
use crate::prefs::Preferences;
use gpui::{
    App, AppContext as _, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle,
    ParentElement as _, SharedString, Task, Window,
};
use gpui_component::input::InputState;
use gpui_component::WindowExt as _;
use sakura_core::engine::accounts::{ProviderConfig, Readiness};
use sakura_core::engine::cli::ActResult;
use sakura_core::engine::setup::{
    DetectedHarness, DetectedLogin, SetupAccount, SetupApplied, SetupChoices, SetupDetection,
    SetupMaster,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const STANDARD: &str = "standard";
const NORMAL: &str = "normal";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    FirstLaunch,
    LookAgain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Ask,
    Searching,
    SearchFailed,
    Review,
    Applying,
    ApplyFailed(Key),
}

pub struct Setup {
    source: Source,
    facts: Entity<EngineFacts>,
    focus: FocusHandle,
    mode: Mode,
    stage: Stage,
    programs: Vec<DetectedHarness>,
    logins: Vec<Login>,
    chosen_master: Option<usize>,
    preset: Option<String>,
    pace: Option<String>,
    pending: Option<Operation>,
    review_scroll: gpui::ScrollHandle,
    _work: Option<Task<()>>,
    _dialog: DialogToken,
    _facts_changed: gpui::Subscription,
}

struct Login {
    detected: DetectedLogin,
    agent: MasterAgent,
    name: Option<Entity<InputState>>,
    provider: Option<Entity<InputState>>,
    is_checked: bool,
}

#[derive(Clone)]
struct Operation {
    op: String,
    choices: SetupChoices,
    master_folder: Option<String>,
    existing_master_name: Option<String>,
}

#[derive(PartialEq, Eq)]
pub enum Finished {
    Applied,
    Dismissed,
}

impl EventEmitter<Finished> for Setup {}

pub fn offer(
    source: Source,
    facts: Entity<EngineFacts>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Entity<Setup>> {
    let preferences = Preferences::current(cx);
    if preferences.is_setup_done
        && preferences
            .pending_setup
            .and_then(Operation::from_json)
            .is_none()
    {
        return None;
    }
    Some(open(source, facts, Mode::FirstLaunch, window, cx))
}

pub fn open(
    source: Source,
    facts: Entity<EngineFacts>,
    mode: Mode,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Setup> {
    let setup = cx.new(|cx| Setup::new(source, facts, mode, cx));
    let shown = setup.clone();
    window.open_dialog(cx, move |dialog, window, _| {
        let closing = shown.downgrade();
        super::dialog_frame::placed(dialog, super::dialog_frame::Width::Medium, window)
            .overlay_closable(false)
            .on_ok(|_, _, _| false)
            .on_close(move |_, _, cx| {
                drop(closing.update(cx, Setup::dismiss));
            })
            .child(shown.clone())
    });
    setup.update(cx, |setup, cx| setup.start(window, cx));
    setup
}

impl Setup {
    fn new(source: Source, facts: Entity<EngineFacts>, mode: Mode, cx: &mut Context<Self>) -> Self {
        let preferences = Preferences::current(cx);
        Self {
            source,
            facts: facts.clone(),
            focus: cx.focus_handle(),
            mode,
            stage: Stage::Ask,
            programs: Vec::new(),
            logins: Vec::new(),
            chosen_master: None,
            preset: (mode == Mode::FirstLaunch).then(|| STANDARD.into()),
            pace: (mode == Mode::FirstLaunch).then(|| NORMAL.into()),
            pending: preferences.pending_setup.and_then(Operation::from_json),
            review_scroll: gpui::ScrollHandle::new(),
            _work: None,
            _dialog: DialogToken::take(cx),
            _facts_changed: cx.observe(&facts, |_, _, cx| cx.notify()),
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            self.apply(window, cx);
        } else if self.mode == Mode::LookAgain {
            self.search(window, cx);
        }
    }

    fn search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        self.stage = Stage::Searching;
        let pending = self.source.setup_detect();
        self._work = Some(cx.spawn_in(window, async move |setup, cx| {
            let found = pending.await;
            drop(setup.update_in(cx, |setup, window, cx| match found {
                Ok(found) => setup.show_results(found, window, cx),
                Err(error) => {
                    eprintln!("sakura: setup detection failed: {error}");
                    setup.stage = Stage::SearchFailed;
                    cx.notify();
                }
            }));
        }));
        cx.notify();
    }

    fn show_results(&mut self, found: SetupDetection, window: &mut Window, cx: &mut Context<Self>) {
        self.programs = found
            .harnesses
            .into_iter()
            .filter(|item| (item.found || item.readiness.is_some()) && agent(&item.id).is_some())
            .collect();
        self.merge_logins(found.logins, window, cx);
        self.stage = Stage::Review;
        cx.notify();
    }

    fn merge_logins(
        &mut self,
        detected: Vec<DetectedLogin>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let chosen = self
            .chosen_master
            .and_then(|index| self.logins.get(index))
            .map(|login| Login::identity(&login.detected));
        let mut previous: HashMap<_, _> = self
            .logins
            .drain(..)
            .map(|login| (Login::identity(&login.detected), login))
            .collect();
        self.logins = detected
            .into_iter()
            .filter_map(|detected| {
                let earlier = previous.remove(&Login::identity(&detected));
                Login::refresh(detected, earlier, window, cx)
            })
            .collect();
        self.chosen_master = chosen.and_then(|chosen| {
            self.logins.iter().position(|login| {
                login.can_be_master() && Login::identity(&login.detected) == chosen
            })
        });
    }

    fn default_master(&self) -> Option<usize> {
        let default_claude =
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude"));
        self.logins
            .iter()
            .enumerate()
            .filter(|(_, login)| login.can_be_master())
            .min_by_key(|(_, login)| {
                let is_claude = login.agent == MasterAgent::Claude;
                let is_default_claude = is_claude
                    && default_claude.as_deref() == Some(Path::new(&login.detected.folder));
                (!is_claude, !is_default_claude)
            })
            .map(|(index, _)| index)
    }

    fn master(&self) -> Option<usize> {
        self.chosen_master.or_else(|| self.default_master())
    }

    fn check(&mut self, index: usize, is_checked: bool, cx: &mut Context<Self>) {
        self.logins[index].is_checked = is_checked;
        if self.chosen_master == Some(index) && !self.logins[index].can_be_master() {
            self.chosen_master = None;
        }
        cx.notify();
    }

    fn log_in(
        &self,
        agent: MasterAgent,
        login: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let login = login.map(|index| &self.logins[index].detected);
        let name = login.map(|login| login.in_book_as.as_deref().unwrap_or(&login.proposed_name));
        let existing = login.and_then(|login| {
            let program = self
                .programs
                .iter()
                .find(|program| program.id == login.harness)?;
            let program = program
                .program_path
                .as_ref()
                .filter(|path| path.starts_with('/'))?;
            Some(account_add::DetectedSignIn {
                folder: login.folder.clone(),
                program: program.clone(),
                is_in_book: login.in_book_as.is_some(),
            })
        });
        let sheet = account_add::open_for(
            self.source.clone(),
            self.facts.clone(),
            agent,
            name,
            existing,
            window,
            cx,
        );
        cx.subscribe_in(&sheet, window, |setup, _, event, window, cx| {
            if *event == AddAccountEvent::Added {
                setup.search(window, cx);
            }
        })
        .detach();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stage != Stage::Review {
            return;
        }
        let accounts: Vec<_> = self
            .logins
            .iter()
            .filter(|login| login.is_checked)
            .filter_map(|login| login.choice(cx))
            .collect();
        if accounts.iter().any(|account| account.name.is_empty()) {
            return;
        }
        let master = self.master().map(|at| &self.logins[at].detected);
        self.pending = Some(Operation {
            op: uuid::Uuid::new_v4().to_string(),
            choices: SetupChoices {
                accounts,
                preset: self.preset.clone(),
                pace: self.pace.clone(),
                master_account: master.filter(|login| login.readiness.is_some()).and_then(
                    |login| {
                        let selected = if login.in_book_as.is_some() {
                            SetupMaster::Registered(login.registered_account.clone()?)
                        } else {
                            SetupMaster::Selected {
                                harness: login.harness.clone(),
                                folder: login.folder.clone(),
                            }
                        };
                        Some(Some(selected))
                    },
                ),
            },
            master_folder: master.map(|login| login.folder.clone()),
            existing_master_name: master.and_then(|login| login.in_book_as.clone()),
        });
        self.apply(window, cx);
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(operation) = self.pending.clone() else {
            return;
        };
        self.focus.focus(window, cx);
        self.stage = Stage::Applying;
        let source = self.source.clone();
        let record = operation.to_json();
        let saved =
            Preferences::change_and_save(cx, move |prefs| prefs.pending_setup = Some(record));
        self._work = Some(cx.spawn_in(window, async move |setup, cx| {
            let applied = if saved.await {
                operation.run(&source).await
            } else {
                Err(Key("first_launch.save_failed"))
            };
            match applied {
                Ok(account) => Self::finish(setup, account, cx).await,
                Err(key) => drop(setup.update_in(cx, |setup, _, cx| setup.apply_failed(key, cx))),
            }
        }));
        cx.notify();
    }

    async fn finish(
        setup: gpui::WeakEntity<Self>,
        account: Option<String>,
        cx: &mut AsyncWindowContext,
    ) {
        let Ok(saved) = setup.update_in(cx, |_, _, cx| {
            Preferences::change_and_save(cx, move |prefs| {
                prefs.is_setup_done = true;
                prefs.pending_setup = None;
                if let Some(account) = account {
                    if prefs.new_master_account.as_ref() != Some(&account) {
                        prefs.new_master_model = None;
                        prefs.new_master_effort = None;
                    }
                    prefs.new_master_account = Some(account);
                }
            })
        }) else {
            return;
        };
        let is_saved = saved.await;
        drop(setup.update_in(cx, |setup, window, cx| {
            if !is_saved {
                setup.apply_failed(Key("first_launch.save_failed"), cx);
                return;
            }
            setup.facts.update(cx, |facts, cx| facts.read(cx));
            window.close_dialog(cx);
            cx.emit(Finished::Applied);
        }));
    }

    fn apply_failed(&mut self, key: Key, cx: &mut Context<Self>) {
        self.stage = Stage::ApplyFailed(key);
        cx.notify();
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self._work = None;
        if self.stage == Stage::Applying {
            return;
        }
        Preferences::change(cx, |prefs| {
            prefs.is_setup_done = true;
            prefs.pending_setup = None;
        });
        cx.emit(Finished::Dismissed);
    }
}

impl Login {
    fn identity(detected: &DetectedLogin) -> (String, String) {
        (detected.harness.clone(), detected.folder.clone())
    }

    fn refresh(
        detected: DetectedLogin,
        previous: Option<Self>,
        window: &mut Window,
        cx: &mut Context<Setup>,
    ) -> Option<Self> {
        if let Some(mut previous) = previous.filter(|login| {
            login.name.is_some() && Self::is_recommended(&detected) && detected.in_book_as.is_none()
        }) {
            previous.detected = detected;
            return Some(previous);
        }
        Self::new(detected, window, cx)
    }

    fn new(detected: DetectedLogin, window: &mut Window, cx: &mut Context<Setup>) -> Option<Self> {
        let agent = agent(&detected.harness)?;
        let can_choose = Self::is_recommended(&detected) && detected.in_book_as.is_none();
        let is_checked = can_choose && Self::is_default_folder(agent, &detected.folder);
        let name = can_choose.then(|| {
            let name = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(tr(Key("settings_modal.accounts.name_hint")))
            });
            name.update(cx, |input, cx| {
                input.set_value(detected.proposed_name.clone(), window, cx)
            });
            cx.observe(&name, |_, _, cx| cx.notify()).detach();
            name
        });
        let provider = (can_choose && agent == MasterAgent::Goose).then(|| {
            cx.new(|cx| InputState::new(window, cx).placeholder(tr(Key("goose.provider_hint"))))
        });
        Some(Self {
            detected,
            agent,
            name,
            provider,
            is_checked,
        })
    }

    fn is_default_folder(agent: MasterAgent, folder: &str) -> bool {
        let Some(home) = std::env::var_os("HOME") else {
            return false;
        };
        let relative = match agent {
            MasterAgent::Claude => ".claude",
            MasterAgent::Codex => ".codex",
            MasterAgent::Goose => ".config/goose",
        };
        Path::new(folder) == PathBuf::from(home).join(relative)
    }

    fn can_be_master(&self) -> bool {
        (Self::is_recommended(&self.detected) || self.detected.registered_account.is_some())
            && (self.is_checked || self.detected.in_book_as.is_some())
    }

    fn is_recommended(detected: &DetectedLogin) -> bool {
        match detected.readiness {
            Some(Readiness::Unknown | Readiness::Ready) => true,
            Some(Readiness::NeedsSetup | Readiness::Unavailable) => false,
            None => detected.logged_in,
        }
    }

    fn label(&self, cx: &App) -> SharedString {
        if let Some(name) = &self.detected.in_book_as {
            return name.clone().into();
        }
        self.name.as_ref().map_or_else(
            || self.detected.proposed_name.clone().into(),
            |name| name.read(cx).value(),
        )
    }

    fn choice(&self, cx: &App) -> Option<SetupAccount> {
        Some(SetupAccount {
            harness: self.detected.harness.clone(),
            folder: self.detected.folder.clone(),
            name: self.name.as_ref()?.read(cx).value().trim().to_string(),
            provider_config: self.provider.as_ref().and_then(|field| {
                let provider = field.read(cx).value();
                let provider = provider.trim();
                (!provider.is_empty()).then(|| ProviderConfig {
                    provider: provider.into(),
                })
            }),
        })
    }
}

impl Operation {
    fn to_json(&self) -> Value {
        json!({"op": self.op, "choices": self.choices, "master": self.master_folder, "existing_master_name": self.existing_master_name})
    }

    fn from_json(value: Value) -> Option<Self> {
        Some(Self {
            op: value.get("op")?.as_str()?.to_string(),
            choices: serde_json::from_value(value.get("choices")?.clone()).ok()?,
            master_folder: value
                .get("master")
                .and_then(Value::as_str)
                .map(str::to_string),
            existing_master_name: value
                .get("existing_master_name")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    async fn run(&self, source: &Source) -> Result<Option<String>, Key> {
        let failed = Key("first_launch.apply_failed");
        let result = source
            .act(self.choices.act(&self.op))
            .await
            .map_err(|error| {
                eprintln!("sakura: setup apply failed: {error}");
                failed
            })?;
        let ActResult::Done {
            exit_code: 0,
            output,
            ..
        } = result
        else {
            return Err(failed);
        };
        let applied: SetupApplied = serde_json::from_str(&output).map_err(|_| failed)?;
        if let Some(account) = applied.master_account {
            return Ok(Some(account.id));
        }
        let Some(folder) = self.master_folder.as_deref() else {
            return Ok(None);
        };
        let label = applied
            .accounts
            .iter()
            .find(|account| account.folder == folder)
            .map(|account| account.label.as_str())
            .or(self.existing_master_name.as_deref())
            .ok_or(failed)?;
        source
            .setup_account_id(label.to_string())
            .await
            .map_err(|_| failed)?
            .map(Some)
            .ok_or(failed)
    }
}

fn agent(id: &str) -> Option<MasterAgent> {
    MasterAgent::ALL
        .into_iter()
        .find(|agent| agent.code() == id)
}

#[cfg(test)]
mod detection_tests {
    use super::*;

    #[gpui::test]
    fn review_detection_refresh_keeps_choices_by_folder(cx: &mut gpui::TestAppContext) {
        let (rig, _desk, cx) = crate::ui::engine_desk_tests::open_unread(
            cx,
            r#"{"api":1,"waits":[],"items":[],"projects":[]}"#,
        );
        let fixture = include_str!("../../tests/fixtures/setup-detect.json");
        cx.update(|window, cx| {
            let setup =
                cx.new(|cx| Setup::new(rig.source.clone(), rig.facts.clone(), Mode::LookAgain, cx));
            setup.update(cx, |setup, cx| {
                setup.show_results(serde_json::from_str(fixture).unwrap(), window, cx);
                setup.logins[0]
                    .name
                    .as_ref()
                    .unwrap()
                    .update(cx, |name, cx| {
                        name.set_value("Personal", window, cx);
                    });
                setup.check(0, true, cx);
                setup.check(2, false, cx);
                setup.chosen_master = Some(0);
                let mut refreshed: SetupDetection = serde_json::from_str(fixture).unwrap();
                refreshed.logins.swap(0, 2);
                refreshed.logins[3].logged_in = true;
                setup.show_results(refreshed, window, cx);
                assert_eq!(setup.master(), Some(2));
                assert_eq!(setup.logins[2].label(cx), "Personal");
                assert!(!setup.logins[0].is_checked);
                assert!(!setup.logins[3].is_checked);
            });
        });
    }
}
