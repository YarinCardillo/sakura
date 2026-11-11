//! The palette on screen: the kit's command list (a search field over grouped commands, each with its key
//! when it has one) in a panel near the top of the window, over a dimmed window. It appears and goes at
//! once, with no animation. Opened by its key it has the caret in its field, the one place in the app that
//! takes the keyboard without a click; opened any other way (a menu, a button) the palette itself has the
//! keyboard, for Esc, and the field takes the caret when it is pressed. Closing gives the keyboard back
//! to what had it.

use super::commands::{groups, is_offered, Ask, Entry, Group, Run, Situation};
use super::metrics::{
    EMPTY_PADDING_Y, EMPTY_TEXT_SIZE, LAYER, LIST_MAX_HEIGHT, SCRIM_ALPHA, SIDE_MARGIN, TOP, WIDTH,
};
use crate::l10n::{tr, Key, Language, LanguageChoice};
use crate::menus::{
    LanguageAutomatic, LanguageEnglish, LanguageItalian, NewTask, ShowSettings, StartPlay,
    StopPlay, ToggleChanges, ToggleSidebar,
};
use crate::ui::centred;
use crate::ui::files::quick_open::ToggleQuickOpen;
use crate::ui::tabs::keys::CloseTab;
use gpui::{
    anchored, deferred, div, point, px, AnyElement, App, AppContext as _, Context, Div, Entity,
    EventEmitter, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    ParentElement as _, Pixels, Render, Size, Stateful, Styled as _, WeakEntity, Window,
};
use gpui_component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_component::{ActiveTheme as _, IndexPath};
use std::rc::Rc;

/// What the field says before anything is typed.
pub const PLACEHOLDER: Key = Key("palette.placeholder");
/// The key that closes the palette.
const ESCAPE: &str = "escape";
/// What the list says when nothing matches.
pub const NOTHING_FOUND: Key = Key("palette.nothing");

/// A command was chosen that the desk carries out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteEvent {
    Asked(Ask),
}

/// The command palette of one window.
pub struct Palette {
    state: Entity<CommandState>,
    /// The commands listed while the palette is open.
    listed: Option<Vec<Group>>,
    /// Where the desk's panel started when the palette opened: it is centred in the panel.
    panel_left: Pixels,
    /// What had the keyboard before the palette opened.
    before: Option<FocusHandle>,
    /// The palette's own place for the keyboard when it was not opened by a key.
    handle: FocusHandle,
    /// Where the window is now, asked again right before a chosen command is carried out.
    now: Option<Rc<Now>>,
}

/// Reads where the window is.
pub type Now = dyn Fn(&App) -> Situation;

impl EventEmitter<PaletteEvent> for Palette {}

impl Palette {
    /// A closed palette for `window`.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            state: cx.new(|cx| CommandState::new(window, cx)),
            listed: None,
            panel_left: Pixels::ZERO,
            before: None,
            handle: cx.focus_handle(),
            now: None,
        }
    }

    /// Gives the palette the way to read where the window is, so a chosen command is checked against
    /// the window as it is when it is chosen and not as it was when the palette opened.
    pub fn watch(&mut self, now: Rc<Now>) {
        self.now = Some(now);
    }

    /// Whether the palette is on screen.
    pub fn is_open(&self) -> bool {
        self.listed.is_some()
    }

    /// What is typed in its field.
    #[cfg(test)]
    pub fn query(&self, cx: &gpui::App) -> gpui::SharedString {
        self.state.read(cx).query(cx)
    }

    /// Opens the palette for `situation`, or closes it when it is open.
    pub fn toggle(&mut self, situation: &Situation, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_open() {
            self.close(window, cx);
        } else {
            self.open(situation, window, cx);
        }
    }

    /// Lists the commands for `situation` with an empty field; the caret is in it when a key opened it.
    pub fn open(&mut self, situation: &Situation, window: &mut Window, cx: &mut Context<Self>) {
        self.listed = Some(groups(situation));
        self.panel_left = situation.panel_left;
        self.before = window.focused(cx);
        // A list of its own for each opening: an empty field and its first command highlighted, whatever
        // was typed or highlighted the last time.
        self.state = cx.new(|cx| CommandState::new(window, cx));
        // Keyboard-opened input receives focus so typing starts immediately.
        // Opened with the pointer, nothing is given a caret the person did not ask for.
        if window.last_input_was_keyboard() {
            self.state.update(cx, |state, cx| state.focus(window, cx));
        } else {
            self.handle.focus(window, cx);
        }
        cx.notify();
    }

    /// Takes the palette off the screen and gives the keyboard back.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.listed.take().is_none() {
            return;
        }
        match self.before.take() {
            Some(before) => before.focus(window, cx),
            None => window.blur(cx),
        }
        cx.notify();
    }

    /// The command at `path` was chosen: the palette goes, and the desk is requested what only it can do
    /// (a command with an action of its own was already dispatched by the list). A command the window no
    /// longer offers (its task is gone, is not the one shown any more, or cannot do it now) is dropped.
    fn chosen(&mut self, path: IndexPath, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self
            .listed
            .as_ref()
            .and_then(|listed| listed.get(path.section)?.entries.get(path.row));
        let run = entry.map(|entry| entry.run.clone());
        self.close(window, cx);
        let Some(Run::Ask(ask)) = run else {
            return;
        };
        let is_current = self
            .now
            .as_ref()
            .is_none_or(|now| is_offered(&now(cx), &Run::Ask(ask.clone())));
        if is_current {
            cx.emit(PaletteEvent::Asked(ask));
        }
    }
}

impl Render for Palette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(listed) = &self.listed else {
            return div().into_any_element();
        };
        let me = cx.weak_entity();
        let viewport = window.viewport_size();
        let panel = div()
            .id("palette-panel")
            .debug_selector(|| "palette-panel".into())
            .w(px(WIDTH))
            .max_w(viewport.width - px(SIDE_MARGIN * 2.))
            .rounded(cx.theme().radius_lg)
            .shadow_lg()
            .track_focus(&self.handle)
            .on_key_down(cx.listener(|palette, event: &KeyDownEvent, window, cx| {
                // Only when the palette itself has the keyboard: the field has its own Esc.
                let is_mine = palette.handle.is_focused(window);
                if is_mine && event.keystroke.key == ESCAPE {
                    palette.close(window, cx);
                }
            }))
            // A press on the panel is not a press outside it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(list(&self.state, listed, &me));
        let placed = anchored()
            .position(point(px(0.), px(0.)))
            .child(scrim(viewport, self.panel_left, me).child(panel));
        deferred(placed).with_priority(LAYER).into_any_element()
    }
}

/// The dimmed layer over the whole window, with the panel near its top and centred across the desk's
/// panel, which starts `panel_left` from the window's left edge; a press on it closes the palette.
fn scrim(
    viewport: Size<Pixels>,
    panel_left: Pixels,
    palette: WeakEntity<Palette>,
) -> Stateful<Div> {
    div()
        .id("palette-scrim")
        .debug_selector(|| "palette-scrim".into())
        .occlude()
        .w(viewport.width)
        .h(viewport.height)
        .bg(gpui::black().opacity(SCRIM_ALPHA))
        .flex()
        .justify_center()
        .items_start()
        .pt(px(TOP))
        .pl(centred::panel_offset(panel_left) * 2.)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            drop(palette.update(cx, |palette, cx| palette.close(window, cx)));
        })
}

/// The kit's command list over `listed`, telling the palette what was chosen and when Esc asks to leave.
fn list(state: &Entity<CommandState>, listed: &[Group], palette: &WeakEntity<Palette>) -> Command {
    let (chosen, left) = (palette.clone(), palette.clone());
    let command = Command::new(state)
        .placeholder(tr(PLACEHOLDER))
        .max_h(px(LIST_MAX_HEIGHT))
        .empty(|_, _, cx| nothing_found(cx))
        .on_confirm(move |path, window, cx| {
            drop(chosen.update(cx, |palette, cx| palette.chosen(path, window, cx)));
        })
        .on_cancel(move |window, cx| {
            // The list is in the middle of its own update: the palette leaves right after it.
            let left = left.clone();
            window.defer(cx, move |window, cx| {
                drop(left.update(cx, |palette, cx| palette.close(window, cx)));
            });
        });
    listed
        .iter()
        .fold(command, |command, group| command.group(kit_group(group)))
}

fn kit_group(group: &Group) -> CommandGroup {
    CommandGroup::new()
        .label(group.label.clone())
        .items(group.entries.iter().map(kit_item))
}

/// The entry as the kit's item: a command of the window carries its action, whose key the row then shows.
fn kit_item(entry: &Entry) -> CommandItem {
    let item = CommandItem::new()
        .label(entry.label.clone())
        .checked(entry.is_current);
    match entry.run {
        Run::NewTask => item.action(Box::new(NewTask)),
        Run::OpenFile => item.action(Box::new(ToggleQuickOpen)),
        Run::Language(LanguageChoice::Automatic) => item.action(Box::new(LanguageAutomatic)),
        Run::Language(LanguageChoice::Is(Language::English)) => {
            item.action(Box::new(LanguageEnglish))
        }
        Run::Language(LanguageChoice::Is(Language::Italian)) => {
            item.action(Box::new(LanguageItalian))
        }
        Run::ToggleSidebar => item.action(Box::new(ToggleSidebar)),
        Run::ToggleChanges => item.action(Box::new(ToggleChanges)),
        Run::CloseTab => item.action(Box::new(CloseTab)),
        Run::StartPlay => item.action(Box::new(StartPlay)),
        Run::StopPlay => item.action(Box::new(StopPlay)),
        Run::Settings => item.action(Box::new(ShowSettings)),
        Run::Ask(_) => item,
    }
}

fn nothing_found(cx: &gpui::App) -> AnyElement {
    div()
        .py(px(EMPTY_PADDING_Y))
        .w_full()
        .text_center()
        .text_size(px(EMPTY_TEXT_SIZE))
        .text_color(cx.theme().muted_foreground)
        .child(tr(NOTHING_FOUND))
        .into_any_element()
}
