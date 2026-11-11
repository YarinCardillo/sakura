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
use sakura_core::models::{Account, BoxStatus, DiskUse, Place};
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
            account("personal", Place::Mac, true),
        ];
        self.read("accounts", accounts)
    }

    fn status(&self) -> Reading<BoxStatus> {
        self.read("status", serde_json::from_str(STATUS).expect("the fixture"))
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

fn click(cx: &mut VisualTestContext, selector: &'static str) {
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
    let sign_in = bounds(opened.cx, "settings-sign-in");
    let work = bounds(opened.cx, "settings-account-box-work");
    assert!(
        work.contains(&sign_in.center()),
        "the one offer is on the account that is signed out"
    );
    click(opened.cx, "settings-sign-in");
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
        let (projects, panel) = (
            bounds(opened.cx, "settings-section-projects"),
            bounds(opened.cx, "settings-panel"),
        );
        assert_near(projects.size.height, theme::ROW_HEIGHT, "a section's row");
        assert_near(projects.left(), theme::PANEL_INSET, "the list's inset");
        assert_near(
            panel.left(),
            sizes::SECTIONS_WIDTH,
            "the panel starts where the list ends",
        );
        assert_near(
            panel.right(),
            window.width - theme::PANEL_INSET,
            "the panel's inset",
        );
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
            first.size.width <= sizes::PAGE_MAX_WIDTH,
            "the page is one column"
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
            theme.left(),
            language.left(),
            "the rows share their left edge",
        );
        assert_near(theme.right(), language.right(), "and their right edge");
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
        assert_near(font.left(), theme.left(), "the terminal's rows start there");
        assert_near(size.right(), theme.right(), "and end there");
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
        assert!(request.size.width <= sizes::PAGE_MAX_WIDTH, "one column");
        click(opened.cx, "settings-section-storage");
        inside(
            opened.cx,
            "settings-storage-folder",
            &["settings-storage-folder-path"],
        );
        for row in ["settings-storage-folder", "settings-storage-task-0"] {
            let row = bounds(opened.cx, row);
            assert_near(
                row.left(),
                request.left(),
                "Storage starts on the same line",
            );
            assert_near(row.right(), request.right(), "and ends on it");
        }
        click(opened.cx, "settings-section-shortcuts");
        let shortcut = bounds(opened.cx, "settings-shortcut-0");
        assert_near(
            shortcut.left(),
            request.left(),
            "every page starts on one line",
        );
        assert_near(shortcut.right(), request.right(), "and ends on one");
    });
}
