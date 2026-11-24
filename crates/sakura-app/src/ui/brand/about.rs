//! About Sakura: a small window with the lockup, the version, the licence and where the source is.
//! The same column is the content of Settings' About page ([`content`]). The lockup is the brand's
//! file; the version and the licence are the manifest's, so they cannot drift from what was built.

use super::lockup::lockup;
use crate::l10n::{tr, tr_with, Key};
use crate::theme::{self, Palette, Theme};
use crate::theme_brand::{
    ABOUT_GAP, ABOUT_HEIGHT, ABOUT_LINE_GAP, ABOUT_LOCKUP_CELL, ABOUT_WIDTH, REPOSITORY,
};
use gpui::{
    actions, div, size, AnyWindowHandle, App, AppContext as _, Bounds, Context, Div, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

actions!(sakura, [About]);

/// "About Sakura": the app menu's item and the window's title.
pub const TITLE: Key = Key("brand.about.title");

/// The About window while it is open.
struct Shown(AnyWindowHandle);

impl Global for Shown {}

/// Makes [`About`] open the window.
pub fn install(cx: &mut App) {
    cx.on_action(|_: &About, cx| open(cx));
    cx.intercept_keystrokes(|event, window, cx| {
        let is_about = cx
            .try_global::<Shown>()
            .is_some_and(|shown| shown.0 == window.window_handle());
        let key = &event.keystroke;
        let is_close = key.key == "escape" || (key.key == "w" && key.modifiers.secondary(cx));
        if is_about && is_close {
            cx.stop_propagation();
            window.remove_window();
        }
    })
    .detach();
}

/// Opens the About window, or brings it forward when it is open already.
pub fn open(cx: &mut App) {
    let shown = cx.try_global::<Shown>().map(|shown| shown.0);
    let is_forward = shown.is_some_and(|window| {
        let brought = window.update(cx, |_, window, _| crate::window::activate(window));
        brought.is_ok()
    });
    if is_forward {
        return;
    }
    let opened = cx.open_window(options(cx), |window, cx| {
        let view = cx.new(|_| AboutView);
        cx.new(|cx| gpui_component::Root::new(view, window, cx))
    });
    match opened {
        Ok(window) => cx.set_global(Shown(window.into())),
        Err(error) => eprintln!("sakura: cannot open the About window: {error}"),
    }
}

fn options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(ABOUT_WIDTH, ABOUT_HEIGHT), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(tr(TITLE).into()),
            ..TitlebarOptions::default()
        }),
        focus: crate::window::can_activate(),
        show: crate::window::can_activate(),
        is_resizable: false,
        is_minimizable: false,
        ..WindowOptions::default()
    }
}

/// The content of the About window.
pub struct AboutView;

impl Render for AboutView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        div()
            .size_full()
            .bg(palette.panel)
            .flex()
            .items_center()
            .justify_center()
            .child(content(&palette))
    }
}

/// The lockup over the version, the licence and the link to the source: the whole of the About window,
/// and of Settings' About page. It stands on the panel of `palette`.
pub fn content(palette: &Palette) -> Div {
    let version = [("version", env!("CARGO_PKG_VERSION"))];
    let license = [("license", env!("CARGO_PKG_LICENSE"))];
    let lines = div()
        .flex()
        .flex_col()
        .items_center()
        .gap(ABOUT_LINE_GAP)
        .child(
            div()
                .debug_selector(|| "about-version".into())
                .text_size(theme::TEXT_BODY)
                .text_color(palette.text)
                .child(tr_with(Key("brand.about.version"), &version)),
        )
        .child(
            div()
                .text_size(theme::TEXT_LABEL)
                .text_color(palette.text_secondary)
                .child(tr_with(Key("brand.about.license"), &license)),
        )
        .child(repository_link(palette));
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(ABOUT_GAP)
        .child(lockup(ABOUT_LOCKUP_CELL, palette.appearance))
        .child(lines)
}

/// The repository's address, which opens it in the browser.
fn repository_link(palette: &Palette) -> impl IntoElement {
    let hovered = palette.text;
    div()
        .id("about-repository")
        .debug_selector(|| "about-repository".into())
        .text_size(theme::TEXT_LABEL)
        .text_color(palette.text_secondary)
        .underline()
        .cursor_pointer()
        .hover(move |style| style.text_color(hovered))
        .on_click(|_, _, cx| cx.open_url(REPOSITORY))
        .child(REPOSITORY.trim_start_matches("https://"))
}
