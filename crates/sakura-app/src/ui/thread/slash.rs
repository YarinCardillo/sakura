//! Slash commands in the master's composer. The agent announces its commands (`available_commands_update`);
//! typing "/" at the start of the composer opens a menu of them with their descriptions, filtered as the
//! word is typed. Return, or a click, puts the chosen command in the composer with a space after it, ready
//! for its arguments; sending it is the person's next Return, and the command goes to the agent as the text
//! of a message, which is how the adapter runs every command it announces.

use super::{copy, ThreadView};
use crate::l10n::{tr, Key};
use crate::theme::thread as tokens;
use crate::theme::{self, Palette};
use crate::ui::chrome::text;
use gpui::{
    div, AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, Window,
};
use sakura_acp::model::SlashCommand;

/// The word being typed as a command at the start of `typed`: `co` of `/co`. None when the text does not start
/// with a slash, or the word is done (a space or a line break follows it).
pub(super) fn slash_word(typed: &str) -> Option<&str> {
    let word = typed.strip_prefix('/')?;
    (!word.contains(char::is_whitespace)).then_some(word)
}

/// The commands that match `word`: the one named exactly so first, then those whose name starts with it, then
/// those that hold it, each in the order the agent announced them; the case of the letters does not matter.
pub(super) fn matching<'a>(commands: &'a [SlashCommand], word: &str) -> Vec<&'a SlashCommand> {
    let word = word.to_lowercase();
    let (starts, holds): (Vec<_>, Vec<_>) = commands
        .iter()
        .filter(|command| command.name.to_lowercase().contains(&word))
        .partition(|command| command.name.to_lowercase().starts_with(&word));
    let (exact, starts): (Vec<_>, Vec<_>) = starts
        .into_iter()
        .partition(|command| command.name.to_lowercase() == word);
    exact.into_iter().chain(starts).chain(holds).collect()
}

impl ThreadView {
    /// The commands the composer's first word asks for; none when the master's composer holds no such word.
    fn slash_matches(&self, cx: &App) -> Vec<SlashCommand> {
        if self.master.is_none() {
            return Vec::new();
        }
        let typed = self.composer.read(cx).value();
        // The Return that picks a command has put a line break after what was typed by the time it is heard.
        let typed = typed.trim_end_matches(['\n', '\r']);
        let Some(word) = slash_word(typed) else {
            return Vec::new();
        };
        self.listed_for(word)
    }

    /// The commands that match `word`: the window's own (`/clear`, `/model`, `/mode`, `/help`) first, then the
    /// agent's, each group ranked by [`matching`].
    fn listed_for(&self, word: &str) -> Vec<SlashCommand> {
        let own = self.local_listing();
        let own = matching(&own, word).into_iter().cloned();
        let agents = matching(&self.commands, word).into_iter().cloned();
        own.chain(agents).collect()
    }

    /// Whether the menu of commands is open: Return then picks a command instead of sending.
    pub fn slash_is_open(&self, cx: &App) -> bool {
        !self.slash_matches(cx).is_empty()
    }

    /// Moves the highlighted command by `step`, round the list.
    pub fn slash_move(&mut self, step: isize, cx: &mut Context<Self>) {
        let count = self.slash_matches(cx).len() as isize;
        if count > 0 {
            let at = (self.slash_selected as isize + step).rem_euclid(count);
            self.slash_selected = at as usize;
            self.slash_scroll.scroll_to_item(at as usize);
            cx.notify();
        }
    }

    /// Puts the highlighted command in the composer, with a space after it.
    pub fn slash_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let matches = self.slash_matches(cx);
        if let Some(command) = matches.get(self.slash_selected.min(matches.len().saturating_sub(1)))
        {
            self.take_listed(&command.name.clone(), window, cx);
        }
    }

    pub(super) fn slash_insert(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let written = format!("/{name} ");
        let end = gpui_component::input::Position::new(0, written.encode_utf16().count() as u32);
        // The caret goes to the end, where the command's arguments are typed.
        self.composer.update(cx, |composer, cx| {
            composer.set_value(written, window, cx);
            composer.set_cursor_position(end, window, cx);
        });
        self.slash_selected = 0;
        self.slash_scroll.scroll_to_item(0);
        self.is_help_open = false;
        cx.notify();
    }

    /// Every command there is, for the list of `/help`: the window's own, then the agent's.
    fn help_listing(&self) -> Vec<SlashCommand> {
        let mut all = self.local_listing();
        all.extend(self.commands.iter().cloned());
        all
    }

    /// The commands the agent announced, for the tests.
    #[cfg(test)]
    pub fn announced_commands(&self) -> &[SlashCommand] {
        &self.commands
    }

    /// The menu over the composer: one row for each command that matches, the highlighted one filled.
    pub(super) fn slash_menu(
        &self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let matches = match self.slash_matches(cx) {
            typed if !typed.is_empty() => typed,
            _ if self.is_help_open => self.help_listing(),
            _ => return None,
        };
        // The list of `/help` highlights nothing: Return in an empty composer must not run a command.
        let selected = match self.is_help_open {
            true => usize::MAX,
            false => self.slash_selected.min(matches.len() - 1),
        };
        let rows = matches.into_iter().enumerate().map(|(index, command)| {
            let name = command.name.clone();
            let picked = name.clone();
            let row_selector = format!("slash-row-{name}");
            let label = action_words(&command);
            div()
                .id(SharedString::from(row_selector.clone()))
                .debug_selector(move || row_selector.clone())
                .w_full()
                .min_h(tokens::SLASH_ROW_HEIGHT)
                .px(tokens::BLOCK_PADDING_X)
                .flex()
                .items_center()
                .gap(theme::GAP)
                .rounded(theme::RADIUS_CONTROL)
                .when_selected(index == selected, palette.fill_hover)
                .child(
                    text(theme::TEXT_SMALL, palette.text)
                        .min_w_0()
                        .truncate()
                        .font_weight(theme::WEIGHT_MEDIUM)
                        .child(label),
                )
                .on_click(
                    cx.listener(move |view, _, window, cx| view.take_listed(&picked, window, cx)),
                )
        });
        Some(
            div()
                .id("slash-menu")
                .debug_selector(|| "slash-menu".to_string())
                .absolute()
                .left_0()
                .right_0()
                .bottom(gpui::relative(1.))
                .w_full()
                .max_h(tokens::SLASH_MENU_HEIGHT)
                .mb(theme::GAP)
                .p(theme::GAP_TIGHT)
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .track_scroll(&self.slash_scroll)
                .rounded(tokens::COMPOSER_RADIUS)
                .bg(palette.fill)
                .children(rows)
                .into_any_element(),
        )
    }
}

fn action_words(command: &SlashCommand) -> String {
    let key = match command.name.as_str() {
        "compact" => Key("slash.compact"),
        "review" => Key("slash.review"),
        "context" => Key("slash.context"),
        _ if !command.description.trim().is_empty() => return copy::shown(&command.description),
        _ => Key("slash.agent_action"),
    };
    tr(key).to_string()
}

/// A row's fill when it is the highlighted one.
trait WhenSelected: Styled + Sized {
    fn when_selected(self, is_selected: bool, fill: gpui::Hsla) -> Self {
        match is_selected {
            true => self.bg(fill),
            false => self,
        }
    }
}

impl<T: Styled + Sized> WhenSelected for T {}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str) -> SlashCommand {
        SlashCommand {
            name: name.into(),
            description: String::new(),
            hint: None,
        }
    }

    #[test]
    fn the_command_named_exactly_comes_before_those_that_only_start_with_it() {
        let all = [command("model"), command("mode")];
        let first = |word: &str| matching(&all, word)[0].name.clone();
        assert_eq!(
            first("mode"),
            "mode",
            "Return on /mode must not take /model"
        );
        assert_eq!(first("mod"), "model");
    }

    #[test]
    fn a_slash_word_is_what_follows_the_slash_at_the_start_until_a_space() {
        assert_eq!(slash_word("/"), Some(""));
        assert_eq!(slash_word("/co"), Some("co"));
        assert_eq!(slash_word("/compact "), None, "the word is done");
        assert_eq!(slash_word("/review 12"), None);
        assert_eq!(slash_word("hello /co"), None, "only at the start");
        assert_eq!(slash_word(" /co"), None);
        assert_eq!(slash_word(""), None);
    }

    #[test]
    fn commands_match_by_what_the_word_starts_then_by_what_it_holds_in_the_announced_order() {
        let all = [
            command("review"),
            command("compact"),
            command("context"),
            command("mcp:docs:search"),
        ];
        let names = |word: &str| {
            matching(&all, word)
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(""),
            ["review", "compact", "context", "mcp:docs:search"]
        );
        assert_eq!(names("co"), ["compact", "context"]);
        assert_eq!(
            names("c"),
            ["compact", "context", "mcp:docs:search"],
            "holds come after starts"
        );
        assert_eq!(names("REV"), ["review"], "case does not matter");
        assert!(names("zz").is_empty());
    }
}
