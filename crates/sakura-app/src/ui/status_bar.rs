//! The status bar at the window's foot, a real bar in the manner of Zed's: a group of items at its left
//! end, a group at its right end, and between them, quiet, what is running. It holds only what Sakura has:
//! at the left the sidebar's toggle; in the middle a sentence that says a thing only while it is true (a
//! read that failed, the check the runner runs and how many wait, the machine while it is strained); at
//! the right the lavoro's terminal and the right pane's toggle. An item enters the bar when its function
//! exists, in [`left_items`] or [`right_items`], and nowhere else. At rest the middle is empty.
//!
//! Each pane's toggle shows whether its pane is open (the icon filled on the pane's side) or closed (the
//! outline); the terminal's item has the fill of a chosen view while the terminal is what the centre
//! shows. One sentence, words and commas; a part takes the warning tone only when it reports strain, a
//! read error the danger tone; when the bar is too short the sentence gives way with an ellipsis and
//! never runs under the items at its right.
//!
//! On the layout grid (`theme.rs`, "Layout grid"): one row high, on the canvas, in the place of the panel's
//! bottom inset; its items are the small control (22, as in Zed's status bar, whose items stand 4 apart),
//! so their fill keeps clear of the window's edge; the sidebar toggle's icon on the sidebar's inset, the
//! pane toggle's icon ending on the window's right content inset, the sentence starting where the
//! sidebar's names start.

use super::chrome::{tabular, DETAILS};
use super::pane_toggle;
use crate::data::facts::Facts;
use crate::icons::PaneIcon;
use crate::l10n::{say, tr, Key, Said};
use crate::menus::{self, PanesShown, SwitchTerminal, ToggleChanges, ToggleFollow, ToggleSidebar};
use crate::theme::{self, Palette};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, App, HighlightStyle, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, StyledText,
};
use gpui_component::tooltip::Tooltip;
use sakura_core::model;
use sakura_core::models::{Machine, RunnerState};
use std::ops::Range;

/// The tone of a part of the sentence that is not plain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// The machine is strained: load above the cores, or critical memory pressure.
    Strained,
    /// A read of `box` failed.
    Failed,
}

/// The sentence of the status bar and the parts of it that are not in the quiet tone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    pub text: String,
    pub toned: Vec<(Range<usize>, Tone)>,
}

impl Line {
    /// Adds `part` after a comma (its first letter lowered, as after a comma), in `tone` when it has one.
    fn push(&mut self, part: &str, tone: Option<Tone>) {
        let part = match self.text.is_empty() {
            true => part.to_string(),
            false => lowercase_first(part),
        };
        if !self.text.is_empty() {
            self.text.push_str(", ");
        }
        let start = self.text.len();
        self.text.push_str(&part);
        if let Some(tone) = tone {
            self.toned.push((start..self.text.len(), tone));
        }
    }
}

/// The sentence for what is true now: a read error first, then what the runner does, then the machine
/// while it is strained. Nothing running and nothing strained is an empty sentence.
pub fn line(machine: Option<&Machine>, runner: Option<&RunnerState>, error: Option<&Said>) -> Line {
    let mut line = Line::default();
    if let Some(error) = error {
        line.push(&say(error), Some(Tone::Failed));
    }
    if let Some(runner) = runner.filter(|runner| is_worth_saying(runner)) {
        line.push(&say(&model::runner_status(runner)), None);
        if let Some(waiting) = model::runner_waiting(runner) {
            line.push(&say(&waiting), None);
        }
    }
    if let Some(machine) = machine {
        if model::is_load_high(machine) {
            line.push(&say(&model::load(machine)), Some(Tone::Strained));
        }
        if model::is_memory_critical(machine) {
            line.push(tr(model::memory(machine)), Some(Tone::Strained));
        }
    }
    line
}

/// Whether the runner has something to say: a check under way, or checks that wait (also when the
/// runner is stopped, which is then why they wait). An idle runner says nothing.
fn is_worth_saying(runner: &RunnerState) -> bool {
    runner.current.is_some() || !runner.queue.is_empty()
}

/// Where the centre of the lavoro on screen stands with its terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalItem {
    /// The lavoro has no terminal: a press opens one.
    None,
    /// It has one, and the centre shows the agent: a press shows the terminal.
    Open,
    /// The terminal is what the centre shows: a press goes back to the agent.
    Shown,
}

impl TerminalItem {
    /// What a press would do.
    fn label(self) -> Key {
        match self {
            TerminalItem::None => Key("task_menu.open_terminal"),
            TerminalItem::Open => Key("status.terminal.show"),
            TerminalItem::Shown => Key("status.terminal.back"),
        }
    }
}

/// What the bar shows besides the facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bar {
    pub panes: PanesShown,
    /// The terminal of the lavoro on screen; none while no lavoro is shown.
    pub terminal: Option<TerminalItem>,
    /// Whether the right pane of the lavoro on screen follows its agent; none while no lavoro is shown
    /// or its files cannot be read.
    pub follow: Option<bool>,
    /// Whether the Play of the lavoro on screen runs.
    pub is_playing: bool,
}

/// The status bar for the current facts, panes and lavoro.
pub fn view(facts: &Facts, bar: Bar, palette: &Palette, cx: &App) -> AnyElement {
    let error = facts.error();
    let line = line(facts.machine(), facts.runner(), error.as_ref());
    let right = div()
        .flex_none()
        .flex()
        .items_center()
        .gap(theme::GAP_TIGHT)
        .children(right_items(bar, palette, cx));
    div()
        .debug_selector(|| "status-bar".into())
        .h(theme::STATUS_BAR_HEIGHT)
        .w_full()
        .flex_none()
        .pl(theme::SIDEBAR_INSET - theme::STATUS_CONTROL_BLEED)
        .pr(theme::CONTENT_INSET - theme::STATUS_CONTROL_BLEED)
        .flex()
        .items_center()
        .children(left_items(bar, cx))
        .child(sentence(line, facts.error_details(), palette))
        .child(right)
        .into_any_element()
}

/// The items at the bar's left end, in order. A new item of the left end is added here.
fn left_items(bar: Bar, cx: &App) -> Vec<AnyElement> {
    vec![pane_toggle::button(
        ("toggle-sidebar", "toggle-sidebar-icon"),
        (icon(true, bar.panes.sidebar), theme::STATUS_CONTROL_HEIGHT),
        tr(menus::sidebar_label(bar.panes.sidebar)),
        &ToggleSidebar,
        cx,
    )]
}

/// The items at the bar's right end, in order; the right pane's toggle is the last. A new item of the
/// right end ("Follow the agent", a Play that runs) is added here, when its function exists.
fn right_items(bar: Bar, palette: &Palette, cx: &App) -> Vec<AnyElement> {
    let playing = bar.is_playing.then(|| playing_item(palette));
    let follow = bar.follow.map(|is_on| follow_item(is_on, palette, cx));
    let terminal = bar.terminal.map(|item| terminal_item(item, palette, cx));
    let pane = pane_toggle::button(
        ("toggle-changes", "toggle-changes-icon"),
        (icon(false, bar.panes.changes), theme::STATUS_CONTROL_HEIGHT),
        tr(menus::changes_label(bar.panes.changes)),
        &ToggleChanges,
        cx,
    );
    let items = playing.into_iter().chain(follow).chain(terminal);
    items.chain([pane]).collect()
}

/// The Play of the lavoro on screen runs: said in words, beside the bar's sentence.
fn playing_item(palette: &Palette) -> AnyElement {
    super::chrome::text(theme::TEXT_LABEL, palette.text_secondary)
        .debug_selector(|| "status-playing".into())
        .flex_none()
        .px(theme::GAP)
        .child(tr(Key("status.playing")))
        .into_any_element()
}

/// "Follow the agent": while on, the right pane opens the file and line the agent is at, and the item
/// has the fill of a chosen view and the icon of a place held; off, the icon is struck. A press turns
/// it off, or on again at the agent's last place.
fn follow_item(is_on: bool, palette: &Palette, cx: &App) -> AnyElement {
    let (mark, label) = match is_on {
        true => (crate::icons::FOLLOWING, Key("status.follow.stop")),
        false => (crate::icons::NOT_FOLLOWING, Key("files.follow")),
    };
    let button = pane_toggle::button(
        ("status-follow", "status-follow-icon"),
        (mark, theme::STATUS_CONTROL_HEIGHT),
        tr(label),
        &ToggleFollow,
        cx,
    );
    div()
        .flex_none()
        .rounded(theme::RADIUS_CONTROL)
        .when(is_on, |slot| slot.bg(palette.fill))
        .child(button)
        .into_any_element()
}

/// The terminal of the lavoro on screen: it opens one, shows it, or goes back to the agent. While the
/// terminal is what the centre shows the item has the fill of a chosen view.
fn terminal_item(item: TerminalItem, palette: &Palette, cx: &App) -> AnyElement {
    let button = pane_toggle::button(
        ("status-terminal", "status-terminal-icon"),
        (crate::icons::TERMINAL, theme::STATUS_CONTROL_HEIGHT),
        tr(item.label()),
        &SwitchTerminal,
        cx,
    );
    div()
        .flex_none()
        .rounded(theme::RADIUS_CONTROL)
        .when(item == TerminalItem::Shown, |slot| slot.bg(palette.fill))
        .child(button)
        .into_any_element()
}

/// The icon of a pane's toggle: the left pane's or the right one's, open or closed.
fn icon(is_left: bool, is_open: bool) -> PaneIcon {
    match is_left {
        true => PaneIcon::Left { is_open },
        false => PaneIcon::Right { is_open },
    }
}

/// The sentence, in the room the two toggles leave it: it starts on the line of the sidebar's names and
/// ends with an ellipsis when the room is short.
fn sentence(line: Line, details: Option<&str>, palette: &Palette) -> impl IntoElement {
    let colour = |tone: Tone| -> Hsla {
        match tone {
            Tone::Strained => palette.warning,
            Tone::Failed => palette.danger,
        }
    };
    let highlights = line.toned.into_iter().map(|(range, tone)| {
        let style = HighlightStyle {
            color: Some(colour(tone)),
            ..HighlightStyle::default()
        };
        (range, style)
    });
    let text = StyledText::new(SharedString::from(line.text)).with_highlights(highlights);
    let details = details.map(|details| SharedString::from(format!("{}: {details}", tr(DETAILS))));
    div()
        .id("status-line")
        .when_some(details, |line, details| {
            line.tooltip(move |window, cx| Tooltip::new(details.clone()).build(window, cx))
        })
        .debug_selector(|| "status-line".into())
        .flex_1()
        .min_w_0()
        .ml(theme::ROW_GAP - theme::STATUS_CONTROL_BLEED)
        .mr(theme::GAP)
        .truncate()
        .text_size(theme::TEXT_LABEL)
        .text_color(palette.text_tertiary)
        .font_features(tabular())
        .child(text)
}

/// "Carico alto" reads "carico alto" after a comma.
fn lowercase_first(text: &str) -> String {
    let mut letters = text.chars();
    match letters.next() {
        Some(first) => first.to_lowercase().chain(letters).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakura_core::models::{BoxStatus, MemoryPressure};

    fn machine(load1: f64, pressure: MemoryPressure) -> Machine {
        Machine {
            cpus: 10,
            load1,
            memory_pressure: pressure,
            swap_used_mb: 1228,
            swap_total_mb: 2048,
            disk_free_gb: 100.0,
        }
    }

    fn runner() -> RunnerState {
        let status: BoxStatus = serde_json::from_str(include_str!(
            "../../../sakura-core/tests/fixtures/status.json"
        ))
        .expect("the fixture parses");
        status.runner
    }

    fn toned(line: &Line, tone: Tone) -> Vec<&str> {
        let parts = line.toned.iter().filter(|(_, found)| *found == tone);
        parts.map(|(range, _)| &line.text[range.clone()]).collect()
    }

    /// The fixture's runner with nothing under way and nothing waiting.
    fn idle() -> RunnerState {
        let mut runner = runner();
        (runner.current, runner.queue) = (None, Vec::new());
        runner
    }

    #[test]
    fn at_rest_the_bar_says_nothing() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let line = line(
            Some(&machine(3.2, MemoryPressure::Normal)),
            Some(&idle()),
            None,
        );
        assert_eq!(
            line,
            Line::default(),
            "nothing runs and nothing is strained"
        );
    }

    #[test]
    fn a_check_under_way_is_said_with_its_lavoro_and_how_many_wait() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let rested = machine(3.2, MemoryPressure::Normal);
        let line = line(Some(&rested), Some(&runner()), None);
        assert_eq!(
            line.text,
            "Checking test of sample-folders, 1 check waiting"
        );
        assert!(
            line.toned.is_empty(),
            "work under way is said in the quiet tone"
        );
        assert!(!line.text.contains('\u{b7}'), "no middle dots");
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let line = super::line(Some(&rested), Some(&runner()), None);
        assert_eq!(
            line.text,
            "Verifica test di sample-folders, 1 verifica in attesa"
        );
    }

    #[test]
    fn the_machine_is_said_only_while_it_is_strained_and_only_the_part_that_is() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let loaded = line(Some(&machine(14.0, MemoryPressure::Normal)), None, None);
        assert_eq!(toned(&loaded, Tone::Strained), [loaded.text.as_str()]);
        assert!(loaded.text.starts_with("Carico alto"), "{}", loaded.text);
        assert!(!loaded.text.contains("swap"), "{}", loaded.text);
        let pressed = line(Some(&machine(1.0, MemoryPressure::Critical)), None, None);
        assert_eq!(toned(&pressed, Tone::Strained), [pressed.text.as_str()]);
        assert!(pressed.text.starts_with("Memoria"), "{}", pressed.text);
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let both = line(Some(&machine(14.0, MemoryPressure::Critical)), None, None);
        assert_eq!(toned(&both, Tone::Strained).len(), 2);
        assert!(both.text.starts_with("High load, 14"), "{}", both.text);
        assert!(both.text.ends_with(", memory critical"), "{}", both.text);
    }

    #[test]
    fn a_read_error_comes_first_in_the_danger_tone() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let error = Said::new("facts.error.of_task")
            .text("task", "m1")
            .said("error", Said::new("error.docker.down"));
        let said = say(&error);
        let line = line(
            Some(&machine(14.0, MemoryPressure::Normal)),
            None,
            Some(&error),
        );
        assert!(
            line.text.starts_with(&format!("{said}, carico alto")),
            "{}",
            line.text
        );
        assert_eq!(toned(&line, Tone::Failed), [said.as_ref()]);
    }

    #[test]
    fn a_stopped_runner_is_said_only_when_checks_wait_for_it() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let mut stopped = runner();
        stopped.state = sakura_core::models::RunnerPhase::Stopped;
        stopped.current = None;
        let line = line(None, Some(&stopped), None);
        assert!(line.text.ends_with("1 check waiting"), "{}", line.text);
        assert!(line.text.starts_with("Checks stopped"), "{}", line.text);
        stopped.queue.clear();
        assert_eq!(super::line(None, Some(&stopped), None), Line::default());
    }

    #[test]
    fn nothing_known_is_an_empty_sentence() {
        assert_eq!(line(None, None, None), Line::default());
    }

    #[test]
    fn lowercase_first_lowers_only_the_first_letter() {
        assert_eq!(lowercase_first("Carico alto"), "carico alto");
        assert_eq!(lowercase_first("Élite"), "élite");
        assert_eq!(lowercase_first(""), "");
    }
}
