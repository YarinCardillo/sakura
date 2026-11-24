use super::{model, InboxEvent, InboxPage, Shown};
use gpui::{actions, App, Context, KeyBinding, Window};

/// The key context of the page's root.
pub const CONTEXT: &str = "inbox";
/// Where the bindings hold: in the page, but not while a field inside it is written in, which has the keys
/// for itself (the context of the kit's field is `Input`).
const KEYS_WHERE: &str = "inbox && !Input";

actions!(inbox_page, [NextAsk, PreviousAsk]);

/// Binds the keys of the page.
pub fn install_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("j", NextAsk, Some(KEYS_WHERE)),
        KeyBinding::new("down", NextAsk, Some(KEYS_WHERE)),
        KeyBinding::new("k", PreviousAsk, Some(KEYS_WHERE)),
        KeyBinding::new("up", PreviousAsk, Some(KEYS_WHERE)),
    ]);
}

impl InboxPage {
    pub(super) fn next(&mut self, _: &NextAsk, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    pub(super) fn previous(&mut self, _: &PreviousAsk, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }

    /// Held answer keys repeat at the OS level. Admit only the first press, without consuming typing.
    pub(super) fn answer_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stroke = &event.keystroke;
        let is_plain = !stroke.modifiers.modified();
        if !is_plain || event.is_held || !self.focus.is_focused(window) {
            return;
        }
        if self.filter.snoozed {
            if stroke.key == "enter" {
                self.open_selected_lane(cx);
                cx.stop_propagation();
            }
            return;
        }
        match stroke.key.as_str() {
            "enter" if self.shown == Shown::Done => self.open_selected_lane(cx),
            "a" | "enter" => {
                self.press_primary(window, cx);
                self.move_past_answered(cx);
            }
            "d" => {
                self.press_decline(window, cx);
                self.move_past_answered(cx);
            }
            "r" => self.press_reply(window, cx),
            "u" => self.toggle_read(cx),
            "h" => self.show_snooze(cx),
            "escape" if self.note.is_some() => self.leave_note(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Moves the selection `by` asks, staying inside the list.
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        let ids = self.ids(&snapshot, cx);
        let at = self
            .selected
            .as_ref()
            .and_then(|id| ids.iter().position(|other| other == id));
        let to = at.map_or(0, |at| {
            at.saturating_add_signed(by)
                .min(ids.len().saturating_sub(1))
        });
        if let Some(id) = ids.get(to).filter(|_| at != Some(to)) {
            self.open_row(id.clone(), cx);
            self.reveal_selected_row();
        }
    }

    /// Asks the desk to open the lane of the ask selected, in the Open list; the Done list opens it as well.
    pub(super) fn open_selected_lane(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        if let Some(lane) = self.selected_lane_key(&snapshot) {
            cx.emit(InboxEvent::OpenLane(lane));
        }
    }

    /// The key of the lane the selected ask is about.
    fn selected_lane_key(
        &self,
        snapshot: &sakura_core::engine::snapshot::Snapshot,
    ) -> Option<String> {
        let id = self.selected.as_deref()?;
        match self.shown {
            Shown::Open => snapshot
                .waits
                .iter()
                .find(|wait| wait.id == id)
                .map(|wait| wait.lane_key()),
            Shown::Done => {
                let mut decided = snapshot.decided.iter().flatten();
                let decided = decided.find(|decided| model::decided_key(decided) == id)?;
                model::decided_lane(decided, snapshot).map(|lane| lane.lane_key())
            }
        }
    }
}
