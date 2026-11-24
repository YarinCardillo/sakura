use super::model::{self, Verb};
use super::InboxPage;
use crate::l10n::{tr, Key};
use crate::theme::{self, Theme};
use crate::ui::dialog_frame;
use crate::ui::kit::{self, ButtonKind};
use crate::ui::settings_modal::controls;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window,
};
use gpui_component::input::InputState;
use gpui_component::WindowExt as _;
use sakura_core::engine::snapshot::{Snapshot, WaitProposal};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct ReproposalTimes {
    by_id: HashMap<String, (i64, String)>,
}

impl ReproposalTimes {
    pub(super) fn reconcile(&mut self, snapshot: &Snapshot) {
        self.by_id.retain(|id, (answered, _)| {
            snapshot
                .waits
                .iter()
                .any(|wait| wait.id == *id && model::raised(wait) <= Some(*answered))
        });
        let mut answers = HashMap::new();
        for decision in snapshot.decided.iter().flatten() {
            if let Some(at) = model::decided_at(decision) {
                let newest = answers.entry(decision.id.as_str()).or_insert(at);
                *newest = (*newest).max(at);
            }
        }
        for wait in &snapshot.waits {
            let Some(&answered) = answers.get(wait.id.as_str()) else {
                continue;
            };
            if model::raised(wait) <= Some(answered)
                && self
                    .by_id
                    .get(&wait.id)
                    .is_none_or(|(previous, _)| *previous < answered)
            {
                // Older engines can reuse the answered wait's timestamp when it returns.
                self.by_id.insert(
                    wait.id.clone(),
                    (
                        answered,
                        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    ),
                );
            }
        }
    }

    pub(super) fn apply(&self, snapshot: &mut std::rc::Rc<Snapshot>) {
        if !self.by_id.is_empty() {
            for wait in &mut std::rc::Rc::make_mut(snapshot).waits {
                if let Some((_, since)) = self.by_id.get(&wait.id) {
                    wait.since = Some(since.clone());
                }
            }
        }
    }
}

impl InboxPage {
    pub(super) fn open_proposal(
        &mut self,
        verb: Verb,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        let defaults = snapshot
            .waits
            .iter()
            .find(|wait| wait.id == verb.target)
            .and_then(|wait| wait.proposal.as_ref());
        let Some(defaults) = defaults else { return };
        if self.is_answered(&verb.target, cx) {
            return;
        }
        let inbox = cx.weak_entity();
        let editor = cx.new(|cx| ProposalEditor::new(inbox, verb, defaults, &snapshot, window, cx));
        crate::ui::dialog_frame::open_guarded(window, cx, move |dialog, window, _| {
            crate::ui::dialog_frame::placed(dialog, crate::ui::dialog_frame::Width::Small, window)
                .on_ok(|_, _, _| false)
                .child(editor.clone())
        });
    }
}

struct Choice {
    input: &'static str,
    label: Key,
    selected: Option<String>,
    default: Option<String>,
    options: Vec<(String, String)>,
}

impl Choice {
    fn word(&self) -> &str {
        self.selected.as_deref().map_or(
            self.default.as_deref().unwrap_or_else(|| {
                let field = if self.input == "account" {
                    "scheduled.account"
                } else {
                    "scheduled.preset"
                };
                crate::ui::scheduled::words::unresolved_default(field)
            }),
            |value| {
                self.options
                    .iter()
                    .find(|(option, _)| option == value)
                    .map_or_else(
                        || match self.input {
                            "account" => tr(Key("inbox.property.unavailable_account")),
                            _ => value,
                        },
                        |(_, label)| label.as_str(),
                    )
            },
        )
    }
}

struct ProposalEditor {
    inbox: WeakEntity<InboxPage>,
    verb: Verb,
    title: Entity<InputState>,
    choices: [Choice; 3],
}

impl ProposalEditor {
    fn new(
        inbox: WeakEntity<InboxPage>,
        mut verb: Verb,
        defaults: &WaitProposal,
        snapshot: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        verb.inputs.insert("op".into(), defaults.op.clone());
        let title = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(defaults.title.clone(), window, cx);
            input
        });
        let [_, preset, account] = crate::ui::scheduled::words::defaults(snapshot);
        let account = account.or_else(|| {
            let named = snapshot.engine.as_ref()?.master_account.as_ref()?;
            snapshot
                .accounts
                .iter()
                .find(|entry| entry.id == named.id)
                .map(|entry| entry.name().to_string())
        });
        let choices = [
            Choice {
                input: "project",
                label: Key("inbox.filter.project"),
                selected: defaults.project.clone(),
                default: None,
                options: snapshot
                    .projects
                    .iter()
                    .map(|p| (p.folder.clone(), p.name.clone()))
                    .collect(),
            },
            Choice {
                input: "preset",
                label: Key("inbox.property.preset"),
                selected: defaults.preset.clone(),
                default: preset,
                options: snapshot
                    .engine
                    .iter()
                    .flat_map(|engine| &engine.presets)
                    .map(|preset| (preset.name.clone(), preset.words()))
                    .collect(),
            },
            Choice {
                input: "account",
                label: Key("inbox.property.account"),
                selected: defaults.account.clone(),
                default: account,
                options: snapshot
                    .accounts
                    .iter()
                    .filter(|account| !account.is_hidden())
                    .map(|account| (account.id.clone(), account.name().to_string()))
                    .collect(),
            },
        ];
        Self {
            inbox,
            verb,
            title,
            choices,
        }
    }

    fn submit(&self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title.read(cx).value();
        let title = title.trim();
        if title.is_empty() || self.choices[0].selected.is_none() {
            return;
        }
        let mut verb = self.verb.clone();
        verb.inputs.insert("title".into(), title.into());
        for choice in &self.choices {
            if let Some(value) = &choice.selected {
                verb.inputs.insert(choice.input.into(), value.clone());
            }
        }
        if self
            .inbox
            .update(cx, |page, cx| page.start_proposal(verb, window, cx))
            .unwrap_or(false)
        {
            window.close_dialog(cx);
        }
    }

    fn choice(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let choice = &self.choices[index];
        let mut options = choice.options.clone();
        if choice.input == "project" {
            if let Ok(Some(snapshot)) = self.inbox.read_with(cx, |page, cx| page.snapshot(cx)) {
                options.retain(|(folder, _)| !snapshot.is_project_folder_missing(folder));
            }
        }
        let current = choice.selected.clone();
        let word = if choice.input == "project" && current.is_none() {
            tr(Key("new_lane.choose_project"))
        } else {
            choice.word()
        };
        let editor = cx.weak_entity();
        let palette = Theme::palette(cx);
        let choices = options
            .into_iter()
            .map(|(value, label)| {
                let editor = editor.clone();
                controls::Choice {
                    label: label.into(),
                    is_on: current.as_ref() == Some(&value),
                    on_pick: std::rc::Rc::new(move |_, cx| {
                        drop(editor.update(cx, |editor, cx| {
                            editor.choices[index].selected = Some(value.clone());
                            cx.notify();
                        }));
                    }),
                }
            })
            .collect();
        let menu = controls::menu_with_width(
            choice.input,
            word.to_string().into(),
            choices,
            theme::DIALOG_WIDTH_SMALL / 2.,
            cx,
        );
        crate::ui::settings_modal::group::row(
            format!("proposal-{}-row", choice.input),
            tr(choice.label),
            None,
            menu,
            None,
            &palette,
        )
    }
}

impl Render for ProposalEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let label = tr(Key("new_lane.start"));
        let start = if self.choices[0].selected.is_some() {
            kit::button("proposal-submit", label, ButtonKind::Primary, &palette)
                .on_click(cx.listener(|editor, _, window, cx| editor.submit(window, cx)))
        } else {
            kit::button_disabled("proposal-submit", label, ButtonKind::Primary, &palette)
        };
        let cancel = kit::button(
            "proposal-cancel",
            tr(Key("common.cancel")),
            ButtonKind::Secondary,
            &palette,
        )
        .on_click(|_, window, cx| window.close_dialog(cx));
        let body = crate::ui::dialog_frame::scrolling_body("inbox-proposal-body")
            .gap(theme::GAP_WIDE)
            .child(dialog_frame::title_field(
                ("proposal-title", "proposal-title-label"),
                tr(Key("new_lane.title_label")),
                &self.title,
                &palette,
            ))
            .child(crate::ui::settings_modal::group::group(
                (0..self.choices.len())
                    .map(|index| self.choice(index, cx))
                    .collect(),
                &palette,
            ));
        dialog_frame::surface("inbox-proposal-editor", window, &palette)
            .child(dialog_frame::heading(
                "proposal-heading",
                tr(Key("inbox.start_lane")),
                &palette,
            ))
            .child(body)
            .child(dialog_frame::footer(&palette).child(cancel).child(start))
    }
}

#[cfg(test)]
mod account_copy_tests {
    use super::*;

    #[test]
    fn review_missing_proposal_account_uses_a_label() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let choice = Choice {
            input: "account",
            label: Key("inbox.property.account"),
            selected: Some("old-account-id".into()),
            default: None,
            options: Vec::new(),
        };
        assert_eq!(choice.word(), "Unavailable account");
    }
}
