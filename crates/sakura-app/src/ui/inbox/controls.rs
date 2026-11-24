//! What the Inbox puts at the right end of the page's header row: the filter, and the menu that switches the
//! list between the asks that wait and the asks answered.

use super::model::{self, Filter};
use super::{InboxPage, Shown};
use crate::l10n::{tr, Key};
use crate::theme::kit::scale::{Typeset as _, CONTROL_LABEL, CONTROL_REGULAR, SPACE_M};
use crate::theme::{self, inbox as sizes, Palette, Theme};
use crate::ui::kit::{self, filter_button};
use crate::ui::kit::{ScrollMenu, ScrollMenuItem};
use crate::ui::page_bar::PageControls;
use gpui::{
    div, Anchor, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::popover::Popover;
use sakura_core::engine::snapshot::{Snapshot, Wait};
use std::collections::BTreeSet;

impl PageControls for InboxPage {
    fn controls(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = Theme::palette(cx);
        let snapshot = self.snapshot(cx);
        let (projects, kinds) = snapshot
            .as_ref()
            .map_or_else(Default::default, |s| self.choices(s));
        let page = cx.entity();
        let has_asks = snapshot
            .as_ref()
            .is_some_and(|snapshot| self.has_to_filter(snapshot));
        let filter = has_asks.then(|| {
            let current = self.filter.clone();
            let allows_snoozed = self.shown == Shown::Open;
            filter_button(
                "inbox-filter",
                tr(Key("inbox.filter")),
                Some(
                    move |menu: ScrollMenu, window: &mut Window, _: &mut gpui::App| {
                        let width = window.viewport_size().width - theme::PANEL_INSET * 2.;
                        let menu = menu.min_w(sizes::MENU_WIDTH.min(width));
                        filter_menu(menu, &page, &current, &projects, &kinds, allows_snoozed)
                    },
                ),
                &palette,
                cx,
            )
        });
        let views = [Shown::Open, Shown::Done];
        let options = views.map(|target| (view_word(target), self.shown == target));
        let page = cx.weak_entity();
        let tabs = kit::segments("inbox-view", &options, move |index, _, cx| {
            drop(page.update(cx, |page, cx| page.show(views[index], cx)));
        });
        div()
            .flex()
            .items_center()
            .gap(theme::GAP_TIGHT)
            .children(filter)
            .child(crate::ui::tour::track(
                crate::ui::tour::Step::Inbox,
                theme::RADIUS_CONTROL,
                tabs,
            ))
            .into_any_element()
    }
}

impl InboxPage {
    /// The projects and the kinds of what the list holds: what the filter can narrow to.
    fn choices(&self, snapshot: &Snapshot) -> (Vec<String>, Vec<String>) {
        let (mut projects, mut kinds) = (BTreeSet::new(), BTreeSet::new());
        match self.shown {
            Shown::Open => snapshot.waits.iter().for_each(|wait| {
                // A morning digest is no project's: its asks name none.
                if !wait.project.is_empty() {
                    projects.insert(wait.project.clone());
                }
                kinds.insert(model::wait_kind(wait).to_string());
            }),
            Shown::Done => snapshot.decided.iter().flatten().for_each(|decided| {
                projects.insert(decided.project.clone());
                kinds.insert(model::history_kind(decided).to_string());
            }),
        }
        (projects.into_iter().collect(), kinds.into_iter().collect())
    }

    /// Whether the list the page shows has anything a filter could narrow: with nothing listed, a filter
    /// has nothing to do.
    fn has_to_filter(&self, snapshot: &Snapshot) -> bool {
        match self.shown {
            Shown::Open => true,
            Shown::Done => snapshot
                .decided
                .as_ref()
                .is_some_and(|list| !list.is_empty()),
        }
    }

    /// Switches between the open asks and the answered ones.
    pub(super) fn show(&mut self, shown: Shown, cx: &mut Context<Self>) {
        if self.shown != shown || (shown == Shown::Open && self.filter.snoozed) {
            self.shown = shown;
            self.selected = None;
            self.note = None;
            self.snooze_open = false;
            self.filter = Filter::default();
            cx.notify();
        }
    }

    pub(super) fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        self.selected = None;
        cx.notify();
    }
}

impl InboxPage {
    pub(super) fn attention_buttons(
        &self,
        wait: &Wait,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut buttons = Vec::new();
        let (action, label) = if self.is_read(wait) {
            ("unread", "inbox.mark_unread")
        } else {
            ("read", "inbox.mark_read")
        };
        if offers(wait, action) {
            buttons.push(
                super::face::action(
                    "inbox-read",
                    tr(Key(label)).into(),
                    Some("U".into()),
                    super::face::Face::Quiet,
                    false,
                    palette,
                    cx,
                )
                .on_click(cx.listener(|page, _, _, cx| page.toggle_read(cx)))
                .into_any_element(),
            );
        }
        if offers(wait, "snooze") {
            buttons.push(self.snooze_button(palette, cx));
        }
        buttons
    }

    pub(super) fn show_snooze(&mut self, cx: &mut Context<Self>) {
        if self
            .snapshot(cx)
            .as_ref()
            .and_then(|snapshot| self.selected_ask(snapshot))
            .is_some_and(|wait| offers(wait, "snooze"))
        {
            self.set_snooze(!self.snooze_open, cx);
        }
    }

    fn set_snooze(&mut self, is_open: bool, cx: &mut Context<Self>) {
        if self.snooze_open != is_open {
            self.snooze_open = is_open;
            cx.notify();
        }
    }

    fn snooze_button(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let button = Button::new("inbox-snooze")
            .debug_selector(|| "inbox-snooze".into())
            .custom(crate::ui::chrome::quiet_variant(palette, cx))
            .h(CONTROL_REGULAR)
            .px(SPACE_M)
            .rounded(theme::RADIUS_CONTROL)
            .child(
                div()
                    .debug_selector(|| "inbox-snooze-label".into())
                    .text_style(CONTROL_LABEL)
                    .child(tr(Key("inbox.snooze"))),
            );
        let page = cx.weak_entity();
        let choosing = page.clone();
        Popover::new("inbox-snooze-popover")
            .anchor(Anchor::BottomRight)
            .open(self.snooze_open)
            .on_open_change(move |is_open, _, cx| {
                drop(page.update(cx, |page, cx| page.set_snooze(*is_open, cx)));
            })
            .trigger(button)
            .content(move |_, _, cx| {
                let palette = Theme::palette(cx);
                div()
                    .flex()
                    .flex_col()
                    .children(SNOOZE_CHOICES.map(|(value, key)| {
                        let page = choosing.clone();
                        Button::new(format!("inbox-snooze-{value}"))
                            .debug_selector(move || format!("inbox-snooze-{value}"))
                            .custom(crate::ui::chrome::quiet_variant(&palette, cx))
                            .label(tr(Key(key)))
                            .on_click(move |_, _, cx| {
                                drop(page.update(cx, |page, cx| {
                                    page.set_snooze(false, cx);
                                    page.attention("snooze", Some(value), cx);
                                }));
                            })
                    }))
            })
            .into_any_element()
    }
}

const SNOOZE_CHOICES: [(&str, &str); 3] = [
    ("1h", "inbox.snooze.hour"),
    ("tomorrow", "inbox.snooze.tomorrow"),
    ("nextweek", "inbox.snooze.next_week"),
];

fn offers(wait: &Wait, key: &str) -> bool {
    wait.actions.iter().any(|action| action.key == key)
}

fn view_word(shown: Shown) -> SharedString {
    let key = match shown {
        Shown::Open => Key("inbox.view.open"),
        Shown::Done => Key("inbox.view.done"),
    };
    SharedString::from(tr(key))
}

fn filter_menu(
    menu: ScrollMenu,
    page: &gpui::Entity<InboxPage>,
    current: &Filter,
    projects: &[String],
    kinds: &[String],
    allows_snoozed: bool,
) -> ScrollMenu {
    let pick = |label: SharedString, is_on: bool, next: Filter| {
        let page = page.clone();
        ScrollMenuItem::new(label)
            .checked(is_on)
            .on_click(move |_, _, cx| page.update(cx, |page, cx| page.set_filter(next.clone(), cx)))
    };
    let mut menu = menu.item(pick(
        SharedString::from(tr(Key("inbox.filter.clear"))),
        current.is_empty(),
        Filter::default(),
    ));
    if allows_snoozed {
        menu = menu.item(pick(
            tr(Key("inbox.filter.snoozed")).into(),
            current.snoozed,
            Filter {
                snoozed: !current.snoozed,
                ..Filter::default()
            },
        ));
    }
    if !projects.is_empty() {
        menu = menu.separator().label(tr(Key("inbox.filter.project")));
    }
    for project in projects {
        let next = Filter {
            project: Some(project.clone()),
            ..current.clone()
        };
        let is_on = current.project.as_ref() == Some(project);
        menu = menu.item(pick(SharedString::from(project.clone()), is_on, next));
    }
    if !kinds.is_empty() {
        menu = menu.separator().label(tr(Key("inbox.filter.kind")));
    }
    for kind in kinds {
        let next = Filter {
            kind: Some(kind.clone()),
            ..current.clone()
        };
        let is_on = current.kind.as_ref() == Some(kind);
        menu = menu.item(pick(model::inbox_kind_words(kind), is_on, next));
    }
    menu
}
