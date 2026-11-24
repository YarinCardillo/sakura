//! Enter in the lane's composer, pressed as a key: nothing while an input method composes, a line break
//! inside a code fence, a send otherwise, and always a send with Cmd.

use super::lane_composer::{ComposerEvent, LaneComposer};
use crate::theme::{Appearance, Theme};
use gpui::{
    AppContext as _, Entity, EntityInputHandler as _, Focusable as _, TestAppContext,
    VisualTestContext,
};
use gpui_component::input::InputEvent;
use std::cell::RefCell;
use std::rc::Rc;

type Sent = Rc<RefCell<Vec<String>>>;

fn open(cx: &mut TestAppContext) -> (Entity<LaneComposer>, Sent, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
    });
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let sent: Sent = Rc::default();
    let heard = sent.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let composer = cx.new(|cx| LaneComposer::new(window, cx));
        cx.subscribe(&composer, move |_, _, event: &ComposerEvent, _| {
            let ComposerEvent::Send(text) = event;
            heard.borrow_mut().push(text.clone());
        })
        .detach();
        *kept.borrow_mut() = Some(composer.clone());
        gpui_component::Root::new(composer, window, cx)
    });
    let composer = slot.borrow_mut().take().expect("the composer");
    cx.update(|window, cx| {
        window.activate_window();
        let field = composer.read(cx).field();
        window.focus(&field.read(cx).focus_handle(cx), cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (composer, sent, cx)
}

fn text(composer: &Entity<LaneComposer>, cx: &mut VisualTestContext) -> String {
    cx.update(|_, cx| composer.read(cx).field().read(cx).value().to_string())
}

#[gpui::test]
fn enter_with_text_sends_it(cx: &mut TestAppContext) {
    let (_, sent, cx) = open(cx);
    cx.simulate_input("wave");
    cx.simulate_keystrokes("enter");
    assert_eq!(*sent.borrow(), ["wave"]);
}

#[gpui::test]
fn enter_while_an_accent_is_marked_sends_nothing(cx: &mut TestAppContext) {
    let (composer, sent, cx) = open(cx);
    cx.simulate_input("perche");
    cx.update(|window, cx| {
        let field = composer.read(cx).field();
        field.update(cx, |field, cx| {
            field.replace_and_mark_text_in_range(None, "`", None, window, cx)
        });
    });
    cx.simulate_keystrokes("enter");
    assert!(sent.borrow().is_empty(), "sent {:?}", sent.borrow());
}

#[gpui::test]
fn enter_inside_a_code_fence_breaks_the_line(cx: &mut TestAppContext) {
    let (composer, sent, cx) = open(cx);
    cx.simulate_input("```rust");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("let a = 1;");
    assert!(sent.borrow().is_empty(), "sent {:?}", sent.borrow());
    assert_eq!(text(&composer, cx), "```rust\nlet a = 1;");
}

#[gpui::test]
fn cmd_enter_inside_a_code_fence_sends(cx: &mut TestAppContext) {
    let (_, sent, cx) = open(cx);
    cx.simulate_input("```rust\nlet a = 1;");
    cx.simulate_keystrokes("cmd-enter");
    assert_eq!(*sent.borrow(), ["```rust\nlet a = 1;"]);
}

#[gpui::test]
fn a_return_heard_while_an_accent_is_marked_sends_nothing(cx: &mut TestAppContext) {
    let (composer, sent, cx) = open(cx);
    cx.simulate_input("perche");
    cx.update(|window, cx| {
        let field = composer.read(cx).field();
        field.update(cx, |field, cx| {
            field.replace_and_mark_text_in_range(None, "`", None, window, cx);
            cx.emit(InputEvent::PressEnter {
                secondary: false,
                shift: false,
            });
        });
    });
    cx.run_until_parked();
    assert!(sent.borrow().is_empty(), "sent {:?}", sent.borrow());
}
