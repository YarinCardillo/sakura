//! A command of the palette and a labelled group of them, whatever the build's commands are.

/// One command: its words, what choosing it does, whether it is the choice in force among
/// alternatives (the view shown, the language), and the mark of a lane's state when it goes to one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry<R> {
    pub label: String,
    pub run: R,
    pub is_current: bool,
    pub glyph: Option<crate::ui::kit::StateGlyph>,
}

/// A labelled group of commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group<R> {
    pub label: String,
    pub entries: Vec<Entry<R>>,
}
