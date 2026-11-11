//! The transcript: the list that follows its end, and the rows that are words only (the person's message,
//! the agent's answer, a thought, a plan, a problem). Tool calls and permission cards have their own files.

use super::states::{self, Face};
use super::{column, copy, glyphs, Phase, RowState, ThreadView};
use crate::l10n::tr;
use crate::theme::thread as tokens;
use crate::theme::{self, Palette, Theme};
use crate::ui::chrome::{tabular, text, DETAILS};
use gpui::{
    div, AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement,
    SharedString, Stateful, StatefulInteractiveElement as _, StyleRefinement, Styled, Window,
};
use gpui_base::motion::{transition, Transition};
use gpui_component::message_scroller::MessageScroller;
use gpui_component::shimmer::ShimmerText;
use gpui_component::text::TextView;
use sakura_acp::model::{PlanEntry, PlanStatus};
use sakura_acp::thread::{Entry, EntryKind, Message, Problem, ProblemKind};

/// A one-line row: a glyph, then what follows it.
pub(super) fn line_row() -> Div {
    div()
        .w_full()
        .h(tokens::LINE_ROW_HEIGHT)
        .flex()
        .items_center()
        .gap(tokens::LINE_ROW_GAP)
}

/// A label that waits: it shimmers in one direction, and stands still under reduced motion.
pub(super) fn waiting_label(words: impl Into<SharedString>, palette: &Palette) -> AnyElement {
    div()
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_tertiary)
        .child(
            ShimmerText::new(words)
                .duration(tokens::SHIMMER_SWEEP)
                .highlight_color(palette.text),
        )
        .into_any_element()
}

impl ThreadView {
    /// What fills the space above the composer: the state of an empty conversation, or the transcript.
    pub(super) fn body(
        &mut self,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_empty = self.thread.entries().is_empty();
        match &self.phase {
            Phase::Loading => self.waiting(tr(copy::LOADING), palette),
            // The conversation kept from before is about to be loaded: the view never reads as empty,
            // and says the same thing from the first frame until it is whole.
            Phase::Idle | Phase::Connecting if self.shows_kept_next(cx) => {
                self.waiting(tr(copy::LOADING), palette)
            }
            Phase::Connecting if is_empty => self.waiting(tr(copy::CONNECTING), palette),
            // The composer says what to do: the empty conversation says only where it stands.
            Phase::Idle if is_empty => states::quiet(tr(copy::NO_SESSION), None, palette),
            Phase::Ready if is_empty => states::quiet(tr(copy::EMPTY_READY), None, palette),
            Phase::SignIn | Phase::NoAccount | Phase::NoConversation if is_empty => {
                div().flex_1().into_any_element()
            }
            _ => self.transcript(palette, window, cx),
        }
    }

    /// The waiting state of an empty conversation: the connecting sign over its words while the agent
    /// connects, the words alone otherwise.
    fn waiting(&self, words: &'static str, palette: &Palette) -> AnyElement {
        let sign = self.is_connecting.then(|| self.connecting_sign());
        states::waiting(sign, words, palette)
    }

    /// The brand's sign of an agent connecting, in its own box.
    fn connecting_sign(&self) -> AnyElement {
        div()
            .debug_selector(|| "thread-sign".into())
            .flex_none()
            .child(self.sign.clone())
            .into_any_element()
    }

    fn transcript(
        &mut self,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = cx.entity().downgrade();
        let list =
            MessageScroller::new("thread", self.scroller.clone(), move |index, window, cx| {
                view.update(cx, |view, cx| view.row(index, window, cx))
                    .unwrap_or_else(|_| div().into_any_element())
            })
            .jump_button(false)
            .with_bottom_fade(palette.panel)
            .with_list_style(StyleRefinement::default().py(tokens::TRANSCRIPT_PADDING_Y))
            .with_row_style(StyleRefinement::default().px_0().pb_0());
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .child(list)
            .children(self.jump_button(palette, window, cx))
            .into_any_element()
    }

    /// "Vai in fondo", while the person has scrolled away from the end: it fades in and out.
    fn jump_button(
        &self,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let is_away = self.scroller.read(cx).is_scrolled_up();
        let target = if is_away { 1. } else { 0. };
        let fade = Transition::new(theme::DURATION_BASE);
        let opacity: f32 = transition("thread-jump", target, fade, window, cx);
        if opacity <= 0. {
            return None;
        }
        let scroller = self.scroller.clone();
        let button = states::button(
            "thread-jump-button",
            tr(copy::JUMP_TO_END),
            Face::Secondary,
            palette,
        )
        .debug_selector(|| "thread-jump".to_string())
        .on_click(move |_, _, cx| {
            scroller.update(cx, |scroller, cx| scroller.scroll_to_end(cx));
        });
        let layer = div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(tokens::ROW_GAP)
            .flex()
            .justify_center()
            .opacity(opacity)
            .child(button);
        Some(layer.into_any_element())
    }

    /// One row of the list, in the column, with the space that separates it from the row above. An
    /// answer's parts are a paragraph gap apart, as they are inside one markdown view.
    pub(super) fn row(
        &self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = Theme::palette(cx);
        let located = self.locate(row);
        let Some((entry, part)) =
            located.and_then(|(index, part)| Some((self.thread.entries().get(index)?, part)))
        else {
            return div().into_any_element();
        };
        let content = match &entry.kind {
            EntryKind::User(words) => prompt(words, &palette),
            EntryKind::Assistant(_) => self.answer_part(entry, part, &palette),
            EntryKind::Thought(message) => self.thought(entry, message, &palette, cx),
            EntryKind::Tool(call) => self.tool(entry, call, &palette, window, cx),
            EntryKind::Plan(steps) => self.plan(entry, steps, &palette, cx),
            EntryKind::Permission(permission) => self.permission(permission, &palette, cx),
            EntryKind::Problem(problem) => self.problem(entry, problem, &palette, cx),
        };
        let column = match (row, part, &entry.kind) {
            (0, _, _) => column(),
            (_, 1.., _) => column().pt(tokens::ANSWER_PART_GAP),
            (_, _, EntryKind::User(_)) => column().pt(tokens::TURN_GAP),
            _ => column().pt(tokens::ROW_GAP),
        };
        // The agent connects for the message above: its sign stands where its answer will.
        let is_last = row + 1 == self.row_count();
        let sign = (is_last && self.is_connecting)
            .then(|| div().pt(tokens::ROW_GAP).child(self.connecting_sign()));
        column.child(content).children(sign).into_any_element()
    }

    /// One part of an answer: its markdown, selectable. A link in it is the agent's text: a click opens
    /// it only through [`open_link`].
    fn answer_part(&self, entry: &Entry, part: usize, palette: &Palette) -> AnyElement {
        let view = match self.rows.get(&entry.id) {
            Some(RowState::Markdown(answer)) => answer.parts().get(part),
            _ => None,
        };
        let Some(view) = view else {
            return div().into_any_element();
        };
        let id = entry.id;
        div()
            .debug_selector(move || format!("thread-answer-{id}-{part}"))
            .w_full()
            .text_size(theme::TEXT_BODY)
            .line_height(tokens::MESSAGE_LINE_HEIGHT)
            .text_color(palette.text)
            .child(
                TextView::new(view)
                    .selectable(true)
                    .on_link_click(|address, _, _, cx| open_link(address, cx)),
            )
            .into_any_element()
    }

    /// Whether a row that opens is open: the person's choice, else `by_default`.
    pub(super) fn is_open(&self, id: u64, by_default: bool) -> bool {
        match self.rows.get(&id) {
            Some(RowState::Foldable { is_open, .. }) => is_open.unwrap_or(by_default),
            _ => by_default,
        }
    }

    /// The header of a row that opens: a click toggles it.
    pub(super) fn fold_header(
        &self,
        id: u64,
        is_open: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        line_row()
            .id(SharedString::from(format!("thread-fold-{id}")))
            .debug_selector(move || format!("thread-fold-{id}"))
            .cursor_pointer()
            .on_click(cx.listener(move |view, _, _, cx| view.toggle(id, is_open, cx)))
    }

    /// Why the conversation stopped: one plain sentence. The agent's or the system's own words are
    /// untrusted and often long, so they open under "Details" only when the person asks for them.
    fn problem(
        &self,
        entry: &Entry,
        problem: &Problem,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if problem.kind == ProblemKind::Stopped {
            // A stop is no failure: one quiet line, with no mark.
            let stopped = text(theme::TEXT_SMALL, palette.text_tertiary)
                .debug_selector(|| "thread-stopped".to_string())
                .child(tr(copy::problem_sentence(problem.kind)));
            let line = line_row().pl(tokens::LINE_ROW_INDENT).child(stopped);
            return div().w_full().child(line).into_any_element();
        }
        let sentence = line_row().child(glyphs::problem(palette.danger)).child(
            text(theme::TEXT_SMALL, palette.text)
                .debug_selector(|| "thread-problem-sentence".to_string())
                .child(tr(copy::problem_sentence(problem.kind))),
        );
        if problem.detail.is_empty() {
            return div().w_full().child(sentence).into_any_element();
        }
        let is_open = self.is_open(entry.id, false);
        let header = self
            .fold_header(entry.id, is_open, cx)
            .pl(tokens::LINE_ROW_INDENT)
            .child(text(theme::TEXT_SMALL, palette.text_tertiary).child(tr(DETAILS)))
            .child(glyphs::chevron(is_open, palette.text_tertiary));
        let detail = is_open.then(|| {
            crate::ui::chrome::mono()
                .debug_selector(|| "thread-problem-detail".to_string())
                .pl(tokens::LINE_ROW_INDENT)
                .text_size(theme::TEXT_LABEL)
                .line_height(tokens::QUIET_LINE_HEIGHT)
                .text_color(palette.text_tertiary)
                .child(copy::shown(&problem.detail))
        });
        div()
            .w_full()
            .child(sentence)
            .child(header)
            .children(detail)
            .into_any_element()
    }

    /// A thought: one row, open while it streams and closed after, unless the person chose otherwise.
    fn thought(
        &self,
        entry: &Entry,
        message: &Message,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_open = self.is_open(entry.id, message.is_streaming);
        let label = match message.is_streaming {
            true => waiting_label(tr(copy::THINKING), palette),
            false => text(theme::TEXT_SMALL, palette.text_tertiary)
                .child(tr(copy::THOUGHT))
                .into_any_element(),
        };
        let header = self
            .fold_header(entry.id, is_open, cx)
            .child(label)
            .child(glyphs::chevron(is_open, palette.text_tertiary));
        let body = is_open.then(|| {
            div()
                .pb(theme::GAP_TIGHT)
                .text_size(theme::TEXT_SMALL)
                .line_height(tokens::QUIET_LINE_HEIGHT)
                .text_color(palette.text_tertiary)
                .child(copy::shown(message.text.trim()))
        });
        div()
            .w_full()
            .child(header)
            .children(body)
            .into_any_element()
    }

    /// The plan: a row with how far it is, open by default, and its steps with the one in progress marked.
    fn plan(
        &self,
        entry: &Entry,
        steps: &[PlanEntry],
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_open = self.is_open(entry.id, true);
        let progress = text(theme::TEXT_LABEL, palette.text_tertiary)
            .font_features(tabular())
            .child(copy::plan_progress(steps));
        let header = self
            .fold_header(entry.id, is_open, cx)
            .child(text(theme::TEXT_SMALL, palette.text_secondary).child(tr(copy::PLAN)))
            .child(progress)
            .child(glyphs::chevron(is_open, palette.text_tertiary));
        let body = is_open.then(|| {
            div()
                .w_full()
                .flex()
                .flex_col()
                .children(steps.iter().map(|step| plan_step(step, palette)))
        });
        div()
            .w_full()
            .child(header)
            .children(body)
            .into_any_element()
    }
}

/// The person's message: a filled block, never an outline.
fn prompt(words: &str, palette: &Palette) -> AnyElement {
    div()
        .debug_selector(|| "thread-prompt".to_string())
        .w_full()
        .px(tokens::PROMPT_PADDING_X)
        .py(tokens::PROMPT_PADDING_Y)
        .rounded(theme::RADIUS_CARD)
        .bg(palette.fill)
        .text_size(theme::TEXT_BODY)
        .line_height(tokens::MESSAGE_LINE_HEIGHT)
        .text_color(palette.text)
        .child(copy::shown(words))
        .into_any_element()
}

/// One step of the plan: its state as a glyph, and its words in the weight of that state.
fn plan_step(step: &PlanEntry, palette: &Palette) -> AnyElement {
    let (colour, weight) = match step.status {
        PlanStatus::Pending => (palette.text_secondary, FontWeight::NORMAL),
        PlanStatus::InProgress => (palette.text, theme::WEIGHT_MEDIUM),
        PlanStatus::Completed => (palette.text_tertiary, FontWeight::NORMAL),
    };
    div()
        .w_full()
        .min_h(tokens::PLAN_ROW_HEIGHT)
        .flex()
        .items_center()
        .gap(tokens::LINE_ROW_GAP)
        .child(glyphs::plan(step.status, palette))
        .child(
            text(theme::TEXT_SMALL, colour)
                .font_weight(weight)
                .line_height(tokens::QUIET_LINE_HEIGHT)
                .min_w_0()
                .child(copy::shown(&step.content)),
        )
        .into_any_element()
}

/// Opens the address of a link the person clicked in the agent's words, when it is a web address. The
/// words of a link say nothing about where it goes, and an address may name a file or another program:
/// only `http` and `https` are ever handed to the system, by the rule terminals follow for a Cmd+click.
fn open_link(address: &str, cx: &mut gpui::App) {
    match sakura_core::links::web_url(address) {
        Some(address) => cx.open_url(&address),
        None => {
            eprintln!(
                "sakura: a link in the agent's answer is not a web address and was not opened"
            )
        }
    }
}
