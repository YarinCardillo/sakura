//! The window comes back as it was. Its tabs, its panes and the lavoro shown are kept where they change;
//! here the window's place on the screen and the view of the right pane each lavoro had are kept in the
//! preferences as they change, and read back when a window is built: at launch and when the app is opened
//! again with no window. Nothing is kept on a timer.

use super::desk::Desk;
use super::right_pane::PaneView;
use crate::prefs::{Preferences, WindowPlace};
use gpui::{App, Context, Subscription, Window};
use std::collections::{BTreeMap, HashMap};

impl Desk {
    /// The view of the right pane each lavoro had, as the preferences kept them.
    pub(super) fn pane_views_from(preferences: &Preferences) -> HashMap<String, PaneView> {
        let kept = preferences.pane_views.iter();
        let named = |(task, view): (&String, &String)| Some((task.clone(), PaneView::named(view)?));
        kept.filter_map(named).collect()
    }

    /// Keeps the window's place when it is moved or resized, and each lavoro's view of the right pane
    /// when one changes.
    pub(super) fn keep_window(window: &mut Window, cx: &mut Context<Self>) -> [Subscription; 2] {
        let place = cx.observe_window_bounds(window, |_, window, cx| keep_place(window, cx));
        let views = cx.observe_self(|desk, cx| desk.keep_pane_views(cx));
        [place, views]
    }

    /// Writes the views of the right pane to the preferences when one is not the one kept last.
    fn keep_pane_views(&mut self, cx: &mut Context<Self>) {
        if self.pane_views == self.pane_views_kept || is_not_kept(cx) {
            return;
        }
        self.pane_views_kept = self.pane_views.clone();
        let named = |(task, view): (&String, &PaneView)| (task.clone(), view.id().to_string());
        let views: BTreeMap<String, String> = self.pane_views.iter().map(named).collect();
        Preferences::change(cx, |preferences| preferences.pane_views = views);
    }
}

/// A snapshot's window is the size and the view its script requested: none of it is the person's, and
/// before the preferences are the window's there is nowhere to keep anything.
fn is_not_kept(cx: &App) -> bool {
    crate::snapshot::is_asked() || Preferences::try_current(cx).is_none()
}

/// Keeps where the window stands and how large it is; for a window that fills the screen, the place it
/// goes back to.
fn keep_place(window: &Window, cx: &mut App) {
    if is_not_kept(cx) {
        return;
    }
    let bounds = window.window_bounds().get_bounds();
    let place = WindowPlace {
        x: bounds.origin.x.into(),
        y: bounds.origin.y.into(),
        width: bounds.size.width.into(),
        height: bounds.size.height.into(),
    };
    Preferences::change(cx, |preferences| preferences.window = Some(place));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::source::{Recorder, Source};
    use crate::ui::interaction_tests::{click, draw, open_remembering, pane_view, wait_for};
    use gpui::TestAppContext;
    use std::sync::Arc;

    const FIRST: &str = "sample-accent";
    const SECOND: &str = "headings-26";

    fn remembering(views: &[(&str, &str)]) -> Preferences {
        let named = |(task, view): &(&str, &str)| (task.to_string(), view.to_string());
        Preferences {
            last_task: Some(FIRST.to_string()),
            pane_views: views.iter().map(named).collect(),
            ..Default::default()
        }
    }

    #[gpui::test]
    fn each_lavoro_comes_back_on_the_view_of_the_right_pane_it_had(cx: &mut TestAppContext) {
        let kept = remembering(&[(FIRST, "git"), (SECOND, "files"), ("gone", "no-such-view")]);
        let start = |runtime| Source::for_test(runtime, Arc::new(Recorder::default()));
        let (cx, desk) = open_remembering(cx, start, None, kept);
        assert_eq!(pane_view(cx, &desk, FIRST), PaneView::Git);
        assert_eq!(pane_view(cx, &desk, SECOND), PaneView::Files);
        assert_eq!(pane_view(cx, &desk, "gone"), PaneView::default());
    }

    #[gpui::test]
    fn a_view_of_the_right_pane_is_kept_when_it_is_chosen(cx: &mut TestAppContext) {
        let start = |runtime| Source::for_test(runtime, Arc::new(Recorder::default()));
        let (cx, desk) = open_remembering(cx, start, None, remembering(&[]));
        wait_for(cx, "the rows", |cx| {
            cx.debug_bounds("row-sample-accent").is_some()
        });
        click(cx, "row-sample-accent");
        desk.update(cx, |desk, cx| {
            desk.show_pane_view(FIRST, PaneView::Checks, cx)
        });
        draw(cx);
        let kept = cx.update(|_, cx| Preferences::current(cx).pane_views);
        assert_eq!(kept.get(FIRST).map(String::as_str), Some("checks"));
    }
}
