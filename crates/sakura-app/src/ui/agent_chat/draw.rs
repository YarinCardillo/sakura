//! What an agent's conversation looks like: its header, its rows in the master chat's own look (the person's
//! messages as filled blocks, the agent's as markdown, tools as quiet lines with their time, the text being
//! written with the stream fade), and under them the box or the words of why there is none.

use super::rows::{folded, result_words, verdict_of, Row, ToolRow, Verdict};
use super::AgentChat;
use crate::icons;
use crate::l10n::{tr, Key};
use crate::theme::kit::scale::{Typeset as _, ICON_REGULAR, SPACE_S, TEXT_FOOTNOTE, WEIGHT_MEDIUM};
use crate::theme::thread as tokens;
use crate::theme::{self, lane_page as sizes, Palette};
use crate::ui::agent_words::{model_words, state_glyph, state_words, tool_words};
use crate::ui::chrome::{mono, tabular, text};
use crate::ui::inert_markdown::{inert, open};
use crate::ui::kit::{self, ButtonKind};
use crate::ui::lane_agents::round_words;
use crate::ui::lanes::role_words;
use crate::ui::unix_now as now;
use gpui::prelude::FluentBuilder as _;
use gpui::AppContext as _;
use gpui::{
    div, AnyElement, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, StyleRefinement,
    Styled as _, Window,
};
use gpui_component::message_scroller::MessageScroller;
use gpui_component::text::{TextView, TextViewState};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;
use sakura_core::engine::agents::{AgentRun, Listening, RunState};

impl AgentChat {
    /// The whole view: the header, the conversation and the box.
    pub(super) fn chat(
        &mut self,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let run = self.run_of(cx);
        let header = self.header(run.as_ref(), palette, cx);
        let body = self.conversation(palette, cx);
        let problem = self.read_problem(palette);
        let footer = self.footer(run.as_ref(), palette, window, cx);
        div()
            .id("agent-chat")
            .debug_selector(|| "agent-chat".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
            .children(problem)
            .child(footer)
            .into_any_element()
    }

    /// Back, the role, the model and the account, and the state: who the person is talking to.
    fn header(
        &self,
        run: Option<&AgentRun>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let back = kit::button_icon(
            "agent-chat-back",
            tr(Key("agent_chat.back")),
            icons::BACK,
            ButtonKind::Quiet,
            palette,
        )
        .on_click(cx.listener(|_, _, _, cx| cx.emit(super::AgentChatEvent::Back)));
        let who = run.map(|run| who_view(run, super::shown_state(run.state, self.state), palette));
        div()
            .debug_selector(|| "agent-chat-header".into())
            .flex_none()
            .flex()
            .items_center()
            .gap(theme::GAP_WIDE)
            .pl(sizes::HEADER_PADDING_X - crate::theme::kit::scale::SPACE_M)
            .pr(sizes::HEADER_PADDING_X)
            .py(SPACE_S)
            .border_b_1()
            .border_color(palette.border)
            .child(back)
            .children(who)
            .into_any_element()
    }

    fn conversation(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.rows.is_empty() && self.live.is_none() {
            return self.quiet(palette);
        }
        self.page_older_rows(cx);
        let view = cx.entity().downgrade();
        let shown = *palette;
        let list = MessageScroller::new(
            "agent-chat-list",
            self.scroller.clone(),
            move |index, _, cx| {
                view.update(cx, |chat, cx| {
                    chat.row(index + chat.first_shown, &shown, cx)
                })
                .unwrap_or_else(|_| div().into_any_element())
            },
        )
        .jump_button(true)
        .with_jump_button_label(tr(crate::ui::thread::JUMP_TO_END))
        .with_bottom_fade(palette.panel)
        .with_list_style(StyleRefinement::default().py(tokens::TRANSCRIPT_PADDING_Y))
        .with_row_style(StyleRefinement::default().px_0().pb_0());
        div()
            .debug_selector(|| "agent-chat-rows".into())
            .track_focus(&self.focus)
            .capture_any_mouse_up(cx.listener(|chat, _, window, cx| window.focus(&chat.focus, cx)))
            .relative()
            .size_full()
            .min_h_0()
            .child(list)
            .child(self.drag_scroll.watcher())
            .into_any_element()
    }

    /// Beside a conversation that has content, the words of why the last read failed: what is shown stays, and
    /// the reading goes on from the last good cursor. Without content the words stand in its place instead.
    fn read_problem(&self, palette: &Palette) -> Option<AnyElement> {
        let has_content = !self.rows.is_empty() || self.live.is_some();
        let words = self.problem.clone().filter(|_| has_content)?;
        Some(
            text(theme::TEXT_SMALL, palette.text_secondary)
                .debug_selector(|| "agent-chat-problem".into())
                .flex_none()
                .px(sizes::FEED_PADDING_X)
                .py(theme::GAP)
                .child(words)
                .into_any_element(),
        )
    }

    /// What stands where the conversation will be: why there is nothing yet, or why it could not be read.
    fn quiet(&self, palette: &Palette) -> AnyElement {
        let words = match &self.problem {
            Some(problem) => SharedString::from(problem.clone()),
            None => tr(Key("agent_chat.empty")).into(),
        };
        text(theme::TEXT_SMALL, palette.text_tertiary)
            .debug_selector(|| "agent-chat-quiet".into())
            .px(sizes::FEED_PADDING_X)
            .py(theme::GAP_WIDE)
            .child(words)
            .into_any_element()
    }

    /// The row at `index`: an entry, or the text being written after the last one.
    fn row(&mut self, index: usize, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let content = match self.rows.get(index).cloned() {
            Some(row) => self.content(index, row, palette, cx),
            None if index == self.rows.len() => self.live_text(palette),
            None => return div().into_any_element(),
        };
        let gap = match (index, self.rows.get(index)) {
            (0, _) => gpui::px(0.),
            (_, Some(Row::You(_))) => tokens::TURN_GAP,
            _ => tokens::ROW_GAP,
        };
        let (selector, column) = (format!("chat-row-{index}"), format!("chat-column-{index}"));
        div()
            .debug_selector(move || selector)
            .w_full()
            .px(sizes::FEED_PADDING_X)
            .pt(gap)
            .flex()
            .justify_center()
            .child(
                div()
                    .debug_selector(move || column)
                    .w_full()
                    .max_w(sizes::FEED_MAX_WIDTH)
                    .child(content),
            )
            .into_any_element()
    }

    fn content(
        &mut self,
        index: usize,
        row: Row,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match row {
            Row::You(words) => {
                let selector = format!("chat-you-{index}");
                let block = crate::ui::thread::prompt(index as u64, &words, true, palette);
                div()
                    .debug_selector(move || selector)
                    .child(block)
                    .into_any_element()
            }
            Row::Words { text, is_cut } => self.answer(index, &text, is_cut, palette, cx),
            Row::Tool(tool) => tool_line(index, &tool, palette),
            Row::Engine(words) => engine_line(index, &words, palette),
            Row::Result { verdict, summary } => result_card(index, &verdict, &summary, palette),
            Row::Round(round) => round_divider(index, round, palette),
        }
    }

    /// What the agent wrote: markdown made inert, folded past a dozen lines with "Show all" under it, and a
    /// line under a message the engine cut at its limit.
    fn answer(
        &mut self,
        index: usize,
        words: &str,
        is_cut: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The last message of the agent is the one being read: it is open. Only the older ones fold.
        let is_open = self.expanded.contains(&index) || self.is_last_message(index);
        let cut = match is_open {
            true => None,
            false => folded(words),
        };
        let shown = cut.as_deref().unwrap_or(words);
        let state = self.markdown(index, cut.is_some(), shown, cx);
        let (answer, body, fold) = (
            format!("chat-answer-{index}"),
            format!("chat-body-{index}"),
            format!("chat-fold-{index}"),
        );
        let reading = TextView::new(&state)
            .selectable(true)
            .on_link_click(|address, _, _, cx| open(address, cx));
        let body = div()
            .debug_selector(move || body)
            .text_size(theme::TEXT_BODY)
            .line_height(tokens::MESSAGE_LINE_HEIGHT)
            .text_color(palette.text)
            .child(reading)
            .when(cut.is_some(), |body| {
                body.child(div().debug_selector(move || fold))
            });
        let more = cut.is_some().then(|| self.show_all(index, palette, cx));
        let note = is_cut.then(|| {
            text(theme::TEXT_LABEL, palette.text_tertiary).child(tr(Key("lane_page.cut")))
        });
        div()
            .debug_selector(move || answer)
            .flex()
            .flex_col()
            .child(body)
            .children(more)
            .children(note)
            .into_any_element()
    }

    fn markdown(
        &mut self,
        index: usize,
        is_folded: bool,
        words: &str,
        cx: &mut Context<Self>,
    ) -> Entity<TextViewState> {
        const MAX_PARSED_ROWS: usize = 128;
        self.text_access += 1;
        let key = (index, is_folded);
        if let Some((access, state)) = self.texts.get_mut(&key) {
            *access = self.text_access;
            return state.clone();
        }
        if self.texts.len() >= MAX_PARSED_ROWS {
            if let Some(oldest) = self
                .texts
                .iter()
                .min_by_key(|(_, (access, _))| *access)
                .map(|(key, _)| *key)
            {
                self.texts.remove(&oldest);
            }
        }
        let state = cx.new(|cx| TextViewState::markdown(&inert(words), cx));
        self.texts.insert(key, (self.text_access, state.clone()));
        state
    }

    #[cfg(test)]
    pub(in crate::ui) fn parsed_cache_size_after(
        &mut self,
        rows: usize,
        cx: &mut Context<Self>,
    ) -> usize {
        for index in 0..rows {
            self.markdown(index, false, "message", cx);
        }
        self.texts.len()
    }

    fn show_all(&self, index: usize, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let selector = format!("chat-more-{index}");
        let id = selector.clone();
        let (hover, fill) = (palette.fill_hover, palette.fill);
        div()
            .flex()
            .child(
                div()
                    .id(SharedString::from(id))
                    .debug_selector(move || selector)
                    .mt(sizes::MORE_GAP_ABOVE)
                    .h(theme::CONTROL_HEIGHT)
                    .px(theme::CONTROL_PADDING_X)
                    .flex()
                    .items_center()
                    .gap(theme::GAP_SMALL)
                    .rounded(theme::RADIUS_CONTROL)
                    .bg(fill)
                    .text_size(theme::TEXT_SMALL)
                    .text_color(palette.text_secondary)
                    .hover(move |style| style.bg(hover))
                    .child(tr(Key("lane_page.show_all")))
                    .child(Icon::new(IconName::ChevronDown).with_size(theme::HAND_MARK_SIZE))
                    .on_click(cx.listener(move |chat, _, _, cx| {
                        chat.expanded.insert(index);
                        chat.remeasure(index, cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// The text the agent is writing now, drawn as the master's answers are, its new words fading in.
    fn live_text(&self, palette: &Palette) -> AnyElement {
        let Some(live) = &self.live else {
            return div().into_any_element();
        };
        let parts = live.answer.parts().iter().enumerate().map(|(at, part)| {
            div()
                .when(at > 0, |part| part.pt(tokens::ANSWER_PART_GAP))
                .child(
                    TextView::new(part)
                        .selectable(true)
                        .on_link_click(|address, _, _, cx| open(address, cx)),
                )
        });
        div()
            .debug_selector(|| "chat-live".into())
            .w_full()
            .text_size(theme::TEXT_BODY)
            .line_height(tokens::MESSAGE_LINE_HEIGHT)
            .text_color(palette.text)
            .children(parts)
            .into_any_element()
    }

    /// Under the conversation: what the engine said to the last message when it was not a plain delivery, then
    /// the box, or the words of why nothing can read a message.
    fn footer(
        &mut self,
        run: Option<&AgentRun>,
        palette: &Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let outcome = self.outcome.clone().map(|words| {
            text(theme::TEXT_SMALL, palette.text_secondary)
                .debug_selector(|| "chat-outcome".into())
                .child(words)
        });
        let listening = run.map(|run| &run.listening);
        let role = run.map_or("", crate::ui::lane_agents::role_of);
        let to = listening.and_then(|listening| super::composer_words(listening, role));
        let below = match (to, listening) {
            (Some(words), _) => {
                self.composer
                    .update(cx, |composer, cx| composer.set_placeholder(words, cx));
                self.composer.clone().into_any_element()
            }
            (None, Some(Listening::No(why))) => not_listening(why, palette),
            (None, _) => div().into_any_element(),
        };
        div()
            .flex_none()
            .px(sizes::COMPOSER_PADDING_X)
            .pt(sizes::COMPOSER_PADDING_TOP)
            .pb(sizes::COMPOSER_PADDING_BOTTOM)
            .flex()
            .flex_col()
            .items_center()
            .gap(theme::GAP_TIGHT)
            .child(below)
            .children(outcome)
            .into_any_element()
    }
}

/// The role, and under it the model and the account; at the end the state.
fn who_view(run: &AgentRun, state: RunState, palette: &Palette) -> AnyElement {
    let model = model_words(&run.provider, &run.model);
    let detail = match run.account.as_deref().filter(|account| !account.is_empty()) {
        Some(account) => crate::l10n::tr_with(
            Key("agents.detail"),
            &[
                ("model", &model),
                ("account", &crate::ui::thread::shown(account)),
            ],
        ),
        None => model,
    };
    let state = div()
        .debug_selector(|| "agent-chat-state".into())
        .flex_none()
        .flex()
        .items_center()
        .gap(SPACE_S)
        .text_style(TEXT_FOOTNOTE)
        .text_color(palette.text_secondary)
        .child(state_glyph(state).paint(ICON_REGULAR, palette))
        .child(state_words(state));
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .child(
            div()
                .debug_selector(|| "agent-chat-role".into())
                .text_size(theme::TEXT_BODY)
                .font_weight(WEIGHT_MEDIUM)
                .text_color(palette.text)
                .child(role_words(crate::ui::lane_agents::role_of(run))),
        )
        .child(
            div()
                .debug_selector(|| "agent-chat-detail".into())
                .min_w_0()
                .truncate()
                .text_style(TEXT_FOOTNOTE)
                .text_color(palette.text_tertiary)
                .child(detail),
        )
        .child(div().flex_1())
        .child(state)
        .into_any_element()
}

/// A tool call in a quiet line: what it is, where, and how long it has run or took.
pub(in crate::ui) fn tool_line(index: usize, tool: &ToolRow, palette: &Palette) -> AnyElement {
    let (line, time) = (
        format!("chat-tool-{index}"),
        format!("chat-tool-elapsed-{index}"),
    );
    let elapsed = tool.elapsed_words(now()).map(|words| {
        text(theme::TEXT_LABEL, palette.text_tertiary)
            .debug_selector(move || time)
            .flex_none()
            .font_features(tabular())
            .child(words)
    });
    let place = tool.place();
    let has_target = !place.is_empty();
    let target = has_target.then(|| {
        mono()
            .min_w_0()
            .flex_1()
            .truncate()
            .text_color(palette.text_tertiary)
            .child(crate::ui::thread::shown(&place))
    });
    text(theme::TEXT_SMALL, palette.text_secondary)
        .debug_selector(move || line)
        .flex()
        .items_center()
        .gap(theme::GAP)
        .child(div().flex_none().child(tool_words(&tool.name, has_target)))
        .children(target)
        .child(div().flex_1())
        .children(elapsed)
        .into_any_element()
}

/// A line the engine wrote about this run, one muted line.
pub(in crate::ui) fn engine_line(index: usize, words: &str, palette: &Palette) -> AnyElement {
    let selector = format!("chat-engine-{index}");
    text(sizes::NOTICE_TEXT, palette.text_tertiary)
        .debug_selector(move || selector)
        .child(crate::ui::thread::shown(words))
        .into_any_element()
}

/// The start of a round after the first: a hairline each side of its name, quiet.
pub(in crate::ui) fn round_divider(index: usize, round: usize, palette: &Palette) -> AnyElement {
    let selector = format!("chat-round-{index}");
    let rule = || div().flex_1().h_px().bg(palette.border);
    div()
        .debug_selector(move || selector)
        .flex()
        .items_center()
        .gap(theme::GAP)
        .child(rule())
        .child(text(sizes::NOTICE_TEXT, palette.text_tertiary).child(round_words(round)))
        .child(rule())
        .into_any_element()
}

/// A result block as a card: the verdict with its mark (a check for a success, a cross only for a failure, a
/// warning for a request for changes, none for a verdict nobody here knows), then what it says.
pub(in crate::ui) fn result_card(
    index: usize,
    verdict: &str,
    summary: &str,
    palette: &Palette,
) -> AnyElement {
    let (heading, summary) = result_words(verdict, summary);
    let verdict = verdict_of(verdict);
    let (kind, mark) = match &verdict {
        v if v.is_good() => ("pass", Some((IconName::Check, palette.success))),
        v if v.is_bad() => ("fail", Some((IconName::Close, palette.danger))),
        Verdict::ChangesRequested => ("warn", Some((IconName::TriangleAlert, palette.warning))),
        _ => ("none", None),
    };
    let (card, line) = (
        format!("chat-result-{index}"),
        format!("chat-verdict-{index}"),
    );
    let icon_id = format!("chat-verdict-{kind}-{index}");
    let icon = mark.map(|(icon, colour)| {
        div().debug_selector(move || icon_id).child(
            Icon::new(icon)
                .with_size(theme::HAND_MARK_SIZE)
                .text_color(colour),
        )
    });
    kit::card(card, palette)
        .child(
            div()
                .debug_selector(move || line)
                .flex()
                .items_center()
                .gap(theme::GAP_SMALL)
                .text_size(theme::TEXT_SMALL)
                .font_weight(theme::WEIGHT_MEDIUM)
                .text_color(palette.text)
                .children(icon)
                .child(SharedString::from(heading)),
        )
        .children(summary.map(|summary| {
            text(theme::TEXT_SMALL, palette.text_secondary)
                .child(crate::ui::thread::shown(&summary))
        }))
        .into_any_element()
}

/// The words of why nothing can read a message, in the box's place.
pub(in crate::ui) fn not_listening(why: &str, palette: &Palette) -> AnyElement {
    let div: Div = text(theme::TEXT_SMALL, palette.text_secondary);
    div.debug_selector(|| "chat-not-listening".into())
        .w_full()
        .max_w(sizes::COMPOSER_MAX_WIDTH)
        .child(crate::ui::thread::shown(why))
        .into_any_element()
}
