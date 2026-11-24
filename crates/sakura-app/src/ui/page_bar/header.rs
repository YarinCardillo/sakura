//! The header row of the page panel: the page's name at the left (for a lane, the project it belongs to: the
//! lane's own name is on its tab and in its page), the page's own controls at the right.

use super::pill::Crumb;
use super::PageBar;
use crate::theme::{self, page_bar as bar, Palette};
use gpui::{
    div, AnyElement, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _,
};
use sakura_core::tabs::Tab;

impl PageBar {
    /// The header row of the page panel for `page`: its breadcrumb, with `controls` at the right end.
    pub fn header_row(&self, tab: &Tab, controls: AnyElement, palette: &Palette) -> AnyElement {
        let id = tab.id().as_u64();
        div()
            .debug_selector(move || format!("tab-header-{id}"))
            .flex_none()
            .child(row(self.crumb(tab.page(), false), controls, palette))
            .into_any_element()
    }
}

/// The header for a page called `crumb`, with `controls` at its right end.
fn row(crumb: Crumb, controls: AnyElement, palette: &Palette) -> AnyElement {
    let crumb = div()
        .debug_selector(|| "crumb-name".into())
        .min_w_0()
        .truncate()
        .text_size(theme::TEXT_BODY)
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.text)
        .child(crumb.in_header());
    div()
        .debug_selector(|| "page-header".into())
        .h(bar::HEADER_HEIGHT)
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .gap(bar::BAR_GAP)
        .pl(bar::HEADER_PADDING_START)
        .pr(bar::BAR_PADDING_END)
        .border_b_1()
        .border_color(palette.border)
        .child(crumb)
        .child(div().flex_1())
        .child(controls)
        .into_any_element()
}
