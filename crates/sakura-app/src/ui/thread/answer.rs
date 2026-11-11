//! The agent's answer as the rows that draw it. The markdown is cut into parts where that changes nothing
//! of the picture (`segments`), each part is its own row of the list, and only the last part takes in new
//! text. So a streaming answer lays out its last part again and nothing above it, and a long answer costs
//! what is on screen.

use super::breaks::Breaks;
use super::copy;
use super::segments::{Segmenter, Step};
use crate::theme::thread as tokens;
use gpui::{App, AppContext as _, Entity};
use gpui_component::text::{TextViewMotion, TextViewState};

/// How an answer's rows changed when it took in new text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Growth {
    /// The same parts; the last one may have grown.
    Same,
    /// This many parts were added after the ones there were.
    Added(usize),
    /// The answer became one part again, from this many.
    Joined { from: usize },
}

/// One answer: its parts, and the reading of its text that cuts them.
pub(super) struct Answer {
    parts: Vec<Entity<TextViewState>>,
    segmenter: Segmenter,
    /// The reading that keeps the line breaks of what the parts were given.
    breaks: Breaks,
    /// A replayed answer is shown at once; a live one fades its new words in.
    is_replayed: bool,
}

impl Answer {
    /// The answer for `text`, complete or still streaming.
    pub(super) fn new(text: &str, is_streaming: bool, is_replayed: bool, cx: &mut App) -> Self {
        let mut answer = Self {
            parts: vec![part(is_replayed, cx)],
            segmenter: Segmenter::default(),
            breaks: Breaks::default(),
            is_replayed,
        };
        answer.grow(text, is_streaming, cx);
        answer
    }

    /// The markdown states of the parts, in order.
    pub(super) fn parts(&self) -> &[Entity<TextViewState>] {
        &self.parts
    }

    /// Takes in the answer's text, which only ever grows; when it no longer streams, what was held back
    /// while its meaning was open goes to the last part.
    pub(super) fn grow(&mut self, text: &str, is_streaming: bool, cx: &mut App) -> Growth {
        let before = self.parts.len();
        let mut steps = self.segmenter.feed(text);
        if !is_streaming {
            steps.extend(self.segmenter.finish(text));
        }
        for step in steps {
            match step {
                Step::Cut => self.parts.push(part(self.is_replayed, cx)),
                Step::Text(range) => self.push(&copy::shown(&text[range]), cx),
                Step::Literal(more) => self.push(&more, cx),
            }
        }
        if self.segmenter.is_whole() && before > 1 {
            return self.join(text, cx);
        }
        match self.parts.len() - before {
            0 => Growth::Same,
            added => Growth::Added(added),
        }
    }

    /// Adds `more` to the last part, its line breaks kept.
    fn push(&mut self, more: &str, cx: &mut App) {
        let more = self.breaks.keep(more);
        let last = self.parts.last().expect("an answer has a part");
        last.update(cx, |last, cx| last.push_str(&more, cx));
    }

    /// The text defines references its parts would not share: it becomes one part, shown at once.
    fn join(&mut self, text: &str, cx: &mut App) -> Growth {
        let from = self.parts.len();
        let whole = part(true, cx);
        self.breaks = Breaks::default();
        let shown = self.breaks.keep(&copy::shown(self.segmenter.given(text)));
        whole.update(cx, |whole, cx| whole.set_text(&shown, cx));
        self.parts = vec![whole];
        Growth::Joined { from }
    }
}

/// An empty markdown state for one part.
fn part(is_replayed: bool, cx: &mut App) -> Entity<TextViewState> {
    let fade = match is_replayed {
        true => std::time::Duration::ZERO,
        false => tokens::STREAM_FADE,
    };
    let motion = TextViewMotion::default().with_stream_fade(fade);
    cx.new(|cx| TextViewState::markdown("", cx).motion(motion))
}
