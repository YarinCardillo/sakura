//! Play: the header's control that starts and stops the run command of a task's project, and the
//! right pane's "Play" view with its log.
//!
//! One [`Play`] per open task holds both. [`header_control`] draws the control: "Play", then the
//! first port as a link and "Stop" while it runs; a press on the port emits
//! [`PlayEvent::OpenUrl`] and the app opens the browser. The entity itself renders the view: where
//! Play stands and the log, selectable, bounded, without colour codes.
//!
//! `box` is asked where Play stands once when the task is opened, and its log is read when the
//! view is shown, when Play starts or stops, and on "Refresh". `box` offers no following read, so
//! there is none, and no timer. Each asking has a number: an older answer is dropped. After a
//! failure nothing is asked again by itself.

mod source;

pub use source::{BoxPlay, PlaySource};

use crate::l10n::{say, tr, tr_with, Key};
use crate::theme::settings as sizes;
use crate::theme::{self, Palette, Theme};
use crate::ui::chrome::{failure_sentence, steady_label, text};
use gpui::{
    div, relative, AnyElement, AnyWindowHandle, App, AppContext as _, ClickEvent, Context, Div,
    Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::input::{CursorBlink, InputEvent, Position, Textarea, TextareaState};
use gpui_component::Disableable as _;
use sakura_core::error::{BridgeError, BridgeResult};
use sakura_core::play::{self, Action, Availability, Log, Outcome};
use std::rc::Rc;

/// The words the control can show: it keeps room for the widest, so a press moves nothing.
const WORDS: [Key; 4] = [
    Key("play.start"),
    Key("play.starting"),
    Key("play.stop"),
    Key("play.stopping"),
];

/// The room a port's link takes at its widest, for the header's fit.
pub const PORT_ROOM: &str = "localhost:65535";

/// The width of the widest word the control can show, measured by `width`.
pub fn widest_word(width: impl Fn(&str) -> f32) -> f32 {
    WORDS
        .map(|word| width(tr(word)))
        .into_iter()
        .fold(0.0, f32::max)
}

/// What Play tells the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayEvent {
    /// A port was pressed: the app opens this address in the browser.
    OpenUrl(String),
    /// "Project settings" was pressed where the project has no run command.
    ProjectSettingsAsked,
    /// Play started or stopped.
    Changed,
}

/// The log as the view has it.
#[derive(Debug, Clone, PartialEq)]
enum LogState {
    /// Not read yet: the view was never shown.
    Unread,
    Reading,
    Ready {
        is_empty: bool,
        is_cut: bool,
    },
    Failed(BridgeError),
}

/// The Play of one task: its control and its log view.
pub struct Play {
    source: Rc<dyn PlaySource>,
    availability: Availability,
    /// Where Play stands, as `box` last said; none before its first answer.
    state: Option<play::Play>,
    /// What was asked and is not answered yet, with the number of the asking.
    asked: Option<(Action, u64)>,
    askings: u64,
    /// What the last asking failed with.
    failure: Option<BridgeError>,
    log: LogState,
    log_reads: u64,
    is_shown: bool,
    text: Entity<TextareaState>,
    /// The window the view is in: the log's text is written in it.
    window: AnyWindowHandle,
    /// The caret policy that was in force before the log took the keyboard.
    blink_before: Option<CursorBlink>,
    _observers: [Subscription; 2],
}

impl EventEmitter<PlayEvent> for Play {}

impl Play {
    /// The Play of the task behind `source`. When the task can play, `box` is asked once where
    /// its Play stands.
    pub fn new(
        source: Rc<dyn PlaySource>,
        availability: Availability,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = cx.new(|cx| {
            let state = TextareaState::new(window, cx);
            state.soft_wrap(false).searchable(false)
        });
        let on_keyboard = cx.subscribe(&text, Self::keyboard_changed);
        let on_release = cx.on_release(|play, cx| play.restore_caret(cx));
        let mut play = Self {
            source,
            availability,
            state: None,
            asked: None,
            askings: 0,
            failure: None,
            log: LogState::Unread,
            log_reads: 0,
            is_shown: false,
            text,
            window: window.window_handle(),
            blink_before: None,
            _observers: [on_keyboard, on_release],
        };
        if availability == Availability::Ready {
            play.ask(Action::Status, cx);
        }
        play
    }

    /// Whether the service said where Play stands and nothing is being asked: a press is taken.
    #[cfg(test)]
    pub fn is_at_rest(&self) -> bool {
        self.state.is_some() && self.asked.is_none()
    }

    pub fn is_running(&self) -> bool {
        self.state.as_ref().is_some_and(play::Play::is_running)
    }

    /// Whether the task can play changed: its project got a run command, or lost it.
    pub fn set_availability(&mut self, availability: Availability, cx: &mut Context<Self>) {
        if self.availability == availability {
            return;
        }
        self.availability = availability;
        if availability == Availability::Ready && self.state.is_none() {
            self.ask(Action::Status, cx);
        }
        cx.notify();
    }

    /// Starts the run command.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        self.ask(Action::Start, cx);
    }

    /// Stops it.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.ask(Action::Stop, cx);
    }

    /// The "Play" view came on screen or left it. Coming on screen reads the log.
    pub fn shown(&mut self, is_shown: bool, cx: &mut Context<Self>) {
        let came = is_shown && !self.is_shown;
        self.is_shown = is_shown;
        if came && self.availability == Availability::Ready {
            self.read_log(cx);
        }
    }

    /// "Refresh": where Play stands and its log are read again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.availability == Availability::Ready && self.asked.is_none() {
            self.ask(Action::Status, cx);
            self.read_log(cx);
        }
    }

    /// Asks `box` one thing; while it is not answered nothing else is asked.
    fn ask(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.asked.is_some() || self.availability != Availability::Ready {
            return;
        }
        self.askings += 1;
        let asking = self.askings;
        self.asked = Some((action, asking));
        self.failure = None;
        let answer = self.source.play(action);
        cx.spawn(async move |play, cx| {
            let answer = answer.await;
            drop(play.update(cx, |play, cx| play.answered(action, asking, answer, cx)));
        })
        .detach();
        cx.notify();
    }

    fn answered(
        &mut self,
        action: Action,
        asking: u64,
        answer: BridgeResult<Outcome>,
        cx: &mut Context<Self>,
    ) {
        if self.asked != Some((action, asking)) {
            return;
        }
        self.asked = None;
        match answer {
            Ok(Outcome::State(state)) => {
                let changed = self.is_running() != state.is_running();
                self.state = Some(state);
                if changed || action != Action::Status {
                    cx.emit(PlayEvent::Changed);
                    // A start or a stop wrote to the log: it is read again when it is on screen.
                    if self.is_shown {
                        self.read_log(cx);
                    }
                }
            }
            Ok(Outcome::NotInBox) => self.availability = Availability::InBox,
            Err(error) => self.failure = Some(error),
        }
        cx.notify();
    }

    /// Reads the log once. What was shown stays until the answer is there.
    fn read_log(&mut self, cx: &mut Context<Self>) {
        self.log_reads += 1;
        let read = self.log_reads;
        if !matches!(self.log, LogState::Ready { .. }) {
            self.log = LogState::Reading;
        }
        let answer = self.source.log();
        cx.spawn(async move |play, cx| {
            let answer = answer.await;
            drop(play.update(cx, |play, cx| {
                if play.log_reads == read {
                    play.log_read(answer, cx);
                }
            }));
        })
        .detach();
        cx.notify();
    }

    fn log_read(&mut self, answer: BridgeResult<Log>, cx: &mut Context<Self>) {
        match answer {
            Ok(log) => {
                self.log = LogState::Ready {
                    is_empty: log.lines().is_empty(),
                    is_cut: log.is_cut(),
                };
                self.write_log(&log, cx);
            }
            Err(error) => self.log = LogState::Failed(error),
        }
        cx.notify();
    }

    /// Puts the log in the text view, its newest line in sight.
    fn write_log(&mut self, log: &Log, cx: &mut Context<Self>) {
        let (text, body) = (self.text.clone(), log.text());
        let last = u32::try_from(log.lines().len().saturating_sub(1)).unwrap_or(u32::MAX);
        drop(self.window.update(cx, |_, window, cx| {
            text.update(cx, |text, cx| {
                text.set_value(body, window, cx);
                text.set_cursor_position(Position::new(last, 0), window, cx);
            });
        }));
    }

    /// While the log has the keyboard its caret does not blink (the kit's policy is app-wide):
    /// a view nobody types in draws nothing at rest. The policy in force before comes back when
    /// the log gives the keyboard up.
    fn keyboard_changed(
        &mut self,
        _: Entity<TextareaState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Focus if self.blink_before.is_none() => {
                let steady = |theme: &mut gpui_component::Theme| {
                    std::mem::replace(&mut theme.cursor_blink, CursorBlink::steady())
                };
                self.blink_before = Some(gpui_component::Theme::update(cx, steady));
            }
            InputEvent::Blur => self.restore_caret(cx),
            _ => {}
        }
    }

    fn restore_caret(&mut self, cx: &mut App) {
        if let Some(before) = self.blink_before.take() {
            gpui_component::Theme::update(cx, |theme| theme.cursor_blink = before);
        }
    }

    /// The word the control shows now.
    fn word(&self) -> Key {
        match (self.asked, self.is_running()) {
            (Some((Action::Start, _)), _) => Key("play.starting"),
            (Some((Action::Stop, _)), _) => Key("play.stopping"),
            (_, true) => Key("play.stop"),
            (_, false) => Key("play.start"),
        }
    }

    /// Why the task cannot play, when it cannot.
    fn unavailable(&self) -> Option<Key> {
        match self.availability {
            Availability::Ready => None,
            Availability::InBox => Some(Key("play.unavailable.box")),
            Availability::NoRunCommand => Some(Key("play.unavailable.no_command")),
        }
    }

    /// The port the running command listens on.
    fn running_port(&self) -> Option<u16> {
        let running = self.state.as_ref().filter(|state| state.is_running());
        running.and_then(play::Play::first_port)
    }

    /// The view's first row: where Play stands, and "Refresh".
    fn bar(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let standing = match (self.unavailable(), self.is_running()) {
            (Some(_), _) => None,
            (None, true) => Some(tr(Key("play.state.running"))),
            (None, false) => Some(tr(Key("play.state.stopped"))),
        };
        let refresh = cx.listener(|play, _: &ClickEvent, _, cx| play.refresh(cx));
        let ink = palette.text;
        let refresh = div()
            .id("play-refresh")
            .debug_selector(|| "play-refresh".into())
            .flex_none()
            .text_size(theme::TEXT_SMALL)
            .text_color(palette.text_secondary)
            .hover(move |style| style.text_color(ink))
            .on_click(refresh)
            .child(tr(Key("play.refresh")));
        div()
            .h(theme::TOOLBAR_HEIGHT)
            .flex_none()
            .px(theme::SIDE_PANE_INSET)
            .flex()
            .items_center()
            .gap(theme::GAP)
            .children(standing.map(|standing| {
                text(theme::TEXT_SMALL, palette.text_secondary)
                    .flex_none()
                    .child(standing)
            }))
            .children(
                self.running_port()
                    .map(|port| port_link("play-view-port", port, cx.entity(), palette)),
            )
            .child(div().flex_1())
            .children(self.unavailable().is_none().then_some(refresh))
            .into_any_element()
    }

    /// One sentence in the view's body, with what follows it.
    fn sentence(
        &self,
        id: &'static str,
        sentence: AnyElement,
        after: Option<AnyElement>,
    ) -> AnyElement {
        div()
            .debug_selector(move || id.into())
            .px(theme::SIDE_PANE_INSET)
            .flex()
            .flex_col()
            .items_start()
            .gap(theme::GAP)
            .child(sentence)
            .children(after)
            .into_any_element()
    }

    /// The body: why there is no Play, or the log.
    fn body(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let quiet = |key: Key| {
            text(theme::TEXT_SMALL, palette.text_tertiary)
                .child(tr(key))
                .into_any_element()
        };
        if let Some(why) = self.unavailable() {
            let settings = (self.availability == Availability::NoRunCommand).then(|| {
                let ask = cx
                    .listener(|_, _: &ClickEvent, _, cx| cx.emit(PlayEvent::ProjectSettingsAsked));
                let ink = palette.text;
                div()
                    .id("play-project-settings")
                    .debug_selector(|| "play-project-settings".into())
                    .text_size(theme::TEXT_SMALL)
                    .text_color(palette.text_secondary)
                    .hover(move |style| style.text_color(ink))
                    .on_click(ask)
                    .child(tr(Key("play.project_settings")))
                    .into_any_element()
            });
            return self.sentence("play-unavailable", quiet(why), settings);
        }
        match &self.log {
            LogState::Unread | LogState::Reading => {
                self.sentence("play-log-reading", quiet(Key("play.log.reading")), None)
            }
            LogState::Failed(error) => {
                self.sentence("play-log-failed", failure(error, palette), None)
            }
            LogState::Ready { is_empty: true, .. } => {
                self.sentence("play-log-empty", quiet(Key("play.log.empty")), None)
            }
            LogState::Ready { is_cut, .. } => self.log_text(*is_cut, palette),
        }
    }

    /// The log: its text, selectable, in the monospaced face; a line above it when older output
    /// was let go. The kit's text view keeps its own padding inside: the rest of the pane's inset
    /// is added around it, so the log starts on the line of the bar above.
    fn log_text(&self, is_cut: bool, palette: &Palette) -> AnyElement {
        let cut = is_cut.then(|| {
            text(theme::TEXT_LABEL, palette.text_tertiary)
                .px(theme::SIDE_PANE_INSET)
                .pb(theme::GAP_TIGHT)
                .child(tr_with(
                    Key("play.log.cut"),
                    &[("lines", &play::LOG_MAX_LINES.to_string())],
                ))
        });
        let log = Textarea::new(&self.text)
            .readonly(true)
            .appearance(false)
            .bordered(false)
            .h(relative(1.))
            .px(theme::SIDE_PANE_INSET - gpui_component::Size::default().input_px())
            .font_family(theme::FONT_MONO)
            .font_features(crate::ui::chrome::mono_features())
            .text_size(sizes::PLAY_LOG_TEXT)
            .line_height(sizes::PLAY_LOG_LINE_HEIGHT)
            .text_color(palette.text_secondary);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .children(cut)
            .child(
                div()
                    .debug_selector(|| "play-log".into())
                    .flex_1()
                    .min_h_0()
                    .child(log),
            )
            .into_any_element()
    }
}

/// A failure said in place: Sakura's sentence, with what `box` wrote as its details.
fn failure(error: &BridgeError, palette: &Palette) -> AnyElement {
    failure_sentence("play-failure", say(&error.said), error.details.as_deref())
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.danger)
        .into_any_element()
}

impl Render for Play {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let failed = self.failure.as_ref().map(|error| {
            div()
                .debug_selector(|| "play-failed".into())
                .px(theme::SIDE_PANE_INSET)
                .pb(theme::GAP)
                .child(failure(error, &palette))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.bar(&palette, cx))
            .children(failed)
            .child(self.body(&palette, cx))
    }
}

/// The secondary button: a filled box a step above its surface, never an outline.
fn secondary(palette: &Palette, cx: &App) -> ButtonCustomVariant {
    ButtonCustomVariant::new(cx)
        .color(palette.fill)
        .foreground(palette.text)
        .hover(palette.fill_hover)
        .active(palette.fill_hover)
}

/// The header's control for `play`: "Play", or the first port and "Stop" while it runs, the port in
/// room that is kept for it. Where the
/// task cannot play the control is there, disabled, and says why under the pointer; the "Play"
/// view says it in full, with the link to the project's settings.
pub fn header_control(play: &Entity<Play>, cx: &App) -> impl IntoElement {
    let palette = Theme::palette(cx);
    let state = play.read(cx);
    let words = WORDS.map(tr);
    let label = steady_label(tr(state.word()), &words)
        .text_size(theme::TEXT_SMALL)
        .font_weight(theme::WEIGHT_MEDIUM);
    let is_running = state.is_running();
    let press = {
        let play = play.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            play.update(cx, |play, cx| {
                if is_running {
                    play.stop(cx)
                } else {
                    play.start(cx)
                }
            });
        }
    };
    let why: Option<SharedString> = state.unavailable().map(|key| tr(key).into());
    let button = Button::new("play-control")
        .debug_selector(|| "play-control".into())
        .custom(secondary(&palette, cx))
        .h(theme::CONTROL_HEIGHT)
        .px(theme::CONTROL_PADDING_X)
        .child(label)
        .disabled(why.is_some() || state.asked.is_some())
        .on_click(press);
    let can_play = why.is_none();
    let button = match why {
        Some(why) => button.tooltip(why),
        None => button,
    };
    let port = state
        .running_port()
        .map(|port| port_link("play-port", port, play.clone(), &palette));
    div()
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .children(can_play.then(|| port_room(port)))
        .child(button)
}

/// The room of a port's link at its widest, with the running port at its right end. The room is
/// there whenever the lavoro can play, so a start moves nothing beside the control.
fn port_room(port: Option<AnyElement>) -> Div {
    let room = div().h(Pixels::ZERO).overflow_hidden().child(PORT_ROOM);
    div()
        .flex_none()
        .flex()
        .flex_col()
        .items_end()
        .whitespace_nowrap()
        .text_size(theme::TEXT_SMALL)
        .font_features(crate::ui::chrome::tabular())
        .children(port)
        .child(room)
}

/// A port as a link: a press asks the app to open it in the browser.
fn port_link(id: &'static str, port: u16, play: Entity<Play>, palette: &Palette) -> AnyElement {
    let ink = palette.text;
    div()
        .id(id)
        .debug_selector(move || id.into())
        .flex_none()
        .text_size(theme::TEXT_SMALL)
        .font_features(crate::ui::chrome::tabular())
        .text_color(palette.text_secondary)
        .hover(move |style| style.text_color(ink))
        .on_click(move |_, _, cx| {
            play.update(cx, |_, cx| cx.emit(PlayEvent::OpenUrl(play::url(port))));
        })
        .child(format!("localhost:{port}"))
        .into_any_element()
}

#[cfg(test)]
mod tests;
