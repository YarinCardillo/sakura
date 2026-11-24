//! The controls the app sends, held to what the real engine lists. The engine's own snapshot, its table of
//! actions and `cockpit act --help` are in `sakura-core/tests/fixtures/engine-real-*` (captured, not written by
//! hand): every key, target, input name and input value the app can send must be one the engine takes on that
//! target, and every input the engine asks for must be one the app sends. The sends are the Inbox's answers
//! (also the master's cards and the palette's verbs, which go through the same held path), the lane actions,
//! New lane, and the controls of Settings and the Engine page.

use super::engine_acts::{
    on, Control, ALL_CONTROLS, CONNECT, KEEPER_SERVICE, SET_DIGEST, SET_LANE_PACE, SET_PACE,
    SET_RULE, SET_SERVICE,
};
use super::engine_page::digest::{LANGUAGES, LOOKBACK_HOURS, RUN_HOURS};
use super::engine_page::pace::{LANES_MIN_RULE, LANES_RULE, LEVELS};
use super::inbox::model::{self, Verb, Verbs};
use super::lane_paces::LANE_PACES;
use super::new_lane::{action_inputs, PACE_INPUT, PRESET_INPUT};
use super::settings_modal::connectors::KINDS;
use regex::Regex;
use sakura_core::engine::snapshot::{Action, Snapshot, Wait};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const SNAPSHOT: &str =
    include_str!("../../../sakura-core/tests/fixtures/engine-real-snapshot.json");
const ACT_HELP: &str = include_str!("../../../sakura-core/tests/fixtures/engine-real-act-help.txt");
const RULES: &str = include_str!("../../../sakura-core/tests/fixtures/engine-real-rules.json");
const TABLE: &str =
    include_str!("../../../sakura-core/tests/fixtures/engine-real-action-table.json");

/// The inputs of the Inbox's answers: a mark's verdict and note, a reply's choice and text, an approval's
/// remember.
const ANSWER_INPUTS: [&str; 5] = ["verdict", "note", "choice", "text", "remember"];

/// What the engine lists for one action: the inputs it needs and those it may take.
struct Listed {
    needs: BTreeSet<String>,
    optional: BTreeSet<String>,
}

fn names(value: &Value) -> BTreeSet<String> {
    let list = value.as_array().expect("a list");
    list.iter()
        .map(|name| name.as_str().expect("a name").to_string())
        .collect()
}

/// The actions listed beside `target` in the snapshot, by key.
fn listing(actions: &Value, key: &str) -> Option<Listed> {
    let found = actions
        .as_array()?
        .iter()
        .find(|action| action["key"] == key)?;
    Some(Listed {
        needs: names(&found["needs"]),
        optional: names(&found["optional"]),
    })
}

/// The part of the snapshot whose `target` is `target` (a fixed target the engine names in its document), as
/// its `actions`.
fn actions_of_target<'a>(snapshot: &'a Value, target: &str) -> &'a Value {
    let candidates = [
        (&snapshot["engine"]["id"], &snapshot["engine"]["actions"]),
        (&snapshot["host"]["id"], &snapshot["host"]["actions"]),
        (
            &snapshot["engine"]["keeper"]["target"],
            &snapshot["engine"]["keeper"]["actions"],
        ),
        (
            &snapshot["digest"]["connect"]["target"],
            &snapshot["digest"]["connect"]["actions"],
        ),
        (
            &snapshot["digest"]["target"],
            &snapshot["digest"]["actions"],
        ),
        (
            &snapshot["automations"]["target"],
            &snapshot["automations"]["actions"],
        ),
    ];
    let found = candidates
        .iter()
        .find(|(named, _)| named.as_str() == Some(target));
    found.map_or_else(
        || panic!("the engine names no target {target}"),
        |(_, actions)| *actions,
    )
}

/// What the engine lists for `control`: on its fixed target, or on the first lane or account that has it.
fn listed_for(snapshot: &Value, control: &Control) -> Listed {
    let found = match control.target {
        Some(target) => listing(actions_of_target(snapshot, target), control.key),
        None => {
            let mut rows = ["items", "accounts"]
                .iter()
                .flat_map(|part| snapshot[*part].as_array().expect("rows").iter());
            rows.find_map(|row| listing(&row["actions"], control.key))
        }
    };
    found.unwrap_or_else(|| panic!("the engine lists no action {}", control.key))
}

#[test]
fn every_control_the_app_sends_is_one_the_engine_lists_with_the_inputs_it_asks_for() {
    let snapshot: Value = serde_json::from_str(SNAPSHOT).expect("the engine's snapshot");
    for control in ALL_CONTROLS {
        let listed = listed_for(&snapshot, &control);
        let sent_needs: BTreeSet<String> = control.needs.iter().map(|n| n.to_string()).collect();
        let sent_optional: BTreeSet<String> =
            control.optional.iter().map(|n| n.to_string()).collect();
        assert_eq!(sent_needs, listed.needs, "inputs of {}", control.key);
        assert!(
            sent_optional.is_subset(&listed.optional),
            "optional inputs of {}: {sent_optional:?} against {:?}",
            control.key,
            listed.optional
        );
    }
}

#[test]
fn every_input_name_the_app_sends_is_an_option_of_the_engine_command() {
    let sent: BTreeSet<&str> = ALL_CONTROLS
        .iter()
        .flat_map(|control| control.needs.iter().chain(control.optional))
        .copied()
        .collect();
    for name in sent
        .iter()
        .chain(&[PRESET_INPUT, PACE_INPUT])
        .chain(&ANSWER_INPUTS)
    {
        assert!(
            ACT_HELP.contains(&format!("--{name} ")),
            "the engine's `cockpit act` has no --{name}"
        );
    }
}

#[test]
fn the_engine_switches_take_yes_and_no() {
    assert_eq!((on(true), on(false)), ("yes", "no"));
}

#[test]
fn every_fixed_target_the_app_names_is_the_one_the_engine_documents() {
    let snapshot: Value = serde_json::from_str(SNAPSHOT).expect("the engine's snapshot");
    for control in ALL_CONTROLS {
        if let Some(target) = control.target {
            // A panic here says the engine names no such target.
            actions_of_target(&snapshot, target);
        }
    }
}

// The engine's table of actions: what each takes, on which kind of target, and in which shape.

/// The engine's own table of actions, as `make-engine-real.py` captured it from the engine's code.
struct Table(Value);

impl Table {
    fn read() -> Table {
        Table(serde_json::from_str(TABLE).expect("the engine's table of actions"))
    }

    fn action(&self, key: &str) -> &Value {
        let found = self.0["actions"].get(key);
        found.unwrap_or_else(|| panic!("the engine has no action {key}"))
    }

    /// The action as the engine's snapshot lists it on a target.
    fn listed(&self, key: &str) -> Value {
        let action = self.action(key);
        let names = |part: &str| action[part].as_array().cloned().unwrap_or_default();
        json!({"key": key, "label": action["label"], "needs": names("needs"),
               "optional": names("optional"), "confirm": action["confirm"]})
    }

    /// The shape the engine takes for the input `name`, when it states one.
    fn shape(&self, name: &str) -> Option<Regex> {
        let pattern = self.0["input_patterns"].get(format!("{name}/"))?;
        let pattern = pattern.as_str().expect("a pattern");
        Some(Regex::new(&format!("^(?:{pattern})$")).expect("the engine's pattern"))
    }

    fn list(&self, part: &str) -> Vec<String> {
        let list = self.0[part].as_array();
        let list = list.unwrap_or_else(|| panic!("the engine's table has no list {part}"));
        list.iter()
            .map(|word| word.as_str().expect("a word").into())
            .collect()
    }
}

/// The names in `part` of an action of the table; none when the engine leaves it to the target (`needs` of
/// `m`).
fn taken(action: &Value, part: &str) -> Option<BTreeSet<String>> {
    action[part].as_array().map(|list| names(&json!(list)))
}

/// Holds one send to the engine's table: the action applies to the kind of target it goes to, every input it
/// needs is given, none it does not take is, and each value has the shape the engine accepts.
fn check_send(table: &Table, key: &str, kind: &str, inputs: &BTreeMap<String, String>) {
    let action = table.action(key);
    let kinds = taken(action, "kinds").expect("kinds");
    assert!(
        kinds.contains(kind),
        "{key} goes to a {kind}; the engine takes it on {kinds:?}"
    );
    let optional = taken(action, "optional").expect("optional");
    if let Some(needs) = taken(action, "needs") {
        let given: BTreeSet<String> = inputs.keys().cloned().collect();
        assert!(
            needs.is_subset(&given),
            "{key} needs {needs:?}; the app sends {given:?}"
        );
        let unknown: Vec<_> = given
            .iter()
            .filter(|name| !needs.contains(*name) && !optional.contains(*name))
            .collect();
        assert!(
            unknown.is_empty(),
            "{key} takes {needs:?} and {optional:?}; the app also sends {unknown:?}"
        );
    }
    for (name, value) in inputs {
        let is_valid = table.shape(name).is_none_or(|shape| shape.is_match(value));
        assert!(is_valid, "{key}: the engine refuses {name} = {value:?}");
    }
}

#[test]
fn every_control_is_an_action_of_the_engine_table_for_the_kind_of_target_it_goes_to() {
    let table = Table::read();
    for control in ALL_CONTROLS {
        let action = table.action(control.key);
        let kind = match control.target {
            Some(target) => {
                let target: Value = serde_json::from_str(target).expect("a target");
                let takes = names(&table.0["host_targets"][target[1].as_str().expect("a name")]);
                assert!(takes.contains(control.key), "{} on {target}", control.key);
                "host"
            }
            None if control.key == "q" => "flow",
            None => "account",
        };
        assert!(
            taken(action, "kinds").expect("kinds").contains(kind),
            "{} on a {kind}",
            control.key
        );
        let needs: BTreeSet<String> = control.needs.iter().map(|name| name.to_string()).collect();
        assert_eq!(Some(needs), taken(action, "needs"), "{}", control.key);
    }
}

// The Inbox's answers, and with them the master's cards and the palette's verbs, which go through its held path.

const ORCHARD: &str = "/work/orchard";

/// An ask of `kind` as the engine lists it: the actions the engine's table gives that kind of ask, bound to
/// the gate `name`.
fn ask_of(table: &Table, kind: &str, name: &str) -> Wait {
    let keys = table.0["wait_actions"][kind]
        .as_array()
        .expect("the actions of the kind");
    let actions: Vec<Value> = keys
        .iter()
        .map(|key| table.listed(key.as_str().expect("a key")))
        .collect();
    let key = json!(["wait", ORCHARD, "add-search", kind, format!("{name}@1a2b")]);
    let ask = json!({"id": key.to_string(), "key": key, "kind": kind, "repo": ORCHARD, "project": "orchard",
        "task": "add-search", "title": "Ask", "since": null, "actions": actions, "choices": ["yes", "no"]});
    serde_json::from_value(ask).expect("an ask")
}

fn verbs_sent(verbs: &Verbs) -> Vec<&Verb> {
    verbs.primary.iter().chain(&verbs.secondary).collect()
}

/// Checks every answer the Inbox builds for `ask`, and returns how many it checked.
fn check_answers(
    table: &Table,
    ask: &Wait,
    lane: Option<&sakura_core::engine::snapshot::Item>,
) -> usize {
    let verbs = model::verbs(ask, lane);
    let mut checked = 0;
    for verb in verbs_sent(&verbs) {
        let is_lane_verb = verb.target != ask.id;
        let kind = if is_lane_verb { "flow" } else { "wait" };
        assert!(
            is_lane_verb || ask_keys(table, &ask.kind).contains(&verb.action.key),
            "{ask:?}"
        );
        check_send(table, &verb.action.key, kind, &verb.inputs);
        if verb.takes_note {
            let mut noted = verb.inputs.clone();
            noted.insert("note".into(), "not yet".into());
            check_send(table, &verb.action.key, kind, &noted);
        }
        checked += 1;
    }
    for reply in verbs.reply.iter() {
        for sent in [pairs(&[("choice", "yes")]), pairs(&[("text", "go on")])] {
            check_send(table, &reply.key, "wait", &sent);
            checked += 1;
        }
    }
    checked
}

fn ask_keys(table: &Table, kind: &str) -> Vec<String> {
    let keys = table.0["wait_actions"][kind].as_array().expect("keys");
    keys.iter()
        .map(|key| key.as_str().expect("a key").into())
        .collect()
}

fn pairs(list: &[(&str, &str)]) -> BTreeMap<String, String> {
    list.iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn every_answer_the_inbox_sends_is_one_the_engine_takes_for_that_kind_of_ask() {
    let table = Table::read();
    for kind in [
        "approval",
        "owner_row",
        "question",
        "proposal",
        "automation",
    ] {
        for gate in ["spec", "checks", "overlap"] {
            let ask = ask_of(&table, kind, gate);
            assert!(
                check_answers(&table, &ask, None) > 0,
                "the Inbox answers nothing on a {kind}"
            );
        }
    }
}

#[test]
fn a_blocked_ask_is_answered_through_the_actions_the_engine_lists_on_its_lane() {
    let table = Table::read();
    let snapshot = Snapshot::parse(SNAPSHOT.as_bytes()).expect("the engine's snapshot");
    let ask = ask_of(&table, "blocked", "blocked");
    let lane = model::lane_of(&ask, &snapshot).expect("the lane of the ask");
    assert_eq!(
        check_answers(&table, &ask, Some(lane)),
        2,
        "Resume and Stop"
    );
}

#[test]
fn the_remember_input_is_sent_only_on_the_checks_gate_the_engine_lets_it_apply_to() {
    let table = Table::read();
    let sends_remember = |gate: &str| {
        let verbs = model::verbs(&ask_of(&table, "approval", gate), None);
        verbs_sent(&verbs)
            .iter()
            .any(|verb| verb.inputs.contains_key("remember"))
    };
    assert!(sends_remember("checks"));
    assert!(!sends_remember("spec") && !sends_remember("overlap"));
}

#[test]
fn every_kind_of_ask_and_every_answer_the_engine_has_is_worded() {
    let table = Table::read();
    let kinds = table.0["wait_actions"]
        .as_object()
        .expect("the kinds of ask");
    for kind in kinds.keys() {
        assert_ne!(
            model::kind_words(kind).as_ref(),
            kind,
            "no words for the ask kind {kind}"
        );
    }
    for answer in table.list("decided_answers") {
        let decided = decided_with(&answer);
        assert_ne!(
            model::answer_words(&decided).as_ref(),
            answer,
            "no words for the answer {answer}"
        );
    }
}

fn decided_with(answer: &str) -> sakura_core::engine::snapshot::Decided {
    let row = json!({"id": "x", "project": "orchard", "task": "add-search", "answer": answer,
        "choice": null, "text": null, "at": null});
    serde_json::from_value(row).expect("a decided ask")
}

// New lane.

#[test]
fn new_lane_sends_the_real_start_action_only_what_it_takes_and_names_the_pace_as_the_engine_does() {
    let table = Table::read();
    let snapshot = Snapshot::parse(SNAPSHOT.as_bytes()).expect("the engine's snapshot");
    let start = snapshot.projects[0]
        .start_lane()
        .cloned()
        .expect("the start action");
    let chosen = (Some("lean"), Some("fast"));
    check_send(
        &table,
        "L",
        "project",
        &action_inputs(&start, "a lane".into(), chosen),
    );
    let taking = Action {
        optional: vec![PRESET_INPUT.into(), PACE_INPUT.into()],
        ..start
    };
    let sent = action_inputs(&taking, "a lane".into(), chosen);
    assert_eq!(sent.get(PACE_INPUT).map(String::as_str), Some("fast"));
    assert!(
        sent.keys()
            .all(|name| ACT_HELP.contains(&format!("--{name} "))),
        "{sent:?}"
    );
}

// The values the Engine page and Settings send.

fn engine_rules() -> Vec<Value> {
    let rules: Value = serde_json::from_str(RULES).expect("the engine's rules");
    let nodes = rules["nodes"].as_array().expect("the nodes");
    nodes
        .iter()
        .flat_map(|node| node["rules"].as_array().expect("rules").clone())
        .collect()
}

#[test]
fn the_pace_levels_the_app_sends_are_the_engine_s_own_in_the_same_order() {
    let table = Table::read();
    let engine = table.list("pace_levels");
    let app: Vec<&str> = LEVELS.iter().map(|(name, _, _)| *name).collect();
    assert_eq!(app, engine);
    for (name, _) in LANE_PACES {
        assert!(engine.iter().any(|level| level == name), "{name}");
    }
    for name in &app {
        let sent = SET_PACE.act(&[("level", name)]);
        check_send(&table, &sent.action.key, "host", &sent.inputs);
        let lane = SET_LANE_PACE.act_on("[\"flow\"]", &[("level", name)]);
        check_send(&table, &lane.action.key, "flow", &lane.inputs);
    }
}

#[test]
fn the_rules_the_engine_page_sets_are_rules_the_engine_has_with_the_type_the_app_sends() {
    let table = Table::read();
    let rules = engine_rules();
    let rule_of = |id: &str| {
        rules
            .iter()
            .find(|rule| rule["id"] == id)
            .unwrap_or_else(|| panic!("no rule {id}"))
    };
    for id in [LANES_MIN_RULE, LANES_RULE] {
        assert_eq!(rule_of(id)["type"], "int", "{id}");
        let sent = SET_RULE.act(&[("rule", id), ("value", "2")]);
        check_send(&table, &sent.action.key, "host", &sent.inputs);
    }
    for rule in rules.iter().filter(|rule| rule["read_only"] != true) {
        let sent = SET_RULE.act(&[
            ("rule", rule["id"].as_str().expect("an id")),
            ("value", "x"),
        ]);
        check_send(&table, &sent.action.key, "host", &sent.inputs);
        assert!(
            ["bool", "int", "choice"].contains(&rule["type"].as_str().expect("a type")),
            "{rule}"
        );
    }
}

#[test]
fn the_digest_settings_the_engine_page_sends_have_the_shapes_the_engine_takes() {
    let table = Table::read();
    let shape = |name: &str| {
        let pattern = table.0["digest_inputs"][name].as_str().expect("a shape");
        Regex::new(&format!("^(?:{pattern})$")).expect("a pattern")
    };
    for hour in RUN_HOURS {
        assert!(shape("at").is_match(&format!("{hour:02}:00")), "{hour}");
    }
    let most = table.0["digest_inputs"]["max_lookback_hours"]
        .as_u64()
        .expect("a limit");
    assert!(LOOKBACK_HOURS
        .iter()
        .all(|hours| (1..=most as u32).contains(hours)));
    for (code, _) in LANGUAGES {
        assert!(shape("language").is_match(code), "{code}");
        let sent = SET_DIGEST.act(&[("language", code)]);
        check_send(&table, &sent.action.key, "host", &sent.inputs);
    }
}

#[test]
fn the_services_and_connector_kinds_the_app_names_are_the_engine_s() {
    let table = Table::read();
    assert!(table
        .list("service_names")
        .iter()
        .any(|name| name == KEEPER_SERVICE));
    let sent = SET_SERVICE.act(&[("name", KEEPER_SERVICE), ("on", on(true))]);
    check_send(&table, &sent.action.key, "host", &sent.inputs);
    let kinds = table.list("connector_kinds");
    for kind in KINDS {
        assert!(kinds.iter().any(|known| known == kind.id), "{}", kind.id);
        let sent = if kind.is_many {
            CONNECT.act(&[
                ("kind", kind.id),
                ("label", "News"),
                ("address", "https://example.com/feed.xml"),
            ])
        } else {
            CONNECT.act(&[("kind", kind.id)])
        };
        check_send(&table, &sent.action.key, "host", &sent.inputs);
    }
}

#[test]
fn the_controls_the_snapshot_does_not_list_are_actions_of_the_engine_table_with_the_input_they_send(
) {
    let table: Value = serde_json::from_str(TABLE).expect("the engine's table");
    let unlisted = crate::ui::engine_acts::PHONE_CONTROLS
        .into_iter()
        .chain(crate::ui::engine_acts::ACCOUNT_CONTROLS)
        .chain(crate::ui::engine_acts::MODEL_CONTROLS);
    for control in unlisted {
        let listed = &table["actions"][control.key];
        assert!(
            listed.is_object(),
            "the engine's table has no action {}",
            control.key
        );
        let needs: Vec<&str> = listed["needs"]
            .as_array()
            .expect("needs")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(needs, control.needs, "{}", control.key);
        assert!(
            listed["kinds"]
                .as_array()
                .expect("kinds")
                .iter()
                .any(|kind| kind == "host"),
            "{} is a host action",
            control.key
        );
    }
}

#[test]
fn the_lane_step_switch_is_an_action_of_the_engine_table_for_a_flow_lane_with_the_inputs_it_sends()
{
    let table: Value = serde_json::from_str(TABLE).expect("the engine's table");
    for control in crate::ui::engine_acts::LANE_STEP_CONTROLS {
        let listed = &table["actions"][control.key];
        let needs: Vec<&str> = listed["needs"]
            .as_array()
            .expect("needs")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(needs, control.needs);
        assert_eq!(listed["kinds"], serde_json::json!(["flow"]));
    }
}

#[test]
fn connection_mutations_require_supported_confirmed_engine_controls() {
    let document = include_bytes!("../../../sakura-core/tests/fixtures/connector-controls.json");
    let mut controls = Snapshot::parse(document)
        .unwrap()
        .connector_controls
        .unwrap();
    let pairs = [
        ("op", "request-id"),
        ("name", "custom-id"),
        ("revision", "2"),
        ("candidate", "candidate-id"),
        ("candidate_revision", "1"),
    ];
    let inputs = super::engine_acts::inputs(&pairs);
    let act = super::engine_acts::connector_request(&controls, "connectorreplace", inputs.clone())
        .unwrap();
    assert_eq!(act.inputs, inputs);
    assert_eq!(act.action.needs.len(), 5);
    assert!(act.confirm.is_none());
    controls.version += 1;
    assert!(super::engine_acts::connector_request(&controls, "connectorreplace", inputs).is_none());
}

#[test]
fn track_actions_advertise_their_target_specific_inputs() {
    let snapshot = Snapshot::parse(include_bytes!(
        "../../../sakura-core/tests/fixtures/engine-real-tracks.json"
    ))
    .unwrap();
    let create = snapshot.tracks.action("tracknew").unwrap();
    assert_eq!(create.needs, ["name", "goal", "op"]);
    assert_eq!(create.optional, ["project"]);
    let wait = snapshot
        .waits
        .iter()
        .find(|wait| wait.original_kind() == "track_decision")
        .unwrap();
    let answer = wait
        .actions
        .iter()
        .find(|action| action.key == "tracksettle")
        .unwrap();
    assert_eq!(answer.needs, ["answer", "op"]);
    let track = snapshot.track(&wait.track.as_ref().unwrap().id).unwrap();
    assert_eq!(
        track.action("tracksettle").unwrap().needs,
        ["decision", "answer", "op"]
    );
    let start = snapshot
        .projects
        .iter()
        .find_map(|project| project.start_lane())
        .unwrap();
    assert!(start.optional.iter().any(|input| input == "track"));
    assert!(start.optional.iter().any(|input| input == "op"));
}
