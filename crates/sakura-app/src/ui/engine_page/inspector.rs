//! The rules of the step chosen on the drawing, beside it: each drawn by its type (a switch, a stepper, a
//! choice) and sent to the engine as it is changed. A rule the engine marks read only shows its value.

use super::schema::{Node, Pipeline};
use super::words::rule_value_words;
use super::EnginePage;
use crate::l10n::{tr, Key};
use crate::theme::engine_page as sizes;
use crate::theme::{self, Palette};
use crate::ui::engine_acts::{self, flag, EngineActs, COALESCE_PAUSE, SET_RULE};
use crate::ui::kit::{segments, stepper, toggle, OnNumber};
use crate::ui::settings_modal::controls;
use gpui::{
    div, AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, Styled as _,
};
use sakura_core::engine::rules::{Rule, Rules};
use std::rc::Rc;

/// Choices that fit as words on the page; more are a menu.
const SEGMENTS_AT_MOST: usize = 2;
/// What a change of a whole number is sent as.
/// What a change of a rule's value, written out, sends.
type SendValue = Rc<dyn Fn(&str, &mut App)>;

impl EnginePage {
    /// The rules beside the drawing: the step's name and what it does, then its rules.
    pub(super) fn inspector(
        &self,
        rules: Option<&Rules>,
        pipeline: &Pipeline,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let node = self.selected;
        let rows = match rules {
            Some(rules) => self.rule_rows(rules, node, pipeline, palette, cx),
            None => Vec::new(),
        };
        div()
            .debug_selector(|| "engine-inspector".into())
            .flex_1()
            .min_w(sizes::INSPECTOR_MIN_WIDTH)
            .flex()
            .flex_col()
            .gap(theme::TAB_GAP)
            .child(
                div()
                    .text_size(theme::TEXT_SMALL)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(tr(node.label())),
            )
            .child(
                div()
                    .pb(theme::GAP_SMALL)
                    .text_size(sizes::CARD_NOTE_TEXT)
                    .text_color(palette.text_tertiary)
                    .child(tr(node.note())),
            )
            .children(rows)
            .into_any_element()
    }

    fn rule_rows(
        &self,
        rules: &Rules,
        node: Node,
        pipeline: &Pipeline,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(found) = rules.node(node.id()) else {
            return Vec::new();
        };
        if node == Node::PlanReview && !pipeline.is_on(Node::Plan) {
            let needs = div()
                .text_color(palette.text_tertiary)
                .child(tr(Key("engine_page.needs_plan")));
            return vec![rule_line(
                "engine-rule-needs-plan",
                tr(Key("engine_page.on")).into(),
                needs.into_any_element(),
                palette,
            )];
        }
        let is_off = node.on_rule().is_some_and(|_| !pipeline.is_on(node));
        // A step whose model rule says which agent it runs on has one menu for it, as the master's chip has: the
        // account, the model and the effort; its separate rows are not drawn.
        let agent_step = found
            .rules
            .iter()
            .find(|rule| rule.id.ends_with(".model") && rule.agent.is_some())
            .map(|rule| rule.id.trim_end_matches("model").to_string());
        found
            .rules
            .iter()
            .filter(|rule| !is_fixed_on(rule))
            .filter(|rule| {
                let is_covered = agent_step.as_ref().is_some_and(|prefix| {
                    rule.id.starts_with(prefix.as_str())
                        && (rule.id.ends_with(".account") || rule.id.ends_with(".effort"))
                });
                !is_covered
            })
            .filter(|rule| !is_off || Some(rule.id.as_str()) == node.on_rule())
            .map(|rule| {
                let is_agent = rule.agent.is_some() && rule.id.ends_with(".model");
                let control = match (is_agent, &self.rules) {
                    (true, Some(rules)) => self.agent_control(rule, rules, cx),
                    _ => self.rule_control(rule, palette, cx),
                };
                let key = format!("engine-rule-{}", rule.id);
                let told = engine_acts::line(&self.acts, &key, palette, cx);
                let pending = super::pending_setting(&self.acts, &key, palette, cx);
                let (row, name) = match is_agent {
                    true => (
                        format!("engine-row-{}agent", rule.id.trim_end_matches("model")),
                        tr(Key("engine_page.agent")).into(),
                    ),
                    false => (format!("engine-row-{}", rule.id), rule.label.clone().into()),
                };
                let line = rule_line(&row, name, control, palette);
                div()
                    .child(line)
                    .children(pending)
                    .children(told)
                    .into_any_element()
            })
            .collect()
    }

    /// The one menu of a step's agent: the same groups and chip as the master's, over what the engine lists. A
    /// pick of the account, the model or the effort sets the rule of that name.
    fn agent_control(&self, rule: &Rule, rules: &Rules, cx: &mut Context<Self>) -> AnyElement {
        let Some(agent) = rule.agent.as_ref() else {
            return gpui::Empty.into_any_element();
        };
        let prefix = rule.id.trim_end_matches("model").to_string();
        let provider = agent.provider.clone();
        let accounts = step_accounts(rules);
        let models = models_of(rules, &agent.account, &provider);
        let effort = rules
            .rule(&format!("{prefix}effort"))
            .map(|rule| rule.as_text());
        let value = rule.as_text();
        let model_id = value.split_once(':').map_or(value.as_str(), |(_, id)| id);
        let mut groups = crate::ui::master_agent::step_menu_groups(
            &accounts,
            &agent.account,
            &models,
            Some(model_id),
            effort.as_deref(),
        );
        let notes = menu_notes(agent)
            .into_iter()
            .filter_map(|note| note_group(&note));
        groups.splice(0..0, notes.collect::<Vec<_>>());
        let acts = self.acts.clone();
        let chip = crate::ui::thread::agent_menu::chip_groups(&groups, |pick| {
            let (acts, pick, prefix, provider) =
                (acts.clone(), pick.clone(), prefix.clone(), provider.clone());
            Rc::new(move |_: &mut gpui::Window, cx: &mut App| {
                let (rule, value) = rule_of_pick(&prefix, &provider, &pick);
                send_rule(&acts, &rule, &value, cx);
            })
        });
        let label = self.pipeline_line(rules, &prefix);
        crate::ui::thread::agent_menu::grouped_chip(
            format!("engine-rule-{prefix}agent"),
            label,
            chip,
            cx,
        )
        .into_any_element()
    }

    /// The step's agent in words, the way its box says it.
    fn pipeline_line(&self, rules: &Rules, prefix: &str) -> SharedString {
        let node = Node::ALL
            .into_iter()
            .find(|node| node.model_rule() == Some(format!("{prefix}model").as_str()));
        node.and_then(|node| Pipeline::from_rules(Some(rules)).agent_line(node))
            .unwrap_or_default()
    }

    fn rule_control(&self, rule: &Rule, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = format!("engine-rule-{}", rule.id);
        if rule.read_only {
            return div()
                .text_color(palette.text_secondary)
                .child(read_only_words(rule))
                .into_any_element();
        }
        let acts = self.acts.clone();
        let rule_id = rule.id.clone();
        let send = move |value: &str, cx: &mut App| send_rule(&acts, &rule_id, value, cx);
        match rule.kind.as_str() {
            "bool" => toggle(id, rule.as_bool().unwrap_or(false), move |asked, _, cx| {
                send(flag(asked), cx)
            }),
            "int" => {
                let (acts, rule_id) = (self.acts.clone(), rule.id.clone());
                let on_number: OnNumber =
                    Rc::new(move |value, _, cx| set_number(&acts, &rule_id, value, cx));
                let shown = self
                    .acts
                    .read(cx)
                    .shown(&id)
                    .and_then(|text| text.parse().ok());
                let confirmed = rule.as_int().unwrap_or(0);
                stepper(
                    &id,
                    (confirmed, shown.unwrap_or(confirmed)),
                    rule.min,
                    rule.max,
                    on_number,
                    palette,
                )
                .w(sizes::INSPECTOR_CONTROL_WIDTH)
                .into_any_element()
            }
            "choice" => self.choice_control(rule, &id, Rc::new(send), cx),
            _ => div()
                .text_color(palette.text_secondary)
                .child(rule.as_text())
                .into_any_element(),
        }
    }

    fn choice_control(
        &self,
        rule: &Rule,
        id: &str,
        send: SendValue,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = rule.as_text();
        let choices = offered_choices(rule);
        let rules = self.rules.clone();
        let said = move |word: &str| match rules.as_ref() {
            Some(rules) => listed_words(rules, &rule_id_of(rule), word),
            None => rule_value_words(&rule_id_of(rule), word),
        };
        if choices.len() <= SEGMENTS_AT_MOST {
            let options: Vec<(SharedString, bool)> = choices
                .iter()
                .map(|word| (said(word), *word == current))
                .collect();
            return segments(id.to_string(), &options, move |nth, _, cx| {
                send(&choices[nth], cx)
            })
            .into_any_element();
        }
        let picks = choices
            .iter()
            .map(|word| {
                let (send, word) = (send.clone(), word.clone());
                controls::Choice {
                    label: said(&word),
                    is_on: word == current,
                    on_pick: Rc::new(move |_, cx: &mut App| send(&word, cx)),
                }
            })
            .collect();
        controls::menu_with_width(
            id.to_string(),
            said(&current),
            picks,
            sizes::INSPECTOR_CONTROL_WIDTH,
            cx,
        )
    }
}

/// A step's own on switch the engine fixes (Build is always on): a row that says "Build" and "true" tells
/// nothing, so it is not shown.
fn is_fixed_on(rule: &Rule) -> bool {
    rule.read_only && rule.kind == "bool" && rule.id.ends_with(".on")
}

/// The value of a rule that cannot be changed, in words: a switch is On or Off, a model or an account is
/// named, anything else is as the engine wrote it.
fn read_only_words(rule: &Rule) -> SharedString {
    match rule.as_bool() {
        Some(true) => tr(Key("engine_page.on")).into(),
        Some(false) => tr(Key("engine_page.off")).into(),
        None => rule_value_words(&rule.id, &rule.as_text()),
    }
}

/// Sends the rule `id` set to `value`, kept under the press `engine-rule-<id>`.
fn send_rule(acts: &Entity<EngineActs>, id: &str, value: &str, cx: &mut App) {
    let act = SET_RULE.act(&[("rule", id), ("value", value)]);
    let key = format!("engine-rule-{id}");
    acts.update(cx, |acts, cx| acts.press(&key, act, "", cx));
}

/// Sets the whole-number rule `id` to `value`: shown at once, sent after the presses that follow it, kept under
/// the press `engine-rule-<id>`.
fn set_number(acts: &Entity<EngineActs>, id: &str, value: i64, cx: &mut App) {
    let act = SET_RULE.act(&[("rule", id), ("value", &value.to_string())]);
    let key = format!("engine-rule-{id}");
    acts.update(cx, |acts, cx| {
        acts.set_value(&key, act, value.to_string(), COALESCE_PAUSE, cx)
    });
}

/// One row of rules: its name at the left, its control at the right, a hairline above all but the first.
fn rule_line(id: &str, name: SharedString, control: AnyElement, palette: &Palette) -> AnyElement {
    let (selector, name_selector) = (id.to_string(), format!("{id}-name"));
    div()
        .debug_selector(move || selector.clone())
        .min_h(sizes::ROW_MIN_HEIGHT)
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .border_t_1()
        .border_color(palette.border)
        .text_size(sizes::ROW_TEXT)
        .child(
            div()
                .debug_selector(move || name_selector.clone())
                .flex_none()
                .whitespace_nowrap()
                .child(name),
        )
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .justify_end()
                .overflow_hidden()
                .child(control),
        )
        .into_any_element()
}

/// The engine's words a step's menu leads with: why the step runs on another provider's account, and that the
/// lists of its account's models are being read or could not be (nothing when they are kept).
pub(super) fn menu_notes(agent: &sakura_core::engine::rules::RuleAgent) -> Vec<String> {
    let is_kept = matches!(agent.models_state.as_str(), "" | "kept");
    let lists = (!is_kept).then(|| agent.models_note.clone());
    [Some(agent.note.clone()), lists]
        .into_iter()
        .flatten()
        .filter(|note| !note.is_empty())
        .collect()
}

/// The engine's note of why the step runs on another provider's account, as the line of words that leads the
/// step's menu; none when the step runs on its own.
fn note_group(note: &str) -> Option<crate::ui::thread::MenuGroup> {
    (!note.is_empty()).then(|| crate::ui::thread::MenuGroup {
        title: Some(note.to_string().into()),
        entries: Vec::new(),
    })
}

/// The accounts a step's menu offers: every account of the engine, each by its Leeway name, whatever its
/// provider. No provider is kept for or from a step.
fn step_accounts(rules: &Rules) -> Vec<(String, String)> {
    let listed = rules.agents.iter();
    listed
        .map(|account| (account.id.clone(), account.label.clone()))
        .collect()
}

/// [`models_of`], for the tests.
#[cfg(test)]
pub(super) fn models_for_test(
    rules: &Rules,
    account: &str,
    provider: &str,
) -> Vec<sakura_acp::model::Choice> {
    models_of(rules, account, provider)
}

/// The models the program of `provider` lists for the step's account: those of that account, or of every account
/// of the program when the step runs on the project's own.
fn models_of(rules: &Rules, account: &str, provider: &str) -> Vec<sakura_acp::model::Choice> {
    let listed: Vec<_> = rules
        .agents
        .iter()
        .filter(|found| found.provider == provider && (found.id == account || account == "project"))
        .collect();
    if provider == "goose" && listed.len() != 1 {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    listed
        .into_iter()
        .flat_map(|found| &found.models)
        .filter(|model| seen.insert(model.id.clone()))
        .map(|model| sakura_acp::model::Choice {
            id: model.id.clone(),
            name: model.label.clone(),
            efforts: model
                .efforts
                .iter()
                .map(|effort| sakura_acp::model::Choice {
                    id: effort.clone(),
                    name: effort.clone(),
                    ..Default::default()
                })
                .collect(),
            default_effort: model.default_effort.clone(),
            ..Default::default()
        })
        .collect()
}

/// The rule a pick of the shared menu sets, and the value it is sent: the account's id, the model as
/// `provider:model`, the effort's id.
fn rule_of_pick(
    prefix: &str,
    provider: &str,
    pick: &crate::ui::thread::MenuPick,
) -> (String, String) {
    use crate::ui::thread::MenuPick;
    match pick {
        MenuPick::Model(id) => (format!("{prefix}model"), format!("{provider}:{id}")),
        MenuPick::Effort(id) => (format!("{prefix}effort"), id.clone()),
        MenuPick::Option { value, .. } => (format!("{prefix}account"), value.clone()),
        MenuPick::Mode(id) => (format!("{prefix}effort"), id.clone()),
    }
}

fn rule_id_of(rule: &Rule) -> String {
    rule.id.clone()
}

/// The choices a rule offers: a step pinned to one account is offered the models of that account's program
/// only (a Claude account never lists a Codex model), whatever other accounts the step may be moved to.
fn offered_choices(rule: &Rule) -> Vec<String> {
    let is_model = rule.id.ends_with(".model");
    let provider = match (&rule.agent, is_model) {
        (Some(agent), true) if !matches!(agent.account.as_str(), "" | "project") => Some(agent),
        _ => None,
    };
    let Some(agent) = provider else {
        return rule.choices.clone();
    };
    let prefix = format!("{}:", agent.provider);
    rule.choices
        .iter()
        .filter(|choice| **choice == agent.provider || choice.starts_with(&prefix))
        .cloned()
        .collect()
}

/// A rule's value in words, by what the engine lists: an account by its Leeway name, a model by the name its
/// program gives it; anything else as [`rule_value_words`] says it.
fn listed_words(rules: &Rules, rule: &str, value: &str) -> SharedString {
    let words = match rule.rsplit('.').next() {
        Some("model") => rules.model_label(value),
        Some("account") => rules.account_label(value),
        _ => None,
    };
    match words {
        Some(words) => SharedString::from(words.to_string()),
        None => rule_value_words(rule, value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_rule(account: &str) -> Rule {
        serde_json::from_value(serde_json::json!({
            "id": "reviews.task_review.model", "type": "choice", "value": "claude:opus",
            "choices": ["claude:opus", "claude:sonnet", "codex:gpt-6.2-sol"],
            "agent": {"account": account, "provider": "claude", "model": "opus", "effort": "high"}
        }))
        .expect("a rule")
    }

    #[test]
    fn the_engine_s_note_leads_the_step_menu_as_a_line_of_words_with_no_entry() {
        let group = note_group("No Claude account exists.").expect("a group");
        assert_eq!(group.title.as_deref(), Some("No Claude account exists."));
        assert!(group.entries.is_empty());
        assert!(note_group("").is_none(), "no note, no line");
    }

    #[test]
    fn every_step_offers_every_account_whatever_its_provider() {
        let rules: Rules = serde_json::from_value(serde_json::json!({
            "preset": "standard",
            "agents": [
                {"id": "a", "label": "Claude Work", "provider": "claude", "models": []},
                {"id": "b", "label": "Codex", "provider": "codex", "models": []}
            ]
        }))
        .expect("rules");
        let offered: Vec<String> = step_accounts(&rules)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(offered, ["a", "b"]);
    }

    #[test]
    fn a_step_on_a_claude_account_is_offered_claude_models_only() {
        assert_eq!(
            offered_choices(&model_rule("a")),
            ["claude:opus", "claude:sonnet"]
        );
        assert_eq!(offered_choices(&model_rule("project")).len(), 3);
    }

    #[test]
    fn a_model_and_an_account_are_said_as_the_engine_lists_them() {
        let rules: Rules = serde_json::from_value(serde_json::json!({
            "preset": "standard",
            "agents": [{"id": "a", "label": "Claude Work", "provider": "claude",
                        "models": [{"id": "opus", "label": "Opus 5.5"}]}]
        }))
        .expect("rules");
        assert_eq!(
            listed_words(&rules, "steps.build.model", "claude:opus"),
            "Opus 5.5"
        );
        assert_eq!(
            listed_words(&rules, "steps.build.account", "a"),
            "Claude Work"
        );
        assert_eq!(
            listed_words(&rules, "steps.build.model", "claude:other"),
            "Other"
        );
    }
}
