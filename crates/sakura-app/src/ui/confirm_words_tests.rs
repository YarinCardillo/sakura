//! What a confirmation says: the lane and the action in words, never the engine's command line.

use super::confirm_words::{confirm_statement, confirm_words, plain_summary, summary_adds_words};
use super::engine_rig_tests::SNAPSHOT;
use sakura_core::engine::cli::Act;
use sakura_core::engine::snapshot::{Action, Snapshot};
use std::collections::BTreeMap;

const ENGINE_SUMMARY: &str = "Stop demo: run in /work/repo: sakura flow stop demo";

fn snapshot() -> Snapshot {
    Snapshot::parse(SNAPSHOT.as_bytes()).expect("the fixture")
}

fn act(snapshot: &Snapshot, key: &str, target: &str) -> Act {
    let known = snapshot
        .waits
        .iter()
        .flat_map(|wait| &wait.actions)
        .chain(snapshot.items.iter().flat_map(|item| &item.actions))
        .find(|action| action.key == key)
        .cloned();
    Act {
        action: known.unwrap_or(Action {
            key: key.to_string(),
            label: "Close lane".to_string(),
            needs: Vec::new(),
            optional: Vec::new(),
            confirm: true,
        }),
        target: target.to_string(),
        inputs: BTreeMap::new(),
        confirm: None,
    }
}

fn words(key: &str, target: &str) -> String {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let snapshot = snapshot();
    confirm_words(
        Some(&snapshot),
        &act(&snapshot, key, target),
        ENGINE_SUMMARY,
    )
    .to_string()
}

#[test]
fn approving_a_spec_names_the_spec_and_the_lane() {
    let spec = r#"["wait","/work/repo","gated","approval","spec"]"#;
    assert_eq!(words("a", spec), "Approve the spec of gated?");
}

/// The snapshot with its spec wait of the lane `gated` turned into an approval of `gate`.
fn at_gate(gate: &str) -> Snapshot {
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let waits = document["waits"].as_array_mut().expect("the waits");
    let spec = waits
        .iter_mut()
        .find(|wait| wait["key"][4] == "spec")
        .expect("the spec wait");
    spec["key"][4] = gate.into();
    spec["id"] = format!(r#"["wait","/work/repo","gated","approval","{gate}"]"#).into();
    Snapshot::parse(document.to_string().as_bytes()).expect("a snapshot")
}

#[test]
fn approving_a_gate_says_what_it_means_for_the_person_never_the_engine_s_name_for_it() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let cases = [
        (
            "overlap",
            "Approve delivering gated despite an overlap?",
            "Approving the delivery of gated despite an overlap",
        ),
        (
            "checks",
            "Approve the commands of the checks of gated?",
            "Approving the checks of gated",
        ),
        (
            "budget",
            "Approve the budget of gated?",
            "Approving the budget of gated",
        ),
        ("something-new", "Approve gated?", "Approving gated"),
    ];
    for (gate, question, statement) in cases {
        let snapshot = at_gate(gate);
        let target = format!(r#"["wait","/work/repo","gated","approval","{gate}"]"#);
        let act = act(&snapshot, "a", &target);
        assert_eq!(
            confirm_words(Some(&snapshot), &act, ENGINE_SUMMARY),
            question,
            "{gate}"
        );
        assert_eq!(
            confirm_statement(Some(&snapshot), &act, ENGINE_SUMMARY),
            statement,
            "{gate}"
        );
    }
}

#[test]
fn the_engine_s_summary_is_kept_under_a_toast_only_where_the_app_s_sentence_leaves_something_out() {
    let snapshot = snapshot();
    let spec = act(
        &snapshot,
        "a",
        r#"["wait","/work/repo","gated","approval","spec"]"#,
    );
    assert!(
        summary_adds_words(Some(&snapshot), &spec),
        "what to do with a spec"
    );
    let demo = r#"["flow","/work/repo","demo"]"#;
    for key in ["R", "s"] {
        let act = act(&snapshot, key, demo);
        assert!(
            !summary_adds_words(Some(&snapshot), &act),
            "{key}: the engine's line names the slug"
        );
    }
    let overlap = at_gate("overlap");
    let target = r#"["wait","/work/repo","gated","approval","overlap"]"#;
    assert!(!summary_adds_words(
        Some(&overlap),
        &act(&overlap, "a", target)
    ));
}

#[test]
fn approving_from_the_lane_page_names_the_gate_the_lane_waits_at() {
    assert_eq!(
        words("a", r#"["flow","/work/repo","gated"]"#),
        "Approve the spec of gated?"
    );
}

#[test]
fn approving_a_lane_that_waits_at_no_gate_names_the_lane() {
    assert_eq!(
        words("a", r#"["flow","/work/repo","demo"]"#),
        "Approve demo?"
    );
}

#[test]
fn stopping_and_resuming_name_the_lane() {
    let demo = r#"["flow","/work/repo","demo"]"#;
    assert_eq!(words("s", demo), "Stop demo?");
    assert_eq!(words("R", demo), "Resume demo?");
}

#[test]
fn an_action_the_page_has_no_words_for_says_its_label_and_the_lane() {
    let outside = r#"["outside","measure-help"]"#;
    assert_eq!(words("d", outside), "Close lane: Add --help to measure.py?");
}

#[test]
fn a_target_the_snapshot_no_longer_lists_still_says_the_action() {
    assert_eq!(words("s", r#"["flow","/work/gone","x"]"#), "Stop?");
}

#[test]
fn a_proposal_keeps_the_engines_own_sentence_which_binds_the_text() {
    assert_eq!(
        words("y", r#"["wait","/work/repo","gated","approval","spec"]"#),
        ENGINE_SUMMARY
    );
}

#[test]
fn no_confirmation_shows_a_command_or_a_host_path() {
    for key in ["a", "R", "s", "d", "k"] {
        for target in [
            r#"["wait","/work/repo","gated","approval","spec"]"#,
            r#"["flow","/work/repo","demo"]"#,
            r#"["flow","/work/gone","x"]"#,
        ] {
            let said = words(key, target);
            assert!(
                !said.contains("sakura") && !said.contains("/work"),
                "{said}"
            );
        }
    }
}

fn statement(key: &str, target: &str) -> String {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let snapshot = snapshot();
    confirm_statement(
        Some(&snapshot),
        &act(&snapshot, key, target),
        ENGINE_SUMMARY,
    )
    .to_string()
}

#[test]
fn a_toast_says_what_is_being_done_as_a_statement_and_not_as_a_question() {
    let spec = r#"["wait","/work/repo","gated","approval","spec"]"#;
    let demo = r#"["flow","/work/repo","demo"]"#;
    assert_eq!(statement("a", spec), "Approving the spec of gated");
    assert_eq!(statement("a", demo), "Approving demo");
    assert_eq!(statement("s", demo), "Stopping demo");
    assert_eq!(statement("R", demo), "Resuming demo");
    assert_eq!(statement("s", r#"["flow","/work/gone","x"]"#), "Stop");
    let outside = r#"["outside","measure-help"]"#;
    assert_eq!(
        statement("d", outside),
        "Close lane: Add --help to measure.py"
    );
    for said in [
        statement("a", spec),
        statement("s", demo),
        statement("d", outside),
    ] {
        assert!(!said.ends_with('?'), "{said}");
    }
}

#[test]
fn a_proposal_statement_is_the_engines_own_sentence() {
    let spec = r#"["wait","/work/repo","gated","approval","spec"]"#;
    assert_eq!(statement("y", spec), ENGINE_SUMMARY);
}

#[test]
fn the_engine_s_summary_is_shown_without_its_command_line_or_host_path() {
    assert_eq!(
        plain_summary("Approve on gated: run in /work/repo: sakura flow approve gated spec")
            .as_deref(),
        Some("Approve on gated")
    );
    assert_eq!(
        plain_summary("The spec was edited").as_deref(),
        Some("The spec was edited")
    );
    assert_eq!(plain_summary("sakura flow stop demo"), None);
    assert_eq!(plain_summary("Stop demo in /work/repo"), None);
    assert_eq!(plain_summary("  "), None);
}

#[test]
fn fix36_checks_approval_keeps_the_engine_summary_under_the_short_statement() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let snapshot = at_gate("checks");
    let target = r#"["wait","/work/repo","gated","approval","checks"]"#;
    let act = act(&snapshot, "a", target);
    assert!(summary_adds_words(Some(&snapshot), &act));
    assert_eq!(
        confirm_statement(Some(&snapshot), &act, ENGINE_SUMMARY),
        "Approving the checks of gated"
    );
}
