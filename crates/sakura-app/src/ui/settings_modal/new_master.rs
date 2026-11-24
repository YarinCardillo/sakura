//! "New Master conversations": the account, the model and the effort a new Master conversation starts with.
//! One chip with the same menu as the Master's own, over the accounts the engine lists; the tab +, Cmd-T and
//! the Master page begin on what is kept here.

use super::group::row;
use super::SettingsModal;
use crate::l10n::{tr, Key};
use crate::prefs::Preferences;
use crate::theme::Palette;
use crate::ui::master_accounts::{self, MasterAccount};
use crate::ui::master_agent::{step_menu_groups, ACCOUNT_OPTION};
use crate::ui::thread::agent_menu::{chip_groups, grouped_chip_with_footer};
use crate::ui::thread::{MenuGroup, MenuPick};
use gpui::{AnyElement, App, Context, SharedString};
use sakura_acp::model::Choice;
use std::collections::BTreeMap;
use std::rc::Rc;

impl SettingsModal {
    /// Asks the engine for the models of its accounts' programs, once; the row offers them when they arrive.
    pub(super) fn read_engine_models(&mut self, cx: &mut Context<Self>) {
        let pending = self.source.rules();
        self._engine_models = Some(cx.spawn(async move |modal, cx| {
            let rules = pending.await;
            drop(modal.update(cx, |modal, cx| {
                modal.engine_models = rules.ok();
                cx.notify();
            }));
        }));
    }

    pub(super) fn new_master_row(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = self.snapshot(cx);
        let accounts = snapshot.map(master_accounts::listed).unwrap_or_default();
        let engine_default = snapshot
            .and_then(|snapshot| snapshot.engine.as_ref())
            .and_then(|engine| engine.master_account.as_ref());
        let kept = Preferences::current(cx);
        let mut lists = crate::ui::master_page::kept_models_of(cx);
        if let Some(rules) = &self.engine_models {
            crate::ui::master_agent::take_in_catalogue(&mut lists, rules);
        }
        let shown = Shown::of(&accounts, &kept, &lists, engine_default);
        let this = cx.entity().downgrade();
        let menu = chip_groups(&shown.groups, |pick| {
            let (pick, this) = (pick.clone(), this.clone());
            Rc::new(move |_: &mut gpui::Window, cx: &mut App| {
                Preferences::change(cx, |preferences| set_from_pick(preferences, &pick));
                drop(this.update(cx, |_, cx| cx.notify()));
            })
        });
        let control = grouped_chip_with_footer(
            "settings-new-master",
            shown.label,
            menu,
            None,
            Some(crate::theme::settings::CHOICE_MENU_WIDTH),
            cx,
        );
        let name = tr(Key("settings_modal.general.new_master"));
        let note: SharedString = tr(Key("settings_modal.general.new_master_note")).into();
        row(
            "settings-row-new-master",
            name,
            Some(note),
            control,
            None,
            palette,
        )
    }
}

/// What the chip says and offers.
struct Shown {
    label: SharedString,
    groups: Vec<MenuGroup>,
}

impl Shown {
    fn of(
        accounts: &[MasterAccount],
        kept: &Preferences,
        lists: &BTreeMap<String, Vec<Choice>>,
        engine_default: Option<&sakura_core::engine::accounts::AccountRef>,
    ) -> Self {
        let account = master_accounts::default_with_engine(
            accounts,
            kept.new_master_account.as_deref(),
            engine_default,
        );
        let id = account.map(|account| account.id.clone());
        let models: &[Choice] = account.map_or(&[], |account| {
            crate::ui::master_agent::account_models(lists, account.agent, &account.id)
        });
        // A model and an effort saved with no account chosen stand for the account that is the default.
        let is_kept = kept.new_master_account.is_none() || kept.new_master_account == id;
        let model = kept.new_master_model.as_deref().filter(|_| is_kept);
        let effort = kept.new_master_effort.as_deref().filter(|_| is_kept);
        let named: Vec<(String, String)> = accounts
            .iter()
            .map(|account| (account.id.clone(), account.label.clone()))
            .collect();
        let groups = step_menu_groups(&named, id.as_deref().unwrap_or(""), models, model, effort);
        Self {
            label: words(account, models, model, effort).into(),
            groups,
        }
    }
}

/// The account, then the model and the effort when they were chosen, as the chip says them.
fn words(
    account: Option<&MasterAccount>,
    models: &[Choice],
    model: Option<&str>,
    effort: Option<&str>,
) -> String {
    let chosen = model.and_then(|id| models.iter().find(|choice| choice.id == id));
    let effort_name = effort.map(|id| {
        let listed = chosen.and_then(|model| model.efforts.iter().find(|choice| choice.id == id));
        listed.map_or_else(
            || crate::ui::master_agent::effort_words(id),
            |choice| crate::ui::master_agent::effort_words(&choice.id),
        )
    });
    let parts = [
        account.map(|account| account.label.clone()),
        chosen.map(|model| model.name.clone()),
        effort_name,
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join(", ")
}

/// What choosing an entry of the chip keeps: another account clears the model and the effort (they were chosen
/// for the first account's program), another model clears the effort.
fn set_from_pick(preferences: &mut Preferences, pick: &MenuPick) {
    match pick {
        MenuPick::Option { config_id, value } if config_id == ACCOUNT_OPTION => {
            preferences.new_master_account = Some(value.clone());
            preferences.new_master_model = None;
            preferences.new_master_effort = None;
        }
        MenuPick::Model(id) => {
            preferences.new_master_model = Some(id.clone()).filter(|id| !id.is_empty());
            preferences.new_master_effort = None;
        }
        MenuPick::Effort(id) => preferences.new_master_effort = Some(id.clone()),
        MenuPick::Option { .. } | MenuPick::Mode(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::master_agent::MasterAgent;

    fn pick_account(id: &str) -> MenuPick {
        MenuPick::Option {
            config_id: ACCOUNT_OPTION.into(),
            value: id.into(),
        }
    }

    #[test]
    fn another_account_clears_the_model_and_the_effort_and_another_model_the_effort() {
        let mut preferences = Preferences {
            new_master_account: Some("a".into()),
            new_master_model: Some("sonnet".into()),
            new_master_effort: Some("low".into()),
            ..Preferences::default()
        };
        set_from_pick(&mut preferences, &MenuPick::Model("opus".into()));
        assert_eq!(preferences.new_master_model.as_deref(), Some("opus"));
        assert_eq!(preferences.new_master_effort, None);
        set_from_pick(&mut preferences, &MenuPick::Effort("high".into()));
        assert_eq!(preferences.new_master_effort.as_deref(), Some("high"));
        set_from_pick(&mut preferences, &pick_account("b"));
        assert_eq!(preferences.new_master_account.as_deref(), Some("b"));
        assert_eq!(
            (preferences.new_master_model, preferences.new_master_effort),
            (None, None)
        );
    }

    fn claude_models() -> BTreeMap<String, Vec<Choice>> {
        let efforts = |ids: &[&str]| -> Vec<Choice> {
            ids.iter()
                .map(|id| Choice {
                    id: (*id).into(),
                    name: (*id).into(),
                    ..Choice::default()
                })
                .collect()
        };
        let model = |id: &str, name: &str, with: &[&str]| Choice {
            id: id.into(),
            name: name.into(),
            efforts: efforts(with),
            ..Choice::default()
        };
        let all = vec![
            model("sonnet", "Sonnet 5.5", &["low", "high"]),
            model("haiku", "Haiku 5.5", &["low", "high"]),
        ];
        [("claude".to_string(), all)].into()
    }

    #[test]
    fn the_control_reads_what_was_saved_with_a_check_on_it_and_offers_the_efforts_of_the_model() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let accounts = [MasterAccount {
            id: "personal".into(),
            label: "Claude".into(),
            agent: MasterAgent::Claude,
        }];
        // Saved with no account chosen: the model and the effort still stand for the first Claude account.
        let kept = Preferences {
            new_master_model: Some("haiku".into()),
            new_master_effort: Some("low".into()),
            ..Preferences::default()
        };
        let shown = Shown::of(&accounts, &kept, &claude_models(), None);
        assert_eq!(shown.label, "Claude, Haiku 5.5, Low");
        let on: Vec<String> = shown
            .groups
            .iter()
            .flat_map(|group| &group.entries)
            .filter(|entry| entry.is_on)
            .map(|entry| entry.name.to_string())
            .collect();
        assert_eq!(on, ["Claude", "Haiku 5.5", "Low"]);
    }

    #[test]
    fn the_chip_names_the_account_and_only_what_was_chosen_after_it() {
        let account = MasterAccount {
            id: "a".into(),
            label: "Personal".into(),
            agent: MasterAgent::Claude,
        };
        let sonnet = Choice {
            id: "sonnet".into(),
            name: "Sonnet 5.5".into(),
            ..Choice::default()
        };
        assert_eq!(words(Some(&account), &[], None, None), "Personal");
        let said = words(Some(&account), &[sonnet], Some("sonnet"), Some("low"));
        assert_eq!(said, "Personal, Sonnet 5.5, Low");
    }
}
