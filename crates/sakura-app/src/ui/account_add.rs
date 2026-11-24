//! Adding an account: the program and a name, then either a new login or a login folder they already have. For
//! a new login the engine makes an empty login folder; Claude's login is a command that opens in a terminal for the
//! person to log in (the account is added when they say they did), and Codex's runs in the app over its
//! app-server, which opens the browser and adds the account when the login completes. The engine decides what an
//! account is: this keeps only the conversation of one try, and the app never sees or types a credential.

use crate::data::engine_facts::EngineFacts;
use crate::data::source::Source;
use crate::l10n::{say, tr, tr_with, Key};
use crate::theme::{self, Palette, Theme};
use crate::ui::chrome::text;
use crate::ui::dialog_frame;
use crate::ui::dialog_guard::DialogToken;
use crate::ui::engine_acts::{ADD_ACCOUNT, MAKE_LOGIN_FOLDER};
use crate::ui::kit::{self, ButtonKind};
use crate::ui::master_agent::{self, MasterAgent};
use crate::ui::settings_modal::controls;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task as GpuiTask,
    Window,
};
use gpui_component::input::InputState;
use gpui_component::WindowExt as _;
use sakura_acp::connection::{CodexLogin, LoginEnd};
use sakura_core::engine::cli::{Act, ActResult};
use sakura_core::error::BridgeResult;
use std::rc::Rc;

/// What the panel's button says, and what is said when no name was typed, when the engine did not add the
/// account, when it failed, and when no terminal opened.
const CHOOSE: Key = Key("account_add.choose_folder");
const NEEDS_NAME: Key = Key("account_add.needs_name");
const NOT_ADDED: Key = Key("account_add.not_added");
const FAILED: Key = Key("waiting.failed");
const TERMINAL_FAILED: Key = Key("account_add.terminal_failed");

const TITLE: Key = Key("account_add.title");
const NAME_LABEL: Key = Key("account_add.name_label");
const LOG_IN: Key = Key("account_add.log_in");
const USE_FOLDER: Key = Key("account_add.use_folder");
const WAITING: Key = Key("account_add.waiting");
const ADD: Key = Key("account_add.add");
const MAKE: Key = Key("account_add.make");
const WILL_MAKE: Key = Key("account_add.will_make");
const MAKE_BROWSER: Key = Key("account_add.make_browser");
const WILL_MAKE_BROWSER: Key = Key("account_add.will_make_browser");
const WAITING_BROWSER: Key = Key("account_add.waiting_browser");

/// Runs a command line in a terminal window, for the person to finish a login there.
pub type TerminalOpener = Rc<dyn Fn(&str, &App) -> gpui::Task<std::io::Result<()>>>;

/// Where the person is in adding an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// Choosing the program and the name, and how to log in.
    Choosing,
    /// Waiting for the engine's answer.
    Working,
    /// What will be made, shown before anything is: the engine makes the login folder only once the person
    /// confirms.
    Confirming,
    /// The login folder the engine made, waiting for the person to log in in the terminal and say so.
    Waiting(String),
    /// The Codex login is made in the browser: the account is added in this folder when it completes.
    Browser(String),
}

/// What the sheet tells whoever shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddAccountEvent {
    /// The engine added the account: the sheet is done.
    Added,
    /// The person left without adding one.
    Cancelled,
}

pub(super) struct DetectedSignIn {
    pub folder: String,
    pub program: String,
    pub is_in_book: bool,
}

/// The sheet's state.
pub struct AddAccount {
    source: Source,
    facts: Entity<EngineFacts>,
    open_terminal: TerminalOpener,
    agent: MasterAgent,
    detected: Option<DetectedSignIn>,
    name_field: Entity<InputState>,
    provider_field: Entity<InputState>,
    focus: FocusHandle,
    _restore_focus: Subscription,
    stage: Stage,
    refusal: Option<SharedString>,
    /// The Codex login being made in the browser, while there is one.
    login: Option<CodexLogin>,
    _work: Option<GpuiTask<()>>,
    /// Says to the tab keys that a dialog is open, for as long as the sheet lives.
    _open: DialogToken,
}

impl EventEmitter<AddAccountEvent> for AddAccount {}

impl AddAccount {
    /// A sheet over `source`; `facts` are read again once an account was added, and `open_terminal` runs the
    /// login command.
    pub fn new(
        source: Source,
        facts: Entity<EngineFacts>,
        open_terminal: TerminalOpener,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_field =
            cx.new(|cx| controls::state(Key("settings_modal.accounts.name_hint"), window, cx));
        let provider_field = cx.new(|cx| controls::state(Key("goose.provider_hint"), window, cx));
        let focus = cx.focus_handle();
        let restore_focus = cx.on_focus_lost(window, |sheet, window, cx| {
            if window.focus_lost_restore_target(cx).as_ref() == Some(&sheet.focus) {
                window.focus(&sheet.focus, cx);
            }
        });
        Self {
            source,
            facts,
            open_terminal,
            agent: MasterAgent::Claude,
            detected: None,
            name_field,
            provider_field,
            focus,
            _restore_focus: restore_focus,
            stage: Stage::Choosing,
            refusal: None,
            login: None,
            _work: None,
            _open: DialogToken::take(cx),
        }
    }

    #[cfg(test)]
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// What the engine, or the sheet, said of the last try that did not add the account.
    #[cfg(test)]
    pub fn refusal(&self) -> Option<&SharedString> {
        self.refusal.as_ref()
    }

    pub fn set_agent(&mut self, agent: MasterAgent, cx: &mut Context<Self>) {
        self.agent = agent;
        cx.notify();
    }

    #[cfg(test)]
    pub fn set_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.name_field
            .update(cx, |field, cx| field.set_value(name, window, cx));
    }

    /// Shows what a new login would make; nothing is made until the person confirms.
    pub fn log_in_new(&mut self, cx: &mut Context<Self>) {
        if self.stage != Stage::Choosing || !self.has_name(cx) {
            return;
        }
        self.stage = Stage::Confirming;
        self.refusal = None;
        cx.notify();
    }

    /// The person confirmed: the engine makes the login folder for the program, its command opens in a
    /// terminal, and the sheet waits for the person to log in there.
    pub fn confirm_login(&mut self, cx: &mut Context<Self>) {
        if self.stage != Stage::Confirming {
            return;
        }
        if let Some(detected) = &self.detected {
            let folder = detected.folder.clone();
            match self.agent {
                MasterAgent::Claude => self.start_terminal_login(
                    LoginFolder {
                        command: claude_login_command(&detected.program, &folder),
                        folder,
                    },
                    cx,
                ),
                MasterAgent::Codex => self.log_in_in_the_browser(folder, cx),
                MasterAgent::Goose => match self.source.goose_setup_program() {
                    Ok(program) => self.start_terminal_login(
                        LoginFolder {
                            command: terminal_argv(&[
                                program,
                                "setup".into(),
                                "configure-goose".into(),
                                format!("--folder={folder}"),
                            ]),
                            folder,
                        },
                        cx,
                    ),
                    Err(error) => self.refusal = Some(say(&error.said).to_string().into()),
                },
            }
            cx.notify();
            return;
        }
        let act = MAKE_LOGIN_FOLDER.act(&[("provider", self.agent.code())]);
        self.send(act, Stage::Confirming, Self::login_folder_made, cx);
    }

    /// The person logged in: the folder the engine made is added under the name typed.
    pub fn add_after_login(&mut self, cx: &mut Context<Self>) {
        if let Stage::Waiting(folder) = self.stage.clone() {
            self.add(folder, cx);
        }
    }

    /// Adds the login folder `folder`, which the person already has, under the name typed.
    pub fn add_folder(&mut self, folder: String, cx: &mut Context<Self>) {
        if self.stage == Stage::Choosing {
            self.add(folder, cx);
        }
    }

    /// Opens the Mac's folder panel; the folder chosen is added.
    pub fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stage != Stage::Choosing || !self.has_name(cx) {
            return;
        }
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(tr(CHOOSE).into()),
        });
        self._work = Some(cx.spawn_in(window, async move |sheet, cx| {
            let folder = match chosen.await {
                Ok(Ok(Some(folders))) => folders.into_iter().next(),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(error)) => {
                    eprintln!("sakura: the folder picker could not open: {error}");
                    None
                }
            };
            if let Some(folder) = folder {
                let folder = folder.to_string_lossy().into_owned();
                drop(sheet.update(cx, |sheet, cx| sheet.add_folder(folder, cx)));
            }
        }));
    }

    /// The person left: nothing is added, and a login being made in the browser is cancelled.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self._work = None;
        if let Some(login) = self.login.take() {
            login.cancel();
        }
        cx.emit(AddAccountEvent::Cancelled);
    }

    fn add(&mut self, folder: String, cx: &mut Context<Self>) {
        if self.agent != MasterAgent::Goose
            && self.detected.as_ref().is_some_and(|login| login.is_in_book)
        {
            self.account_added(Ok(String::new()), cx);
            return;
        }
        if !self.has_name(cx) {
            return;
        }
        let label = self.name_field.read(cx).value().trim().to_string();
        let mut inputs = vec![
            ("folder", folder.as_str()),
            ("label", label.as_str()),
            ("provider", self.agent.code()),
        ];
        let choices;
        if self.agent == MasterAgent::Goose {
            let provider = self.provider_field.read(cx).value();
            if provider.trim().is_empty() {
                self.refusal = Some(tr(Key("goose.provider_hint")).into());
                cx.notify();
                return;
            }
            choices = serde_json::json!({"provider": provider.trim()}).to_string();
            inputs.push(("choices", &choices));
        }
        let back = match &self.stage {
            Stage::Waiting(waiting) => Stage::Waiting(waiting.clone()),
            _ => Stage::Choosing,
        };
        let mut act = ADD_ACCOUNT.act(&inputs);
        if self.agent == MasterAgent::Goose {
            act.action.optional.push("choices".into());
        }
        self.send(act, back, Self::account_added, cx);
    }

    /// Whether a name was typed; says so when none was.
    fn has_name(&mut self, cx: &mut Context<Self>) -> bool {
        let has_name = !self.name_field.read(cx).value().trim().is_empty();
        if !has_name {
            self.refusal = Some(tr(NEEDS_NAME).into());
            cx.notify();
        }
        has_name
    }

    /// Sends `act`; `then` gets the engine's output, or the words of why not, and the stage to go back to
    /// when it was not done.
    fn send(
        &mut self,
        act: Act,
        back: Stage,
        then: fn(&mut Self, Result<String, SharedString>, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) {
        self.stage = Stage::Working;
        self.refusal = None;
        let pending = self.source.act(act);
        self._work = Some(cx.spawn(async move |sheet, cx| {
            let result = pending.await;
            drop(sheet.update(cx, |sheet, cx| {
                let outcome = outcome_of(result);
                if outcome.is_err() {
                    sheet.stage = back;
                }
                then(sheet, outcome, cx);
            }));
        }));
        cx.notify();
    }

    fn login_folder_made(&mut self, outcome: Result<String, SharedString>, cx: &mut Context<Self>) {
        match outcome.and_then(|output| made_by_for(&output, self.agent)) {
            Ok(made) if self.agent == MasterAgent::Codex => {
                self.log_in_in_the_browser(made.folder, cx)
            }
            Ok(made) => self.start_terminal_login(made, cx),
            Err(words) => self.refusal = Some(words),
        }
        cx.notify();
    }

    fn start_terminal_login(&mut self, made: LoginFolder, cx: &mut Context<Self>) {
        self.stage = Stage::Waiting(made.folder);
        let opened = (self.open_terminal)(&made.command, cx);
        self._work = Some(cx.spawn(async move |sheet, cx| {
            if let Err(error) = opened.await {
                eprintln!("sakura: the login could not be opened in a terminal: {error}");
                drop(sheet.update(cx, |sheet, cx| {
                    if matches!(sheet.stage, Stage::Waiting(_)) {
                        sheet.refusal =
                            Some(tr_with(TERMINAL_FAILED, &[("command", &made.command)]));
                        cx.notify();
                    }
                }));
            }
        }));
    }

    /// Codex's login runs in the app: the browser opens and the account is added when it completes.
    fn log_in_in_the_browser(&mut self, folder: String, cx: &mut Context<Self>) {
        self.stage = Stage::Browser(folder.clone());
        super::codex_login::run(
            self.source.master_spec(None),
            Some(folder.into()),
            |sheet: &mut Self, login| sheet.login = Some(login),
            Self::browser_login_ended,
            cx,
        )
        .detach();
    }

    /// The Codex login is over: a login in the folder adds the account; any other end is said in words, and the
    /// sheet goes back to the choice (a cancelled one has left already).
    fn browser_login_ended(&mut self, end: &LoginEnd, cx: &mut Context<Self>) {
        self.login = None;
        let Stage::Browser(folder) = self.stage.clone() else {
            return;
        };
        match master_agent::login_end_words(end) {
            None => self.add(folder, cx),
            Some(words) => {
                self.stage = Stage::Choosing;
                self.refusal = Some(words.into());
            }
        }
        cx.notify();
    }

    fn account_added(&mut self, outcome: Result<String, SharedString>, cx: &mut Context<Self>) {
        match outcome {
            Ok(_) => {
                self.facts.update(cx, |facts, cx| facts.read(cx));
                cx.emit(AddAccountEvent::Added);
            }
            Err(words) => self.refusal = Some(words),
        }
        cx.notify();
    }
}

/// What the engine's answer to an action comes to: its output when it ran, else the words of why it did not.
fn outcome_of(result: BridgeResult<ActResult>) -> Result<String, SharedString> {
    match result {
        Ok(ActResult::Done {
            exit_code: 0,
            output,
            ..
        }) => Ok(output),
        Ok(ActResult::Done {
            exit_code, output, ..
        }) => Err(
            super::account_words::reason_words(Some(output.trim())).map_or_else(
                || tr_with(FAILED, &[("code", &exit_code.to_string())]),
                SharedString::from,
            ),
        ),
        Ok(ActResult::Refused { reason, .. }) => Err(reason.into()),
        Ok(ActResult::Confirm { .. }) => Err(tr(NOT_ADDED).into()),
        Err(error) => Err(say(&error.said).to_string().into()),
    }
}

/// The login folder the engine made, and the command that logs in to it.
struct LoginFolder {
    folder: String,
    command: String,
}

fn made_by(output: &str) -> Result<LoginFolder, SharedString> {
    let said: serde_json::Value = serde_json::from_str(output).map_err(|_| tr(NOT_ADDED))?;
    let text = |name: &str| said[name].as_str().filter(|text| !text.is_empty());
    match (text("folder"), text("command")) {
        (Some(folder), Some(command)) => Ok(LoginFolder {
            folder: folder.to_string(),
            command: command.to_string(),
        }),
        _ => Err(tr(NOT_ADDED).into()),
    }
}

fn made_by_for(output: &str, agent: MasterAgent) -> Result<LoginFolder, SharedString> {
    if agent != MasterAgent::Goose {
        return made_by(output);
    }
    let launch: serde_json::Value = serde_json::from_str(output).map_err(|_| tr(NOT_ADDED))?;
    let argv = launch["argv"]
        .as_array()
        .and_then(|args| {
            args.iter()
                .map(|arg| arg.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| tr(NOT_ADDED))?;
    let folder = launch["folder"]
        .as_str()
        .filter(|folder| folder.starts_with('/'))
        .ok_or_else(|| tr(NOT_ADDED))?;
    if launch["provider"] != "goose"
        || !argv.first().is_some_and(|program| program.starts_with('/'))
        || !launch["env"]
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
    {
        return Err(tr(NOT_ADDED).into());
    }
    Ok(LoginFolder {
        folder: folder.into(),
        command: terminal_argv(&argv),
    })
}

fn terminal_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|value| format!("'{}'", value.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Runs `command` in a new Terminal window. The command goes to the script as data, never through a shell of
/// the app's: the person sees it and finishes the login there.
pub fn open_in_terminal(command: &str) -> std::io::Result<()> {
    let escaped = command.replace('\\', "\\\\").replace('"', "\\\"");
    let tell =
        format!("tell application \"Terminal\"\nactivate\ndo script \"{escaped}\"\nend tell");
    std::process::Command::new("osascript")
        .arg("-e")
        .arg(tell)
        .stdin(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

fn claude_login_command(program: &str, folder: &str) -> String {
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    format!(
        "env CLAUDE_CONFIG_DIR={} {} auth login",
        quote(folder),
        quote(program)
    )
}

/// The terminal of the Mac, for the sheet shown to a person.
pub fn terminal() -> TerminalOpener {
    Rc::new(|command, cx| {
        let command = command.to_string();
        cx.background_executor()
            .spawn(async move { open_in_terminal(&command) })
    })
}

/// Opens the sheet over the window, at the place of every dialog; it closes when an account was added or the
/// person left.
pub fn open(source: Source, facts: Entity<EngineFacts>, window: &mut Window, cx: &mut App) {
    open_for(source, facts, MasterAgent::Claude, None, None, window, cx);
}

/// Opens the account sheet with the program and proposed name already filled in.
pub(super) fn open_for(
    source: Source,
    facts: Entity<EngineFacts>,
    agent: MasterAgent,
    name: Option<&str>,
    detected: Option<DetectedSignIn>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<AddAccount> {
    let sheet = cx.new(|cx| {
        let mut sheet = AddAccount::new(source, facts, terminal(), window, cx);
        sheet.agent = agent;
        if detected.is_some() {
            sheet.stage = Stage::Confirming;
            sheet.detected = detected;
        }
        if let Some(name) = name {
            sheet
                .name_field
                .update(cx, |field, cx| field.set_value(name, window, cx));
        }
        sheet
    });
    window
        .subscribe(&sheet, cx, |_, _: &AddAccountEvent, window, cx| {
            window.close_dialog(cx)
        })
        .detach();
    let shown = sheet.clone();
    window.open_dialog(cx, move |dialog, window, _| {
        let cancel = shown.downgrade();
        super::dialog_frame::placed(dialog, super::dialog_frame::Width::Medium, window)
            .on_ok(|_, _, _| false)
            .on_cancel(move |_, _, cx| {
                drop(cancel.update(cx, AddAccount::cancel));
                false
            })
            .child(shown.clone())
    });
    sheet
}

impl Render for AddAccount {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let body = match &self.stage {
            Stage::Waiting(_) => self.waiting(&palette, cx),
            Stage::Browser(_) => self.in_the_browser(&palette, cx),
            Stage::Confirming => self.confirming(&palette, cx),
            _ => self.choosing(&palette, cx),
        };
        dialog_frame::surface("account-add", window, &palette)
            .track_focus(&self.focus)
            .child(dialog_frame::heading(
                "account-add-title",
                tr(TITLE),
                &palette,
            ))
            .child(body)
            .children(self.refusal.clone().map(|words| {
                text(theme::TEXT_SMALL, palette.danger)
                    .debug_selector(|| "account-add-refusal".into())
                    .child(words)
            }))
    }
}

impl AddAccount {
    /// The program, the name, and the two ways to log in.
    fn choosing(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let is_busy = self.stage == Stage::Working;
        let supports_goose = self.agent == MasterAgent::Goose
            || self
                .facts
                .read(cx)
                .snapshot()
                .and_then(|snapshot| snapshot.engine.as_ref())
                .is_some_and(|engine| engine.master_launch_route.is_some());
        let agents: Vec<_> = MasterAgent::ALL
            .into_iter()
            .filter(|agent| *agent != MasterAgent::Goose || supports_goose)
            .collect();
        let options: Vec<(SharedString, bool)> = agents
            .iter()
            .map(|agent| (agent.label().into(), *agent == self.agent))
            .collect();
        let this = cx.entity().downgrade();
        let program = kit::segments("account-add-program", &options, move |at, _, cx| {
            let agent = agents[at];
            drop(this.update(cx, |sheet, cx| sheet.set_agent(agent, cx)));
        });
        let name = controls::labeled_field(
            "account-add-name",
            tr(NAME_LABEL),
            &self.name_field,
            gpui::relative(1.),
            palette,
        );
        let (log_in, use_folder) = (cx.entity().downgrade(), cx.entity().downgrade());
        let button = if is_busy {
            kit::button_disabled
        } else {
            kit::button
        };
        let new_login = button(
            "account-add-log-in",
            tr(if self.agent == MasterAgent::Goose {
                Key("goose.configure")
            } else {
                LOG_IN
            }),
            ButtonKind::Primary,
            palette,
        )
        .when(!is_busy, |button| {
            button
                .on_click(move |_, _, cx| drop(log_in.update(cx, |sheet, cx| sheet.log_in_new(cx))))
        });
        let existing = button(
            "account-add-use-folder",
            tr(if self.agent == MasterAgent::Goose {
                Key("goose.choose_folder")
            } else {
                USE_FOLDER
            }),
            ButtonKind::Secondary,
            palette,
        )
        .when(!is_busy, |button| {
            button.on_click(move |_, window, cx| {
                drop(use_folder.update(cx, |sheet, cx| sheet.choose_folder(window, cx)))
            })
        });
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_WIDE)
            .child(program)
            .child(name)
            .children(self.provider_field(palette))
            .child(
                dialog_frame::footer(palette)
                    .child(self.cancel_button(palette, cx))
                    .child(existing)
                    .child(new_login),
            )
            .into_any_element()
    }

    /// What a new login will make, said before anything is made, with the button that makes it.
    fn confirming(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let is_codex = self.agent == MasterAgent::Codex;
        let (make_words, plan) = match &self.detected {
            _ if self.agent == MasterAgent::Goose => (
                Key("goose.configure"),
                tr(Key("goose.will_configure")).into(),
            ),
            Some(_) => (
                Key("first_launch.log_in"),
                tr_with(
                    Key("first_launch.login_existing"),
                    &[("program", self.agent.label())],
                ),
            ),
            None => {
                let (make, plan) = if is_codex {
                    (MAKE_BROWSER, WILL_MAKE_BROWSER)
                } else {
                    (MAKE, WILL_MAKE)
                };
                (make, tr_with(plan, &[("program", self.agent.label())]))
            }
        };
        let make = kit::button(
            "account-add-open",
            tr(make_words),
            ButtonKind::Primary,
            palette,
        )
        .on_click(move |_, _, cx| drop(this.update(cx, |sheet, cx| sheet.confirm_login(cx))));
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_WIDE)
            .child(
                text(theme::TEXT_SMALL, palette.text)
                    .debug_selector(|| "account-add-confirm-name".into())
                    .child(tr_with(
                        Key("account_add.confirm_name"),
                        &[("account", &self.name_field.read(cx).value())],
                    )),
            )
            .child(
                text(theme::TEXT_SMALL, palette.text_secondary)
                    .debug_selector(|| "account-add-plan".into())
                    .child(plan),
            )
            .children(self.detected.as_ref().map(|detected| {
                div()
                    .w_full()
                    .rounded(theme::RADIUS_CONTROL)
                    .bg(palette.fill)
                    .px(theme::GAP_SMALL)
                    .min_h(theme::CONTROL_HEIGHT)
                    .flex()
                    .items_center()
                    .gap(theme::GAP_SMALL)
                    .child(
                        kit::path_label(
                            "account-add-folder",
                            &detected.folder,
                            (palette.text, palette.text_secondary),
                        )
                        .flex_1(),
                    )
                    .child(kit::reveal_path_icon(
                        "account-add-location",
                        detected.folder.clone(),
                        cx,
                    ))
            }))
            .child(
                dialog_frame::footer(palette)
                    .child(self.cancel_button(palette, cx))
                    .child(make),
            )
            .into_any_element()
    }

    /// The terminal is open: log in there, then say so.
    fn waiting(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let add = kit::button("account-add-add", tr(ADD), ButtonKind::Primary, palette)
            .on_click(move |_, _, cx| drop(this.update(cx, |sheet, cx| sheet.add_after_login(cx))));
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_WIDE)
            .child(text(theme::TEXT_SMALL, palette.text_secondary).child(tr(
                if self.agent == MasterAgent::Goose {
                    Key("goose.waiting")
                } else {
                    WAITING
                },
            )))
            .children(self.provider_field(palette))
            .child(
                dialog_frame::footer(palette)
                    .child(self.cancel_button(palette, cx))
                    .child(add),
            )
            .into_any_element()
    }

    fn provider_field(&self, palette: &Palette) -> Option<AnyElement> {
        (self.agent == MasterAgent::Goose).then(|| {
            controls::labeled_field(
                "account-add-provider",
                tr(Key("goose.provider")),
                &self.provider_field,
                gpui::relative(1.),
                palette,
            )
        })
    }

    /// The browser is open: the login is finished there, and the account is added by itself.
    fn in_the_browser(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_WIDE)
            .child(
                text(theme::TEXT_SMALL, palette.text_secondary)
                    .debug_selector(|| "account-add-browser".into())
                    .child(tr(WAITING_BROWSER)),
            )
            .child(dialog_frame::footer(palette).child(self.cancel_button(palette, cx)))
            .into_any_element()
    }

    fn cancel_button(
        &self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let this = cx.entity().downgrade();
        kit::button(
            "account-add-cancel",
            tr(Key("common.cancel")),
            ButtonKind::Secondary,
            palette,
        )
        .on_click(move |_, _, cx| drop(this.update(cx, |sheet, cx| sheet.cancel(cx))))
    }
}
