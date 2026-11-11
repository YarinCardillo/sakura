//! The palette driven by keys and pointer on GPUI's test platform.

use super::commands::{PlayState, ShownTask, Situation};
use super::view::{Palette, PaletteEvent};
use super::{install, Ask};
use crate::l10n::LanguageChoice;
use crate::menus::{NewTask, PanesShown};
use crate::theme::{Appearance, Theme};
use crate::ui::right_pane::PaneView;
use gpui::{
    div, point, px, AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _,
    Render, Styled as _, TestAppContext, VisualTestContext, Window,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The window's content: an empty page with the palette over it.
struct Host {
    palette: Entity<Palette>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.palette.clone())
    }
}

/// The window on the task `m1`, its agent stopped, two other tasks open.
fn situation() -> Situation {
    Situation {
        tasks: vec!["m1".into(), "m2".into(), "m3".into()],
        shown: Some(ShownTask {
            name: "m1".into(),
            view: PaneView::Modifiche,
            can_publish: true,
            can_archive: true,
            play: PlayState::Absent,
        }),
        panes: PanesShown {
            sidebar: true,
            changes: true,
        },
        language: LanguageChoice::Automatic,
        can_close_tab: true,
        panel_left: gpui::Pixels::ZERO,
    }
}

type Asked = Rc<RefCell<Vec<Ask>>>;

/// A test window with the palette installed and closed; what it asks of the desk is recorded.
fn open(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<Palette>, Asked) {
    open_on(cx, Rc::new(RefCell::new(situation())))
}

/// The same, with the window's situation read from `now` each time the palette asks for it.
fn open_on(
    cx: &mut TestAppContext,
    now: Rc<RefCell<Situation>>,
) -> (&mut VisualTestContext, Entity<Palette>, Asked) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
        crate::menus::install(cx);
    });
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let asked: Asked = Rc::new(RefCell::new(Vec::new()));
    let record = asked.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let palette = cx.new(|cx| Palette::new(window, cx));
        let now = now.clone();
        install(&palette, move |_| now.borrow().clone(), window, cx);
        cx.subscribe(&palette, move |_, _, event: &PaletteEvent, _| {
            let PaletteEvent::Asked(ask) = event;
            record.borrow_mut().push(ask.clone());
        })
        .detach();
        *kept.borrow_mut() = Some(palette.clone());
        let host = cx.new(|_| Host { palette });
        gpui_component::Root::new(host, window, cx)
    });
    let palette = slot.borrow_mut().take().expect("the palette was built");
    cx.update(|window, _| window.activate_window());
    draw(cx);
    (cx, palette, asked)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn is_open(palette: &Entity<Palette>, cx: &mut VisualTestContext) -> bool {
    cx.update(|_, cx| palette.read(cx).is_open())
}

fn press(keys: &str, cx: &mut VisualTestContext) {
    cx.simulate_keystrokes(keys);
    draw(cx);
}

#[gpui::test]
fn palette_cmd_k_opens_it_at_once_with_the_caret_in_its_field(cx: &mut TestAppContext) {
    let (cx, palette, _) = open(cx);
    assert!(!is_open(&palette, cx));
    assert!(cx.debug_bounds("palette-panel").is_none());
    press("cmd-k", cx);
    assert!(is_open(&palette, cx));
    assert!(
        cx.debug_bounds("palette-panel").is_some(),
        "drawn in the frame after the key, with no entrance"
    );
    cx.simulate_input("ver");
    draw(cx);
    let typed = cx.update(|_, cx| palette.read(cx).query(cx));
    assert_eq!(
        typed.as_ref(),
        "ver",
        "typing goes to the field without a click"
    );
}

#[gpui::test]
fn palette_cmd_k_again_or_escape_closes_it_and_gives_the_keyboard_back(cx: &mut TestAppContext) {
    let (cx, palette, _) = open(cx);
    press("cmd-k", cx);
    press("cmd-k", cx);
    assert!(!is_open(&palette, cx));
    press("cmd-k", cx);
    press("escape", cx);
    assert!(!is_open(&palette, cx));
    assert!(cx.debug_bounds("palette-panel").is_none());
    let has_focus = cx.update(|window, cx| window.focused(cx).is_some());
    assert!(
        !has_focus,
        "nothing had the keyboard before, nothing has it after"
    );
}

#[gpui::test]
fn palette_a_chosen_place_is_asked_of_the_desk_and_the_palette_goes(cx: &mut TestAppContext) {
    let (cx, palette, asked) = open(cx);
    press("cmd-k", cx);
    cx.simulate_input("m2");
    draw(cx);
    press("enter", cx);
    assert_eq!(*asked.borrow(), [Ask::ShowTask("m2".into())]);
    assert!(!is_open(&palette, cx));
}

#[gpui::test]
fn palette_the_first_command_is_the_shown_task_s_and_enter_runs_it(cx: &mut TestAppContext) {
    let (cx, _, asked) = open(cx);
    press("cmd-k", cx);
    press("enter", cx);
    let files = Ask::ShowView {
        task: "m1".into(),
        view: PaneView::Files,
    };
    assert_eq!(*asked.borrow(), [files]);
}

#[gpui::test]
fn palette_a_window_command_runs_the_window_s_own_action(cx: &mut TestAppContext) {
    crate::l10n::set_for_test(crate::l10n::Language::Italian);
    let (cx, palette, asked) = open(cx);
    let seen = Rc::new(Cell::new(0));
    let count = seen.clone();
    cx.update(|_, cx| {
        cx.on_action(move |_: &NewTask, _| count.set(count.get() + 1));
    });
    press("cmd-k", cx);
    cx.simulate_input("Nuovo lavoro");
    draw(cx);
    press("enter", cx);
    assert_eq!(
        seen.get(),
        1,
        "the same action as its key and its menu item"
    );
    assert!(asked.borrow().is_empty());
    assert!(!is_open(&palette, cx));
}

#[gpui::test]
fn palette_a_press_outside_closes_it_and_one_inside_does_not(cx: &mut TestAppContext) {
    let (cx, palette, _) = open(cx);
    press("cmd-k", cx);
    let panel = cx
        .debug_bounds("palette-panel")
        .expect("the panel is drawn");
    cx.simulate_mouse_down(panel.center(), gpui::MouseButton::Left, Modifiers::none());
    draw(cx);
    assert!(is_open(&palette, cx));
    let outside = point(px(8.), px(8.));
    cx.simulate_mouse_down(outside, gpui::MouseButton::Left, Modifiers::none());
    draw(cx);
    assert!(!is_open(&palette, cx));
}

#[gpui::test]
fn palette_moving_the_selection_or_the_pointer_moves_nothing(cx: &mut TestAppContext) {
    crate::l10n::set_for_test(crate::l10n::Language::Italian);
    let (cx, _, _) = open(cx);
    press("cmd-k", cx);
    // Few enough commands that the list does not scroll: a row that grew would grow the panel.
    cx.simulate_input("mostra");
    draw(cx);
    let before = cx
        .debug_bounds("palette-panel")
        .expect("the panel is drawn");
    for _ in 0..3 {
        press("down", cx);
        assert_eq!(cx.debug_bounds("palette-panel"), Some(before));
    }
    press("up", cx);
    assert_eq!(cx.debug_bounds("palette-panel"), Some(before));
    let over_a_row = before.center() + point(px(0.), px(24.));
    cx.simulate_mouse_move(over_a_row, None, Modifiers::none());
    draw(cx);
    assert_eq!(cx.debug_bounds("palette-panel"), Some(before));
}

#[gpui::test]
fn palette_a_command_of_a_task_no_longer_shown_does_nothing(cx: &mut TestAppContext) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let now = Rc::new(RefCell::new(situation()));
    let (cx, palette, asked) = open_on(cx, now.clone());
    press("cmd-k", cx);
    cx.simulate_input("Publish");
    draw(cx);
    // Another tab is chosen by its key while the palette is open: the window now shows m2.
    if let Some(shown) = now.borrow_mut().shown.as_mut() {
        shown.name = "m2".into();
    }
    press("enter", cx);
    assert!(
        asked.borrow().is_empty(),
        "Publish was listed for m1 and must not publish m2: {:?}",
        asked.borrow()
    );
    assert!(!is_open(&palette, cx));
}

#[gpui::test]
fn palette_a_command_that_stopped_being_possible_does_nothing(cx: &mut TestAppContext) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let now = Rc::new(RefCell::new(situation()));
    let (cx, _, asked) = open_on(cx, now.clone());
    press("cmd-k", cx);
    cx.simulate_input("Publish");
    draw(cx);
    // Its agent started a turn: the work shown cannot be published now.
    if let Some(shown) = now.borrow_mut().shown.as_mut() {
        shown.can_publish = false;
    }
    press("enter", cx);
    assert!(asked.borrow().is_empty(), "{:?}", asked.borrow());
}

#[gpui::test]
fn palette_a_place_that_is_gone_is_not_asked_for(cx: &mut TestAppContext) {
    let now = Rc::new(RefCell::new(situation()));
    let (cx, _, asked) = open_on(cx, now.clone());
    press("cmd-k", cx);
    cx.simulate_input("m3");
    draw(cx);
    now.borrow_mut().tasks.retain(|task| task != "m3");
    press("enter", cx);
    assert!(asked.borrow().is_empty(), "{:?}", asked.borrow());
}

#[gpui::test]
fn palette_opened_with_the_pointer_takes_no_caret_until_its_field_is_pressed(
    cx: &mut TestAppContext,
) {
    let (cx, palette, _) = open(cx);
    // The pointer was used last: the palette is opened from a menu or a button, not by its key.
    cx.simulate_click(point(px(8.), px(8.)), Modifiers::none());
    cx.update(|window, cx| {
        palette.update(cx, |palette, cx| palette.open(&situation(), window, cx));
    });
    draw(cx);
    assert!(is_open(&palette, cx));
    cx.simulate_input("ver");
    draw(cx);
    let typed = cx.update(|_, cx| palette.read(cx).query(cx));
    assert_eq!(typed.as_ref(), "", "no caret was put in the field");
    press("escape", cx);
    assert!(
        !is_open(&palette, cx),
        "the palette itself has the keyboard: Esc closes it"
    );
}

#[gpui::test]
fn palette_is_centred_in_the_panel_and_not_in_the_window(cx: &mut TestAppContext) {
    let panel_left = px(244.);
    let now = Situation {
        panel_left,
        ..situation()
    };
    let (cx, _, _) = open_on(cx, Rc::new(RefCell::new(now)));
    press("cmd-k", cx);
    let panel = cx
        .debug_bounds("palette-panel")
        .expect("the panel is drawn");
    let window_width = cx.update(|window, _| window.viewport_size().width);
    assert_eq!(panel.center().x, (panel_left + window_width) / 2.);
}
