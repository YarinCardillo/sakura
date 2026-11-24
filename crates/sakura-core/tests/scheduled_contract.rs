use sakura_core::engine::scheduled::{ConversationPage, HistoryPage, Trigger};
use sakura_core::engine::snapshot::Snapshot;
use sakura_core::tabs::{Page, Tabs};

#[test]
fn scheduled_snapshot_distinguishes_absent_empty_and_malformed() {
    let parse = |text: &str| Snapshot::parse(text.as_bytes());
    assert!(parse(r#"{"api":1}"#).unwrap().scheduled.is_none());
    assert!(parse(r#"{"api":1,"scheduled":{"tasks":[]}}"#)
        .unwrap()
        .scheduled
        .unwrap()
        .tasks
        .is_empty());
    assert!(parse(r#"{"api":1,"scheduled":{"tasks":"wrong"}}"#).is_err());

    let snapshot = Snapshot::parse(include_bytes!("fixtures/scheduled.json")).unwrap();
    let tasks = snapshot.scheduled.unwrap().tasks;
    assert_eq!(tasks[0].revision, "1");
    assert!(matches!(tasks[0].definition.when, Trigger::Daily { .. }));
    assert!(!tasks[0].definition.may_start_lanes);
    assert!(tasks[0].definition.enabled);
}

#[test]
fn history_keeps_failed_and_skipped_runs_and_expired_conversations() {
    let history: HistoryPage = serde_json::from_str(
        r#"{"runs":[{"id":"f","state":"failed","reason":"all_sources_failed"},
                    {"id":"s","state":"skipped"}],"before":42}"#,
    )
    .unwrap();
    assert_eq!(
        history.runs[0].reason.as_deref(),
        Some("all_sources_failed")
    );
    assert_eq!(history.runs[1].state, "skipped");
    assert_eq!(history.before, Some(42));
    let expired: ConversationPage =
        serde_json::from_str(r#"{"problem":"run_unavailable"}"#).unwrap();
    assert!(expired.entries.is_empty());
    assert_eq!(expired.problem.as_deref(), Some("run_unavailable"));
}

#[test]
fn scheduled_tab_restores() {
    let mut tabs = Tabs::new();
    tabs.navigate(Page::Scheduled);
    assert_eq!(
        Tabs::restore(Some(&tabs.to_json())).active().page(),
        &Page::Scheduled
    );
}
