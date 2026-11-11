//! The thin platform layer of notifications and the Dock badge. What to tell and what to count is
//! decided by `sakura_core::notify` (never twice for the same fact, nothing while Sakura is in front);
//! here it is shown: a notification through GPUI's own system notifications (on the Mac the user
//! notification centre, which asks the person's permission the first time one is posted and
//! delivers nothing after a refusal, the Mac itself remembering the answer), and the count on the
//! Dock icon.
//!
//! The app owns one [`Notifications`] and calls [`Notifications::observe`] whenever the tasks
//! change or Sakura comes to the front or leaves it; nothing here runs on its own.

mod dock;

pub use dock::{system as dock_badge, Badge};

use crate::l10n::{tr, Key};
use gpui::{App, SharedString, SystemNotification};
use sakura_core::notify::{badge_label, Lavoro, Note, NoteKind, Notifier};
use std::rc::Rc;

/// What a notification's tag starts with; the task's name follows. A newer notification of the
/// same task takes the place of the older one.
const TAG: &str = "sakura.task.";

/// Which news the person wants to be told by the system. The badge counts whatever is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wanted {
    /// A lavoro that starts to need the person.
    pub needs_person: bool,
    /// An agent that finished.
    pub finished: bool,
}

/// The notifications and the badge of the window's tasks.
pub struct Notifications {
    notifier: Notifier,
    badge: Rc<dyn Badge>,
    /// What the badge shows now: it is written only when it changes.
    shown: Option<String>,
    wanted: Wanted,
}

impl Notifications {
    /// Over `badge`: the Dock's ([`dock_badge`]), or a test's own.
    pub fn new(badge: Rc<dyn Badge>) -> Self {
        Self {
            notifier: Notifier::default(),
            badge,
            shown: None,
            wanted: Wanted {
                needs_person: true,
                finished: true,
            },
        }
    }

    /// Which news are posted from now on (the person's choice in Settings).
    pub fn want(&mut self, wanted: Wanted) {
        self.wanted = wanted;
    }

    /// What the badge shows now.
    #[cfg(test)]
    pub fn badge(&self) -> Option<&str> {
        self.shown.as_deref()
    }

    /// Takes in the tasks as they are now and whether Sakura is in front: posts what is news and
    /// writes the badge when its count changed.
    pub fn observe(&mut self, lavori: &[Lavoro], is_in_front: bool, cx: &App) {
        let decision = self.notifier.observe(lavori, is_in_front);
        let is_wanted = |note: &&Note| match note.kind {
            NoteKind::NeedsPerson => self.wanted.needs_person,
            NoteKind::Finished => self.wanted.finished,
        };
        for note in decision.notes.iter().filter(is_wanted) {
            cx.show_system_notification(notification(note));
        }
        let label = badge_label(decision.badge);
        if label != self.shown {
            self.badge.set(label.as_deref());
            self.shown = label;
        }
    }
}

/// The notification of `note`: the task's name, and what happened in the interface's language.
fn notification(note: &Note) -> SystemNotification {
    let body = match note.kind {
        NoteKind::NeedsPerson => Key("notify.needs_person"),
        NoteKind::Finished => Key("notify.finished"),
    };
    SystemNotification {
        tag: SharedString::from(format!("{TAG}{}", note.task)),
        title: note.task.clone().into(),
        body: tr(body).into(),
        actions: Vec::new(),
    }
}

/// The task a notification with this tag is about: what the app opens when the person presses
/// the notification (`App::on_system_notification_response`).
pub fn task_of(tag: &str) -> Option<&str> {
    tag.strip_prefix(TAG).filter(|task| !task.is_empty())
}

#[cfg(test)]
mod tests;
