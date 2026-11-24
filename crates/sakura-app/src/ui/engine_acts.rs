//! The controls of the engine itself, as the Engine page and Settings press them: the pace, the rules of a
//! lane, the keeper, the connectors, the accounts. Every one is an action of `sakura cockpit act` the app
//! names itself (the engine lists the actions of waits, lanes, projects and accounts, but not these), sent
//! to the engine's fixed target. The engine decides; this keeps only the conversation of one press: sent,
//! asked to confirm, told what went wrong, or done.

use super::chrome::text;
use super::settings_modal::controls;
use crate::data::engine_facts::{EngineFacts, Refreshed};
use crate::data::source::Source;
use crate::l10n::{say, tr, tr_with, Key};
use crate::theme::{self, Palette};
use gpui::{
    div, AnyElement, App, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Styled as _,
};
use sakura_core::engine::cli::{Act, ActResult};
use sakura_core::engine::snapshot::Action;
use sakura_core::error::BridgeResult;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

/// How long a stepper waits for another press before the value it shows is sent: a run of presses is one act.
pub const COALESCE_PAUSE: std::time::Duration = std::time::Duration::from_millis(400);
/// The engine's always-on service that keeps lanes running without the app.
pub const KEEPER_SERVICE: &str = "keeper";
/// The target of every control that belongs to the engine as a whole: a fixed key, never a path.
pub const ENGINE_TARGET: &str = r#"["host","engine"]"#;
/// The target of "Digest now".
pub const DIGEST_TARGET: &str = r#"["host","digest"]"#;
/// The target of the always-on services (the keeper, the phone page).
pub const SERVICES_TARGET: &str = r#"["host","services"]"#;
pub const CONNECTORS_TARGET: &str = r#"["host","connectors"]"#;
/// The target of the morning digest's settings and sources.
pub const DIGEST_SETTINGS_TARGET: &str = r#"["host","digest-settings"]"#;
pub const AUTOMATIONS_TARGET: &str = r#"["host","automations"]"#;

/// One control of `sakura cockpit act` as the app sends it: the key, the fixed target it goes to (none for a
/// lane's or an account's own, which comes from the snapshot's row) and the inputs it names. The engine lists
/// the same in its snapshot, and `engine_contract_tests` holds this table to that listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Control {
    pub key: &'static str,
    pub target: Option<&'static str>,
    pub needs: &'static [&'static str],
    pub optional: &'static [&'static str],
}

impl Control {
    /// A call of the control on its fixed target. Panics when the control has none: that is a bug of the
    /// caller, which must say a target.
    pub fn act(&self, pairs: &[(&str, &str)]) -> Act {
        let target = self.target.expect("a control with a fixed target");
        self.act_on(target, pairs)
    }

    /// A call of the control on `target`, without a confirmation yet.
    pub fn act_on(&self, target: &str, pairs: &[(&str, &str)]) -> Act {
        Act {
            action: Action {
                key: self.key.to_string(),
                label: String::new(),
                needs: self.needs.iter().map(|name| name.to_string()).collect(),
                optional: self.optional.iter().map(|name| name.to_string()).collect(),
                confirm: false,
            },
            target: target.to_string(),
            inputs: inputs(pairs),
            confirm: None,
        }
    }
}

const fn control(
    key: &'static str,
    target: Option<&'static str>,
    needs: &'static [&'static str],
    optional: &'static [&'static str],
) -> Control {
    Control {
        key,
        target,
        needs,
        optional,
    }
}

/// The pace of the whole engine (`level`).
pub const SET_PACE: Control = control("Q", Some(ENGINE_TARGET), &["level"], &[]);
/// A rule of the flow (`rule`, `value`).
pub const SET_RULE: Control = control("K", Some(ENGINE_TARGET), &["rule", "value"], &[]);
/// The preset the flow uses (`preset`).
pub const USE_PRESET: Control = control("U", Some(ENGINE_TARGET), &["preset"], &[]);
/// Saves the rules in force as a new preset (`name`).
pub const SAVE_PRESET: Control = control("V", Some(ENGINE_TARGET), &["name"], &[]);
/// Asks every account's program for its models again; a lane never waits for it.
pub const REFRESH_MODELS: Control = control("M", Some(ENGINE_TARGET), &[], &[]);
/// Gives priority to the person's own sessions (`on`).
pub const SET_PRIORITY: Control = control("Y", Some(ENGINE_TARGET), &["on"], &[]);
/// Lets a device of the Mac's own overlay network open the phone page (`device`, its id).
pub const ALLOW_DEVICE: Control = control("F", Some(ENGINE_TARGET), &["device"], &[]);
/// Stops a device from opening the phone page (`device`).
pub const REMOVE_DEVICE: Control = control("Z", Some(ENGINE_TARGET), &["device"], &[]);
/// Turns a scheduled automation on or off (`name`, `on`).
pub const SET_AUTOMATION: Control = control("A", Some(AUTOMATIONS_TARGET), &["name", "on"], &[]);
/// Changes settings of the morning digest, each by the input of its own name.
pub const SET_DIGEST: Control = control(
    "I",
    Some(DIGEST_SETTINGS_TARGET),
    &[],
    &["at", "lookback_hours", "language", "model", "account"],
);
/// Turns one of the digest's sources on or off (`name`, `on`).
pub const SET_DIGEST_SOURCE: Control =
    control("J", Some(DIGEST_SETTINGS_TARGET), &["name", "on"], &[]);
/// Runs the morning digest now.
pub const RUN_DIGEST: Control = control("G", Some(DIGEST_TARGET), &[], &[]);
/// Turns one step of one lane on or off, on the lane's own target (`step`, `state`: `on` or `off`).
pub const SET_LANE_STEP: Control = control("t", None, &["state", "step"], &[]);
/// The pace of one lane, on the lane's own target (`level`).
pub const SET_LANE_PACE: Control = control("q", None, &["level"], &[]);
/// Turns an always-on service, the keeper or the phone page, on or off (`name`, `on`).
pub const SET_SERVICE: Control = control("B", Some(SERVICES_TARGET), &["name", "on"], &[]);
/// Connects a connector of `kind`; a feed or a program also takes its `label` and its `address`.
pub const CONNECT: Control = control(
    "O",
    Some(CONNECTORS_TARGET),
    &["kind"],
    &["label", "address"],
);
/// Disconnects a connector (`name`, its id).
pub const DISCONNECT: Control = control("o", Some(CONNECTORS_TARGET), &["name"], &[]);
/// Renames an account, on the account's own target (`label`).
pub const RENAME_ACCOUNT: Control = control("l", None, &["label"], &[]);
/// Removes an account from the book, on the account's own target; the engine asks first and refuses an
/// account a step is pinned to or a pace allow-list holds.
pub const REMOVE_ACCOUNT: Control = control("X", None, &[], &[]);
/// Makes an empty login folder for an account of a program (`provider`) and answers the command that logs in to it.
pub const MAKE_LOGIN_FOLDER: Control = control("g", Some(ENGINE_TARGET), &["provider"], &[]);
/// Adds a login folder to the account book under a name (`folder`, `label`, `provider`).
pub const ADD_ACCOUNT: Control = control(
    "j",
    Some(ENGINE_TARGET),
    &["folder", "label", "provider"],
    &[],
);
/// The two phone page controls: the engine's table has them, but its snapshot does not list them under the engine
/// yet (`host_actions` admits only `KUVQY` there), so the contract test holds them to the table alone.
#[cfg(test)]
pub const PHONE_CONTROLS: [Control; 2] = [ALLOW_DEVICE, REMOVE_DEVICE];
/// The two controls that add an account, held to the engine's table alone for the same reason.
#[cfg(test)]
pub const ACCOUNT_CONTROLS: [Control; 2] = [MAKE_LOGIN_FOLDER, ADD_ACCOUNT];
/// The control that refreshes the models, held to the engine's table alone for the same reason.
#[cfg(test)]
pub const MODEL_CONTROLS: [Control; 1] = [REFRESH_MODELS];
/// The lane step switch, held to the engine's table alone: its snapshot fixture is older than the action.
#[cfg(test)]
pub const LANE_STEP_CONTROLS: [Control; 1] = [SET_LANE_STEP];
/// Every control the app sends, for the test that holds them to the engine's own listing.
#[cfg(test)]
pub const ALL_CONTROLS: [Control; 15] = [
    SET_PACE,
    SET_RULE,
    USE_PRESET,
    SAVE_PRESET,
    SET_PRIORITY,
    SET_AUTOMATION,
    SET_DIGEST,
    SET_DIGEST_SOURCE,
    RUN_DIGEST,
    SET_LANE_PACE,
    SET_SERVICE,
    CONNECT,
    DISCONNECT,
    RENAME_ACCOUNT,
    REMOVE_ACCOUNT,
];

/// An on or off input of the engine's switches (`on`), as it takes it: `yes` or `no`.
pub fn on(is_on: bool) -> &'static str {
    if is_on {
        "yes"
    } else {
        "no"
    }
}

/// A rule's boolean value, as `engine set` takes it: `true` or `false`.
pub fn flag(is_on: bool) -> &'static str {
    if is_on {
        "true"
    } else {
        "false"
    }
}

/// The inputs of one press, from pairs.
pub fn inputs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

/// Where a press stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Sent; the engine has not answered.
    Running,
    /// The engine requested a confirmation: the question in words, and the call that runs it.
    Confirming { act: Act, words: String },
    /// The engine did not do it: its words.
    Told(String),
}

/// A press the engine did, said by its key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub key: String,
    pub output: String,
    pub connector: Option<sakura_core::engine::connectors::ConnectorReceipt>,
}

/// The presses of a window, by the key of the control that sent each.
pub struct EngineActs {
    source: Source,
    facts: Entity<EngineFacts>,
    steps: HashMap<String, Step>,
    /// The values requested with [`EngineActs::set_value`], shown until the engine has been read again.
    pub(super) values: HashMap<String, super::engine_values::Value>,
    _refreshes: gpui::Subscription,
}

impl EventEmitter<Done> for EngineActs {}

impl EngineActs {
    /// Presses go through `source`; `facts` are read again after one the engine did.
    pub fn new(source: Source, facts: Entity<EngineFacts>, cx: &mut Context<Self>) -> Self {
        let refreshes = cx.subscribe(&facts, |acts, _, _: &Refreshed, cx| acts.settle(cx));
        Self {
            source,
            facts,
            steps: HashMap::new(),
            values: HashMap::new(),
            _refreshes: refreshes,
        }
    }

    /// Where the press kept under `key` stands, when there is one.
    pub fn step(&self, key: &str) -> Option<&Step> {
        self.steps.get(key)
    }

    /// Sends the action `act_key` to `target` with `inputs`, as the control `key`. When the engine asks to
    /// confirm, `words` is what the person is asked.
    pub fn press(&mut self, key: &str, act: Act, words: impl Into<String>, cx: &mut Context<Self>) {
        self.send(key.to_string(), act, words.into(), cx);
    }

    /// The person confirmed: the same call, with the engine's token.
    pub fn confirm(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(Step::Confirming { act, words }) = self.steps.remove(key) {
            self.send(key.to_string(), act, words, cx);
        }
    }

    /// The person left what was asked: nothing runs.
    pub fn cancel(&mut self, key: &str, cx: &mut Context<Self>) {
        self.steps.remove(key);
        cx.notify();
    }

    /// Whether the press kept under `key` is on its way to the engine: its control is not pressed again.
    pub fn is_running(&self, key: &str) -> bool {
        matches!(self.steps.get(key), Some(Step::Running))
    }

    pub(super) fn send(&mut self, key: String, act: Act, words: String, cx: &mut Context<Self>) {
        if self.is_running(&key) {
            return;
        }
        self.steps.insert(key.clone(), Step::Running);
        let pending = self.source.act(act.clone());
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            drop(this.update(cx, |acts, cx| acts.answered(key, act, words, result, cx)));
        })
        .detach();
        cx.notify();
    }

    pub(super) fn facts(&self) -> &Entity<EngineFacts> {
        &self.facts
    }

    fn answered(
        &mut self,
        key: String,
        act: Act,
        words: String,
        result: BridgeResult<ActResult>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(ActResult::Confirm {
                token,
                display_args,
                ..
            }) => {
                let words = if act.action.key == "connectorreplace" {
                    display_args
                        .as_ref()
                        .and_then(replacement_words)
                        .unwrap_or(words)
                } else {
                    words
                };
                let act = Act {
                    confirm: Some(token),
                    ..act
                };
                self.steps.insert(key, Step::Confirming { act, words });
            }
            Ok(ActResult::Done {
                exit_code: 0,
                audit: _,
                output,
                connector,
                ..
            }) => {
                self.steps.remove(&key);
                let until = self.reads_to_wait_for(cx);
                self.facts.update(cx, |facts, cx| facts.read(cx));
                self.value_done(&key, until, cx);
                cx.emit(Done {
                    key,
                    output,
                    connector,
                });
            }
            Ok(ActResult::Done {
                exit_code, output, ..
            }) => {
                let line = output.lines().map(str::trim).find(|line| !line.is_empty());
                let failed = || tr_with(Key("waiting.failed"), &[("code", &exit_code.to_string())]);
                let told = if act.action.key.starts_with("scheduled") {
                    super::scheduled::words::reason(line.unwrap_or_default()).to_string()
                } else if act.action.key.starts_with("connector") {
                    connector_refusal(line.unwrap_or_default()).to_string()
                } else {
                    line.map_or_else(|| failed().to_string(), str::to_string)
                };
                self.steps.insert(key.clone(), Step::Told(told));
                self.value_failed(&key, cx);
            }
            Ok(ActResult::Refused {
                reason,
                reason_code,
                reason_args,
            }) => {
                let reason = if act.action.key.starts_with("scheduled") {
                    if reason == "stale_task" {
                        self.facts.update(cx, |facts, cx| facts.read(cx));
                    }
                    super::scheduled::words::reason(&reason).to_string()
                } else if act.action.key.starts_with("connector") {
                    let code = reason_code.as_deref().unwrap_or(&reason);
                    if code == "stale_revision" {
                        self.facts.update(cx, |facts, cx| facts.read(cx));
                    }
                    connector_refusal(code).to_string()
                } else if act.is_track() {
                    self.facts.update(cx, |facts, cx| facts.read(cx));
                    say(&sakura_core::engine::tracks::refusal(
                        reason_code.as_deref(),
                        reason_args.as_ref(),
                        &reason,
                    ))
                    .to_string()
                } else {
                    reason
                };
                self.steps.insert(key.clone(), Step::Told(reason));
                self.value_failed(&key, cx);
            }
            Err(error) => {
                let told = Step::Told(say(&error.said).to_string());
                self.steps.insert(key.clone(), told);
                self.value_failed(&key, cx);
            }
        }
        cx.notify();
    }
}

/// What the press `key` says under its control: the question it asks before it runs, or what the engine
/// refused. Nothing while it is on its way or done.
pub fn line(
    acts: &Entity<EngineActs>,
    key: &str,
    palette: &Palette,
    cx: &App,
) -> Option<AnyElement> {
    let selector = format!("{key}-told");
    match acts.read(cx).step(key)? {
        Step::Running => None,
        Step::Told(told) => Some(
            text(theme::TEXT_LABEL, palette.danger)
                .debug_selector(move || selector.clone())
                .child(told.clone())
                .into_any_element(),
        ),
        Step::Confirming { words, .. } => Some(asking(acts, key, words, palette, cx)),
    }
}

/// The question, with the two ways to answer it.
fn asking(
    acts: &Entity<EngineActs>,
    key: &str,
    words: &str,
    palette: &Palette,
    cx: &App,
) -> AnyElement {
    let (yes, no) = (acts.clone(), acts.clone());
    let (yes_key, no_key) = (key.to_string(), key.to_string());
    let confirm = controls::button(
        format!("{key}-confirm"),
        tr(Key("waiting.confirm")),
        Rc::new(move |_, cx| yes.update(cx, |acts, cx| acts.confirm(&yes_key, cx))),
        cx,
    );
    let cancel = controls::button(
        format!("{key}-cancel"),
        tr(Key("common.cancel")),
        Rc::new(move |_, cx| no.update(cx, |acts, cx| acts.cancel(&no_key, cx))),
        cx,
    );
    div()
        .flex()
        .items_center()
        .gap(theme::GAP)
        .child(text(theme::TEXT_SMALL, palette.text_secondary).child(words.to_string()))
        .child(confirm)
        .child(cancel)
        .into_any_element()
}

fn connector_refusal(code: &str) -> &'static str {
    super::settings_modal::versioned_connectors::words::refusal(code)
}

fn replacement_words(args: &BTreeMap<String, String>) -> Option<String> {
    Some(
        tr_with(
            Key("connectors.replacement_preview"),
            &[
                ("label", args.get("label")?),
                ("candidate", args.get("candidate_label")?),
            ],
        )
        .to_string(),
    )
}

pub(crate) fn connector_request(
    controls: &sakura_core::engine::connectors::ConnectorControls,
    key: &str,
    inputs: BTreeMap<String, String>,
) -> Option<Act> {
    if controls.version != sakura_core::engine::connectors::CONTROLS_VERSION
        || controls.target != CONNECTORS_TARGET
    {
        return None;
    }
    let action = controls
        .actions
        .iter()
        .find(|action| action.key == key && action.confirm)?;
    Some(Act {
        action: action.clone(),
        target: controls.target.clone(),
        inputs,
        confirm: None,
    })
}
