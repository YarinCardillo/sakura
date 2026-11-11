//! The right pane of a lavoro's page: its own row of views over the view chosen, as the panels of Cursor and
//! Zed have theirs. The views are text tabs at the pane's top, the chosen one a filled pill: "Files" (the
//! tree and the open file), "Changes", which counts the changed files, "Checks", which counts the checks
//! that failed on HEAD, and "Git" (the project's commits). Choosing a view changes a fill
//! and a colour, never a weight or a size, so nothing beside it moves. Which view a lavoro shows is kept
//! per lavoro by the desk; the one place where a view becomes its content is [`Desk::right_pane`].

use super::chrome::{tabular, text, text_width};
use super::desk::Desk;
use super::view_row::{self, ViewTab};
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette};
use gpui::{
    div, px, AnyElement, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement, Pixels, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled,
    Window,
};
use sakura_core::model;
use sakura_core::models::Task;

/// The views of the right pane, in the order of its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneView {
    /// The whole tree of the lavoro and the open file, which follows the agent.
    Files,
    /// The diffs of the lavoro.
    #[default]
    Changes,
    /// Its checks and their results.
    Checks,
    /// The commits of the lavoro's project: the base and every lavoro's branch.
    Git,
    /// The run command of the lavoro's project: where it stands and what it wrote.
    Play,
}

impl PaneView {
    /// All views in the order of the row.
    pub const ALL: [PaneView; 5] = [
        PaneView::Files,
        PaneView::Changes,
        PaneView::Checks,
        PaneView::Git,
        PaneView::Play,
    ];

    /// The name the view's elements are found by, in any language.
    pub fn id(self) -> &'static str {
        match self {
            PaneView::Files => "files",
            PaneView::Changes => "changes",
            PaneView::Checks => "checks",
            PaneView::Git => "git",
            PaneView::Play => "play",
        }
    }

    /// The view a snapshot names: by its id in any language, or by the Italian name its scripts used
    /// before the interface had two languages.
    pub fn named(name: &str) -> Option<PaneView> {
        match name {
            "files" => Some(PaneView::Files),
            "changes" | "modifiche" => Some(PaneView::Changes),
            "checks" | "verifiche" => Some(PaneView::Checks),
            "git" => Some(PaneView::Git),
            "play" => Some(PaneView::Play),
            _ => None,
        }
    }

    /// The label on the row.
    pub fn label(self) -> &'static str {
        match self {
            PaneView::Files => tr(Key("view.files")),
            PaneView::Changes => tr(Key("view.changes")),
            PaneView::Checks => tr(Key("view.checks")),
            PaneView::Git => tr(Key("view.git")),
            PaneView::Play => tr(Key("play.start")),
        }
    }
}

/// What the row of views shows of a lavoro.
pub struct PaneFacts<'a> {
    pub task: &'a Task,
    /// Only the lavoro's name is known: its facts are being read, so nothing is counted.
    pub is_pending: bool,
    pub view: PaneView,
    /// How many files "Changes" lists, once the whole patch of every change was read.
    pub listed_files: Option<usize>,
    /// Whether the lavoro has a Play: its view is in the row only then.
    pub has_play: bool,
    /// Where the row of views is scrolled to, in a pane too narrow for all of them.
    pub scroll: ScrollHandle,
}

/// The scroll of the row of views. A pane too narrow for every view scrolls its row, with no rail, and
/// the view chosen is brought into it whole when the choice or the lavoro changes; between two changes
/// the row stays where the person scrolled it.
#[derive(Default)]
pub struct ViewsScroll {
    handle: ScrollHandle,
    followed: Option<(String, PaneView)>,
}

impl ViewsScroll {
    /// The row's scroll for `task` showing `view`, brought to that view when it was not the one shown.
    pub fn following(&mut self, task: &str, view: PaneView, has_play: bool) -> ScrollHandle {
        let is_followed =
            matches!(&self.followed, Some((name, shown)) if name == task && *shown == view);
        if !is_followed {
            let nth = shown_views(has_play).position(|shown| shown == view);
            self.handle.scroll_to_item(nth.unwrap_or_default());
            self.followed = Some((task.to_string(), view));
        }
        self.handle.clone()
    }
}

/// The views of the row, in its order: Play only for a lavoro that has one.
fn shown_views(has_play: bool) -> impl Iterator<Item = PaneView> {
    let is_shown = move |view: &PaneView| has_play || *view != PaneView::Play;
    PaneView::ALL.into_iter().filter(is_shown)
}

/// The pane: the row of views over `body`, the content of the view chosen.
pub fn view(
    facts: &PaneFacts,
    body: AnyElement,
    palette: &Palette,
    window: &Window,
    cx: &Context<Desk>,
) -> AnyElement {
    div()
        .debug_selector(|| "right-pane".into())
        .size_full()
        .flex()
        .flex_col()
        .child(views(facts, palette, window, cx))
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

/// The row of views: each a tab, the first one starting on the pane's inset. Where the pane is narrow
/// the tabs first give up room at their sides; only a pane too narrow even so scrolls its row, with no
/// rail.
fn views(
    facts: &PaneFacts,
    palette: &Palette,
    window: &Window,
    cx: &Context<Desk>,
) -> impl IntoElement {
    let tabs = shown_views(facts.has_play).map(|view| tab(view, facts, palette, window, cx));
    // The tabs scroll between the row's insets, so the last one stops where the pane's content does.
    let strip = div()
        .id("pane-views-strip")
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .gap(theme::TAB_GAP)
        .overflow_x_scroll()
        .track_scroll(&facts.scroll)
        .children(tabs);
    view_row::row("pane-views", theme::SIDE_PANE_INSET, palette).child(strip)
}

/// One view: its label and what it counts.
fn tab(
    view: PaneView,
    facts: &PaneFacts,
    palette: &Palette,
    window: &Window,
    cx: &Context<Desk>,
) -> impl IntoElement {
    let counted = counted(view, facts, palette);
    let figures = counted.as_ref().map(|count| count.figures.as_str());
    let floor = narrowest(view.label(), figures, window);
    let tab = ViewTab {
        id: view.id(),
        selector: format!("pane-view-{}", view.id()),
        label: view.label(),
        is_current: view == facts.view,
        counted: counted.map(count),
    };
    let task = facts.task.name.clone();
    view_row::tight_tab(tab, floor, palette)
        .on_click(cx.listener(move |desk, _, _, cx| desk.show_pane_view(&task, view, cx)))
}

/// How narrow a tab may get: its label and its count as they are drawn, with the least room at each side.
fn narrowest(label: &str, figures: Option<&str>, window: &Window) -> Pixels {
    let label = text_width(window, label, theme::TEXT_SMALL, theme::WEIGHT_MEDIUM);
    let count = figures.map(|figures| {
        let drawn = text_width(window, figures, theme::TEXT_LABEL, FontWeight::NORMAL);
        f32::from(theme::GAP_SMALL) + drawn.max(f32::from(theme::TAB_COUNT_WIDTH))
    });
    px((label + count.unwrap_or_default()).ceil()) + theme::TAB_PADDING_MIN * 2.
}

/// What a view counts, as it is written, where and in which colour.
struct Counted {
    selector: &'static str,
    figures: String,
    colour: Hsla,
}

/// What a view counts beside its label: "Changes" the changed files when there are any, "Checks" the
/// checks that failed on HEAD, in the danger tone, when one did.
fn counted(view: PaneView, facts: &PaneFacts, palette: &Palette) -> Option<Counted> {
    if facts.is_pending {
        return None;
    }
    let (selector, number, colour) = match view {
        PaneView::Changes => {
            let files = model::changed_files(facts.task, facts.listed_files)?;
            ("pane-count-changes", files, palette.text_tertiary)
        }
        PaneView::Checks => {
            let failed = model::failed_checks(facts.task)?;
            ("pane-count-checks", failed, palette.danger)
        }
        PaneView::Files | PaneView::Git | PaneView::Play => return None,
    };
    let figures = crate::l10n::grouped(u64::from(number));
    Some(Counted {
        selector,
        figures,
        colour,
    })
}

/// A count on a view: tabular figures centred in the room of two, so a count going from 9 to 10 moves
/// nothing beside it.
fn count(counted: Counted) -> AnyElement {
    let selector = counted.selector;
    text(theme::TEXT_LABEL, counted.colour)
        .debug_selector(move || selector.into())
        .min_w(theme::TAB_COUNT_WIDTH)
        .flex_none()
        .flex()
        .justify_center()
        .font_features(tabular())
        .font_weight(FontWeight::NORMAL)
        .child(SharedString::from(counted.figures))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_names_a_view_by_its_id_whatever_the_language() {
        for view in PaneView::ALL {
            assert_eq!(PaneView::named(view.id()), Some(view));
        }
        let older = PaneView::named("verifiche");
        assert_eq!(older, Some(PaneView::Checks), "the scripts' older name");
        assert_eq!(PaneView::named("Checks"), None, "never by a label");
    }
}
