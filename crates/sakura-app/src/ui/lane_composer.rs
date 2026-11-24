//! The box under a lane's conversation where the person writes to the lane: a rounded, filled box with the
//! words, the recipient and a round send button. Enter sends (inside an
//! unclosed code fence it breaks the line and Cmd+Enter sends) and Shift+Enter breaks the line. It decides
//! nothing: it tells its page what was written, and the page sends it through the engine's action. The
//! field is never focused by the box and has no outline.

use super::answers::Step;
use super::lanes::role_words;
use crate::l10n::{tr, tr_with, Key};
use crate::theme::{self, lane_page as sizes, thread as thread_sizes, Theme};
use crate::ui::enter::{guard_enter, is_composing};
use crate::ui::thread::{send_face, send_glyph, send_look};
use gpui::{
    div, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window,
};
use gpui_component::input::{InputEvent, Textarea, TextareaState};
use gpui_component::Sizable as _;

const PLACEHOLDER: Key = Key("lane_composer.placeholder");
/// What the engine's answer starts with when the message went to the role that is running.
const ANSWER_DELIVERED: &str = "delivered";
/// What it starts with when the message waits for the next role.
const ANSWER_QUEUED: &str = "queued";
/// "Delivered to the builder."
const SAID_DELIVERED: Key = Key("lane.said.delivered");
/// "Delivered." for an answer that names no role.
const SAID_DELIVERED_UNNAMED: Key = Key("lane.said.delivered_unnamed");
/// "Queued for the next role."
const SAID_QUEUED: Key = Key("lane.said.queued");

/// What the box tells its page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerEvent {
    /// The person sent this text, trimmed and not empty.
    Send(String),
}

/// The box of one lane's page.
pub struct LaneComposer {
    field: Entity<TextareaState>,
    /// The role the message goes to, when the lane has one now.
    to: Option<SharedString>,
    /// What the box says while it is empty, when the page names who the message goes to.
    placeholder: Option<SharedString>,
    /// A message is on its way: the box waits for the outcome and takes no other.
    is_busy: bool,
    /// Set once the engine took the text: the field is emptied on the next frame, which has the window.
    is_clear_due: bool,
    _subscription: Subscription,
}

impl EventEmitter<ComposerEvent> for LaneComposer {}

impl LaneComposer {
    /// An empty box. Nothing takes the focus.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(sizes::COMPOSER_MIN_ROWS, sizes::COMPOSER_MAX_ROWS)
                .placeholder(tr(PLACEHOLDER))
                .submit_on_enter(true)
        });
        let subscription = cx.subscribe_in(
            &field,
            window,
            |composer, field, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, .. } => {
                    if !is_composing(field, window, cx) {
                        composer.send(cx);
                    }
                }
                _ => cx.notify(),
            },
        );
        Self {
            field,
            to: None,
            placeholder: None,
            is_busy: false,
            is_clear_due: false,
            _subscription: subscription,
        }
    }

    /// Names the role the next message goes to; none while the lane has no role at work.
    pub fn set_to(&mut self, role: Option<SharedString>, cx: &mut Context<Self>) {
        if self.to != role {
            self.to = role;
            cx.notify();
        }
    }

    /// Says who the message goes to while the box is empty.
    pub fn set_placeholder(&mut self, words: SharedString, cx: &mut Context<Self>) {
        if self.placeholder.as_ref() != Some(&words) {
            self.placeholder = Some(words);
            cx.notify();
        }
    }

    /// While a message is on its way the box takes no other, and the field stays as it was.
    pub fn set_busy(&mut self, is_busy: bool, cx: &mut Context<Self>) {
        if self.is_busy != is_busy {
            self.is_busy = is_busy;
            cx.notify();
        }
    }

    /// What the person has typed and not sent: nothing once the field is due to be emptied.
    pub fn draft(&self, cx: &gpui::App) -> String {
        match self.is_clear_due {
            true => String::new(),
            false => self.field.read(cx).value().to_string(),
        }
    }

    /// Puts back the words a last launch left unsent.
    pub fn restore(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_string();
        self.field
            .update(cx, |field, cx| field.set_value(text, window, cx));
    }

    /// Empties the field on the next frame: the engine took what it held.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.is_clear_due = true;
        cx.notify();
    }

    /// The field the person writes in.
    #[cfg(test)]
    pub fn field(&self) -> Entity<TextareaState> {
        self.field.clone()
    }

    /// Tells the page what was written, when there is anything.
    pub fn send(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        if !text.is_empty() && !self.is_busy {
            cx.emit(ComposerEvent::Send(text));
        }
    }

    fn text(&self, cx: &Context<Self>) -> String {
        self.field.read(cx).value().trim().to_string()
    }
}

impl Render for LaneComposer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.is_clear_due) {
            self.field
                .update(cx, |field, cx| field.set_value("", window, cx));
        }
        if let Some(words) = self.placeholder.clone() {
            self.field
                .update(cx, |field, cx| field.set_placeholder(words, window, cx));
        }
        self.field
            .update(cx, |field, cx| field.set_disabled(self.is_busy, cx));
        let palette = Theme::palette(cx);
        let can_send = !self.text(cx).is_empty() && !self.is_busy;
        let to = self.to.as_ref().map(|role| {
            let words = tr_with(Key("lane_composer.to"), &[("role", role)]);
            crate::ui::chrome::text(theme::TEXT_LABEL, palette.text_secondary)
                .debug_selector(|| "lane-composer-to".into())
                .child(words)
        });
        let (_, mark) = send_look(&palette, can_send);
        let send = send_face("lane-send", &palette, can_send)
            .debug_selector(|| "lane-send".into())
            .aria_label(tr(Key("lane_composer.send")))
            .child(send_glyph(mark));
        let send = match can_send {
            true => send
                .active(|style| style.opacity(thread_sizes::PRESSED_OPACITY))
                .on_click(cx.listener(|composer, _, _, cx| composer.send(cx))),
            false => send,
        };
        div()
            .debug_selector(|| "lane-composer".into())
            .w_full()
            .max_w(sizes::COMPOSER_MAX_WIDTH)
            .pl(sizes::COMPOSER_INNER_LEFT)
            .pt(sizes::COMPOSER_INNER_TOP)
            .pr(sizes::COMPOSER_INNER_RIGHT)
            .pb(sizes::COMPOSER_INNER_BOTTOM)
            .flex()
            .flex_col()
            .gap(sizes::COMPOSER_GAP)
            .rounded(sizes::COMPOSER_RADIUS)
            .bg(palette.fill)
            .child(
                div()
                    .text_size(theme::TEXT_BODY)
                    .capture_action(guard_enter(self.field.clone()))
                    .child(
                        Textarea::new(&self.field)
                            .small()
                            .appearance(false)
                            .bordered(false),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::GAP)
                    .children(to)
                    .child(div().flex_1())
                    .child(send),
            )
    }
}

/// The step the composer shows for the engine's answer to a send: where the message went, in the interface's
/// words, when the engine's line says it was delivered or queued; any other answer as the engine said it.
/// A delivered message names the role the engine says took it, as `delivered (builder): ...`; an answer that
/// names none says "delivered" alone, never a role the feed merely last showed.
pub fn composer_answer(step: &Step) -> Step {
    let Step::Told {
        text,
        is_problem: false,
    } = step
    else {
        return step.clone();
    };
    let said = if text.starts_with(ANSWER_DELIVERED) {
        match role_named(text) {
            Some(role) => tr_with(
                SAID_DELIVERED,
                &[("role", &role_words(role).to_lowercase())],
            ),
            None => SharedString::from(tr(SAID_DELIVERED_UNNAMED)),
        }
    } else if text.starts_with(ANSWER_QUEUED) {
        SharedString::from(tr(SAID_QUEUED))
    } else {
        return step.clone();
    };
    Step::Told {
        text: said.to_string(),
        is_problem: false,
    }
}

/// The role an answer names after "delivered", as `delivered (builder): ...`; none when it names none.
fn role_named(answer: &str) -> Option<&str> {
    let named = answer.strip_prefix(ANSWER_DELIVERED)?.strip_prefix(" (")?;
    let (role, _) = named.split_once(')')?;
    let is_role = !role.is_empty() && role.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    is_role.then_some(role)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn told(text: &str) -> Step {
        Step::Told {
            text: text.to_string(),
            is_problem: false,
        }
    }

    #[test]
    fn composer_send_shows_engine_answer_word() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let to_builder = told("delivered (builder): steer for demo went to the running role");
        assert_eq!(
            composer_answer(&to_builder),
            told("Delivered to the builder.")
        );
        let to_reviewer =
            told("delivered (code_reviewer): steer for demo went to the running role");
        assert_eq!(
            composer_answer(&to_reviewer),
            told("Delivered to the code reviewer.")
        );
        let queued = told("queued: steer for demo; it is taken up by the next role");
        assert_eq!(composer_answer(&queued), told("Queued for the next role."));
    }

    #[test]
    fn a_delivered_answer_that_names_no_role_says_delivered_and_no_role() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let unnamed = told("delivered: steer for demo went to the running role");
        assert_eq!(composer_answer(&unnamed), told("Delivered."));
        let odd = told("delivered (Builder; rm -rf): steer");
        assert_eq!(
            composer_answer(&odd),
            told("Delivered."),
            "a name that is not a role is not shown"
        );
    }

    #[test]
    fn composer_answer_leaves_what_it_does_not_know_as_the_engine_said_it() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let other = told("unknown task demo");
        assert_eq!(composer_answer(&other), other);
        let problem = Step::Told {
            text: "delivered nothing".to_string(),
            is_problem: true,
        };
        assert_eq!(composer_answer(&problem), problem);
        assert_eq!(composer_answer(&Step::Running), Step::Running);
    }

    #[test]
    fn the_answer_words_exist_in_italian_too() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        assert_eq!(
            composer_answer(&told("queued: x")),
            told("In coda per il prossimo ruolo.")
        );
    }
}
