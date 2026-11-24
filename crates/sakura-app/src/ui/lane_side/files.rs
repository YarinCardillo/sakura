//! Files: the file the agent is on, scrolled to its line, read only, with a switch that follows the agent. It
//! follows until the person takes the view (a scroll, a press, another file picked), and the switch turns it on
//! again at the agent's last place. Everything is read from the lane's own folder; a place outside it is never
//! opened.

pub use crate::ui::files::Location;

use super::worktree::WorktreeSource;
use crate::data::source::Source;
use crate::l10n::{tr, Key};
use crate::theme::{self, Theme};
use crate::ui::chrome::text;
use crate::ui::files::{FileView, FileViewEvent, Follow, Source as FileSource};
use gpui::{
    div, AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, Window,
};
use gpui_component::switch::Switch;
use sakura_core::files::{is_role_file, relative_path};
use std::path::Path;
use std::rc::Rc;

/// The files of one lane.
pub struct LaneFiles {
    /// What the files are read from, and the view that shows one; none for a lane with no folder.
    place: Option<(Rc<dyn FileSource>, Entity<FileView>)>,
    follow: Follow,
    /// The agent's last place, so the same place told again is not shown again.
    last_place: Option<Location>,
    /// A place to open at the next draw, which has a window.
    to_open: Option<Location>,
    /// The file on screen is read again at the next draw.
    is_stale: bool,
    _subscription: Option<Subscription>,
}

impl LaneFiles {
    /// The files below `worktree`, when the lane has a folder this Mac can read.
    pub fn new(
        worktree: Option<String>,
        source: &Source,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let folder = worktree.filter(|folder| Path::new(folder).is_dir());
        let place = folder.map(|folder| {
            let reading: Rc<dyn FileSource> = Rc::new(WorktreeSource::new(
                Path::new(&folder),
                source.runtime().clone(),
            ));
            let view = cx.new(|cx| FileView::new(reading.clone(), window, cx).without_asking());
            (reading, view)
        });
        let subscription = place
            .as_ref()
            .map(|(_, view)| cx.subscribe(view, Self::view_changed));
        Self {
            place,
            follow: Follow::default(),
            last_place: None,
            to_open: None,
            is_stale: false,
            _subscription: subscription,
        }
    }

    /// What quick open reads from, when the lane has a folder.
    pub fn source(&self) -> Option<Rc<dyn FileSource>> {
        self.place.as_ref().map(|(source, _)| source.clone())
    }

    #[cfg(test)]
    pub fn open_path(&self, cx: &gpui::App) -> Option<String> {
        let (_, view) = self.place.as_ref()?;
        view.read(cx).path().map(str::to_string)
    }

    /// The agent is at `place`: shown at the next draw while following, kept for when following goes on.
    pub fn agent_at(&mut self, place: Option<Location>, cx: &mut Context<Self>) {
        let Some(place) = place.filter(|place| Some(place) != self.last_place.as_ref()) else {
            return;
        };
        self.last_place = Some(place.clone());
        if relative_path(&place.path).is_err() || is_role_file(&place.path) {
            return;
        }
        if let Some(shown) = self.follow.agent_at(place).cloned() {
            self.to_open = Some(shown);
            cx.notify();
        }
    }

    /// Opens the file the person picked: the view is theirs from here.
    pub fn open(&mut self, path: &str, cx: &mut Context<Self>) {
        self.follow.person_acted();
        self.to_open = Some(Location::new(path, None));
        cx.notify();
    }

    /// The facts changed: the file on screen is read again at the next draw.
    pub fn reread(&mut self, cx: &mut Context<Self>) {
        self.is_stale = true;
        cx.notify();
    }

    fn view_changed(&mut self, _: Entity<FileView>, event: &FileViewEvent, cx: &mut Context<Self>) {
        if matches!(event, FileViewEvent::PersonActed) && self.follow.person_acted() {
            cx.notify();
        }
    }

    /// The switch was turned: on goes to the agent's last place, off leaves the view where it is.
    fn follow_switched(&mut self, is_on: bool, cx: &mut Context<Self>) {
        if is_on {
            let (_, last) = self.follow.turn_on();
            self.to_open = last.cloned();
        } else {
            self.follow.person_acted();
        }
        cx.notify();
    }
}

impl Render for LaneFiles {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let Some((_, view)) = self.place.clone() else {
            let words = tr(Key("lane_side.files.no_folder"));
            return note("files-no-folder", words, &palette);
        };
        if let Some(place) = self.to_open.take() {
            let line = place.line;
            view.update(cx, |view, cx| view.open(&place.path, line, window, cx));
        }
        if std::mem::take(&mut self.is_stale) {
            view.update(cx, |view, cx| view.reread(window, cx));
        }
        let has_file = view.read(cx).path().is_some();
        let switch = div().debug_selector(|| "follow-switch".into()).child(
            Switch::new("follow").checked(self.follow.is_on()).on_click(
                cx.listener(|files, is_on: &bool, _, cx| files.follow_switched(*is_on, cx)),
            ),
        );
        let row = div()
            .h(crate::theme::lane_side::FOLLOW_HEIGHT)
            .mx(crate::theme::lane_side::BODY_PADDING_X)
            .flex()
            .items_center()
            .justify_between()
            .text_size(theme::TEXT_SMALL)
            .text_color(palette.text_secondary)
            .child(tr(Key("files.follow")))
            .child(switch);
        let shown: AnyElement = match has_file {
            true => div().flex_1().min_h_0().child(view).into_any_element(),
            false => text(theme::TEXT_SMALL, palette.text_tertiary)
                .debug_selector(|| "files-nothing".into())
                .mx(crate::theme::lane_side::BODY_PADDING_X)
                .child(tr(Key("lane_side.files.nothing")))
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(row)
            .child(shown)
            .into_any_element()
    }
}

fn note(
    selector: &'static str,
    words: &'static str,
    palette: &crate::theme::Palette,
) -> AnyElement {
    text(theme::TEXT_SMALL, palette.text_tertiary)
        .debug_selector(move || selector.into())
        .mx(crate::theme::lane_side::BODY_PADDING_X)
        .child(words)
        .into_any_element()
}
