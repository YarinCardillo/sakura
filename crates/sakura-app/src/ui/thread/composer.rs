//! Under the transcript: what the person must do when the agent cannot go on (sign in, restart, take up
//! the stored conversation, use the terminal), the line that says the agent is starting or working and
//! for how long, and the composer. Enter sends, Shift+Enter breaks the
//! line; while a turn runs the button stops it. The field is never focused by the view and has no outline.

use super::session::{Login, Readiness};
use super::states::{self, Face};
use super::{column, copy, glyphs, rows, Phase, ThreadView};
use crate::l10n::{tr, tr_with};
use crate::theme::thread as tokens;
use crate::theme::{self, Palette};
use crate::ui::chrome::{mono, tabular, text, DETAILS};
use gpui::{
    div, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled, Window,
};
use gpui_component::input::Textarea;
use gpui_component::Sizable as _;
use sakura_acp::thread::Turn;

impl ThreadView {
    pub(super) fn footer(
        &mut self,
        palette: &Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        column()
            .flex_none()
            .pb(tokens::COLUMN_PADDING_X)
            .flex()
            .flex_col()
            .children(self.notice(palette, cx))
            .child(self.status_line(palette, cx))
            .child(self.composer_box(palette, cx))
            .into_any_element()
    }

    /// What only the person can do next, when the agent cannot go on by itself: sign in, name an account,
    /// restart, or use the terminal where no conversation can be held; and what became of a conversation
    /// kept from before that could not be shown again.
    fn notice(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (words, buttons) = match &self.phase {
            Phase::Preparing => return self.creation_failure(palette, cx),
            Phase::SignIn => {
                let login = (self.host.login)(cx);
                let buttons = self.sign_in_buttons(&login, palette, cx);
                (self.sign_in_words(&login, palette), buttons)
            }
            Phase::NoAccount => self.account_question(palette, cx),
            Phase::Ended(_) => (None, vec![self.restart_button(palette, cx)]),
            Phase::NoConversation => {
                let sentence =
                    text(theme::TEXT_BODY, palette.text).child(tr(copy::NO_CONVERSATION));
                (Some(sentence), vec![self.terminal_button(palette, cx)])
            }
            Phase::Ready if self.is_kept_lost => (Some(self.kept_lost(palette)), Vec::new()),
            _ => return None,
        };
        let notice = div()
            .debug_selector(|| "thread-notice".to_string())
            .w_full()
            .pb(tokens::ROW_GAP)
            .flex()
            .flex_col()
            .gap(tokens::CARD_GAP)
            .children(words)
            .children(
                (!buttons.is_empty())
                    .then(|| div().flex().gap(tokens::BUTTON_GAP).children(buttons)),
            );
        Some(notice.into_any_element())
    }

    /// The lavoro of the first message could not be made: why, in the interface's words, "Try again",
    /// and what the service itself wrote for who asks for it. The message stays where it is.
    fn creation_failure(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Readiness::Failed { said, details } = (self.host.readiness)(cx) else {
            return None;
        };
        let retry = states::button("thread-retry", tr(copy::RETRY), Face::Secondary, palette)
            .debug_selector(|| "thread-retry".to_string())
            .on_click(cx.listener(|view, _, window, cx| {
                view.shows_details = false;
                (view.host.retry_creation)(window, cx);
                cx.notify();
            }));
        let more = details.is_some().then(|| {
            states::button("thread-details", tr(DETAILS), Face::Quiet, palette)
                .debug_selector(|| "thread-details".to_string())
                .on_click(cx.listener(|view, _, _, cx| {
                    view.shows_details = !view.shows_details;
                    cx.notify();
                }))
        });
        let raw = details.filter(|_| self.shows_details).map(|details| {
            mono()
                .debug_selector(|| "thread-details-text".to_string())
                .text_size(theme::TEXT_LABEL)
                .text_color(palette.text_tertiary)
                .child(details)
        });
        let notice = div()
            .debug_selector(|| "thread-notice".to_string())
            .w_full()
            .pb(tokens::ROW_GAP)
            .flex()
            .flex_col()
            .gap(tokens::CARD_GAP)
            .child(
                text(theme::TEXT_BODY, palette.text)
                    .debug_selector(|| "thread-creation-failed".to_string())
                    .child(said),
            )
            .child(
                div()
                    .flex()
                    .gap(tokens::BUTTON_GAP)
                    .child(retry)
                    .children(more),
            )
            .children(raw);
        Some(notice.into_any_element())
    }

    /// Why a sign-in is needed and what to do about it. An account of the boxes signs in from here; the
    /// person's own login on this Mac is named, because it is renewed outside Sakura.
    fn sign_in_words(&self, login: &Login, palette: &Palette) -> Option<gpui::Div> {
        let (sentence, hint) = match login {
            Login::Box => (tr(copy::SIGN_IN_NEEDED).into(), tr(copy::SIGN_IN_HINT)),
            Login::Mac { account } => (
                copy::login_expired(account.as_deref()),
                tr(copy::LOGIN_EXPIRED_HINT),
            ),
        };
        let sentence: SharedString = sentence;
        let sentence = text(theme::TEXT_BODY, palette.text)
            .debug_selector(|| "thread-sign-in-sentence".to_string())
            .child(sentence);
        let words = div()
            .flex()
            .flex_col()
            .gap(theme::GAP_TIGHT)
            .child(sentence)
            .child(text(theme::TEXT_SMALL, palette.text_tertiary).child(hint));
        Some(words)
    }

    /// No account is named for the agent: which of those there are to use, a button each. With none to
    /// choose from, that is said, with "Try again" for when one was added meanwhile.
    fn account_question(
        &self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> (Option<gpui::Div>, Vec<Stateful<gpui::Div>>) {
        let accounts = (self.host.accounts)(cx);
        let sentence = match accounts.is_empty() {
            true => copy::NO_ACCOUNT_AT_ALL,
            false => copy::NO_ACCOUNT,
        };
        let words = text(theme::TEXT_BODY, palette.text)
            .debug_selector(|| "thread-no-account".to_string())
            .child(tr(sentence));
        let retry = || {
            states::button("thread-retry", tr(copy::RETRY), Face::Secondary, palette)
                .debug_selector(|| "thread-retry".to_string())
                .on_click(cx.listener(|view, _, _, cx| view.retry(cx)))
        };
        let buttons = match accounts.is_empty() {
            true => vec![retry()],
            false => accounts
                .into_iter()
                .map(|account| self.account_button(account, palette, cx))
                .collect(),
        };
        (Some(words), buttons)
    }

    fn account_button(
        &self,
        account: String,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let (id, name) = (format!("thread-account-{account}"), account.clone());
        let label = SharedString::from(copy::shown(&account));
        states::button(
            SharedString::from(id.clone()),
            label,
            Face::Secondary,
            palette,
        )
        .debug_selector(move || id.clone())
        .on_click(cx.listener(move |view, _, _, cx| (view.host.choose_account)(&name, cx)))
    }

    /// "Sign in" for an account of the boxes, and "Try again" for both.
    fn sign_in_buttons(
        &self,
        login: &Login,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<gpui::Div>> {
        let retry = states::button("thread-retry", tr(copy::RETRY), Face::Secondary, palette)
            .debug_selector(|| "thread-retry".to_string())
            .on_click(cx.listener(|view, _, _, cx| view.retry(cx)));
        if *login != Login::Box {
            return vec![retry];
        }
        let sign_in = states::button("thread-sign-in", tr(copy::SIGN_IN), Face::Primary, palette)
            .debug_selector(|| "thread-sign-in".to_string())
            .on_click(cx.listener(|view, _, window, cx| (view.host.sign_in)(window, cx)));
        vec![sign_in, retry]
    }

    fn restart_button(&self, palette: &Palette, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        states::button(
            "thread-restart",
            tr(copy::RESTART),
            Face::Secondary,
            palette,
        )
        .debug_selector(|| "thread-restart".to_string())
        .on_click(cx.listener(|view, _, _, cx| view.connect(cx)))
    }

    /// The conversation kept for the lavoro is gone where the agent keeps it: a new one is open, and
    /// one line says so until the person writes.
    fn kept_lost(&self, palette: &Palette) -> gpui::Div {
        text(theme::TEXT_SMALL, palette.text_tertiary)
            .debug_selector(|| "thread-kept-lost".to_string())
            .child(tr(copy::KEPT_LOST))
    }

    fn terminal_button(&self, palette: &Palette, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        let label = tr(copy::OPEN_TERMINAL);
        states::button("thread-terminal", label, Face::Secondary, palette)
            .debug_selector(|| "thread-terminal".to_string())
            .on_click(cx.listener(|view, _, window, cx| (view.host.open_terminal)(window, cx)))
    }

    /// While a turn runs: what the agent is doing, in a word, and the time the turn has taken. The line
    /// keeps its height when idle, so the composer does not move when a turn starts.
    fn status_line(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let line = div()
            .debug_selector(|| "thread-status".to_string())
            .w_full()
            .h(tokens::STATUS_LINE_HEIGHT)
            .flex()
            .items_center()
            .gap(tokens::LINE_ROW_GAP);
        let is_starting = matches!(self.phase, Phase::Connecting | Phase::Loading);
        let preparing = (self.phase == Phase::Preparing).then(|| (self.host.readiness)(cx));
        let label = match (self.thread.turn(), self.waits_for_person) {
            // The lavoro is being made: the service's own line says how far it is. A creation that
            // failed is said above, with what to do about it.
            _ if matches!(preparing, Some(Readiness::Failed { .. })) => {
                return line.into_any_element()
            }
            // The message waits for an account: the question above says so, nothing works yet.
            _ if self.phase == Phase::NoAccount => return line.into_any_element(),
            _ if preparing.is_some() => {
                let words = match preparing {
                    Some(Readiness::Preparing(Some(progress))) => {
                        tr_with(copy::PREPARING_LINE, &[("line", &progress)])
                    }
                    _ => tr(copy::PREPARING).into(),
                };
                rows::waiting_label(words, palette)
            }
            // While the person is being asked the card says so: this line says nothing a second time.
            (Turn::Idle, _) | (_, true) => return line.into_any_element(),
            (Turn::Running, false) if is_starting => {
                rows::waiting_label(tr(copy::CONNECTING), palette)
            }
            (Turn::Running, false) => rows::waiting_label(tr(copy::WORKING), palette),
            (Turn::Cancelling, false) => rows::waiting_label(tr(copy::STOPPING), palette),
        };
        let elapsed = self.turn_started.map(|started| {
            text(theme::TEXT_LABEL, palette.text_tertiary)
                .font_features(tabular())
                .child(copy::elapsed(started.elapsed()))
        });
        line.child(label).children(elapsed).into_any_element()
    }

    fn composer_box(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let field = div()
            .debug_selector(|| "thread-composer".to_string())
            .flex_1()
            .min_w_0()
            .min_h(tokens::SEND_BUTTON_SIZE)
            .flex()
            .items_center()
            .text_size(theme::TEXT_BODY)
            .child(
                Textarea::new(&self.composer)
                    .small()
                    .appearance(false)
                    .bordered(false),
            );
        div()
            .debug_selector(|| "thread-composer-box".to_string())
            .w_full()
            .p(tokens::COMPOSER_PADDING)
            .flex()
            .items_end()
            .gap(tokens::LINE_ROW_GAP)
            .rounded(tokens::COMPOSER_RADIUS)
            .bg(palette.fill)
            .child(field)
            .child(self.send_button(palette, cx))
            .into_any_element()
    }

    /// The round button at the composer's corner: send, or stop while a turn runs. It is a mark with no
    /// word, so it carries its name for who cannot see it.
    fn send_button(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let is_running = self.can_stop();
        let name = if is_running { copy::STOP } else { copy::SEND };
        let face = send_face("thread-send", palette)
            .debug_selector(|| "thread-send".to_string())
            .aria_label(tr(name));
        if is_running {
            return face
                .cursor_pointer()
                .active(|style| style.opacity(tokens::PRESSED_OPACITY))
                .on_click(cx.listener(|view, _, _, cx| view.stop(cx)))
                .child(glyphs::stop(palette.primary_foreground))
                .into_any_element();
        }
        let mark = glyphs::send(palette.primary_foreground);
        if !self.can_send(cx) {
            return face
                .opacity(tokens::SEND_BUTTON_DISABLED_OPACITY)
                .child(mark)
                .into_any_element();
        }
        face.cursor_pointer()
            .active(|style| style.opacity(tokens::PRESSED_OPACITY))
            .on_click(cx.listener(|view, _, window, cx| view.send(window, cx)))
            .child(mark)
            .into_any_element()
    }
}

/// The round face of a send button, in the action colour, with no mark and no behaviour yet: the
/// conversation's composer and "New task" both send with it.
pub fn send_face(id: &'static str, palette: &Palette) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .role(gpui::Role::Button)
        .size(tokens::SEND_BUTTON_SIZE)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(palette.primary)
}
