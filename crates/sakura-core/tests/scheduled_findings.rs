use sakura_core::engine::snapshot::Snapshot;

#[test]
fn scheduled_finding_retains_named_provenance_and_optional_run() {
    let bytes = include_bytes!("fixtures/scheduled-finding.json");
    let snapshot = Snapshot::parse(bytes).unwrap();
    let proposal = snapshot.waits[0].proposal.as_ref().unwrap();
    let origin = proposal.origin.as_ref().unwrap();
    assert_eq!(origin.title, "Morning summary");
    assert_eq!(origin.source.as_deref(), Some("News feed"));
    assert_eq!(proposal.run_id.as_deref(), Some("saved-run"));

    let mut expired: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    expired["waits"][0]["proposal"]["run_id"] = serde_json::Value::Null;
    let expired = Snapshot::parse(&serde_json::to_vec(&expired).unwrap()).unwrap();
    let proposal = expired.waits[0].proposal.as_ref().unwrap();
    assert!(proposal.run_id.is_none());
    assert_eq!(proposal.origin.as_ref().unwrap().title, "Morning summary");
}
