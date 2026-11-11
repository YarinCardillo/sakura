//! What the command palette offers, as a pure function of where the window is: the commands on the task
//! shown first, then the places to go, the window's commands and the language. A command the window
//! already has an action and a key for (new task, open file, the panes, close tab, the language) is that
//! action, so the palette shows its key and a choice does exactly what the key does; the others are asked
//! of the desk.

use crate::l10n::{tr, tr_with, Key, Language, LanguageChoice};
use crate::menus::{self, PanesShown};
use crate::ui::right_pane::PaneView;
use gpui::Pixels;

/// What a chosen command asks of the desk. A command on a task names that task: it was listed for it,
/// and it is carried out on it or not at all, whatever the window shows by then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// Show this task.
    ShowTask(String),
    /// Show this view of this task's right pane.
    ShowView { task: String, view: PaneView },
    /// Publish the work of this task.
    Publish(String),
    /// End this task.
    Archive(String),
}

/// What choosing a command does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    /// Asked of the desk.
    Ask(Ask),
    /// The window's own actions, with their keys.
    NewTask,
    OpenFile,
    ToggleSidebar,
    ToggleChanges,
    CloseTab,
    StartPlay,
    StopPlay,
    Settings,
    Language(LanguageChoice),
}

/// One command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    pub run: Run,
    /// The choice in force among alternatives (the view shown, the language).
    pub is_current: bool,
}

/// A labelled group of commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub label: String,
    pub entries: Vec<Entry>,
}

/// What the commands on the task shown depend on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownTask {
    pub name: String,
    /// The view its right pane shows.
    pub view: PaneView,
    /// Its work can be published now.
    pub can_publish: bool,
    /// It can be ended.
    pub can_archive: bool,
    /// Where its Play stands.
    pub play: PlayState,
}

/// Where the Play of the task shown stands, for what the palette offers of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    /// The task has no Play: neither its view nor a command.
    Absent,
    /// It has one that cannot start here: its view says why.
    Unable,
    /// It can start.
    AtRest,
    /// It runs.
    Running,
}

/// Where the window is when the palette opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Situation {
    /// Every task, in the sidebar's order.
    pub tasks: Vec<String>,
    /// The task shown, or `None` when no lavoro is.
    pub shown: Option<ShownTask>,
    pub panes: PanesShown,
    pub language: LanguageChoice,
    /// Another tab remains, so the one shown can close.
    pub can_close_tab: bool,
    /// Where the panel starts, from the window's left edge: the palette is centred in the panel.
    pub panel_left: Pixels,
}

const GO_LABEL: Key = Key("palette.group.go");
const WINDOW_LABEL: Key = Key("palette.group.window");
const LANGUAGE_LABEL: Key = Key("menu.app.language");
/// The commands for `situation`, in the order the palette lists them.
pub fn groups(situation: &Situation) -> Vec<Group> {
    let task = situation.shown.as_ref().map(task_group);
    let rest = [places(situation), window(situation), languages(situation)];
    // The only lavoro there is, already shown, leaves nowhere to go: no label over nothing.
    let listed = task.into_iter().chain(rest);
    listed.filter(|group| !group.entries.is_empty()).collect()
}

/// The commands on the task shown, under its name: its views first, so the command a bare Enter runs
/// changes nothing that cannot be changed back, then its Play, then what ends its work.
fn task_group(task: &ShownTask) -> Group {
    let name = || task.name.clone();
    let has_play = task.play != PlayState::Absent;
    let views = PaneView::ALL
        .iter()
        .filter(move |view| has_play || **view != PaneView::Play);
    let views = views.map(|view| {
        let ask = Ask::ShowView {
            task: name(),
            view: *view,
        };
        Entry {
            is_current: *view == task.view,
            ..entry(
                tr_with(Key("palette.show_view"), &[("view", view.label())]),
                ask,
            )
        }
    });
    let publish = task
        .can_publish
        .then(|| entry(tr(Key("publish.action")), Ask::Publish(name())));
    let archive = task
        .can_archive
        .then(|| entry(tr(Key("task_menu.delete")), Ask::Archive(name())));
    Group {
        label: task.name.clone(),
        entries: views
            .chain(play(task))
            .chain(publish)
            .chain(archive)
            .collect(),
    }
}

/// Start or Stop of the task's Play, whichever a press would do now; nothing for a Play that cannot
/// start.
fn play(task: &ShownTask) -> Option<Entry> {
    let (label, run) = match task.play {
        PlayState::Absent | PlayState::Unable => return None,
        PlayState::AtRest => (Key("menu.view.play_start"), Run::StartPlay),
        PlayState::Running => (Key("menu.view.play_stop"), Run::StopPlay),
    };
    Some(Entry {
        label: tr(label).to_string(),
        run,
        is_current: false,
    })
}

/// Whether `run` is a command of `situation`: the check made right before a chosen command is carried
/// out, on the window as it is then.
pub fn is_offered(situation: &Situation, run: &Run) -> bool {
    groups(situation)
        .iter()
        .flat_map(|group| &group.entries)
        .any(|entry| entry.run == *run)
}

/// Every task, less the one already shown.
fn places(situation: &Situation) -> Group {
    let shown = situation.shown.as_ref().map(|task| task.name.as_str());
    let tasks = situation
        .tasks
        .iter()
        .filter(|name| Some(name.as_str()) != shown)
        .map(|name| entry(name.clone(), Ask::ShowTask(name.clone())));
    Group {
        label: tr(GO_LABEL).to_string(),
        entries: tasks.collect(),
    }
}

/// The window's commands, worded as its menus word them.
fn window(situation: &Situation) -> Group {
    let command = |label: &str, run: Run| Entry {
        label: label.to_string(),
        run,
        is_current: false,
    };
    let mut entries = vec![
        command(tr(menus::NEW_TASK), Run::NewTask),
        command(tr(Key("menu.file.open_file")), Run::OpenFile),
        command(
            tr(menus::sidebar_label(situation.panes.sidebar)),
            Run::ToggleSidebar,
        ),
        command(
            tr(menus::changes_label(situation.panes.changes)),
            Run::ToggleChanges,
        ),
    ];
    if situation.can_close_tab {
        entries.push(command(tr(Key("tabs.close")), Run::CloseTab));
    }
    entries.push(command(tr(Key("menu.app.settings")), Run::Settings));
    Group {
        label: tr(WINDOW_LABEL).to_string(),
        entries,
    }
}

/// The language of the interface, as the app's menu lists it.
fn languages(situation: &Situation) -> Group {
    let named = |language: Language| (language.name(), LanguageChoice::Is(language));
    let choices = [
        (tr(Key("language.automatic")), LanguageChoice::Automatic),
        named(Language::English),
        named(Language::Italian),
    ];
    let entries = choices.into_iter().map(|(label, choice)| Entry {
        label: label.to_string(),
        run: Run::Language(choice),
        is_current: choice == situation.language,
    });
    Group {
        label: tr(LANGUAGE_LABEL).to_string(),
        entries: entries.collect(),
    }
}

fn entry(label: impl Into<String>, ask: Ask) -> Entry {
    Entry {
        label: label.into(),
        run: Run::Ask(ask),
        is_current: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panes() -> PanesShown {
        PanesShown {
            sidebar: true,
            changes: true,
        }
    }

    fn no_lavoro_shown() -> Situation {
        Situation {
            tasks: vec!["m1".into(), "m2".into()],
            shown: None,
            panes: panes(),
            language: LanguageChoice::Automatic,
            can_close_tab: false,
            panel_left: Pixels::ZERO,
        }
    }

    fn on_task(can_publish: bool, can_archive: bool) -> Situation {
        Situation {
            shown: Some(ShownTask {
                name: "m1".into(),
                view: PaneView::Modifiche,
                can_publish,
                can_archive,
                play: PlayState::Absent,
            }),
            can_close_tab: true,
            ..no_lavoro_shown()
        }
    }

    fn labels(group: &Group) -> Vec<&str> {
        group
            .entries
            .iter()
            .map(|entry| entry.label.as_str())
            .collect()
    }

    fn group<'a>(groups: &'a [Group], label: &str) -> &'a Group {
        match groups.iter().find(|group| group.label == label) {
            Some(group) => group,
            None => panic!("no group {label}"),
        }
    }

    #[test]
    fn with_no_lavoro_shown_the_palette_goes_places_and_has_no_task_commands() {
        crate::l10n::set_for_test(Language::Italian);
        let groups = groups(&no_lavoro_shown());
        let names: Vec<&str> = groups.iter().map(|group| group.label.as_str()).collect();
        assert_eq!(names, ["Vai a", "Finestra", "Lingua"]);
        assert_eq!(
            labels(group(&groups, "Vai a")),
            ["m1", "m2"],
            "not where it is"
        );
    }

    #[test]
    fn on_a_task_its_commands_come_first_under_its_name() {
        crate::l10n::set_for_test(Language::English);
        let groups = groups(&on_task(true, true));
        assert_eq!(groups[0].label, "m1");
        assert_eq!(
            labels(&groups[0]),
            [
                "Show Files",
                "Show Changes",
                "Show Checks",
                "Show Git",
                "Publish",
                "Archive"
            ]
        );
        assert_eq!(labels(group(&groups, "Go to")), ["m2"]);
    }

    #[test]
    fn with_the_only_lavoro_shown_there_is_nowhere_to_go_and_no_empty_group() {
        crate::l10n::set_for_test(Language::Italian);
        let mut situation = on_task(false, false);
        situation.tasks = vec!["m1".into()];
        let groups = groups(&situation);
        let names: Vec<&str> = groups.iter().map(|group| group.label.as_str()).collect();
        assert_eq!(names, ["m1", "Finestra", "Lingua"]);
    }

    #[test]
    fn a_bare_enter_shows_a_view_and_never_publishes_or_archives() {
        let first = &groups(&on_task(true, true))[0].entries[0];
        assert!(matches!(first.run, Run::Ask(Ask::ShowView { .. })));
    }

    #[test]
    fn a_command_is_offered_only_to_the_task_and_the_state_it_was_listed_for() {
        let publish = Run::Ask(Ask::Publish("m1".into()));
        assert!(is_offered(&on_task(true, false), &publish));
        assert!(
            !is_offered(&on_task(false, false), &publish),
            "its agent is at work by now"
        );
        let mut elsewhere = on_task(true, true);
        if let Some(shown) = elsewhere.shown.as_mut() {
            shown.name = "m2".into();
        }
        assert!(!is_offered(&elsewhere, &publish), "another task is shown");
        assert!(is_offered(&elsewhere, &Run::NewTask));
    }

    #[test]
    fn what_ends_the_work_is_offered_only_when_it_can_be_done() {
        crate::l10n::set_for_test(Language::English);
        let neither = groups(&on_task(false, false));
        assert!(!labels(&neither[0]).contains(&"Publish"));
        assert!(!labels(&neither[0]).contains(&"Archive"));
        let both = groups(&on_task(true, true));
        assert!(labels(&both[0]).contains(&"Publish"));
        assert!(labels(&both[0]).contains(&"Archive"));
    }

    #[test]
    fn the_view_shown_and_the_language_in_force_are_marked() {
        crate::l10n::set_for_test(Language::Italian);
        let groups = groups(&on_task(false, false));
        let current: Vec<&str> = groups
            .iter()
            .flat_map(|group| &group.entries)
            .filter(|entry| entry.is_current)
            .map(|entry| entry.label.as_str())
            .collect();
        assert_eq!(current, ["Mostra Modifiche", "Automatica"]);
    }

    #[test]
    fn the_window_commands_say_what_a_press_would_do() {
        crate::l10n::set_for_test(Language::Italian);
        let mut situation = on_task(false, false);
        situation.panes.sidebar = false;
        let groups = groups(&situation);
        assert_eq!(
            labels(group(&groups, "Finestra")),
            [
                "Nuovo lavoro",
                "Apri file…",
                "Mostra barra laterale",
                "Nascondi modifiche",
                "Chiudi scheda",
                "Impostazioni…"
            ]
        );
        let alone = window_commands_with_no_lavoro_shown();
        assert!(
            !alone.contains(&"Chiudi scheda".to_string()),
            "the last tab stays"
        );
    }

    #[test]
    fn play_is_offered_as_what_a_press_would_do_and_only_to_a_task_that_has_one() {
        crate::l10n::set_for_test(Language::English);
        let task_commands = |play: PlayState| {
            let mut situation = on_task(false, false);
            if let Some(shown) = situation.shown.as_mut() {
                shown.play = play;
            }
            let groups = groups(&situation);
            let labels: Vec<String> = labels(&groups[0]).into_iter().map(String::from).collect();
            labels[4..].to_vec()
        };
        assert_eq!(
            task_commands(PlayState::AtRest),
            ["Show Play", "Start Play"]
        );
        assert_eq!(
            task_commands(PlayState::Running),
            ["Show Play", "Stop Play"]
        );
        assert_eq!(
            task_commands(PlayState::Unable),
            ["Show Play"],
            "its view says why it cannot start"
        );
        assert!(task_commands(PlayState::Absent).is_empty());
    }

    #[test]
    fn settings_is_a_command_of_the_window_wherever_it_is() {
        assert!(is_offered(&no_lavoro_shown(), &Run::Settings));
        assert!(is_offered(&on_task(true, true), &Run::Settings));
    }

    fn window_commands_with_no_lavoro_shown() -> Vec<String> {
        let groups = groups(&no_lavoro_shown());
        let window = group(&groups, "Finestra");
        window
            .entries
            .iter()
            .map(|entry| entry.label.clone())
            .collect()
    }
}
