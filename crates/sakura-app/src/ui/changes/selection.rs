//! Text selection in the review list: a press on a code line anchors it, a drag extends it across lines, and
//! Cmd+C copies the code lines in it, each from where the selection covers it. Points are byte indices into a
//! line's shown text, so a selection is exact for wide characters too.

use std::ops::Range;

/// A place in the list: a row and a byte index into that row's shown text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPoint {
    pub row: usize,
    pub index: usize,
}

/// The selection of one list: empty, or from an anchor to a head in either order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiffSelection {
    anchor: Option<TextPoint>,
    head: Option<TextPoint>,
    is_dragging: bool,
}

impl DiffSelection {
    /// A press at `at`: the selection starts there, empty.
    pub fn start(&mut self, at: TextPoint) {
        self.anchor = Some(at);
        self.head = Some(at);
        self.is_dragging = true;
    }

    /// A drag to `to`, while the press lasts. Returns whether the selection changed.
    pub fn extend(&mut self, to: TextPoint) -> bool {
        if !self.is_dragging || self.head == Some(to) {
            return false;
        }
        self.head = Some(to);
        true
    }

    /// The press ended.
    pub fn end(&mut self) {
        self.is_dragging = false;
    }

    /// Nothing selected any more. Returns whether something was.
    pub fn clear(&mut self) -> bool {
        let had = self.bounds().is_some();
        *self = Self::default();
        had
    }

    /// Whether a press is being dragged.
    pub fn is_dragging(&self) -> bool {
        self.is_dragging
    }

    /// The selected part of `row`, whose shown text is `len` bytes long, and whether the selection goes on
    /// past its end (onto the next line).
    pub fn range_in(&self, row: usize, len: usize) -> Option<(Range<usize>, bool)> {
        let (start, end) = self.bounds()?;
        if row < start.row || row > end.row {
            return None;
        }
        let from = if row == start.row {
            start.index.min(len)
        } else {
            0
        };
        let to = if row == end.row {
            end.index.min(len)
        } else {
            len
        };
        Some((from..to, row < end.row))
    }

    /// The selected text: every selected row that has text (`text_of` answers `None` for rows that are not
    /// code), each from where the selection covers it, joined by line breaks.
    pub fn text(&self, text_of: impl Fn(usize) -> Option<String>) -> Option<String> {
        let (start, end) = self.bounds()?;
        let parts: Vec<String> = (start.row..=end.row)
            .filter_map(|row| {
                let text = text_of(row)?;
                let (range, _) = self.range_in(row, text.len())?;
                text.get(range).map(str::to_string)
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    /// Start and end in list order, when the selection is not empty.
    fn bounds(&self) -> Option<(TextPoint, TextPoint)> {
        let (anchor, head) = (self.anchor?, self.head?);
        (anchor != head).then(|| (anchor.min(head), anchor.max(head)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(row: usize, index: usize) -> TextPoint {
        TextPoint { row, index }
    }

    fn lines(row: usize) -> Option<String> {
        match row {
            0 => Some("first line".into()),
            1 => None,
            2 => Some("third".into()),
            3 => Some("fourth".into()),
            _ => None,
        }
    }

    #[test]
    fn a_press_alone_selects_nothing() {
        let mut selection = DiffSelection::default();
        selection.start(at(0, 3));
        assert_eq!(selection.range_in(0, 10), None);
        assert_eq!(selection.text(lines), None);
    }

    #[test]
    fn a_drag_within_a_line_selects_that_part() {
        let mut selection = DiffSelection::default();
        selection.start(at(0, 6));
        assert!(selection.extend(at(0, 10)));
        assert_eq!(selection.range_in(0, 10), Some((6..10, false)));
        assert_eq!(selection.text(lines).as_deref(), Some("line"));
    }

    #[test]
    fn a_backward_drag_across_lines_copies_code_lines_only() {
        let mut selection = DiffSelection::default();
        selection.start(at(3, 3));
        selection.extend(at(0, 6));
        assert_eq!(selection.range_in(0, 10), Some((6..10, true)));
        assert_eq!(selection.range_in(2, 5), Some((0..5, true)));
        assert_eq!(selection.range_in(3, 6), Some((0..3, false)));
        assert_eq!(selection.range_in(4, 6), None);
        assert_eq!(selection.text(lines).as_deref(), Some("line\nthird\nfou"));
    }

    #[test]
    fn a_drag_after_the_press_ended_changes_nothing() {
        let mut selection = DiffSelection::default();
        selection.start(at(0, 0));
        selection.extend(at(0, 5));
        selection.end();
        assert!(!selection.extend(at(2, 1)));
        assert_eq!(selection.text(lines).as_deref(), Some("first"));
        assert!(selection.clear());
        assert!(!selection.clear());
    }
}
