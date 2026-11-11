//! The toolbar of "Modifiche" in the right pane, as its own view: the commit picker ("All changes" or one commit ahead).
//! It is drawn again when the task's reads or the appearance change, never while the diffs below scroll.

use super::header;
use super::line::display_text;
use crate::data::changes::{Changes, Scope};
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette, Theme};
use crate::ui::chrome::mono;
use crate::ui::task_menu::quiet_variant;
use gpui::{
    div, AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement, Render,
    Styled, Subscription, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use sakura_core::model::SHORT_SHA;
use sakura_core::models::Commit;
use std::sync::Arc;

/// The toolbar of one task's Modifiche view.
pub struct ReviewToolbar {
    task: String,
    changes: Entity<Changes>,
    _subscriptions: [Subscription; 2],
}

impl ReviewToolbar {
    /// The toolbar of `task`, over the shared reads.
    pub fn new(task: String, changes: Entity<Changes>, cx: &mut Context<Self>) -> Self {
        let subscriptions = [
            cx.observe(&changes, |_, _, cx| cx.notify()),
            cx.observe_global::<Theme>(|_, cx| cx.notify()),
        ];
        Self {
            task,
            changes,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for ReviewToolbar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = &Theme::palette(cx);
        let read = self.changes.read(cx).task(&self.task);
        let scope = read.map(|read| read.scope().clone()).unwrap_or_default();
        let commits = read
            .and_then(|read| read.commits().cloned())
            .unwrap_or_default();
        div()
            .flex_none()
            .size_full()
            .pl(theme::SIDE_PANE_INSET)
            .pr(theme::CONTENT_PADDING)
            .flex()
            .items_center()
            .border_b_1()
            .border_color(palette.border)
            .child(self.picker(scope, commits, palette, cx))
    }
}

impl ReviewToolbar {
    /// The commit picker; with no commit ahead there is nothing to pick, and it is not shown.
    fn picker(
        &self,
        scope: Scope,
        commits: Arc<Vec<Commit>>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if commits.is_empty() {
            return div().into_any_element();
        }
        let words = div()
            .debug_selector(|| "commit-picker-label".into())
            .min_w_0()
            .child(picker_label(&scope, &commits, palette));
        let trigger = div()
            .flex()
            .items_center()
            .gap(theme::GAP_TIGHT)
            .min_w_0()
            .text_size(theme::TEXT_SMALL)
            .child(words)
            .child(header::chevron_mark(true, palette.text_tertiary));
        let (changes, task) = (self.changes.clone(), self.task.clone());
        let button = Button::new("commit-picker")
            .debug_selector(|| "commit-picker".into())
            .custom(quiet_variant(palette, cx))
            .h(theme::CONTROL_HEIGHT)
            .px(theme::CONTROL_PADDING_X)
            .child(trigger)
            .dropdown_menu(move |menu, _, _| menu_items(menu, &scope, &commits, &changes, &task));
        // A quiet control: its words sit on the content inset, its fill under the pointer reaches one
        // control padding past them.
        div()
            .min_w_0()
            .ml(-theme::CONTROL_PADDING_X)
            .child(button)
            .into_any_element()
    }
}

/// What the choice of every change reads, on the trigger and in the menu.
const ALL_CHANGES: Key = Key("changes.picker.all");

/// "All changes", or the chosen commit as its short sha in mono and its subject.
fn picker_label(scope: &Scope, commits: &[Commit], palette: &Palette) -> AnyElement {
    let commit = match scope {
        Scope::All => None,
        Scope::Commit(sha) => commits.iter().find(|commit| commit.sha == *sha),
    };
    match commit {
        Some(commit) => commit_label(commit, palette).into_any_element(),
        None => div().child(tr(ALL_CHANGES)).into_any_element(),
    }
}

fn commit_label(commit: &Commit, palette: &Palette) -> impl IntoElement {
    let short: String = commit.sha.chars().take(SHORT_SHA).collect();
    div()
        .flex()
        .items_center()
        .gap(theme::GAP)
        .min_w_0()
        .child(
            mono()
                .flex_none()
                .text_color(palette.text_tertiary)
                .child(short),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(display_text(&commit.subject)),
        )
}

fn menu_items(
    menu: PopupMenu,
    scope: &Scope,
    commits: &Arc<Vec<Commit>>,
    changes: &Entity<Changes>,
    task: &str,
) -> PopupMenu {
    // Every choice is drawn by this view, the first as the commits are: one kind of item, so the kit
    // keeps the check's column for each and every label starts on one line.
    let all = PopupMenuItem::element(|_, _| choice_label("all", div().child(tr(ALL_CHANGES))));
    let all = choice(all, Scope::All, scope, changes, task);
    let menu = menu
        .min_w(theme::PICKER_MENU_WIDTH)
        .max_w(theme::PICKER_MENU_WIDTH);
    // The menu scrolls only when its choices do not fit. A menu that scrolls is scrolled by the kit to its
    // checked choice as it opens, and one whose choices all fit was moved a few points by that: its first
    // choice cut at the top, and room left under the last.
    let is_long = commits.len() >= theme::PICKER_MENU_CHOICES;
    let menu = match is_long {
        true => menu.max_h(theme::PICKER_MENU_HEIGHT).scrollable(true),
        false => menu,
    };
    let menu = menu.item(all);
    commits.iter().fold(menu, |menu, commit| {
        let shown = commit.clone();
        let item = PopupMenuItem::element(move |_, cx| {
            let short: String = shown.sha.chars().take(SHORT_SHA).collect();
            choice_label(&short, commit_label(&shown, &Theme::palette(cx)))
        });
        let target = Scope::Commit(commit.sha.clone());
        menu.item(choice(item, target, scope, changes, task))
    })
}

/// A choice's label in the menu, named after what it chooses.
fn choice_label(name: &str, label: impl IntoElement) -> AnyElement {
    let selector = format!("commit-choice-{name}");
    div()
        .debug_selector(move || selector)
        .min_w_0()
        .child(label)
        .into_any_element()
}

/// A menu item that shows `target`, checked when it is the current scope.
fn choice(
    item: PopupMenuItem,
    target: Scope,
    current: &Scope,
    changes: &Entity<Changes>,
    task: &str,
) -> PopupMenuItem {
    let (changes, task) = (changes.clone(), task.to_string());
    item.checked(target == *current).on_click(move |_, _, cx| {
        changes.update(cx, |changes, cx| {
            changes.set_scope(&task, target.clone(), cx)
        });
    })
}
