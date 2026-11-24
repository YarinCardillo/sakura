//! A choice between a few words, all on screen: the chosen one is filled, never outlined, and choosing
//! changes no size. It has two forms. The inline form hugs its words and sits in a row beside other
//! things (a pace, a preset, a theme). The full-width form spans its container in equal parts, each word
//! centred in its part (the switch of a column's views).

use crate::theme::kit::scale::{Typeset as _, CONTROL_LABEL};
use crate::theme::{segments as sizes, Palette, Theme, CONTROL_HEIGHT};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use std::rc::Rc;

/// The measures of one form of the choice.
#[derive(Clone, Copy)]
struct Form {
    is_full: bool,
    part_gap: Pixels,
    part_padding_x: Pixels,
}

/// Hugs its words, in a row beside other things.
const INLINE: Form = Form {
    is_full: false,
    part_gap: sizes::INLINE_GAP,
    part_padding_x: sizes::INLINE_PART_PADDING_X,
};

/// Spans its container in equal parts that touch each other, each word centred.
const FULL: Form = Form {
    is_full: true,
    part_gap: gpui::px(0.),
    part_padding_x: gpui::px(0.),
};

/// A press handler of a control.
pub type OnPress = Rc<dyn Fn(&mut Window, &mut App)>;

/// What picking a word of [`segments`] does, given the word's index.
pub type OnPick = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// One word of a [`segment_track`] choice: what names it in the window, its word, whether it is the one
/// chosen, and what a press does.
#[derive(Clone)]
pub struct Segment {
    pub id: &'static str,
    pub label: SharedString,
    pub is_on: bool,
    pub on_press: OnPress,
}

/// A choice between a few words whose ids and presses are each its own, in the inline form.
pub fn segment_track(options: Vec<Segment>, palette: &Palette) -> AnyElement {
    let parts = options.into_iter().map(|option| {
        part(
            option.id.into(),
            option.label,
            option.is_on,
            option.on_press,
            INLINE,
            palette,
        )
    });
    track(None, parts, INLINE, palette)
}

/// A choice between a few words, inline: `options` are each word and whether it is the one chosen,
/// `on_pick` is called with the index of the word pressed. The choice is named `id` in the window and
/// each word `{id}-{index}`.
pub fn segments(
    id: impl Into<SharedString>,
    options: &[(SharedString, bool)],
    on_pick: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> Segments {
    Segments {
        id: id.into(),
        options: options.to_vec(),
        on_pick: Rc::new(on_pick),
        form: INLINE,
    }
}

/// The element [`segments`] builds: it takes its colours from the theme when it is drawn.
#[derive(IntoElement)]
pub struct Segments {
    id: SharedString,
    options: Vec<(SharedString, bool)>,
    on_pick: OnPick,
    form: Form,
}

impl Segments {
    /// The full-width form: the choice spans its container in equal parts.
    pub fn full_width(mut self) -> Self {
        self.form = FULL;
        self
    }
}

impl RenderOnce for Segments {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let on_pick = self.on_pick;
        let id = self.id.clone();
        let parts = self
            .options
            .into_iter()
            .enumerate()
            .map(|(index, (label, is_on))| {
                let on_pick = on_pick.clone();
                let on_press: OnPress = Rc::new(move |window, cx| on_pick(index, window, cx));
                part(
                    format!("{id}-{index}").into(),
                    label,
                    is_on,
                    on_press,
                    self.form,
                    &palette,
                )
            });
        track(Some(self.id), parts, self.form, &palette)
    }
}

fn track(
    id: Option<SharedString>,
    parts: impl IntoIterator<Item = AnyElement>,
    form: Form,
    palette: &Palette,
) -> AnyElement {
    let track = div()
        .h(CONTROL_HEIGHT)
        .px(sizes::TRACK_PADDING)
        .flex()
        .items_center()
        .gap(form.part_gap)
        .rounded(sizes::TRACK_RADIUS)
        .bg(palette.segment_track)
        .children(parts);
    let track = match id {
        Some(id) => track.debug_selector(move || id.to_string()),
        None => track,
    };
    if form.is_full {
        return track.w_full().into_any_element();
    }
    // A flex row in a block parent stretches; a row of its own around the track lets it keep its words' width.
    div().flex().child(track).into_any_element()
}

fn part(
    id: SharedString,
    label: SharedString,
    is_on: bool,
    on_press: OnPress,
    form: Form,
    palette: &Palette,
) -> AnyElement {
    let selector = id.clone();
    div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .h(sizes::PART_HEIGHT)
        .px(form.part_padding_x)
        .flex()
        .items_center()
        .justify_center()
        .when(form.is_full, |part| part.flex_1().min_w_0())
        .rounded(sizes::PART_RADIUS)
        .text_style(CONTROL_LABEL)
        .whitespace_nowrap()
        // One weight for every word; only the fill and a step of ink tell the chosen one, so choosing never
        // reads as a word turning bolder.
        .text_color(match is_on {
            true => palette.text,
            false => palette.text_secondary,
        })
        .when(is_on, |part| part.bg(palette.segment_selected))
        .on_click(move |_, window, cx| on_press(window, cx))
        .child(label)
        .into_any_element()
}
