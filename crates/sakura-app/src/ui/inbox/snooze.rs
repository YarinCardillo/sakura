use super::{model, InboxPage};
use crate::ui::answers::{lane_key, Step};
use chrono::{DateTime, Days, Local, TimeDelta, TimeZone, Utc};
use gpui::{App, Context};
use sakura_core::engine::snapshot::{Snapshot, Wait};

pub(super) const UNSNOOZE: &str = "unsnooze";

pub(super) fn is_engine_snooze(wait: &Wait) -> bool {
    wait.snoozed_until.is_some() && wait.actions.iter().any(|action| action.key == UNSNOOZE)
}

pub(super) struct Snooze {
    pub wait: Wait,
    pub is_pending: bool,
}

impl InboxPage {
    pub(super) fn settle_snoozes(&mut self, cx: &App) {
        let answers = self.answers.read(cx);
        self.snoozes.retain(|id, snooze| {
            if !snooze.is_pending {
                return true;
            }
            match answers.step(&lane_key(id, "attention")) {
                Some(Step::Running) => true,
                Some(Step::Told {
                    is_problem: false, ..
                }) => {
                    snooze.is_pending = false;
                    true
                }
                _ => false,
            }
        });
    }

    pub(super) fn reconcile_snoozes(&mut self, cx: &App) {
        if let Some(snapshot) = self.facts.read(cx).snapshot() {
            for wait in snapshot
                .waits
                .iter()
                .filter(|wait| wait.is_snoozed.is_some() || is_engine_snooze(wait))
            {
                self.snoozes.remove(&wait.id);
            }
        }
    }

    pub(super) fn expire_snoozes(&mut self, cx: &mut Context<Self>) {
        let now = super::list::now();
        let before = self.snoozes.len();
        self.snoozes
            .retain(|_, snooze| snooze.wait.is_snoozed_at(now));
        if self.snoozes.len() != before {
            cx.notify();
        }
    }

    pub(super) fn kept_snooze<'a>(&'a self, wait: &Wait) -> Option<&'a Wait> {
        let kept = self.snoozes.get(&wait.id).filter(|item| !item.is_pending)?;
        if wait.is_snoozed.is_some() {
            return None;
        }
        (kept.wait.since == wait.since && kept.wait.key == wait.key).then_some(&kept.wait)
    }

    // Older snapshots omit snoozed waits. Keep the last copy until its acknowledged wake time.
    pub(super) fn merge_snoozes(&self, snapshot: &mut Snapshot) {
        for wait in &mut snapshot.waits {
            if wait.snoozed_until.is_none() {
                if let Some(kept) = self.kept_snooze(wait) {
                    wait.snoozed_until.clone_from(&kept.snoozed_until);
                }
            }
        }
        if !self.filter.snoozed {
            return;
        }
        for (id, snooze) in self.snoozes.iter().filter(|(_, item)| !item.is_pending) {
            let is_listed = snapshot.waits.iter().any(|wait| wait.id == *id);
            let is_answered = snapshot.decided.iter().flatten().any(|answer| {
                answer.id == *id && model::decided_at(answer) >= model::raised(&snooze.wait)
            });
            if !is_listed && !is_answered {
                snapshot.waits.push(snooze.wait.clone());
            }
        }
    }
}

pub(super) fn deadline(preset: &str, now: DateTime<Local>) -> Option<String> {
    let wake = match preset {
        "1h" => now + TimeDelta::hours(1),
        "tomorrow" | "nextweek" => {
            let days = if preset == "tomorrow" { 1 } else { 7 };
            let date = now.date_naive().checked_add_days(Days::new(days))?;
            Local
                .from_local_datetime(&date.and_hms_opt(9, 0, 0)?)
                .earliest()?
        }
        _ => return None,
    };
    Some(wake.with_timezone(&Utc).to_rfc3339())
}
