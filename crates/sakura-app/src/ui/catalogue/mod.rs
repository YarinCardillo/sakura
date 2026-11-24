//! The catalogue: one scrollable page that shows every component of the kit in every state (rest, hover,
//! pressed, selected, disabled, focus by keyboard, a long label, an icon), then the type scale, the icon sizes,
//! the spacing steps, the control heights and the radii, each labelled with its token name. Snapshot builds
//! only; the drive step `catalogue` opens it, and a snapshot of a tall window photographs all of it.

mod controls;
mod conversation;
mod scale_page;
mod surfaces;

use crate::l10n::{tr, Key};
use crate::theme::kit::scale::{
    Typeset as _, ROW_PADDING, SPACE_L, SPACE_M, SPACE_XXL, SPACE_XXXL, TEXT_CAPTION, TEXT_TITLE,
};
use crate::theme::{Palette, Theme};
use crate::ui::kit::section_header;
use gpui::{
    div, AnyElement, App, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::input::InputState;
use gpui_component::menu::PopupMenu;

pub(super) const SAMPLE_COMMAND: &str = "cargo test -p orchard-search";

pub const TOAST_WORDS: Key = Key("catalogue.toast");

/// The page: the entities its live specimens need (two fields and an open menu).
pub struct Catalogue {
    fields: [Entity<InputState>; 2],
    answer: Entity<gpui_component::text::TextViewState>,
    menu: Entity<PopupMenu>,
    part: Option<usize>,
}

/// The part of the page each section belongs to, in the order of the sections: a snapshot of one part fits
/// the window, and the parts are stitched into one picture.
const PART_OF_SECTION: [usize; 14] = [0, 1, 1, 1, 2, 1, 3, 3, 3, 4, 4, 4, 4, 4];

impl Catalogue {
    /// The page with its specimens made: the whole page, or only `part` of it.
    pub fn new(part: Option<usize>, window: &mut Window, cx: &mut App) -> Self {
        Self {
            fields: controls::fields_made(window, cx),
            answer: conversation::answer_made(cx),
            menu: controls::menu_made(window, cx),
            part,
        }
    }

    /// Whether the toast of the page belongs to the part shown.
    pub fn shows_toast(&self) -> bool {
        self.part.is_none_or(|part| part == 4)
    }
}

impl Render for Catalogue {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let sections = [
            scale_page::type_scale(&palette),
            scale_page::icons_and_spacing(&palette),
            scale_page::heights_and_radii(&palette),
            scale_page::accent(&palette),
            controls::buttons(&palette),
            controls::chips_and_tags(&palette, cx),
            controls::choices(&palette, cx),
            controls::fields(&self.fields, &palette, cx),
            controls::menu(&self.menu, &palette),
            surfaces::rows(&palette),
            surfaces::agent_rows(&palette),
            surfaces::cards(&palette),
            surfaces::feedback(&palette, cx),
            conversation::conversation(&self.answer, &palette),
        ];
        div()
            .id("catalogue")
            .size_full()
            .overflow_y_scroll()
            .p(SPACE_XXL)
            .flex()
            .flex_col()
            .gap(SPACE_XXXL)
            .children((self.part.is_none() || self.part == Some(0)).then(|| {
                div()
                    .text_style(TEXT_TITLE)
                    .text_color(palette.text)
                    .child(tr(Key("catalogue.title")))
            }))
            .children(
                sections
                    .into_iter()
                    .zip(PART_OF_SECTION)
                    .filter(|(_, part)| self.part.is_none_or(|shown| shown == *part))
                    .map(|(section, _)| section),
            )
    }
}

/// A specimen: `element` with the name of what it shows under it, in the caption size.
pub(super) fn specimen(name: &str, element: impl IntoElement, palette: &Palette) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(SPACE_M)
        .child(element)
        .child(
            div()
                .text_style(TEXT_CAPTION)
                .text_color(palette.text_tertiary)
                .child(name.to_string()),
        )
        .into_any_element()
}

/// A group of specimens under a header named `title`, laid out in a wrapping row.
pub(super) fn group(
    id: &str,
    title: &str,
    specimens: Vec<AnyElement>,
    palette: &Palette,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(SPACE_L)
        .child(section_header(
            id.to_string(),
            title.to_string(),
            None,
            palette,
        ))
        .child(row(specimens).px(ROW_PADDING))
        .into_any_element()
}

/// A wrapping row of specimens, the room of a large step between them.
pub(super) fn row(specimens: Vec<AnyElement>) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_start()
        .gap(SPACE_XXL)
        .children(specimens)
}

#[cfg(test)]
mod tests;
