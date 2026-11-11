//! The colours of code in the file view, from the window's own palette: comments recede, strings take
//! the green of the palette and numbers its amber, the words of the language are quieter than the names
//! the person wrote. Nothing is blue and the accent keeps its one meaning. The kit's editor reads them
//! from the kit's theme, where [`install`] puts them each time the appearance changes.

use crate::theme::{Palette, Theme};
use gpui::{App, Hsla, SharedString};
use gpui_component::highlighter::SyntaxColors;
use serde_json::{json, Value};
use std::sync::Arc;

/// Gives the kit's editor the palette's colours for its text, its gutter and its syntax.
pub fn install(cx: &mut App) {
    let palette = Theme::palette(cx);
    let mut theme = (*gpui_component::Theme::global(cx).highlight_theme).clone();
    let style = &mut theme.style;
    style.editor_background = Some(palette.panel);
    style.editor_foreground = Some(palette.text);
    style.editor_gutter_background = Some(palette.panel);
    style.editor_line_number = Some(palette.text_tertiary);
    // The line the caret is on looks like every other: a click moves nothing and lights nothing.
    style.editor_active_line_number = Some(palette.text_tertiary);
    style.editor_active_line = Some(gpui::transparent_black());
    style.syntax = syntax_colours(&palette);
    gpui_component::Theme::global_mut(cx).highlight_theme = Arc::new(theme);
}

/// What the kit's highlighter is requested to colour `path`: the file's extension, or its whole name
/// when it has none (`Makefile`, `Dockerfile`), which is how the highlighter finds a syntax.
pub fn language_of(path: &str) -> SharedString {
    let name = path.rsplit('/').next().unwrap_or(path);
    let extension = name.rsplit_once('.').map(|(_, extension)| extension);
    let token = extension.filter(|extension| !extension.is_empty());
    SharedString::from(token.unwrap_or(name).to_string())
}

/// The palette's colour of each kind of token the highlighter names.
fn syntax_colours(palette: &Palette) -> SyntaxColors {
    let quiet = palette.text_secondary;
    let names: [(&[&str], Hsla); 5] = [
        (&["comment", "comment.doc"], palette.text_tertiary),
        (
            &["string", "string.escape", "string.regex"],
            palette.success,
        ),
        (&["number", "boolean", "constant"], palette.warning),
        (&["keyword", "operator", "preproc", "punctuation"], quiet),
        (
            &[
                "function",
                "type",
                "variable",
                "property",
                "tag",
                "attribute",
            ],
            palette.text,
        ),
    ];
    let colours = names.iter().flat_map(|(names, colour)| {
        names
            .iter()
            .map(move |name| (name.to_string(), json!({ "color": colour })))
    });
    let colours: serde_json::Map<String, Value> = colours.collect();
    // Every name above is a field of the kit's colours; were one not, the code keeps the text colour.
    serde_json::from_value(Value::Object(colours)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_is_the_extension_or_the_whole_name() {
        assert_eq!(language_of("src/app/main.ts").as_ref(), "ts");
        assert_eq!(language_of("a.spec.tsx").as_ref(), "tsx");
        assert_eq!(language_of("Makefile").as_ref(), "Makefile");
        assert_eq!(language_of("docker/Dockerfile").as_ref(), "Dockerfile");
        assert_eq!(language_of(".gitignore").as_ref(), "gitignore");
        assert_eq!(language_of("odd.").as_ref(), "odd.");
    }

    #[test]
    fn every_token_takes_a_colour_of_the_palette_and_none_is_blue_or_the_accent() {
        for palette in [Palette::dark(), Palette::light()] {
            let colours = syntax_colours(&palette);
            let named = [
                "comment", "string", "number", "keyword", "function", "type", "operator",
            ];
            for name in named {
                let colour = colours.style(name).and_then(|style| style.color);
                let colour = colour.unwrap_or_else(|| panic!("{name} has a colour"));
                let is_blue = (0.5..0.75).contains(&colour.h) && colour.s > 0.2;
                assert!(!is_blue, "{name}");
                assert_ne!(colour, palette.attention, "{name}");
            }
            let comment = colours.style("comment").and_then(|style| style.color);
            assert_eq!(comment, Some(palette.text_tertiary));
        }
    }
}
