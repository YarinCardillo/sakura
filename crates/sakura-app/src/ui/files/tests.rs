//! The files pane driven on GPUI's test platform over a source that counts what is read: the tree read
//! one level at a time, the file that follows the agent and stops when the person takes the view, the
//! file read again when the task changes, and where things stand on the layout grid in both languages
//! at the widths the right pane has.

use super::file_view::FileView;
use super::source::{Reading, Source};
use super::{FilesEvent, FilesPane};
use crate::l10n::Key;
use crate::theme::{self, Appearance, Theme};
use crate::ui::real_text;
use gpui::{
    div, point, px, AppContext as _, Bounds, Context, Entity, InteractiveElement as _, IntoElement,
    Modifiers, ParentElement as _, Pixels, Render, ScrollDelta, ScrollEvent, Styled as _,
    TestAppContext, VisualTestContext, Window,
};
use sakura_core::error::{BridgeError, BridgeResult};
use sakura_core::files::{Entry, EntryKind, FileBody, Level, Matches, Reference};
use sakura_core::patch::Patch;
use sakura_core::said::Said;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// The folder the pretend agent works in.
const WORKSPACE: &str = "/work/demo";
/// The files of the big folder: as many as `box` lists for one level, far more than a pane shows.
const MANY: usize = 2000;
/// The pane's widths in the desk: at first launch, and the narrowest it gets.
const WIDTHS: [f32; 2] = [451., 212.];
const WINDOW_HEIGHT: f32 = 700.;
/// How far two edges may stand apart and still be on one line.
const NEAR: f32 = 0.5;

/// What a test noted of the views' events, in order.
type Noted<T> = Rc<RefCell<Vec<T>>>;

/// A source that answers from memory and notes every read: `tree <dir>`, `file <path>`, `search <text>`.
#[derive(Default)]
pub(super) struct Fake {
    levels: RefCell<HashMap<String, Level>>,
    files: RefCell<HashMap<String, FileBody>>,
    read: RefCell<Vec<String>>,
    pub(super) is_failing: Cell<bool>,
    /// While set, an answer waits until the test lets it go ([`Fake::release`]).
    pub(super) is_holding: Cell<bool>,
    /// The answers that wait, in the order they were requested.
    held: RefCell<Vec<Box<dyn FnOnce()>>>,
}

impl Fake {
    fn answer<T: Send + 'static>(&self, call: String, found: Option<T>) -> Reading<T> {
        self.read.borrow_mut().push(call);
        let failed = || BridgeError::failed(Said::new("error.internal"));
        let answer: BridgeResult<T> = found.filter(|_| !self.is_failing.get()).ok_or_else(failed);
        if !self.is_holding.get() {
            return Box::pin(std::future::ready(answer));
        }
        let (send, receive) = tokio::sync::oneshot::channel();
        let let_go = move || drop(send.send(answer));
        self.held.borrow_mut().push(Box::new(let_go));
        Box::pin(async move { receive.await.unwrap_or_else(|_| Err(failed())) })
    }

    /// Whether an answer waits to be let go.
    pub(super) fn is_waiting(&self) -> bool {
        !self.held.borrow().is_empty()
    }

    /// Lets the answer requested last go, before the ones requested earlier.
    fn release_newest(&self) {
        let let_go = self.held.borrow_mut().pop().expect("an answer waits");
        let_go();
    }

    /// Lets the answer that has waited longest go; what was asked after it keeps waiting.
    pub(super) fn release(&self) {
        let let_go = self.held.borrow_mut().remove(0);
        let_go();
    }

    pub(super) fn read(&self) -> Vec<String> {
        self.read.borrow().clone()
    }

    pub(super) fn set_file(&self, path: &str, text: &str) {
        let body = FileBody::Text(text.to_string());
        self.files.borrow_mut().insert(path.to_string(), body);
    }
}

impl Source for Fake {
    fn level(&self, dir: &str) -> Reading<Level> {
        let level = self.levels.borrow().get(dir).cloned();
        self.answer(format!("tree {dir}"), level)
    }

    fn file(&self, path: &str) -> Reading<FileBody> {
        let body = self.files.borrow().get(path).cloned();
        self.answer(format!("file {path}"), body)
    }

    fn search(&self, text: &str) -> Reading<Matches> {
        let files = self.files.borrow();
        let mut matches: Vec<String> = files
            .keys()
            .filter(|path| path.contains(text))
            .cloned()
            .collect();
        matches.sort();
        let found = Matches {
            query: text.to_string(),
            matches,
            truncated: false,
        };
        self.answer(format!("search {text}"), Some(found))
    }
}

fn entry(name: &str, kind: EntryKind, figures: Option<(u32, u32)>) -> Entry {
    Entry {
        name: name.to_string(),
        kind,
        changed: figures.is_some(),
        added: figures.map(|(added, _)| added),
        removed: figures.map(|(_, removed)| removed),
    }
}

fn level(path: &str, entries: Vec<Entry>) -> (String, Level) {
    let level = Level {
        path: path.to_string(),
        entries,
        truncated: false,
    };
    (path.to_string(), level)
}

/// `count` numbered lines, each `long` when given so that it wraps.
fn lines(count: usize, long: bool) -> String {
    let tail = if long {
        " // ".to_string() + &"wrap ".repeat(40)
    } else {
        String::new()
    };
    (1..=count)
        .map(|line| format!("const line{line} = {line};{tail}\n"))
        .collect()
}

/// A task with a touched folder, an untouched one, a folder of thousands of files and files of every
/// kind the view tells apart.
pub(super) fn task() -> Rc<Fake> {
    let dir = |name: &str, touched: bool| entry(name, EntryKind::Dir, touched.then_some((0, 0)));
    let file = |name: &str, figures| entry(name, EntryKind::File, figures);
    let many = (0..MANY).map(|index| file(&format!("file-{index:04}.ts"), None));
    let levels = [
        level(
            "",
            vec![
                dir("generated", false),
                dir("src", true),
                dir("tests", false),
                file("README.md", None),
                file("big.bin", None),
                file("logo.png", None),
                file("package.json", Some((2, 1))),
            ],
        ),
        level(
            "src",
            vec![
                dir("app", true),
                file("lib.rs", Some((12, 3))),
                entry("old.rs", EntryKind::Deleted, Some((0, 40))),
            ],
        ),
        level(
            "src/app",
            vec![file("folders.ts", Some((4, 3))), file("sort.ts", None)],
        ),
        level("generated", many.collect()),
        level("tests", vec![file("folders.spec.ts", None)]),
    ];
    let fake = Fake::default();
    fake.levels.borrow_mut().extend(levels);
    fake.set_file("src/app/folders.ts", &lines(200, false));
    fake.set_file("src/app/sort.ts", &lines(300, true));
    fake.set_file("src/lib.rs", &lines(120, false));
    let spec = format!("// The spec of the folders.\n{}", lines(80, false));
    fake.set_file("tests/folders.spec.ts", &spec);
    fake.set_file("README.md", "# Folders\n");
    let mut files = fake.files.borrow_mut();
    files.insert("logo.png".into(), FileBody::Binary);
    files.insert("big.bin".into(), FileBody::TooLarge);
    drop(files);
    Rc::new(fake)
}

/// The pane in a column as wide as the desk's right pane, with the status bar's control under it.
struct Host {
    pane: Entity<FilesPane>,
    width: Pixels,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = div()
            .debug_selector(|| "files-pane".into())
            .w(self.width)
            .h(px(WINDOW_HEIGHT) - theme::STATUS_BAR_HEIGHT)
            .child(self.pane.clone());
        div()
            .size_full()
            .child(pane)
            .child(follow_control(&self.pane, cx))
    }
}

/// The status bar's control in the tests' window: a press stops following, or follows again.
fn follow_control(pane: &Entity<FilesPane>, cx: &gpui::App) -> impl IntoElement {
    let (is_on, pane) = (pane.read(cx).is_following(), pane.clone());
    use gpui::StatefulInteractiveElement as _;
    div()
        .id("files-follow")
        .debug_selector(|| "files-follow".into())
        .size(theme::STATUS_CONTROL_HEIGHT)
        .on_click(move |_, window, cx| {
            pane.update(cx, |pane, cx| pane.set_following(!is_on, window, cx));
        })
}

struct Opened<'a> {
    cx: &'a mut VisualTestContext,
    host: Entity<Host>,
    pane: Entity<FilesPane>,
    source: Rc<Fake>,
    events: Noted<FilesEvent>,
}

pub(super) fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // What a frame started (a level's answer, a jump's second pass) is taken in by the next.
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn open(cx: &mut TestAppContext) -> Opened<'_> {
    open_over(cx, task())
}

/// The pane over `source`, drawn once its first reads were answered.
fn open_over(cx: &mut TestAppContext, source: Rc<Fake>) -> Opened<'_> {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
    });
    let events: Noted<FilesEvent> = Rc::default();
    let noted = Rc::clone(&events);
    let files: Rc<dyn Source> = source.clone();
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let host = cx.new(|cx| {
            let pane = cx.new(|cx| FilesPane::new(files, WORKSPACE, window, cx));
            cx.subscribe(&pane, move |_, _, event: &FilesEvent, _| {
                noted.borrow_mut().push(event.clone());
            })
            .detach();
            Host {
                pane,
                width: px(WIDTHS[0]),
            }
        });
        *kept.borrow_mut() = Some(host.clone());
        gpui_component::Root::new(host, window, cx)
    });
    let host: Entity<Host> = slot.borrow_mut().take().expect("the host was built");
    let pane = host.read_with(cx, |host, _| host.pane.clone());
    cx.update(|window, _| window.activate_window());
    draw(cx);
    Opened {
        cx,
        host,
        pane,
        source,
        events,
    }
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"))
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let centre = bounds(cx, selector).center();
    cx.simulate_click(centre, Modifiers::none());
    draw(cx);
}

fn assert_near(found: Pixels, wanted: Pixels, what: &str) {
    let apart = (f32::from(found) - f32::from(wanted)).abs();
    assert!(apart <= NEAR, "{what}: {found:?}, wanted {wanted:?}");
}

impl Opened<'_> {
    fn agent_at(&mut self, path: &str, line: u32) {
        let location = format!("{WORKSPACE}/{path}");
        self.pane.update_in(self.cx, |pane, window, cx| {
            pane.show_location(&location, Some(line), window, cx);
        });
        draw(self.cx);
    }

    fn open_path(&mut self) -> Option<String> {
        let pane = self.pane.clone();
        self.cx
            .read(|cx| pane.read(cx).open_path(cx).map(str::to_string))
    }

    fn file(&mut self) -> Entity<FileView> {
        self.pane.read_with(self.cx, |pane, _| pane.file_view())
    }

    /// How far down the open file is scrolled.
    fn scrolled(&mut self) -> Pixels {
        let file = self.file();
        self.cx.read(|cx| {
            let editor = file.read(cx).editor();
            -editor.read(cx).scroll_offset().y
        })
    }

    fn text(&mut self) -> String {
        let file = self.file();
        self.cx.read(|cx| {
            let editor = file.read(cx).editor();
            editor.read(cx).value().to_string()
        })
    }

    fn events(&self) -> Vec<FilesEvent> {
        self.events.borrow().clone()
    }

    fn set_width(&mut self, width: f32) {
        self.host.update(self.cx, |host, cx| {
            host.width = px(width);
            cx.notify();
        });
        draw(self.cx);
    }
}

#[gpui::test]
fn the_root_is_read_at_once_and_a_folder_only_when_it_is_opened(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    assert_eq!(opened.source.read(), ["tree "]);
    assert!(opened.cx.debug_bounds("files-row-src").is_some());
    assert!(opened.cx.debug_bounds("files-row-src/lib.rs").is_none());
    click(opened.cx, "files-row-src");
    assert_eq!(opened.source.read(), ["tree ", "tree src"]);
    assert!(opened.cx.debug_bounds("files-row-src/lib.rs").is_some());
    click(opened.cx, "files-row-src");
    click(opened.cx, "files-row-src");
    assert_eq!(
        opened.source.read().len(),
        2,
        "closed and opened again: read once"
    );
    assert_eq!(opened.open_path(), None, "a folder opens no file");
}

#[gpui::test]
fn a_folder_of_thousands_of_files_draws_the_rows_on_screen_and_reads_one_level(
    cx: &mut TestAppContext,
) {
    let opened = open(cx);
    click(opened.cx, "files-row-generated");
    assert_eq!(opened.source.read(), ["tree ", "tree generated"]);
    assert!(opened
        .cx
        .debug_bounds("files-row-generated/file-0000.ts")
        .is_some());
    let last = format!("files-row-generated/file-{:04}.ts", MANY - 1);
    let shown = opened.cx.debug_bounds(Box::leak(last.into_boxed_str()));
    assert!(shown.is_none(), "a row far below is not drawn");
    let wheel = ScrollEvent {
        position: bounds(opened.cx, "files-tree-area").center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-20_000.))),
        ..ScrollEvent::default()
    };
    opened.cx.simulate_event(wheel);
    draw(opened.cx);
    assert!(opened
        .cx
        .debug_bounds("files-row-generated/file-0000.ts")
        .is_none());
    assert_eq!(opened.source.read().len(), 2, "scrolling reads nothing");
}

#[gpui::test]
fn following_opens_each_place_of_the_agent_across_three_files(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    assert!(opened
        .pane
        .read_with(opened.cx, |pane, _| pane.is_following()));
    opened.agent_at("src/app/folders.ts", 150);
    assert_eq!(opened.open_path().as_deref(), Some("src/app/folders.ts"));
    let line_height = px(theme::DIFF_LINE_HEIGHT);
    assert_near(
        opened.scrolled(),
        line_height * 146.,
        "three rows over line 150",
    );
    opened.agent_at("src/lib.rs", 100);
    assert_eq!(opened.open_path().as_deref(), Some("src/lib.rs"));
    assert_near(
        opened.scrolled(),
        line_height * 96.,
        "line 100 of the second file",
    );
    opened.agent_at("tests/folders.spec.ts", 40);
    assert_eq!(opened.open_path().as_deref(), Some("tests/folders.spec.ts"));
    assert!(
        opened.text().starts_with("// The spec of the folders."),
        "the third file's own text"
    );
    assert_near(
        opened.scrolled(),
        line_height * 36.,
        "line 40 of the third file",
    );
    let files: Vec<String> = opened
        .source
        .read()
        .into_iter()
        .filter(|call| call.starts_with("file"))
        .collect();
    assert_eq!(
        files,
        [
            "file src/app/folders.ts",
            "file src/lib.rs",
            "file tests/folders.spec.ts"
        ],
        "three files, each read once"
    );
    assert!(
        opened
            .cx
            .debug_bounds("files-row-tests/folders.spec.ts")
            .is_some(),
        "the folder above the file was opened and read, and its row is in view"
    );
    assert!(opened.events().is_empty(), "following never turned off");
}

#[gpui::test]
fn a_line_of_a_file_with_wrapped_lines_is_reached_by_its_rows(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    opened.agent_at("src/app/sort.ts", 100);
    let rows_above = opened.scrolled() / px(theme::DIFF_LINE_HEIGHT);
    assert!(
        rows_above > 150.,
        "each long line takes several rows: {rows_above}"
    );
}

#[gpui::test]
fn a_place_outside_the_task_is_refused(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    for outside in ["/etc/passwd", "/work/demo/../secret", "/work/demo-2/a.ts"] {
        opened.pane.update_in(opened.cx, |pane, window, cx| {
            pane.show_location(outside, Some(1), window, cx);
        });
        draw(opened.cx);
    }
    assert_eq!(opened.open_path(), None);
    assert_eq!(opened.source.read(), ["tree "], "nothing was asked of box");
}

#[gpui::test]
fn following_stops_when_the_person_scrolls_and_one_call_goes_back_to_the_agent(
    cx: &mut TestAppContext,
) {
    let mut opened = open(cx);
    opened.agent_at("src/app/folders.ts", 150);
    let wheel = ScrollEvent {
        position: bounds(opened.cx, "files-file-area").center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(200.))),
        ..ScrollEvent::default()
    };
    opened.cx.simulate_event(wheel);
    draw(opened.cx);
    assert_eq!(opened.events(), [FilesEvent::FollowChanged(false)]);
    opened.agent_at("src/lib.rs", 100);
    assert_eq!(
        opened.open_path().as_deref(),
        Some("src/app/folders.ts"),
        "the agent moved on, the view stayed with the person"
    );
    opened
        .pane
        .update_in(opened.cx, |pane, window, cx| pane.follow_agent(window, cx));
    draw(opened.cx);
    assert_eq!(opened.open_path().as_deref(), Some("src/lib.rs"));
    assert_near(
        opened.scrolled(),
        px(theme::DIFF_LINE_HEIGHT) * 96.,
        "at the agent's line",
    );
    let changes = [
        FilesEvent::FollowChanged(false),
        FilesEvent::FollowChanged(true),
    ];
    assert_eq!(opened.events(), changes);
}

#[gpui::test]
fn following_stops_when_the_person_presses_in_the_file_or_picks_one(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    opened.agent_at("src/app/folders.ts", 10);
    click(opened.cx, "files-file-area");
    assert_eq!(opened.events(), [FilesEvent::FollowChanged(false)]);
    click(opened.cx, "files-follow");
    assert_eq!(
        opened.events().len(),
        2,
        "the status bar's control turns it on again"
    );
    click(opened.cx, "files-row-src/lib.rs");
    assert_eq!(opened.open_path().as_deref(), Some("src/lib.rs"));
    assert_eq!(
        opened.events().last(),
        Some(&FilesEvent::FollowChanged(false))
    );
    click(opened.cx, "files-follow");
    click(opened.cx, "files-follow");
    assert_eq!(
        opened.events().last(),
        Some(&FilesEvent::FollowChanged(false)),
        "pressed while on, it turns off"
    );
}

#[gpui::test]
fn a_file_that_changes_while_open_is_read_again_and_keeps_its_place(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    opened.agent_at("src/app/folders.ts", 150);
    let place = opened.scrolled();
    let changed = lines(200, false).replace("const line1 = 1;", "const first = 1;");
    opened.source.set_file("src/app/folders.ts", &changed);
    let before = opened.source.read().len();
    opened
        .pane
        .update_in(opened.cx, |pane, window, cx| pane.task_changed(window, cx));
    draw(opened.cx);
    assert!(
        opened.text().starts_with("const first = 1;"),
        "the new text is shown"
    );
    assert_eq!(opened.scrolled(), place, "where the person had it");
    let again = opened.source.read()[before..].to_vec();
    assert_eq!(
        again,
        [
            "tree ",
            "tree src",
            "tree src/app",
            "file src/app/folders.ts"
        ],
        "what is on screen, and nothing else"
    );
}

#[gpui::test]
fn a_file_whose_reread_fails_stays_as_it_was_and_says_so(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    opened.agent_at("src/lib.rs", 5);
    let text = opened.text();
    opened.source.is_failing.set(true);
    opened
        .pane
        .update_in(opened.cx, |pane, window, cx| pane.task_changed(window, cx));
    draw(opened.cx);
    let file = opened.file();
    assert!(file.read_with(opened.cx, |file, _| file.is_stale()));
    assert_eq!(opened.text(), text, "what was read before stays");
    assert!(
        opened.cx.debug_bounds("files-row-src").is_some(),
        "and so does the tree"
    );
}

#[gpui::test]
fn a_file_that_is_too_large_or_not_text_is_one_sentence(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    let note = |opened: &mut Opened, path: &str| {
        opened
            .pane
            .update_in(opened.cx, |pane, window, cx| pane.open(path, window, cx));
        draw(opened.cx);
        let file = opened.file();
        file.read_with(opened.cx, |file, _| file.note())
    };
    assert_eq!(
        note(&mut opened, "big.bin"),
        Some(Key("files.view.too_large"))
    );
    assert_eq!(
        note(&mut opened, "logo.png"),
        Some(Key("files.view.binary"))
    );
    assert_eq!(
        note(&mut opened, "gone.ts"),
        Some(Key("files.view.unreadable"))
    );
    assert_eq!(note(&mut opened, "README.md"), None);
    assert_eq!(opened.text(), "# Folders\n");
}

#[gpui::test]
fn the_lines_that_differ_from_the_base_are_marked_in_the_gutter(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    opened.agent_at("src/app/folders.ts", 1);
    let patch = "diff --git a/src/app/folders.ts b/src/app/folders.ts\n--- a/src/app/folders.ts\n+++ b/src/app/folders.ts\n@@ -1,5 +1,5 @@\n const line1 = 1;\n-old\n+const line2 = 2;\n const line3 = 3;\n-gone\n const line4 = 4;\n+const line5 = 5;\n";
    let file = opened.file();
    assert_eq!(file.read_with(opened.cx, |file, cx| file.mark_count(cx)), 0);
    opened.pane.update(opened.cx, |pane, cx| {
        pane.set_patch(Some(Arc::new(Patch::parse(patch))), cx);
    });
    draw(opened.cx);
    let marks = file.read_with(opened.cx, |file, cx| file.mark_count(cx));
    assert_eq!(marks, 3, "a modified line, a removal and an added line");
    opened.agent_at("src/lib.rs", 1);
    let marks = file.read_with(opened.cx, |file, cx| file.mark_count(cx));
    assert_eq!(marks, 0, "the patch says nothing of this file");
}

#[gpui::test]
fn asking_the_agent_carries_the_selected_lines_and_sends_nothing(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    opened.agent_at("src/lib.rs", 1);
    let file = opened.file();
    let editor = file.read_with(opened.cx, |file, _| file.editor());
    let second_and_third = 17..51;
    editor.update(opened.cx, |editor, cx| {
        editor.set_selected_range(second_and_third, cx)
    });
    draw(opened.cx);
    click(opened.cx, "files-ask-agent");
    let asked = Reference {
        path: "src/lib.rs".into(),
        first_line: 2,
        last_line: 3,
        text: Some("const line2 = 2;\nconst line3 = 3;\n".into()),
    };
    assert_eq!(opened.events().last(), Some(&FilesEvent::AskAgent(asked)));
}

#[gpui::test]
fn only_touched_shows_what_differs_with_its_folders_open(cx: &mut TestAppContext) {
    let opened = open(cx);
    click(opened.cx, "files-only-touched");
    assert!(opened.cx.debug_bounds("files-row-README.md").is_none());
    assert!(opened.cx.debug_bounds("files-row-generated").is_none());
    assert!(opened.cx.debug_bounds("files-row-package.json").is_some());
    assert!(opened
        .cx
        .debug_bounds("files-row-src/app/folders.ts")
        .is_some());
    assert!(opened
        .cx
        .debug_bounds("files-row-src/app/sort.ts")
        .is_none());
    assert_eq!(opened.source.read(), ["tree ", "tree src", "tree src/app"]);
    click(opened.cx, "files-only-touched");
    assert!(opened.cx.debug_bounds("files-row-README.md").is_some());
}

#[gpui::test]
fn nothing_is_read_while_nothing_changes(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    opened.agent_at("src/app/folders.ts", 40);
    let read = opened.source.read();
    for _ in 0..5 {
        opened
            .cx
            .executor()
            .advance_clock(std::time::Duration::from_secs(60));
        draw(opened.cx);
    }
    assert_eq!(opened.source.read(), read);
}

#[test]
fn rows_and_controls_stand_on_the_grid_in_both_languages_at_both_widths() {
    real_text::run(|cx| {
        let mut opened = open(cx);
        click(opened.cx, "files-row-src");
        opened.agent_at("src/lib.rs", 3);
        for width in WIDTHS {
            opened.set_width(width);
            let pane = bounds(opened.cx, "files-pane");
            let inset = pane.left() + theme::SIDE_PANE_INSET;
            let switch = bounds(opened.cx, "files-only-touched");
            assert_near(
                switch.left(),
                inset,
                "the switch starts on the pane's inset",
            );
            assert!(switch.right() <= pane.right() - theme::SIDE_PANE_INSET);
            let toolbar_centre = pane.top() + theme::TOOLBAR_HEIGHT / 2.;
            assert_near(
                switch.center().y,
                toolbar_centre,
                "centred in the toolbar's row",
            );
            assert!(switch.size.height <= theme::TOOLBAR_HEIGHT);
            let row = bounds(opened.cx, "files-row-src");
            assert_eq!(row.size.height, theme::ROW_HEIGHT, "one row pitch");
            assert_near(row.left() + theme::ROW_PADDING_X, inset, "a row's content");
            assert_near(
                row.right(),
                pane.right() - theme::SIDE_PANE_INSET + theme::ROW_PADDING_X,
                "a row's fill",
            );
            let child = bounds(opened.cx, "files-row-src/lib.rs");
            assert_near(
                child.top() - row.top(),
                theme::ROW_HEIGHT * 2.,
                "two rows under its folder",
            );
            let name = bounds(opened.cx, "files-open-name");
            assert_near(name.left(), inset, "the open file's name");
            let header = bounds(opened.cx, "files-open-path");
            assert_eq!(
                header.size.height,
                theme::TOOLBAR_HEIGHT,
                "one row over the file"
            );
            assert_near(header.right(), pane.right(), "to the pane's edge");
        }
    });
}

#[test]
fn a_selection_moves_nothing() {
    real_text::run(|cx| {
        let mut opened = open(cx);
        opened.agent_at("src/lib.rs", 3);
        let names = [
            "files-open-path",
            "files-open-name",
            "files-row-src",
            "files-file-area",
        ];
        let before = names.map(|name| bounds(opened.cx, name));
        let file = opened.file();
        let editor = file.read_with(opened.cx, |file, _| file.editor());
        editor.update(opened.cx, |editor, cx| editor.set_selected_range(0..40, cx));
        draw(opened.cx);
        let after = names.map(|name| bounds(opened.cx, name));
        assert_eq!(before, after);
        let pane = bounds(opened.cx, "files-pane");
        let ask = bounds(opened.cx, "files-ask-agent");
        assert_near(
            ask.right(),
            pane.right() - theme::SIDE_PANE_INSET,
            "the action's end",
        );
        assert_eq!(ask.size.height, theme::STATUS_CONTROL_HEIGHT);
    });
}

#[gpui::test]
fn a_root_that_cannot_be_read_is_asked_once_and_again_only_by_retry(cx: &mut TestAppContext) {
    let source = task();
    source.is_failing.set(true);
    let opened = open_over(cx, source);
    for _ in 0..3 {
        draw(opened.cx);
    }
    assert_eq!(
        opened.source.read(),
        ["tree "],
        "nothing asks again by itself"
    );
    assert!(opened.cx.debug_bounds("files-row-src").is_none());
    opened.source.is_failing.set(false);
    click(opened.cx, "files-retry");
    assert_eq!(opened.source.read(), ["tree ", "tree "]);
    assert!(opened.cx.debug_bounds("files-row-src").is_some());
    assert!(opened.cx.debug_bounds("files-retry").is_none());
}

#[gpui::test]
fn an_answer_for_another_folder_is_a_failure_and_is_not_asked_again(cx: &mut TestAppContext) {
    let source = task();
    let (_, other) = level("docs", Vec::new());
    source.levels.borrow_mut().insert(String::new(), other);
    let opened = open_over(cx, source);
    draw(opened.cx);
    assert_eq!(opened.source.read(), ["tree "]);
    assert!(
        opened.cx.debug_bounds("files-retry").is_some(),
        "said unreadable"
    );
}

#[gpui::test]
fn a_folder_that_cannot_be_read_says_so_and_is_asked_again_when_opened_again(
    cx: &mut TestAppContext,
) {
    let opened = open(cx);
    opened.source.is_failing.set(true);
    click(opened.cx, "files-row-src");
    draw(opened.cx);
    assert_eq!(opened.source.read(), ["tree ", "tree src"], "asked once");
    assert!(opened.cx.debug_bounds("files-row-src/lib.rs").is_none());
    opened.source.is_failing.set(false);
    click(opened.cx, "files-row-src");
    click(opened.cx, "files-row-src");
    assert_eq!(opened.source.read().len(), 3, "the person opened it again");
    assert!(opened.cx.debug_bounds("files-row-src/lib.rs").is_some());
}

#[gpui::test]
fn an_older_answer_for_a_folder_never_replaces_a_newer_one(cx: &mut TestAppContext) {
    let opened = open(cx);
    let changed = |pane: &Entity<FilesPane>, cx: &mut VisualTestContext| {
        pane.update_in(cx, |pane, window, cx| pane.task_changed(window, cx));
    };
    opened.source.is_holding.set(true);
    changed(&opened.pane, opened.cx);
    let (root, mut with_new) = level("", Vec::new());
    with_new.entries = opened.source.levels.borrow()[""].entries.clone();
    with_new
        .entries
        .push(entry("new.ts", EntryKind::File, Some((3, 0))));
    opened.source.levels.borrow_mut().insert(root, with_new);
    changed(&opened.pane, opened.cx);
    opened.source.release_newest();
    draw(opened.cx);
    assert!(
        opened.cx.debug_bounds("files-row-new.ts").is_some(),
        "the newer answer"
    );
    opened.source.release();
    draw(opened.cx);
    assert!(
        opened.cx.debug_bounds("files-row-new.ts").is_some(),
        "the older answer came last and is dropped"
    );
}

#[gpui::test]
fn a_press_on_the_file_s_name_or_on_a_file_that_is_not_text_stops_following(
    cx: &mut TestAppContext,
) {
    let mut opened = open(cx);
    opened.agent_at("src/lib.rs", 3);
    click(opened.cx, "files-open-path");
    assert_eq!(
        opened.events(),
        [FilesEvent::FollowChanged(false)],
        "the row over the file"
    );
    opened
        .pane
        .update_in(opened.cx, |pane, window, cx| pane.follow_agent(window, cx));
    opened.agent_at("big.bin", 1);
    assert_eq!(opened.open_path().as_deref(), Some("big.bin"));
    click(opened.cx, "files-file-area");
    let changes = [
        FilesEvent::FollowChanged(false),
        FilesEvent::FollowChanged(true),
        FilesEvent::FollowChanged(false),
    ];
    assert_eq!(
        opened.events(),
        changes,
        "the sentence of a file that is not shown"
    );
}

#[gpui::test]
fn a_file_that_has_the_keyboard_is_not_drawn_again_while_nothing_happens(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    opened.agent_at("src/lib.rs", 3);
    click(opened.cx, "files-file-area");
    let file = opened.file();
    let editor = file.read_with(opened.cx, |file, _| file.editor());
    let has_keyboard = editor.read_with(opened.cx, |editor, cx| {
        gpui::Focusable::focus_handle(editor, cx)
    });
    assert!(
        opened
            .cx
            .update(|window, _| has_keyboard.is_focused(window)),
        "the press gave it the keyboard"
    );
    let asked = Rc::new(Cell::new(0));
    let (of_file, of_editor) = (asked.clone(), asked.clone());
    opened.cx.update(|_, cx| {
        cx.observe(&file, move |_, _| of_file.set(of_file.get() + 1))
            .detach();
        cx.observe(&editor, move |_, _| of_editor.set(of_editor.get() + 1))
            .detach();
    });
    for _ in 0..20 {
        opened
            .cx
            .executor()
            .advance_clock(std::time::Duration::from_millis(500));
        opened.cx.run_until_parked();
    }
    assert_eq!(asked.get(), 0, "no caret timer asks for a frame at rest");
}

#[gpui::test]
fn the_caret_blinks_again_elsewhere_once_the_file_gives_the_keyboard_back(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    let policy =
        |cx: &mut VisualTestContext| cx.read(|cx| gpui_component::Theme::global(cx).cursor_blink);
    let before = policy(opened.cx);
    assert!(!before.is_steady(), "the app's own policy blinks");
    opened.agent_at("src/lib.rs", 3);
    click(opened.cx, "files-file-area");
    assert!(
        policy(opened.cx).is_steady(),
        "while the file has the keyboard"
    );
    opened.cx.update(|window, cx| window.blur(cx));
    draw(opened.cx);
    assert_eq!(policy(opened.cx), before, "put back as it was");
}
