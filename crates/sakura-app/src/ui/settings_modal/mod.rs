//! Settings as a modal inside the window, over the dimmed page: a list of sections at the left, the section
//! at the right as groups of rows (a name, the sentence under it, its control). The X and Escape close it.
//! A change applies at once; there is no Save. It is about the app and what the app touches: the rules of
//! the work (pace, reviewers, lanes at once) are on the Engine page.
//!
//! The modal never takes the caret by itself: a field has it once the person presses it.

mod about;
mod accent;
mod accounts;
pub(super) mod connectors;
#[allow(dead_code)]
#[path = "../settings/controls.rs"]
pub(crate) mod controls;
mod general;
pub(super) mod group;
pub(crate) use group::button as group_button;
mod login_item;
mod new_master;
mod notifications;
mod projects;
mod shortcuts;
mod updates;
pub(in crate::ui) mod versioned_connectors;

use super::dialog_guard::DialogToken;
use super::engine_acts::EngineActs;
use super::project_add::ProjectAdds;
use crate::data::engine_facts::EngineFacts;
use crate::data::source::Source;
use crate::l10n::{tr, Key};
use crate::theme::settings_modal as sizes;
use crate::theme::{self, Palette, Theme};
use crate::theme_brand::Accent;
use crate::ui::enter::is_composing;
use gpui::{
    div, transparent_black, AnyElement, App, AppContext as _, Context, Entity, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::WindowExt as _;
use sakura_core::engine::snapshot::Snapshot;
use sakura_core::error::BridgeError;
use std::path::PathBuf;

/// The key that closes the modal.
const ESCAPE: &str = "escape";

/// The sections, in the order of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    Projects,
    Accounts,
    Connectors,
    Notifications,
    Shortcuts,
    Updates,
    About,
}

impl Section {
    /// Every section, in the order of the list.
    pub const ALL: [Section; 8] = [
        Section::General,
        Section::Projects,
        Section::Accounts,
        Section::Connectors,
        Section::Notifications,
        Section::Shortcuts,
        Section::Updates,
        Section::About,
    ];

    /// What names the section in the window.
    pub fn id(self) -> &'static str {
        match self {
            Section::General => "general",
            Section::Projects => "projects",
            Section::Accounts => "accounts",
            Section::Connectors => "connectors",
            Section::Notifications => "notifications",
            Section::Shortcuts => "shortcuts",
            Section::Updates => "updates",
            Section::About => "about",
        }
    }

    fn title(self) -> Key {
        match self {
            Section::General => Key("settings_modal.section.general"),
            Section::Projects => Key("settings.section.projects"),
            Section::Accounts => Key("settings.section.accounts"),
            Section::Connectors => Key("settings_modal.section.connectors"),
            Section::Notifications => Key("settings.section.notifications"),
            Section::Shortcuts => Key("settings.section.shortcuts"),
            Section::Updates => Key("settings.section.updates"),
            Section::About => Key("settings.section.about"),
        }
    }
}

/// The two fields of the form that adds a feed or a program.
struct AddForm {
    name: Entity<InputState>,
    address: Entity<InputState>,
}

/// The modal's view.
pub struct SettingsModal {
    section: Section,
    source: Source,
    facts: Entity<EngineFacts>,
    /// The engine's controls the modal presses, and what the engine said of them.
    acts: Entity<EngineActs>,
    /// Adding a project, and what the engine refused of the last folder.
    adds: Entity<ProjectAdds>,
    /// The person's home: where the login item is.
    home: PathBuf,
    /// The path of the engine's command, and why the last one written names no command.
    path: Entity<InputState>,
    refused: Option<BridgeError>,
    login_enabled: bool,
    login_request: u64,
    /// The account being renamed, and the field it is renamed in.
    renaming: Option<String>,
    name_field: Entity<InputState>,
    /// The kind of connector whose row is open, the fields of each kind's add form, and what the form
    /// last refused of what was written.
    open_connector: Option<String>,
    add_forms: Vec<(&'static str, AddForm)>,
    form_problem: Option<SharedString>,
    /// The section's scroll, and how far it could scroll when a form was opened: once it can scroll further
    /// the form is laid out, and the section scrolls to its end so the form is in view.
    scroll: ScrollHandle,
    /// What the engine lists for its accounts' programs: the models and the efforts the menu of a new Master
    /// conversation offers.
    engine_models: Option<sakura_core::engine::rules::Rules>,
    _engine_models: Option<gpui::Task<()>>,
    /// The keyboard stops of the accent swatches, in the order of the row.
    accent_focus: Vec<FocusHandle>,
    reveal_from: Option<gpui::Pixels>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    open: Option<DialogToken>,
}

/// Opens the modal over the window, or does nothing when one is open already.
pub fn open(
    source: Source,
    facts: Entity<EngineFacts>,
    section: Section,
    window: &mut Window,
    cx: &mut App,
) -> Option<Entity<SettingsModal>> {
    if window.has_active_dialog(cx) {
        return None;
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let modal = cx.new(|cx| SettingsModal::new(source, facts, home, window, cx));
    modal.update(cx, |modal, cx| {
        modal.show(section, cx);
        modal.read_engine_models(cx);
    });
    let shown = modal.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        let closing = shown.downgrade();
        crate::ui::dialog_frame::placed(dialog, crate::ui::dialog_frame::Width::Large, window)
            .rounded(sizes::RADIUS)
            .overlay(super::tour::active_step(cx) != Some(super::tour::Step::Connectors))
            .on_close(move |_, _, cx| {
                drop(closing.update(cx, |modal, cx| modal.dismiss(cx)));
            })
            .child(shown.clone())
    });
    Some(modal)
}

impl SettingsModal {
    /// The modal over `source`, in the home `home`, on the General section.
    pub fn new(
        source: Source,
        facts: Entity<EngineFacts>,
        home: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let acts = cx.new(|cx| EngineActs::new(source.clone(), facts.clone(), cx));
        let adds = cx.new(|_| ProjectAdds::new(source.clone(), facts.clone()));
        let kept = crate::prefs::Preferences::try_current(cx).and_then(|prefs| prefs.engine_bin);
        let shown = kept.or_else(|| source.command().map(|path| path.display().to_string()));
        let path = cx.new(|cx| {
            controls::state(Key("settings.engine.placeholder"), window, cx)
                .default_value(shown.unwrap_or_default())
        });
        let name_field =
            cx.new(|cx| controls::state(Key("settings_modal.accounts.name_hint"), window, cx));
        let add_forms = connectors::add_forms(window, cx);
        let owner = window.window_handle();
        let modal = cx.weak_entity();
        let escape = cx.intercept_keystrokes(move |event, window, cx| {
            if event.keystroke.key == ESCAPE && window.window_handle() == owner {
                drop(modal.update(cx, |modal, cx| {
                    if modal.open.is_some() && window.has_active_dialog(cx) {
                        modal.close(window, cx);
                        cx.stop_propagation();
                    }
                }));
            }
        });
        let subscriptions = vec![
            escape,
            cx.observe(&facts, |_, _, cx| cx.notify()),
            cx.observe(&acts, |_, _, cx| cx.notify()),
            cx.observe(&adds, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &path,
                window,
                |modal, path, event: &InputEvent, window, cx| {
                    let is_entered = matches!(event, InputEvent::PressEnter { .. })
                        && !is_composing(path, window, cx);
                    if is_entered || matches!(event, InputEvent::Blur) {
                        modal.save_engine_path(cx);
                    }
                },
            ),
        ];
        let mut modal = Self {
            section: Section::General,
            source,
            facts,
            acts,
            adds,
            home,
            path,
            refused: None,
            login_enabled: false,
            login_request: 0,
            renaming: None,
            name_field,
            open_connector: None,
            add_forms,
            form_problem: None,
            scroll: ScrollHandle::new(),
            engine_models: None,
            _engine_models: None,
            accent_focus: Accent::ALL
                .map(|_| cx.focus_handle().tab_stop(true))
                .to_vec(),
            reveal_from: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
            open: Some(DialogToken::take(cx)),
        };
        modal.read_general(window, cx);
        modal
    }

    /// Shows `section`.
    pub fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.section == Section::Connectors && section != Section::Connectors {
            super::tour::connectors_closed(cx);
        }
        self.section = section;
        cx.notify();
    }

    /// The snapshot the engine last gave, when it has given one.
    fn snapshot<'a>(&self, cx: &'a App) -> Option<&'a Snapshot> {
        self.facts.read(cx).snapshot()
    }

    fn nav(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let title = div()
            .px(theme::GAP)
            .pb(theme::GAP)
            .text_size(sizes::NAV_TITLE_TEXT)
            .text_color(palette.text_tertiary)
            .child(tr(Key("status.settings")));
        let rows = Section::ALL.map(|section| self.nav_row(section, palette, cx));
        div()
            .w(sizes::NAV_WIDTH)
            .h_full()
            .flex_none()
            .px(sizes::NAV_PADDING)
            .pt(sizes::NAV_PADDING_TOP)
            .flex()
            .flex_col()
            .gap(theme::TAB_GAP)
            .rounded_tl(sizes::NAV_RADIUS)
            .rounded_bl(sizes::NAV_RADIUS)
            .bg(palette.canvas)
            .child(title)
            .children(rows)
            .into_any_element()
    }

    fn nav_row(&self, section: Section, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let is_open = self.section == section;
        let selector = format!("settings-section-{}", section.id());
        let (hover, selected) = (palette.hover, palette.selected_row);
        let row = div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .h(sizes::NAV_ROW_HEIGHT)
            .px(theme::GAP)
            .flex()
            .items_center()
            .rounded(theme::RADIUS_CONTROL)
            .text_size(theme::TEXT_SMALL)
            .text_color(if is_open {
                palette.text
            } else {
                palette.text_secondary
            })
            .bg(if is_open {
                selected
            } else {
                transparent_black()
            })
            .hover(move |row| row.bg(if is_open { selected } else { hover }))
            .on_click(cx.listener(move |modal, _, _, cx| modal.show(section, cx)))
            .child(tr(section.title()));
        if section == Section::Connectors {
            super::tour::track(super::tour::Step::Connectors, theme::RADIUS_CONTROL, row)
        } else {
            row.into_any_element()
        }
    }

    fn header(&self, palette: &Palette) -> AnyElement {
        let close = super::dialog_frame::close_button("settings-close");
        div()
            .flex()
            .items_center()
            .child(
                div()
                    .text_size(sizes::TITLE_TEXT)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(palette.text)
                    .child(tr(self.section.title())),
            )
            .child(div().flex_1())
            .child(close)
            .into_any_element()
    }

    fn content(
        &mut self,
        palette: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        match self.section {
            Section::General => self.general(palette, window, cx),
            Section::Projects => self.projects(palette, cx),
            Section::Accounts => self.accounts(palette, cx),
            Section::Connectors => self.connectors(palette, cx),
            Section::Notifications => self.notifications(palette, cx),
            Section::Shortcuts => self.shortcuts(palette),
            Section::Updates => self.updates(palette, cx),
            Section::About => self.about(palette, cx),
        }
    }
}

impl SettingsModal {
    /// Scrolls the section to its end once the form just opened has made it longer.
    fn reveal_form(&mut self) {
        let Some(before) = self.reveal_from else {
            return;
        };
        let longest = self.scroll.max_offset().y;
        if longest != before {
            self.reveal_from = None;
            self.scroll.set_offset(gpui::point(gpui::px(0.), -longest));
        }
    }
}

impl Focusable for SettingsModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SettingsModal {
    fn dismiss(&mut self, cx: &mut App) {
        if self.section == Section::Connectors {
            super::tour::connectors_closed(cx);
        }
        self.open = None;
    }

    fn close(&mut self, window: &mut Window, cx: &mut App) {
        self.dismiss(cx);
        window.close_dialog(cx);
    }
}

impl Render for SettingsModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.reveal_form();
        let palette = Theme::palette(cx);
        let nav = self.nav(&palette, cx);
        let header = self.header(&palette);
        let groups = self.content(&palette, window, cx);
        let scrolling = div()
            .id("settings-scroll")
            .debug_selector(|| "settings-scroll".into())
            .track_scroll(&self.scroll)
            .border_t_1()
            .border_color(if self.scroll.offset().y < gpui::px(0.) {
                palette.border
            } else {
                transparent_black()
            })
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(sizes::GROUP_GAP)
            .pb(sizes::MAIN_PADDING_Y)
            .children(groups);
        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .px(sizes::MAIN_PADDING_X)
            .py(sizes::MAIN_PADDING_Y)
            .flex()
            .flex_col()
            .gap(sizes::GROUP_GAP)
            .child(header)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(scrolling)
                    .child(super::page_scroll::edge_fades(
                        &self.scroll,
                        palette.overlay_surface,
                    )),
            );
        crate::ui::dialog_frame::surface("settings-modal", window, &palette)
            .track_focus(&self.focus)
            .h(crate::ui::dialog_frame::available_height(window))
            .p_0()
            .gap_0()
            .flex_row()
            .overflow_hidden()
            .text_color(palette.text)
            .font_family(theme::FONT_UI)
            .text_size(theme::TEXT_SMALL)
            .child(nav)
            .child(main)
    }
}

#[cfg(test)]
mod tests;
