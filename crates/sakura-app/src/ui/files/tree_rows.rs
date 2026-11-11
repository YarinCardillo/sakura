//! The rows of the file tree: the tree as far as it was read, handed to the kit's tree as its items, and
//! how one row looks. A row is one pitch of the grid: its indent (one icon a level), the chevron of a
//! folder, the name, and at its end the added and removed lines of a file that differs from the base.
//! What differs from the base is in full ink, the rest is quieter; the open file has the selected fill.

use crate::l10n::{signed_lines, tr, Key};
use crate::theme::{self, Palette};
use crate::ui::chrome::tabular;
use gpui::{
    div, prelude::FluentBuilder as _, Div, Hsla, InteractiveElement as _, ParentElement as _,
    SharedString, Styled as _,
};
use gpui_base::TreeItem;
use gpui_component::{Icon, Sizable as _};
use sakura_core::files::{Children, Entry, EntryKind, Node};
use std::collections::HashMap;

/// What marks the id of a row that is not an entry: no path `box` gives holds it.
const NOT_AN_ENTRY: char = '\0';

/// What a row of the tree shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Entry(Entry),
    /// Stands under a folder whose level is being read.
    Reading,
    /// Stands under a folder whose level could not be read.
    Unreadable,
    /// Stands at the end of a level `box` cut short.
    Truncated,
}

/// The rows by the id of their item: a path for an entry.
pub type Rows = HashMap<SharedString, RowKind>;

/// The kit's items for `nodes`, the root's entries, with what each row shows.
pub fn items(nodes: &[Node], is_truncated: bool) -> (Vec<TreeItem>, Rows) {
    let mut rows = Rows::new();
    let mut items: Vec<TreeItem> = nodes.iter().map(|node| item(node, &mut rows)).collect();
    if is_truncated {
        items.push(placeholder("", RowKind::Truncated, &mut rows));
    }
    (items, rows)
}

fn item(node: &Node, rows: &mut Rows) -> TreeItem {
    let id = SharedString::from(node.path.clone());
    rows.insert(id.clone(), RowKind::Entry(node.entry.clone()));
    let is_inert = matches!(node.entry.kind, EntryKind::Deleted | EntryKind::Other);
    let item = TreeItem::new(id, node.entry.name.clone())
        .expanded(node.is_open)
        .disabled(is_inert);
    match &node.children {
        Children::None => item,
        // The kit's tree takes for a folder what has something under it: an unread folder holds the
        // row that says it is being read, which is also what shows while it is.
        Children::Unread => item.child(placeholder(&node.path, RowKind::Reading, rows)),
        Children::Failed => item.child(placeholder(&node.path, RowKind::Unreadable, rows)),
        Children::Read { nodes, truncated } => {
            let mut children: Vec<TreeItem> =
                nodes.iter().map(|node| self::item(node, rows)).collect();
            if *truncated {
                children.push(placeholder(&node.path, RowKind::Truncated, rows));
            }
            item.children(children)
        }
    }
}

/// A row under `dir` that is not an entry: it cannot be picked.
fn placeholder(dir: &str, kind: RowKind, rows: &mut Rows) -> TreeItem {
    let id = SharedString::from(format!("{NOT_AN_ENTRY}{kind:?}:{dir}"));
    rows.insert(id.clone(), kind);
    TreeItem::new(id, SharedString::default()).disabled(true)
}

/// How a row stands in the tree at the moment it is drawn.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub depth: usize,
    pub is_expanded: bool,
    /// The row the keyboard is on.
    pub is_selected: bool,
    /// The row of the file open under the tree.
    pub is_open_file: bool,
}

/// One row of the tree.
pub fn row(kind: &RowKind, look: Look, palette: &Palette) -> Div {
    let base = div()
        .h(theme::ROW_HEIGHT)
        .flex()
        .items_center()
        .gap(theme::ROW_GAP)
        .pl(theme::ROW_PADDING_X + theme::ICON_SIZE * look.depth as f32)
        .pr(theme::ROW_PADDING_X)
        .rounded(theme::RADIUS_CONTROL)
        .text_size(theme::TEXT_BODY);
    match kind {
        RowKind::Entry(entry) => entry_row(base, entry, look, palette),
        RowKind::Reading => note(base, tr(Key("files.tree.reading")), palette),
        RowKind::Unreadable => note(base, tr(Key("files.tree.folder_unreadable")), palette),
        RowKind::Truncated => note(base, tr(Key("files.tree.truncated")), palette),
    }
}

fn entry_row(base: Div, entry: &Entry, look: Look, palette: &Palette) -> Div {
    let is_dir = entry.kind == EntryKind::Dir;
    let is_gone = entry.kind == EntryKind::Deleted;
    let ink = match (look.is_open_file || entry.changed, is_gone) {
        (_, true) => palette.text_tertiary,
        (true, false) => palette.text,
        (false, false) => palette.text_secondary,
    };
    let mark = div()
        .size(theme::ICON_SIZE)
        .flex_none()
        .when(is_dir, |mark| {
            mark.child(chevron(look.is_expanded, palette.text_tertiary))
        });
    let name = div()
        .flex_1()
        .min_w_0()
        .truncate()
        .when(is_gone, |name| name.line_through())
        .child(sakura_core::display::label(&entry.name));
    fill(base, look, palette)
        .text_color(ink)
        .child(mark)
        .child(name)
        .children(figures(entry, palette))
}

/// The fill of a row: the open file's is the selected one, the row the keyboard is on has the hover's,
/// and any other row takes the hover's under the pointer.
fn fill(row: Div, look: Look, palette: &Palette) -> Div {
    let hover = palette.hover;
    match (look.is_open_file, look.is_selected) {
        (true, _) => row.bg(palette.selected_row),
        (false, true) => row.bg(hover),
        (false, false) => row.hover(move |style| style.bg(hover)),
    }
}

/// The added and removed lines of a file that differs from the base; a part that is zero is left out.
fn figures(entry: &Entry, palette: &Palette) -> Option<Div> {
    let (added, removed) = (entry.added.unwrap_or(0), entry.removed.unwrap_or(0));
    // Told apart from nothing without adding them up: the two come from `box`.
    if entry.kind == EntryKind::Dir || (added == 0 && removed == 0) {
        return None;
    }
    let (added_text, removed_text) = signed_lines(added, removed);
    let figures = div()
        .flex_none()
        .flex()
        .items_center()
        .gap(theme::GAP_SMALL)
        .text_size(theme::TEXT_LABEL)
        .font_features(tabular())
        .when(added > 0, |row| {
            row.child(div().text_color(palette.success).child(added_text))
        })
        .when(removed > 0, |row| {
            row.child(div().text_color(palette.danger).child(removed_text))
        });
    Some(figures)
}

/// The chevron of a folder: pointing right when closed, down when open. The same icons, at the same
/// size, as every row that opens (`ui::glyph::chevron`, which the desk's rows use, draws these two).
fn chevron(is_open: bool, colour: Hsla) -> Icon {
    let name = match is_open {
        true => crate::icons::CHEVRON_OPEN,
        false => crate::icons::CHEVRON_CLOSED,
    };
    Icon::new(name)
        .with_size(theme::MARK_SIZE)
        .text_color(colour)
}

/// A quiet row that says something of its level, where a name would stand.
fn note(base: Div, sentence: &'static str, palette: &Palette) -> Div {
    let room = div().size(theme::ICON_SIZE).flex_none();
    let said = div().flex_1().min_w_0().truncate().child(sentence);
    base.text_color(palette.text_tertiary)
        .child(room)
        .child(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_largest_counts_a_reply_can_hold_are_shown_and_never_added_up() {
        let entry = |added, removed| Entry {
            name: "big.ts".into(),
            kind: EntryKind::File,
            changed: true,
            added: Some(added),
            removed: Some(removed),
        };
        let palette = Palette::dark();
        assert!(figures(&entry(u32::MAX, 1), &palette).is_some());
        assert!(figures(&entry(u32::MAX, u32::MAX), &palette).is_some());
        assert!(figures(&entry(0, 0), &palette).is_none(), "nothing to show");
    }
}
