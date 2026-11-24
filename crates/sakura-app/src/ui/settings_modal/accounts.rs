//! "Accounts": the accounts the engine spends, each with its provider and plan. An account is renamed
//! through the action the engine offers on it; the app adds none of its own.

use super::controls;
use super::group::{button, group, row, sentence};
use super::SettingsModal;
use crate::l10n::{tr, tr_with, Key};
use crate::theme::{self, Palette};
use crate::ui::account_add;
use crate::ui::account_words::{kind_line, provider_name};
use crate::ui::engine_acts::{self, REMOVE_ACCOUNT, RENAME_ACCOUNT};

use gpui::{
    div, AnyElement, App, Context, IntoElement as _, ParentElement as _, SharedString, Styled as _,
    Window,
};
use sakura_core::engine::accounts::Account;
use sakura_core::engine::cli::Act;
use sakura_core::engine::snapshot::Action;
use std::collections::BTreeMap;
use std::rc::Rc;

/// The engine's action on an account: rename it.
const RENAME: &str = RENAME_ACCOUNT.key;
/// The engine's action that takes an account out of the book.
const REMOVE: &str = REMOVE_ACCOUNT.key;

impl SettingsModal {
    /// The groups of "Accounts".
    pub(super) fn accounts(
        &mut self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let actions = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(theme::GAP_SMALL)
            .child(self.add_account_button(cx))
            .child(self.look_again_button(cx))
            .into_any_element();
        let Some(snapshot) = self.snapshot(cx) else {
            let reading = sentence(tr(Key("settings.reading")), palette);
            return vec![actions, reading];
        };
        let accounts = snapshot.accounts.clone();
        if accounts.is_empty() {
            let none = sentence(tr(Key("settings_modal.accounts.none")), palette);
            return vec![actions, none];
        }
        let rows = accounts
            .iter()
            .map(|account| self.account_row(account, palette, cx));
        vec![actions, group(rows.collect(), palette)]
    }

    /// "Add account": opens the sheet that adds one, over Settings.
    fn add_account_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let (source, facts) = (self.source.clone(), self.facts.clone());
        let press = Rc::new(move |window: &mut Window, cx: &mut App| {
            account_add::open(source.clone(), facts.clone(), window, cx);
        });
        let label = tr(Key("account_add.title"));
        let button = button("settings-accounts-add", label, press, cx);
        div().flex().child(button).into_any_element()
    }

    fn look_again_button(&self, cx: &App) -> AnyElement {
        let (source, facts) = (self.source.clone(), self.facts.clone());
        let press = Rc::new(move |window: &mut Window, cx: &mut App| {
            use crate::ui::first_launch::{self, Mode};
            first_launch::open(source.clone(), facts.clone(), Mode::LookAgain, window, cx);
        });
        let button = button(
            "setup-look-again",
            tr(Key("first_launch.look_again")),
            press,
            cx,
        );
        div().flex().child(button).into_any_element()
    }

    /// Starts renaming the account `id`, its field showing `name`.
    pub fn start_rename(
        &mut self,
        id: &str,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.renaming = Some(id.to_string());
        self.name_field
            .update(cx, |field, cx| field.set_value(name, window, cx));
        cx.notify();
    }

    fn account_row(
        &self,
        account: &Account,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = format!("settings-account-{}", account.id);
        let mut buttons = Vec::new();
        if account.action(RENAME).is_some() {
            buttons.push(self.rename_button(account, &id, cx));
        }
        if let Some(remove) = account.action(REMOVE) {
            buttons.push(self.remove_button(account, remove, &id, cx));
        }
        let mut lines = vec![
            engine_acts::line(&self.acts, &id, palette, cx),
            engine_acts::line(&self.acts, &format!("{id}-remove"), palette, cx),
        ];
        for (action_key, label) in goose_actions(account) {
            if let Some(action) = account.action(action_key) {
                let action_id = format!("{id}-{action_key}");
                buttons.push(self.account_action_button(account, action, &action_id, label, cx));
                lines.push(engine_acts::line(&self.acts, &action_id, palette, cx));
            }
        }
        if self.renaming.as_deref() == Some(&account.id) {
            lines.push(Some(self.rename_form(account, &id, palette, cx)));
        }
        let actions = div()
            .max_w(theme::DIALOG_WIDTH_MEDIUM * 0.46)
            .flex()
            .flex_wrap()
            .justify_end()
            .gap(theme::GAP_SMALL)
            .children(buttons);
        let lines: Vec<_> = lines.into_iter().flatten().collect();
        let below =
            (!lines.is_empty()).then(|| div().flex().flex_col().children(lines).into_any_element());
        row(
            id,
            crate::ui::account_words::display_name(account),
            note_of(account),
            actions,
            below,
            palette,
        )
    }

    fn account_action_button(
        &self,
        account: &Account,
        action: &Action,
        id: &str,
        label: Key,
        cx: &App,
    ) -> AnyElement {
        let act = Act {
            action: action.clone(),
            target: account.target.clone(),
            inputs: BTreeMap::new(),
            confirm: None,
        };
        let (acts, key) = (self.acts.clone(), id.to_string());
        let words = tr_with(
            Key("goose.disable_ask"),
            &[("account", &crate::ui::account_words::display_name(account))],
        );
        let press = Rc::new(move |_: &mut Window, cx: &mut App| {
            acts.update(cx, |acts, cx| {
                acts.press(&key, act.clone(), words.to_string(), cx)
            });
        });
        button(id.to_string(), tr(label), press, cx).into_any_element()
    }

    fn rename_button(&self, account: &Account, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let (this, account_id, name) = (
            cx.entity().downgrade(),
            account.id.clone(),
            account.label.clone(),
        );
        let press = Rc::new(move |window: &mut Window, cx: &mut App| {
            drop(this.update(cx, |modal, cx| {
                modal.start_rename(&account_id, &name, window, cx)
            }));
        });
        button(
            format!("{id}-rename"),
            tr(Key("settings_modal.accounts.rename")),
            press,
            cx,
        )
        .into_any_element()
    }

    /// "Remove": asks the engine to take the account out. The engine asks first (the question names the
    /// account) and runs it only once the person confirms.
    fn remove_button(
        &self,
        account: &Account,
        action: &Action,
        id: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let act = Act {
            action: action.clone(),
            target: account.target.clone(),
            inputs: BTreeMap::new(),
            confirm: None,
        };
        let (acts, key) = (self.acts.clone(), format!("{id}-remove"));
        let words = remove_words(&crate::ui::account_words::display_name(account));
        let press = Rc::new(move |_: &mut Window, cx: &mut App| {
            let (key, act, words) = (key.clone(), act.clone(), words.clone());
            acts.update(cx, |acts, cx| acts.press(&key, act, words, cx));
        });
        button(
            format!("{id}-remove"),
            tr(Key("settings_modal.accounts.remove")),
            press,
            cx,
        )
        .into_any_element()
    }

    /// The field an account is renamed in, with the two ways to leave it.
    fn rename_form(
        &self,
        account: &Account,
        id: &str,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let field = controls::labeled_field(
            format!("{id}-name"),
            tr(Key("settings_modal.accounts.name_label")),
            &self.name_field,
            crate::theme::settings_modal::FORM_FIELD_WIDTH,
            palette,
        );
        let (this, acts) = (cx.entity().downgrade(), self.acts.clone());
        let (target, field_state, press_key) = (
            account.target.clone(),
            self.name_field.clone(),
            id.to_string(),
        );
        let save = Rc::new(move |_: &mut Window, cx: &mut App| {
            let label = field_state.read(cx).value().trim().to_string();
            let act = RENAME_ACCOUNT.act_on(&target, &[("label", &label)]);
            acts.update(cx, |acts, cx| acts.press(&press_key, act, "", cx));
            drop(this.update(cx, |modal, cx| modal.stop_rename(cx)));
        });
        let this = cx.entity().downgrade();
        let leave = Rc::new(move |_: &mut Window, cx: &mut App| {
            drop(this.update(cx, |modal, cx| modal.stop_rename(cx)));
        });
        let buttons = div()
            .flex()
            .items_center()
            .gap(theme::GAP)
            .child(controls::primary_button(
                format!("{id}-save"),
                tr(Key("settings_modal.accounts.save")),
                save,
                cx,
            ))
            .child(button(
                format!("{id}-leave"),
                tr(Key("common.cancel")),
                leave,
                cx,
            ));
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_WIDE)
            .child(field)
            .child(buttons)
            .into_any_element()
    }

    fn stop_rename(&mut self, cx: &mut Context<Self>) {
        self.renaming = None;
        cx.notify();
    }
}

fn goose_actions(account: &Account) -> Vec<(&'static str, Key)> {
    if account.provider != "goose" {
        return Vec::new();
    }
    if account.is_hidden() {
        return vec![("e", Key("goose.enable"))];
    }
    vec![
        ("accountcheck", Key("goose.check")),
        ("n", Key("goose.disable")),
    ]
}

/// The question asked before an account is removed: it names the account.
pub(super) fn remove_words(name: &str) -> String {
    tr_with(
        Key("settings_modal.accounts.remove_ask"),
        &[("account", name)],
    )
    .to_string()
}

/// What the row says under the name: the kind of account (its plan, the provider's name once), and that the
/// account is hidden.
fn note_of(account: &Account) -> Option<SharedString> {
    let mut kind = kind_line(account);
    if let Some(state) = account.readiness {
        let status = crate::ui::account_words::readiness_words(state, account.reason.as_deref());
        let forecast =
            (account.forecast_available == Some(false)).then(|| tr(Key("goose.forecast")));
        kind = Some(
            kind.as_deref()
                .into_iter()
                .chain([status])
                .chain(forecast)
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if !account.is_hidden() {
        return kind.map(SharedString::from);
    }
    let named = kind.unwrap_or_else(|| provider_name(&account.provider));
    Some(tr_with(
        Key("settings_modal.accounts.hidden_note"),
        &[("account", &named)],
    ))
}
