//! The first card: how a lane runs. The preset bar, the pipeline drawn and the rules of the step chosen.

use super::schema::{self, Pipeline};
use super::EnginePage;
use crate::l10n::{tr, tr_with, Key};
use crate::theme::engine_page as sizes;
use crate::theme::{self, Palette};
use crate::ui::engine_acts::{SAVE_PRESET, USE_PRESET};
use crate::ui::kit::segments;
use crate::ui::settings_modal::controls;
use gpui::{
    div, AnyElement, App, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, Styled as _, Window,
};
use sakura_core::engine::rules::Rules;
use std::rc::Rc;

/// The key the preset choice's press is kept under, and the save's.
const PRESET_PRESS: &str = "engine-preset";
pub(super) const SAVE_PRESS: &str = "engine-preset-save";

impl EnginePage {
    /// Marks the preset the engine reports in its rules.
    pub(super) fn marked_presets(&self, cx: &App) -> Vec<bool> {
        let snapshot = self.snapshot(cx);
        let engine = snapshot.and_then(|snapshot| snapshot.engine.as_ref());
        let offered = engine.map_or(&[][..], |engine| &engine.presets[..]);
        let in_force = self.rules.as_ref().map(|rules| rules.preset.as_str());
        offered
            .iter()
            .map(|preset| match in_force {
                Some(name) => preset.name == name,
                None => preset.chosen,
            })
            .collect()
    }

    /// Requests a preset change and keeps the confirmed rules visible.
    pub(super) fn choose_preset(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.acts.read(cx).is_running(PRESET_PRESS) {
            return;
        }
        self.asked_preset = Some(name.to_string());
        let act = USE_PRESET.act(&[("preset", name)]);
        self.acts
            .update(cx, |acts, cx| acts.press(PRESET_PRESS, act, "", cx));
        cx.notify();
    }

    /// What was shown ahead of the engine goes: the engine answered, or refused and the page goes back.
    pub(super) fn drop_asked_preset(&mut self) {
        self.asked_preset = None;
    }

    /// The engine refused the preset: the page goes back to what it had, with the engine's words.
    pub(super) fn resync_preset(&mut self, cx: &App) {
        let was_told = matches!(
            self.acts.read(cx).step(PRESET_PRESS),
            Some(crate::ui::engine_acts::Step::Told(_))
        );
        if was_told {
            self.drop_asked_preset();
        }
    }

    /// Counts the nodes that were switched on or off since the last drawing, so that they fade to their new look.
    fn note_flips(&mut self, pipeline: &Pipeline) {
        for node in schema::Node::ALL {
            let is_on = pipeline.is_on(node);
            if let Some(was_on) = self.drawn_on.insert(node, is_on) {
                if was_on != is_on {
                    *self.node_flips.entry(node).or_default() += 1;
                }
            }
        }
    }

    /// The card "How a lane runs".
    pub(super) fn runs_card(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let has_engine = self
            .snapshot(cx)
            .is_some_and(|snapshot| snapshot.engine.is_some());
        let rules = if has_engine { self.rules.clone() } else { None };
        let pipeline = Pipeline::from_rules(rules.as_ref());
        let on_select = rules.as_ref().map(|_| {
            let page = cx.entity().downgrade();
            let on_select: schema::OnSelect = Rc::new(move |node, _: &mut Window, cx: &mut App| {
                drop(page.update(cx, |page, cx| page.select(node, cx)));
            });
            on_select
        });
        self.note_flips(&pipeline);
        let drawn = schema::drawing(
            &pipeline,
            (self.selected, &self.node_flips),
            on_select,
            palette,
        );
        let beside = self.inspector(rules.as_ref(), &pipeline, palette, cx);
        let schema = div()
            .flex()
            .flex_wrap()
            .gap(sizes::INSPECTOR_GAP)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::GAP_WIDE)
                    .child(drawn)
                    .child(self.legend(palette)),
            )
            .child(beside);
        let mut body = Vec::new();
        if let Some(rules) = &rules {
            body.push(self.preset_bar(rules, palette, cx));
        } else {
            body.push(self.unreported(
                Key("engine_page.rules_unreported"),
                "engine-rules-unreported",
                palette,
            ));
        }
        body.push(schema.into_any_element());
        self.card(
            Key("engine_page.runs.title"),
            Key("engine_page.runs.note"),
            None,
            body,
            palette,
        )
    }

    fn legend(&self, palette: &Palette) -> AnyElement {
        div()
            .max_w(sizes::SCHEMA_WIDTH)
            .text_size(sizes::SMALL_TEXT)
            .text_color(palette.text_tertiary)
            .child(tr(Key("engine_page.legend")))
            .into_any_element()
    }

    /// The presets as a choice, and what to do once a rule was changed: it says "Edited" and offers to save.
    fn preset_bar(
        &mut self,
        rules: &Rules,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let offered = self
            .snapshot(cx)
            .and_then(|snapshot| snapshot.engine.as_ref());
        let offered = offered.map_or_else(Vec::new, |engine| engine.presets.clone());
        let options: Vec<(SharedString, bool)> = offered
            .iter()
            .zip(self.marked_presets(cx))
            .map(|(preset, is_marked)| (preset.words().into(), is_marked))
            .collect();
        let names: Vec<String> = offered.iter().map(|preset| preset.name.clone()).collect();
        let page = cx.entity().downgrade();
        let track = segments("engine-preset", &options, move |nth, _, cx| {
            drop(page.update(cx, |page, cx| page.choose_preset(&names[nth], cx)));
        });
        let label = div()
            .text_size(theme::TEXT_LABEL)
            .text_color(palette.text_tertiary)
            .child(tr(Key("engine_page.preset")));
        let mut bar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme::GAP_WIDE)
            .child(label)
            .child(track);
        if rules.edited {
            bar = bar.child(self.edited_part(rules, palette, cx));
        }
        let told = crate::ui::engine_acts::line(&self.acts, PRESET_PRESS, palette, cx);
        let pending = self.asked_preset.as_ref().map(|_| {
            crate::ui::chrome::text(theme::TEXT_LABEL, palette.text_secondary)
                .debug_selector(|| "engine-preset-pending".into())
                .child(tr(Key("thread.status.pending")))
        });
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(bar)
            .children(pending)
            .children(told)
            .into_any_element()
    }

    /// "Standard, edited" and the way to keep the rules as a preset of their own.
    fn edited_part(
        &mut self,
        rules: &Rules,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let words = tr_with(
            Key("engine_page.edited"),
            &[("preset", &rules.preset_name())],
        );
        let note = div()
            .debug_selector(|| "engine-edited".into())
            .text_size(theme::TEXT_LABEL)
            .text_color(palette.text_tertiary)
            .child(words);
        let page = cx.entity().downgrade();
        let ask = Rc::new(move |_: &mut Window, cx: &mut App| {
            drop(page.update(cx, |page, cx| {
                page.is_saving = true;
                cx.notify();
            }));
        });
        let save = crate::ui::settings_modal::group_button(
            "engine-save-preset",
            tr(Key("engine_page.save_preset")),
            ask,
            cx,
        );
        let preset = rules.preset.clone();
        let page = cx.entity().downgrade();
        let back = Rc::new(move |_: &mut Window, cx: &mut App| {
            drop(page.update(cx, |page, cx| page.choose_preset(&preset, cx)));
        });
        let back = crate::ui::settings_modal::group_button(
            "engine-back-to-preset",
            tr_with(
                Key("engine_page.back_to_preset"),
                &[("preset", &rules.preset_name())],
            ),
            back,
            cx,
        );
        let line = div()
            .flex()
            .items_center()
            .gap(theme::GAP_WIDE)
            .child(note)
            .children((!self.is_saving).then_some(save))
            .children((!self.is_saving).then_some(back));
        let mut part = div().flex().flex_col().gap(theme::GAP_WIDE).child(line);
        if self.is_saving {
            part = part.child(self.save_form(palette, cx));
        }
        part.into_any_element()
    }

    /// The form that keeps the rules as a preset: its field named and showing an example, the one button
    /// named for its action, Cancel, and what the engine refused.
    fn save_form(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let field = controls::labeled_field(
            "engine-save-preset-name",
            tr(Key("engine_page.preset_name_label")),
            &self.preset_name,
            sizes::SAVE_FIELD_WIDTH,
            palette,
        );
        let (acts, name) = (self.acts.clone(), self.preset_name.clone());
        let send = Rc::new(move |_: &mut Window, cx: &mut App| {
            let written = name.read(cx).value().trim().to_string();
            let act = SAVE_PRESET.act(&[("name", &written)]);
            acts.update(cx, |acts, cx| acts.press(SAVE_PRESS, act, "", cx));
        });
        let save = controls::primary_button(
            "engine-save-preset-send",
            tr(Key("engine_page.save_preset_send")),
            send,
            cx,
        );
        let page = cx.entity().downgrade();
        let leave = Rc::new(move |_: &mut Window, cx: &mut App| {
            drop(page.update(cx, |page, cx| {
                page.is_saving = false;
                cx.notify();
            }));
        });
        let cancel = crate::ui::settings_modal::group_button(
            "engine-save-preset-cancel",
            tr(Key("common.cancel")),
            leave,
            cx,
        );
        let told = crate::ui::engine_acts::line(&self.acts, SAVE_PRESS, palette, cx);
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_WIDE)
            .child(field)
            .children(told)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::GAP)
                    .child(save)
                    .child(cancel),
            )
            .into_any_element()
    }
}
