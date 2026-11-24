//! The boxes made ready before what needs one: the sign-in of an account of the boxes, a new task in a
//! box, a check of a task in a box. The service is asked whether a box can be used (`box up --check`,
//! which starts nothing); when one cannot, `box up` runs where a sign-in shows, under one plain line, and
//! what was asked goes on once it ended well. A failure stays there with its last lines and "Try again";
//! Docker that does not answer is said in one line with what to do (`console::rules`).

use super::centre::CentreView;
use super::console::rules::ConsoleKind;
use super::console::{At, BoxesReady};
use super::desk::Desk;
use gpui::{Context, Window};
use sakura_core::error::BridgeResult;
use sakura_core::models::BoxReadiness;
use std::collections::HashMap;

/// What goes on once a box can be used.
pub type AfterBoxes = Box<dyn FnOnce(&mut Desk, &mut Window, &mut Context<Desk>)>;

/// What waits for `box up`, by the place it runs in: one thing per place, the newest. A check that
/// waits is also known by its task and verb, so its row reads "Queued" meanwhile.
#[derive(Default)]
pub struct BoxWaits {
    waits: HashMap<At, AfterBoxes>,
    checks: HashMap<(String, String), At>,
}

impl BoxWaits {
    /// Whether the check `verb` of `task` waits for the boxes.
    pub fn has_check(&self, task: &str, verb: &str) -> bool {
        self.checks
            .contains_key(&(task.to_string(), verb.to_string()))
    }
}

impl Desk {
    /// Runs `then` once a box can be used: at once when one can, else after `box up` ended well where a
    /// sign-in asked now would show. A service that cannot say lets `then` go on, which then says why
    /// it cannot.
    pub(super) fn with_boxes(
        &mut self,
        then: AfterBoxes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = self.boxes_place(cx);
        let asked = self.source.boxes();
        cx.spawn_in(window, async move |desk, cx| {
            let answer = asked.await;
            drop(desk.update_in(cx, |desk, window, cx| {
                desk.boxes_answered(at, answer, then, window, cx)
            }));
        })
        .detach();
    }

    /// Queues the check `verb` of `task`, a task in a box, once the boxes are ready; it reads
    /// "Queued" from now on.
    pub(super) fn check_with_boxes(
        &mut self,
        task: String,
        verb: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let check = (task, verb);
        let at = self.boxes_place(cx);
        self.box_waits.checks.insert(check.clone(), at);
        let then = move |desk: &mut Desk, _: &mut Window, cx: &mut Context<Desk>| {
            desk.box_waits.checks.remove(&check);
            desk.queue_check(check.0, check.1, cx);
        };
        self.with_boxes(Box::new(then), window, cx);
        cx.notify();
    }

    /// "Cancel" over `box up`: it is hung up, and what waited for it is let go.
    pub fn cancel_boxes(&mut self, at: &At, cx: &mut Context<Self>) {
        self.box_waits.waits.remove(at);
        self.box_waits.checks.retain(|_, waits_at| waits_at != at);
        self.consoles
            .update(cx, |consoles, cx| consoles.close(at, cx));
    }

    /// `box up` ended well: what waited where it ran goes on.
    pub(super) fn boxes_ready(
        &mut self,
        ready: &BoxesReady,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(then) = self.box_waits.waits.remove(&ready.at) {
            then(self, window, cx);
        }
    }

    fn boxes_answered(
        &mut self,
        at: At,
        answer: BridgeResult<BoxReadiness>,
        then: AfterBoxes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match answer {
            Ok(boxes) if boxes.ready => then(self, window, cx),
            Ok(_) => self.make_boxes(at, then, window, cx),
            Err(error) => {
                eprintln!("sakura: whether a box can be used could not be read: {error}");
                then(self, window, cx);
            }
        }
    }

    /// Runs `box up` at `at`, and keeps `then` until it ended well.
    fn make_boxes(
        &mut self,
        at: At,
        then: AfterBoxes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.box_waits.waits.insert(at.clone(), then);
        if let At::Task(task, _) = &at {
            self.show_centre_view(task, CentreView::Agent, cx);
        }
        self.consoles.update(cx, |consoles, cx| {
            consoles.open(at, ConsoleKind::BoxUp, window, cx)
        });
    }

    /// Where `box up` shows: where a sign-in asked now would, in the task on screen or in the
    /// window's own centre.
    fn boxes_place(&self, cx: &Context<Self>) -> At {
        match self.sign_in_lavoro(cx) {
            Some(task) => At::task(&task, &ConsoleKind::BoxUp),
            None => At::Window,
        }
    }
}
