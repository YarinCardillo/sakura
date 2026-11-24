//! The two agents the master can be: Claude, over the adapter the engine's spec names, and Codex, over its
//! app-server. Which one the person works with is remembered for each project, and each keeps the
//! conversation it was left in: a switch shows the other one's, and starting fresh leaves only the open
//! one's in History. The engine's spec is the one source of how either starts; Codex is given the same
//! master tools and instructions, in Codex's own configuration, and the engine's decider answers what it asks.

use super::agent_words::capitalised;
use super::thread::{
    AgentMenu, MenuEntry, MenuGroup, MenuPick, StartChoice, LOGIN_OPTION, PLAN_MODE,
};
use crate::l10n::{tr, tr_with, Key};
use crate::prefs::Preferences;
use sakura_acp::connection::{
    AgentConfig, CodexConfig, CodexLogin, CodexLoginConfig, Deadlines, LoginEnd, LoginEvent,
    LoginFailure, SessionSetup,
};
use sakura_acp::model::{Choice, Selector, SessionChoices};
use sakura_acp::policy::decider::Decider;
use sakura_acp::thread::{Problem, ProblemKind};
use sakura_core::engine::master::MasterSpec;
use std::path::{Path, PathBuf};

/// What is said when no Codex program is found.
const NO_CODEX: Key = Key("master.no_codex");
/// The variable that names Codex's program, as an absolute path.
pub const CODEX_VARIABLE: &str = "SAKURA_CODEX";
/// The program, and the argument that makes it the server the app talks to.
const CODEX_PROGRAM: &str = "codex";
const CODEX_ARGUMENT: &str = "app-server";
/// The variable that tells Codex where its login and settings are: the one the person's own Codex uses.
const CODEX_HOME_VARIABLE: &str = "CODEX_HOME";
/// The folder of the engine's home that is Sakura's own Codex home: its login and its threads. The person's own
/// Codex home is never used, so a Master conversation is never in their Codex or ChatGPT app.
pub const CODEX_HOME_FOLDER: &str = "codex-home";
/// What is said while Sakura's Codex home has no login.
const CODEX_NEEDS_LOGIN: Key = Key("master.codex.needs_login");
/// The prefix of the names under which the Claude adapter shows an MCP server's tools.
const TOOL_PREFIX: &str = "mcp__";

/// An agent the master can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MasterAgent {
    Claude,
    Codex,
    Goose,
}

impl MasterAgent {
    /// The supported harnesses in display order.
    pub const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Goose];

    /// The agent as the preferences write it.
    pub fn code(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Goose => "goose",
        }
    }

    /// The agent a preference names; Claude for anything else, as before the switch existed.
    pub fn from_code(code: Option<&str>) -> Self {
        match code {
            Some("codex") => Self::Codex,
            Some("goose") => Self::Goose,
            _ => Self::Claude,
        }
    }

    /// The agent's name, as it is shown.
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => tr(Key("master.agent.claude")),
            Self::Codex => tr(Key("master.agent.codex")),
            Self::Goose => tr(Key("master.agent.goose")),
        }
    }

    /// The name the agent's session is kept under: each agent keeps its own, so a switch never opens one
    /// agent's session with the other.
    pub fn session_name(self) -> &'static str {
        match self {
            Self::Claude => super::master_page::MASTER_SESSION,
            Self::Codex => "@master-codex",
            Self::Goose => "@master-goose",
        }
    }

    /// The folder of the master's own files this agent's History is kept in, inside the master's folder.
    pub fn history_folder(self) -> Option<&'static str> {
        match self {
            Self::Claude => None,
            Self::Codex => Some("codex"),
            Self::Goose => Some("goose"),
        }
    }

    /// The agent the person last chose for `project` (none for "Any project"), Claude when none was chosen.
    pub fn remembered(preferences: &Preferences, project: Option<&str>) -> Self {
        let key = project.unwrap_or_default();
        Self::from_code(preferences.master_agents.get(key).map(String::as_str))
    }

    /// Keeps `self` as the agent of `project` in the preferences.
    pub fn remember(self, preferences: &mut Preferences, project: Option<&str>) {
        let key = project.unwrap_or_default().to_string();
        preferences
            .master_agents
            .insert(key, self.code().to_string());
    }
}

/// The mode of a session that asks nothing special of the master, and the one that only plans.
const MODE_DEFAULT: &str = "default";
const MODE_PLAN: &str = PLAN_MODE;
/// The id of the entry that asks the agent for its own model: nothing is named to it.
const OWN_MODEL: &str = "";
/// The efforts the app has words for, lowest first; any other the agent lists is its id with a capital.
const EFFORT_WORDS: [(&str, Key); 6] = [
    ("low", Key("master.effort.low")),
    ("medium", Key("master.effort.medium")),
    ("high", Key("master.effort.high")),
    ("xhigh", Key("master.effort.xhigh")),
    ("max", Key("master.effort.max")),
    ("ultra", Key("master.effort.ultra")),
];

pub fn effort_words(id: &str) -> String {
    match EFFORT_WORDS.iter().find(|(known, _)| *known == id) {
        Some((_, words)) => tr(*words).to_string(),
        None => capitalised(id),
    }
}

/// The models the menu lists: what the started session offers, else the last list the agent gave (kept in
/// the preferences), else nothing.
fn listed_models<'a>(session: &'a SessionChoices, kept: &'a [Choice]) -> &'a [Choice] {
    match &session.models {
        Some(models) if !models.choices.is_empty() => &models.choices,
        _ => kept,
    }
}

/// The version a model's name carries ("Opus 5.5" is 5.5) and the family before it ("opus"): none for a name
/// with no version, which is never put away.
fn family_and_version(name: &str) -> Option<(String, Vec<u32>)> {
    let words: Vec<&str> = name.split_whitespace().collect();
    let at = words.iter().position(|word| {
        word.chars().all(|c| c.is_ascii_digit() || c == '.')
            && word.chars().any(|c| c.is_ascii_digit())
    })?;
    let version = words[at]
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect();
    Some((words[..at].join(" ").to_lowercase(), version))
}

/// The models split as the agent's own pickers split them: a list that marks models hidden shows the others
/// and puts the hidden ones away; otherwise the newest of each family (by the version in its name) is shown
/// and the older ones are put away, newest first. A name with no version is shown.
pub fn split_models(models: &[Choice]) -> (Vec<&Choice>, Vec<&Choice>) {
    if models.iter().any(|model| model.is_hidden) {
        return models.iter().partition(|model| !model.is_hidden);
    }
    let mut newest: std::collections::HashMap<String, (&Vec<u32>, usize)> =
        std::collections::HashMap::new();
    let parsed: Vec<Option<(String, Vec<u32>)>> = models
        .iter()
        .map(|model| family_and_version(&model.name))
        .collect();
    for (at, found) in parsed.iter().enumerate() {
        if let Some((family, version)) = found {
            let best = newest.get(family).map(|(best, _)| *best);
            if best.is_none_or(|best| version > best) {
                newest.insert(family.clone(), (version, at));
            }
        }
    }
    let is_shown = |at: usize| match &parsed[at] {
        Some((family, _)) => newest.get(family).is_some_and(|(_, best)| *best == at),
        None => true,
    };
    let (shown, mut put_away): (Vec<_>, Vec<_>) = (0..models.len()).partition(|at| is_shown(*at));
    put_away.sort_by(|a, b| {
        parsed[*b]
            .as_ref()
            .map(|p| &p.1)
            .cmp(&parsed[*a].as_ref().map(|p| &p.1))
    });
    (
        shown.into_iter().map(|at| &models[at]).collect(),
        put_away.into_iter().map(|at| &models[at]).collect(),
    )
}

/// What the agent chip says and offers, for the conversation's own agent: its models (older ones under "More
/// models"), the effort of the chosen model, every other option the agent sends, and the mode. `start` is what
/// drives the first session, and the running session's choices take precedence once it opens.
pub fn agent_menu(
    agent: MasterAgent,
    start: &StartChoice,
    kept: &[Choice],
    session: &SessionChoices,
) -> AgentMenu {
    let models = listed_models(session, kept);
    let model_id = session
        .models
        .as_ref()
        .map(|models| models.current.as_str())
        .filter(|id| !id.is_empty())
        .or_else(|| start.model.as_deref().filter(|id| !id.is_empty()));
    let current = current_model(models, model_id);
    let effort = current_effort(current, start, session);
    if agent == MasterAgent::Goose {
        let effort = effort.filter(|id| {
            current.is_some_and(|model| model.efforts.iter().any(|effort| effort.id == *id))
        });
        let fixed = session.models.is_some();
        let groups = if fixed {
            Vec::new()
        } else {
            let mut groups = model_groups(models, current);
            groups.extend(effort_group(current, effort.as_deref(), session, false));
            groups
        };
        return AgentMenu {
            name: chip_words(
                Some(
                    current.map_or_else(|| tr(Key("goose.default_model")).to_string(), model_name),
                ),
                effort.as_deref(),
                false,
            ),
            groups,
        };
    }
    let uses_session_effort = agent == MasterAgent::Claude
        && session
            .uses_session_effort
            .unwrap_or(start.uses_session_effort);
    let is_claude = agent == MasterAgent::Claude;
    let checked_effort = if uses_session_effort || (is_claude && effort.is_none()) {
        Some("default")
    } else {
        effort.as_deref()
    };
    let mode = session
        .modes
        .as_ref()
        .map(|modes| modes.current.as_str())
        .filter(|mode| super::master_picks::is_master_mode(mode))
        .or(start.mode.as_deref())
        .unwrap_or(MODE_DEFAULT);
    let mut groups = vec![mode_group(mode)];
    let mut model_groups = model_groups(models, current);
    if is_claude && !models.is_empty() && !models.iter().any(|model| model.id == "default") {
        model_groups[0].entries.insert(
            0,
            MenuEntry {
                name: tr(Key("master.menu.recommended_model")).into(),
                is_on: model_id == Some("default"),
                pick: MenuPick::Model("default".into()),
            },
        );
    }
    if let Some(id) = model_id.filter(|id| current.is_none() && *id != "default") {
        for entry in &mut model_groups[0].entries {
            entry.is_on = false;
        }
        model_groups[0].entries.insert(
            0,
            MenuEntry {
                name: capitalised(id).into(),
                is_on: true,
                pick: MenuPick::Model(id.into()),
            },
        );
    }
    groups.extend(model_groups);
    groups.extend(effort_group(current, checked_effort, session, is_claude));
    groups.extend(other_groups(session));
    let model = chip_model(current, model_id);
    let shown_effort = checked_effort.filter(|id| agent != MasterAgent::Claude || *id != "default");
    AgentMenu {
        name: chip_words(model, shown_effort, mode == MODE_PLAN),
        groups,
    }
}

/// The menu of one agent as a step of a lane runs it: the account (each a flat entry under its Leeway name), the
/// model of that account's program and the effort of that model, built with the same functions as the master's
/// chip, so one look and one code path serve both. Picks are `Option` for the account (`config_id` `account`),
/// `Model` and `Effort` for the others.
pub fn step_menu_groups(
    accounts: &[(String, String)],
    account: &str,
    models: &[Choice],
    model: Option<&str>,
    effort: Option<&str>,
) -> Vec<MenuGroup> {
    let current = model.and_then(|id| models.iter().find(|choice| choice.id == id));
    let mut groups = Vec::new();
    if !accounts.is_empty() {
        let entries = accounts
            .iter()
            .map(|(id, label)| MenuEntry {
                name: label.clone().into(),
                is_on: id == account,
                pick: MenuPick::Option {
                    config_id: ACCOUNT_OPTION.to_string(),
                    value: id.clone(),
                },
            })
            .collect();
        groups.push(MenuGroup {
            title: Some(tr(Key("master.menu.account")).into()),
            entries,
        });
    }
    groups.extend(model_groups(models, current));
    groups.extend(effort_group(
        current,
        effort,
        &SessionChoices::default(),
        false,
    ));
    groups
}

/// The `config_id` an account's pick carries.
pub const ACCOUNT_OPTION: &str = "account";

pub(super) fn current_model<'a>(
    models: &'a [Choice],
    model_id: Option<&str>,
) -> Option<&'a Choice> {
    if let Some(model_id) = model_id {
        let selector = Selector {
            choices: models.to_vec(),
            ..Selector::default()
        };
        let id = &selector.matching(model_id)?.id;
        return models.iter().find(|model| &model.id == id);
    }
    models
        .iter()
        .find(|model| model.id == "default")
        .or_else(|| models.first())
}

/// The effort in force: the session's, else the one picked before it, else the model's own default.
fn current_effort(
    model: Option<&Choice>,
    start: &StartChoice,
    session: &SessionChoices,
) -> Option<String> {
    let own = session
        .efforts
        .as_ref()
        .map(|efforts| efforts.current.clone());
    let accepted = |id: &String| {
        !id.is_empty()
            && model.is_none_or(|model| {
                model.efforts.is_empty() || model.efforts.iter().any(|effort| effort.id == *id)
            })
    };
    own.filter(|id| !id.is_empty())
        .or_else(|| start.effort.clone().filter(accepted))
        .or_else(|| {
            if start.uses_session_effort || session.uses_session_effort == Some(true) {
                return None;
            }
            model
                .and_then(|model| model.default_effort.clone())
                .filter(|id| !id.is_empty())
        })
}

/// The chip's words: "Opus 5.5 High", the model and its effort. Before the agent has listed its models, the
/// model selected for the conversation in its own id's words. With none selected the chip has no
/// name yet, and says nothing: never the program's name and never a "Default" nobody picked. With ", plan" while
/// the master only plans.
fn chip_model(model: Option<&Choice>, saved: Option<&str>) -> Option<String> {
    model.map(model_name).or_else(|| saved.map(capitalised))
}

fn chip_words(model: Option<String>, effort: Option<&str>, is_plan: bool) -> gpui::SharedString {
    let Some(model_words) = model else {
        return gpui::SharedString::default();
    };
    let name = match effort {
        Some(effort) => format!("{model_words} {}", effort_words(effort)),
        None => model_words,
    };
    match is_plan {
        true => tr_with(Key("master.chip.plan"), &[("name", &name)]),
        false => name.into(),
    }
}

/// A model's name for the chip: the agent's own words for its recommended default read "Default".
fn model_name(model: &Choice) -> String {
    let name = model.name.trim_end_matches(" (recommended)");
    name.to_string()
}

fn model_entry(model: &Choice, in_force: Option<&Choice>) -> MenuEntry {
    MenuEntry {
        name: model.name.clone().into(),
        is_on: in_force.is_some_and(|current| current.id == model.id),
        pick: MenuPick::Model(model.id.clone()),
    }
}

/// The models, then those put away under "More models"; before any list is known, the one entry that asks the
/// agent for its own model.
fn model_groups(models: &[Choice], in_force: Option<&Choice>) -> Vec<MenuGroup> {
    if models.is_empty() {
        let own = MenuEntry {
            name: tr(Key("master.menu.own_model")).into(),
            is_on: in_force.is_none(),
            pick: MenuPick::Model(OWN_MODEL.to_string()),
        };
        return vec![MenuGroup {
            title: None,
            entries: vec![own],
        }];
    }
    let (shown, away) = split_models(models);
    let entries = |models: Vec<&Choice>| {
        models
            .into_iter()
            .map(|model| model_entry(model, in_force))
            .collect::<Vec<_>>()
    };
    let mut groups = vec![MenuGroup {
        title: None,
        entries: entries(shown),
    }];
    if !away.is_empty() {
        groups.push(MenuGroup {
            title: Some(tr(Key("master.menu.more_models")).into()),
            entries: entries(away),
        });
    }
    groups
}

/// The efforts the model accepts, the one in force marked and the model's own default said: the session's
/// list, else the model's.
fn effort_group(
    model: Option<&Choice>,
    in_force: Option<&str>,
    session: &SessionChoices,
    has_session_default: bool,
) -> Option<MenuGroup> {
    let mut listed: Vec<Choice> = match (&session.efforts, model) {
        (Some(efforts), _) => efforts.choices.clone(),
        (None, Some(model)) => model.efforts.clone(),
        _ => Vec::new(),
    };
    if has_session_default
        && (model.is_some() || session.efforts.is_some())
        && !listed.iter().any(|effort| effort.id == "default")
    {
        listed.insert(
            0,
            Choice {
                id: "default".into(),
                ..Choice::default()
            },
        );
    }
    if listed.is_empty() {
        return None;
    }
    let default = model.and_then(|model| model.default_effort.as_deref());
    let entries = listed.iter().map(|effort| {
        let words = if has_session_default && effort.id == "default" {
            tr(Key("master.menu.own_model")).to_string()
        } else {
            effort_words(&effort.id)
        };
        let name = match !has_session_default && default == Some(effort.id.as_str()) {
            true => tr_with(Key("master.effort.default"), &[("name", &words)]).to_string(),
            false => words,
        };
        MenuEntry {
            name: name.into(),
            is_on: in_force == Some(effort.id.as_str()),
            pick: MenuPick::Effort(effort.id.clone()),
        }
    });
    Some(MenuGroup {
        title: Some(tr(Key("master.menu.effort")).into()),
        entries: entries.collect(),
    })
}

/// Every other option the agent sends, each in words under its own name: a switch is one entry that is checked
/// while it is on.
fn other_groups(session: &SessionChoices) -> Vec<MenuGroup> {
    session
        .others
        .iter()
        .filter_map(|other| {
            let config_id = other.config_id.clone()?;
            if other.is_toggle {
                let is_on = other.current == "true";
                let pick = MenuPick::Option {
                    config_id,
                    value: (!is_on).to_string(),
                };
                let entry = MenuEntry {
                    name: other.name.clone().into(),
                    is_on,
                    pick,
                };
                return Some(MenuGroup {
                    title: None,
                    entries: vec![entry],
                });
            }
            let entries = other.choices.iter().map(|choice| MenuEntry {
                name: choice.name.clone().into(),
                is_on: choice.id == other.current,
                pick: MenuPick::Option {
                    config_id: config_id.clone(),
                    value: choice.id.clone(),
                },
            });
            Some(MenuGroup {
                title: Some(other.name.clone().into()),
                entries: entries.collect(),
            })
        })
        .collect()
}

/// The mode, in the same words for both agents: the session's own two (edit, plan) in the words of the app.
fn mode_group(in_force: &str) -> MenuGroup {
    let modes = [
        (MODE_DEFAULT, Key("master.menu.mode.default")),
        (MODE_PLAN, Key("master.menu.mode.plan")),
    ];
    let entries = modes.into_iter().map(|(id, words)| MenuEntry {
        name: tr(words).into(),
        is_on: in_force == id,
        pick: MenuPick::Mode(id.into()),
    });
    MenuGroup {
        title: Some(tr(Key("thread.mode")).into()),
        entries: entries.collect(),
    }
}

/// A list of models as the preferences keep it, so the menu of an empty conversation shows the last list the
/// agent gave.
pub fn models_to_json(models: &[Choice]) -> serde_json::Value {
    let named = |choice: &Choice| serde_json::json!({ "id": choice.id, "name": choice.name });
    let list = models.iter().map(|model| {
        serde_json::json!({
            "id": model.id, "name": model.name, "hidden": model.is_hidden,
            "efforts": model.efforts.iter().map(named).collect::<Vec<_>>(),
            "defaultEffort": model.default_effort,
            "tiers": model.tiers.iter().map(named).collect::<Vec<_>>(),
        })
    });
    serde_json::Value::Array(list.collect())
}

/// The models the engine lists for the program `provider`, each once, as the chip's menu lists them: what an
/// empty conversation offers before any session has said what its agent has.
pub fn catalogue_of(rules: &sakura_core::engine::rules::Rules, provider: &str) -> Vec<Choice> {
    catalogue(rules, provider, None)
}

fn catalogue(
    rules: &sakura_core::engine::rules::Rules,
    provider: &str,
    id: Option<&str>,
) -> Vec<Choice> {
    let mut seen = std::collections::HashSet::new();
    let listed = rules
        .agents
        .iter()
        .filter(|account| account.provider == provider && id.is_none_or(|id| account.id == id))
        .flat_map(|account| &account.models);
    listed
        .filter(|model| seen.insert(model.id.clone()))
        .map(|model| Choice {
            id: model.id.clone(),
            name: model.label.clone(),
            efforts: model
                .efforts
                .iter()
                .map(|effort| Choice {
                    id: effort.clone(),
                    name: effort.clone(),
                    ..Choice::default()
                })
                .collect(),
            default_effort: model.default_effort.clone(),
            ..Choice::default()
        })
        .collect()
}

/// Puts the engine's models in `kept` (the lists the agents last gave, by their code) for every program whose
/// list is missing or says no efforts: the engine's lists carry the efforts each model takes.
pub fn take_in_catalogue(
    kept: &mut std::collections::BTreeMap<String, Vec<Choice>>,
    rules: &sakura_core::engine::rules::Rules,
) {
    kept.retain(|key, _| !key.starts_with("goose/"));
    for account in rules
        .agents
        .iter()
        .filter(|account| account.provider == "goose")
    {
        kept.insert(
            format!("goose/{}", account.id),
            catalogue(rules, "goose", Some(&account.id)),
        );
    }
    for agent in [MasterAgent::Claude, MasterAgent::Codex] {
        let listed = catalogue_of(rules, agent.code());
        let has_efforts = |list: &Vec<Choice>| list.iter().any(|model| !model.efforts.is_empty());
        let is_wanted = kept.get(agent.code()).is_none_or(|own| !has_efforts(own));
        if !listed.is_empty() && is_wanted {
            kept.insert(agent.code().to_string(), listed);
        }
    }
}

pub fn account_models<'a>(
    models: &'a std::collections::BTreeMap<String, Vec<Choice>>,
    agent: MasterAgent,
    account: &str,
) -> &'a [Choice] {
    let found = if agent == MasterAgent::Goose {
        models.get(&format!("goose/{account}"))
    } else {
        models.get(agent.code())
    };
    found.map_or(&[], Vec::as_slice)
}

/// The list [`models_to_json`] wrote; what cannot be read is left out.
pub fn models_from_json(kept: &serde_json::Value) -> Vec<Choice> {
    let text = |value: &serde_json::Value, key: &str| value.get(key)?.as_str().map(str::to_string);
    let named = |list: Option<&serde_json::Value>| -> Vec<Choice> {
        let listed = list
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten();
        listed
            .filter_map(|value| {
                Some(Choice {
                    id: text(value, "id")?,
                    name: text(value, "name")?,
                    ..Choice::default()
                })
            })
            .collect()
    };
    let listed = kept.as_array().into_iter().flatten();
    listed
        .filter_map(|value| {
            Some(Choice {
                id: text(value, "id")?,
                name: text(value, "name")?,
                efforts: named(value.get("efforts")),
                default_effort: text(value, "defaultEffort"),
                tiers: named(value.get("tiers")),
                is_hidden: value
                    .get("hidden")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                description: None,
            })
        })
        .collect()
}

/// Resolves Codex for both the login and the Master, including launches from Finder.
pub fn find_codex(
    spec: &MasterSpec,
    named: Option<&str>,
    app_path: Option<&str>,
) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    find_codex_for_home(spec, named, app_path, home.as_deref())
}

fn find_codex_for_home(
    spec: &MasterSpec,
    named: Option<&str>,
    app_path: Option<&str>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(named) = named.filter(|named| Path::new(named).is_absolute()) {
        return is_program(Path::new(named)).then(|| PathBuf::from(named));
    }
    let local_bin = home.map(|home| home.join(".local/bin"));
    let folders = [
        spec.path.as_str(),
        spec.env.get("PATH").map_or("", String::as_str),
        app_path.unwrap_or_default(),
    ];
    let candidates = folders
        .into_iter()
        .flat_map(std::env::split_paths)
        .chain(local_bin)
        .filter(|folder| folder.is_absolute())
        .map(|folder| folder.join(CODEX_PROGRAM));
    configured_codex(spec)
        .into_iter()
        .chain(candidates)
        .find(|candidate| is_program(candidate))
}

fn configured_codex(spec: &MasterSpec) -> Option<PathBuf> {
    let engine = Path::new(spec.env.get("SAKURA_ENGINE_HOME")?);
    if !engine.is_absolute() {
        return None;
    }
    let programs = std::fs::read_to_string(engine.join("programs.toml")).ok()?;
    let programs: toml::Table = programs.parse().ok()?;
    let path = Path::new(programs.get(CODEX_PROGRAM)?.as_str()?);
    path.is_absolute().then(|| path.to_path_buf())
}

fn is_program(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = path
        .metadata()
        .map(|metadata| metadata.permissions().mode());
    path.is_file() && mode.is_ok_and(|mode| mode & 0o111 != 0)
}

/// Offer only explicit MCP tools allowed by the engine's Claude settings.
/// A deny rule can cover a tool, server or wildcard. Ignore parenthesized qualifiers when denying.
/// Give Codex no tools if a deny rule is unreadable.
pub fn allowed_tools(spec: &MasterSpec) -> Vec<String> {
    let rules = |name: &str| -> Vec<String> {
        let rules = spec
            .options
            .pointer(&format!("/settings/permissions/{name}"));
        let rules = rules
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten();
        rules
            .filter_map(|rule| rule.as_str().map(str::to_string))
            .collect()
    };
    let denied = rules("deny");
    let is_tool = |rule: &String| rule.starts_with(TOOL_PREFIX) && !rule.contains(['*', '(']);
    let is_kept = |tool: &String| {
        denied
            .iter()
            .all(|rule| denial(rule, tool) == Denial::Spares)
    };
    rules("allow")
        .into_iter()
        .filter(is_tool)
        .filter(is_kept)
        .collect()
}

/// What a deny rule does to a tool.
#[derive(Debug, PartialEq, Eq)]
enum Denial {
    Denies,
    Spares,
    /// The rule is not one this reads, so it may cover the tool.
    Unreadable,
}

/// The characters of a rule's name: those of a tool's, and `*`.
fn is_name_char(letter: char) -> bool {
    letter.is_ascii_alphanumeric() || matches!(letter, '_' | '.' | '-' | '*')
}

fn denial(rule: &str, tool: &str) -> Denial {
    let rule = rule.trim();
    let name = match rule.split_once('(') {
        Some((name, specifier)) if specifier.ends_with(')') => name,
        Some(_) => return Denial::Unreadable,
        None => rule,
    };
    if name.is_empty() || !name.chars().all(is_name_char) {
        return Denial::Unreadable;
    }
    let is_server = name.starts_with(TOOL_PREFIX) && tool.starts_with(&format!("{name}__"));
    match name == tool || is_server || glob_matches(name, tool) {
        true => Denial::Denies,
        false => Denial::Spares,
    }
}

/// Whether `text` fits `pattern`, whose `*` stands for any run of characters.
fn glob_matches(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return false;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// The menu group that offers the login while Sakura's Codex home has none, and the way out of it while it is
/// being made in the browser.
pub fn login_group(is_waiting: bool) -> MenuGroup {
    let name = match is_waiting {
        true => Key("master.codex.login_cancel"),
        false => Key("master.codex.login"),
    };
    MenuGroup {
        title: None,
        entries: vec![MenuEntry {
            name: tr(name).into(),
            is_on: false,
            pick: MenuPick::Option {
                config_id: LOGIN_OPTION.to_string(),
                value: "codex".to_string(),
            },
        }],
    }
}

/// The Codex home of the account the spec is for: the folder the engine names with `codex_home` when it is
/// another account's, else the engine's own under its home. The person's own (`~/.codex`), which the engine names
/// for the default account, is not Sakura's to run in: Sakura's own folder stands in its place, where its login
/// is kept.
pub fn sakura_codex_home(spec: &MasterSpec) -> Option<PathBuf> {
    let named = PathBuf::from(&spec.codex_home);
    if named.is_absolute() && !is_the_persons_codex_home(&named) {
        return Some(named);
    }
    let engine = spec
        .env
        .get("SAKURA_ENGINE_HOME")
        .filter(|home| Path::new(home).is_absolute())?;
    let own = PathBuf::from(engine).join(CODEX_HOME_FOLDER);
    (!is_the_persons_codex_home(&own)).then_some(own)
}

/// Whether `home` is the `.codex` folder of the person's own home.
fn is_the_persons_codex_home(home: &Path) -> bool {
    account_home().is_some_and(|own| is_codex_home_of(home, &own))
}

/// Whether `home` and the `.codex` folder of `account_home` are one folder, by their real paths: a link or a
/// `..` does not make it another.
fn is_codex_home_of(home: &Path, account_home: &Path) -> bool {
    let personal = account_home.join(".codex");
    home == personal || real_path(home) == real_path(&personal)
}

/// The real path of `path`, though it may not exist yet: its nearest folder that does is resolved and the rest
/// is put back.
fn real_path(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut at = path;
    loop {
        if let Ok(real) = at.canonicalize() {
            return rest.iter().rev().fold(real, |real, part| real.join(part));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                at = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// The home of the account the app runs as, from the password database as the engine reads it: `$HOME` is
/// whatever the app was started with, never the answer.
fn account_home() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = std::env::var_os("SAKURA_TEST_ACCOUNT_HOME") {
        return Some(home.into());
    }
    use nix::unistd::{getuid, User};

    User::from_uid(getuid()).ok().flatten().map(|user| user.dir)
}

/// Whether Sakura's Codex home holds a login. Only that a login file is there is looked at: its contents are
/// never read.
#[cfg(test)]
pub fn has_codex_login(spec: &MasterSpec) -> bool {
    sakura_codex_home(spec).is_some_and(|home| home.join("auth.json").is_file())
}

/// Whether the home of the app's own engine holds a Codex login, for the menu before any spec was read.
pub fn app_has_codex_login() -> bool {
    sakura_core::engine::watch::home()
        .join(CODEX_HOME_FOLDER)
        .join("auth.json")
        .is_file()
}

/// How the login of Codex is started for Sakura's `home`: the program's own app-server, in the environment
/// the engine gave the master and with Sakura's home as the Codex home, so the app never sees a credential.
pub fn codex_login_config(spec: &MasterSpec, program: &Path, home: &Path) -> CodexLoginConfig {
    let (env, env_allow) = codex_environment(spec, program, home);
    CodexLoginConfig {
        command: vec![program.display().to_string(), CODEX_ARGUMENT.to_string()],
        env,
        env_allow: Some(env_allow),
        unset_env: spec.unset_env.clone(),
        patience: crate::theme::thread::CODEX_LOGIN_PATIENCE,
    }
}

/// Starts the login of Codex for the engine's spec, in `home` or, when none is given, in Sakura's own Codex home.
/// The error is why it could not start, as a sentence.
pub fn start_codex_login_in(
    spec: &MasterSpec,
    home: Option<&Path>,
) -> Result<(CodexLogin, tokio::sync::mpsc::Receiver<LoginEvent>), String> {
    let named = std::env::var(CODEX_VARIABLE).ok();
    let app_path = std::env::var("PATH").ok();
    let program = find_codex(spec, named.as_deref(), app_path.as_deref())
        .ok_or_else(|| tr(NO_CODEX).to_string())?;
    let home = match home {
        Some(home) => home.to_path_buf(),
        None => sakura_codex_home(spec)
            .ok_or_else(|| tr(Key("master.codex.login_no_home")).to_string())?,
    };
    std::fs::create_dir_all(&home).map_err(|error| error.to_string())?;
    Ok(sakura_acp::connection::start_codex_login(
        codex_login_config(spec, &program, &home),
    ))
}

/// What the person is told when a login ended without a login: in words, for the Master tab. None for a
/// login that is in the home.
pub fn login_end_words(end: &LoginEnd) -> Option<String> {
    let reason = |key: Key, reason: &str| tr_with(key, &[("reason", reason)]).to_string();
    match end {
        LoginEnd::Done => None,
        LoginEnd::Cancelled => Some(tr(Key("master.codex.login_cancelled")).to_string()),
        LoginEnd::Failed(LoginFailure::NotStarted(why)) => {
            Some(reason(Key("master.codex.login_not_started"), why))
        }
        LoginEnd::Failed(LoginFailure::Refused(why)) => {
            Some(reason(Key("master.codex.login_refused"), why))
        }
        LoginEnd::Failed(LoginFailure::Declined(Some(why))) => {
            Some(reason(Key("master.codex.login_declined"), why))
        }
        LoginEnd::Failed(LoginFailure::Declined(None)) => {
            Some(tr(Key("master.codex.login_declined_plain")).to_string())
        }
        LoginEnd::Failed(LoginFailure::Broke) => {
            Some(tr(Key("master.codex.login_broke")).to_string())
        }
        LoginEnd::Failed(LoginFailure::TimedOut) => {
            Some(tr(Key("master.codex.login_timed_out")).to_string())
        }
    }
}

/// How Codex is started from the engine's spec: the program and its server argument, the environment the
/// engine gave the master (and Codex's own home, where its login is), the master's folder, instructions and
/// MCP servers, and the engine as the one who decides what Codex asks to do.
pub fn codex_config_of(
    spec: &MasterSpec,
    program: Option<&Path>,
    decider: Decider,
) -> Result<CodexConfig, Problem> {
    let home = sakura_codex_home(spec).ok_or_else(|| Problem {
        kind: ProblemKind::NotStarted,
        detail: tr(Key("master.codex.login_no_home")).to_string(),
    })?;
    if !home.join("auth.json").is_file() {
        return Err(Problem {
            kind: ProblemKind::AuthRequired,
            detail: tr(CODEX_NEEDS_LOGIN).to_string(),
        });
    }
    let program = program.ok_or_else(no_codex)?;
    let (env, env_allow) = codex_environment(spec, program, &home);
    let command = vec![program.display().to_string(), CODEX_ARGUMENT.to_string()];
    let session = SessionSetup {
        mcp_servers: spec.mcp_servers.clone(),
        env_allow: Some(env_allow),
        unset_env: spec.unset_env.clone(),
        decider: Some(decider),
        ..SessionSetup::default()
    };
    Ok(CodexConfig {
        agent: AgentConfig {
            command,
            env,
            workspace: spec.cwd.clone(),
            deadlines: Deadlines::default(),
            max_unread_bytes: sakura_acp::transport::MAX_UNREAD_BYTES,
            asks_beyond_workspace: false,
            session,
        },
        instructions: spec.system_prompt_append.clone(),
        model: None,
        effort: None,
        collaboration: None,
        allowed_tools: allowed_tools(spec),
    })
}

/// The environment Codex runs in for the engine's spec, and the variables of the app's own it keeps. Never the
/// person's own Codex home: it is not passed on, and Sakura's own stands in its place.
fn codex_environment(
    spec: &MasterSpec,
    program: &Path,
    home: &Path,
) -> (Vec<(String, String)>, Vec<String>) {
    let env_allow = spec
        .env_allow
        .iter()
        .filter(|name| name.as_str() != CODEX_HOME_VARIABLE)
        .cloned()
        .collect();
    let mut env = super::master_page::env_of(spec);
    if let Some(path) = codex_path(spec, program) {
        env.retain(|(name, _)| name != "PATH");
        env.push(("PATH".into(), path));
    }
    env.retain(|(name, _)| name != CODEX_HOME_VARIABLE);
    env.push((CODEX_HOME_VARIABLE.to_string(), home.display().to_string()));
    (env, env_allow)
}

fn codex_path(spec: &MasterSpec, program: &Path) -> Option<String> {
    let base = if spec.path.is_empty() {
        spec.env.get("PATH").map_or("", String::as_str)
    } else {
        &spec.path
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let known = sakura_core::engine::cli::known_path(home.as_deref());
    let mut seen = std::collections::HashSet::new();
    // An npm-installed Codex may need Node before app-server can answer.
    let folders = std::env::split_paths(base)
        .chain(Path::new(&spec.node).parent().map(Path::to_path_buf))
        .chain(program.parent().map(Path::to_path_buf))
        .chain(std::env::split_paths(&known))
        .filter(|folder| folder.is_absolute() && seen.insert(folder.clone()));
    std::env::join_paths(folders).ok()?.into_string().ok()
}

/// The problem when no Codex program is found.
pub fn no_codex() -> Problem {
    Problem {
        kind: ProblemKind::NotStarted,
        detail: tr(NO_CODEX).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt as _;

    fn spec() -> MasterSpec {
        MasterSpec {
            acp: None,
            argv: vec!["/opt/acp".into()],
            env: [("SAKURA_ENGINE_HOME".to_string(), "/engine".to_string())].into(),
            cwd: "/engine/master".into(),
            options: json!({ "settings": { "permissions": {
                "allow": ["mcp__sakura__lanes", "mcp__sakura__lane_open", "Read(/x)", "mcp__sakura__*", "mcp__sakura__lane_stop"],
                "deny": ["mcp__sakura__lane_stop", "Bash"],
            } } }),
            mcp_servers: vec![
                json!({ "name": "sakura", "command": "/bin/sakura", "args": [], "env": [] }),
            ],
            start_mode: "default".into(),
            close_sessions: true,
            unset_env: vec!["CLAUDECODE".into()],
            node: String::new(),
            adapter: String::new(),
            adapter_problem: String::new(),
            path: String::new(),
            account: String::new(),
            account_provider: String::new(),
            account_folder: String::new(),
            codex_home: String::new(),
            env_allow: vec!["HOME".into()],
            system_prompt_append: "You are the master.".into(),
        }
    }

    fn program_in(folder: &Path) -> PathBuf {
        let program = folder.join("codex");
        std::fs::write(&program, "#!/bin/sh\n").expect("a program");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("mode");
        program
    }

    #[test]
    fn an_agent_is_remembered_for_its_project_and_any_project_has_its_own() {
        let mut preferences = Preferences::default();
        assert_eq!(
            MasterAgent::remembered(&preferences, None),
            MasterAgent::Claude
        );
        MasterAgent::Codex.remember(&mut preferences, Some("demo"));
        MasterAgent::Claude.remember(&mut preferences, None);
        assert_eq!(
            MasterAgent::remembered(&preferences, Some("demo")),
            MasterAgent::Codex
        );
        assert_eq!(
            MasterAgent::remembered(&preferences, None),
            MasterAgent::Claude
        );
        assert_eq!(
            MasterAgent::remembered(&preferences, Some("other")),
            MasterAgent::Claude
        );
    }

    #[test]
    fn each_agent_keeps_its_own_session_and_history() {
        assert_ne!(
            MasterAgent::Claude.session_name(),
            MasterAgent::Codex.session_name()
        );
        assert_eq!(
            MasterAgent::Claude.session_name(),
            "@master",
            "what the earlier app kept still opens"
        );
        assert_eq!(MasterAgent::Claude.history_folder(), None);
        assert_eq!(
            MasterAgent::from_code(Some("nonsense")),
            MasterAgent::Claude
        );
    }

    #[test]
    fn codex_is_found_by_the_named_path_else_in_the_masters_path_else_in_the_apps() {
        let (named, masters, apps) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let home = tempfile::tempdir().unwrap();
        let (named_program, masters_program, apps_program) = (
            program_in(named.path()),
            program_in(masters.path()),
            program_in(apps.path()),
        );
        let mut spec = spec();
        spec.env.clear();
        spec.path = masters.path().display().to_string();
        let apps_path = Some(apps.path().to_str().unwrap());
        let discover = |spec: &MasterSpec, named, path| {
            find_codex_for_home(spec, named, path, Some(home.path()))
        };
        let by_name = discover(&spec, named_program.to_str(), apps_path);
        assert_eq!(by_name, Some(named_program.clone()));
        assert_eq!(discover(&spec, None, apps_path), Some(masters_program));
        spec.path = String::new();
        assert_eq!(discover(&spec, None, apps_path), Some(apps_program.clone()));
        assert_eq!(discover(&spec, None, None), None);
        let relative = discover(&spec, Some("relative/codex"), apps_path);
        assert_eq!(
            relative,
            Some(apps_program),
            "a name that is not absolute is not taken: the search goes on"
        );
    }

    #[test]
    fn fix25_finder_uses_the_configured_codex_program() {
        let engine = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let program = program_in(folder.path());
        let mut spec = spec();
        spec.path = "/usr/bin:/bin".into();
        spec.env.insert(
            "SAKURA_ENGINE_HOME".into(),
            engine.path().display().to_string(),
        );
        std::fs::write(
            engine.path().join("programs.toml"),
            format!("codex = {:?}\n", program.to_str().unwrap()),
        )
        .unwrap();
        assert_eq!(
            find_codex(&spec, None, Some("/usr/bin:/bin")),
            Some(program)
        );
    }

    #[test]
    fn finder_finds_a_local_install_without_an_override_or_config() {
        let home = tempfile::tempdir().unwrap();
        let folder = home.path().join(".local/bin");
        std::fs::create_dir_all(&folder).unwrap();
        let program = program_in(&folder);
        let mut spec = spec();
        spec.env.clear();
        assert_eq!(
            find_codex_for_home(&spec, None, None, Some(home.path())),
            Some(program)
        );
    }

    #[test]
    fn finder_starts_the_sakura_default_account_with_the_read_only_sandbox() {
        use sakura_acp::connection::{Agent, AgentEvent};

        let engine = tempfile::tempdir().unwrap();
        let mut spec = spec_in(engine.path(), true);
        spec.path = "/usr/bin:/bin".into();
        spec.cwd = engine.path().to_path_buf();
        spec.argv.clear();
        spec.account = "codex-default".into();
        spec.account_provider = "codex".into();
        spec.codex_home = account_home().unwrap().join(".codex").display().to_string();
        let log = engine.path().join("server.log");
        spec.env
            .insert("SAKURA_FAKE_LOG".into(), log.display().to_string());
        let installed = engine.path().join("installed");
        std::fs::create_dir(&installed).unwrap();
        let interpreter = installed.join("sakura-test-interpreter");
        let program = installed.join("codex");
        let server = super::super::master_page_tests::fake_codex();
        sakura_core::test_support::write_script(
            &interpreter,
            &format!(
                "#!/bin/sh\nshift\nexec {:?} \"$@\"\n",
                server.to_str().unwrap()
            ),
        );
        sakura_core::test_support::write_script(
            &program,
            "#!/usr/bin/env sakura-test-interpreter\n",
        );
        std::fs::write(
            engine.path().join("programs.toml"),
            format!("codex = {:?}\n", program.to_str().unwrap()),
        )
        .unwrap();
        let program = find_codex(&spec, None, Some("/usr/bin:/bin")).unwrap();
        let config = codex_config_of(&spec, Some(&program), quiet_decider()).unwrap();
        let (agent, mut events) = Agent::start_codex_resolving(async { Ok(config) });
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    match events.recv().await.unwrap() {
                        AgentEvent::Connected { .. } => agent.open_session(None).unwrap(),
                        AgentEvent::SessionOpened { .. } => break,
                        AgentEvent::Ended(problem) => panic!("Codex ended: {problem:?}"),
                        _ => {}
                    }
                }
            })
            .await
            .unwrap();
        });
        let received = std::fs::read_to_string(log).unwrap();
        let start = received
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|message| message["method"] == "thread/start")
            .unwrap();
        assert_eq!(start["params"]["sandbox"], "read-only");
        assert_eq!(start["params"]["cwd"], spec.cwd.to_str().unwrap());
    }

    #[test]
    fn a_file_that_cannot_run_is_not_codex() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("codex"), "x").unwrap();
        let mut spec = spec();
        spec.env.clear();
        spec.path = folder.path().display().to_string();
        assert_eq!(
            find_codex_for_home(&spec, None, None, Some(folder.path())),
            None
        );
    }

    #[test]
    fn codex_is_given_the_tools_the_engine_allows_less_those_it_denies_and_no_pattern() {
        assert_eq!(
            allowed_tools(&spec()),
            ["mcp__sakura__lanes", "mcp__sakura__lane_open"]
        );
    }

    fn tools_under(allow: &[&str], deny: &[&str]) -> Vec<String> {
        let mut spec = spec();
        spec.options = json!({ "settings": { "permissions": { "allow": allow, "deny": deny } } });
        allowed_tools(&spec)
    }

    const ALL_TOOLS: [&str; 3] = [
        "mcp__sakura__lanes",
        "mcp__sakura__lane_open",
        "mcp__other__read",
    ];

    #[test]
    fn a_deny_rule_of_a_servers_name_removes_all_its_tools() {
        assert_eq!(
            tools_under(&ALL_TOOLS, &["mcp__sakura"]),
            ["mcp__other__read"]
        );
        assert_eq!(
            tools_under(&ALL_TOOLS, &["mcp__sakura__lanes"]),
            ["mcp__sakura__lane_open", "mcp__other__read"]
        );
        assert_eq!(
            tools_under(&ALL_TOOLS, &["mcp__sak"]).len(),
            3,
            "a part of a name is no name"
        );
    }

    #[test]
    fn a_deny_rule_with_a_glob_removes_what_it_matches() {
        assert_eq!(
            tools_under(&ALL_TOOLS, &["mcp__sakura__lane*"]),
            ["mcp__other__read"]
        );
        assert_eq!(
            tools_under(&ALL_TOOLS, &["*__read"]),
            ["mcp__sakura__lanes", "mcp__sakura__lane_open"]
        );
        assert!(tools_under(&ALL_TOOLS, &["*"]).is_empty());
    }

    #[test]
    fn a_deny_rule_that_cannot_be_read_removes_every_tool_and_one_for_another_tool_removes_none() {
        for rule in [
            "mcp__sakura__lanes (",
            "",
            "mcp sakura",
            "Bash(rm -rf /)x(",
            "[x]",
        ] {
            assert!(tools_under(&ALL_TOOLS, &[rule]).is_empty(), "{rule:?}");
        }
        assert_eq!(
            tools_under(&ALL_TOOLS, &["Bash", "Read(/x)", "WebFetch(domain:x.org)"]).len(),
            3
        );
        assert!(tools_under(&ALL_TOOLS, &["mcp__sakura__lanes(any)"])
            .iter()
            .all(|tool| tool != "mcp__sakura__lanes"));
    }

    /// A spec whose engine home is `engine`, with Sakura's Codex login there when `is_logged_in`.
    fn spec_in(engine: &Path, is_logged_in: bool) -> MasterSpec {
        let mut spec = spec();
        spec.env
            .insert("SAKURA_ENGINE_HOME".into(), engine.display().to_string());
        if is_logged_in {
            let home = engine.join(CODEX_HOME_FOLDER);
            std::fs::create_dir_all(&home).expect("a home");
            std::fs::write(home.join("auth.json"), "{}").expect("a login");
        }
        spec
    }

    fn quiet_decider() -> Decider {
        Decider::new(|_| Box::pin(async { sakura_acp::policy::decider::Verdict::Ask }))
    }

    #[test]
    fn the_spec_becomes_codex_s_configuration_as_the_engine_wrote_it() {
        let engine = tempfile::tempdir().expect("a folder");
        let spec = spec_in(engine.path(), true);
        let config = codex_config_of(&spec, Some(Path::new("/opt/codex")), quiet_decider())
            .expect("a configuration");
        assert_eq!(config.agent.command, ["/opt/codex", "app-server"]);
        assert_eq!(config.agent.workspace, Path::new("/engine/master"));
        assert_eq!(config.instructions, "You are the master.");
        assert_eq!(config.model, None, "a Claude model name is not Codex's");
        let session = &config.agent.session;
        assert_eq!(session.mcp_servers.len(), 1);
        assert!(session.decider.is_some(), "the engine decides");
        assert_eq!(session.env_allow, Some(vec!["HOME".to_string()]));
        assert_eq!(session.unset_env, ["CLAUDECODE"]);
    }

    /// Owner decision (20/11): a Master conversation on Codex stays in Sakura. Its Codex home is Sakura's own, never
    /// the person's, so no thread of it can show in their Codex or ChatGPT app.
    #[test]
    fn the_engine_s_lists_replace_a_kept_list_that_has_no_efforts_and_never_one_that_has_them() {
        let rules: sakura_core::engine::rules::Rules = serde_json::from_value(json!({
            "preset": "standard",
            "agents": [{"id": "a", "label": "Claude", "provider": "claude", "models": [
                {"id": "sonnet", "label": "Sonnet 5.5", "efforts": ["low", "high"]}]}]
        }))
        .expect("rules");
        let bare = vec![Choice {
            id: "sonnet".into(),
            name: "Sonnet 5.5".into(),
            ..Choice::default()
        }];
        let mut kept = std::collections::BTreeMap::from([("claude".to_string(), bare)]);
        take_in_catalogue(&mut kept, &rules);
        assert_eq!(kept["claude"][0].efforts.len(), 2);
        let own = kept["claude"].clone();
        kept.get_mut("claude").unwrap()[0].efforts.pop();
        let before = kept.clone();
        take_in_catalogue(&mut kept, &rules);
        assert_eq!(kept, before, "a list with efforts stays");
        let _ = own;
    }

    #[test]
    fn codex_runs_in_a_home_of_sakura_s_own_and_never_the_persons() {
        let engine = tempfile::tempdir().expect("a folder");
        let spec = spec_in(engine.path(), true);
        let config = codex_config_of(&spec, Some(Path::new("/opt/codex")), quiet_decider())
            .expect("a configuration");
        let home = config
            .agent
            .env
            .iter()
            .find(|(name, _)| name == "CODEX_HOME")
            .map(|(_, home)| home.clone());
        assert_eq!(
            home,
            Some(engine.path().join(CODEX_HOME_FOLDER).display().to_string())
        );
        let allowed = config.agent.session.env_allow.clone().unwrap_or_default();
        assert!(
            !allowed.contains(&"CODEX_HOME".to_string()),
            "the person's own is not passed on"
        );
    }

    #[test]
    fn a_codex_account_runs_in_the_home_the_engine_names_for_it() {
        let engine = tempfile::tempdir().expect("a folder");
        let mut spec = spec_in(engine.path(), false);
        let named = engine.path().join("accounts").join("codex-b");
        std::fs::create_dir_all(&named).expect("a home");
        std::fs::write(named.join("auth.json"), "{}").expect("a login");
        spec.codex_home = named.display().to_string();
        let config = codex_config_of(&spec, Some(Path::new("/opt/codex")), quiet_decider())
            .expect("a configuration");
        let home = config
            .agent
            .env
            .iter()
            .find(|(name, _)| name == "CODEX_HOME");
        assert_eq!(home.map(|(_, home)| home.clone()), Some(spec.codex_home));
    }

    #[test]
    fn the_persons_own_codex_home_is_never_run_in_even_when_the_engine_names_it() {
        let Some(own) = account_home().map(|home| home.join(".codex")) else {
            return;
        };
        let engine = tempfile::tempdir().expect("a folder");
        let mut spec = spec_in(engine.path(), true);
        spec.codex_home = own.display().to_string();
        let home = sakura_codex_home(&spec).expect("Sakura's own stands in its place");
        assert!(!is_the_persons_codex_home(&home), "{home:?}");
    }

    #[test]
    fn the_default_account_whose_folder_is_the_persons_own_starts_on_the_login_in_sakura_s_home() {
        let Some(own) = account_home().map(|home| home.join(".codex")) else {
            return;
        };
        let engine = tempfile::tempdir().expect("a folder");
        let mut spec = spec_in(engine.path(), true);
        spec.codex_home = own.display().to_string();
        assert!(has_codex_login(&spec));
        let config = codex_config_of(&spec, Some(Path::new("/opt/codex")), quiet_decider())
            .expect("a configuration");
        let home = config
            .agent
            .env
            .iter()
            .find(|(name, _)| name == "CODEX_HOME");
        assert_eq!(
            home.map(|(_, home)| home.clone()),
            Some(engine.path().join(CODEX_HOME_FOLDER).display().to_string())
        );
    }

    #[test]
    fn the_default_account_without_a_login_in_sakura_s_home_asks_for_the_login() {
        let Some(own) = account_home().map(|home| home.join(".codex")) else {
            return;
        };
        let engine = tempfile::tempdir().expect("a folder");
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let mut spec = spec_in(engine.path(), false);
        spec.codex_home = own.display().to_string();
        assert!(!has_codex_login(&spec));
        let refused = codex_config_of(&spec, Some(Path::new("/opt/codex")), quiet_decider())
            .expect_err("no login yet");
        assert!(
            refused.detail.contains("own login for Sakura"),
            "{}",
            refused.detail
        );
    }

    #[test]
    fn the_guard_reads_the_password_database_and_not_home_so_another_home_does_not_let_the_real_one_through(
    ) {
        let account = account_home().expect("the account's home");
        assert!(account.is_absolute());
        // Whatever $HOME says, the folder of the account is refused: the guard never asks for $HOME.
        assert!(is_codex_home_of(&account.join(".codex"), &account));
        let other = tempfile::tempdir().expect("a folder");
        assert!(!is_codex_home_of(&other.path().join(".codex"), &account));
    }

    #[test]
    fn a_link_or_a_dot_dot_to_the_persons_codex_home_is_the_same_home_by_its_real_path() {
        let root = tempfile::tempdir().expect("a folder");
        let account = root.path().join("person");
        std::fs::create_dir_all(account.join(".codex")).expect("the home");
        std::fs::create_dir_all(account.join("x")).expect("a folder");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&account, &link).expect("a link");
        assert!(is_codex_home_of(&link.join(".codex"), &account));
        assert!(is_codex_home_of(&account.join("x/../.codex"), &account));
        let not_yet = account.join(".codex-2");
        assert!(!is_codex_home_of(&not_yet, &account));
    }

    #[test]
    fn until_sakura_s_codex_home_has_a_login_codex_does_not_start_and_says_why() {
        let engine = tempfile::tempdir().expect("a folder");
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let spec = spec_in(engine.path(), false);
        let refused = codex_config_of(&spec, Some(Path::new("/opt/codex")), quiet_decider())
            .expect_err("no login yet");
        assert!(
            refused.detail.contains("own login for Sakura"),
            "{}",
            refused.detail
        );
        assert!(!has_codex_login(&spec));
    }

    #[test]
    fn the_login_runs_the_programs_app_server_in_sakura_s_home_and_not_in_a_terminal() {
        let engine = tempfile::tempdir().expect("a folder");
        let spec = spec_in(engine.path(), false);
        let home = engine.path().join(CODEX_HOME_FOLDER);
        let config = codex_login_config(&spec, Path::new("/opt/codex"), &home);
        assert_eq!(config.command, ["/opt/codex", "app-server"]);
        let homes: Vec<&String> = config
            .env
            .iter()
            .filter(|(name, _)| name == "CODEX_HOME")
            .map(|(_, home)| home)
            .collect();
        assert_eq!(homes, [&home.display().to_string()]);
        let allowed = config.env_allow.unwrap_or_default();
        assert!(!allowed.contains(&"CODEX_HOME".to_string()));
    }

    #[test]
    fn a_login_that_did_not_work_is_said_in_words_and_one_that_did_says_nothing() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        assert_eq!(login_end_words(&LoginEnd::Done), None);
        let said = |end: LoginEnd| login_end_words(&end).expect("words");
        assert!(said(LoginEnd::Cancelled).contains("cancelled"));
        assert!(said(LoginEnd::Failed(LoginFailure::TimedOut)).contains("in time"));
        assert!(said(LoginEnd::Failed(LoginFailure::Broke)).contains("ended"));
        let declined = said(LoginEnd::Failed(LoginFailure::Declined(Some(
            "it closed".into(),
        ))));
        assert!(declined.contains("it closed") && declined.contains("did not finish"));
        assert!(said(LoginEnd::Failed(LoginFailure::Declined(None))).contains("did not finish"));
        assert!(said(LoginEnd::Failed(LoginFailure::Refused("no".into()))).contains("no"));
        assert!(said(LoginEnd::Failed(LoginFailure::NotStarted("gone".into()))).contains("gone"));
    }

    fn names(menu: &AgentMenu) -> Vec<(String, Vec<String>)> {
        let entries =
            |group: &MenuGroup| group.entries.iter().map(|e| e.name.to_string()).collect();
        let titled = |group: &MenuGroup| {
            (
                group
                    .title
                    .as_ref()
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
                entries(group),
            )
        };
        menu.groups.iter().map(titled).collect()
    }

    /// The models the person's Claude Code lists, as the adapter's model option names them.
    fn claude_models() -> Vec<Choice> {
        let model = |id: &str, name: &str| Choice {
            id: id.into(),
            name: name.into(),
            ..Choice::default()
        };
        [
            ("opus", "Opus 5.5"),
            ("fable", "Fable 5.2"),
            ("sonnet", "Sonnet 5.5"),
            ("haiku", "Haiku 5.5"),
            ("fable-5.1", "Fable 5.1"),
            ("haiku-4.5", "Haiku 4.5"),
            ("sonnet-5", "Sonnet 5"),
            ("opus-5", "Opus 5"),
            ("fable-5", "Fable 5"),
            ("opus-4.8", "Opus 4.8"),
            ("opus-4.7", "Opus 4.7"),
            ("opus-4.6", "Opus 4.6"),
            ("sonnet-4.6", "Sonnet 4.6"),
        ]
        .iter()
        .map(|(id, name)| model(id, name))
        .collect()
    }

    /// The models the person's Codex lists: each accepts efforts, the first and a few others a fast tier, and one
    /// legacy model is hidden.
    fn codex_models() -> Vec<Choice> {
        let efforts = |ids: &[&str]| {
            ids.iter()
                .map(|id| Choice {
                    id: id.to_string(),
                    name: id.to_string(),
                    ..Choice::default()
                })
                .collect::<Vec<_>>()
        };
        let model = |id: &str, name: &str, is_hidden: bool| Choice {
            id: id.into(),
            name: name.into(),
            is_hidden,
            efforts: efforts(&["low", "medium", "high", "xhigh", "max", "ultra"]),
            default_effort: Some("low".into()),
            tiers: vec![Choice {
                id: "priority".into(),
                name: "Fast".into(),
                ..Choice::default()
            }],
            ..Choice::default()
        };
        [
            ("gpt-6.2-sol", "GPT-6.2-Sol", false),
            ("gpt-6.2-astra", "GPT-6.2-Astra", false),
            ("gpt-6.1-terra", "GPT-6.1 Terra", false),
            ("gpt-6.1-sol", "GPT-6.1-Sol", false),
            ("gpt-6-astra", "GPT-6-Astra", false),
            ("gpt-6-sol", "GPT-6-Sol", false),
            ("gpt-6-luna", "GPT-6-Luna", false),
            ("gpt-5.6-sol", "GPT-5.6-Sol", false),
            ("gpt-5.6-terra", "GPT-5.6-Terra", false),
            ("gpt-5.6-luna", "GPT-5.6-Luna", false),
            ("gpt-reserve", "GPT-Reserve", true),
        ]
        .iter()
        .map(|(id, name, hidden)| model(id, name, *hidden))
        .collect()
    }

    #[test]
    fn claudes_models_come_from_the_agent_the_newest_of_each_family_first_and_the_older_under_more_models(
    ) {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let menu = agent_menu(
            MasterAgent::Claude,
            &StartChoice::default(),
            &claude_models(),
            &SessionChoices::default(),
        );
        let listed = names(&menu);
        assert_eq!(
            listed[1].1,
            [
                "Default (recommended)",
                "Opus 5.5",
                "Fable 5.2",
                "Sonnet 5.5",
                "Haiku 5.5"
            ]
        );
        assert_eq!(listed[2].0, "More models");
        assert_eq!(
            listed[2].1,
            [
                "Fable 5.1",
                "Sonnet 5",
                "Opus 5",
                "Fable 5",
                "Opus 4.8",
                "Opus 4.7",
                "Opus 4.6",
                "Sonnet 4.6",
                "Haiku 4.5"
            ],
            "all nine older ones are listed, the newest version first, the agent's own order within a version"
        );
    }

    #[test]
    fn codexs_models_come_from_the_server_with_the_hidden_one_under_more_models() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let menu = agent_menu(
            MasterAgent::Codex,
            &StartChoice::default(),
            &codex_models(),
            &SessionChoices::default(),
        );
        let listed = names(&menu);
        assert_eq!(listed[1].1.len(), 10);
        assert_eq!(listed[1].1[0], "GPT-6.2-Sol");
        assert_eq!(
            listed[2],
            ("More models".to_string(), vec!["GPT-Reserve".to_string()])
        );
        let all: Vec<String> = listed
            .iter()
            .flat_map(|(_, entries)| entries.clone())
            .collect();
        assert!(
            all.iter().all(|name| !name.contains("5.5")),
            "no hand-written model: {all:?}"
        );
    }

    #[test]
    fn with_no_list_yet_the_menu_offers_the_agents_own_model_and_never_an_invented_list() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        for agent in [MasterAgent::Claude, MasterAgent::Codex] {
            let menu = agent_menu(
                agent,
                &StartChoice::default(),
                &[],
                &SessionChoices::default(),
            );
            assert_eq!(names(&menu)[1].1, ["Default"], "{agent:?}");
            assert_eq!(
                menu.groups[1].entries[0].pick,
                MenuPick::Model(String::new())
            );
            assert_eq!(
                menu.name, "",
                "the chip says no name, neither the program's nor a Default nobody picked: {agent:?}"
            );
        }
    }

    #[test]
    fn with_no_list_yet_the_chip_says_the_model_saved_with_the_conversation_and_its_effort() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let saved = StartChoice {
            model: Some("sonnet".into()),
            effort: Some("low".into()),
            ..StartChoice::default()
        };
        let menu = agent_menu(MasterAgent::Claude, &saved, &[], &SessionChoices::default());
        assert_eq!(menu.name, "Sonnet Low");
        let tier = agent_menu(
            MasterAgent::Claude,
            &StartChoice {
                model: Some("opus".into()),
                ..StartChoice::default()
            },
            &[],
            &SessionChoices::default(),
        );
        assert_eq!(
            tier.name, "Opus",
            "the model selected for the first session"
        );
    }

    #[test]
    fn the_efforts_of_the_chosen_model_come_under_the_models_with_its_default_marked_and_the_chip_says_them(
    ) {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let start = StartChoice {
            model: Some("gpt-6.2-sol".into()),
            effort: Some("high".into()),
            ..StartChoice::default()
        };
        let menu = agent_menu(
            MasterAgent::Codex,
            &start,
            &codex_models(),
            &SessionChoices::default(),
        );
        assert_eq!(menu.name, "GPT-6.2-Sol High");
        let efforts = menu
            .groups
            .iter()
            .find(|group| group.title.as_deref() == Some("Effort"))
            .expect("the effort row");
        let words: Vec<String> = efforts
            .entries
            .iter()
            .map(|entry| entry.name.to_string())
            .collect();
        assert_eq!(
            words,
            [
                "Low (default)",
                "Medium",
                "High",
                "Extra high",
                "Max",
                "Ultra"
            ]
        );
        assert_eq!(
            efforts.entries.iter().filter(|entry| entry.is_on).count(),
            1
        );
        assert!(efforts.entries[2].is_on);
        // An effort the chosen model does not accept is not the one in force: its own default is.
        let none = StartChoice {
            model: Some("gpt-6.2-sol".into()),
            effort: Some("bogus".into()),
            ..StartChoice::default()
        };
        assert_eq!(
            agent_menu(
                MasterAgent::Codex,
                &none,
                &codex_models(),
                &SessionChoices::default()
            )
            .name,
            "GPT-6.2-Sol Low"
        );
    }

    #[test]
    fn every_other_option_the_agent_sends_is_in_the_menu_in_words() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let switch = Selector {
            name: "Fast".into(),
            config_id: Some("service_tier".into()),
            current: "false".into(),
            is_toggle: true,
            choices: vec![
                Choice {
                    id: "false".into(),
                    name: "Off".into(),
                    ..Choice::default()
                },
                Choice {
                    id: "true".into(),
                    name: "On".into(),
                    ..Choice::default()
                },
            ],
        };
        let style = Selector {
            name: "Output style".into(),
            config_id: Some("style".into()),
            current: "plain".into(),
            is_toggle: false,
            choices: vec![
                Choice {
                    id: "plain".into(),
                    name: "Plain".into(),
                    ..Choice::default()
                },
                Choice {
                    id: "terse".into(),
                    name: "Terse".into(),
                    ..Choice::default()
                },
            ],
        };
        let session = SessionChoices {
            others: vec![switch, style],
            ..SessionChoices::default()
        };
        let menu = agent_menu(
            MasterAgent::Codex,
            &StartChoice::default(),
            &codex_models(),
            &session,
        );
        let fast = menu
            .groups
            .iter()
            .flat_map(|group| &group.entries)
            .find(|entry| entry.name == "Fast")
            .expect("the switch");
        assert_eq!(
            fast.pick,
            MenuPick::Option {
                config_id: "service_tier".into(),
                value: "true".into()
            },
            "pressing turns it on"
        );
        let styles = menu
            .groups
            .iter()
            .find(|group| group.title.as_deref() == Some("Output style"))
            .expect("the select");
        assert_eq!(
            styles
                .entries
                .iter()
                .map(|e| e.name.to_string())
                .collect::<Vec<_>>(),
            ["Plain", "Terse"]
        );
    }

    #[test]
    fn the_mode_is_visible_before_the_catalogue_and_checks_the_current_mode() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        for agent in [MasterAgent::Claude, MasterAgent::Codex] {
            let menu = agent_menu(
                agent,
                &StartChoice::default(),
                &claude_models(),
                &SessionChoices::default(),
            );
            let mode = menu.groups.first().expect("a group");
            assert_eq!(mode.title.as_deref(), Some("Mode"));
            assert!(mode.entries[0].is_on && !mode.entries[1].is_on);
            assert!(
                mode.entries[0]
                    .name
                    .starts_with("Edit: acts on what you ask"),
                "{}",
                mode.entries[0].name
            );
            assert!(
                mode.entries[0].name.len() <= 32,
                "the Edit line fits the menu's width"
            );
            assert_eq!(
                mode.entries[1].name,
                "Plan: reads and proposes, changes nothing"
            );
        }
        let plan = StartChoice {
            mode: Some("plan".into()),
            ..StartChoice::default()
        };
        let menu = agent_menu(
            MasterAgent::Claude,
            &StartChoice {
                model: Some("opus".into()),
                ..plan
            },
            &claude_models(),
            &SessionChoices {
                modes: Some(Selector {
                    current: "plan".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert_eq!(menu.name, "Opus 5.5, plan");
        let mode = &menu.groups[0];
        assert_eq!(mode.title.as_deref(), Some("Mode"));
        assert!(!mode.entries[0].is_on && mode.entries[1].is_on);
    }

    #[test]
    fn the_kept_list_round_trips_through_the_preferences() {
        let kept = models_from_json(&models_to_json(&codex_models()));
        assert_eq!(
            kept,
            codex_models()
                .into_iter()
                .map(|model| Choice {
                    tiers: model.tiers,
                    ..model
                })
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_menu_words_exist_in_italian_too() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let models = codex_models();
        let start = StartChoice {
            model: Some(models[0].id.clone()),
            ..StartChoice::default()
        };
        let menu = agent_menu(
            MasterAgent::Claude,
            &start,
            &models,
            &SessionChoices::default(),
        );
        let titles: Vec<String> = menu
            .groups
            .iter()
            .filter_map(|g| g.title.as_ref().map(|t| t.to_string()))
            .collect();
        assert!(
            titles.contains(&"Altri modelli".to_string())
                && titles.contains(&"Impegno".to_string())
                && titles.contains(&"Modalità".to_string()),
            "{titles:?}"
        );
        assert!(menu.groups[0].entries[1].name.starts_with("Piano"));
    }

    /// One group of a menu as it reads: its title and its entries with their marks and picks.
    type Shown = (Option<String>, Vec<(String, bool, MenuPick)>);

    /// The groups a menu holds, to compare menus.
    fn shown(groups: &[MenuGroup]) -> Vec<Shown> {
        groups
            .iter()
            .map(|group| {
                let entries = group
                    .entries
                    .iter()
                    .map(|e| (e.name.to_string(), e.is_on, e.pick.clone()))
                    .collect();
                (group.title.as_ref().map(|title| title.to_string()), entries)
            })
            .collect()
    }

    /// One component: the master's chip and an Engine page step's menu are built from the same data by the same
    /// code, so the same models and effort read the same in both; the step's adds the account, the chip's the mode.
    #[test]
    fn the_adapter_s_own_modes_never_reach_the_menu_only_edit_and_plan_do() {
        let modes = Selector {
            current: "default".into(),
            choices: [
                "default",
                "acceptEdits",
                "plan",
                "auto",
                "bypassPermissions",
            ]
            .iter()
            .map(|id| Choice {
                id: (*id).into(),
                name: (*id).into(),
                ..Choice::default()
            })
            .collect(),
            ..Selector::default()
        };
        let session = SessionChoices {
            modes: Some(modes),
            ..SessionChoices::default()
        };
        let menu = agent_menu(MasterAgent::Claude, &StartChoice::default(), &[], &session);
        let titled: Vec<_> = menu
            .groups
            .iter()
            .filter(|group| group.title.as_deref() == Some("Mode"))
            .collect();
        assert_eq!(titled.len(), 1, "one Mode group, the app's");
        let picks: Vec<_> = titled[0].entries.iter().map(|entry| &entry.pick).collect();
        assert_eq!(
            picks,
            [
                &MenuPick::Mode("default".into()),
                &MenuPick::Mode("plan".into())
            ]
        );
    }

    #[test]
    fn the_masters_chip_and_a_steps_menu_list_the_same_entries_for_the_same_data() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let models = codex_models();
        let model = models[0].id.clone();
        let effort = models[0]
            .default_effort
            .clone()
            .unwrap_or_else(|| "high".into());
        let start = StartChoice {
            model: Some(model.clone()),
            effort: Some(effort.clone()),
            ..StartChoice::default()
        };
        let master = agent_menu(
            MasterAgent::Codex,
            &start,
            &models,
            &SessionChoices::default(),
        );
        let accounts = [("a".to_string(), "Codex".to_string())];
        let step = step_menu_groups(&accounts, "a", &models, Some(&model), Some(&effort));
        let (master, step) = (shown(&master.groups), shown(&step));
        let without = |all: &[Shown], title: &str| {
            all.iter()
                .filter(|(t, _)| t.as_deref() != Some(title))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            without(&without(&master, "Mode"), "Account"),
            without(&without(&step, "Mode"), "Account"),
            "the models and the effort are the same entries"
        );
        let account = step
            .iter()
            .find(|(title, _)| title.as_deref() == Some("Account"))
            .expect("an account group");
        assert_eq!(account.1.len(), 1);
        assert_eq!(account.1[0].0, "Codex");
        assert!(account.1[0].1, "the step's account is marked");
    }
}

#[cfg(test)]
#[test]
fn fix36_the_master_menu_offers_both_defaults_before_and_after_a_session() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let choice = |id: &str, name: &str| Choice {
        id: id.into(),
        name: name.into(),
        ..Choice::default()
    };
    let mut haiku = choice("haiku", "Haiku");
    haiku.efforts = vec![choice("high", "High")];
    let start = StartChoice {
        model: Some("haiku".into()),
        uses_session_effort: true,
        ..StartChoice::default()
    };
    let session = SessionChoices {
        models: Some(Selector {
            current: "haiku".into(),
            choices: vec![choice("default", "Default (recommended)"), haiku.clone()],
            ..Selector::default()
        }),
        efforts: Some(Selector {
            current: "default".into(),
            choices: vec![choice("default", "Default"), choice("high", "High")],
            ..Selector::default()
        }),
        ..SessionChoices::default()
    };
    let entries = |choices: &SessionChoices| {
        agent_menu(
            MasterAgent::Claude,
            &start,
            std::slice::from_ref(&haiku),
            choices,
        )
        .groups
        .into_iter()
        .flat_map(|group| group.entries)
        .map(|entry| (entry.name.to_string(), entry.is_on))
        .collect::<Vec<_>>()
    };
    let before = entries(&SessionChoices::default());
    assert!(
        before.contains(&("Default (recommended)".into(), false)),
        "{before:?}"
    );
    assert!(before.contains(&("Default".into(), true)), "{before:?}");
    assert_eq!(before, entries(&session));
}

#[test]
fn an_engine_named_model_is_checked_before_its_catalogue_arrives() {
    let chosen = StartChoice {
        model: Some("Opus 5.5".into()),
        ..StartChoice::default()
    };
    let menu = agent_menu(
        MasterAgent::Claude,
        &chosen,
        &[],
        &SessionChoices::default(),
    );
    let picked: Vec<_> = menu.groups[1]
        .entries
        .iter()
        .filter(|entry| entry.is_on)
        .map(|entry| entry.name.as_ref())
        .collect();
    assert_eq!(picked, [menu.name.as_ref()]);
}
