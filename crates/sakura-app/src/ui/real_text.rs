//! Test windows whose text is shaped by the Mac's own text system. GPUI's test platform gives every glyph
//! the same width whatever its font and its weight, so a layout that depends on how wide a text really is
//! (a tab that grows when its label gets heavier) could never fail there. The layout tests run here.

use crate::l10n::{self, Language};
use gpui::TestAppContext;
#[cfg(target_os = "macos")]
use std::sync::Arc;

/// Runs `test` once in every language of the interface, each time on a test application with the real
/// text system: a layout holds whatever the words are, or it fails here naming the language.
pub fn run(test: fn(&mut TestAppContext)) {
    for language in Language::ALL {
        let _named = Named(language);
        l10n::set_for_test(language);
        run_once(test, language);
    }
}

/// Says which language a failing layout test was in.
struct Named(Language);

impl Drop for Named {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("sakura: the test failed in {}", self.0.name());
        }
    }
}

/// Off the Mac there is no real text system these tests can shape with yet: each is skipped, and says so.
#[cfg(not(target_os = "macos"))]
fn run_once(_test: fn(&mut TestAppContext), language: Language) {
    eprintln!(
        "sakura: skipped in {}: this layout test needs the Mac's text system",
        language.name()
    );
}

/// Runs `test` on a test application with the real text system, and ends it as `#[gpui::test]` does.
#[cfg(target_os = "macos")]
fn run_once(test: fn(&mut TestAppContext), language: Language) {
    gpui::run_test_once(
        0,
        Box::new(move |dispatcher| {
            let text_system = Arc::new(gpui_apple::AppleTextSystem::new());
            let mut cx = TestAppContext::build_with_text_system(dispatcher, None, text_system);
            assert_eq!(
                l10n::language(),
                language,
                "the test runs where its language is set"
            );
            test(&mut cx);
            cx.run_until_parked();
            cx.update(|cx| {
                cx.background_executor().forbid_parking();
                cx.quit();
            });
            cx.run_until_parked();
        }),
    );
}
