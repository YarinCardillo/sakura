//! Quick open, Cmd+P: any file of the task by name. The kit's command list in a panel near the top of
//! the window, as the command palette is (the same sizes); what is typed is asked of `box search`, which
//! answers with the files whose path holds it, a match in the name first. Each row is a file's name with
//! its folder beside it, quieter (Zed's file finder). It appears and goes at once, with no animation.
//! Opened by its key it has the caret in its field; closing gives the keyboard back to what had it.

use super::source::Source;
use crate::l10n::{tr, Key};
use crate::theme::palette::{
    EMPTY_PADDING_Y, EMPTY_TEXT_SIZE, LAYER, LIST_MAX_HEIGHT, SCRIM_ALPHA, SIDE_MARGIN, TOP, WIDTH,
};
use crate::theme::{self, Theme};
use crate::ui::centred;
use gpui::{
    actions, anchored, deferred, div, point, px, AnyElement, App, AppContext as _, Context, Div,
    Entity, EventEmitter, FocusHandle, InteractiveElement as _, IntoElement, KeyBinding,
    KeyDownEvent, MouseButton, ParentElement as _, Pixels, Render, Size, Stateful, Styled as _,
    WeakEntity, Window,
};
use gpui_component::command::{Command, CommandItem, CommandState};
use gpui_component::{ActiveTheme as _, IndexPath};
use sakura_core::error::BridgeResult;
use sakura_core::files::Matches;
use std::rc::Rc;

actions!(sakura_files, [ToggleQuickOpen]);

/// The key that closes quick open.
const ESCAPE: &str = "escape";

/// A file was picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickOpenEvent {
    /// The task quick open was opened for, and the path of the file inside it. The file is that
    /// task's, whatever is on screen by the time the event is taken.
    Picked { task: String, path: String },
}

/// What quick open is opened for: a task and its files.
#[derive(Clone)]
pub struct Target {
    /// The task's name.
    pub task: String,
    pub source: Rc<dyn Source>,
    /// Where the panel starts from the window's left edge: quick open is centred in the panel.
    pub panel_left: Pixels,
}

/// What the list has to show for what is typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Found {
    /// Nothing is typed yet.
    #[default]
    Nothing,
    Files {
        paths: Vec<String>,
        is_truncated: bool,
    },
    /// `box` could not search.
    Failed,
}

/// Quick open of one window.
pub struct QuickOpen {
    state: Entity<CommandState>,
    /// The task it was opened for and its files; nothing while it is closed.
    target: Option<Target>,
    /// Counts the openings and the closings: an answer requested in another one is dropped, so the
    /// files of one task never show for another.
    opening: u64,
    found: Found,
    /// What had the keyboard before it opened.
    before: Option<FocusHandle>,
    /// Its own place for the keyboard when it was not opened by a key.
    handle: FocusHandle,
    /// What is typed, trimmed: the text whose answer is shown.
    wanted: String,
    /// Whether `box` is searching now.
    is_searching: bool,
}

impl EventEmitter<QuickOpenEvent> for QuickOpen {}

/// Binds Cmd+P to `quick_open` in `window`. `target` gives the task on screen and its files each time
/// the key is pressed; with no task on screen the key does nothing.
pub fn install(
    quick_open: &Entity<QuickOpen>,
    target: impl Fn(&App) -> Option<Target> + 'static,
    window: &Window,
    cx: &mut App,
) {
    cx.bind_keys([KeyBinding::new("cmd-p", ToggleQuickOpen, None)]);
    let (handle, quick_open) = (window.window_handle(), quick_open.downgrade());
    cx.on_action(move |_: &ToggleQuickOpen, cx| {
        let (Some(quick_open), target) = (quick_open.upgrade(), target(cx)) else {
            return;
        };
        // The window is being updated while its key dispatches: quick open opens right after.
        cx.defer(move |cx| {
            let toggled = handle.update(cx, |_, window, cx| {
                quick_open.update(cx, |quick_open, cx| quick_open.toggle(target, window, cx));
            });
            if let Err(error) = toggled {
                eprintln!("sakura: the window is gone: {error}");
            }
        });
    });
}

impl QuickOpen {
    /// A closed quick open for `window`.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            state: cx.new(|cx| CommandState::new(window, cx)),
            target: None,
            opening: 0,
            found: Found::default(),
            before: None,
            handle: cx.focus_handle(),
            wanted: String::new(),
            is_searching: false,
        }
    }

    /// Whether it is on screen.
    pub fn is_open(&self) -> bool {
        self.target.is_some()
    }

    /// Opens it for `target`, or closes it when it is open (or there is no task).
    pub fn toggle(&mut self, target: Option<Target>, window: &mut Window, cx: &mut Context<Self>) {
        match target.filter(|_| !self.is_open()) {
            Some(target) => self.open(target, window, cx),
            None => self.close(window, cx),
        }
    }

    /// Shows it with an empty field over the files of `target`; the caret is in the field when a key
    /// opened it. Whatever was asked in an earlier opening is no longer waited for.
    pub fn open(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.target = Some(target);
        self.opening += 1;
        self.is_searching = false;
        self.found = Found::default();
        self.wanted.clear();
        self.before = window.focused(cx);
        // A list of its own for each opening: an empty field, whatever was typed the last time.
        self.state = cx.new(|cx| CommandState::new(window, cx));
        // Keyboard-opened input receives focus so typing starts immediately.
        if window.last_input_was_keyboard() {
            self.state.update(cx, |state, cx| state.focus(window, cx));
        } else {
            self.handle.focus(window, cx);
        }
        cx.notify();
    }

    /// Takes it off the screen and gives the keyboard back.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.target.take().is_none() {
            return;
        }
        // An answer still on its way is for an opening that is over: it is dropped.
        self.opening += 1;
        self.is_searching = false;
        self.wanted.clear();
        match self.before.take() {
            Some(before) => before.focus(window, cx),
            None => window.blur(cx),
        }
        cx.notify();
    }

    /// What is typed changed: `box` is requested the files that hold it. One search runs at a time:
    /// what is typed while one is under way is asked when it ends, so fast typing starts no pile of
    /// processes.
    fn search(&mut self, text: &str, cx: &mut Context<Self>) {
        self.wanted = text.trim().to_string();
        if self.wanted.is_empty() {
            self.found = Found::Nothing;
            return cx.notify();
        }
        if !self.is_searching {
            self.ask(cx);
        }
    }

    /// Asks `box` for what is typed now.
    fn ask(&mut self, cx: &mut Context<Self>) {
        let Some(target) = &self.target else {
            return;
        };
        let asked = (self.opening, self.wanted.clone());
        let searching = target.source.search(&asked.1);
        self.is_searching = true;
        cx.spawn(async move |quick_open, cx| {
            let answer = searching.await;
            drop(quick_open.update(cx, |quick_open, cx| quick_open.answered(asked, answer, cx)));
        })
        .detach();
    }

    /// `box` answered for `asked`, an opening and a text. An answer for another opening is dropped;
    /// one for this opening is shown when its text is still what is typed, else what is typed now is
    /// asked.
    fn answered(
        &mut self,
        (opening, asked): (u64, String),
        answer: BridgeResult<Matches>,
        cx: &mut Context<Self>,
    ) {
        if opening != self.opening {
            return;
        }
        self.is_searching = false;
        if asked != self.wanted {
            return match self.wanted.is_empty() {
                true => {}
                false => self.ask(cx),
            };
        }
        self.found = match answer {
            Ok(matches) => Found::Files {
                paths: matches.matches,
                is_truncated: matches.truncated,
            },
            Err(_) => Found::Failed,
        };
        cx.notify();
    }

    /// The file at `path` of the list was picked: quick open goes and says which.
    fn picked(&mut self, path: IndexPath, window: &mut Window, cx: &mut Context<Self>) {
        let Found::Files { paths, .. } = &self.found else {
            return;
        };
        let picked = paths.get(path.row).cloned();
        let task = self.target.as_ref().map(|target| target.task.clone());
        self.close(window, cx);
        if let (Some(task), Some(path)) = (task, picked) {
            cx.emit(QuickOpenEvent::Picked { task, path });
        }
    }

    /// The kit's command list over what was found. `box` already matched the files against what is
    /// typed, so the list shows them all, in the order `box` gave.
    fn list(&self, me: &WeakEntity<Self>) -> Command {
        let (asked, chosen, left) = (me.clone(), me.clone(), me.clone());
        let found = self.found.clone();
        let command = Command::new(&self.state)
            .placeholder(tr(Key("files.quick_open.placeholder")))
            .filterable(false)
            .max_h(px(LIST_MAX_HEIGHT))
            .empty(move |_, _, cx| said(empty_words(&found), cx))
            .on_query(move |text, _, cx| {
                drop(asked.update(cx, |quick_open, cx| quick_open.search(text, cx)));
            })
            .on_confirm(move |path, window, cx| {
                drop(chosen.update(cx, |quick_open, cx| quick_open.picked(path, window, cx)));
            })
            .on_cancel(move |window, cx| {
                // The list is in the middle of its own update: quick open leaves right after it.
                let left = left.clone();
                window.defer(cx, move |window, cx| {
                    drop(left.update(cx, |quick_open, cx| quick_open.close(window, cx)));
                });
            });
        self.with_files(command)
    }

    /// `command` with the files found as its rows, and a line under them when `box` has more.
    fn with_files(&self, command: Command) -> Command {
        let Found::Files {
            paths,
            is_truncated,
        } = &self.found
        else {
            return command;
        };
        let command = command.items(paths.iter().map(|path| item(path)));
        match is_truncated {
            true => command.footer(|_, _, cx| said(Key("files.quick_open.truncated"), cx)),
            false => command,
        }
    }
}

/// What the list says when it has no file to show.
fn empty_words(found: &Found) -> Key {
    match found {
        Found::Nothing => Key("files.quick_open.hint"),
        Found::Files { .. } => Key("files.quick_open.none"),
        Found::Failed => Key("files.quick_open.failed"),
    }
}

/// One file of the list: its name, and its folder beside it, quieter and cut when the row is short.
fn item(path: &str) -> CommandItem {
    let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
    let name = sakura_core::display::label(name);
    let folder = sakura_core::display::label(folder);
    let selector = format!("quick-open-file-{path}");
    CommandItem::new()
        .label(path.to_string())
        .child(move |_, cx| {
            let selector = selector.clone();
            let quiet = cx.theme().muted_foreground;
            let folder = div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(quiet)
                .child(folder.clone());
            div()
                .debug_selector(move || selector)
                .w_full()
                .flex()
                .items_center()
                .gap(theme::GAP)
                .child(div().flex_none().child(name.clone()))
                .child(folder)
        })
}

/// A quiet sentence in the list's place or under it.
fn said(words: Key, cx: &App) -> AnyElement {
    div()
        .py(px(EMPTY_PADDING_Y))
        .w_full()
        .text_center()
        .text_size(px(EMPTY_TEXT_SIZE))
        .text_color(Theme::palette(cx).text_tertiary)
        .child(tr(words))
        .into_any_element()
}

/// The dimmed layer over the whole window, with the panel near its top; a press on it closes quick open.
fn scrim(
    viewport: Size<Pixels>,
    panel_left: Pixels,
    quick_open: WeakEntity<QuickOpen>,
) -> Stateful<Div> {
    div()
        .id("quick-open-scrim")
        .debug_selector(|| "quick-open-scrim".into())
        .occlude()
        .w(viewport.width)
        .h(viewport.height)
        .bg(gpui::black().opacity(SCRIM_ALPHA))
        // What it holds is centred in the panel, by the one rule for that: not in the window.
        .pl(centred::panel_offset(panel_left) * 2.)
        .flex()
        .justify_center()
        .items_start()
        .pt(px(TOP))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            drop(quick_open.update(cx, |quick_open, cx| quick_open.close(window, cx)));
        })
}

impl Render for QuickOpen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_open() {
            return div().into_any_element();
        }
        let me = cx.weak_entity();
        let viewport = window.viewport_size();
        let target = self.target.as_ref();
        let panel_left = target.map_or(Pixels::ZERO, |target| target.panel_left);
        let panel = div()
            .id("quick-open-panel")
            .debug_selector(|| "quick-open-panel".into())
            .w(px(WIDTH))
            .max_w(viewport.width - px(SIDE_MARGIN * 2.))
            .rounded(cx.theme().radius_lg)
            .shadow_lg()
            .track_focus(&self.handle)
            .on_key_down(cx.listener(|quick_open, event: &KeyDownEvent, window, cx| {
                // Only when quick open itself has the keyboard: the field has its own Esc.
                let is_mine = quick_open.handle.is_focused(window);
                if is_mine && event.keystroke.key == ESCAPE {
                    quick_open.close(window, cx);
                }
            }))
            // A press on the panel is not a press outside it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.list(&me));
        let placed = anchored()
            .position(point(Pixels::ZERO, Pixels::ZERO))
            .child(scrim(viewport, panel_left, me).child(panel));
        deferred(placed).with_priority(LAYER).into_any_element()
    }
}
