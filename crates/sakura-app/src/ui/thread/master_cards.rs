//! The master's cards: the lane it started and the note it proposes, drawn in place of the tool call that
//! made them. What a card says and what it can do come from the engine's facts through the kit's world,
//! bound by an exact name only (see `ui::master_binding`); the call's own input is never shown. A card that
//! binds nothing says so and offers no action on the engine.

use super::master::{CardTool, LaneAsk, LaneCard, Proposal, ProposalAsk};
use super::states::{self, Face};
use super::{copy, ThreadView};
use crate::l10n::{tr, tr_count, tr_with};
use crate::theme::thread as tokens;
use crate::theme::{self, Palette};
use crate::ui::chrome::{mono, tabular, text};
use crate::ui::kit::{self, progress_line};
use gpui::{
    div, px, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled,
};
use sakura_acp::thread::ToolCall;

/// The lines of a proposed note shown before the rest is left to the Inbox.
const PROPOSAL_LINES: usize = 14;

impl ThreadView {
    /// The card a master's tool call is drawn as; none for any other call, or in a conversation that is not
    /// the master's.
    pub(super) fn master_card(
        &self,
        call: &ToolCall,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.master.as_ref()?;
        match CardTool::of(call)? {
            CardTool::LaneOpen => Some(self.lane_card(call, palette, cx)),
            CardTool::KnowledgePropose => Some(self.proposal_card(call, palette, cx)),
        }
    }

    fn lane_card(&self, call: &ToolCall, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(kit) = self.master.as_ref() else {
            return div().into_any_element();
        };
        let lane = kit.world.lane(&LaneAsk::of(call), cx);
        let world = kit.world.clone();
        let key = lane.key.clone();
        let card =
            card_frame(&call.id, "lane", palette).child(card_label(tr(copy::CARD_LANE), palette));
        if key.is_none() {
            let unbound =
                text(theme::TEXT_LABEL, palette.text_tertiary).child(tr(copy::CARD_STEP_UNKNOWN));
            return card.child(unbound).into_any_element();
        }
        let card = card
            .child(lane_title(&lane, palette))
            .child(lane_meta(&lane, palette));
        match key {
            Some(key) => card
                .hover(|style| style.bg(palette.fill_hover))
                .on_click(move |_, window, cx| world.open_lane(&key, window, cx))
                .into_any_element(),
            None => card.into_any_element(),
        }
    }

    fn proposal_card(
        &self,
        call: &ToolCall,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(kit) = self.master.as_ref() else {
            return div().into_any_element();
        };
        let proposal = kit.world.proposal(&ProposalAsk::of(call), cx);
        let frame = card_frame(&call.id, "proposal", palette)
            .child(card_label(tr(copy::CARD_PROPOSAL), palette));
        match proposal {
            Proposal::Waiting {
                wait,
                title,
                detail,
            } => {
                let is_whole = self.shows_whole_note(&call.id, &detail);
                frame
                    .child(
                        text(theme::TEXT_SMALL, palette.text)
                            .font_weight(theme::WEIGHT_MEDIUM)
                            .child(super::copy::shown(&title)),
                    )
                    .children(
                        (!detail.trim().is_empty())
                            .then(|| self.note_diff(&call.id, &detail, palette, cx)),
                    )
                    .child(self.decision_row(&wait, is_whole, palette, cx))
                    .into_any_element()
            }
            Proposal::Unbound => frame
                .child(self.open_inbox_button(&call.id, palette))
                .into_any_element(),
        }
    }

    /// The way to the Inbox, where the engine's own waits are answered.
    fn open_inbox_button(&self, call: &str, palette: &Palette) -> AnyElement {
        let Some(kit) = self.master.as_ref() else {
            return div().into_any_element();
        };
        let world = kit.world.clone();
        let id = format!("master-open-inbox-{call}");
        let selector = id.clone();
        let button = states::button(
            SharedString::from(id),
            tr(copy::CARD_OPEN_INBOX),
            Face::OnFill,
            palette,
        )
        .debug_selector(move || selector.clone())
        .on_click(move |_, window, cx| world.open_inbox(window, cx));
        div().flex().child(button).into_any_element()
    }

    /// Whether every line of the note is drawn in the card `call`: it fits, or the person opened it.
    fn shows_whole_note(&self, call: &str, detail: &str) -> bool {
        detail.lines().count() <= PROPOSAL_LINES || self.expanded_notes.contains(call)
    }

    /// Approve and Decline, or what the engine said to the answer given: the question to confirm, or its
    /// word. Approve is offered only over a note shown whole (`is_whole`); Decline is always there.
    fn decision_row(
        &self,
        wait: &str,
        is_whole: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(kit) = self.master.as_ref() else {
            return div().into_any_element();
        };
        if let Some(said) = kit.world.decision(wait, cx) {
            return said;
        }
        let button = |name: &str, words, face, approve: bool| {
            let (world, wait) = (kit.world.clone(), wait.to_string());
            let id = format!("master-{name}-{wait}");
            let selector = id.clone();
            states::button(SharedString::from(id), tr(words), face, palette)
                .debug_selector(move || selector.clone())
                .on_click(move |_, window, cx| world.decide(&wait, approve, window, cx))
        };
        div()
            .flex()
            .gap(tokens::BUTTON_GAP)
            .children(is_whole.then(|| button("approve", copy::CARD_APPROVE, Face::Primary, true)))
            .child(button("decline", copy::CARD_DECLINE, Face::OnFill, false))
            .into_any_element()
    }

    /// Draws again the rows that are cards: what they say follows the engine, which changes without the
    /// transcript changing.
    pub fn refresh_cards(&mut self, cx: &mut Context<Self>) {
        let cards: Vec<usize> = self
            .thread
            .entries()
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match &entry.kind {
                sakura_acp::thread::EntryKind::Tool(call) => CardTool::of(call).map(|_| index),
                _ => None,
            })
            .collect();
        for index in cards {
            self.remeasure(index, cx);
        }
        cx.notify();
    }
}

/// The filled block every card sits in, named by its call so a test finds it.
fn card_frame(call: &str, kind: &str, palette: &Palette) -> gpui::Stateful<gpui::Div> {
    kit::card(format!("master-{kind}-card-{call}"), palette).w_full()
}

fn card_label(words: &'static str, palette: &Palette) -> gpui::Div {
    text(theme::TEXT_LABEL, palette.text_tertiary).child(words)
}

fn lane_title(lane: &LaneCard, palette: &Palette) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(theme::GAP)
        .child(lane.glyph.paint(tokens::CARD_GLYPH_SIZE, palette))
        .child(
            text(theme::TEXT_SMALL, palette.text)
                .font_weight(theme::WEIGHT_MEDIUM)
                .min_w_0()
                .child(super::copy::shown(&lane.title)),
        )
}

/// The project, where the lane is, and how far.
fn lane_meta(lane: &LaneCard, palette: &Palette) -> gpui::Div {
    let project = div()
        .px(theme::GAP_SMALL)
        .rounded(theme::RADIUS_CONTROL)
        .bg(palette.fill_hover)
        .child(super::copy::shown(&lane.project));
    let step = match lane.progress {
        Some((done, total)) => tr_with(
            copy::CARD_STEP,
            &[
                ("phase", &lane.phase),
                ("done", &done.to_string()),
                ("total", &total.to_string()),
            ],
        ),
        None => tr(copy::CARD_STEP_UNKNOWN).into(),
    };
    let progress = lane
        .progress
        .map(|(done, total)| progress_line(done, total, tokens::CARD_PROGRESS_WIDTH));
    text(theme::TEXT_LABEL, palette.text_tertiary)
        .flex()
        .items_center()
        .gap(theme::GAP)
        .font_features(tabular())
        .child(project)
        .child(step)
        .children(progress)
}

impl ThreadView {
    /// The engine's text of the proposal as added lines, in a well of its own. Long lines wrap, so nothing is
    /// cut at the side; past [`PROPOSAL_LINES`] a button opens the rest, and only then can the note be approved
    /// from the card.
    fn note_diff(
        &self,
        call: &str,
        detail: &str,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lines: Vec<&str> = detail.lines().collect();
        let shown = match self.shows_whole_note(call, detail) {
            true => lines.len(),
            false => PROPOSAL_LINES,
        };
        let rows = lines.iter().take(shown).enumerate().map(|(at, line)| {
            let selector = format!("master-proposal-line-{at}");
            mono()
                .debug_selector(move || selector)
                .px(tokens::BLOCK_PADDING_X)
                .bg(palette.success.opacity(0.12))
                .child(super::copy::shown(&format!("+ {line}")))
        });
        let more = lines.len().saturating_sub(shown);
        let rest = (more > 0).then(|| self.expand_button(call, more, palette, cx));
        mono()
            .debug_selector(|| "master-proposal-diff".to_string())
            .w_full()
            .py(tokens::BLOCK_PADDING_Y)
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(theme::RADIUS_CONTROL)
            .bg(palette.inset)
            .text_size(theme::TEXT_LABEL)
            .line_height(px(theme::LOG_LINE_HEIGHT))
            .text_color(palette.text)
            .children(rows)
            .children(rest)
            .into_any_element()
    }

    /// The line under a cut note that opens the rest of it.
    fn expand_button(
        &self,
        call: &str,
        more: usize,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (id, opened) = (format!("master-proposal-expand-{call}"), call.to_string());
        let selector = id.clone();
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector.clone())
            .px(tokens::BLOCK_PADDING_X)
            .text_color(palette.text_secondary)
            .hover(|style| style.text_color(palette.text))
            .on_click(cx.listener(move |view, _, _, cx| {
                view.expanded_notes.insert(opened.clone());
                view.refresh_cards(cx);
            }))
            .child(tr_count(copy::CARD_MORE_LINES, more as u64, &[]))
            .into_any_element()
    }
}
