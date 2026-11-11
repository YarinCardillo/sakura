//! How the pane is laid out: one toolbar row with the "Only touched" switch (the kit's), the tree, and under a
//! hairline the open file. With no file open the tree has the whole pane; with one, the tree keeps the
//! share of the height the pane itself takes of the window's width at first launch. Rows start on the
//! side pane's inset, as the rows of "Changes" do, and their fill reaches past it by the row's padding.

use super::{FilesPane, RowKind, Rows};
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette, Theme};
use crate::ui::files::tree_rows::{self, Look};
use gpui::{
    div, prelude::FluentBuilder as _, relative, AnyElement, App, Context, Div,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, Stateful,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, WeakEntity, Window,
};
use gpui_base::{TreeEntry, TreeEntryState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::switch::Switch;
use gpui_component::{FocusableExt as _, Sizable as _};
use std::rc::Rc;

/// The key that opens the file the keyboard is on.
const OPEN_KEY: &str = "enter";

/// A two-state control of a bar, filled while it is on: the look of the right pane's own row of views.
fn bar_toggle(
    id: &'static str,
    label: &'static str,
    is_on: bool,
    palette: &Palette,
) -> Stateful<Div> {
    let (fill, ink) = (palette.fill, palette.text);
    div()
        .id(id)
        .debug_selector(move || id.into())
        .h(theme::STATUS_CONTROL_HEIGHT)
        .px(theme::CONTROL_PADDING_X)
        .flex_none()
        .flex()
        .items_center()
        .rounded(theme::RADIUS_CONTROL)
        .text_size(theme::TEXT_SMALL)
        .font_weight(theme::WEIGHT_MEDIUM)
        .whitespace_nowrap()
        .when(is_on, |toggle| toggle.bg(fill).text_color(ink))
        .when(!is_on, |toggle| {
            toggle
                .text_color(palette.text_secondary)
                .hover(move |style| style.text_color(ink))
        })
        .child(label)
}

/// One quiet sentence where the tree would stand, on the line its rows start on.
fn sentence(words: &'static str, palette: &Palette) -> Div {
    div()
        .px(theme::SIDE_PANE_INSET)
        .h(theme::ROW_HEIGHT)
        .flex()
        .items_center()
        .gap(theme::GAP)
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_tertiary)
        .child(words)
}

impl FilesPane {
    /// The row over the tree: the kit's switch for "Only touched", its track on the pane's inset.
    /// It shows no ring after a press, and its track when on is the window's one action colour.
    fn toolbar(&self, palette: &Palette, cx: &mut Context<Self>) -> Div {
        let switch = Switch::new("files-only-touched-switch")
            .checked(self.tree.is_only_touched())
            .label(tr(Key("files.only_touched")))
            .small()
            .focus_ring(false)
            .color(palette.primary)
            .text_size(theme::TEXT_SMALL)
            .text_color(palette.text_secondary)
            .on_click(cx.listener(|pane, is_on: &bool, _, cx| pane.set_only_touched(*is_on, cx)));
        let placed = div()
            .debug_selector(|| "files-only-touched".into())
            .flex_none()
            .child(switch);
        div()
            .h(theme::TOOLBAR_HEIGHT)
            .flex_none()
            .px(theme::SIDE_PANE_INSET)
            .flex()
            .items_center()
            .child(placed)
    }

    /// What stands in the tree's place when it has no rows: why, in one sentence.
    fn empty(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.tree.has_failed("") {
            let retry = bar_toggle("files-retry", tr(Key("common.retry")), true, palette)
                .on_click(cx.listener(|pane, _, _, cx| pane.retry(cx)));
            let failed = sentence(tr(Key("files.tree.unreadable")), palette).child(retry);
            return Some(failed.into_any_element());
        }
        let words = match (self.tree.is_read(), self.tree.is_only_touched()) {
            (false, _) => Key("files.tree.reading"),
            (true, _) if !self.rows.is_empty() => return None,
            (true, true) => Key("files.tree.none_touched"),
            (true, false) => Key("files.tree.empty"),
        };
        Some(sentence(tr(words), palette).into_any_element())
    }

    /// The kit's tree over the rows, with its scrollbar; Enter opens the file the keyboard is on.
    fn tree_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let drawing = RowDrawing {
            rows: self.rows.clone(),
            open: self.open_path(cx).map(str::to_string),
            pane: cx.weak_entity(),
        };
        let row = move |_: usize,
                        entry: &TreeEntry,
                        state: TreeEntryState,
                        _: &mut Window,
                        cx: &mut App| { drawing.row(entry, state, cx) };
        let scroll = self.tree_state.read(cx).scroll_handle().clone();
        div()
            .id("files-tree")
            .size_full()
            .px(theme::SIDE_PANE_INSET - theme::ROW_PADDING_X)
            .on_key_down(cx.listener(Self::on_key))
            .child(
                gpui_base::Tree::new(&self.tree_state)
                    .item(row)
                    .list_style(StyleRefinement::default().size_full())
                    .size_full(),
            )
            .vertical_scrollbar(&scroll)
            .into_any_element()
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != OPEN_KEY {
            return;
        }
        if let Some(path) = self.selected_file(cx) {
            self.open(&path, window, cx);
        }
    }
}

/// What drawing a row of the tree needs of the pane.
struct RowDrawing {
    rows: Rc<Rows>,
    /// The path of the open file.
    open: Option<String>,
    pane: WeakEntity<FilesPane>,
}

impl RowDrawing {
    /// The row of `entry` as the kit's tree asks for it: a press on a file opens it.
    fn row(&self, entry: &TreeEntry, state: TreeEntryState, cx: &App) -> AnyElement {
        let id = entry.item().id.clone();
        let Some(kind) = self.rows.get(&id) else {
            return div().into_any_element();
        };
        let look = Look {
            depth: entry.depth(),
            is_expanded: entry.is_expanded(),
            is_selected: state.is_selected(),
            is_open_file: self.open.as_deref() == Some(id.as_ref()),
        };
        let (pane, path, selector) = (self.pane.clone(), id.clone(), format!("files-row-{id}"));
        tree_rows::row(kind, look, &Theme::palette(cx))
            .id(id)
            .debug_selector(move || selector)
            .when(is_file(kind), |row| {
                row.on_click(move |_, window, cx| {
                    drop(pane.update(cx, |pane, cx| pane.open(&path, window, cx)));
                })
            })
            .into_any_element()
    }
}

/// Whether a row is a file a press opens.
pub(super) fn is_file(kind: &RowKind) -> bool {
    use sakura_core::files::EntryKind;
    matches!(kind, RowKind::Entry(entry) if matches!(entry.kind, EntryKind::File | EntryKind::Link))
}

impl Render for FilesPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let has_file = self.open_path(cx).is_some();
        let tree = match self.empty(&palette, cx) {
            Some(empty) => empty,
            None => self.tree_view(cx),
        };
        let tree = div()
            .debug_selector(|| "files-tree-area".into())
            .min_h_0()
            .when(has_file, |tree| {
                tree.flex_none().h(relative(theme::CHANGES_SHARE_DEFAULT))
            })
            .when(!has_file, |tree| tree.flex_1())
            .child(tree);
        let file = has_file.then(|| {
            div()
                .debug_selector(|| "files-file-area".into())
                .flex_1()
                .min_h_0()
                .border_t(theme::PANEL_RING_WIDTH)
                .border_color(palette.border)
                .child(self.file.clone())
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.toolbar(&palette, cx))
            .child(tree)
            .children(file)
    }
}
