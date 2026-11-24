//! The second card: the pace. Where each account is meant to land at its reset (five levels on a slider), how
//! many lanes run at once, each account's use now with where it is forecast to land, and whether lanes give
//! way to the person's own sessions. The engine asks Leeway before every step; the app only sets the level.

use super::reset;
use super::EnginePage;
use crate::l10n::{tr, tr_with, Key};
use crate::theme::engine_page as sizes;
use crate::theme::{self, Palette};
use crate::ui::account_words::kind_line;
use crate::ui::engine_acts::{on, COALESCE_PAUSE, SET_PACE, SET_PRIORITY, SET_RULE};
use crate::ui::kit::stepper;
use crate::ui::kit::toggle;
use gpui::{
    div, relative, AnyElement, App, Context, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::slider::{Slider, SliderEvent, SliderState};
use sakura_core::engine::accounts::Account;
use std::rc::Rc;

/// The five levels the engine knows, slowest first, with the word of each and the line it puts at the
/// accounts' week (none at the fastest: only the reserve stops lanes).
pub(in crate::ui) const LEVELS: [(&str, Key, Option<u32>); 5] = [
    ("slowest", Key("pace.slowest"), Some(50)),
    ("slow", Key("pace.slow"), Some(70)),
    ("normal", Key("pace.normal"), Some(90)),
    ("fast", Key("pace.fast"), Some(100)),
    ("fastest", Key("pace.fastest"), None),
];
/// The engine's rules for the fewest lanes and the most that run at once.
pub(in crate::ui) const LANES_MIN_RULE: &str = "lanes_min";
pub(in crate::ui) const LANES_RULE: &str = "lanes";
/// The key the pace slider's press is kept under.
pub(super) const PACE_PRESS: &str = "engine-pace";
/// The keys the steppers of the fewest and the most lanes are kept under.
const LANES_MIN_PRESS: &str = "engine-lanes-min";
const LANES_MAX_PRESS: &str = "engine-lanes-max";
/// The most lanes the person can ask to run at once; the engine says its own range when it refuses.
const MAX_LANES: i64 = 16;
/// The fewest lanes the engine can be asked to run.
const MIN_LANES: i64 = 1;

/// How many frames the thumb's glide to its level takes: the layout duration, a frame at a time.
const GLIDE_FRAMES: u32 = 12;

/// The slider: five levels, from 0 (slowest) to 4 (fastest), on a continuous track: the thumb follows the
/// pointer between the levels and glides to the nearest one when let go.
pub(super) fn slider() -> SliderState {
    SliderState::new()
        .min(0.)
        .max((LEVELS.len() - 1) as f32)
        .step(0.)
        .default_value(2.)
}

/// The slider's position for the pace an engine reports; none for one this app has no word for.
pub(super) fn level_of(pace: Option<&str>) -> Option<usize> {
    LEVELS.iter().position(|(name, _, _)| Some(*name) == pace)
}

/// Where an account's week stands and where it is forecast to land under a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Landing {
    pub used: u32,
    pub end: u32,
    pub runs_out: bool,
}

/// The landing of an account that has used `used` percent and is forecast at `forecast`, under `target`: the
/// forecast is held at the line, never below what is used already; with no line it is the forecast itself.
pub(super) fn landing(used: u32, forecast: Option<u32>, target: Option<u32>) -> Landing {
    let natural = forecast.unwrap_or(used);
    let end = match target {
        Some(line) => natural.min(line).max(used),
        None => natural,
    };
    Landing {
        used,
        end,
        runs_out: end > 100,
    }
}

impl EnginePage {
    pub(super) fn pace_card(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = self.snapshot(cx);
        let engine = snapshot.and_then(|snapshot| snapshot.engine.clone());
        let accounts: Vec<Account> = snapshot
            .map(|snapshot| {
                snapshot
                    .accounts
                    .iter()
                    .filter(|account| {
                        !account.is_hidden() && account.forecast_available != Some(false)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let problem = snapshot.and_then(|snapshot| snapshot.problems.first().cloned());
        let as_of = snapshot
            .and_then(|snapshot| snapshot.usage.as_ref())
            .and_then(|usage| usage.as_of.as_deref())
            .and_then(reset::updated);
        let asked = self.acts.read(cx).shown(PACE_PRESS);
        let level = self.dragging.or_else(|| {
            level_of(asked.or_else(|| engine.as_ref().and_then(|e| e.pace.as_deref())))
        });
        let target = level.and_then(|level| LEVELS[level].2);
        let mut body = Vec::new();
        match (&engine, level) {
            (Some(engine), Some(level)) => {
                let left = self.pace_left(level, &accounts, palette, cx);
                let right = self.meters(&accounts, target, (problem, as_of), palette);
                body.push(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(sizes::PACE_GAP)
                        .child(left)
                        .child(right)
                        .into_any_element(),
                );
                if let (Some(from), Some(to)) = (engine.lanes_min, engine.lanes) {
                    body.push(self.lanes_at_once(from, to, palette, cx));
                }
                body.push(self.priority_row(engine.yield_to_person, palette, cx));
                body.push(self.lanes_table(palette, cx));
            }
            _ => body.push(self.unreported(
                Key("engine_page.pace_unreported"),
                "engine-pace-unreported",
                palette,
            )),
        }
        let card = self.card(
            Key("engine_page.pace.title"),
            Key("engine_page.pace.note"),
            None,
            body,
            palette,
        );
        div()
            .debug_selector(|| "engine-card-pace".into())
            .child(card)
            .into_any_element()
    }

    /// The big word, its sentence and the slider.
    fn pace_left(
        &self,
        level: usize,
        accounts: &[Account],
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (word, target) = (LEVELS[level].1, LEVELS[level].2);
        let sentence: SharedString = match target {
            None => tr(Key("engine_page.pace.no_line")).into(),
            Some(line) => {
                let key = if accounts.len() > 1 {
                    "engine_page.pace.lands"
                } else {
                    "engine_page.pace.lands_one"
                };
                tr_with(Key(key), &[("target", &line.to_string())])
            }
        };
        let slider = div()
            .debug_selector(|| "engine-pace-slider".into())
            .w(sizes::PACE_SLIDER_WIDTH)
            .max_w_full()
            .child(Slider::new(&self.pace).horizontal());
        let ends = div()
            .debug_selector(|| "engine-pace-ends".into())
            .w(sizes::PACE_SLIDER_WIDTH)
            .max_w_full()
            .flex()
            .justify_between()
            .text_size(sizes::SMALL_TEXT)
            .text_color(palette.text_tertiary)
            .child(tr(Key("pace.slow")))
            .child(tr(Key("pace.fast")));
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .flex_1()
            .min_w(sizes::PACE_LEFT_MIN_WIDTH)
            .child(
                div()
                    .debug_selector(|| "engine-pace-word".into())
                    .text_size(sizes::PACE_WORD_TEXT)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(tr(word)),
            )
            .child(
                div()
                    .debug_selector(|| "engine-pace-sentence".into())
                    .text_size(sizes::CARD_NOTE_TEXT)
                    .text_color(palette.text_tertiary)
                    .child(sentence),
            )
            .child(slider)
            .child(ends)
            .children(crate::ui::engine_acts::line(
                &self.acts, PACE_PRESS, palette, cx,
            ))
            .into_any_element()
    }

    /// "Lanes at once from N to M": the fewest and the most that run together, in a row of their own under
    /// the pace and the accounts.
    fn lanes_at_once(
        &self,
        from: u32,
        to: u32,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (requested_from, requested_to) = (
            self.shown_number(LANES_MIN_PRESS, from, cx),
            self.shown_number(LANES_MAX_PRESS, to, cx),
        );
        let send = |acts: gpui::Entity<crate::ui::engine_acts::EngineActs>,
                    rule: &'static str,
                    key: &'static str| {
            move |value: i64, _: &mut Window, cx: &mut App| {
                let act = SET_RULE.act(&[("rule", rule), ("value", &value.to_string())]);
                let shown = value.to_string();
                acts.update(cx, |acts, cx| {
                    acts.set_value(key, act, shown, COALESCE_PAUSE, cx)
                });
            }
        };
        let min_stepper = stepper(
            "engine-lanes-min",
            (i64::from(from), requested_from),
            Some(MIN_LANES),
            Some(requested_to),
            Rc::new(send(self.acts.clone(), LANES_MIN_RULE, LANES_MIN_PRESS)),
            palette,
        );
        let max_stepper = stepper(
            "engine-lanes-max",
            (i64::from(to), requested_to),
            Some(requested_from),
            Some(MAX_LANES),
            Rc::new(send(self.acts.clone(), LANES_RULE, LANES_MAX_PRESS)),
            palette,
        );
        let word = |key: Key| {
            div()
                .text_size(sizes::CARD_NOTE_TEXT)
                .text_color(palette.text_tertiary)
                .child(tr(key))
        };
        let controls = div()
            .debug_selector(|| "engine-lanes-controls".into())
            .flex_none()
            .flex()
            .items_center()
            .gap(theme::GAP)
            .child(word(Key("engine_page.pace.from")))
            .child(min_stepper)
            .child(word(Key("engine_page.pace.to")))
            .child(max_stepper);
        let told = [LANES_MIN_PRESS, LANES_MAX_PRESS]
            .map(|key| crate::ui::engine_acts::line(&self.acts, key, palette, cx));
        let pending = [LANES_MIN_PRESS, LANES_MAX_PRESS]
            .map(|key| super::pending_setting(&self.acts, key, palette, cx));
        div()
            .pt(theme::GAP)
            .border_t_1()
            .border_color(palette.border)
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme::GAP_WIDE)
                    .child(
                        div()
                            .debug_selector(|| "engine-lanes-title".into())
                            .flex_1()
                            .min_w_0()
                            .child(self.titled(
                                Key("engine_page.pace.lanes_at_once"),
                                Key("engine_page.pace.lanes_note"),
                                palette,
                            )),
                    )
                    .child(controls),
            )
            .children(pending.into_iter().chain(told).flatten())
            .into_any_element()
    }

    /// The number the control kept under `key` shows: the one just set, else the engine's `engine_value`.
    pub(super) fn shown_number(&self, key: &str, engine_value: u32, cx: &App) -> i64 {
        let asked = self
            .acts
            .read(cx)
            .shown(key)
            .and_then(|text| text.parse().ok());
        asked.unwrap_or(engine_value.into())
    }

    /// A row's name and the sentence under it.
    pub(super) fn titled(&self, name: Key, note: Key, palette: &Palette) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(theme::TAB_GAP)
            .child(div().text_size(sizes::ROW_TEXT).child(tr(name)))
            .child(
                div()
                    .text_size(sizes::SMALL_TEXT)
                    .text_color(palette.text_tertiary)
                    .child(tr(note)),
            )
            .into_any_element()
    }

    /// The accounts, each with its use now, where it lands and its reset; a sentence when there are none.
    fn meters(
        &self,
        accounts: &[Account],
        target: Option<u32>,
        (problem, as_of): (Option<String>, Option<SharedString>),
        palette: &Palette,
    ) -> AnyElement {
        if accounts.is_empty() {
            let mut unavailable = div()
                .debug_selector(|| "engine-usage-unavailable".into())
                .flex()
                .flex_col()
                .gap(theme::TAB_GAP)
                .text_size(theme::TEXT_SMALL)
                .text_color(palette.text_tertiary)
                .child(tr(Key("engine_page.usage_unavailable")));
            if let Some(problem) = problem {
                unavailable = unavailable.child(div().text_size(sizes::SMALL_TEXT).child(problem));
            }
            return unavailable.into_any_element();
        }
        let rows = accounts
            .iter()
            .map(|account| meter(account, target, palette));
        let list = div()
            .id("engine-meters")
            .max_h(
                sizes::METER_ROW_HEIGHT * sizes::METER_ROWS_SHOWN
                    + theme::GAP_SMALL * (sizes::METER_ROWS_SHOWN - 1.),
            )
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(theme::GAP_SMALL)
            .children(rows);
        let updated = as_of.map(|words| {
            div()
                .debug_selector(|| "engine-usage-updated".into())
                .text_size(sizes::SMALL_TEXT)
                .text_color(palette.text_tertiary)
                .child(words)
        });
        div()
            .w(sizes::METER_WIDTH)
            .max_w_full()
            .flex()
            .flex_col()
            .gap(theme::GAP_SMALL)
            .child(list)
            .children(updated)
            .into_any_element()
    }

    fn priority_row(&self, is_on: bool, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let acts = self.acts.clone();
        let switch = toggle("engine-priority", is_on, move |asked, _, cx| {
            let act = SET_PRIORITY.act(&[("on", on(asked))]);
            acts.update(cx, |acts, cx| acts.press("engine-priority", act, "", cx));
        });
        let told = crate::ui::engine_acts::line(&self.acts, "engine-priority", palette, cx);
        div()
            .pt(theme::GAP)
            .border_t_1()
            .border_color(palette.border)
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme::GAP_WIDE)
                    .child(self.titled(
                        Key("engine_page.pace.priority"),
                        Key("engine_page.pace.priority_note"),
                        palette,
                    ))
                    .child(switch),
            )
            .children(told)
            .into_any_element()
    }

    /// The engine refused the level the slider was let go at: the thumb goes back to the engine's own level,
    /// so it never shows a pace the heading does not.
    pub(super) fn resync_pace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let told = matches!(
            self.acts.read(cx).step(PACE_PRESS),
            Some(crate::ui::engine_acts::Step::Told(_))
        );
        let engine_level = self.snapshot(cx).and_then(|snapshot| {
            level_of(snapshot.engine.as_ref().and_then(|e| e.pace.as_deref()))
        });
        if let (true, None, Some(level)) = (told, self.dragging, engine_level) {
            self.glide_pace_to(level, window, cx);
        }
    }

    /// The thumb glides from where it stands to `level` over the layout duration with the shared ease out, and
    /// stands there; with reduced motion it is there at once. A new glide takes over from where the thumb is.
    fn glide_pace_to(&mut self, level: usize, window: &mut Window, cx: &mut Context<Self>) {
        let to = level as f32;
        let from = self.pace.read(cx).value().start();
        if cx.reduce_motion() || (from - to).abs() < f32::EPSILON {
            self.pace_glide = None;
            self.pace
                .update(cx, |slider, cx| slider.set_value(to, window, cx));
            return;
        }
        let slider = self.pace.clone();
        let frame = theme::DURATION_BASE / GLIDE_FRAMES;
        self.pace_glide = Some(cx.spawn_in(window, async move |page, cx| {
            for step in 1..=GLIDE_FRAMES {
                cx.background_executor().timer(frame).await;
                let share = crate::ui::slide::ease_out(step as f32 / GLIDE_FRAMES as f32);
                let at = from + (to - from) * share;
                let moved =
                    slider.update_in(cx, |slider, window, cx| slider.set_value(at, window, cx));
                if moved.is_err() {
                    return;
                }
            }
            page.update(cx, |page, _| page.pace_glide = None).ok();
        }));
    }

    /// The slider moved: while it is held the words follow it, and when it is let go the level is sent.
    pub(super) fn pace_moved(
        &mut self,
        event: &SliderEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SliderEvent::Change(value) => {
                self.dragging =
                    Some(value.start().round().clamp(0., (LEVELS.len() - 1) as f32) as usize);
                cx.notify();
            }
            SliderEvent::Release(value) => {
                let level = value.start().round().clamp(0., (LEVELS.len() - 1) as f32) as usize;
                self.dragging = None;
                self.glide_pace_to(level, window, cx);
                let act = SET_PACE.act(&[("level", LEVELS[level].0)]);
                let named = LEVELS[level].0;
                self.acts.update(cx, |acts, cx| {
                    acts.set_value(PACE_PRESS, act, named, std::time::Duration::ZERO, cx)
                });
                cx.notify();
            }
        }
    }
}

/// One account: its name and plan, its bar (what is used, the forecast as a lighter extension, the line as a
/// tick), where it lands and when it resets.
fn meter(account: &Account, target: Option<u32>, palette: &Palette) -> AnyElement {
    let used = account.week_pct.unwrap_or(0);
    let land = landing(used, account.forecast_at_reset_pct, target);
    let share = |percent: u32| relative(percent.min(100) as f32 / 100.);
    let selector = format!("engine-account-{}", account.id);
    let bar_selector = format!("{selector}-bar");
    let mut bar = div()
        .debug_selector(move || bar_selector.clone())
        .relative()
        .flex_1()
        .min_w(sizes::METER_BAR_MIN_WIDTH)
        .h(sizes::METER_BAR_HEIGHT)
        .rounded_full()
        .bg(palette.fill_hover)
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .h_full()
                .w(share(used))
                .rounded_full()
                .bg(palette.text_secondary),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left(share(used))
                .h_full()
                .w(share(land.end.saturating_sub(used)))
                .bg(palette.text_tertiary.opacity(sizes::METER_FORECAST_OPACITY)),
        );
    if let Some(line) = target {
        let tick = format!("{selector}-line");
        bar = bar.child(
            div()
                .debug_selector(move || tick.clone())
                .absolute()
                .top(-sizes::TICK_OVERSHOOT)
                .bottom(-sizes::TICK_OVERSHOOT)
                .left(share(line))
                .ml(-sizes::TICK_WIDTH / 2.)
                .w(sizes::TICK_WIDTH)
                .rounded(sizes::TICK_RADIUS)
                .bg(palette.text_secondary),
        );
    }
    let ends: SharedString = if land.runs_out {
        tr_with(
            Key("engine_page.meter_runs_out"),
            &[("used", &used.to_string())],
        )
    } else {
        tr_with(
            Key("engine_page.meter"),
            &[("used", &used.to_string()), ("end", &land.end.to_string())],
        )
    };
    let resets = account.week_resets_at.as_deref().and_then(reset::resets);
    let part = |name: &str| {
        let selector = format!("{selector}-{name}");
        move || selector.clone()
    };
    let name = div()
        .debug_selector(part("name"))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(palette.text_secondary)
        .font_weight(theme::WEIGHT_MEDIUM)
        .child(crate::ui::account_words::display_name(account));
    let kind = kind_line(account).map(|kind| {
        div()
            .debug_selector(part("plan"))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(sizes::SMALL_TEXT)
            .text_color(palette.text_tertiary)
            .child(kind)
    });
    let numbers = div()
        .w(sizes::METER_NUMBERS_WIDTH)
        .flex_none()
        .min_w_0()
        .flex()
        .flex_col()
        .font_features(crate::ui::chrome::tabular())
        .child(
            div()
                .debug_selector(part("ends"))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(palette.text_secondary)
                .child(ends),
        )
        .children(resets.map(|resets| {
            div()
                .debug_selector(part("resets"))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(sizes::SMALL_TEXT)
                .text_color(palette.text_tertiary)
                .child(resets)
        }));
    div()
        .debug_selector(move || selector.clone())
        .min_h(sizes::METER_ROW_HEIGHT)
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .text_size(sizes::CARD_NOTE_TEXT)
        .child(
            div()
                .w(sizes::METER_NAME_WIDTH)
                .flex_none()
                .min_w_0()
                .flex()
                .flex_col()
                .child(name)
                .children(kind),
        )
        .child(bar)
        .child(numbers)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_five_levels_have_the_lines_the_owner_chose() {
        let lines: Vec<Option<u32>> = LEVELS.iter().map(|(_, _, line)| *line).collect();
        assert_eq!(lines, [Some(50), Some(70), Some(90), Some(100), None]);
        assert_eq!(level_of(Some("fast")), Some(3));
        assert_eq!(level_of(Some("brisk")), None);
        assert_eq!(level_of(None), None);
    }

    #[test]
    fn a_forecast_over_the_line_is_held_at_the_line_and_never_below_what_is_used() {
        assert_eq!(
            landing(63, Some(93), Some(90)),
            Landing {
                used: 63,
                end: 90,
                runs_out: false
            }
        );
        assert_eq!(
            landing(95, Some(97), Some(90)),
            Landing {
                used: 95,
                end: 95,
                runs_out: false
            }
        );
        assert_eq!(landing(41, Some(86), Some(90)).end, 86);
    }

    #[test]
    fn with_no_line_the_forecast_stands_and_over_a_hundred_runs_out() {
        assert!(landing(70, Some(130), None).runs_out);
        assert_eq!(landing(70, None, None).end, 70, "no forecast: what is used");
        assert!(
            !landing(70, Some(130), Some(100)).runs_out,
            "the line holds it at a hundred"
        );
    }
}
