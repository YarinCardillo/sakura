//! A number or level the person sets on the Engine page (a stepper, the pace slider): it is shown the moment it
//! is set, never after the engine's answer. Presses that follow each other are one act with the last value,
//! sent after a short pause; a value set while the one before is on its way waits for its answer instead of
//! being lost. When the engine did it, the value stays shown until the engine has been read again, so the
//! control never jumps back; when the engine refused, it is dropped and the control shows the engine's own.

use super::engine_acts::EngineActs;
use gpui::{Context, Task as GpuiTask};
use sakura_core::engine::cli::Act;
use std::time::Duration;

/// A value the person requested, as the control shows it.
pub(super) struct Value {
    shown: String,
    stage: Stage,
}

/// Where a value stands on its way to the engine.
enum Stage {
    /// Waiting for another press; the timer sends it when none comes.
    Pausing { act: Act, _timer: GpuiTask<()> },
    /// Ready, but the press before it has not been answered.
    Waiting { act: Act },
    /// On its way to the engine.
    Sent,
    /// Done by the engine; shown until this many reads of the engine have ended.
    Settling { until_read: u64 },
}

impl EngineActs {
    /// The value the control kept under `key` shows now, when one was set and the engine has not been read
    /// again since it did it.
    pub fn shown(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|value| value.shown.as_str())
    }

    /// Shows `shown` at once and sends `act` as the control `key` after `pause` without another call (none
    /// for a control that is let go once, like a slider).
    pub fn set_value(
        &mut self,
        key: &str,
        act: Act,
        shown: impl Into<String>,
        pause: Duration,
        cx: &mut Context<Self>,
    ) {
        let shown = shown.into();
        if pause.is_zero() {
            let stage = Stage::Waiting { act };
            self.values.insert(key.to_string(), Value { shown, stage });
            self.flush(key, cx);
        } else {
            let timer = self.start_pause(key, pause, cx);
            let stage = Stage::Pausing { act, _timer: timer };
            self.values.insert(key.to_string(), Value { shown, stage });
        }
        cx.notify();
    }

    /// How many reads of the engine must have ended before one that is requested now has.
    pub(super) fn reads_to_wait_for(&self, cx: &Context<Self>) -> u64 {
        let facts = self.facts().read(cx);
        facts.reads_ended() + facts.reads_ahead()
    }

    /// The engine did the act of `key`: its value stays shown until the read at `until_read`, and a value
    /// that was set meanwhile goes now.
    pub(super) fn value_done(&mut self, key: &str, until_read: u64, cx: &mut Context<Self>) {
        match self.values.get_mut(key).map(|value| &mut value.stage) {
            Some(stage @ Stage::Sent) => *stage = Stage::Settling { until_read },
            Some(Stage::Waiting { .. }) => self.flush(key, cx),
            _ => {}
        }
    }

    /// The engine did not do the act of `key`: its value is dropped and the control shows the engine's own,
    /// unless another was set meanwhile, which goes now.
    pub(super) fn value_failed(&mut self, key: &str, cx: &mut Context<Self>) {
        match self.values.get(key).map(|value| &value.stage) {
            Some(Stage::Sent) => {
                self.values.remove(key);
            }
            Some(Stage::Waiting { .. }) => self.flush(key, cx),
            _ => {}
        }
    }

    /// The engine was read: the values it has taken up are no longer shown by us.
    pub(super) fn settle(&mut self, cx: &mut Context<Self>) {
        let ended = self.facts().read(cx).reads_ended();
        let before = self.values.len();
        self.values.retain(|_, value| match value.stage {
            Stage::Settling { until_read } => until_read > ended,
            _ => true,
        });
        if self.values.len() != before {
            cx.notify();
        }
    }

    fn start_pause(&self, key: &str, pause: Duration, cx: &mut Context<Self>) -> GpuiTask<()> {
        let key = key.to_string();
        cx.spawn(async move |acts, cx| {
            cx.background_executor().timer(pause).await;
            drop(acts.update(cx, |acts, cx| acts.flush(&key, cx)));
        })
    }

    /// Sends the value of `key` now, or when the press before it is answered.
    fn flush(&mut self, key: &str, cx: &mut Context<Self>) {
        let is_running = self.is_running(key);
        let Some(value) = self.values.get_mut(key) else {
            return;
        };
        let (Stage::Pausing { act, .. } | Stage::Waiting { act }) = &value.stage else {
            return;
        };
        let act = act.clone();
        if is_running {
            value.stage = Stage::Waiting { act };
            return;
        }
        value.stage = Stage::Sent;
        self.send(key.to_string(), act, String::new(), cx);
    }
}
