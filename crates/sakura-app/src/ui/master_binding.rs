//! Which lane or proposal of the engine a card of the master stands for. What the master's tools answered is
//! the agent's own text, so it is never trusted to point at anything but by an exact value: a field of its
//! JSON answer that equals an id the engine lists, for exactly one lane or one proposal. Anything less (text
//! that merely mentions a name, two matches, none) binds nothing, and a card that binds nothing offers no
//! action on the engine's facts. What a bound card shows is what the engine says, never the tool's input.

use super::thread::Proposal;
use sakura_core::engine::snapshot::{Item, Snapshot, Wait};
use serde_json::Value;

/// The fields of a `lane_open` answer that may name the lane.
const LANE_FIELDS: [&str; 5] = ["lane", "lane_id", "id", "key", "task"];
/// The fields of a `knowledge_propose` answer that may name the proposal.
const PROPOSAL_FIELDS: [&str; 6] = ["proposal", "proposal_id", "wait", "wait_id", "id", "key"];
/// The kind of the wait the engine opens for a proposal of the master.
const KIND_PROPOSAL: &str = "proposal";

/// The strings the JSON `answer` holds in `fields` at its top level; none when it is not a JSON object.
fn named_values(answer: &str, fields: &[&str]) -> Vec<String> {
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(answer.trim()) else {
        return Vec::new();
    };
    let named = fields
        .iter()
        .filter_map(|field| object.get(*field)?.as_str());
    named.map(str::to_string).collect()
}

/// The one thing of `candidates` that `is_named` by an exact value; none when no value names one, or when
/// more than one is named.
fn only<'a, T>(
    candidates: impl Iterator<Item = &'a T>,
    is_named: impl Fn(&T) -> bool,
) -> Option<&'a T> {
    let mut named = candidates.filter(|candidate| is_named(candidate));
    let found = named.next()?;
    named.next().is_none().then_some(found)
}

/// The lane a `lane_open` answer names, by the lane's id, key or task id, exactly, when one lane is named.
pub(super) fn lane_named<'a>(snapshot: &'a Snapshot, answer: &str) -> Option<&'a Item> {
    let names = named_values(answer, &LANE_FIELDS);
    let is_named = |item: &Item| {
        let (key, task) = (item.lane_key(), item.task());
        names
            .iter()
            .any(|name| *name == item.id || *name == key || (!task.is_empty() && name == task))
    };
    only(snapshot.items.iter(), is_named)
}

/// The proposal a `knowledge_propose` answer names, by the wait's id or its name, exactly, when one proposal
/// the engine waits on is named; its title and detail are the engine's own.
pub(super) fn proposal_of(snapshot: &Snapshot, answer: &str) -> Proposal {
    let names = named_values(answer, &PROPOSAL_FIELDS);
    let is_named = |wait: &Wait| {
        let name = wait.key_name();
        wait.original_kind() == KIND_PROPOSAL
            && names
                .iter()
                .any(|named| *named == wait.id || (!name.is_empty() && named == name))
    };
    match only(snapshot.waits.iter(), is_named) {
        Some(wait) => Proposal::Waiting {
            wait: wait.id.clone(),
            title: wait.title.clone(),
            detail: wait.detail.clone(),
        },
        None => Proposal::Unbound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAPSHOT: &str = include_str!("../../../sakura-core/tests/fixtures/engine-snapshot.json");

    fn snapshot() -> Snapshot {
        Snapshot::parse(SNAPSHOT.as_bytes()).expect("the fixture")
    }

    /// The fixture with two proposals of the master waiting, `note-1` and `note-2`.
    fn with_two_proposals() -> Snapshot {
        let mut value: Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
        for (name, title, detail) in [
            ("note-1", "First note", "one"),
            ("note-2", "Second note", "two"),
        ] {
            let wait = serde_json::json!({
                "id": format!(r#"["wait","/work/repo","master","proposal","{name}"]"#),
                "key": ["wait", "/work/repo", "master", "proposal", name],
                "kind": "proposal", "repo": "/work/repo", "project": "repo", "task": "master",
                "title": title, "detail": detail, "since": null, "actions": [],
            });
            value["waits"].as_array_mut().expect("waits").push(wait);
        }
        Snapshot::parse(value.to_string().as_bytes()).expect("a snapshot")
    }

    fn waiting(snapshot: &Snapshot, answer: &str) -> Option<(String, String, String)> {
        match proposal_of(snapshot, answer) {
            Proposal::Waiting {
                wait,
                title,
                detail,
            } => Some((wait, title, detail)),
            Proposal::Unbound => None,
        }
    }

    #[test]
    fn a_lane_is_bound_by_an_exact_field_of_the_answer_and_by_nothing_looser() {
        let snapshot = snapshot();
        let named =
            |answer: &str| lane_named(&snapshot, answer).map(|item| item.task().to_string());
        assert_eq!(named(r#"{"lane_id": "demo"}"#).as_deref(), Some("demo"));
        assert_eq!(named(r#"{"key": "gated"}"#).as_deref(), Some("gated"));
        assert_eq!(
            named(r#"{"lane": "dem"}"#),
            None,
            "a part of a name is no name"
        );
        assert_eq!(
            named("Started lane stuck in /work/other"),
            None,
            "prose that mentions a lane binds none"
        );
        assert_eq!(
            named(r#"{"note": "demo"}"#),
            None,
            "only the fields that name a lane count"
        );
        assert_eq!(named(""), None);
    }

    #[test]
    fn a_lane_name_two_lanes_share_binds_none() {
        let mut value: Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
        let items = value["items"].as_array_mut().expect("items");
        let mut twin = items[0].clone();
        twin["id"] = serde_json::json!(r#"["flow","/work/elsewhere","demo"]"#);
        twin["key"] = serde_json::json!(["flow", "/work/elsewhere", "demo"]);
        twin["repo"] = serde_json::json!("/work/elsewhere");
        items.push(twin);
        let snapshot = Snapshot::parse(value.to_string().as_bytes()).expect("a snapshot");
        assert!(lane_named(&snapshot, r#"{"lane": "demo"}"#).is_none());
        let by_id = r#"{"id": "[\"flow\",\"/work/repo\",\"demo\"]"}"#;
        assert!(
            lane_named(&snapshot, by_id).is_some(),
            "the full id is exact"
        );
    }

    #[test]
    fn a_proposal_is_bound_by_its_exact_wait_id_or_name_and_shows_the_engine_s_words() {
        let snapshot = with_two_proposals();
        let by_name = waiting(&snapshot, r#"{"proposal": "note-2"}"#).expect("bound");
        assert_eq!(
            (by_name.1.as_str(), by_name.2.as_str()),
            ("Second note", "two")
        );
        let id = r#"["wait","/work/repo","master","proposal","note-1"]"#;
        let by_id =
            waiting(&snapshot, &serde_json::json!({ "wait_id": id }).to_string()).expect("bound");
        assert_eq!(by_id.0, id);
        assert_eq!(by_id.1, "First note");
    }

    #[test]
    fn an_answer_that_mentions_two_proposals_binds_none() {
        let snapshot = with_two_proposals();
        assert!(
            waiting(&snapshot, "Proposed note-1 and note-2").is_none(),
            "prose binds none"
        );
        let both = r#"{"proposal": "note-1", "id": "note-2"}"#;
        assert!(waiting(&snapshot, both).is_none(), "two names bind none");
        assert!(
            waiting(&snapshot, r#"{"proposal": "note-"}"#).is_none(),
            "a part of a name binds none"
        );
        assert!(waiting(&snapshot, r#"{"proposal": "no-such-note"}"#).is_none());
    }

    #[test]
    fn a_wait_that_is_not_a_proposal_is_never_bound_as_one() {
        let snapshot = with_two_proposals();
        let gate = r#"{"id": "[\"wait\",\"/work/repo\",\"gated\",\"approval\",\"spec\"]"}"#;
        assert!(waiting(&snapshot, gate).is_none());
    }
}
