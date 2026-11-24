//! A page that scrolls the way every scrolling page of the window does: with a wheel or a trackpad, with the
//! keys when the page has focus, with a local scrollbar. The desk routes scroll keys to the active
//! page when the window has no focused control.

use crate::theme::page_scroll::{LINE, PAGE_OVERLAP};
use gpui::{
    App, FocusHandle, InteractiveElement as _, KeyDownEvent, Keystroke, ListOffset, ListState,
    Pixels, Point, ScrollHandle, Stateful, StatefulInteractiveElement as _,
};
use gpui_component::scroll::ScrollableElement as _;

/// The key context the page's root carries.
const CONTEXT: &str = "page-scroll";

/// The scrolling state of one page.
#[derive(Clone)]
pub struct PageScroll {
    handle: ScrollHandle,
    focus: FocusHandle,
}

/// Where a key moves the page.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Move {
    By(Pixels),
    Start,
    End,
}

impl PageScroll {
    /// A page at its top, with nothing focused.
    pub fn new(cx: &mut App) -> Self {
        Self {
            handle: ScrollHandle::new(),
            focus: cx.focus_handle(),
        }
    }

    #[cfg(not(feature = "boxes"))]
    pub(super) fn to_top(&self) {
        self.handle.set_offset(gpui::Point::default());
    }

    /// How far the page is scrolled down, never negative.
    #[cfg(test)]
    pub fn scrolled(&self) -> Pixels {
        -self.handle.offset().y
    }

    pub fn scroll_for(&self, key: &Keystroke) -> bool {
        scroll_for(&self.handle, key)
    }

    pub fn wrap(&self, root: Stateful<gpui::Div>) -> Stateful<gpui::Div> {
        let (handle, focus) = (self.handle.clone(), self.focus.clone());
        root.key_context(CONTEXT)
            .track_focus(&self.focus)
            .track_scroll(&self.handle)
            .overflow_y_scroll()
            .on_key_down(move |event: &KeyDownEvent, window, _| {
                if focus.is_focused(window) && scroll_for(&handle, &event.keystroke) {
                    window.refresh();
                }
            })
            .vertical_scrollbar(&self.handle)
    }

    pub fn wrap_list(&self, root: Stateful<gpui::Div>, list: &ListState) -> Stateful<gpui::Div> {
        let (state, focus) = (list.clone(), self.focus.clone());
        root.key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_key_down(move |event: &KeyDownEvent, window, _| {
                if focus.is_focused(window) && scroll_list_for(&state, &event.keystroke) {
                    window.refresh();
                }
            })
            .vertical_scrollbar(list)
    }
}

#[cfg(not(feature = "boxes"))]
pub fn end_fade(scroll: &ScrollHandle, colour: gpui::Hsla) -> impl gpui::IntoElement {
    edge_fade(scroll, colour, false)
}

#[cfg(not(feature = "boxes"))]
pub fn edge_fades(scroll: &ScrollHandle, colour: gpui::Hsla) -> impl gpui::IntoElement {
    use gpui::{ParentElement as _, Styled as _};
    gpui::div()
        .absolute()
        .inset_0()
        .child(edge_fade(scroll, colour, true))
        .child(edge_fade(scroll, colour, false))
}

#[cfg(not(feature = "boxes"))]
fn edge_fade(scroll: &ScrollHandle, colour: gpui::Hsla, is_top: bool) -> impl gpui::IntoElement {
    use gpui::Styled as _;
    let scroll = scroll.clone();
    let edge = gpui::canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let offset = scroll.offset().y;
            let viewport = scroll.bounds();
            let is_clipped = if is_top {
                scroll
                    .bounds_for_item(0)
                    .is_some_and(|first| first.top() + offset < viewport.top())
            } else {
                scroll
                    .children_count()
                    .checked_sub(1)
                    .and_then(|last| scroll.bounds_for_item(last))
                    .is_some_and(|last| last.bottom() + offset > viewport.bottom())
            };
            if is_clipped {
                window.paint_quad(gpui::fill(
                    bounds,
                    gpui::linear_gradient(
                        if is_top { 0. } else { 180. },
                        gpui::linear_color_stop(colour.opacity(0.), 0.),
                        gpui::linear_color_stop(colour, 1.),
                    ),
                ));
            }
        },
    )
    .absolute()
    .left_0()
    .right_0()
    .h(crate::theme::TEXT_BODY * 1.6);
    if is_top {
        edge.top_0()
    } else {
        edge.bottom_0()
    }
}

pub(super) fn scroll_list_for(state: &ListState, key: &Keystroke) -> bool {
    let page = state.viewport_bounds().size.height - PAGE_OVERLAP;
    let Some(moved) = key_move(key, page) else {
        return false;
    };
    match moved {
        Move::By(by) => state.scroll_by(by),
        Move::Start => state.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: gpui::px(0.),
        }),
        Move::End => state.scroll_to_end(),
    }
    true
}

/// Moves `handle` as `event` asks; says whether it was a key that scrolls.
fn scroll_for(handle: &ScrollHandle, keystroke: &Keystroke) -> bool {
    let page = handle.bounds().size.height - PAGE_OVERLAP;
    let Some(moved) = key_move(keystroke, page) else {
        return false;
    };
    handle.set_offset(moved_to(handle, moved));
    true
}

fn key_move(keystroke: &Keystroke, page: Pixels) -> Option<Move> {
    if keystroke.modifiers.platform || keystroke.modifiers.control || keystroke.modifiers.alt {
        return None;
    }
    let is_shifted = keystroke.modifiers.shift;
    let moved = match (keystroke.key.as_str(), is_shifted) {
        ("pagedown", _) | ("space", false) => Move::By(page),
        ("pageup", _) | ("space", true) => Move::By(-page),
        ("down", _) => Move::By(LINE),
        ("up", _) => Move::By(-LINE),
        ("home", _) => Move::Start,
        ("end", _) => Move::End,
        _ => return None,
    };
    Some(moved)
}

/// The offset the page has after `moved`, kept between its top and its end.
fn moved_to(handle: &ScrollHandle, moved: Move) -> Point<Pixels> {
    let longest = handle.max_offset().y;
    let wanted = match moved {
        Move::By(by) => -handle.offset().y + by,
        Move::Start => Pixels::ZERO,
        Move::End => longest,
    };
    gpui::point(handle.offset().x, -wanted.clamp(Pixels::ZERO, longest))
}

#[cfg(all(test, not(feature = "boxes")))]
mod sheet_fade_tests {
    use super::*;
    use gpui::{
        div, px, Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
        Window,
    };

    struct SheetBody;

    impl Render for SheetBody {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let scroll = ScrollHandle::new();
            div()
                .relative()
                .flex()
                .flex_col()
                .size(px(100.))
                .child(
                    super::super::dialog_frame::scrolling_body("fade-test")
                        .track_scroll(&scroll)
                        .child(div().flex_none().h(px(100.))),
                )
                .child(edge_fades(&scroll, gpui::white()))
        }
    }

    #[gpui::test]
    fn reaudit3_sheet_padding_alone_does_not_draw_a_fade(cx: &mut TestAppContext) {
        let (_, window) = cx.add_window_view(|_, _| SheetBody);
        window.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(window
                .painted_quads()
                .iter()
                .all(|quad| quad.background.as_solid().is_some()));
        });
    }
}
