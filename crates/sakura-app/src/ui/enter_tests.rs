//! Enter in the fields that send: never while an input method composes, a line break inside a code fence,
//! Cmd-Enter always sends.

use super::enter::{guard_enter, is_composing, is_fence_open};
use crate::theme::{Appearance, Theme};
use gpui::{AppContext as _, Entity, EntityInputHandler as _, TestAppContext, VisualTestContext};
use gpui_component::input::{Enter, TextareaState};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn fence_open_no_fence_is_closed() {
    assert!(!is_fence_open(""));
    assert!(!is_fence_open("a plain line\nand another"));
}

#[test]
fn fence_open_after_one_fence_line_is_open() {
    assert!(is_fence_open("see:\n```rust\nfn main() {"));
    assert!(is_fence_open("```"));
}

#[test]
fn fence_open_after_a_closed_block_is_closed() {
    assert!(!is_fence_open("```\ncode\n```\nafter"));
}

#[test]
fn fence_open_a_fence_inside_a_line_does_not_count() {
    assert!(!is_fence_open("use ``` in the middle of a line"));
}

fn open(cx: &mut TestAppContext) -> (Entity<TextareaState>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        Theme::set(Appearance::Dark, cx);
    });
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let field = cx.new(|cx| TextareaState::new(window, cx).submit_on_enter(true));
        *kept.borrow_mut() = Some(field.clone());
        let host = cx.new(|_| Empty);
        gpui_component::Root::new(host, window, cx)
    });
    let field = slot.borrow_mut().take().expect("the field");
    (field, cx)
}

struct Empty;

impl gpui::Render for Empty {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div()
    }
}

#[gpui::test]
fn is_composing_a_marked_dead_key_is_composing_until_it_is_committed(cx: &mut TestAppContext) {
    let (field, cx) = open(cx);
    cx.update(|window, cx| {
        assert!(!is_composing(&field, window, cx));
        field.update(cx, |field, cx| {
            field.replace_and_mark_text_in_range(None, "`", None, window, cx)
        });
        assert!(is_composing(&field, window, cx), "the accent is marked");
        field.update(cx, |field, cx| field.unmark_text(window, cx));
        assert!(!is_composing(&field, window, cx));
    });
}

fn enter() -> Enter {
    Enter {
        secondary: false,
        shift: false,
    }
}

#[gpui::test]
fn guard_enter_inside_a_fence_inserts_the_break_and_stops_the_key(cx: &mut TestAppContext) {
    let (field, cx) = open(cx);
    cx.update(|window, cx| {
        field.update(cx, |field, cx| field.insert("```\nlet a = 1;", window, cx));
        guard_enter(field.clone())(&enter(), window, cx);
        assert_eq!(field.read(cx).value().as_ref(), "```\nlet a = 1;\n");
    });
}

#[gpui::test]
fn guard_enter_outside_a_fence_or_while_composing_does_nothing(cx: &mut TestAppContext) {
    let (field, cx) = open(cx);
    cx.update(|window, cx| {
        field.update(cx, |field, cx| field.insert("wave", window, cx));
        guard_enter(field.clone())(&enter(), window, cx);
        assert_eq!(field.read(cx).value().as_ref(), "wave");
        field.update(cx, |field, cx| field.set_value("", window, cx));
        field.update(cx, |field, cx| field.insert("```\ncode", window, cx));
        field.update(cx, |field, cx| {
            field.replace_and_mark_text_in_range(None, "e", None, window, cx)
        });
        guard_enter(field.clone())(&enter(), window, cx);
        assert_eq!(field.read(cx).value().as_ref(), "```\ncodee");
    });
}
