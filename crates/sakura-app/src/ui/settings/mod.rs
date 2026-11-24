//! Settings, in its own window, built on the kit's settings component: at the left a search field
//! over the sections, at the right the section as rows (a title, the sentence that says what the
//! setting changes, its control), the window's title the section's name. A search lists only the
//! sections that hold what it finds and shows only those rows; emptied, it shows again the section
//! that was open. A change applies at once; there is no Save. Where a change needs the service, the
//! row says it is being applied and says a failure in place.
//!
//! Everything is read through a [`SettingsSource`]: the projects when the window opens, the
//! accounts and the machine when their section is first shown, and again only when the person
//! asks ("Retry", "Refresh") or after a change the service took. Each read has a number: an
//! older answer never replaces a newer one. Nothing is read on a timer.
//!
//! The entry point is [`open_in`]; the app listens to [`SettingsEvent`] on the view it returns.

mod add_project;
mod controls;
#[cfg(feature = "restate-engine")]
mod engine;
mod frame;
pub mod host;
mod project_form;
mod rows;
mod sections;
mod source;
mod updates;

pub use source::{BoxSettings, SettingsSource};

use crate::l10n::{self, tr, Key, Language, LanguageChoice};
use crate::theme::settings as sizes;
use add_project::{AddEvent, AddProject};
use controls::{Loaded, OnPress};
use gpui::{
    size, AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, EventEmitter, Global,
    PathPromptOptions, SharedString, Subscription, Task, TitlebarOptions, WeakEntity, Window,
    WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::input::{InputEvent, InputState, TextareaState};
use gpui_component::Root;
use project_form::{FormEvent, ProjectForm};
use sakura_core::error::{BridgeError, BridgeResult};
#[cfg(feature = "restate-engine")]
use sakura_core::models::RunnerKind;
use sakura_core::models::{Account, BoxStatus};
use sakura_core::settings::{Listed, Project};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// One keyboard shortcut, as the app names it: the menu it is in and what it does (both already in
/// the interface's language) and its keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub menu: SharedString,
    pub action: SharedString,
    pub keys: SharedString,
}

/// Reads the shortcuts as they are now: the menu bar's own table, in the language in force.
pub type ShortcutTable = Rc<dyn Fn(&App) -> Vec<Shortcut>>;

/// Which appearance the window takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeChoice {
    /// The Mac's own.
    System,
    Light,
    Dark,
}

/// What the system tells the person of while the app is not in front. Both are told until one is
/// turned off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notify {
    /// A task starts to need the person.
    pub needs_you: bool,
    /// An agent that was at work ended.
    pub finished: bool,
}

/// Reads the language choice in force.
pub type ReadLanguage = Rc<dyn Fn(&App) -> LanguageChoice>;
/// Makes a language choice: it is kept and in force at once.
pub type SetLanguage = Rc<dyn Fn(LanguageChoice, &mut App)>;

/// The language choice of the localisation layer: how it is read and how it is set.
#[derive(Clone)]
pub struct LanguageControl {
    pub current: ReadLanguage,
    pub set: SetLanguage,
}

/// The terminal's text: its family (none: Sakura's own) and its size in points (none: the theme's).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalText {
    pub family: Option<String>,
    pub size: Option<u8>,
}

/// Reads the installed families a terminal can be set in, by name, off the main thread.
pub type Families = Rc<dyn Fn(&mut App) -> Task<Vec<String>>>;

/// Asks the person for a folder; `None` when none was chosen.
pub type FolderPicker = Rc<dyn Fn(&mut App) -> Task<Option<PathBuf>>>;

/// The Mac's own folder picker.
pub fn system_folder_picker() -> FolderPicker {
    Rc::new(|cx: &mut App| {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.background_spawn(async move {
            let folders = chosen.await.ok()?.ok()??;
            folders.into_iter().next()
        })
    })
}

/// Where a project is, as the person reads it: its repository and the folder in it, the home folder
/// written as `~`.
fn project_folder(project: &Project) -> String {
    let folder = match project.subdir.as_str() {
        "" => project.repo.clone(),
        subdir => format!("{}/{subdir}", project.repo),
    };
    home_as_tilde(Path::new(&folder))
}

/// A folder as the person reads it: one under the home folder is written from `~`.
fn home_as_tilde(folder: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.and_then(|home| folder.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => folder.display().to_string(),
    }
}

/// What Settings is given by the app.
pub struct Deps {
    pub source: Rc<dyn SettingsSource>,
    /// The table "Shortcuts" lists, read only.
    pub shortcuts: ShortcutTable,
    /// The appearance in force.
    pub theme: ThemeChoice,
    pub language: LanguageControl,
    /// The publishing request of each project that has one of its own, by the project's name.
    pub publish_requests: HashMap<String, String>,
    /// What "Publish" asks the agent for a project with no request of its own, when the person
    /// wrote one; none is the app's own request.
    pub publish_default: Option<String>,
    /// What the person is told of while the app is not in front.
    pub notify: Notify,
    /// The terminal's text in force.
    pub terminal: TerminalText,
    pub families: Families,
    /// The folder the service keeps its data in, as the app gave it to the service.
    pub data_folder: SharedString,
    pub pick_folder: FolderPicker,
}

/// What Settings tells the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsEvent {
    /// "Sign in" was pressed for an account of the boxes: the app's own sign-in flow takes over.
    SignInAsked { account: String },
    /// An appearance was chosen: the app keeps it and applies it.
    ThemeChosen(ThemeChoice),
    /// A project's publishing request was changed: its own text, or none (the default one). The
    /// app keeps it.
    PublishRequestChanged {
        project: String,
        request: Option<String>,
    },
    /// The service took a change to the projects (one added, changed or removed).
    ProjectsChanged,
    /// The request "Publish" sends for projects with none of their own was changed: the person's
    /// text, or none (the app's own). The app keeps it.
    PublishDefaultChanged(Option<String>),
    /// What the person is told of was changed. The app keeps it and follows it from now on.
    NotifyChanged(Notify),
    /// The terminal's text was chosen. The app keeps it and the open terminals take it at once.
    TerminalTextChosen(TerminalText),
}

/// The sections, in the order the list shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Projects,
    Accounts,
    Agent,
    Appearance,
    Notifications,
    Machine,
    /// Where the data is and how much of its budget each task in a box has taken.
    Storage,
    /// The running version, the automatic check and Check Now.
    Updates,
    Shortcuts,
    /// The brand's page: the product's name, its version and where it comes from.
    About,
}

impl Section {
    pub const ALL: [Section; 10] = [
        Section::Projects,
        Section::Accounts,
        Section::Agent,
        Section::Appearance,
        Section::Notifications,
        Section::Machine,
        Section::Storage,
        Section::Updates,
        Section::Shortcuts,
        Section::About,
    ];

    /// The key of the section's name: its row in the list and the window's title.
    pub fn title(self) -> Key {
        Key(match self {
            Section::Projects => "settings.section.projects",
            Section::Accounts => "settings.section.accounts",
            Section::Agent => "settings.section.agent",
            Section::Appearance => "settings.section.appearance",
            Section::Notifications => "settings.section.notifications",
            Section::Machine => "settings.section.machine",
            Section::Storage => "settings.section.storage",
            Section::Updates => "settings.section.updates",
            Section::Shortcuts => "settings.section.shortcuts",
            Section::About => "settings.section.about",
        })
    }

    /// What names the section's row in the window whatever the language: for its elements, never
    /// shown.
    fn id(self) -> &'static str {
        match self {
            Section::Projects => "settings-section-projects",
            Section::Accounts => "settings-section-accounts",
            Section::Agent => "settings-section-agent",
            Section::Appearance => "settings-section-appearance",
            Section::Notifications => "settings-section-notifications",
            Section::Machine => "settings-section-machine",
            Section::Storage => "settings-section-storage",
            Section::Updates => "settings-section-updates",
            Section::Shortcuts => "settings-section-shortcuts",
            Section::About => "settings-section-about",
        }
    }
}

/// What is read from the service, each with the number of its newest read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Read {
    Projects,
    Accounts,
    Machine,
}

/// What "Projects" shows: the list, one project, or the flow that adds one.
enum ProjectsPage {
    List,
    Form(Entity<ProjectForm>),
    Adding(Entity<AddProject>),
}

/// The Settings window's view.
pub struct Settings {
    source: Rc<dyn SettingsSource>,
    shortcuts: ShortcutTable,
    theme: ThemeChoice,
    language: LanguageControl,
    requests: HashMap<String, String>,
    /// The request for projects with none of their own, and the field it is written in.
    publish_default: Option<String>,
    default_request: Entity<TextareaState>,
    /// Why the text last typed there was not kept.
    default_refused: Option<BridgeError>,
    notify: Notify,
    terminal: TerminalText,
    families_of: Families,
    /// The families a terminal can be set in; not read before "Appearance" is first shown.
    families: Option<Vec<String>>,
    data_folder: SharedString,
    pick_folder: FolderPicker,
    /// The section the person chose. A search that finds nothing in it shows the first section that
    /// holds what it finds; emptied, it shows this one again.
    section: Section,
    /// The search field over the sections; empty when the window opens, and without the caret.
    search: Entity<InputState>,
    projects: Loaded<Vec<Listed>>,
    /// Not read before their section, or a project, is first shown.
    accounts: Option<Loaded<Vec<Account>>>,
    machine: Option<Loaded<BoxStatus>>,
    /// The runner of the checks the person chose, while `box` writes it.
    #[cfg(feature = "restate-engine")]
    runner_asked: Option<RunnerKind>,
    /// Why `box` did not change the runner, until the next change.
    #[cfg(feature = "restate-engine")]
    runner_refusal: Option<BridgeError>,
    reads: HashMap<Read, u64>,
    page: ProjectsPage,
    /// The project to open once the projects are read.
    wanted: Option<String>,
    /// The window the view is in: a field of the project on screen is written in it.
    window: AnyWindowHandle,
    /// The section and language the window's title was last written for.
    titled: Option<(Section, Language)>,
    _page_subscription: Option<Subscription>,
    _default_request_left: Subscription,
    _search_changed: Subscription,
}

impl EventEmitter<SettingsEvent> for Settings {}

/// The open Settings window, so a second Cmd+comma brings it forward instead of opening another.
struct OpenSettings {
    window: WindowHandle<Root>,
    view: WeakEntity<Settings>,
}

impl Global for OpenSettings {}

/// Opens Settings, or brings its window forward when it is open. The view is returned so the app
/// can listen to its [`SettingsEvent`]s; `None` when the window could not be opened. The window is
/// shown, or not (off screen, it is only rendered to a file).
pub fn open_in(deps: Deps, is_shown: bool, cx: &mut App) -> Option<Entity<Settings>> {
    let is_shown = is_shown && crate::window::can_activate();
    if let Some(open) = cx.try_global::<OpenSettings>() {
        let (window, view) = (open.window, open.view.upgrade());
        let is_there = window
            .update(cx, |_, window, _| {
                if is_shown {
                    crate::window::activate(window);
                }
            })
            .is_ok();
        if let (true, Some(view)) = (is_there, view) {
            return Some(view);
        }
    }
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let window = cx.open_window(options(is_shown, cx), move |window, cx| {
        let view = cx.new(|cx| Settings::new(deps, window, cx));
        *kept.borrow_mut() = Some(view.clone());
        cx.new(|cx| Root::new(view, window, cx))
    });
    let view: Entity<Settings> = slot.borrow_mut().take()?;
    let window = window.ok()?;
    cx.set_global(OpenSettings {
        window,
        view: view.downgrade(),
    });
    Some(view)
}

/// A standard window of the Mac: its own title bar, the title the section's name.
fn options(is_shown: bool, cx: &App) -> WindowOptions {
    let start = size(sizes::SETTINGS_WIDTH, sizes::SETTINGS_HEIGHT);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, start, cx))),
        titlebar: Some(TitlebarOptions {
            title: Some(tr(Section::Projects.title()).into()),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        window_min_size: Some(size(sizes::SETTINGS_MIN_WIDTH, sizes::SETTINGS_MIN_HEIGHT)),
        focus: is_shown,
        show: is_shown,
        ..WindowOptions::default()
    }
}

impl Settings {
    /// The view, on "Projects", whose list is read once now.
    pub fn new(deps: Deps, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let default_request = sections::default_request_field(&deps, window, cx);
        let left = cx.subscribe_in(&default_request, window, Self::default_request_left);
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(tr(Key("settings.search.placeholder")))
                .clean_on_escape()
        });
        let changed = cx.subscribe(&search, |settings, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                settings.query_changed(cx);
            }
        });
        let mut settings = Self {
            source: deps.source,
            shortcuts: deps.shortcuts,
            theme: deps.theme,
            language: deps.language,
            requests: deps.publish_requests,
            publish_default: deps.publish_default,
            default_request,
            default_refused: None,
            notify: deps.notify,
            terminal: deps.terminal,
            families_of: deps.families,
            families: None,
            data_folder: deps.data_folder,
            pick_folder: deps.pick_folder,
            section: Section::Projects,
            search,
            projects: Loaded::Reading,
            accounts: None,
            machine: None,
            #[cfg(feature = "restate-engine")]
            runner_asked: None,
            #[cfg(feature = "restate-engine")]
            runner_refusal: None,
            reads: HashMap::new(),
            page: ProjectsPage::List,
            wanted: None,
            window: window.window_handle(),
            titled: None,
            _page_subscription: None,
            _default_request_left: left,
            _search_changed: changed,
        };
        settings.read(Read::Projects, cx);
        settings
    }

    /// Shows `section`; what it lists is read the first time it is shown.
    pub fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.reveal(section, cx);
        cx.notify();
    }

    /// Reads what `section` lists, the first time it is on screen.
    fn reveal(&mut self, section: Section, cx: &mut Context<Self>) {
        match section {
            Section::Accounts if self.accounts.is_none() => self.read(Read::Accounts, cx),
            Section::Machine | Section::Storage if self.machine.is_none() => {
                self.read(Read::Machine, cx);
            }
            Section::Appearance if self.families.is_none() => self.read_families(cx),
            _ => {}
        }
    }

    /// The search changed: the section on screen may be another, and is read if it never was.
    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let found = self.found(cx);
        self.reveal(self.shown(&found), cx);
        cx.notify();
    }

    /// What the search finds now: each section that holds a row it finds, in the order of the list,
    /// with only those rows. With no search, every section and every row.
    fn found(&self, cx: &Context<Self>) -> Vec<(Section, Vec<rows::Group>)> {
        let query = self.search.read(cx).value();
        let found = Section::ALL.into_iter().map(|section| {
            let groups = rows::matching(self.groups(section, cx), &query);
            (section, groups)
        });
        found.filter(|(_, groups)| !groups.is_empty()).collect()
    }

    /// The section on screen: the one chosen while the search finds something in it, else the first
    /// that holds what the search finds.
    fn shown(&self, found: &[(Section, Vec<rows::Group>)]) -> Section {
        let is_found = |section: Section| found.iter().any(|(listed, _)| *listed == section);
        match found.first() {
            Some((first, _)) if !is_found(self.section) => *first,
            _ => self.section,
        }
    }

    /// Reads the families a terminal can be set in, once: until they are here the menu has Sakura's
    /// own and the one in force.
    fn read_families(&mut self, cx: &mut Context<Self>) {
        self.families = Some(Vec::new());
        let families = (self.families_of)(cx);
        cx.spawn(async move |settings, cx| {
            let families = families.await;
            drop(settings.update(cx, |settings, cx| {
                settings.families = Some(families);
                cx.notify();
            }));
        })
        .detach();
    }

    /// Sets the terminal's family (none: Sakura's own): the app keeps it and applies it at once.
    pub fn set_terminal_family(&mut self, family: Option<String>, cx: &mut Context<Self>) {
        if self.terminal.family != family {
            self.terminal.family = family;
            cx.emit(SettingsEvent::TerminalTextChosen(self.terminal.clone()));
            cx.notify();
        }
    }

    /// Sets the size of the terminal's text, in points: the app keeps it and applies it at once.
    pub fn set_terminal_size(&mut self, size: u8, cx: &mut Context<Self>) {
        if self.terminal.size != Some(size) {
            self.terminal.size = Some(size);
            cx.emit(SettingsEvent::TerminalTextChosen(self.terminal.clone()));
            cx.notify();
        }
    }

    /// Shows the project named, in "Projects": where Play's "no run command" link leads. While
    /// the projects are still being read it opens as soon as they are. A search left in the field
    /// is emptied, so the project is on screen.
    pub fn show_project(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
        self.section = Section::Projects;
        if self.projects.ready().is_some() {
            self.open_project(name, window, cx);
        } else {
            self.wanted = Some(name.to_string());
        }
        cx.notify();
    }

    /// Opens the project that was requested before the projects were read.
    fn open_wanted(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.wanted.take() else {
            return;
        };
        let (settings, window) = (cx.weak_entity(), self.window);
        cx.defer(move |cx| {
            drop(window.update(cx, |_, window, cx| {
                drop(settings.update(cx, |settings, cx| {
                    settings.open_project(&name, window, cx);
                }));
            }));
        });
    }

    /// The number of a new read of `what`: an answer carrying an older number is dropped.
    fn next(&mut self, what: Read) -> u64 {
        let number = self.reads.entry(what).or_insert(0);
        *number += 1;
        *number
    }

    /// Reads `what` from the service: when it is first shown, after a change, or when asked.
    fn read(&mut self, what: Read, cx: &mut Context<Self>) {
        let number = self.next(what);
        match what {
            Read::Projects => {
                let answer = self.source.projects();
                cx.spawn(async move |settings, cx| {
                    let answer = answer.await;
                    drop(settings.update(cx, |settings, cx| {
                        settings.projects_read(number, answer, cx)
                    }));
                })
                .detach();
            }
            Read::Accounts => {
                self.accounts = Some(Loaded::Reading);
                let answer = self.source.accounts();
                cx.spawn(async move |settings, cx| {
                    let answer = answer.await;
                    drop(settings.update(cx, |settings, cx| {
                        settings.accounts_read(number, answer, cx)
                    }));
                })
                .detach();
            }
            Read::Machine => {
                self.machine = Some(Loaded::Reading);
                let answer = self.source.status();
                cx.spawn(async move |settings, cx| {
                    let answer = answer.await;
                    drop(settings.update(cx, |settings, cx| {
                        if settings.reads.get(&Read::Machine) == Some(&number) {
                            settings.machine =
                                Some(answer.map_or_else(Loaded::Failed, Loaded::Ready));
                            cx.notify();
                        }
                    }));
                })
                .detach();
            }
        }
        cx.notify();
    }

    fn projects_read(
        &mut self,
        number: u64,
        answer: BridgeResult<Vec<Listed>>,
        cx: &mut Context<Self>,
    ) {
        if self.reads.get(&Read::Projects) != Some(&number) {
            return;
        }
        self.projects = answer.map_or_else(Loaded::Failed, Loaded::Ready);
        // The project on screen follows what the service has now; one that is gone closes.
        if let ProjectsPage::Form(form) = &self.page {
            let form = form.clone();
            let name = form.read(cx).name().to_string();
            match self.project(&name).cloned() {
                Some(project) => drop(self.window.update(cx, |_, window, cx| {
                    form.update(cx, |form, cx| form.set_project(project, window, cx));
                })),
                None if self.projects.ready().is_some() => self.close_page(),
                None => {}
            }
        }
        self.open_wanted(cx);
        cx.notify();
    }

    fn accounts_read(
        &mut self,
        number: u64,
        answer: BridgeResult<Vec<Account>>,
        cx: &mut Context<Self>,
    ) {
        if self.reads.get(&Read::Accounts) != Some(&number) {
            return;
        }
        let accounts = answer.map_or_else(Loaded::Failed, Loaded::Ready);
        if let (ProjectsPage::Form(form), Some(list)) = (&self.page, accounts.ready()) {
            form.update(cx, |form, cx| form.set_accounts(list.clone(), cx));
        }
        self.accounts = Some(accounts);
        cx.notify();
    }

    /// The usable project named, in the list as it was last read.
    fn project(&self, name: &str) -> Option<&Project> {
        self.projects
            .ready()?
            .iter()
            .find_map(|listed| match listed {
                Listed::Usable(project) if project.name == name => Some(&**project),
                _ => None,
            })
    }

    fn close_page(&mut self) {
        self.page = ProjectsPage::List;
        self._page_subscription = None;
    }

    /// Opens the form of the project named, when it is one that can be used.
    fn open_project(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project(name).cloned() else {
            return;
        };
        if self.accounts.is_none() {
            self.read(Read::Accounts, cx);
        }
        let accounts = self.accounts.as_ref().and_then(Loaded::ready).cloned();
        let (source, request) = (self.source.clone(), self.requests.get(name).cloned());
        let form = cx.new(|cx| {
            ProjectForm::new(
                source,
                project,
                accounts.unwrap_or_default(),
                request,
                window,
                cx,
            )
        });
        self._page_subscription = Some(cx.subscribe(&form, Self::form_said));
        self.page = ProjectsPage::Form(form);
        cx.notify();
    }

    fn form_said(&mut self, form: Entity<ProjectForm>, event: &FormEvent, cx: &mut Context<Self>) {
        match event {
            FormEvent::Back => self.close_page(),
            FormEvent::Changed => {
                self.read(Read::Projects, cx);
                cx.emit(SettingsEvent::ProjectsChanged);
            }
            FormEvent::Removed => {
                self.close_page();
                self.read(Read::Projects, cx);
                cx.emit(SettingsEvent::ProjectsChanged);
            }
            FormEvent::RequestChanged(request) => {
                let project = form.read(cx).name().to_string();
                match request {
                    Some(request) => drop(self.requests.insert(project.clone(), request.clone())),
                    None => drop(self.requests.remove(&project)),
                }
                let request = request.clone();
                cx.emit(SettingsEvent::PublishRequestChanged { project, request });
            }
        }
        cx.notify();
    }

    /// "Add project": the flow starts with the folder picker.
    pub fn add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (source, pick_folder) = (self.source.clone(), self.pick_folder.clone());
        let flow = cx.new(|cx| AddProject::new(source, pick_folder, window, cx));
        self._page_subscription = Some(cx.subscribe(&flow, Self::flow_said));
        self.page = ProjectsPage::Adding(flow);
        cx.notify();
    }

    fn flow_said(&mut self, _: Entity<AddProject>, event: &AddEvent, cx: &mut Context<Self>) {
        self.close_page();
        if let AddEvent::Added(_) = event {
            self.read(Read::Projects, cx);
            cx.emit(SettingsEvent::ProjectsChanged);
        }
        cx.notify();
    }

    /// A handler that calls `act` on this view.
    fn on(
        cx: &Context<Self>,
        act: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> OnPress {
        let settings = cx.weak_entity();
        Rc::new(move |window, cx| {
            drop(settings.update(cx, |settings, cx| act(settings, window, cx)))
        })
    }

    /// The window's title is the name of the section on screen, in the language in force.
    fn write_title(&mut self, shown: Section, window: &mut Window) {
        let now = (shown, l10n::language());
        if self.titled != Some(now) {
            window.set_window_title(tr(shown.title()));
            self.titled = Some(now);
        }
    }
}

#[cfg(test)]
mod tests;
