//! The page of a morning digest: a report a program wrote, read as one. Its day with how many asks are left to
//! decide, its news (each with its source and its links, by their titles) and the asks it raised, with their
//! state (an open one opens the Inbox). It has no composer: nothing listens on the other side.

use super::{LanePage, LanePageEvent};
use crate::l10n::{tr, tr_with, Key};
use crate::theme::{self, digest_page as sizes, Palette};
use crate::ui::chrome::{tabular, text};
use crate::ui::glyph::{glyph_column, placed_state_glyph};
use crate::ui::inbox::model as asks;
use crate::ui::inert_markdown::{inert, open};
use crate::ui::lanes;
use crate::ui::titled_links::titled_links;
use crate::ui::unix_now;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, AppContext as _, Context, Div, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
};
use gpui_component::text::TextView;
use sakura_core::engine::snapshot::{Item, ReadingAsk, ReadingNews, Snapshot};

/// What an ask row says at its right end: when it was raised while it is open, how it was answered once decided.
enum AskEnd {
    Raised(Option<SharedString>),
    Answer(SharedString),
}

/// The words after the title: how many asks are left to decide, or that none is.
fn left_words(item: &Item, snapshot: &Snapshot) -> Option<SharedString> {
    let progress = item
        .progress
        .as_ref()
        .filter(|progress| progress.total > 0)?;
    let left = progress.total.saturating_sub(progress.done);
    let snoozed = lanes::snoozed_owner_rows(item, snapshot, unix_now());
    if left > 0 && left <= snoozed {
        return None;
    }
    Some(match left.saturating_sub(snoozed) {
        0 => tr(Key("digest_page.all_decided")).into(),
        left => tr_with(Key("lanes_page.to_decide"), &[("n", &left.to_string())]),
    })
}

impl LanePage {
    /// The whole page of the digest `item`: a header that stays, and the report under it.
    pub(super) fn digest_page(
        &mut self,
        page: gpui::Stateful<Div>,
        item: &Item,
        snapshot: &Snapshot,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sections = self.sections(item, snapshot, palette, cx);
        let body = div()
            .id("digest-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .max_w(sizes::READING_WIDTH)
                    .px(theme::CONTENT_PADDING)
                    .py(sizes::PADDING_Y)
                    .flex()
                    .flex_col()
                    .gap(sizes::SECTION_GAP)
                    .children(sections),
            );
        page.overflow_hidden()
            .child(digest_header(item, snapshot, palette))
            .child(body)
            .into_any_element()
    }

    /// The sections of the report: its news (its text when it lists none), then its asks.
    fn sections(
        &mut self,
        item: &Item,
        snapshot: &Snapshot,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let reading = item.reading.as_ref();
        let news = reading.map_or(&[][..], |reading| reading.news.as_slice());
        let asks = reading.map_or(&[][..], |reading| reading.asks.as_slice());
        let first = match news.is_empty() {
            true => self.report_text(item, cx),
            false => Some(news_list(news, snapshot, palette, cx)),
        };
        first
            .into_iter()
            .chain(self.ask_rows(asks, snapshot, palette, cx))
            .collect()
    }

    /// What the digest wrote, read as Markdown and made inert, kept while its text is the same. It stands in
    /// for the news when the engine lists none.
    fn report_text(&mut self, item: &Item, cx: &mut Context<Self>) -> Option<AnyElement> {
        let words = item
            .goal
            .as_deref()
            .filter(|words| !words.trim().is_empty())?;
        let kept = self.report.as_ref().filter(|(shown, _)| shown == words);
        let state = match kept {
            Some((_, state)) => state.clone(),
            None => {
                let state =
                    cx.new(|cx| gpui_component::text::TextViewState::markdown(&inert(words), cx));
                self.report = Some((words.to_string(), state.clone()));
                state
            }
        };
        let reading = TextView::new(&state)
            .selectable(true)
            .on_link_click(|address, _, _, cx| open(address, cx));
        Some(
            div()
                .debug_selector(|| "digest-text".into())
                .text_size(sizes::TEXT)
                .child(reading)
                .into_any_element(),
        )
    }

    /// The asks the digest raised, each a row with its title and its state: an open one with the time it was
    /// raised, and a press opens the Inbox, where it is answered; a decided one with how it was answered.
    fn ask_rows(
        &self,
        asks: &[ReadingAsk],
        snapshot: &Snapshot,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if asks.is_empty() {
            return None;
        }
        let now = unix_now();
        let rows = asks.iter().enumerate().map(|(index, ask)| {
            let end = ask_end(ask, snapshot, now);
            ask_row(index, ask, end, palette, cx)
        });
        Some(
            div()
                .debug_selector(|| "digest-asks".into())
                .flex()
                .flex_col()
                .gap(sizes::ROW_GAP)
                .child(section_name("digest_page.asks", palette))
                .children(rows)
                .into_any_element(),
        )
    }
}

/// The name over a section's rows.
fn section_name(key: &'static str, palette: &Palette) -> AnyElement {
    text(theme::TEXT_LABEL, palette.text_tertiary)
        .pb(sizes::HEADING_GAP)
        .child(tr(Key(key)))
        .into_any_element()
}

/// The news of the day: each entry's title with the source it came from, its words and its links.
fn news_list(
    news: &[ReadingNews],
    snapshot: &Snapshot,
    palette: &Palette,
    cx: &mut Context<LanePage>,
) -> AnyElement {
    let entries = news
        .iter()
        .enumerate()
        .map(|(index, entry)| news_entry(index, entry, snapshot, palette, cx));
    div()
        .debug_selector(|| "digest-news".into())
        .flex()
        .flex_col()
        .child(section_name("digest_page.news", palette))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sizes::SECTION_GAP)
                .children(entries),
        )
        .into_any_element()
}

fn news_entry(
    index: usize,
    entry: &ReadingNews,
    snapshot: &Snapshot,
    palette: &Palette,
    cx: &mut Context<LanePage>,
) -> AnyElement {
    let (selector, source_selector) = (
        format!("digest-news-{index}"),
        format!("digest-news-{index}-source"),
    );
    let head = div()
        .flex()
        .items_baseline()
        .gap(theme::GAP_WIDE)
        .child(
            text(theme::TEXT_BODY, palette.text)
                .font_weight(theme::WEIGHT_MEDIUM)
                .min_w_0()
                .child(entry.title.clone()),
        )
        .child(
            text(theme::TEXT_LABEL, palette.text_tertiary)
                .debug_selector(move || source_selector)
                .flex_none()
                .child(source_name(&entry.source, snapshot)),
        );
    let links = titled_links(
        &format!("digest-news-{index}-link"),
        &entry.links,
        palette,
        cx,
    );
    div()
        .debug_selector(move || selector)
        .flex()
        .flex_col()
        .gap(sizes::ROW_GAP)
        .child(head)
        .child(text(sizes::TEXT, palette.text_secondary).child(entry.text.clone()))
        .children(links)
        .into_any_element()
}

/// The source a news entry came from, by the name the engine gives it, else by its id.
fn source_name(id: &str, snapshot: &Snapshot) -> SharedString {
    let listed = snapshot
        .digest
        .iter()
        .flat_map(|digest| &digest.sources)
        .find(|source| source.id == id);
    SharedString::from(listed.map_or(id, |source| source.name()).to_string())
}

/// What the end of an ask's row says.
fn ask_end(ask: &ReadingAsk, snapshot: &Snapshot, now: i64) -> AskEnd {
    if !ask.is_open() {
        return AskEnd::Answer(answer_words(ask));
    }
    let wait = ask
        .wait
        .as_deref()
        .and_then(|id| snapshot.waits.iter().find(|wait| wait.id == id));
    AskEnd::Raised(wait.and_then(|wait| asks::clock_words(wait.since.as_deref(), now)))
}

/// How a decided ask was answered, in the interface's words: a verdict is "Done" or "Not now", a choice or a
/// text is what the person gave.
fn answer_words(ask: &ReadingAsk) -> SharedString {
    let given = ask.answer.as_deref().unwrap_or_default();
    match given {
        "ok" => tr(Key("inbox.answer.marked_done")).into(),
        "ko" => tr(Key("inbox.answer.not_now")).into(),
        other => SharedString::from(other.to_string()),
    }
}

/// The header that stays: the neutral mark, what is left to decide and the digest's day at the right. The
/// title is the breadcrumb's, the tab's and the sidebar's: the page does not say it a fourth time.
fn digest_header(item: &Item, snapshot: &Snapshot, palette: &Palette) -> AnyElement {
    let state = lanes::shown_state(item, Some(snapshot));
    div()
        .debug_selector(|| "lane-header".into())
        .h(theme::HEADER_BOX_HEIGHT)
        .flex_none()
        .px(theme::CONTENT_PADDING)
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .border_b_1()
        .border_color(palette.border)
        .child(glyph_column(
            placed_state_glyph(
                "lane-header",
                state,
                palette,
                crate::ui::breath::Breath::Still,
            ),
            "lane-header-glyph".into(),
        ))
        .children(left_words(item, snapshot).map(|words| {
            text(theme::TEXT_BODY, palette.text)
                .debug_selector(|| "lane-state".into())
                .flex_none()
                .font_features(tabular())
                .child(words)
        }))
        .child(div().flex_1())
        .children(item.date.clone().map(|date| {
            text(theme::TEXT_LABEL, palette.text_tertiary)
                .debug_selector(|| "digest-date".into())
                .flex_none()
                .font_features(tabular())
                .child(date)
        }))
        .into_any_element()
}

/// One ask: its title, and at the right end its time or its answer. Only an open ask can be pressed.
fn ask_row(
    index: usize,
    ask: &ReadingAsk,
    end: AskEnd,
    palette: &Palette,
    cx: &mut Context<LanePage>,
) -> AnyElement {
    let (selector, hover) = (format!("digest-ask-{index}"), palette.hover);
    let is_open = ask.is_open();
    let title_colour = if is_open {
        palette.text
    } else {
        palette.text_tertiary
    };
    let words = match end {
        AskEnd::Raised(when) => when,
        AskEnd::Answer(answer) => Some(answer),
    };
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector)
        .h(sizes::ROW_HEIGHT)
        .px(sizes::ROW_PADDING_X)
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .rounded(theme::RADIUS_CONTROL)
        .bg(palette.fill)
        .when(is_open, |row| {
            row.hover(move |style| style.bg(hover))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(LanePageEvent::OpenInbox)))
        })
        .child(
            text(theme::TEXT_SMALL, title_colour)
                .flex_1()
                .min_w_0()
                .truncate()
                .child(ask.title.clone()),
        )
        .when_some(words, |row, words| {
            row.child(
                text(theme::TEXT_LABEL, palette.text_tertiary)
                    .flex_none()
                    .font_features(tabular())
                    .child(words),
            )
        })
        .into_any_element()
}
