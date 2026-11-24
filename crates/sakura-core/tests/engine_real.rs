//! What the real engine prints, read with the app's own types. The `engine-real-*` fixtures are captured from the
//! engine (see `fixtures/README.md`), not written by hand: a shape the app invents shows up here as a refusal.

use sakura_core::engine::accounts::{Account, Connector};
use sakura_core::engine::automations::{Automation, Digest, DigestSource};
use sakura_core::engine::cli::EngineCli;
use sakura_core::engine::connectors::{
    ConnectorControls, ConnectorReceipt, ConnectorScope, ConnectorSelectors, DependentTask,
};
use sakura_core::engine::feed::{FeedEntry, FeedPage};
use sakura_core::engine::master::{Decided as MasterDecided, Decision, MasterSpec};
use sakura_core::engine::presets::PresetList;
use sakura_core::engine::rules::Rules;
use sakura_core::engine::scheduled::{
    ConversationEntry, ConversationPage, Definition, HistoryPage, Run, RunTrigger, Scheduled,
    Template, Trigger,
};
use sakura_core::engine::snapshot::{
    Action, Decided, Delivery, Event, Item, LaneReview, LaneStep, Link, Progress, Project, Reading,
    ReadingAsk, ReadingNews, ScheduledOrigin, Snapshot, Wait, WaitProposal,
};
use sakura_core::engine::state::{EngineState, Keeper, Restart};
use serde::de::{DeserializeOwned, Deserializer, Error, Visitor};
use serde::forward_to_deserialize_any;
use serde_json::Value;
use std::collections::BTreeSet;

const SNAPSHOT: &str = include_str!("fixtures/engine-real-snapshot.json");
const OUTSIDE_SNAPSHOT: &str = include_str!("fixtures/engine-real-snapshot-outside.json");
const TRACK_SNAPSHOT: &str = include_str!("fixtures/engine-real-tracks.json");
const RULES: &str = include_str!("fixtures/engine-real-rules.json");
const PRESET_LIST: &str = include_str!("fixtures/engine-real-preset-list.json");
const FEED: &str = include_str!("fixtures/engine-real-feed.json");
const FEED_KINDS: &str = include_str!("fixtures/engine-real-feed-kinds.json");
const MASTER_SPEC: &str = include_str!("fixtures/engine-real-master-spec.json");
const MASTER_DECIDE: &str = include_str!("fixtures/engine-real-master-decide.json");
const SCHEDULED_HISTORY: &str = include_str!("fixtures/engine-real-scheduled-history.json");
const SCHEDULED_CONVERSATION: &str =
    include_str!("fixtures/engine-real-scheduled-conversation.json");
const SCHEDULED_FINDING: &str = include_str!("fixtures/engine-real-scheduled-finding.json");
const SCHEDULED_PROBLEMS: &str = include_str!("fixtures/engine-real-scheduled-problems.json");
const CONNECTOR_RECEIPT: &str = include_str!("fixtures/engine-real-connector-receipt.json");

fn snapshot() -> Snapshot {
    Snapshot::parse(SNAPSHOT.as_bytes()).expect("the engine's own snapshot is read")
}

#[tokio::test]
async fn the_real_track_task_id_reaches_every_lane_read_and_steering_command() {
    use std::os::unix::fs::PermissionsExt as _;

    let snapshot = Snapshot::parse(TRACK_SNAPSHOT.as_bytes()).unwrap();
    let task = snapshot
        .tracks
        .rows()
        .iter()
        .flat_map(|track| &track.lanes)
        .find(|lane| lane.task.starts_with("track-"))
        .unwrap()
        .task
        .as_str();
    assert_eq!(task.len(), 48);
    let repo = tempfile::tempdir().unwrap();
    let bin = repo.path().join("fixture-engine");
    std::fs::write(&bin, format!(r#"#!/bin/sh
case "$*" in
  'flow sessions --json -- {task}') printf '[]';;
  'flow feed --json -- {task}'|'flow feed --json --run=intake-1 -- {task}') printf '{{"entries":[],"cursor":""}}';;
  'flow steer --run=intake-1 --actor='*' --json -- {task} Continue') printf '{{"outcome":"queued","run":"intake-1","words":"Queued"}}';;
  'flow changes --json -- {task}'|'flow changes --json --file=src/main.rs -- {task}') printf '{{}}';;
  *) exit 1;;
esac
"#)).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let engine = EngineCli::at(bin);
    let repo = repo.path();
    let calls = [
        ("feed", engine.feed(repo, task, "").await.map(drop)),
        ("agents", engine.agents(repo, task).await.map(drop)),
        (
            "agent chat",
            engine
                .agent_feed(repo, task, "intake-1", None)
                .await
                .map(drop),
        ),
        (
            "steering",
            engine
                .write_to_agent(repo, task, "intake-1", "Continue")
                .await
                .map(drop),
        ),
        ("changes", engine.changes(repo, task).await.map(drop)),
        (
            "file patch",
            engine
                .changes_file(repo, task, "src/main.rs")
                .await
                .map(drop),
        ),
    ];
    assert!(calls.iter().all(|(_, result)| result.is_ok()), "{calls:?}");
}

#[test]
fn the_real_snapshot_is_read_whole() {
    let snapshot = snapshot();
    assert_eq!(snapshot.items.len(), 2);
    assert_eq!(snapshot.waits.len(), 1);
    assert_eq!(snapshot.projects.len(), 1);
    assert_eq!(snapshot.projects[0].branch.as_deref(), Some("main"));
    assert!(snapshot.problems.is_empty(), "{:?}", snapshot.problems);
}

#[test]
fn the_snapshot_reads_the_snooze_flag_and_its_deadline() {
    let mut document: Value = serde_json::from_str(SNAPSHOT).unwrap();
    document["waits"][0]["is_snoozed"] = true.into();
    document["waits"][0]["snoozed_until"] = "2026-11-22T18:00:00Z".into();
    let sleeping = Snapshot::parse(document.to_string().as_bytes()).unwrap();
    document["waits"][0]["is_snoozed"] = false.into();
    let awake = Snapshot::parse(document.to_string().as_bytes()).unwrap();
    assert_ne!(sleeping.waits[0], awake.waits[0]);
}

#[test]
fn the_real_snapshot_names_a_lane_and_its_ask_by_the_lane_s_title() {
    let snapshot = snapshot();
    let names: Vec<&str> = snapshot.items.iter().map(|item| item.name()).collect();
    assert_eq!(names, ["Add search to the orchard", "Fix the login form"]);
    let ask = &snapshot.waits[0];
    assert_eq!(ask.task, "fix-login", "the slug stays the id");
    assert_eq!(ask.lane_name(), "Fix the login form");
}

#[test]
fn the_real_snapshot_accounts_include_one_the_engine_knows_no_plan_for() {
    let snapshot = snapshot();
    let [studio, archive, spare] = snapshot.accounts.as_slice() else {
        panic!("three accounts: {:?}", snapshot.accounts);
    };
    assert_eq!(studio.plan.as_deref(), Some("max"));
    assert_eq!(
        archive.plan, None,
        "an unread account has no plan, not an empty one"
    );
    assert_eq!((studio.week_pct, archive.week_pct), (Some(61), None));
    assert!(spare.is_hidden() && !studio.is_hidden());
}

#[test]
fn the_real_snapshot_keeper_says_which_lanes_it_resumed() {
    let engine = snapshot().engine.expect("the engine part");
    let keeper = engine.keeper.expect("the keeper");
    let restart = keeper.last_restart.expect("a restart");
    assert_eq!(restart.resumed.len(), 1, "{:?}", restart.resumed);
    assert!(restart.waiting.is_empty());
    assert!(keeper.on);
    assert_eq!(
        (engine.pace.as_deref(), engine.preset.as_deref()),
        (Some("normal"), Some("standard"))
    );
}

#[test]
fn the_real_snapshot_automations_come_in_an_envelope_and_the_services_are_told_apart() {
    let automations = snapshot().automations.expect("the automations");
    let names: Vec<&str> = automations.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "hourly-wake",
            "morning-digest",
            "weekly-wake",
            "keeper",
            "phone-page"
        ]
    );
    let services: Vec<&str> = automations
        .iter()
        .filter(|item| item.is_service())
        .map(|item| item.name.as_str())
        .collect();
    assert_eq!(services, ["keeper", "phone-page"]);
    let wake = automations.iter().find(|item| item.name == "weekly-wake");
    let wake = wake.expect("the automation");
    assert_eq!(wake.schedule_text, "daily 06:30");
    assert_eq!(wake.last_result, None, "no run yet");
}

#[test]
fn the_real_snapshot_digest_settings_sit_beside_the_automations() {
    let digest = snapshot().digest.expect("the digest settings");
    assert_eq!(
        (
            digest.at.as_str(),
            digest.lookback_hours,
            digest.language.as_str()
        ),
        ("07:00", Some(24), "en")
    );
    assert_eq!(digest.account, None, "the empty account is the default one");
    let sources: Vec<(&str, bool)> = digest
        .sources
        .iter()
        .map(|source| (source.id.as_str(), source.enabled))
        .collect();
    assert_eq!(
        sources,
        [("github", true), ("news", true), ("local-notes", false)]
    );
    assert_eq!(digest.sources[0].name(), "GitHub");
}

#[test]
fn the_real_digest_sources_say_their_name_and_what_they_read() {
    let digest = snapshot().digest.expect("the digest settings");
    let said: Vec<(&str, &str)> = digest
        .sources
        .iter()
        .map(|source| (source.name(), source.reads.as_str()))
        .collect();
    assert_eq!(
        said,
        [
            ("GitHub", "Pull requests and issues that wait on you"),
            ("News", "New entries of example.com"),
            ("Local notes", ""),
        ]
    );
}

#[test]
fn the_real_snapshot_connectors_read_with_and_without_a_last_use() {
    let connectors = snapshot().connectors.expect("the connectors");
    assert_eq!(
        connectors.len(),
        4,
        "the digest's own program is listed after the connected ones"
    );
    assert!(connectors[0].connected && !connectors[1].connected);
    assert_eq!(connectors[1].last_used_at, None);
    let program = &connectors[3];
    assert_eq!(
        (program.kind.as_str(), program.label.as_str()),
        ("program", "local-notes")
    );
}

#[test]
fn the_real_rules_are_read_with_their_types() {
    let rules: Rules = serde_json::from_str(RULES).expect("the engine's own rules");
    assert_eq!((rules.preset.as_str(), rules.edited), ("standard", false));
    assert!(rules.rule("reviews.plan_review.on").is_some());
    assert!(rules.node("guard").is_some());
    let agent = rules
        .rule("steps.plan.model")
        .and_then(|rule| rule.agent.as_ref())
        .expect("the plan agent");
    assert_eq!(agent.account, "project");
    assert!(agent.account_label.is_empty());
}

#[test]
fn the_real_preset_list_names_every_preset_and_the_chosen_one() {
    let list = PresetList::parse(PRESET_LIST.as_bytes()).expect("the engine's own listing");
    assert_eq!(
        list.names(),
        ["solo", "standard", "verified", "vanilla", "lean"]
    );
    assert_eq!(list.chosen(), Some("standard"));
}

#[test]
fn the_real_preset_list_gives_every_preset_in_words() {
    let list = PresetList::parse(PRESET_LIST.as_bytes()).expect("the engine's own listing");
    let words: Vec<String> = list.presets.iter().map(|preset| preset.words()).collect();
    assert_eq!(words, ["Solo", "Standard", "Verified", "Vanilla", "Lean"]);
    assert!(list.presets.iter().all(|preset| !preset.label.is_empty()));
}

#[test]
fn the_real_snapshot_lists_the_presets_with_the_chosen_one_marked_and_no_legacy_name() {
    let engine = snapshot().engine.expect("the engine part");
    let listed: Vec<(&str, &str, bool)> = engine
        .presets
        .iter()
        .map(|preset| (preset.name.as_str(), preset.label.as_str(), preset.chosen))
        .collect();
    assert_eq!(
        listed,
        [
            ("solo", "Solo", false),
            ("standard", "Standard", true),
            ("verified", "Verified", false),
            ("lean", "Lean", false),
        ]
    );
}

#[test]
fn the_real_engine_names_the_preset_the_plan_and_the_accounts_in_words() {
    let snapshot = snapshot();
    let lane = &snapshot.items[0];
    assert_eq!(lane.preset_name().as_deref(), Some("Standard"));
    assert_eq!(lane.preset_label.as_deref(), Some("Standard"));
    let studio = snapshot.accounts.iter().find(|a| a.name() == "Studio");
    assert_eq!(
        studio.and_then(|a| a.plan_name()).as_deref(),
        Some("Claude Max")
    );
    let rules: Rules = serde_json::from_str(RULES).expect("the engine's own rules");
    assert_eq!(rules.preset_label.as_deref(), Some("Standard"));
    let engine = snapshot.engine.as_ref().expect("the engine part");
    assert_eq!(engine.preset_label.as_deref(), Some("Standard"));
}

#[test]
fn the_real_snapshot_says_when_the_usage_was_read() {
    let snapshot = snapshot();
    let usage = snapshot.usage.as_ref().expect("the usage part");
    assert_eq!(usage.as_of.as_deref(), Some("2026-11-18T07:55:00Z"));
}

#[test]
fn the_real_progress_of_a_flow_lane_is_in_steps_with_the_engine_s_phase_kept() {
    let snapshot = snapshot();
    let progress = snapshot.items[0].progress.as_ref().expect("a progress");
    assert_eq!(
        (progress.phase.as_str(), progress.phase_id()),
        ("Build", "build")
    );
}

// Every field the app reads is one the engine prints. The app reads the snapshot through the types of
// `sakura_core::engine`; asking each type for its field names, and looking for each in the real snapshot, finds
// a name the app invented (a field that is read as empty on the real engine, however well a hand-written
// fixture fills it).

/// Collects the field names a type asks for when it is read as a struct.
struct FieldNames<'a>(&'a mut Vec<&'static str>);

impl<'de> Deserializer<'de> for FieldNames<'_> {
    type Error = serde::de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
        Err(Error::custom("only the field names are wanted"))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        _: V,
    ) -> Result<V::Value, Self::Error> {
        self.0.extend(fields);
        Err(Error::custom("only the field names are wanted"))
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf option unit
        unit_struct newtype_struct seq tuple tuple_struct map enum identifier ignored_any
    }
}

fn fields_of<T: DeserializeOwned>() -> Vec<&'static str> {
    let mut found = Vec::new();
    drop(T::deserialize(FieldNames(&mut found)));
    assert!(
        !found.is_empty(),
        "{} names no fields",
        std::any::type_name::<T>()
    );
    found
}

fn document() -> Value {
    serde_json::from_str(SNAPSHOT).expect("the engine's own snapshot")
}

/// The objects of `value` that are in a list or are one: `value` itself, or each of its entries.
fn rows(value: &Value) -> Vec<&Value> {
    match value {
        Value::Array(list) => list.iter().filter(|row| row.is_object()).collect(),
        Value::Object(_) => vec![value],
        _ => Vec::new(),
    }
}

/// The fields of `T` that no object of `rows` has and that `optional` does not declare absent. `optional` says,
/// for each name, why the real engine's snapshot may lack it.
fn fields_the_engine_lacks<T: DeserializeOwned>(
    rows: &[&Value],
    optional: &[(&str, &str)],
) -> Vec<String> {
    let seen: BTreeSet<&str> = rows
        .iter()
        .flat_map(|row| row.as_object().expect("an object").keys())
        .map(String::as_str)
        .collect();
    let declared: Vec<&str> = optional.iter().map(|(name, _)| *name).collect();
    for name in &declared {
        assert!(
            !seen.contains(name),
            "{name} is declared absent but the engine prints it: drop the entry"
        );
    }
    let wanted = fields_of::<T>();
    let lacks = |name: &&&str| !seen.contains(**name) && !declared.contains(*name);
    wanted
        .iter()
        .filter(lacks)
        .map(|name| name.to_string())
        .collect()
}

const NO_EXAMPLE: &str = "the fixture's home has none, so the engine prints no row to read";

#[test]
fn every_field_the_app_reads_from_a_lane_a_wait_or_a_project_is_one_the_engine_prints() {
    let document = document();
    let outside = outside_document();
    let mut lanes = rows(&document["items"]);
    lanes.extend(rows(&outside["items"]));
    assert_engine_fields::<Item>(&lanes);
    let tracks: Value = serde_json::from_str(TRACK_SNAPSHOT).unwrap();
    let mut waits = rows(&document["waits"]);
    waits.extend(rows(&outside["waits"]));
    waits.extend(rows(&tracks["waits"]));
    assert_engine_fields::<Wait>(&waits);
    let projects = rows(&document["projects"]);
    assert_eq!(
        fields_the_engine_lacks::<Project>(&projects, &[]),
        Vec::<String>::new()
    );
    let decided = rows(&document["decided"]);
    assert_eq!(
        fields_the_engine_lacks::<Decided>(&decided, &[]),
        Vec::<String>::new()
    );
}

#[test]
fn every_field_the_app_reads_from_a_lane_s_parts_is_one_the_engine_prints() {
    let document = document();
    let lanes = rows(&document["items"]);
    let part =
        |name: &str| -> Vec<&Value> { lanes.iter().flat_map(|lane| rows(&lane[name])).collect() };
    assert_eq!(
        fields_the_engine_lacks::<Progress>(&part("progress"), &[]),
        Vec::<String>::new()
    );
    assert_eq!(
        fields_the_engine_lacks::<LaneStep>(&part("steps"), &[]),
        Vec::<String>::new()
    );
    assert_eq!(
        fields_the_engine_lacks::<LaneReview>(&part("reviews"), &[]),
        Vec::<String>::new()
    );
    let delivery = [
        ("kind", NO_EXAMPLE),
        ("head", NO_EXAMPLE),
        ("branch", NO_EXAMPLE),
        ("url", NO_EXAMPLE),
    ];
    assert_eq!(
        fields_the_engine_lacks::<Delivery>(&part("delivery"), &delivery),
        Vec::<String>::new()
    );
    let outside = outside_document();
    let links: Vec<&Value> = rows(&outside["items"])
        .iter()
        .flat_map(|lane| rows(&lane["links"]))
        .collect();
    assert_engine_fields::<Link>(&links);
    let actions: Vec<&Value> = lanes
        .iter()
        .flat_map(|lane| rows(&lane["actions"]))
        .collect();
    assert_eq!(
        fields_the_engine_lacks::<Action>(&actions, &[]),
        Vec::<String>::new()
    );
}

#[test]
fn every_field_the_app_reads_from_the_engine_s_own_parts_is_one_the_engine_prints() {
    let document = document();
    let outside = outside_document();
    assert_engine_fields::<Event>(&rows(&outside["events"]));
    let goose: Value =
        serde_json::from_str(include_str!("fixtures/engine-real-goose-snapshot.json")).unwrap();
    let mut accounts = rows(&document["accounts"]);
    accounts.extend(rows(&goose["accounts"]));
    assert_engine_fields::<Account>(&accounts);
    let mut engine = rows(&document["engine"]);
    engine.extend(rows(&goose["engine"]));
    assert_engine_fields::<EngineState>(&engine);
    let keeper = rows(&document["engine"]["keeper"]);
    assert_eq!(
        fields_the_engine_lacks::<Keeper>(&keeper, &[]),
        Vec::<String>::new()
    );
    let restart = rows(&document["engine"]["keeper"]["last_restart"]);
    assert_eq!(
        fields_the_engine_lacks::<Restart>(&restart, &[]),
        Vec::<String>::new()
    );
    let automations = rows(&document["automations"]["items"]);
    assert_eq!(
        fields_the_engine_lacks::<Automation>(&automations, &[]),
        Vec::<String>::new()
    );
    assert_eq!(
        fields_the_engine_lacks::<Digest>(
            &rows(&document["digest"]),
            &[("problem", "only when the settings cannot be read")]
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        fields_the_engine_lacks::<DigestSource>(&rows(&document["digest"]["sources"]), &[]),
        Vec::<String>::new()
    );
}

fn assert_engine_fields<T: DeserializeOwned>(rows: &[&Value]) {
    let missing = fields_the_engine_lacks::<T>(rows, &[]);
    assert!(
        missing.is_empty(),
        "{}: {missing:?}",
        std::any::type_name::<T>()
    );
}

#[test]
fn scoped_connectors_use_the_real_controls_and_fields() {
    let document = outside_document();
    let controls = &document["connector_controls"];
    let connectors = rows(&document["connectors"]);
    assert_engine_fields::<Connector>(&connectors);
    assert_engine_fields::<ConnectorControls>(&rows(controls));
    assert_engine_fields::<Action>(&rows(&controls["actions"]));
    let scopes: Vec<_> = connectors
        .iter()
        .flat_map(|row| rows(&row["scope"]))
        .collect();
    assert_engine_fields::<ConnectorScope>(&scopes);
    let selectors: Vec<_> = scopes
        .iter()
        .flat_map(|scope| rows(&scope["selectors"]))
        .collect();
    let uncaptured_selectors = [
        ("repo", "the captured GitHub connector has legacy scope"),
        ("branch", "the captured GitHub connector has legacy scope"),
        (
            "whole_repo",
            "the captured GitHub connector has legacy scope",
        ),
        (
            "recipient",
            "the captured Linear connector has no recipient",
        ),
        ("database", "the capture has no Notion connector"),
    ];
    assert_eq!(
        fields_the_engine_lacks::<ConnectorSelectors>(&selectors, &uncaptured_selectors),
        Vec::<String>::new()
    );
    assert_engine_fields::<DependentTask>(
        &connectors
            .iter()
            .flat_map(|row| rows(&row["dependent_tasks"]))
            .collect::<Vec<_>>(),
    );

    let receipt: Value = serde_json::from_str(CONNECTOR_RECEIPT).unwrap();
    assert_engine_fields::<ConnectorReceipt>(&rows(&receipt));
    let receipt: ConnectorReceipt = serde_json::from_value(receipt).unwrap();
    let connectors = outside_snapshot().connectors.unwrap();
    let custom = connectors.iter().find(|row| row.id == receipt.id).unwrap();
    assert_eq!(custom.revision, Some(receipt.revision));
    assert_eq!(
        custom.reads.as_ref().unwrap().text.as_deref(),
        Some("Reads local notes")
    );
    let news = connectors.iter().find(|row| row.label == "News").unwrap();
    let reads = news.reads.as_ref().unwrap();
    assert_eq!(reads.key.as_deref(), Some("connector_reads_feed"));
    assert_eq!(reads.args["host"], "example.com");
    assert_eq!(
        news.event_kinds.as_deref().unwrap(),
        ["entry_added", "new_item"]
    );
}

#[test]
fn the_engine_prints_scheduled_definitions_and_actions() {
    let document = document();
    let scheduled = &document["scheduled"];
    let errors: Value = serde_json::from_str(SCHEDULED_PROBLEMS).unwrap();
    assert_engine_fields::<Scheduled>(&[scheduled, &errors["snapshot"]["scheduled"]]);
    assert_engine_fields::<Template>(&rows(&scheduled["templates"]));
    assert_engine_fields::<Action>(&rows(&scheduled["actions"]));
    let tasks = rows(&scheduled["tasks"]);
    assert_engine_fields::<Definition>(&tasks);
    for task in tasks {
        for field in [
            "id",
            "revision",
            "state",
            "reason",
            "connector_label",
            "next_at",
            "target",
            "actions",
        ] {
            assert!(
                task.get(field).is_some(),
                "scheduled task is missing {field}"
            );
        }
        assert_engine_fields::<Action>(&rows(&task["actions"]));
    }

    let scheduled = snapshot().scheduled.unwrap();
    let daily = &scheduled.templates[0].definitions["en"].when;
    assert!(
        matches!(daily, Trigger::Daily { time, timezone } if time == "07:30" && timezone == "Europe/Rome")
    );
    assert!(scheduled.tasks.iter().any(|task| matches!(
        task.definition.when,
        Trigger::Interval { minutes: 60 }
    ) && task.next_at.is_some()));
    assert!(scheduled.tasks.iter().any(|task| matches!(&task.definition.when, Trigger::ConnectorEvent { connector_id, event_kind } if !connector_id.is_empty() && event_kind == "entry_added")));
}

#[test]
fn the_engine_prints_scheduled_runs_conversations_and_findings() {
    let history: Value = serde_json::from_str(SCHEDULED_HISTORY).unwrap();
    let conversation: Value = serde_json::from_str(SCHEDULED_CONVERSATION).unwrap();
    let errors: Value = serde_json::from_str(SCHEDULED_PROBLEMS).unwrap();
    assert_engine_fields::<HistoryPage>(&[&history, &errors["history"]]);
    assert_engine_fields::<Run>(&rows(&history["runs"]));
    assert_engine_fields::<RunTrigger>(
        &rows(&history["runs"])
            .iter()
            .map(|run| &run["trigger"])
            .collect::<Vec<_>>(),
    );
    assert_engine_fields::<ConversationPage>(&[&conversation, &errors["conversation"]]);
    assert_engine_fields::<ConversationEntry>(&rows(&conversation["entries"]));

    let finding: Value = serde_json::from_str(SCHEDULED_FINDING).unwrap();
    assert_eq!(
        fields_the_engine_lacks::<Wait>(
            &rows(&finding),
            &[(
                "track",
                "only a Track decision wait carries a Track reference"
            )],
        ),
        Vec::<String>::new()
    );
    assert_engine_fields::<WaitProposal>(&rows(&finding["proposal"]));
    assert_engine_fields::<ScheduledOrigin>(&rows(&finding["proposal"]["origin"]));
    let finding: Wait = serde_json::from_value(finding).unwrap();
    let history: HistoryPage = serde_json::from_value(history).unwrap();
    let conversation: ConversationPage = serde_json::from_value(conversation).unwrap();
    let event_run = &history.runs[1];
    let proposal = finding.proposal.unwrap();
    assert_eq!(
        proposal.run_id.as_deref(),
        event_run.conversation.as_deref()
    );
    assert_eq!(Some(proposal.origin.unwrap().task_id), event_run.task_id);
    assert_eq!(history.before, Some(1));
    assert_eq!(conversation.after, Some(1));
    assert_eq!(conversation.entries[0].text, "Summarize new entries.");
}

#[test]
fn the_real_engine_surfaces_scheduled_and_connector_read_problems() {
    let errors: Value = serde_json::from_str(SCHEDULED_PROBLEMS).unwrap();
    let snapshot = Snapshot::parse(errors["snapshot"].to_string().as_bytes()).unwrap();
    assert_eq!(
        snapshot.connectors_problem.as_deref(),
        Some("the connectors cannot be read")
    );
    assert_eq!(
        snapshot.scheduled.unwrap().problem.as_deref(),
        Some("scheduled_store_unavailable")
    );
    let history: HistoryPage = serde_json::from_value(errors["history"].clone()).unwrap();
    assert_eq!(
        history.problem.as_deref(),
        Some("scheduled_store_unavailable")
    );
    let conversation: ConversationPage =
        serde_json::from_value(errors["conversation"].clone()).unwrap();
    assert_eq!(
        conversation.problem.as_deref(),
        Some("scheduled_store_unavailable")
    );
    let expired: ConversationPage = serde_json::from_value(errors["expired"].clone()).unwrap();
    assert_eq!(expired.problem.as_deref(), Some("run_unavailable"));
}

#[test]
fn the_real_feed_page_is_read_with_its_entries_and_cursor() {
    let page: FeedPage = serde_json::from_str(FEED).expect("the engine's own feed page");
    assert_eq!(page.cursor, "build-1=3");
    assert!(!page.more);
    let [words, call, shell] = page.entries.as_slice() else {
        panic!("three entries: {:?}", page.entries);
    };
    assert!(!words.is_tool() && call.is_tool());
    assert_eq!(
        (shell.tool.as_str(), shell.command.as_str()),
        ("Bash", "pytest -q"),
        "a shell call keeps the first line of its command"
    );
    assert_eq!(call.command, "", "a call that runs no command has none");
    assert_eq!(
        (words.run.as_str(), words.role.as_str()),
        ("build-1", "builder")
    );
    assert_eq!(words.text, "Adding the --help flag.");
    assert_eq!((call.tool.as_str(), call.path.as_str()), ("Edit", "cli.py"));
    assert_eq!(call.line, None);
}

#[test]
fn every_field_the_app_reads_from_the_feed_is_one_the_engine_prints() {
    let document: Value = serde_json::from_str(FEED).expect("the feed");
    let kinds: Value = serde_json::from_str(FEED_KINDS).expect("the feed with every entry kind");
    let mut entries = rows(&document["entries"]);
    entries.extend(rows(&kinds["entries"]));
    assert_eq!(
        fields_the_engine_lacks::<FeedPage>(&rows(&document), &[]),
        Vec::<String>::new()
    );
    assert_eq!(
        fields_the_engine_lacks::<FeedEntry>(
            &entries,
            &[
                ("truncated", "only on an entry the engine cut"),
                (
                    "started_at",
                    "only on a tool entry of one agent's conversation"
                ),
                (
                    "ended_at",
                    "only on a tool entry of one agent's conversation, once it ended"
                ),
                ("id", "only on an entry of one agent's conversation"),
                (
                    "pattern",
                    "only on a search call of one agent's conversation"
                ),
                (
                    "technical",
                    "only on an engine entry of one agent's conversation, from an engine that gives plain words"
                ),
                (
                    "continued",
                    "only on a part of a long text in one agent's conversation"
                ),
            ]
        ),
        Vec::<String>::new()
    );
}

#[test]
fn the_real_master_spec_is_read_as_the_adapter_start_the_app_makes_from_it() {
    let spec: MasterSpec = serde_json::from_str(MASTER_SPEC).expect("the engine's own spec");
    assert!(spec.argv.is_empty(), "the sample home names no adapter");
    assert_eq!(spec.start_mode, "default");
    assert!(spec.close_sessions);
    assert_eq!(spec.mcp_servers[0]["name"], "sakura");
    assert!(spec.env_allow.contains(&"SAKURA_ENGINE_HOME".to_string()));
    assert!(spec
        .system_prompt_append
        .starts_with("# You are the master"));
    assert!(!spec.node.is_empty() && !spec.path.is_empty());
    let meta = spec.session_meta();
    assert_eq!(
        meta["systemPrompt"]["append"].as_str(),
        Some(spec.system_prompt_append.as_str())
    );
    assert_eq!(meta["claudeCode"]["options"]["model"], "opus");
}

#[test]
fn every_field_the_app_reads_from_the_master_spec_is_one_the_engine_prints() {
    let document: Value = serde_json::from_str(MASTER_SPEC).expect("the spec");
    let goose: Value =
        serde_json::from_str(include_str!("fixtures/engine-real-goose-master-spec.json")).unwrap();
    assert_engine_fields::<MasterSpec>(&[&document, &goose]);
}

#[test]
fn the_real_master_spec_says_why_it_names_no_adapter_in_the_engine_s_words() {
    let spec: MasterSpec = serde_json::from_str(MASTER_SPEC).expect("the engine's own spec");
    assert!(
        spec.adapter_problem.contains("SAKURA_ACP_ADAPTER"),
        "{:?}",
        spec.adapter_problem
    );
}

#[test]
fn the_real_master_decision_is_read_with_its_reason() {
    let decided: MasterDecided =
        serde_json::from_str(MASTER_DECIDE).expect("the engine's own decision");
    assert_eq!(decided.decision, Decision::Ask);
    assert_eq!(decided.reason, "no deny rule of the master covers Bash");
    let document: Value = serde_json::from_str(MASTER_DECIDE).expect("the decision");
    assert_eq!(
        fields_the_engine_lacks::<MasterDecided>(&rows(&document), &[]),
        Vec::<String>::new()
    );
}

fn outside_document() -> Value {
    serde_json::from_str(OUTSIDE_SNAPSHOT).expect("the engine's own snapshot with an outside lane")
}

fn outside_snapshot() -> Snapshot {
    Snapshot::parse(OUTSIDE_SNAPSHOT.as_bytes()).expect("the app reads it")
}

#[test]
fn the_real_morning_digest_is_its_own_kind_with_no_project_and_its_day() {
    let snapshot = outside_snapshot();
    let digest = snapshot
        .items
        .iter()
        .find(|item| item.is_digest())
        .expect("the digest item");
    assert_eq!(digest.name(), "Morning digest");
    assert_eq!(digest.date.as_deref(), Some("2026-11-18"));
    assert_eq!(
        digest.repo, "",
        "a digest has no repository, not a null one"
    );
    assert_eq!(digest.task(), "digest-2026-11-18");
    let asks: Vec<&Wait> = snapshot
        .waits
        .iter()
        .filter(|wait| wait.is_digest())
        .collect();
    assert_eq!(asks.len(), 2);
    assert!(asks.iter().all(|wait| wait.lane_key() == digest.lane_key()));
    assert_eq!(asks[0].digest.as_deref(), Some("2026-11-18"));
    assert_eq!(
        (asks[0].project.as_str(), asks[0].lane_name()),
        ("", "Morning digest")
    );
}

#[test]
fn the_real_morning_digest_reads_its_news_with_their_sources_and_its_asks_with_their_state() {
    let snapshot = outside_snapshot();
    let digest = snapshot.items.iter().find(|item| item.is_digest());
    let reading = digest
        .and_then(|item| item.reading.as_ref())
        .expect("a reading");
    assert_eq!(reading.date, "2026-11-18");
    let news: Vec<(&str, &str, usize)> = reading
        .news
        .iter()
        .map(|news| (news.source.as_str(), news.title.as_str(), news.links.len()))
        .collect();
    assert_eq!(news, [("github", "Release", 1), ("news", "Outage", 0)]);
    assert_eq!(reading.news[0].links[0].label, "PR 12");
    let asks: Vec<(&str, &str, bool)> = reading
        .asks
        .iter()
        .map(|ask| (ask.kind.as_str(), ask.title.as_str(), ask.is_open()))
        .collect();
    assert_eq!(
        asks,
        [
            ("owner_row", "Check PR 12", true),
            ("question", "Ship today?", true)
        ]
    );
    assert_eq!(reading.asks[1].choices, ["yes", "no"]);
    assert_eq!(
        reading.asks[0].wait.as_deref(),
        snapshot.waits.first().map(|wait| wait.id.as_str())
    );
}

#[test]
fn a_real_outside_row_with_one_step_arrives_as_the_sentence_and_its_links_as_links() {
    let snapshot = outside_snapshot();
    let row = snapshot
        .waits
        .iter()
        .find(|wait| wait.task == "measure-help" && wait.original_kind() == "owner_row");
    let row = row.expect("the row");
    assert_eq!(row.kind, "approval");
    assert_eq!(
        row.detail,
        "Run measure.py --help\nExpected: usage: measure"
    );
    let digest_row = snapshot
        .waits
        .iter()
        .find(|wait| wait.is_digest() && wait.original_kind() == "owner_row");
    assert_eq!(
        digest_row.expect("a check").links[0].url,
        "https://github.com/example/orchard/pull/12"
    );
}

#[test]
fn every_field_the_app_reads_from_a_digest_is_one_the_engine_prints() {
    let document = outside_document();
    let items = rows(&document["items"]);
    let digest = items
        .iter()
        .find(|item| item["kind"] == "digest")
        .expect("a digest");
    for name in ["date", "reading", "title", "links", "goal", "progress"] {
        assert!(digest.get(name).is_some(), "the digest item has no {name}");
    }
    let reading = rows(&digest["reading"]);
    assert_eq!(
        fields_the_engine_lacks::<Reading>(&reading, &[]),
        Vec::<String>::new()
    );
    let news = rows(&digest["reading"]["news"]);
    assert_eq!(
        fields_the_engine_lacks::<ReadingNews>(&news, &[]),
        Vec::<String>::new()
    );
    let asks = rows(&digest["reading"]["asks"]);
    let answer = [("answer", "only on a decided ask")];
    assert_eq!(
        fields_the_engine_lacks::<ReadingAsk>(&asks, &answer),
        Vec::<String>::new()
    );
    let waits = rows(&document["waits"]);
    assert_eq!(
        fields_the_engine_lacks::<Wait>(
            &waits,
            &[(
                "track",
                "only a Track decision wait carries a Track reference"
            )],
        ),
        Vec::<String>::new()
    );
}

#[test]
fn track_collection_projects_decisions_and_reservations_come_from_the_engine() {
    use sakura_core::engine::tracks::{TrackDecisionState, TrackLaneState, TrackStatus, Tracks};
    let snapshot = Snapshot::parse(TRACK_SNAPSHOT.as_bytes()).unwrap();
    let rows = snapshot.tracks.rows();
    let find = |name| rows.iter().find(|track| track.name == name).unwrap();
    let release = find("Release");
    let research = find("Research");
    assert_eq!(release.status, TrackStatus::Open);
    assert_eq!(research.status, TrackStatus::Paused);
    assert_eq!(find("Previous release").status, TrackStatus::Closed);
    assert!(snapshot.tracks.action("tracknew").is_some());
    assert_eq!(
        snapshot.track_of_conversation(&release.conversations[0].key),
        Some(release)
    );
    let wait = snapshot
        .waits
        .iter()
        .find(|wait| wait.original_kind() == "track_decision")
        .unwrap();
    assert_eq!(
        snapshot.track(&wait.track.as_ref().unwrap().id),
        Some(release)
    );
    assert!(release
        .decisions
        .iter()
        .any(|decision| decision.state == TrackDecisionState::Open
            && decision.wait_id.as_deref() == Some(wait.id.as_str())));
    assert!(release
        .decisions
        .iter()
        .any(|decision| decision.state == TrackDecisionState::Settled
            && decision.answer.as_deref() == Some("Yes, on the beta channel.")));
    assert!(snapshot
        .decided
        .unwrap()
        .iter()
        .any(|done| done.place.as_deref() == Some("phone")));
    assert!(research.projects[0].folder_missing && research.lanes[0].folder_missing);
    assert!(release
        .lanes
        .iter()
        .any(|lane| lane.gone && lane.key.is_none()));
    let failed = release
        .lanes
        .iter()
        .find(|lane| lane.state == TrackLaneState::Interrupted)
        .unwrap();
    assert!(failed.did_not_start && failed.startop.is_some());
    assert!(failed
        .actions
        .iter()
        .any(|action| action.key == "trackretry"));

    let legacy = Snapshot::parse(include_bytes!("fixtures/engine-snapshot.json")).unwrap();
    assert_eq!(legacy.tracks, Tracks::Unsupported);
    assert!(legacy.items.iter().all(|item| item.track.is_none()));
    let unavailable = Snapshot::parse(include_bytes!(
        "fixtures/engine-real-tracks-unavailable.json"
    ))
    .unwrap();
    assert!(matches!(unavailable.tracks, Tracks::Unavailable(ref message) if !message.is_empty()));
    assert!(!unavailable.items.is_empty() && !unavailable.waits.is_empty());
}

#[test]
fn goose_setup_applies_the_checked_folder_and_returns_the_snapshot_default() {
    use sakura_core::engine::setup::{SetupApplied, SetupDetection};

    let setup: SetupDetection =
        serde_json::from_str(include_str!("fixtures/engine-real-goose-setup.json")).unwrap();
    assert_eq!(
        setup
            .harnesses
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["claude", "codex", "goose"]
    );
    let goose = setup
        .logins
        .iter()
        .find(|row| row.harness == "goose")
        .unwrap();
    assert!(!goose.logged_in);
    assert!(goose.readiness.is_some());

    let result: Value =
        serde_json::from_str(include_str!("fixtures/engine-real-goose-apply.json")).unwrap();
    let applied: SetupApplied = serde_json::from_str(result["output"].as_str().unwrap()).unwrap();
    let snapshot =
        Snapshot::parse(include_bytes!("fixtures/engine-real-goose-snapshot.json")).unwrap();
    let engine = snapshot.engine.unwrap();
    let master = applied.master_account.unwrap();
    assert_eq!(engine.master_account.as_ref(), Some(&master));
    assert_eq!(engine.master_launch_route.as_deref(), Some("acp"));
    let account = snapshot
        .accounts
        .iter()
        .find(|row| row.id == master.id)
        .unwrap();
    assert_eq!(account.provider_config.as_ref().unwrap().provider, "ollama");
    assert_eq!(account.forecast_available, Some(false));
    assert!(account.action("accountcheck").is_some());
}

#[test]
fn goose_rules_keep_the_stale_account_catalog_and_the_provider_default_effort() {
    use sakura_core::engine::rules::AgentAccount;
    let rules: Rules =
        serde_json::from_str(include_str!("fixtures/engine-real-goose-rules.json")).unwrap();
    let agent = rules
        .agents
        .iter()
        .find(|row| row.provider == "goose")
        .unwrap();
    let listed: Value =
        serde_json::from_str(include_str!("fixtures/engine-real-goose-catalog.json")).unwrap();
    let catalog: Vec<AgentAccount> = serde_json::from_value(listed["accounts"].clone()).unwrap();
    let kept = catalog.iter().find(|row| row.provider == "goose").unwrap();
    assert_eq!((&agent.id, &agent.models), (&kept.id, &kept.models));
    assert_eq!(kept.upstream_provider.as_deref(), Some("ollama"));
    assert_eq!(kept.models_reason.as_deref(), Some("adapter_timeout"));
    assert_eq!(agent.models_state, "stale");
    let [model] = agent.models.as_slice() else {
        panic!("one provider model")
    };
    assert_eq!(model.label, "Qwen 2.5 (Ollama)");
    assert!(model.is_default && model.efforts.is_empty());
}

#[test]
fn goose_master_keeps_the_named_slot_and_connection_check_reason() {
    use sakura_core::engine::accounts::Readiness;
    let spec: MasterSpec =
        serde_json::from_str(include_str!("fixtures/engine-real-goose-master-spec.json")).unwrap();
    let launch = spec.acp.unwrap();
    assert_eq!(launch.protocol_version, 1);
    assert_eq!(launch.slot, "fixture-tab");
    assert_eq!(launch.readiness, Readiness::Unavailable);
    assert_eq!(
        launch.reason.as_deref(),
        Some("goose_account_connection_check_required")
    );
    assert!(launch.argv.is_empty());
}
