//! The palette on screen: the kit's command list (a search field over grouped commands, each with its key
//! when it has one) in a panel near the top of the window, over a dimmed window. It appears and goes at
//! once. The engine desk focuses search on open and restores the previous focus on close.

#[cfg(feature = "boxes")]
use super::metrics::WIDTH as COMMAND_WIDTH;
#[cfg(not(feature = "boxes"))]
use super::metrics::{COMMAND_BOTTOM_MARGIN, COMMAND_LIST_MAX_HEIGHT, COMMAND_TOP, COMMAND_WIDTH};
use super::metrics::{
    EMPTY_PADDING_Y, EMPTY_TEXT_SIZE, FULL_HEIGHT, GLYPH_SIZE, LAYER, LIBRARY_LINE_FRAME_REMS,
    LIST_MAX_HEIGHT, LIST_REM, SCRIM_ALPHA, SIDE_MARGIN,
};
use super::ChooseInNewTab;
use super::{
    action_of, groups, icon_of, is_offered, Ask, Entry, Group, Run, Situation, PLACEHOLDER,
};
use crate::l10n::{tr, Key};
use crate::theme::Theme;
#[cfg(feature = "boxes")]
use crate::ui::centred;
use crate::ui::kit::command_row;
use crate::ui::rem_scope::at_rem;
#[cfg(not(feature = "boxes"))]
use gpui::Focusable as _;
use gpui::{
    anchored, deferred, div, point, px, AnyElement, App, AppContext as _, Context, Div, Entity,
    EventEmitter, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent, Keystroke,
    MouseButton, ParentElement as _, Pixels, Render, SharedString, Size, Stateful, Styled as _,
    WeakEntity, Window,
};
use gpui_base::actions::Confirm;
use gpui_component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_component::kbd::Kbd;
use gpui_component::{ActiveTheme as _, Icon, IndexPath};
use std::rc::Rc;

/// The key that closes the palette.
const ESCAPE: &str = "escape";
/// The key that chooses the highlighted command.
const RETURN: &str = "enter";
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
    /// Receives Escape before the search field is activated.
    handle: FocusHandle,
    /// Where the window is now, asked again right before a chosen command is carried out.
    now: Option<Rc<Now>>,
    /// Cmd-Return requested the choice about to be made to open in a tab of its own.
    is_new_tab: bool,
    /// The page that was on screen when the palette opened: the palette does not outlive a change of it.
    #[cfg(not(feature = "boxes"))]
    page: Option<sakura_core::tabs::Page>,
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
            is_new_tab: false,
            #[cfg(not(feature = "boxes"))]
            page: None,
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

    /// Lists the commands for `situation` with an empty search field.
    pub fn open(&mut self, situation: &Situation, window: &mut Window, cx: &mut Context<Self>) {
        self.listed = Some(groups(situation));
        self.panel_left = situation.panel_left;
        #[cfg(not(feature = "boxes"))]
        {
            self.page = Some(situation.page.clone());
        }
        self.before = window.focused(cx);
        // A list of its own for each opening: an empty field and its first command highlighted, whatever
        // was typed or highlighted the last time.
        self.state = cx.new(|cx| CommandState::new(window, cx));
        #[cfg(feature = "boxes")]
        self.handle.focus(window, cx);
        #[cfg(not(feature = "boxes"))]
        self.state.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Closes the palette when the page on screen is not the one it opened over.
    #[cfg(not(feature = "boxes"))]
    pub fn follow_page(
        &mut self,
        shown: &sakura_core::tabs::Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_open() && self.page.as_ref() != Some(shown) {
            self.close(window, cx);
        }
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
        let is_new_tab = std::mem::take(&mut self.is_new_tab) || window.modifiers().platform;
        self.close(window, cx);
        let Some(Run::Ask(ask)) = run else {
            return;
        };
        let is_current = self
            .now
            .as_ref()
            .is_none_or(|now| is_offered(&now(cx), &Run::Ask(ask.clone())));
        if is_current {
            cx.emit(PaletteEvent::Asked(ask.in_new_tab(is_new_tab)));
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
            .w(px(COMMAND_WIDTH))
            .max_w(viewport.width - px(SIDE_MARGIN * 2.))
            .rounded(cx.theme().radius_lg)
            .shadow_lg()
            .track_focus(&self.handle)
            .on_action(cx.listener(|palette, _: &ChooseInNewTab, window, cx| {
                palette.is_new_tab = true;
                window.dispatch_action(Box::new(Confirm { secondary: false }), cx);
            }))
            // The capture phase, so that the field's own Esc (which only clears what was typed) never
            // answers first: the first Esc leaves the palette, with words typed or not.
            .capture_action(cx.listener(
                |palette, _: &gpui_component::input::Escape, window, cx| palette.close(window, cx),
            ))
            .capture_key_down(cx.listener(|palette, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == ESCAPE {
                    cx.stop_propagation();
                    palette.close(window, cx);
                }
            }))
            // A press on the panel is not a press outside it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(at_rem(
                LIST_REM,
                list(
                    &self.state,
                    listed,
                    &me,
                    self.state.read(cx).selected_index(),
                    list_height(viewport.height),
                ),
            ));
        let placed = anchored()
            .position(point(px(0.), px(0.)))
            .child(scrim(viewport, self.panel_left, me).child(panel));
        deferred(placed).with_priority(LAYER).into_any_element()
    }
}

/// Where the panel's top stands: a fixed distance from the window's top in the engine's window; in the
/// other build the middle of the window's height at its tallest.
fn top(window_height: Pixels) -> Pixels {
    #[cfg(not(feature = "boxes"))]
    {
        let _ = (window_height, FULL_HEIGHT, SIDE_MARGIN);
        px(COMMAND_TOP)
    }
    #[cfg(feature = "boxes")]
    {
        centred::overlay_top(window_height, px(FULL_HEIGHT), px(SIDE_MARGIN))
    }
}

/// The dimmed layer over the whole window, with the panel centred across the desk's panel, which
/// starts `panel_left` from the window's left edge, and standing where it is in the middle of the
/// window's height at its tallest; a press on it closes the palette.
fn scrim(
    viewport: Size<Pixels>,
    panel_left: Pixels,
    palette: WeakEntity<Palette>,
) -> Stateful<Div> {
    let scrim = div()
        .id("palette-scrim")
        .debug_selector(|| "palette-scrim".into())
        .occlude()
        .w(viewport.width)
        .h(viewport.height)
        .bg(gpui::black().opacity(SCRIM_ALPHA))
        .flex()
        .justify_center()
        .items_start()
        .pt(top(viewport.height));
    // The engine's window centres the palette on the window; the file list's window centres it on its panel.
    #[cfg(feature = "boxes")]
    let scrim = scrim.pl(centred::panel_offset(panel_left) * 2.);
    #[cfg(not(feature = "boxes"))]
    let _ = panel_left;
    scrim.on_mouse_down(MouseButton::Left, move |_, window, cx| {
        drop(palette.update(cx, |palette, cx| palette.close(window, cx)));
    })
}

/// How high the list may grow: in the engine's window every group stands in view while the window has the
/// room (the Projects group is the last, and was cut at the bottom edge); in the other build a fixed height.
fn list_height(window_height: Pixels) -> Pixels {
    #[cfg(not(feature = "boxes"))]
    {
        let room = window_height - px(COMMAND_TOP) - px(COMMAND_BOTTOM_MARGIN);
        room.clamp(px(LIST_MAX_HEIGHT), px(COMMAND_LIST_MAX_HEIGHT))
    }
    #[cfg(feature = "boxes")]
    {
        let _ = window_height;
        px(LIST_MAX_HEIGHT)
    }
}

/// The kit's command list over `listed`, telling the palette what was chosen and when Esc asks to leave.
fn list(
    state: &Entity<CommandState>,
    listed: &[Group],
    palette: &WeakEntity<Palette>,
    selected: Option<IndexPath>,
    max_height: Pixels,
) -> Command {
    let (chosen, left, moved) = (palette.clone(), palette.clone(), palette.clone());
    let command = Command::new(state)
        .placeholder(tr(PLACEHOLDER))
        .max_h(max_height)
        .empty(|_, _, cx| nothing_found(cx))
        .on_confirm(move |path, window, cx| {
            drop(chosen.update(cx, |palette, cx| palette.chosen(path, window, cx)));
        })
        .on_select(move |_, _, cx| {
            // The row that shows Return follows the highlight: the palette draws again.
            drop(moved.update(cx, |_, cx| cx.notify()));
        })
        .on_cancel(move |window, cx| {
            // The list is in the middle of its own update: the palette leaves right after it.
            let left = left.clone();
            window.defer(cx, move |window, cx| {
                drop(left.update(cx, |palette, cx| palette.close(window, cx)));
            });
        });
    let groups = listed.iter().enumerate();
    groups.fold(command, |command, (section, group)| {
        command.group(kit_group(group, section, selected))
    })
}

fn kit_group(group: &Group, section: usize, selected: Option<IndexPath>) -> CommandGroup {
    let item = |(row, entry): (usize, &Entry)| {
        let is_selected = selected.is_some_and(|at| at.section == section && at.row == row);
        kit_item(entry, is_selected, format!("palette-row-{section}-{row}"))
    };
    CommandGroup::new()
        .label(group.label.clone())
        .items(group.entries.iter().enumerate().map(item))
}

/// What stands at the end of a line: the key of a command of the window, else, on the highlighted line of
/// any other command, Return, the key that chooses it.
enum Trailing {
    Action(Box<dyn gpui::Action>),
    Return,
    Nothing,
}

/// The entry as the kit's item, drawn by the kit's command row so that every line is as high as the scale's
/// large control: a command of the window carries its action, which the list carries out, and its key.
fn kit_item(entry: &Entry, is_selected: bool, selector: String) -> CommandItem {
    let item = CommandItem::new()
        .label(entry.label.clone())
        .checked(entry.is_current);
    let action = action_of(&entry.run);
    let is_lane = entry.glyph.is_some();
    let trailing = match &action {
        Some(action) if !is_lane => Trailing::Action(action.boxed_clone()),
        _ if is_selected && (is_lane || !entry.is_current) => Trailing::Return,
        _ => Trailing::Nothing,
    };
    let (glyph, icon) = (entry.glyph, icon_of(&entry.run));
    let label: SharedString = entry.label.clone().into();
    let item = match action {
        Some(action) => item.action(action),
        None => item,
    };
    item.child(move |window, cx| {
        let palette = Theme::palette(cx);
        let ink = match is_selected {
            true => cx.theme().accent_foreground,
            false => cx.theme().muted_foreground,
        };
        let lead = match (glyph, icon) {
            (Some(glyph), _) => Some(glyph.paint(px(GLYPH_SIZE), &palette).into_any_element()),
            (None, Some(icon)) => Some(Icon::new(icon).size_4().text_color(ink).into_any_element()),
            (None, None) => None,
        };
        let key = match &trailing {
            Trailing::Action(action) => Kbd::binding_for_action(action.as_ref(), None, window),
            Trailing::Return => Keystroke::parse(RETURN).map(Kbd::new).ok(),
            Trailing::Nothing => None,
        };
        let frame = window.rem_size() * LIBRARY_LINE_FRAME_REMS;
        command_row(
            lead,
            label.clone(),
            key.map(IntoElement::into_any_element),
            frame,
        )
        .debug_selector({
            let selector = selector.clone();
            move || selector.clone()
        })
    })
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
