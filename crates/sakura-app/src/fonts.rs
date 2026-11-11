//! Geist Mono, embedded in the binary (SIL Open Font License 1.1, see `assets/fonts/OFL.txt`), for what is
//! literally code: diffs, terminals, commands, paths and shas. It is kept for its plain zero (stylistic
//! set 9); the Mac's own monospaced face slashes it. The interface itself is set in the system font, which
//! needs no file here.
//!
//! The terminal's font is decided here too: its family and, for the icons a shell prints from the
//! private-use area (a Nerd Font's), an installed Nerd Font tried after it. Nothing is bundled for that:
//! what is installed is looked at once, at start.

use crate::theme;
use gpui::{font, px, App, FontFallbacks, SharedString, TextSystem};
use std::borrow::Cow;
use std::cell::RefCell;
use std::sync::OnceLock;

const FACES: [&[u8]; 3] = [
    include_bytes!("../assets/fonts/GeistMono-Regular.otf"),
    include_bytes!("../assets/fonts/GeistMono-Medium.otf"),
    include_bytes!("../assets/fonts/GeistMono-Bold.otf"),
];

/// The faces that only add the Nerd Font icons and change no letter, the one made for a grid first.
const SYMBOL_FACES: [&str; 2] = ["Symbols Nerd Font Mono", "Symbols Nerd Font"];
/// What every Nerd Font family has in its name.
const NERD_FONT: &str = "Nerd Font";

/// The installed Nerd Font, looked for once at start; `None` inside when there is none.
static INSTALLED_NERD_FONT: OnceLock<Option<String>> = OnceLock::new();

thread_local! {
    /// What the person chose for the terminal in Settings: a family and a size in points; none of either
    /// is Sakura's own (Geist Mono at the theme's size). Kept here so that the one function that decides
    /// the terminal's font reads it wherever a terminal is styled, which is always the interface's thread.
    static CHOSEN: RefCell<(Option<String>, Option<u8>)> = const { RefCell::new((None, None)) };
}

/// The terminal's font: its family and size, and the families tried after it for a glyph it lacks.
pub struct TerminalFont {
    pub family: SharedString,
    pub size: f32,
    pub fallbacks: Option<FontFallbacks>,
}

/// Takes the person's choice of family and size for the terminal (none of either: Sakura's own). The
/// terminals take it the next time they are styled.
pub fn choose_terminal(family: Option<String>, size: Option<u8>) {
    let size = size.filter(|size| theme::TERMINAL_TEXT_SIZES.contains(size));
    CHOSEN.with(|chosen| *chosen.borrow_mut() = (family, size));
}

/// The installed families whose letters all have one width, by name: what a terminal can be set in.
/// Every family is measured, which takes a moment: call it off the main thread.
pub fn monospaced_families(text: &TextSystem) -> Vec<String> {
    let size = px(theme::TERMINAL_TEXT);
    let is_monospaced = |family: &String| {
        let id = text.resolve_font(&font(SharedString::from(family.clone())));
        let width = |letter| text.advance(id, size, letter).map(|advance| advance.width);
        matches!((width('i'), width('M')), (Ok(narrow), Ok(wide)) if narrow == wide)
    };
    let mut families: Vec<String> = text
        .all_font_names()
        .into_iter()
        .filter(|family| !family.starts_with('.') && family != theme::FONT_MONO)
        .filter(is_monospaced)
        .collect();
    families.sort_unstable();
    families.dedup();
    families
}

/// Registers the embedded faces with the text system and looks once for an installed Nerd Font. A
/// failure is logged and the system fallback is used.
pub fn register(cx: &App) {
    let faces = FACES.iter().map(|face| Cow::Borrowed(*face)).collect();
    if let Err(error) = cx.text_system().add_fonts(faces) {
        eprintln!("sakura: cannot register the embedded fonts: {error}");
    }
    INSTALLED_NERD_FONT.get_or_init(|| nerd_font(&cx.text_system().all_font_names()));
}

/// The font every terminal is drawn in. This is the one place that decides it: the family and size the
/// person chose in Settings, else Sakura's own, with the installed Nerd Font after it.
pub fn terminal_font() -> TerminalFont {
    let nerd = INSTALLED_NERD_FONT.get().cloned().flatten();
    let (family, size) = CHOSEN.with(|chosen| chosen.borrow().clone());
    TerminalFont {
        family: family.map_or(theme::FONT_MONO.into(), SharedString::from),
        size: size.map_or(theme::TERMINAL_TEXT, f32::from),
        fallbacks: nerd.map(|nerd| FontFallbacks::from_fonts(vec![nerd])),
    }
}

/// The family among `installed` to take the Nerd Font icons from: a symbols-only face when there is
/// one, else the first family patched with them.
fn nerd_font(installed: &[String]) -> Option<String> {
    let is_installed = |name: &&&str| installed.iter().any(|family| family == **name);
    let symbols = SYMBOL_FACES.iter().find(is_installed);
    let patched = || installed.iter().find(|family| family.contains(NERD_FONT));
    symbols
        .map(|name| name.to_string())
        .or_else(|| patched().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn families(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn nerd_font_a_symbols_only_face_is_taken_before_a_patched_family() {
        let installed = families(&["Helvetica", "JetBrainsMono Nerd Font", "Symbols Nerd Font"]);
        assert_eq!(nerd_font(&installed).as_deref(), Some("Symbols Nerd Font"));
        let with_mono = families(&["Symbols Nerd Font", "Symbols Nerd Font Mono"]);
        assert_eq!(
            nerd_font(&with_mono).as_deref(),
            Some("Symbols Nerd Font Mono"),
            "the face made for a grid first"
        );
    }

    #[test]
    fn nerd_font_any_patched_family_serves_when_no_symbols_face_is_installed() {
        let installed = families(&["Menlo", "Hack Nerd Font Mono"]);
        assert_eq!(
            nerd_font(&installed).as_deref(),
            Some("Hack Nerd Font Mono")
        );
    }

    #[test]
    fn terminal_font_is_the_person_s_choice_and_sakura_s_own_without_one() {
        choose_terminal(None, None);
        let own = terminal_font();
        assert_eq!(own.family.as_ref(), theme::FONT_MONO);
        assert_eq!(own.size, theme::TERMINAL_TEXT);
        choose_terminal(Some("Menlo".into()), Some(15));
        let chosen = terminal_font();
        assert_eq!((chosen.family.as_ref(), chosen.size), ("Menlo", 15.));
        choose_terminal(Some("Menlo".into()), Some(72));
        assert_eq!(
            terminal_font().size,
            theme::TERMINAL_TEXT,
            "a size that is not offered is not taken"
        );
        choose_terminal(None, None);
    }

    #[test]
    fn nerd_font_none_installed_leaves_the_fallback_to_the_system() {
        assert_eq!(nerd_font(&families(&["Menlo", "Helvetica"])), None);
    }
}
