//! What the command palette offers in the engine's window, as a pure function of where the window is: the
//! lanes and the pages to go to first, then the actions of the page on screen, then the projects. A command the
//! window already has an action and a key for (New lane, Settings, close the tab, quick open) is that
//! action, so the palette shows its key and a choice does exactly what the key does; the others are asked
//! of the desk. Typing narrows the list by what the words contain, so a lane is found by its name.

use super::entry;
use crate::icons;
use crate::l10n::{tr, Key};
use crate::ui::kit::StateGlyph;
use gpui::Pixels;
use gpui_kit_assets::IconName;
use sakura_core::tabs::Page;

/// What a chosen command asks of the desk. A command on a lane or an ask names it: it was listed for it,
/// and it is carried out on it or not at all, whatever the window shows by then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// Show this page, in a tab of its own when asked.
    Go {
        page: Page,
        is_new_tab: bool,
    },
    /// Show the Lanes page kept to the lanes of the project in this folder.
    OpenProject {
        folder: String,
        is_new_tab: bool,
    },
    /// Stop this lane (its key).
    StopLane(String),
    NewTrack,
    /// A verb of the ask the Inbox has selected, by the name the Inbox gave it, on the wait `wait`.
    Answer {
        wait: String,
        verb: String,
    },
}

impl Ask {
    /// The ask with a tab of its own for the page it shows, as Cmd-Return chooses.
    pub fn in_new_tab(self, is_new_tab: bool) -> Self {
        match self {
            Ask::Go { page, .. } => Ask::Go { page, is_new_tab },
            Ask::OpenProject { folder, .. } => Ask::OpenProject { folder, is_new_tab },
            other => other,
        }
    }
}

/// What choosing a command does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    /// Asked of the desk.
    Ask(Ask),
    /// The window's own actions, with their keys.
    NewLane,
    QuickOpen,
    CloseTab,
    Settings,
}

/// One command.
pub type Entry = entry::Entry<Run>;

/// A labelled group of commands.
pub type Group = entry::Group<Run>;

/// A lane the palette can go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneRef {
    /// The lane's key, as the tabs name it.
    pub key: String,
    pub name: String,
    /// The mark of the state the lane is in.
    pub glyph: StateGlyph,
}

/// A project the palette can go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRef {
    pub folder: String,
    pub name: String,
}

/// One verb the Inbox offers on its selected ask: its name, which the Inbox understands when it comes back,
/// and the interface's word for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskVerb {
    pub verb: String,
    pub label: String,
}

/// The ask the Inbox has selected, with the verbs the person can answer it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedAsk {
    /// The wait's id.
    pub wait: String,
    pub verbs: Vec<AskVerb>,
}

/// Where the window is when the palette opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Situation {
    /// The page of the tab shown.
    pub page: Page,
    pub has_scheduled: bool,
    pub can_create_track: bool,
    pub lanes: Vec<LaneRef>,
    pub projects: Vec<ProjectRef>,
    pub tracks: Vec<(String, String)>,
    /// The lane on screen, when the engine offers to stop it.
    pub stoppable: Option<String>,
    /// The ask the Inbox has selected, if the Inbox is on screen and has one.
    pub selected_ask: Option<SelectedAsk>,
    /// Another tab remains, so the one shown can close.
    pub can_close_tab: bool,
    /// Where the panel starts, from the window's left edge: the palette is centred in the panel.
    pub panel_left: Pixels,
}

const ACTIONS_LABEL: Key = Key("palette.group.actions");
const GO_LABEL: Key = Key("palette.group.go");
const LANES_LABEL: Key = Key("palette.group.lanes");
const PROJECTS_LABEL: Key = Key("palette.group.projects");
const STOP_LANE: Key = Key("palette.stop_lane");
/// The pages, in the order the sidebar lists them, with the word of each.
const PAGES: [(Page, Key); 5] = [
    (Page::Inbox, Key("page.inbox")),
    (Page::Master, Key("page.master")),
    (Page::Lanes, Key("page.lanes")),
    (Page::Scheduled, Key("page.scheduled")),
    (Page::Engine, Key("page.engine")),
];

/// The commands for `situation`, in the order the palette lists them: the lanes and the pages to go to
/// come first, so that what the list highlights when it opens is a place to go and never an answer to an
/// ask or a stop, which send something; the actions follow, then the projects.
pub fn groups(situation: &Situation) -> Vec<Group> {
    let listed = [
        lanes(situation),
        pages(situation),
        actions(situation),
        projects(situation),
        tracks(situation),
    ];
    listed
        .into_iter()
        .filter(|group| !group.entries.is_empty())
        .collect()
}

/// Whether `run` is a command of `situation`: the check made right before a chosen command is carried out,
/// on the window as it is then.
pub fn is_offered(situation: &Situation, run: &Run) -> bool {
    groups(situation)
        .iter()
        .flat_map(|group| &group.entries)
        .any(|entry| same_command(&entry.run, run))
}

/// Whether two commands are the same one: the way of choosing a page (in a new tab or not) does not make
/// it another command.
fn same_command(listed: &Run, asked: &Run) -> bool {
    match (listed, asked) {
        (Run::Ask(a), Run::Ask(b)) => a.clone().in_new_tab(false) == b.clone().in_new_tab(false),
        (a, b) => a == b,
    }
}

/// The window's own action that `run` is, when it is one: the kit's list dispatches it and shows its key.
pub fn action_of(run: &Run) -> Option<Box<dyn gpui::Action>> {
    let action: Box<dyn gpui::Action> = match run {
        Run::NewLane => Box::new(crate::menus::NewLane),
        Run::QuickOpen => Box::new(crate::menus::QuickOpenFile),
        Run::CloseTab => Box::new(crate::ui::page_bar::keys::CloseTab),
        Run::Settings => Box::new(crate::menus::ShowSettings),
        Run::Ask(_) => return None,
    };
    Some(action)
}

/// The icon in front of a command, where it helps tell it from the others.
pub fn icon_of(run: &Run) -> Option<IconName> {
    let icon = match run {
        Run::Ask(Ask::Go { page, .. }) => page_icon(page),
        Run::Ask(Ask::OpenProject { .. }) => IconName::Folder,
        Run::Ask(Ask::Answer { .. }) => IconName::Check,
        Run::NewLane | Run::Ask(Ask::NewTrack) => IconName::Plus,
        Run::Settings => icons::SETTINGS,
        _ => return None,
    };
    Some(icon)
}

fn page_icon(page: &Page) -> IconName {
    match page {
        Page::Inbox => icons::WAITING_PAGE,
        Page::Master => icons::MASTER_PAGE,
        Page::Lanes => icons::LANES_PAGE,
        Page::Scheduled => IconName::Calendar,
        Page::Engine => icons::ENGINE_PAGE,
        Page::Lane(_) => icons::RING,
        Page::Project(_) => icons::PROJECT_PAGE,
        Page::Track(_) => IconName::Layers,
    }
}

/// What the page on screen offers: the verbs of the selected ask, Stop for the lane on screen, a new lane,
/// quick open on a lane's page, closing the tab and Settings.
fn actions(situation: &Situation) -> Group {
    let verbs = situation.selected_ask.iter().flat_map(|ask| {
        ask.verbs.iter().map(|verb| {
            let answer = Ask::Answer {
                wait: ask.wait.clone(),
                verb: verb.verb.clone(),
            };
            command(verb.label.clone(), Run::Ask(answer))
        })
    });
    let stop = situation
        .stoppable
        .iter()
        .map(|lane| command(tr(STOP_LANE), Run::Ask(Ask::StopLane(lane.clone()))));
    let is_lane = matches!(situation.page, Page::Lane(_));
    let window = [
        Some(command(tr(Key("sidebar.new_lane")), Run::NewLane)),
        situation
            .can_create_track
            .then(|| command(tr(Key("track.new")), Run::Ask(Ask::NewTrack))),
        is_lane.then(|| command(tr(Key("menu.file.open_file")), Run::QuickOpen)),
        situation
            .can_close_tab
            .then(|| command(tr(Key("tabs.close")), Run::CloseTab)),
        Some(command(tr(Key("palette.settings")), Run::Settings)),
    ];
    Group {
        label: tr(ACTIONS_LABEL).to_string(),
        entries: verbs
            .chain(stop)
            .chain(window.into_iter().flatten())
            .collect(),
    }
}

/// Every page but the one on screen.
fn pages(situation: &Situation) -> Group {
    let others = PAGES.iter().filter(|(page, _)| {
        *page != situation.page && (*page != Page::Scheduled || situation.has_scheduled)
    });
    let entries = others.map(|(page, name)| {
        let go = Ask::Go {
            page: page.clone(),
            is_new_tab: false,
        };
        command(tr(*name), Run::Ask(go))
    });
    Group {
        label: tr(GO_LABEL).to_string(),
        entries: entries.collect(),
    }
}

/// Every lane but the one on screen, by name.
fn lanes(situation: &Situation) -> Group {
    let shown = match &situation.page {
        Page::Lane(key) => Some(key.as_str()),
        _ => None,
    };
    let others = situation
        .lanes
        .iter()
        .filter(|lane| Some(lane.key.as_str()) != shown);
    let entries = others.map(|lane| {
        let go = Ask::Go {
            page: Page::Lane(lane.key.clone()),
            is_new_tab: false,
        };
        Entry {
            glyph: Some(lane.glyph),
            ..command(lane.name.clone(), Run::Ask(go))
        }
    });
    Group {
        label: tr(LANES_LABEL).to_string(),
        entries: entries.collect(),
    }
}

/// Every project, by name: it shows the Lanes page kept to that project's lanes.
fn projects(situation: &Situation) -> Group {
    let entries = situation.projects.iter().map(|project| {
        let open = Ask::OpenProject {
            folder: project.folder.clone(),
            is_new_tab: false,
        };
        command(project.name.clone(), Run::Ask(open))
    });
    Group {
        label: tr(PROJECTS_LABEL).to_string(),
        entries: entries.collect(),
    }
}

fn tracks(situation: &Situation) -> Group {
    let entries = situation.tracks.iter().map(|(id, name)| {
        let page = Page::Track(id.clone());
        let is_current = page == situation.page;
        Entry {
            is_current,
            ..command(
                name,
                Run::Ask(Ask::Go {
                    page,
                    is_new_tab: false,
                }),
            )
        }
    });
    Group {
        label: tr(Key("palette.group.tracks")).to_string(),
        entries: entries.collect(),
    }
}

fn command(label: impl Into<String>, run: Run) -> Entry {
    Entry {
        label: label.into(),
        run,
        is_current: false,
        glyph: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn situation(page: Page) -> Situation {
        Situation {
            has_scheduled: false,
            can_create_track: false,
            page,
            lanes: vec![
                LaneRef {
                    key: "lane-a".into(),
                    name: "Upload retries".into(),
                    glyph: StateGlyph::Running,
                },
                LaneRef {
                    key: "lane-b".into(),
                    name: "Phone page words".into(),
                    glyph: StateGlyph::Running,
                },
            ],
            projects: vec![ProjectRef {
                folder: "/work/atlas".into(),
                name: "atlas".into(),
            }],
            tracks: Vec::new(),
            stoppable: None,
            selected_ask: None,
            can_close_tab: false,
            panel_left: Pixels::ZERO,
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
        groups
            .iter()
            .find(|group| group.label == label)
            .unwrap_or_else(|| panic!("no group {label}"))
    }

    #[test]
    fn the_palette_lists_the_lanes_the_pages_the_actions_and_the_projects() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let listed = groups(&situation(Page::Inbox));
        let names: Vec<&str> = listed.iter().map(|group| group.label.as_str()).collect();
        assert_eq!(names, ["Lanes", "Go to", "Actions", "Projects"]);
        assert_eq!(
            labels(group(&listed, "Go to")),
            ["Master", "Lanes", "Engine"],
            "not where it is"
        );
        assert_eq!(
            labels(group(&listed, "Lanes")),
            ["Upload retries", "Phone page words"]
        );
        assert_eq!(labels(group(&listed, "Actions")), ["New lane", "Settings"]);
    }

    #[test]
    fn what_the_list_highlights_first_is_a_place_to_go_and_never_an_action_that_sends_something() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let mut busy = situation(Page::Inbox);
        busy.stoppable = Some("lane-a".into());
        busy.selected_ask = Some(SelectedAsk {
            wait: "w1".into(),
            verbs: vec![AskVerb {
                verb: "approve".into(),
                label: "Approve".into(),
            }],
        });
        for mut asked in [busy.clone(), situation(Page::Inbox)] {
            for lanes in [asked.lanes.clone(), Vec::new()] {
                asked.lanes = lanes;
                let listed = groups(&asked);
                let first = &listed[0].entries[0].run;
                assert!(
                    matches!(first, Run::Ask(Ask::Go { .. })),
                    "the first entry is {first:?}"
                );
            }
        }
    }

    #[test]
    fn a_lane_carries_the_mark_of_its_state() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let mut marked = situation(Page::Inbox);
        marked.lanes[0].glyph = StateGlyph::NeedsYou;
        marked.lanes[1].glyph = StateGlyph::Running;
        let listed = groups(&marked);
        let glyphs: Vec<_> = group(&listed, "Lanes")
            .entries
            .iter()
            .map(|entry| entry.glyph)
            .collect();
        assert_eq!(
            glyphs,
            [Some(StateGlyph::NeedsYou), Some(StateGlyph::Running)]
        );
    }

    #[test]
    fn a_lane_page_offers_stop_and_quick_open_and_leaves_its_own_lane_out_of_the_list() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let mut on_lane = situation(Page::Lane("lane-a".into()));
        on_lane.stoppable = Some("lane-a".into());
        let listed = groups(&on_lane);
        let actions = labels(group(&listed, "Actions"));
        assert_eq!(actions, ["Stop lane", "New lane", "Open file…", "Settings"]);
        assert_eq!(labels(group(&listed, "Lanes")), ["Phone page words"]);
    }

    #[test]
    fn the_verbs_of_the_selected_ask_come_first_and_answer_that_ask() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let mut inbox = situation(Page::Inbox);
        inbox.selected_ask = Some(SelectedAsk {
            wait: "w1".into(),
            verbs: vec![AskVerb {
                verb: "approve".into(),
                label: "Approve".into(),
            }],
        });
        let listed = groups(&inbox);
        let first = &group(&listed, "Actions").entries[0];
        assert_eq!(first.label, "Approve");
        assert_eq!(
            first.run,
            Run::Ask(Ask::Answer {
                wait: "w1".into(),
                verb: "approve".into()
            })
        );
        assert!(is_offered(&inbox, &first.run));
        assert!(
            !is_offered(&situation(Page::Inbox), &first.run),
            "that ask is no longer selected"
        );
    }

    #[test]
    fn choosing_a_page_in_a_new_tab_is_still_the_command_that_was_listed() {
        let engine = Run::Ask(Ask::Go {
            page: Page::Engine,
            is_new_tab: true,
        });
        assert!(is_offered(&situation(Page::Inbox), &engine));
        assert!(
            !is_offered(&situation(Page::Engine), &engine),
            "it is the page on screen"
        );
    }
}
