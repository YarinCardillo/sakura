//! A console on screen: the live terminal; once it ended, the terminal with one sentence and the next action
//! under it; when `box` refused, its sentence and "Try again" in place of the terminal. A login says above its
//! terminal what is happening in plain words, and when it failed why, with "Try again", "Cancel" and what it
//! printed kept below as details. The Edit menu's copy and paste reach the terminal while it has focus.

use super::login::{LoginFailure, LoginPhase};
use super::rules::is_agent_running;
use super::rules::ConsoleKind;
use super::{Console, ConsoleState};
use crate::data::facts::Row;
use crate::l10n::{say, tr, tr_with, Key};
use crate::theme::{self, Palette};
use crate::ui::centred::centred;
use crate::ui::chrome::{control, text, DETAILS};
use crate::ui::desk::Desk;
use crate::ui::task_menu::quiet_variant;
use crate::ui::verify::secondary_variant;
use gpui::{
    div, px, AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement,
    SharedString, Styled,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Copy, Paste};
use sakura_core::box_words::Failure;
use sakura_terminal::TerminalView;

/// What the pane says while its session opens.
pub const OPENING: Key = Key("console.opening");
/// What opens a console that ended once more.
const OPEN_AGAIN: Key = Key("console.open_again");

/// `task`'s console as its pane shows it; `row` is the task's facts. Without `box` (`can_open` false) a
/// refusal offers no "Try again": it would only refuse again.
pub fn view(
    row: &Row,
    console: &Console,
    can_open: bool,
    palette: &Palette,
    cx: &Context<Desk>,
) -> AnyElement {
    let task = row.task.name.as_str();
    let refused = |reason: &Failure| {
        let retry =
            can_open.then(|| next_button(Again(tr(Key("common.retry"))), task, console.kind(), cx));
        refused(reason, retry, palette)
    };
    if let ConsoleKind::Login(_) = console.kind() {
        if let Some(login) = login_view(task, console, palette, cx) {
            return login;
        }
    }
    match (console.state(), console.view()) {
        (ConsoleState::Opening, _) => opening(palette),
        (ConsoleState::Live, Some(view)) => terminal(view),
        (ConsoleState::Ended { code, at }, Some(view)) => {
            let is_detached = is_agent_running(&row.task) && row.fetched_at > *at;
            let (sentence, action) = ended(console.kind(), *code, is_detached);
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().flex_1().min_h_0().child(terminal(view)))
                .child(strip(
                    sentence,
                    [next_button(action, task, console.kind(), cx).into_any_element()],
                    palette,
                ))
                .into_any_element()
        }
        (ConsoleState::Failed { reason, .. }, _) => refused(reason),
        (_, None) | (ConsoleState::Checking { .. } | ConsoleState::LoginFailed { .. }, _) => {
            refused(&Failure {
                said: Key("console.unavailable").into(),
                details: None,
            })
        }
    }
}

/// The next action after a console ended: open the same console again, under this word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Again(&'static str);

/// The sentence and the action under a console that ended.
fn ended(kind: &ConsoleKind, code: Option<i32>, is_detached: bool) -> (SharedString, Again) {
    match kind {
        ConsoleKind::Agent if is_detached => (
            tr(Key("console.detached")).into(),
            Again(tr(Key("console.reattach"))),
        ),
        ConsoleKind::Agent => (
            tr(Key("console.connection_closed")).into(),
            Again(tr(Key("console.reattach"))),
        ),
        // A shell that ended is put away at once (see `terminal_ended`); were one ever shown ended, it
        // would say the same and offer the same.
        ConsoleKind::Shell => (ended_sentence(code), Again(tr(OPEN_AGAIN))),
        ConsoleKind::Login(_) => (
            tr(super::login::NO_REASON).into(),
            Again(tr(Key("common.retry"))),
        ),
    }
}

/// "The terminal closed", with its exit code when it is not zero.
fn ended_sentence(code: Option<i32>) -> SharedString {
    let what = tr(Key("console.closed"));
    match code {
        Some(code) if code != 0 => {
            let values = [("what", what), ("code", &code.to_string())];
            tr_with(Key("console.ended.with_code"), &values)
        }
        _ => tr_with(Key("console.ended.plain"), &[("what", what)]),
    }
}

/// The terminal of an agent that was started in it, under one line that says so: a lavoro has one front
/// end at a time, and the conversation opens when this agent stops.
pub fn in_its_terminal(terminal: AnyElement, palette: &Palette) -> AnyElement {
    let line = text(theme::TEXT_SMALL, palette.text_tertiary)
        .debug_selector(|| "agent-in-terminal".into())
        .flex_none()
        .px(theme::CONTENT_PADDING)
        .py(theme::GAP)
        .child(tr(Key("centre.agent_in_terminal")));
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(line)
        .child(div().flex_1().min_h_0().child(terminal))
        .into_any_element()
}

/// The quiet line under the centre after `task`'s own terminal ended by itself with `code`: that it
/// ended, the way to open it again, and a way to put the line away. Nothing else closed.
pub fn terminal_ended(
    task: &str,
    code: Option<i32>,
    palette: &Palette,
    cx: &Context<Desk>,
) -> AnyElement {
    let (again, put_away) = (task.to_string(), task.to_string());
    let open = Button::new("terminal-open-again")
        .debug_selector(|| "terminal-open-again".into())
        .custom(secondary_variant(cx));
    let open = control(open, tr(OPEN_AGAIN), &[tr(OPEN_AGAIN)])
        .on_click(cx.listener(move |desk, _, window, cx| desk.toggle_terminal(&again, window, cx)));
    let close = Button::new("terminal-ended-close")
        .debug_selector(|| "terminal-ended-close".into())
        .custom(quiet_variant(palette, cx));
    let close = control(close, tr(Key("common.close")), &[tr(Key("common.close"))])
        .on_click(cx.listener(move |desk, _, _, cx| desk.dismiss_terminal_ended(&put_away, cx)));
    let actions = [open.into_any_element(), close.into_any_element()];
    strip(ended_sentence(code), actions, palette)
        .debug_selector(|| "terminal-ended".into())
        .into_any_element()
}

/// The terminal, answering the Edit menu while it has focus.
fn terminal(view: &Entity<TerminalView>) -> AnyElement {
    let (copy, paste) = (view.clone(), view.clone());
    div()
        .size_full()
        .on_action(move |_: &Copy, _, cx| {
            copy.update(cx, |view, cx| {
                view.copy(cx);
            });
        })
        .on_action(move |_: &Paste, _, cx| paste.update(cx, |view, cx| view.paste(cx)))
        .child(view.clone())
        .into_any_element()
}

/// One sentence and its actions under the centre, as tall as the page header.
fn strip(
    sentence: SharedString,
    actions: impl IntoIterator<Item = AnyElement>,
    palette: &Palette,
) -> gpui::Div {
    div()
        .h(theme::HEADER_HEIGHT)
        .flex_none()
        .px(theme::CONTENT_PADDING)
        .flex()
        .items_center()
        .gap(theme::GAP)
        .border_t_1()
        .border_color(palette.border)
        .child(
            text(theme::TEXT_SMALL, palette.text_secondary)
                .flex_1()
                .min_w_0()
                .truncate()
                .child(sentence),
        )
        .children(actions)
}

/// The pane while the session opens off the UI thread: one quiet sentence, no spinner.
fn opening(palette: &Palette) -> AnyElement {
    centred(text(theme::TEXT_SMALL, palette.text_tertiary).child(tr(OPENING))).into_any_element()
}

/// Why the pane has no terminal, and "Try again" when it can help, centred in the pane. A line of `box`
/// is said in Sakura's words, with the line itself under "Dettagli"; Sakura's own sentence is shown as it is.
fn refused(reason: &Failure, retry: Option<impl IntoElement>, palette: &Palette) -> AnyElement {
    let (sentence, details) = (say(&reason.said), reason.details.clone());
    let details = details.map(|line| {
        text(theme::TEXT_LABEL, palette.text_tertiary)
            .debug_selector(|| "refusal-details".into())
            .flex()
            .flex_col()
            .gap(theme::GAP_TIGHT)
            .child(tr(DETAILS))
            .child(line)
    });
    let column = div()
        .w(px(theme::CONSOLE_COLUMN_WIDTH))
        .flex()
        .flex_col()
        .items_start()
        .gap(theme::GAP_WIDE)
        .child(text(theme::TEXT_SMALL, palette.text_secondary).child(sentence))
        .children(details)
        .children(retry);
    centred(column).into_any_element()
}

/// A login as its pane shows it: what is happening over the terminal, or why it failed over what it
/// printed. `None` for a login that could not start at all, which reads like any refusal.
fn login_view(
    task: &str,
    console: &Console,
    palette: &Palette,
    cx: &Context<Desk>,
) -> Option<AnyElement> {
    let phase = match console.state() {
        ConsoleState::Opening => LoginPhase::Opening,
        ConsoleState::Live if console.sign_in().is_some() => LoginPhase::InBrowser,
        ConsoleState::Live => LoginPhase::SigningIn,
        ConsoleState::Checking { .. } => LoginPhase::Checking,
        ConsoleState::LoginFailed { failure, .. } => {
            return Some(login_failed(failure, task, console, palette, cx));
        }
        ConsoleState::Ended { .. } | ConsoleState::Failed { .. } => return None,
    };
    let can_cancel = phase != LoginPhase::Checking;
    let cancel = can_cancel.then(|| login_button(LoginAction::Cancel, task, console.kind(), cx));
    let reopen = (phase == LoginPhase::InBrowser)
        .then(|| login_button(LoginAction::Reopen, task, console.kind(), cx));
    let header = login_bar(palette)
        .child(sentence_cell(phase.sentence().into(), palette))
        .children(reopen)
        .children(cancel);
    Some(over_terminal(header.into_any_element(), console, palette))
}

/// Why the login failed, "Try again" and "Cancel", then what the terminal printed as details.
fn login_failed(
    failure: &LoginFailure,
    task: &str,
    console: &Console,
    palette: &Palette,
    cx: &Context<Desk>,
) -> AnyElement {
    let sentence = text(theme::TEXT_SMALL, palette.text)
        .flex_1()
        .min_w_0()
        .child(say(&failure.said()));
    let header = login_bar(palette)
        .debug_selector(|| "login-failed".into())
        .child(sentence)
        .child(login_button(LoginAction::Retry, task, console.kind(), cx))
        .child(login_button(LoginAction::Cancel, task, console.kind(), cx));
    let details = text(theme::TEXT_LABEL, palette.text_tertiary)
        .debug_selector(|| "login-details".into())
        .flex_none()
        .px(theme::CONTENT_PADDING)
        .pt(theme::GAP_WIDE)
        .child(tr(DETAILS));
    let top = div().flex_none().child(header).child(details);
    over_terminal(top.into_any_element(), console, palette)
}

/// `top` over the console's terminal, which fills the rest of the pane.
fn over_terminal(top: AnyElement, console: &Console, palette: &Palette) -> AnyElement {
    let below = match console.view() {
        Some(view) => terminal(view),
        None => opening(palette),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(top)
        .child(div().flex_1().min_h_0().child(below))
        .into_any_element()
}

/// The row over a login's terminal.
fn login_bar(palette: &Palette) -> gpui::Div {
    div()
        .min_h(theme::HEADER_HEIGHT)
        .flex_none()
        .px(theme::CONTENT_PADDING)
        .flex()
        .items_center()
        .gap(theme::GAP)
        .border_b_1()
        .border_color(palette.border)
}

fn sentence_cell(sentence: SharedString, palette: &Palette) -> impl IntoElement {
    text(theme::TEXT_SMALL, palette.text_secondary)
        .flex_1()
        .min_w_0()
        .truncate()
        .child(sentence)
}

/// What a button of a login does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginAction {
    /// Runs `box login` again for the same account.
    Retry,
    /// Puts the login away; one that runs is hung up.
    Cancel,
    /// Opens the sign-in address in the browser again.
    Reopen,
}

fn login_button(
    action: LoginAction,
    task: &str,
    kind: &ConsoleKind,
    cx: &Context<Desk>,
) -> impl IntoElement {
    let (task, kind) = (task.to_string(), kind.clone());
    let (id, label) = match action {
        LoginAction::Retry => ("login-retry", tr(Key("common.retry"))),
        LoginAction::Cancel => ("login-cancel", tr(Key("common.cancel"))),
        LoginAction::Reopen => ("login-reopen", tr(Key("console.open_again"))),
    };
    let button = Button::new(id)
        .debug_selector(move || id.into())
        .custom(secondary_variant(cx));
    control(button, label, &[label]).on_click(cx.listener(move |desk, _, window, cx| {
        match action {
            LoginAction::Retry => desk.open_console(&task, kind.clone(), window, cx),
            LoginAction::Cancel => desk.close_console(&task, cx),
            LoginAction::Reopen => desk
                .consoles
                .update(cx, |consoles, cx| consoles.reopen_sign_in(&task, cx)),
        }
    }))
}

fn next_button(
    again: Again,
    task: &str,
    kind: &ConsoleKind,
    cx: &Context<Desk>,
) -> impl IntoElement {
    let (task, kind) = (task.to_string(), kind.clone());
    let button = Button::new("console-next")
        .debug_selector(|| "console-next".into())
        .custom(secondary_variant(cx));
    control(button, again.0, &[again.0]).on_click(
        cx.listener(move |desk, _, window, cx| desk.open_console(&task, kind.clone(), window, cx)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ended_agent_offers_to_attach_again_and_says_whether_it_still_works() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let (sentence, next) = ended(&ConsoleKind::Agent, Some(0), true);
        assert_eq!(sentence, "Staccato dall'agente, che continua a lavorare.");
        assert_eq!(next, Again("Riattacca"));
        let (sentence, _) = ended(&ConsoleKind::Agent, Some(0), false);
        assert_eq!(sentence, "Il collegamento all'agente si è chiuso.");
    }

    #[test]
    fn a_line_from_box_is_said_in_sakura_s_words_with_the_line_as_details() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let line = "box: no agent is running for task 'm2'";
        let refusal = sakura_core::box_words::said(line);
        assert_eq!(say(&refusal.said), "Nessun agente al lavoro qui.");
        assert_eq!(refusal.details.as_deref(), Some(line));
    }

    #[test]
    fn a_login_never_says_only_an_exit_code() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let (sentence, next) = ended(&ConsoleKind::Login("a".into()), Some(1), false);
        assert!(!sentence.contains("codice"), "{sentence}");
        assert_eq!(next, Again("Riprova"));
    }

    #[test]
    fn a_terminal_that_ended_says_so_with_its_code_only_when_it_failed() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        assert_eq!(
            ended_sentence(Some(130)),
            "Il terminale si è chiuso con codice 130."
        );
        assert_eq!(ended_sentence(Some(0)), "Il terminale si è chiuso.");
        assert_eq!(ended_sentence(None), "Il terminale si è chiuso.");
        crate::l10n::set_for_test(crate::l10n::Language::English);
        assert_eq!(ended_sentence(Some(0)), "The terminal closed.");
        let (sentence, again) = ended(&ConsoleKind::Shell, Some(1), false);
        assert_eq!(sentence, "The terminal closed with code 1.");
        assert_eq!(again, Again("Open again"));
    }
}
