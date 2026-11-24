//! Settings driven on GPUI's test platform over a source that notes what is asked: what is read
//! and when, changes sent at once and said in place when they fail, answers that come late, the
//! flow that adds a project, and where things stand on the layout grid in both languages.

use super::controls::Sent;
use super::project_form::{apply, ProjectForm, Row};
use super::source::SettingsSource;
use super::{
    Deps, LanguageControl, Notify, ProjectsPage, Read, Settings, SettingsEvent, Shortcut,
    TerminalText, ThemeChoice,
};
use crate::l10n::{self, Language, LanguageChoice};
use crate::theme::settings as sizes;
use crate::theme::{self, Appearance, Theme};
use crate::ui::files::Reading;
use crate::ui::real_text;
use gpui::{
    AppContext as _, Bounds, Entity, Modifiers, Pixels, Task, TestAppContext, VisualTestContext,
};
use gpui_component::input::InputState;
use sakura_core::error::{BridgeError, BridgeResult};
use sakura_core::models::{
    Account, BoxStatus, CheckRunner, DiskUse, EnginePrerequisites, Place, RunnerKind,
};
use sakura_core::said::Said;
use sakura_core::settings::detect::{parse_detected, Addition, Detected};
use sakura_core::settings::{parse_projects, Branches, Change, Listed};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

/// How far two edges may stand apart and still be on one line.
const NEAR: f32 = 0.5;

const PROJECTS: &str = r#"[
 { "name": "old-site", "error": "its repository does not exist" },
 { "name": "shop-web", "repo": "/home/demo/code/shop", "subdir": "apps/web", "base": "main", "where": "mac",
   "branchPrefix": "sakura/", "account": "personal", "setup": "npm ci", "archive": null,
   "run": "npm run dev", "ports": 10, "runMode": "one", "install": "npm ci",
   "checks": [ { "verb": "test", "command": "npm test" } ],
   "protected": null, "testFiles": null, "tasks": ["sort-folders"], "error": null } ]"#;

const DETECTED: &str = r#"{ "repo": "/home/demo/code/atlante", "subdir": "apps/site", "name": "site",
  "currentBranch": "main", "defaultBranch": "main", "branches": ["main"], "install": null,
  "checks": [ { "verb": "test", "command": "npm test", "reason": "script test" } ],
  "protected": [], "testFiles": [], "notes": ["a toolchain the boxes do not have"] }"#;

const STATUS: &str = r#"{ "runner": { "state": "stopped", "current": null, "queue": [] },
 "tasks": [ { "name": "in-un-box", "project": "atlas-api", "profile": "atlas-api", "where": "box", "folder": null,
   "checks": [], "base": "main", "baseSha": "abcdefabcdefabcdefabcdefabcdefabcdefabcd", "readable": true,
   "branch": "box/in-un-box", "head": "0123456789012345678901234567890123456789",
   "headSubject": "Add the endpoint", "headTime": "2026-11-10T15:00:00Z", "ahead": 0, "dirtyFiles": 0, "filesChanged": 0, "insertions": 0,
   "deletions": 0, "taken": null, "agent": { "container": "c", "account": "personal", "state": "running",
   "cpuPercent": 12.5, "memoryMb": 812, "startedAt": "2026-11-10T15:30:00Z", "exitCode": null },
   "otherContainers": 0, "results": {},
   "disk": { "bytes": 1288490188, "entries": 12400, "limitBytes": 8589934592, "entryLimit": 200000,
     "overBudget": false, "enforcement": "checked" } } ] }"#;

/// A source that answers from memory and notes every asking.
struct Fake {
    asked: RefCell<Vec<String>>,
    projects: RefCell<Vec<Listed>>,
    /// Reads fail.
    is_unreadable: Cell<bool>,
    /// Changes, additions and removals are refused.
    is_refusing: Cell<bool>,
    /// While set, an answer waits until the test lets it go.
    is_holding: Cell<bool>,
    /// The choice of runner `box status` says; none from a service with one runner.
    check_runner: RefCell<Option<CheckRunner>>,
    held: RefCell<Vec<Box<dyn FnOnce()>>>,
}

fn refusal() -> BridgeError {
    BridgeError::failed(Said::new("error.internal")).with_details(Some("box: refused".into()))
}

impl Fake {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            asked: RefCell::default(),
            projects: RefCell::new(parse_projects(PROJECTS.as_bytes()).expect("the fixture")),
            is_unreadable: Cell::new(false),
            is_refusing: Cell::new(false),
            is_holding: Cell::new(false),
            check_runner: RefCell::new(None),
            held: RefCell::default(),
        })
    }

    fn answer<T: Send + 'static>(&self, call: String, answer: BridgeResult<T>) -> Reading<T> {
        self.asked.borrow_mut().push(call);
        if !self.is_holding.get() {
            return Box::pin(std::future::ready(answer));
        }
        let (send, receive) = tokio::sync::oneshot::channel();
        self.held
            .borrow_mut()
            .push(Box::new(move || drop(send.send(answer))));
        Box::pin(async move { receive.await.unwrap_or_else(|_| Err(refusal())) })
    }

    fn read<T: Send + 'static>(&self, call: &str, found: T) -> Reading<T> {
        let answer = if self.is_unreadable.get() {
            Err(refusal())
        } else {
            Ok(found)
        };
        self.answer(call.to_string(), answer)
    }

    fn write(&self, call: String, change: impl FnOnce(&mut Vec<Listed>)) -> Reading<()> {
        if self.is_refusing.get() {
            return self.answer(call, Err(refusal()));
        }
        change(&mut self.projects.borrow_mut());
        self.answer(call, Ok(()))
    }

    /// Lets the answer that has waited longest go.
    fn release(&self) {
        let let_go = self.held.borrow_mut().remove(0);
        let_go();
    }

    /// Lets the answer requested last go, before the ones requested earlier.
    fn release_newest(&self) {
        let let_go = self.held.borrow_mut().pop().expect("an answer waits");
        let_go();
    }

    fn asked(&self) -> Vec<String> {
        self.asked.borrow().clone()
    }

    /// How many askings hold `start`.
    fn count(&self, start: &str) -> usize {
        self.asked
            .borrow()
            .iter()
            .filter(|asked| asked.contains(start))
            .count()
    }
}

impl SettingsSource for Fake {
    fn projects(&self) -> Reading<Vec<Listed>> {
        self.read("projects", self.projects.borrow().clone())
    }

    fn branches(&self, project: &str) -> Reading<Branches> {
        let branches = Branches {
            default: "main".into(),
            names: vec!["feature/cart".into(), "main".into()],
        };
        self.read(&format!("branches {project}"), branches)
    }

    fn accounts(&self) -> Reading<Vec<Account>> {
        let account = |name: &str, place, logged_in| Account {
            name: name.into(),
            place,
            logged_in,
            exists: (place == Place::Mac).then_some(true),
        };
        let accounts = vec![
            account("personal", Place::Box, true),
            account("work", Place::Box, false),
            account("studio", Place::Box, false),
            account("personal", Place::Mac, true),
        ];
        self.read("accounts", accounts)
    }

    fn status(&self) -> Reading<BoxStatus> {
        let mut status: BoxStatus = serde_json::from_str(STATUS).expect("the fixture");
        status.check_runner = self.check_runner.borrow().clone();
        self.read("status", status)
    }

    #[cfg(feature = "restate-engine")]
    fn set_runner(&self, runner: RunnerKind) -> Reading<()> {
        let call = format!("runner {}", runner.word());
        if self.is_refusing.get() {
            return self.answer(call, Err(refusal()));
        }
        if let Some(choice) = self.check_runner.borrow_mut().as_mut() {
            choice.runner = runner;
        }
        self.answer(call, Ok(()))
    }

    fn set(&self, project: &str, change: Change) -> Reading<()> {
        let (option, value) = change.option().expect("only checked changes are sent");
        let call = format!("set {project} {option} {value}");
        let name = project.to_string();
        self.write(call, move |projects| {
            for listed in projects.iter_mut() {
                if let Listed::Usable(project) = listed {
                    if project.name == name {
                        apply(project, &change);
                    }
                }
            }
        })
    }

    fn detect(&self, folder: &str) -> Reading<Detected> {
        let detected: Detected = parse_detected(DETECTED.as_bytes()).expect("the fixture");
        let answer = if self.is_refusing.get() {
            Err(refusal())
        } else {
            Ok(detected)
        };
        self.answer(format!("detect {folder}"), answer)
    }

    fn add(&self, addition: Addition) -> Reading<()> {
        let call = format!("add {}", addition.name);
        self.write(call, |projects| {
            projects.push(Listed::Unusable {
                name: addition.name,
                error: "just added".into(),
            });
        })
    }

    fn remove(&self, project: &str) -> Reading<()> {
        let name = project.to_string();
        self.write(format!("rm {project}"), move |projects| {
            projects.retain(|listed| listed.name() != name);
        })
    }
}

struct Opened<'a> {
    cx: &'a mut VisualTestContext,
    settings: Entity<Settings>,
    source: Rc<Fake>,
    events: Rc<RefCell<Vec<SettingsEvent>>>,
    /// The language choices the view asked the localisation layer to make.
    languages: Rc<RefCell<Vec<LanguageChoice>>>,
    /// What the folder picker answers.
    folder: Rc<RefCell<Option<PathBuf>>>,
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn open(cx: &mut TestAppContext) -> Opened<'_> {
    open_over(cx, Fake::new())
}

#[gpui::test]
fn updates_section_shows_the_build_version_and_its_unconfigured_state(cx: &mut TestAppContext) {
    crate::l10n::set_for_test(Language::English);
    let opened = open(cx);
    let section = super::Section::ALL
        .into_iter()
        .find(|section| section.title() == crate::l10n::Key("settings.section.updates"))
        .expect("Updates is a Settings section");
    opened
        .settings
        .update(opened.cx, |settings, cx| settings.show(section, cx));
    draw(opened.cx);
    assert!(is_there(opened.cx, "settings-updates-version"));
    assert!(is_there(opened.cx, "settings-updates-automatic"));
    assert!(is_there(opened.cx, "settings-updates-check"));
}

#[gpui::test]
fn updates_rows_show_each_state_as_a_searchable_sentence(cx: &mut TestAppContext) {
    use crate::updates::{Release, UpdateState};
    let opened = open(cx);
    let section = super::Section::ALL
        .into_iter()
        .find(|section| section.title() == crate::l10n::Key("settings.section.updates"))
        .expect("Updates is a Settings section");
    let available = UpdateState::Available(Release {
        version: "999.0.0".into(),
        url: "https://github.com/example/app/releases/tag/v999.0.0".into(),
    });
    for state in [
        UpdateState::NotSetUp,
        UpdateState::Ready,
        UpdateState::Checking,
        UpdateState::UpToDate,
        available,
        UpdateState::Failed,
    ] {
        opened
            .cx
            .update(|_, cx| crate::updates::install_for_test(state.clone(), cx));
        opened.settings.update(opened.cx, |settings, cx| {
            let groups = settings.groups(section, cx);
            let row = &groups[0].rows[2];
            assert_eq!(
                row.description().map(|text| text.as_ref()),
                Some(crate::updates::sentence(&state).as_str())
            );
            assert!(row.matches(&crate::updates::sentence(&state)));
        });
    }
}

#[gpui::test]
fn updates_toggle_is_kept_without_taking_the_keyboard_or_drawing_a_ring(cx: &mut TestAppContext) {
    let opened = open(cx);
    opened
        .cx
        .update(|_, cx| crate::prefs::Preferences::default().install(None, cx));
    let section = super::Section::ALL
        .into_iter()
        .find(|section| section.title() == crate::l10n::Key("settings.section.updates"))
        .expect("Updates is a Settings section");
    opened
        .settings
        .update(opened.cx, |settings, cx| settings.show(section, cx));
    draw(opened.cx);
    opened
        .cx
        .update(|window, cx| assert!(window.focused(cx).is_none()));
    click(opened.cx, "settings-updates-automatic");
    opened.cx.update(|window, cx| {
        assert!(!crate::prefs::Preferences::current(cx).is_automatic_updates_enabled);
        assert!(window.focused(cx).is_none());
    });
    assert_eq!(crate::ui::focus_tests::rings(opened.cx), 0);
}

#[gpui::test]
fn updates_available_release_opens_its_public_page_without_taking_focus(cx: &mut TestAppContext) {
    let opened = open(cx);
    let url = "https://github.com/example/app/releases/tag/v999.0.0";
    opened.cx.update(|_, cx| {
        crate::updates::install_for_test(
            crate::updates::UpdateState::Available(crate::updates::Release {
                version: "999.0.0".into(),
                url: url.into(),
            }),
            cx,
        )
    });
    let section = super::Section::ALL
        .into_iter()
        .find(|section| section.title() == crate::l10n::Key("settings.section.updates"))
        .unwrap();
    opened
        .settings
        .update(opened.cx, |settings, cx| settings.show(section, cx));
    draw(opened.cx);
    click(opened.cx, "settings-update-release");
    assert_eq!(opened.cx.opened_url().as_deref(), Some(url));
    opened
        .cx
        .update(|window, cx| assert!(window.focused(cx).is_none()));
    assert_eq!(crate::ui::focus_tests::rings(opened.cx), 0);
}

fn open_over(cx: &mut TestAppContext, source: Rc<Fake>) -> Opened<'_> {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
    });
    let events: Rc<RefCell<Vec<SettingsEvent>>> = Rc::default();
    let languages: Rc<RefCell<Vec<LanguageChoice>>> = Rc::default();
    let folder = Rc::new(RefCell::new(Some(PathBuf::from(
        "/home/demo/code/atlante/apps/site",
    ))));
    let (chosen, picked) = (languages.clone(), folder.clone());
    let deps = Deps {
        source: source.clone(),
        shortcuts: Rc::new(|_| {
            vec![Shortcut {
                menu: "File".into(),
                action: "New task".into(),
                keys: "⌘N".into(),
            }]
        }),
        theme: ThemeChoice::System,
        language: LanguageControl {
            current: Rc::new(|_| LanguageChoice::Automatic),
            set: Rc::new(move |choice, _| chosen.borrow_mut().push(choice)),
        },
        publish_requests: HashMap::new(),
        publish_default: None,
        notify: Notify {
            needs_you: true,
            finished: true,
        },
        terminal: TerminalText::default(),
        families: Rc::new(|_| Task::ready(vec!["Menlo".into(), "Monaco".into()])),
        data_folder: "/Volumes/work/a-folder-with-a-long-name/and-another-inside-it/sakura-data"
            .into(),
        pick_folder: Rc::new(move |_| Task::ready(picked.borrow().clone())),
    };
    let noted = events.clone();
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let settings = cx.new(|cx| Settings::new(deps, window, cx));
        cx.subscribe(&settings, move |_, _, event: &SettingsEvent, _| {
            noted.borrow_mut().push(event.clone());
        })
        .detach();
        *kept.borrow_mut() = Some(settings.clone());
        gpui_component::Root::new(settings, window, cx)
    });
    let settings: Entity<Settings> = slot.borrow_mut().take().expect("the view was built");
    cx.update(|window, _| window.activate_window());
    draw(cx);
    Opened {
        cx,
        settings,
        source,
        events,
        languages,
        folder,
    }
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"))
}

fn is_there(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

/// A press on what `selector` names; one below the window is scrolled into view first, as the
/// person scrolls the page to reach it.
fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let height = cx.update(|window, _| window.viewport_size().height);
    let target = bounds(cx, selector);
    if target.bottom() > height {
        let wheel = gpui::ScrollEvent {
            position: gpui::point(target.center().x, height / 2.),
            delta: gpui::ScrollDelta::Pixels(gpui::point(
                gpui::px(0.),
                height / 2. - target.bottom(),
            )),
            ..Default::default()
        };
        cx.simulate_event(wheel);
        draw(cx);
    }
    let centre = bounds(cx, selector).center();
    cx.simulate_click(centre, Modifiers::none());
    draw(cx);
}

impl Opened<'_> {
    fn rest(&mut self, minutes: u64) {
        for _ in 0..minutes {
            self.cx.executor().advance_clock(Duration::from_secs(60));
            self.cx.run_until_parked();
        }
    }

    /// The form of the project on screen.
    fn form(&mut self) -> Entity<ProjectForm> {
        self.settings
            .read_with(self.cx, |settings, _| match &settings.page {
                ProjectsPage::Form(form) => form.clone(),
                _ => panic!("no project is open"),
            })
    }

    fn open_project(&mut self) -> Entity<ProjectForm> {
        click(self.cx, "settings-project-shop-web");
        self.form()
    }

    /// Gives `field` the keyboard, replaces what it holds with `text`, and presses Enter.
    fn enter(&mut self, field: &Entity<InputState>, text: &str) {
        self.type_in(field, text);
        self.cx.simulate_keystrokes("enter");
        draw(self.cx);
    }

    /// Gives `field` the keyboard and replaces what it holds with `text`.
    fn type_in(&mut self, field: &Entity<InputState>, text: &str) {
        field.update_in(self.cx, |field, window, cx| {
            field.focus(window, cx);
            field.set_value("", window, cx);
        });
        draw(self.cx);
        self.cx.simulate_input(text);
        draw(self.cx);
    }

    fn sent(&mut self, row: &Row) -> Option<Sent> {
        let form = self.form();
        form.read_with(self.cx, |form, _| form.sent.get(row).cloned())
    }

    fn field(&mut self, pick: fn(&ProjectForm) -> &Entity<InputState>) -> Entity<InputState> {
        let form = self.form();
        form.read_with(self.cx, |form, _| pick(form).clone())
    }
}

#[gpui::test]
fn the_about_page_is_the_last_section_and_says_the_version_and_reads_nothing(
    cx: &mut TestAppContext,
) {
    let opened = open(cx);
    assert_eq!(super::Section::ALL.last(), Some(&super::Section::About));
    assert!(!is_there(opened.cx, "about-version"));
    click(opened.cx, "settings-section-about");
    assert!(is_there(opened.cx, "about-version"));
    assert!(is_there(opened.cx, "about-repository"));
    assert_eq!(opened.source.asked(), ["projects"], "the page asks nothing");
}

/// At rest the window asks for nothing: ten idle minutes of the test clock start no read, ask for no
/// frame and tell the view nothing.
#[gpui::test]
fn ten_idle_minutes_ask_for_no_frame_and_read_nothing(cx: &mut TestAppContext) {
    let opened = open(cx);
    let (cx, settings, source) = (opened.cx, opened.settings, opened.source);
    cx.update(|window, cx| window.simulate_next_frame(cx));
    draw(cx);
    let asked = source.asked();
    let told = Rc::new(Cell::new(0));
    let counted = told.clone();
    cx.update(|_, cx| {
        cx.observe(&settings, move |_, _| counted.set(counted.get() + 1))
            .detach();
    });
    let mut frames = 0;
    for _ in 0..IDLE_SECONDS {
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        frames += cx.update(|window, cx| window.simulate_next_frame(cx));
    }
    assert_eq!(frames, 0, "frames requested at rest");
    assert_eq!(told.get(), 0, "the view was told to draw again at rest");
    assert_eq!(source.asked(), asked, "nothing is read at rest");
}

/// Ten minutes.
const IDLE_SECONDS: usize = 600;

#[gpui::test]
fn the_projects_are_read_when_the_window_opens_and_the_rest_when_first_shown(
    cx: &mut TestAppContext,
) {
    let mut opened = open(cx);
    assert_eq!(opened.source.asked(), ["projects"]);
    click(opened.cx, "settings-section-accounts");
    click(opened.cx, "settings-section-machine");
    assert!(is_there(opened.cx, "settings-machine-agent-0"));
    click(opened.cx, "settings-section-accounts");
    click(opened.cx, "settings-section-shortcuts");
    click(opened.cx, "settings-section-appearance");
    click(opened.cx, "settings-section-machine");
    assert_eq!(opened.source.asked(), ["projects", "accounts", "status"]);
    opened.rest(5);
    assert_eq!(opened.source.asked().len(), 3, "nothing is read at rest");
    click(opened.cx, "settings-machine-refresh");
    assert_eq!(
        opened.source.count("status"),
        2,
        "Refresh reads the machine again"
    );
}

#[gpui::test]
fn the_window_s_title_is_the_section_s_name_in_the_language_in_force(cx: &mut TestAppContext) {
    l10n::set_for_test(Language::English);
    let opened = open(cx);
    assert_eq!(opened.cx.window_title().as_deref(), Some("Projects"));
    click(opened.cx, "settings-section-machine");
    assert_eq!(opened.cx.window_title().as_deref(), Some("Machine"));
    l10n::set_for_test(Language::Italian);
    opened.settings.update(opened.cx, |_, cx| cx.notify());
    draw(opened.cx);
    assert_eq!(opened.cx.window_title().as_deref(), Some("Macchina"));
}

#[gpui::test]
fn what_cannot_be_read_is_said_once_and_read_again_only_by_retry(cx: &mut TestAppContext) {
    let source = Fake::new();
    source.is_unreadable.set(true);
    let mut opened = open_over(cx, source);
    assert!(is_there(opened.cx, "settings-unread"));
    opened.rest(5);
    assert_eq!(
        opened.source.asked(),
        ["projects"],
        "nothing is read again by itself"
    );
    opened.source.is_unreadable.set(false);
    click(opened.cx, "settings-retry");
    assert_eq!(opened.source.count("projects"), 2);
    assert!(is_there(opened.cx, "settings-project-shop-web"));
    assert!(!is_there(opened.cx, "settings-unread"));
}

#[gpui::test]
fn an_older_list_of_projects_never_replaces_a_newer_one(cx: &mut TestAppContext) {
    let source = Fake::new();
    source.is_holding.set(true);
    let opened = open_over(cx, source);
    // The second read is asked after a project was removed; its answer comes first.
    opened.source.projects.borrow_mut().remove(0);
    opened
        .settings
        .update(opened.cx, |settings, cx| settings.read(Read::Projects, cx));
    opened.source.release_newest();
    draw(opened.cx);
    assert!(!is_there(opened.cx, "settings-project-old-site"));
    opened.source.release();
    draw(opened.cx);
    assert!(
        !is_there(opened.cx, "settings-project-old-site"),
        "the older answer is dropped"
    );
    assert!(is_there(opened.cx, "settings-project-shop-web"));
}

#[gpui::test]
fn a_project_that_cannot_be_used_says_why_and_opens_nothing(cx: &mut TestAppContext) {
    let opened = open(cx);
    assert!(is_there(opened.cx, "settings-project-old-site-unusable"));
    click(opened.cx, "settings-project-old-site");
    assert!(!is_there(opened.cx, "settings-project-back"));
    assert_eq!(opened.source.asked(), ["projects"]);
}

#[gpui::test]
fn a_choice_is_sent_when_it_is_made_and_a_text_when_enter_is_pressed_or_its_field_is_left(
    cx: &mut TestAppContext,
) {
    let mut opened = open(cx);
    opened.open_project();
    assert_eq!(
        opened.source.asked()[1..],
        ["accounts", "branches shop-web"]
    );
    click(opened.cx, "settings-place-box");
    assert_eq!(opened.source.count("set shop-web --where box"), 1);
    click(opened.cx, "settings-place-box");
    assert_eq!(
        opened.source.count("set "),
        1,
        "the choice already made sends nothing"
    );
    click(opened.cx, "settings-run-mode-many");
    assert_eq!(opened.source.count("set shop-web --run-mode many"), 1);
    let run = opened.field(|form| &form.fields.run);
    opened.enter(&run, "npm start");
    assert_eq!(opened.source.count("set shop-web --run npm start"), 1);
    opened.cx.simulate_keystrokes("enter");
    draw(opened.cx);
    assert_eq!(
        opened.source.count("--run npm"),
        1,
        "Enter on what the project has sends nothing"
    );
    // A field that is left sends what it holds; an emptied command takes the command away.
    let setup = opened.field(|form| &form.fields.setup);
    opened.type_in(&setup, "");
    let archive = opened.field(|form| &form.fields.archive);
    opened.type_in(&archive, "make clean");
    assert_eq!(opened.source.count("set shop-web --setup "), 1);
    assert_eq!(
        opened.source.count("--archive"),
        0,
        "the field still has the keyboard"
    );
    opened.type_in(&setup, "");
    assert_eq!(opened.source.count("set shop-web --archive make clean"), 1);
    assert_eq!(
        opened.source.count("--setup"),
        1,
        "what the project has is not sent again"
    );
    let events = opened.events.borrow();
    let changes = events
        .iter()
        .filter(|event| **event == SettingsEvent::ProjectsChanged);
    assert_eq!(changes.count(), 5);
    assert_eq!(
        opened.source.count("projects"),
        6,
        "the list is read again after each change"
    );
}

#[gpui::test]
fn a_change_on_its_way_is_said_and_a_refusal_is_said_in_place_and_not_sent_again(
    cx: &mut TestAppContext,
) {
    let mut opened = open(cx);
    opened.open_project();
    opened.source.is_holding.set(true);
    opened.source.is_refusing.set(true);
    click(opened.cx, "settings-place-box");
    assert!(matches!(opened.sent(&Row::Place), Some(Sent::Applying(_))));
    assert!(!is_there(opened.cx, "settings-row-place-failed"));
    opened.source.release();
    draw(opened.cx);
    assert!(is_there(opened.cx, "settings-row-place-failed"));
    let place = opened
        .form()
        .read_with(opened.cx, |form, _| form.project.place);
    assert_eq!(place, Place::Mac, "what was refused is not shown as chosen");
    opened.rest(5);
    assert_eq!(
        opened.source.count("set "),
        1,
        "nothing is sent again by itself"
    );
    assert!(opened.events.borrow().is_empty());
}

#[gpui::test]
fn a_value_box_would_refuse_is_said_in_place_and_never_sent(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    opened.open_project();
    let ports = opened.field(|form| &form.fields.ports);
    for not_ports in ["0", "101", "many"] {
        opened.enter(&ports, not_ports);
        assert!(
            is_there(opened.cx, "settings-row-ports-failed"),
            "{not_ports}"
        );
    }
    let prefix = opened.field(|form| &form.fields.prefix);
    opened.enter(&prefix, "not a prefix/");
    assert!(is_there(opened.cx, "settings-row-branch-prefix-failed"));
    assert_eq!(opened.source.count("set "), 0);
    opened.enter(&ports, "3");
    assert_eq!(opened.source.count("set shop-web --ports 3"), 1);
    assert!(!is_there(opened.cx, "settings-row-ports-failed"));
}

#[gpui::test]
fn an_answer_for_an_earlier_sending_of_the_same_row_is_dropped(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    opened.open_project();
    let ports = opened.field(|form| &form.fields.ports);
    opened.source.is_holding.set(true);
    opened.enter(&ports, "3");
    opened.enter(&ports, "5");
    opened.source.release_newest();
    draw(opened.cx);
    assert_eq!(opened.sent(&Row::Ports), None);
    opened.source.release();
    draw(opened.cx);
    let has = opened
        .form()
        .read_with(opened.cx, |form, _| form.project.ports);
    assert_eq!(has, 5, "the earlier answer changes nothing");
}

#[gpui::test]
fn a_check_is_changed_removed_and_added(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    opened.open_project();
    let test = opened.field(|form| &form.fields.checks[0].1);
    opened.enter(&test, "npm run test:ci");
    assert_eq!(
        opened
            .source
            .count("set shop-web --check test=npm run test:ci"),
        1
    );
    let verb = opened.field(|form| &form.fields.new_verb);
    let command = opened.field(|form| &form.fields.new_command);
    opened.type_in(&verb, "test");
    opened.enter(&command, "npm test");
    assert!(
        is_there(opened.cx, "settings-row-new-check-failed"),
        "the project has that check"
    );
    opened.type_in(&verb, "lint");
    opened.type_in(&command, "npm run lint");
    click(opened.cx, "settings-check-add");
    assert_eq!(
        opened
            .source
            .count("set shop-web --check lint=npm run lint"),
        1
    );
    assert!(is_there(opened.cx, "settings-row-check-lint"));
    assert_eq!(
        verb.read_with(opened.cx, |verb, _| verb.value().to_string()),
        ""
    );
    opened.enter(&test, "");
    assert_eq!(opened.source.count("set shop-web --check test="), 2);
    assert!(
        !is_there(opened.cx, "settings-row-check-test"),
        "an emptied command takes the check away"
    );
}

#[gpui::test]
fn the_publishing_request_is_kept_by_the_app_when_its_field_is_left(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    let form = opened.open_project();
    let request = form.read_with(opened.cx, |form, _| form.fields.request.clone());
    request.update_in(opened.cx, |request, window, cx| request.focus(window, cx));
    draw(opened.cx);
    opened.cx.simulate_input("Push it and open a draft.");
    let run = opened.field(|form| &form.fields.run);
    run.update_in(opened.cx, |run, window, cx| run.focus(window, cx));
    draw(opened.cx);
    let changed = SettingsEvent::PublishRequestChanged {
        project: "shop-web".into(),
        request: Some("Push it and open a draft.".into()),
    };
    assert_eq!(*opened.events.borrow(), [changed]);
    assert_eq!(
        opened.source.count("set "),
        0,
        "box keeps no publishing request"
    );
}

#[gpui::test]
fn removing_is_asked_twice_and_a_refusal_is_said_in_place(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    opened.open_project();
    click(opened.cx, "settings-remove");
    assert_eq!(opened.source.count("rm "), 0, "the first press only asks");
    click(opened.cx, "settings-remove-cancel");
    assert!(is_there(opened.cx, "settings-remove"));
    opened.source.is_refusing.set(true);
    click(opened.cx, "settings-remove");
    click(opened.cx, "settings-remove-confirm");
    assert_eq!(opened.source.count("rm shop-web"), 1);
    assert!(is_there(opened.cx, "settings-row-remove-failed"));
    opened.rest(5);
    assert_eq!(opened.source.count("rm "), 1);
    opened.source.is_refusing.set(false);
    click(opened.cx, "settings-remove");
    click(opened.cx, "settings-remove-confirm");
    assert!(
        !is_there(opened.cx, "settings-project-back"),
        "the list is back"
    );
    assert!(!is_there(opened.cx, "settings-project-shop-web"));
    assert_eq!(*opened.events.borrow(), [SettingsEvent::ProjectsChanged]);
}

#[gpui::test]
fn adding_a_project_asks_for_a_folder_shows_what_was_recognised_and_adds_it(
    cx: &mut TestAppContext,
) {
    let mut opened = open(cx);
    click(opened.cx, "settings-add-project");
    assert_eq!(
        opened.source.asked()[1],
        "detect /home/demo/code/atlante/apps/site"
    );
    assert!(is_there(opened.cx, "settings-add-name"));
    assert!(is_there(opened.cx, "settings-add-check-0"));
    assert!(
        is_there(opened.cx, "settings-add-notes"),
        "what the service noted is said once, its words as the details"
    );
    opened.source.is_refusing.set(true);
    click(opened.cx, "settings-add-confirm");
    assert!(
        is_there(opened.cx, "settings-add-failed"),
        "a refusal is said in place"
    );
    opened.rest(5);
    assert_eq!(
        opened.source.count("add"),
        1,
        "and is not sent again by itself"
    );
    opened.source.is_refusing.set(false);
    click(opened.cx, "settings-add-confirm");
    assert_eq!(opened.source.count("add site"), 2);
    assert!(
        is_there(opened.cx, "settings-project-site"),
        "the list was read again"
    );
    assert_eq!(*opened.events.borrow(), [SettingsEvent::ProjectsChanged]);
}

#[gpui::test]
fn no_folder_chosen_and_a_folder_box_refuses_add_nothing(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    *opened.folder.borrow_mut() = None;
    click(opened.cx, "settings-add-project");
    assert!(
        is_there(opened.cx, "settings-add-project"),
        "the list is back"
    );
    assert_eq!(opened.source.asked(), ["projects"]);
    *opened.folder.borrow_mut() = Some(PathBuf::from("/home/demo/not-a-repository"));
    opened.source.is_refusing.set(true);
    click(opened.cx, "settings-add-project");
    assert!(is_there(opened.cx, "settings-add-failed"));
    assert!(!is_there(opened.cx, "settings-add-confirm"));
    opened.rest(5);
    assert_eq!(opened.source.count("detect"), 1);
    click(opened.cx, "settings-add-cancel");
    assert!(is_there(opened.cx, "settings-add-project"));
    assert!(opened.events.borrow().is_empty());
}

#[gpui::test]
fn only_an_account_of_the_boxes_that_is_not_signed_in_offers_to_sign_in(cx: &mut TestAppContext) {
    let opened = open(cx);
    click(opened.cx, "settings-section-accounts");
    for account in [
        "settings-account-box-personal",
        "settings-account-box-work",
        "settings-account-mac-personal",
    ] {
        assert!(is_there(opened.cx, account), "{account}");
    }
    let sign_in = bounds(opened.cx, "settings-sign-in-work");
    let work = bounds(opened.cx, "settings-account-box-work");
    assert!(
        work.contains(&sign_in.center()),
        "the offer is on the account that is signed out"
    );
    assert!(!is_there(opened.cx, "settings-sign-in-personal"));
    click(opened.cx, "settings-sign-in-work");
    let asked = SettingsEvent::SignInAsked {
        account: "work".into(),
    };
    assert_eq!(*opened.events.borrow(), [asked]);
    assert_eq!(
        opened.source.asked(),
        ["projects", "accounts"],
        "nothing is asked of box to sign in"
    );
}

#[gpui::test]
fn two_accounts_signed_out_have_a_sign_in_each_and_a_press_asks_for_that_one_only(
    cx: &mut TestAppContext,
) {
    let opened = open(cx);
    click(opened.cx, "settings-section-accounts");
    for (row, sign_in) in [
        ("settings-account-box-work", "settings-sign-in-work"),
        ("settings-account-box-studio", "settings-sign-in-studio"),
    ] {
        let (row_at, sign_in_at) = (bounds(opened.cx, row), bounds(opened.cx, sign_in));
        assert!(
            row_at.contains(&sign_in_at.center()),
            "{sign_in} is on {row}"
        );
    }
    click(opened.cx, "settings-sign-in-studio");
    let asked = SettingsEvent::SignInAsked {
        account: "studio".into(),
    };
    assert_eq!(*opened.events.borrow(), [asked]);
}

#[gpui::test]
fn the_theme_is_told_to_the_app_and_the_language_to_the_localisation_layer(
    cx: &mut TestAppContext,
) {
    let opened = open(cx);
    click(opened.cx, "settings-section-appearance");
    click(opened.cx, "settings-theme-dark");
    click(opened.cx, "settings-theme-dark");
    assert_eq!(
        *opened.events.borrow(),
        [SettingsEvent::ThemeChosen(ThemeChoice::Dark)]
    );
    click(opened.cx, "settings-language-it");
    assert_eq!(
        *opened.languages.borrow(),
        [LanguageChoice::Is(Language::Italian)]
    );
    assert_eq!(opened.source.asked(), ["projects"]);
}

#[gpui::test]
fn the_request_for_every_project_is_kept_by_the_app_when_its_field_is_left(
    cx: &mut TestAppContext,
) {
    let opened = open(cx);
    click(opened.cx, "settings-section-agent");
    let field = opened
        .settings
        .read_with(opened.cx, |settings, _| settings.default_request.clone());
    field.update_in(opened.cx, |field, window, cx| field.focus(window, cx));
    draw(opened.cx);
    opened.cx.simulate_input("Push and open a pull request.");
    click(opened.cx, "settings-section-appearance");
    click(opened.cx, "settings-section-agent");
    let kept = SettingsEvent::PublishDefaultChanged(Some("Push and open a pull request.".into()));
    assert_eq!(
        opened.events.borrow().as_slice(),
        std::slice::from_ref(&kept)
    );
    // Left again with the same text: nothing is said twice.
    field.update_in(opened.cx, |field, window, cx| field.focus(window, cx));
    draw(opened.cx);
    click(opened.cx, "settings-section-appearance");
    assert_eq!(
        opened.events.borrow().as_slice(),
        std::slice::from_ref(&kept)
    );
    // One control brings the app's own request back, once.
    click(opened.cx, "settings-section-agent");
    click(opened.cx, "settings-default-request-restore");
    click(opened.cx, "settings-default-request-restore");
    assert_eq!(
        *opened.events.borrow(),
        [kept, SettingsEvent::PublishDefaultChanged(None)]
    );
    assert_eq!(field.read_with(opened.cx, |field, _| field.value()), "");
    assert_eq!(
        opened.source.asked(),
        ["projects"],
        "the service keeps no publishing request"
    );
}

#[gpui::test]
fn each_notification_switch_tells_the_app_at_once(cx: &mut TestAppContext) {
    let opened = open(cx);
    click(opened.cx, "settings-section-notifications");
    click(opened.cx, "settings-notify-finished");
    click(opened.cx, "settings-notify-needs-you");
    click(opened.cx, "settings-notify-finished");
    let told = |needs_you, finished| {
        SettingsEvent::NotifyChanged(Notify {
            needs_you,
            finished,
        })
    };
    assert_eq!(
        *opened.events.borrow(),
        [told(true, false), told(false, false), told(false, true)]
    );
    assert_eq!(opened.source.asked(), ["projects"]);
}

#[gpui::test]
fn the_terminal_s_font_and_size_are_told_to_the_app_once_each_and_the_fonts_are_read_once(
    cx: &mut TestAppContext,
) {
    let opened = open(cx);
    let families = |cx: &mut VisualTestContext, settings: &Entity<Settings>| {
        settings.read_with(cx, |settings, _| settings.families.clone())
    };
    assert_eq!(families(opened.cx, &opened.settings), None, "not read yet");
    click(opened.cx, "settings-section-appearance");
    opened.cx.run_until_parked();
    let read = Some(vec!["Menlo".to_string(), "Monaco".to_string()]);
    assert_eq!(families(opened.cx, &opened.settings), read);
    assert!(is_there(opened.cx, "settings-terminal-font"));
    click(opened.cx, "settings-terminal-size-15");
    click(opened.cx, "settings-terminal-size-15");
    let chosen = |family: Option<&str>, size| {
        SettingsEvent::TerminalTextChosen(TerminalText {
            family: family.map(str::to_string),
            size,
        })
    };
    assert_eq!(*opened.events.borrow(), [chosen(None, Some(15))]);
    let pick = |cx: &mut VisualTestContext, family: Option<&str>| {
        let family = family.map(str::to_string);
        opened
            .settings
            .update(cx, |settings, cx| settings.set_terminal_family(family, cx));
    };
    pick(opened.cx, Some("Menlo"));
    pick(opened.cx, Some("Menlo"));
    pick(opened.cx, None);
    assert_eq!(
        opened.events.borrow()[1..],
        [chosen(Some("Menlo"), Some(15)), chosen(None, Some(15))]
    );
    assert_eq!(
        opened.source.asked(),
        ["projects"],
        "nothing of the service"
    );
}

#[gpui::test]
fn storage_says_the_folder_and_what_each_task_in_a_box_took_from_the_machine_s_one_read(
    cx: &mut TestAppContext,
) {
    let opened = open(cx);
    click(opened.cx, "settings-section-storage");
    assert!(is_there(opened.cx, "settings-storage-folder"));
    assert!(is_there(opened.cx, "settings-storage-task-0"));
    click(opened.cx, "settings-section-machine");
    click(opened.cx, "settings-section-storage");
    assert_eq!(
        opened.source.asked(),
        ["projects", "status"],
        "one read for both"
    );
    click(opened.cx, "settings-storage-refresh");
    assert_eq!(opened.source.count("status"), 2, "Refresh reads it again");
}

#[test]
fn storage_figures_read_as_a_person_reads_them_in_both_languages() {
    let disk = DiskUse {
        bytes: 1_288_490_188,
        entries: 12_400,
        limit_bytes: 8_589_934_592,
        entry_limit: 200_000,
        over_budget: false,
    };
    let said = |language, disk: &DiskUse| {
        l10n::set_for_test(language);
        super::sections::disk_said(disk)
    };
    assert_eq!(
        said(Language::English, &disk).as_ref(),
        "1.2 GB of 8 GB, 12,400 of 200,000 files"
    );
    assert_eq!(
        said(Language::Italian, &disk).as_ref(),
        "1,2 GB di 8 GB, 12.400 di 200.000 file"
    );
    let small = DiskUse {
        bytes: 52 * 1024 * 1024,
        ..disk
    };
    let said = said(Language::English, &small);
    assert!(said.starts_with("52 MB of 8 GB"), "{said}");
}

fn assert_near(found: Pixels, wanted: Pixels, what: &str) {
    let apart = (f32::from(found) - f32::from(wanted)).abs();
    assert!(apart <= NEAR, "{what}: {found:?}, wanted {wanted:?}");
}

#[test]
fn the_window_stands_on_the_grid_and_a_choice_moves_nothing() {
    real_text::run(|cx| {
        let mut opened = open(cx);
        let window = opened.cx.update(|window, _| window.viewport_size());
        let (projects, accounts, panel) = (
            bounds(opened.cx, "settings-section-projects"),
            bounds(opened.cx, "settings-section-accounts"),
            bounds(opened.cx, "settings-panel"),
        );
        assert!(
            projects.right() <= sizes::SECTIONS_WIDTH,
            "a section's row is in the list"
        );
        assert!(
            accounts.top() > projects.bottom(),
            "the rows stand in a column"
        );
        assert_near(
            panel.left(),
            sizes::SECTIONS_WIDTH,
            "the panel starts where the list ends",
        );
        assert_near(panel.right(), window.width, "and ends at the window's edge");
        opened.open_project();
        let rows = [
            "settings-row-base",
            "settings-row-place",
            "settings-row-branch-prefix",
            "settings-row-run",
            "settings-row-ports",
        ];
        let first = bounds(opened.cx, rows[0]);
        for row in rows {
            let row = bounds(opened.cx, row);
            assert_near(row.left(), first.left(), "the rows share their left edge");
            assert_near(row.right(), first.right(), "and their right edge");
            assert!(
                row.size.height >= sizes::SETTING_ROW_HEIGHT,
                "a row is at least a row tall"
            );
        }
        assert!(
            first.left() >= panel.left() && first.right() <= panel.right(),
            "the form stands in the panel"
        );
        let before = (
            bounds(opened.cx, "settings-place-mac"),
            bounds(opened.cx, "settings-place-box"),
            bounds(opened.cx, "settings-row-run"),
        );
        opened.source.is_holding.set(true);
        click(opened.cx, "settings-place-box");
        let applying = (
            bounds(opened.cx, "settings-place-mac"),
            bounds(opened.cx, "settings-place-box"),
            bounds(opened.cx, "settings-row-run"),
        );
        assert_eq!(applying, before, "a change on its way moves nothing");
        opened.source.is_holding.set(false);
        opened.source.release();
        draw(opened.cx);
        let chosen = (
            bounds(opened.cx, "settings-place-mac"),
            bounds(opened.cx, "settings-place-box"),
            bounds(opened.cx, "settings-row-run"),
        );
        assert_eq!(chosen, before, "the choice made moves nothing");
    });
}

#[test]
fn every_section_stands_in_the_same_panel_and_choosing_one_moves_nothing() {
    real_text::run(|cx| {
        let opened = open(cx);
        let sections = [
            "settings-section-projects",
            "settings-section-accounts",
            "settings-section-agent",
            "settings-section-appearance",
            "settings-section-notifications",
            "settings-section-machine",
            "settings-section-storage",
            "settings-section-shortcuts",
            "settings-section-about",
        ];
        let places = |cx: &mut VisualTestContext| -> Vec<Bounds<Pixels>> {
            let mut places = Vec::new();
            for section in sections {
                places.push(bounds(cx, section));
            }
            places.push(bounds(cx, "settings-panel"));
            places
        };
        let first = places(opened.cx);
        for pair in first[..sections.len()].windows(2) {
            assert_near(pair[0].left(), pair[1].left(), "the sections share an edge");
            assert!(
                pair[0].bottom() <= pair[1].top(),
                "and stand one under the other"
            );
        }
        for section in sections {
            click(opened.cx, section);
            assert_eq!(places(opened.cx), first, "{section} moved the window");
        }
    });
}

#[test]
fn the_controls_of_every_section_end_on_one_line_and_stay_inside_their_rows() {
    real_text::run(|cx| {
        let opened = open(cx);
        let inside = |cx: &mut VisualTestContext, row: &'static str, controls: &[&'static str]| {
            let row = bounds(cx, row);
            for control in controls {
                let control = bounds(cx, control);
                assert!(
                    control.left() >= row.left() && control.right() <= row.right(),
                    "{control:?} leaves its row {row:?}"
                );
                assert!(control.top() >= row.top() && control.bottom() <= row.bottom());
            }
            row
        };
        click(opened.cx, "settings-section-appearance");
        let themes = [
            "settings-theme-system",
            "settings-theme-light",
            "settings-theme-dark",
        ];
        let languages = [
            "settings-language-automatic",
            "settings-language-en",
            "settings-language-it",
        ];
        let theme = inside(opened.cx, "settings-row-theme", &themes);
        let language = inside(opened.cx, "settings-row-language", &languages);
        assert_near(
            theme.right(),
            language.right(),
            "the controls end on one line",
        );
        let before = bounds(opened.cx, "settings-language-it");
        click(opened.cx, "settings-theme-light");
        assert_eq!(
            bounds(opened.cx, "settings-language-it"),
            before,
            "a choice moves nothing"
        );
        let sizes_of_text = ["settings-terminal-size-11", "settings-terminal-size-16"];
        let font = inside(
            opened.cx,
            "settings-row-terminal-font",
            &["settings-terminal-font"],
        );
        let size = inside(opened.cx, "settings-row-terminal-size", &sizes_of_text);
        assert_near(
            font.right(),
            theme.right(),
            "the terminal's controls end there",
        );
        assert_near(size.right(), theme.right(), "and its sizes");
        assert_near(
            bounds(opened.cx, "settings-terminal-font").right(),
            bounds(opened.cx, "settings-theme-dark").right() + theme::TAB_GAP,
            "the font's menu ends where the track of the choices ends",
        );
        let before = (bounds(opened.cx, "settings-terminal-size-11"), font);
        click(opened.cx, "settings-terminal-size-16");
        let after = (
            bounds(opened.cx, "settings-terminal-size-11"),
            bounds(opened.cx, "settings-row-terminal-font"),
        );
        assert_eq!(after, before, "a size chosen moves nothing");

        click(opened.cx, "settings-section-notifications");
        let switches = [
            ("settings-notify-needs-you-row", "settings-notify-needs-you"),
            ("settings-notify-finished-row", "settings-notify-finished"),
        ];
        let ends: Vec<Pixels> = switches
            .iter()
            .map(|(row, switch)| {
                inside(opened.cx, row, &[switch]);
                bounds(opened.cx, switch).right()
            })
            .collect();
        assert_near(ends[0], ends[1], "the switches end on one line");
        let before = bounds(opened.cx, "settings-notify-needs-you");
        click(opened.cx, "settings-notify-finished");
        assert_eq!(
            bounds(opened.cx, "settings-notify-needs-you"),
            before,
            "a switch moves nothing"
        );

        click(opened.cx, "settings-section-agent");
        let request = bounds(opened.cx, "settings-row-default-request");
        let panel = bounds(opened.cx, "settings-panel");
        assert!(
            request.right() <= panel.right(),
            "the field stands in the panel"
        );
        click(opened.cx, "settings-section-storage");
        inside(
            opened.cx,
            "settings-storage-folder",
            &["settings-storage-folder-path"],
        );
        for row in ["settings-storage-folder", "settings-storage-task-0"] {
            let row = bounds(opened.cx, row);
            assert_near(
                row.right(),
                request.right(),
                "Storage ends on the same line",
            );
        }
        click(opened.cx, "settings-section-shortcuts");
        let shortcut = bounds(opened.cx, "settings-shortcut-0");
        assert_near(
            shortcut.right(),
            request.right(),
            "every page ends on one line",
        );
    });
}

/// The words typed in the search field, with the keyboard as the person types them.
fn search_for(cx: &mut VisualTestContext, words: &str) {
    click(cx, "settings-search");
    cx.simulate_input(words);
    draw(cx);
}

/// The search field emptied with Escape.
fn empty_search(cx: &mut VisualTestContext) {
    cx.simulate_keystrokes("escape");
    draw(cx);
}

const SECTION_ROWS: [&str; 9] = [
    "settings-section-projects",
    "settings-section-accounts",
    "settings-section-agent",
    "settings-section-appearance",
    "settings-section-notifications",
    "settings-section-machine",
    "settings-section-storage",
    "settings-section-shortcuts",
    "settings-section-about",
];

/// The sections the list shows now, in its order.
fn listed(cx: &mut VisualTestContext) -> Vec<&'static str> {
    SECTION_ROWS
        .into_iter()
        .filter(|row| is_there(cx, row))
        .collect()
}

#[gpui::test]
fn a_search_lists_only_the_sections_that_hold_it_and_shows_only_its_rows(cx: &mut TestAppContext) {
    l10n::set_for_test(Language::English);
    let opened = open(cx);
    assert_eq!(
        listed(opened.cx),
        SECTION_ROWS,
        "every section before a search"
    );
    search_for(opened.cx, "theme");
    assert_eq!(listed(opened.cx), ["settings-section-appearance"]);
    assert_eq!(opened.cx.window_title().as_deref(), Some("Appearance"));
    assert!(is_there(opened.cx, "settings-row-theme"));
    assert!(
        !is_there(opened.cx, "settings-row-language"),
        "a row the search does not find is not shown"
    );
    empty_search(opened.cx);
    search_for(opened.cx, " DARK MODE");
    assert!(
        is_there(opened.cx, "settings-row-theme"),
        "a word the title does not have finds the row, whatever its case"
    );
}

#[gpui::test]
fn a_section_with_nothing_the_search_finds_is_not_listed_and_nothing_found_is_said(
    cx: &mut TestAppContext,
) {
    l10n::set_for_test(Language::English);
    let opened = open(cx);
    search_for(opened.cx, "notification");
    assert_eq!(listed(opened.cx), ["settings-section-notifications"]);
    assert!(is_there(opened.cx, "settings-notify-needs-you"));
    empty_search(opened.cx);
    search_for(opened.cx, "no setting is called this");
    assert!(listed(opened.cx).is_empty());
    assert!(is_there(opened.cx, "settings-no-results"));
}

#[gpui::test]
fn emptying_the_search_shows_again_the_section_that_was_open(cx: &mut TestAppContext) {
    l10n::set_for_test(Language::English);
    let opened = open(cx);
    click(opened.cx, "settings-section-notifications");
    search_for(opened.cx, "terminal");
    assert_eq!(
        opened.cx.window_title().as_deref(),
        Some("Appearance"),
        "the section that holds it is on screen"
    );
    empty_search(opened.cx);
    assert_eq!(listed(opened.cx), SECTION_ROWS);
    assert_eq!(opened.cx.window_title().as_deref(), Some("Notifications"));
    assert!(is_there(opened.cx, "settings-notify-finished"));
}

#[gpui::test]
fn a_section_shown_by_a_search_is_read_the_first_time_as_one_chosen_is(cx: &mut TestAppContext) {
    l10n::set_for_test(Language::English);
    let opened = open(cx);
    search_for(opened.cx, "runner");
    assert_eq!(opened.cx.window_title().as_deref(), Some("Machine"));
    assert_eq!(opened.source.asked(), ["projects", "status"]);
}

#[gpui::test]
fn every_setting_has_a_sentence_that_says_what_it_changes_in_both_languages(
    cx: &mut TestAppContext,
) {
    let opened = open(cx);
    // What these list is read when they are first shown.
    for row in [
        "settings-section-accounts",
        "settings-section-machine",
        "settings-section-appearance",
    ] {
        click(opened.cx, row);
    }
    for language in [Language::English, Language::Italian] {
        l10n::set_for_test(language);
        opened.settings.update(opened.cx, |settings, cx| {
            for section in super::Section::ALL {
                let groups = settings.groups(section, cx);
                let rows = groups.iter().flat_map(|group| group.rows.iter());
                let mut count = 0;
                for row in rows.filter(|row| row.is_setting()) {
                    let description = row.description().map(|text| text.trim().len());
                    assert!(
                        description.is_some_and(|length| length > 10),
                        "{section:?} in {language:?}: {:?}",
                        row.description()
                    );
                    count += 1;
                }
                // The brand's page and the menu bar's table hold no setting.
                let is_list = matches!(section, super::Section::About | super::Section::Shortcuts);
                assert!(count > 0 || is_list, "{section:?} has settings");
            }
        });
    }
}

#[gpui::test]
fn the_window_opens_with_the_caret_nowhere_and_a_click_or_typing_in_the_search_draws_no_ring(
    cx: &mut TestAppContext,
) {
    let opened = open(cx);
    opened.cx.update(|window, cx| {
        assert!(window.focused(cx).is_none(), "nothing has the caret");
    });
    click(opened.cx, "settings-search");
    opened.cx.update(|window, cx| {
        assert!(
            window.focused(cx).is_some(),
            "the click gave the field the caret"
        );
    });
    assert_eq!(crate::ui::focus_tests::rings(opened.cx), 0, "with no ring");
    opened.cx.simulate_input("theme");
    draw(opened.cx);
    assert_eq!(
        crate::ui::focus_tests::rings(opened.cx),
        0,
        "typing draws none"
    );
}

#[gpui::test]
fn a_click_on_a_switch_draws_no_ring(cx: &mut TestAppContext) {
    let opened = open(cx);
    click(opened.cx, "settings-section-notifications");
    click(opened.cx, "settings-notify-finished");
    assert_eq!(crate::ui::focus_tests::rings(opened.cx), 0);
}

#[gpui::test]
fn a_search_finds_a_shortcut_by_its_command_or_its_menu(cx: &mut TestAppContext) {
    l10n::set_for_test(Language::English);
    let opened = open(cx);
    for words in ["new task", "file", "keyboard"] {
        search_for(opened.cx, words);
        assert!(
            listed(opened.cx).contains(&"settings-section-shortcuts"),
            "{words}"
        );
        click(opened.cx, "settings-section-shortcuts");
        assert!(is_there(opened.cx, "settings-shortcut-0"), "{words}");
        empty_search(opened.cx);
    }
}

#[gpui::test]
fn tab_gives_the_search_field_the_caret_first(cx: &mut TestAppContext) {
    // As the app binds it: Tab before anything has the focus moves to the first control.
    cx.update(crate::ui::keyboard::install);
    let opened = open(cx);
    opened.cx.simulate_keystrokes("tab");
    draw(opened.cx);
    let search = opened
        .settings
        .read_with(opened.cx, |settings, _| settings.search.clone());
    opened.cx.update(|window, cx| {
        let handle = gpui::Focusable::focus_handle(search.read(cx), cx);
        assert!(handle.is_focused(window), "the first stop is the search");
    });
    opened.cx.simulate_input("theme");
    draw(opened.cx);
    assert_eq!(listed(opened.cx), ["settings-section-appearance"]);
}

/// A service that offers the choice of runner, with the engine's programs as `restate_server` and
/// `node` say.
fn offering_the_engine(restate_server: bool, node: bool) -> Rc<Fake> {
    let source = Fake::new();
    *source.check_runner.borrow_mut() = Some(CheckRunner {
        runner: RunnerKind::Bash,
        engine: Some(EnginePrerequisites {
            restate_server,
            node,
            setup: None,
        }),
    });
    source
}

#[cfg(feature = "restate-engine")]
/// The sentence of the first row of "Checks" in "Machine".
fn first_check_row(opened: &mut Opened) -> String {
    opened.settings.update(opened.cx, |settings, cx| {
        let groups = settings.groups(super::Section::Machine, cx);
        let row = &groups[0].rows[0];
        row.description()
            .map(|text| text.to_string())
            .unwrap_or_default()
    })
}

#[cfg(feature = "restate-engine")]
#[gpui::test]
fn the_engine_switch_is_off_by_default_and_a_press_writes_the_runner_and_reads_again(
    cx: &mut TestAppContext,
) {
    l10n::set_for_test(Language::English);
    let mut opened = open_over(cx, offering_the_engine(true, true));
    click(opened.cx, "settings-section-machine");
    assert!(is_there(opened.cx, "settings-machine-engine"));
    assert_eq!(
        first_check_row(&mut opened),
        l10n::tr(l10n::Key("settings.machine.engine.description"))
    );
    click(opened.cx, "settings-machine-engine");
    assert_eq!(
        opened.source.asked(),
        ["projects", "status", "runner engine", "status"]
    );
    let is_on = opened.settings.read_with(opened.cx, |settings, _| {
        let status = settings.machine.as_ref().and_then(|loaded| loaded.ready());
        status
            .and_then(|status| status.check_runner.clone())
            .map(|c| c.runner)
    });
    assert_eq!(is_on, Some(RunnerKind::Engine));
    click(opened.cx, "settings-machine-engine");
    assert_eq!(
        opened.source.count("runner bash"),
        1,
        "a second press turns it off"
    );
}

#[cfg(feature = "restate-engine")]
#[gpui::test]
fn the_engine_on_without_its_programs_says_what_to_install_in_one_line(cx: &mut TestAppContext) {
    l10n::set_for_test(Language::English);
    let mut opened = open_over(cx, offering_the_engine(false, true));
    click(opened.cx, "settings-section-machine");
    let off = first_check_row(&mut opened);
    assert!(
        !off.contains("brew install"),
        "off, nothing to install: {off}"
    );
    click(opened.cx, "settings-machine-engine");
    let on = first_check_row(&mut opened);
    assert_eq!(
        on,
        l10n::tr(l10n::Key("settings.machine.engine.install.restate"))
    );
    assert!(on.contains("brew install restatedev/tap/restate-server"));
    l10n::set_for_test(Language::Italian);
    let italian = first_check_row(&mut opened);
    assert!(
        italian.starts_with("Per eseguire le verifiche installa"),
        "{italian}"
    );
}

#[cfg(feature = "restate-engine")]
#[gpui::test]
fn a_refused_change_of_runner_is_said_in_the_row_and_the_switch_stays(cx: &mut TestAppContext) {
    l10n::set_for_test(Language::English);
    let source = offering_the_engine(true, true);
    source.is_refusing.set(true);
    let mut opened = open_over(cx, source);
    click(opened.cx, "settings-section-machine");
    click(opened.cx, "settings-machine-engine");
    assert_eq!(first_check_row(&mut opened), "Not changed: refused");
    let runner = opened.settings.read_with(opened.cx, |settings, _| {
        let status = settings.machine.as_ref().and_then(|loaded| loaded.ready());
        status
            .and_then(|status| status.check_runner.clone())
            .map(|c| c.runner)
    });
    assert_eq!(runner, Some(RunnerKind::Bash));
}

#[gpui::test]
fn a_service_with_one_runner_offers_no_switch(cx: &mut TestAppContext) {
    let opened = open(cx);
    click(opened.cx, "settings-section-machine");
    assert!(is_there(opened.cx, "settings-machine-runner"));
    assert!(!is_there(opened.cx, "settings-machine-engine"));
}

#[cfg(not(feature = "restate-engine"))]
#[gpui::test]
fn a_service_that_offers_the_engine_still_shows_no_switch_in_the_default_build(
    cx: &mut TestAppContext,
) {
    let opened = open_over(cx, offering_the_engine(true, true));
    click(opened.cx, "settings-section-machine");
    assert!(is_there(opened.cx, "settings-machine-runner"));
    assert!(!is_there(opened.cx, "settings-machine-engine"));
    assert_eq!(opened.source.asked(), ["projects", "status"]);
}
