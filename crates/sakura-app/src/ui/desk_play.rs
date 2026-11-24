//! Play in the desk: each open task has one [`Play`], made the first time its page is drawn once
//! the run command of its project is known. The header shows its control, the right pane its view,
//! the status bar an item while the task on screen plays. The service is asked where a Play
//! stands once, when it is made; after that only a press asks. Nothing is asked on a timer.

use super::chrome::{keep_click, text_width};
use super::desk::Desk;
use super::panes;
use super::play::{self, BoxPlay, Play, PlayEvent, PlaySource};
use crate::data::projects::Listed;
use crate::data::source::Source;
use crate::l10n::tr;
use crate::menus::{StartPlay, StopPlay};
use crate::theme;
use crate::ui::page;
use gpui::{
    div, AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement as _,
    ParentElement as _, Styled as _, Subscription, Window,
};
use sakura_core::links;
use sakura_core::models::{Place, Task};
use sakura_core::play::Availability;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Makes the source of a task's Play; none where there is no service to ask (the sample data).
pub type PlaySources = Rc<dyn Fn(&str) -> Option<Rc<dyn PlaySource>>>;

/// The Plays of the open tasks.
pub struct Plays {
    sources: PlaySources,
    of: HashMap<String, (Entity<Play>, [Subscription; 2])>,
    /// The tasks whose Play runs, as each Play last said.
    running: HashSet<String>,
    /// The tasks that can play: the header keeps the room of their port.
    able: HashSet<String>,
    /// Whether there is a service to ask for a Play: the row of views and the header offer one from
    /// the first frame, before the run command of any project is known.
    is_served: bool,
}

impl Plays {
    /// Over the service `source` talks to.
    pub fn over(source: &Source) -> Self {
        let (cli, runtime) = (source.cli().cloned(), source.runtime().clone());
        let is_served = cli.is_some();
        Self::with(
            Rc::new(move |task| {
                let play = BoxPlay::new(cli.clone()?, task, runtime.clone());
                Some(Rc::new(play) as Rc<dyn PlaySource>)
            }),
            is_served,
        )
    }

    /// Over the sources `sources` makes; `is_served` tells whether it makes one at all.
    pub fn with(sources: PlaySources, is_served: bool) -> Self {
        Self {
            sources,
            of: HashMap::new(),
            running: HashSet::new(),
            able: HashSet::new(),
            is_served,
        }
    }

    /// Whether tasks have a Play: the row of views and the header offer it from the first frame,
    /// whatever is still being read, so that nothing appears or moves when the answer comes.
    pub fn is_offered(&self) -> bool {
        self.is_served
    }

    /// Whether the Play of `task` runs now.
    pub fn is_running(&self, task: &str) -> bool {
        self.running.contains(task)
    }

    /// Whether the Play of `task` can start: its project names a run command and it is on the Mac.
    pub fn can_start(&self, task: &str) -> bool {
        self.able.contains(task)
    }

    /// Drops the Plays of the tasks that no longer exist.
    pub fn retain(&mut self, exists: impl Fn(&str) -> bool) {
        self.of.retain(|task, _| exists(task));
        self.running.retain(|task| exists(task));
        self.able.retain(|task| exists(task));
    }

    /// The Play of `task`, when it was made.
    pub fn of(&self, task: &str) -> Option<&Entity<Play>> {
        self.of.get(task).map(|(play, _)| play)
    }
}

impl Desk {
    /// The Play of `task`: made the first time it is requested after the projects were read. None
    /// on the sample data, and while the projects are being read.
    pub(super) fn play_of(
        &mut self,
        task: &Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Play>> {
        let availability = self.play_availability(task, cx)?;
        match availability {
            Availability::Ready => drop(self.plays.able.insert(task.name.clone())),
            _ => drop(self.plays.able.remove(&task.name)),
        }
        if let Some(play) = self.plays.of(&task.name).cloned() {
            play.update(cx, |play, cx| play.set_availability(availability, cx));
            return Some(play);
        }
        let source = (self.plays.sources)(&task.name)?;
        let play = cx.new(|cx| Play::new(source, availability, window, cx));
        let (name, project) = (task.name.clone(), task.profile.clone());
        let said = cx.subscribe(&play, move |desk, _, event: &PlayEvent, cx| {
            desk.play_said(&name, &project, event, cx);
        });
        // The header draws the control: it is drawn again whenever the Play changes (its word while
        // the service answers, the port when it runs).
        let changed = cx.observe(&play, |_, _, cx| cx.notify());
        let kept = (play.clone(), [said, changed]);
        self.plays.of.insert(task.name.clone(), kept);
        Some(play)
    }

    /// Whether `task` can play: in a box it cannot yet; on the Mac it can when its project names a
    /// run command. The projects are requested once when they were never read, and until they
    /// answer nothing is known. A task in a box asks for nothing.
    fn play_availability(&mut self, task: &Task, cx: &mut Context<Self>) -> Option<Availability> {
        if task.place == Place::Box {
            return Some(Availability::InBox);
        }
        let projects = self.projects.read(cx);
        if *projects.listed() == Listed::Unknown {
            self.projects
                .update(cx, |projects, cx| projects.load_once(cx));
            return None;
        }
        let run = projects
            .project(&task.profile)
            .and_then(|p| p.run.as_deref());
        Some(Availability::of(task.place, run))
    }

    /// The header's control of `task`'s Play, when tasks have one: while the Play is not made (the
    /// projects are being read) the same control stands there, disabled.
    pub(super) fn play_control(
        &mut self,
        task: &Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.plays.is_offered() {
            return None;
        }
        let control = div().id("header-play").flex_none();
        let control = match self.play_of(task, window, cx) {
            Some(play) => control.child(play::header_control(&play, cx)),
            None => control.child(play::header_placeholder(cx)),
        };
        Some(keep_click(control).into_any_element())
    }

    /// The "Play" view of the right pane: the task's Play, which reads its log when it comes on
    /// screen.
    pub(super) fn play_view(
        &mut self,
        task: &Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.play_of(task, window, cx) {
            Some(play) => {
                play.update(cx, |play, cx| play.shown(true, cx));
                play.into_any_element()
            }
            None => panes::empty(tr(page::READING_SENTENCE), cx),
        }
    }

    /// The "Play" view of `task` left the screen.
    pub(super) fn play_view_left(&mut self, task: &str, cx: &mut Context<Self>) {
        if let Some(play) = self.plays.of(task).cloned() {
            play.update(cx, |play, cx| play.shown(false, cx));
        }
    }

    /// The room the Play control of `task` takes in the header: its widest word, and the room of
    /// the port's link unless the task is in a box, which cannot play.
    pub(super) fn play_width(&self, task: &Task, window: &Window) -> f32 {
        if !self.plays.is_offered() {
            return 0.0;
        }
        let small =
            |label: &str| text_width(window, label, theme::TEXT_SMALL, theme::WEIGHT_MEDIUM);
        let gap = f32::from(theme::GAP_WIDE);
        let control =
            play::widest_word(|word| small(word)) + f32::from(theme::CONTROL_PADDING_X) * 2.0 + gap;
        let port = match task.place {
            Place::Box => 0.0,
            Place::Mac => small(play::PORT_ROOM) + gap,
        };
        control + port
    }

    /// Answers "Start Play" and "Stop Play" of the menu bar for the task on screen, wherever the
    /// keyboard is. A task that cannot play, or whose Play already stands there, is left alone.
    pub(super) fn handle_play_actions(window: &Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        let desk = cx.weak_entity();
        let app: &mut gpui::App = cx;
        let press = move |runs: bool, cx: &mut gpui::App| {
            let desk = desk.clone();
            // The window is being updated while its action dispatches: the press comes right after.
            cx.defer(move |cx| {
                drop(handle.update(cx, |_, _, cx| {
                    drop(desk.update(cx, |desk, cx| desk.press_play(runs, cx)));
                }));
            });
        };
        let start = press.clone();
        app.on_action(move |_: &StartPlay, cx| start(true, cx));
        app.on_action(move |_: &StopPlay, cx| press(false, cx));
    }

    /// Starts (`runs`) or stops the Play of the task on screen, when it does not stand there yet.
    fn press_play(&mut self, runs: bool, cx: &mut Context<Self>) {
        let shown = self.shown(cx);
        let Some(play) = shown.and_then(|task| self.plays.of(&task).cloned()) else {
            return;
        };
        play.update(cx, |play, cx| match (runs, play.is_running()) {
            (true, false) => play.start(cx),
            (false, true) => play.stop(cx),
            _ => {}
        });
    }

    /// A snapshot's script starts or stops the Play of the task on screen.
    #[cfg(feature = "snapshot")]
    pub fn drive_play(&mut self, verb: &str, cx: &mut Context<Self>) {
        self.press_play(verb == "start", cx);
    }

    /// A snapshot's script shows a view of the right pane of the task on screen.
    #[cfg(feature = "snapshot")]
    pub fn drive_pane(&mut self, view: &str, cx: &mut Context<Self>) {
        let shown = self.shown(cx).zip(super::right_pane::PaneView::named(view));
        match shown {
            Some((task, view)) => self.show_pane_view(&task, view, cx),
            None => eprintln!("sakura: no lavoro on screen, or no view named {view:?}"),
        }
    }

    fn play_said(&mut self, task: &str, project: &str, event: &PlayEvent, cx: &mut Context<Self>) {
        match event {
            PlayEvent::OpenUrl(address) => {
                if let Some(address) = links::web_url(address) {
                    cx.open_url(&address);
                }
            }
            PlayEvent::ProjectSettingsAsked => {
                let project = project.to_string();
                cx.defer(move |cx| crate::ui::settings::host::show_project(&project, cx));
            }
            PlayEvent::Changed => {
                let is_running = self
                    .plays
                    .of(task)
                    .is_some_and(|play| play.read(cx).is_running());
                match is_running {
                    true => drop(self.plays.running.insert(task.to_string())),
                    false => drop(self.plays.running.remove(task)),
                }
                cx.notify();
            }
        }
    }
}
