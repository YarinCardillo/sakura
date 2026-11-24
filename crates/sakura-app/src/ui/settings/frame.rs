//! The window's frame, built on the kit: at the left the kit's sidebar with the search field on top
//! and a row for each section the search finds; at the right the section on screen, which the kit's
//! settings component draws as a page of rows with its title over them.
//!
//! The component also has a sidebar of its own, which keeps the page shown and the search to itself.
//! The window must say which section is on screen (its title, what is read when), open a project from
//! a link, and show the section that was open again when a search is emptied, so the sections and the
//! search are this frame's and the component is given only the page on screen; its own sidebar is
//! not shown and takes no room.

use super::rows::{self, Group};
use super::{ProjectsPage, Section, Settings};
use crate::icons;
use crate::l10n::tr;
use crate::theme::settings as sizes;
use crate::theme::{self, Palette, Theme};
use crate::ui::rem_scope::at_rem;
use gpui::{
    div, px, relative, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StyleRefinement, Styled, Window,
};
use gpui_component::input::Input;
use gpui_component::setting::{SettingPage, Settings as KitSettings};
use gpui_component::sidebar::{Sidebar, SidebarMenu, SidebarMenuItem};
use gpui_component::Icon;

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let found = self.found(cx);
        let shown = self.shown(&found);
        self.write_title(shown, window);
        let listed: Vec<Section> = found.iter().map(|(section, _)| *section).collect();
        let groups = found
            .into_iter()
            .find(|(section, _)| *section == shown)
            .map(|(_, groups)| groups)
            .unwrap_or_default();
        let navigation = self.navigation(&listed, shown, &palette, cx);
        let page = self.page(shown, groups, cx);
        let page = div()
            .debug_selector(|| "settings-panel".into())
            .flex_1()
            .min_w_0()
            .h_full()
            .child(page);
        let frame = div()
            .relative()
            .size_full()
            .flex()
            .child(navigation)
            .child(page)
            .child(still_divider());
        div()
            .size_full()
            .bg(palette.panel)
            .text_color(palette.text)
            .text_size(theme::TEXT_SMALL)
            .line_height(relative(sizes::SETTING_LINE_HEIGHT))
            .font_family(theme::FONT_UI)
            .child(at_rem(sizes::KIT_REM, frame))
    }
}

/// The line between the sections and the page is the component's divider, whose sidebar is not
/// shown: a layer over it takes the pointer, so it offers no resizing it could not do.
fn still_divider() -> impl IntoElement {
    let reach = sizes::KIT_DIVIDER_REACH;
    div()
        .id("settings-divider")
        .absolute()
        .top_0()
        .bottom_0()
        .left(sizes::SECTIONS_WIDTH - reach)
        .w(reach * 2.)
        .occlude()
}

impl Settings {
    /// The kit's sidebar: the search field, then a row for each section in `listed`; the one on screen
    /// is filled. Choosing a row moves nothing.
    fn navigation(
        &self,
        listed: &[Section],
        shown: Section,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let rows = listed.iter().map(|&section| {
            let settings = cx.weak_entity();
            SidebarMenuItem::new(tr(section.title()))
                .active(section == shown)
                .on_click(move |_, _, cx| {
                    drop(settings.update(cx, |settings, cx| settings.show(section, cx)));
                })
                .suffix(move |_, _| row_anchor(section))
        });
        // The page's divider is the line between the two: the sidebar draws none of its own.
        Sidebar::new("settings-sections")
            .w(sizes::SECTIONS_WIDTH)
            .border_r(px(0.))
            .collapsible(false)
            .header(self.search_field(palette))
            .child(SidebarMenu::new().children(rows))
    }

    /// The search field: filled, never outlined, with no ring when it has the caret.
    fn search_field(&self, palette: &Palette) -> impl IntoElement {
        let mark = Icon::new(icons::SEARCH).text_color(palette.text_tertiary);
        div()
            .debug_selector(|| "settings-search".into())
            .w_full()
            .rounded(theme::RADIUS_CONTROL)
            .bg(palette.fill)
            .child(Input::new(&self.search).prefix(mark).appearance(false))
    }

    /// The section on screen as the kit's page: its title over its rows, or the kit's own words for a
    /// search that finds nothing.
    fn page(&self, shown: Section, groups: Vec<Group>, cx: &Context<Self>) -> KitSettings {
        let page = SettingPage::new(tr(shown.title()))
            .resettable(false)
            .header_style(&page_header())
            .groups(rows::kit_groups(groups, cx));
        let hidden = StyleRefinement::default().hidden();
        KitSettings::new(self.page_key(shown, cx))
            .sidebar_width(px(0.))
            .sidebar_size_range(px(0.)..px(0.))
            .sidebar_style(&hidden)
            .page(page)
    }

    /// What names the page in the window: one for each section, each project's form, and each search,
    /// so a page that changes what it holds starts at its top and keeps no measure of another.
    fn page_key(&self, shown: Section, cx: &Context<Self>) -> SharedString {
        let form = match (&self.page, shown) {
            (ProjectsPage::Form(form), Section::Projects) => form.read(cx).name().to_string(),
            (ProjectsPage::Adding(_), Section::Projects) => "+".to_string(),
            _ => String::new(),
        };
        let query = self.search.read(cx).value();
        format!("settings-page-{}-{form}-{query}", shown.id()).into()
    }
}

/// The page's title: the size of a section's title. The hairlines are the rows'; none runs under the
/// title across the page.
fn page_header() -> StyleRefinement {
    StyleRefinement::default()
        .text_size(sizes::PAGE_TITLE)
        .font_weight(theme::WEIGHT_MEDIUM)
        .border_b(px(0.))
}

/// An empty layer over a section's row in the sidebar, under the section's name in the window: the
/// kit's row takes no name of its own, and tests and scripts find the row by this one.
fn row_anchor(section: Section) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .debug_selector(move || section.id().into())
}
