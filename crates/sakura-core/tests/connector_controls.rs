use sakura_core::engine::cli::ActResult;
use sakura_core::engine::snapshot::Snapshot;

const DOCUMENT: &str = include_str!("fixtures/connector-controls.json");

#[test]
fn connector_control_capability_and_registry_errors_are_independent() {
    let old =
        Snapshot::parse(br#"{"api":1,"connectors":[{"id":"github","kind":"github"}]}"#).unwrap();
    assert!(old.connector_controls.is_none());
    assert!(old.connectors.unwrap()[0].revision.is_none());

    let snapshot = Snapshot::parse(DOCUMENT.as_bytes()).unwrap();
    let controls = snapshot.connector_controls.unwrap();
    assert_eq!(controls.version, 1);
    assert_eq!(controls.target, r#"["host","connectors"]"#);
    assert!(controls.actions.iter().all(|action| action.confirm));
    let connector = &snapshot.connectors.unwrap()[0];
    assert_eq!(connector.revision, Some(1));
    assert_eq!(
        connector.scope.as_ref().unwrap().labels["resource"],
        "Acme/Website"
    );
    assert_eq!(
        connector.dependent_tasks.as_ref().unwrap()[0].label,
        "Check the staging build"
    );

    let mut empty: serde_json::Value = serde_json::from_str(DOCUMENT).unwrap();
    empty["connectors"] = serde_json::json!([]);
    let parse = |value: &serde_json::Value| Snapshot::parse(value.to_string().as_bytes()).unwrap();
    assert!(parse(&empty).connectors.unwrap().is_empty());
    assert!(parse(&empty).connectors_problem.is_none());
    empty["connectors_problem"] = "the connectors cannot be read".into();
    assert!(parse(&empty).connectors_problem.is_some());
}

#[test]
fn connector_action_replies_preserve_safe_preview_and_revision() {
    let preview: ActResult = serde_json::from_str(r#"{"result":"confirm","token":"tk","expires":"2026-11-23T12:00:00Z","display_args":{"label":"Custom reader","candidate_label":"Website PRs"}}"#).unwrap();
    assert!(
        matches!(preview, ActResult::Confirm { display_args: Some(args), .. } if args["candidate_label"] == "Website PRs")
    );
    let done: ActResult = serde_json::from_str(r#"{"result":"done","exit_code":0,"output":"{\"id\":\"connection-id\",\"revision\":2}","connector":{"id":"connection-id","revision":2}}"#).unwrap();
    assert!(
        matches!(done, ActResult::Done { connector: Some(receipt), .. } if receipt.revision == 2)
    );
    let refusal: ActResult = serde_json::from_str(r#"{"result":"refused","reason":"stale_revision","reason_code":"stale_revision","reason_args":{}}"#).unwrap();
    assert!(
        matches!(refusal, ActResult::Refused { reason_code: Some(code), .. } if code == "stale_revision")
    );
}

#[test]
fn custom_reads_accepts_the_scheduled_engines_plain_sentence() {
    let snapshot = Snapshot::parse(
        br#"{
        "api": 1,
        "scheduled": {"tasks": []},
        "connectors": [{"id":"program-notes", "kind":"program", "reads":"Reads morning notes."}]
    }"#,
    )
    .unwrap();
    let rows = snapshot.connectors.unwrap();
    assert_eq!(
        rows[0].reads.as_ref().unwrap().text.as_deref(),
        Some("Reads morning notes.")
    );
}
