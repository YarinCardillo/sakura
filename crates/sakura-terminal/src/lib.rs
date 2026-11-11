//! Sakura desk terminal: a terminal emulator view for GPUI.
//!
//! An `alacritty_terminal` grid drawn by a GPUI element, fed by a [`TerminalSource`]: a `sakura-core` pty session
//! through [`CoreSession`], or a local pty through [`LocalPty`].

mod blocks;
mod core;
mod core_session;
mod grid_view;
mod ime;
mod keys;
mod links;
mod local_pty;
mod mouse;
mod overlay;
mod palette;
mod pointer;
mod rows;
mod scroll;
mod size;
mod source;
mod stats;
mod style;
mod terminal;
mod view;

#[cfg(test)]
mod core_tests;
#[cfg(all(test, feature = "test-support"))]
mod view_tests;

pub use core_session::{CoreRouter, CoreSession};
pub use local_pty::LocalPty;
pub use palette::TerminalPalette;
pub use size::GridSize;
pub use source::{OutputSink, SourceError, SourceOutput, TerminalSource};
pub use stats::RenderStats;
pub use style::{
    FocusRing, TerminalStyle, DEFAULT_FONT_SIZE, DEFAULT_PADDING, STANDARD_LINE_HEIGHT,
};
pub use terminal::{Terminal, TerminalChange};
pub use view::TerminalView;
