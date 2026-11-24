//! Commands the Claude adapter does not announce but people type in the master's composer: `/clear` starts
//! fresh, `/model` and `/mode` open the menu of the agent chip, `/plan` puts the conversation in plan mode, `/help` lists every command there is, the
//! agent's and these. Running one empties the composer and leaves nothing in it. A command the agent
//! announces under the same name stays the agent's, and any other word after a slash is sent as text, as
//! before.

use super::master::{MenuPick, PLAN_MODE};
use super::master_choices::AGENT_CHIP;
use super::{composed, ThreadView, ThreadViewEvent};
use crate::l10n::{tr, Key};
use gpui::{Context, Window};
use sakura_acp::model::SlashCommand;

/// A command the window runs itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Local {
    Clear,
    Model,
    Mode,
    /// Puts the conversation in plan mode: the agent reads and proposes, and changes nothing.
    Plan,
    Help,
}

/// The local command `typed` is: its name alone, in any case, with nothing after it, and not a name the
/// agent announces.
pub(super) fn parse(typed: &str, announced: &[SlashCommand]) -> Option<Local> {
    let name = typed.trim().strip_prefix('/')?.to_lowercase();
    let local = match name.as_str() {
        "clear" => Local::Clear,
        "model" => Local::Model,
        "mode" => Local::Mode,
        "plan" => Local::Plan,
        "help" => Local::Help,
        _ => return None,
    };
    let is_announced = announced
        .iter()
        .any(|command| command.name.to_lowercase() == name);
    (!is_announced).then_some(local)
}

impl Local {
    /// Its name after the slash, and what it does in a line.
    fn name_and_words(self) -> (&'static str, Key) {
        match self {
            Local::Clear => ("clear", Key("slash.clear")),
            Local::Model => ("model", Key("slash.model")),
            Local::Mode => ("mode", Key("slash.mode")),
            Local::Plan => ("plan", Key("slash.plan")),
            Local::Help => ("help", Key("slash.help")),
        }
    }
}

impl ThreadView {
    /// The local commands the list of `/help` adds to the agent's: those the agent does not announce and
    /// the window can run now.
    pub(super) fn local_listing(&self) -> Vec<SlashCommand> {
        let all = [
            Local::Clear,
            Local::Model,
            Local::Mode,
            Local::Plan,
            Local::Help,
        ];
        let offered = all.into_iter().filter(|local| match local {
            Local::Model | Local::Mode | Local::Plan => self.has_agent_chip(),
            Local::Clear | Local::Help => true,
        });
        let listed = offered.map(|local| {
            let (name, words) = local.name_and_words();
            SlashCommand {
                name: name.to_string(),
                description: tr(words).to_string(),
                hint: None,
            }
        });
        listed
            .filter(|command| parse(&format!("/{}", command.name), &self.commands).is_some())
            .collect()
    }

    /// The command `name` as the list of `/help` was pressed on it: a local one runs, the agent's is put in
    /// the composer for its arguments.
    pub(super) fn take_listed(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        match parse(&format!("/{name}"), &self.commands) {
            Some(local) => self.run_local(local, window, cx),
            None => self.slash_insert(name, window, cx),
        }
    }

    /// The local command the composer holds, when this is the master's composer and the command can run:
    /// `/model` and `/mode` only while the agent chip is shown, otherwise they are sent as text.
    pub(super) fn local_command(&self, cx: &gpui::App) -> Option<Local> {
        self.master.as_ref()?;
        let local = parse(&composed(&self.composer, cx), &self.commands)?;
        let can_run = match local {
            Local::Model | Local::Mode | Local::Plan => self.has_agent_chip(),
            Local::Clear | Local::Help => true,
        };
        can_run.then_some(local)
    }

    /// Runs `local`: the composer is emptied first, then the command does its work.
    pub(super) fn run_local(&mut self, local: Local, window: &mut Window, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.is_help_open = local == Local::Help;
        match local {
            Local::Clear => cx.emit(ThreadViewEvent::StartFresh),
            Local::Model | Local::Mode => self.open_chip(AGENT_CHIP, window, cx),
            Local::Plan => self.pick_entry(&MenuPick::Mode(PLAN_MODE.to_string()), cx),
            Local::Help => {}
        }
        cx.notify();
    }

    /// The list of `/help` goes once something is typed; emptying the composer, as running a command does,
    /// leaves it.
    pub(super) fn close_help_when_typing(&mut self, cx: &gpui::App) {
        if !composed(&self.composer, cx).is_empty() {
            self.is_help_open = false;
        }
    }

    /// Closes the list of `/help`; says whether it was open.
    pub(super) fn close_help(&mut self, cx: &mut Context<Self>) -> bool {
        let was_open = std::mem::take(&mut self.is_help_open);
        if was_open {
            cx.notify();
        }
        was_open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn announced(names: &[&str]) -> Vec<SlashCommand> {
        let command = |name: &&str| SlashCommand {
            name: name.to_string(),
            description: String::new(),
            hint: None,
        };
        names.iter().map(command).collect()
    }

    #[test]
    fn the_five_local_commands_are_known_by_their_name_alone_in_any_case() {
        assert_eq!(parse("/plan", &[]), Some(Local::Plan));
        assert_eq!(parse("/clear", &[]), Some(Local::Clear));
        assert_eq!(parse("/model", &[]), Some(Local::Model));
        assert_eq!(parse("/mode", &[]), Some(Local::Mode));
        assert_eq!(parse("/Help\n", &[]), Some(Local::Help));
    }

    #[test]
    fn any_other_word_or_a_command_with_arguments_is_not_local() {
        for typed in [
            "/clearly",
            "/clear all",
            "clear",
            "/",
            "/models",
            "/frobnicate",
            "",
        ] {
            assert_eq!(parse(typed, &[]), None, "{typed:?}");
        }
    }

    #[test]
    fn a_name_the_agent_announces_stays_the_agent_s() {
        let all = announced(&["help", "compact"]);
        assert_eq!(parse("/help", &all), None);
        assert_eq!(parse("/clear", &all), Some(Local::Clear));
    }
}
