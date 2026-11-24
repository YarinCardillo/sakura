use super::{Drafts, STORAGE_VERSION};
use crate::tabs::Page;
use serde_json::json;

fn lane(key: &str) -> Page {
    Page::Lane(key.to_string())
}

#[test]
fn drafts_set_then_get_returns_the_text_of_that_page_only() {
    let mut drafts = Drafts::default();
    drafts.set(&Page::Master, "ask about the build");
    drafts.set(&lane("a"), "fix the title");
    assert_eq!(drafts.get(&Page::Master), Some("ask about the build"));
    assert_eq!(drafts.get(&lane("a")), Some("fix the title"));
    assert_eq!(drafts.get(&lane("b")), None);
}

#[test]
fn drafts_set_empty_or_blank_text_forgets_the_draft() {
    let mut drafts = Drafts::default();
    drafts.set(&Page::Master, "words");
    drafts.set(&Page::Master, "  \n ");
    assert_eq!(drafts.get(&Page::Master), None);
    assert!(drafts.is_empty());
}

#[test]
fn drafts_set_keeps_the_text_as_typed() {
    let mut drafts = Drafts::default();
    drafts.set(&Page::Master, "  indented\n```\ncode");
    assert_eq!(drafts.get(&Page::Master), Some("  indented\n```\ncode"));
}

#[test]
fn drafts_round_trip_through_json() {
    let mut drafts = Drafts::default();
    drafts.set(&Page::Master, "one");
    drafts.set(&lane("p/x"), "two");
    let restored = Drafts::restore(Some(&drafts.to_json()));
    assert_eq!(restored, drafts);
}

#[test]
fn drafts_restore_ignores_what_it_cannot_read() {
    assert!(Drafts::restore(None).is_empty());
    assert!(Drafts::restore(Some(&json!("nonsense"))).is_empty());
    let other_version = json!({ "version": STORAGE_VERSION + 1, "drafts": [] });
    assert!(Drafts::restore(Some(&other_version)).is_empty());
}

#[test]
fn drafts_restore_keeps_the_good_entries_and_drops_the_stale_ones() {
    let stored = json!({
        "version": STORAGE_VERSION,
        "drafts": [
            { "page": { "page": "master" }, "text": "kept" },
            { "page": { "page": "nowhere" }, "text": "unknown page" },
            { "page": { "page": "lane", "lane": "a" } },
            { "page": { "page": "lane", "lane": "b" }, "text": "" },
            { "page": { "page": "lane", "lane": "c" }, "text": "also kept" },
        ],
    });
    let drafts = Drafts::restore(Some(&stored));
    assert_eq!(drafts.get(&Page::Master), Some("kept"));
    assert_eq!(drafts.get(&lane("c")), Some("also kept"));
    assert_eq!(drafts.len(), 2);
}

#[test]
fn drafts_to_json_is_stable_whatever_the_order_of_writing() {
    let mut first = Drafts::default();
    first.set(&lane("b"), "2");
    first.set(&lane("a"), "1");
    let mut second = Drafts::default();
    second.set(&lane("a"), "1");
    second.set(&lane("b"), "2");
    assert_eq!(first.to_json(), second.to_json());
}

#[test]
fn drafts_retain_keeps_only_the_pages_that_are_wanted() {
    let mut drafts = Drafts::default();
    drafts.set(&Page::Master, "one");
    drafts.set(&lane("a"), "two");
    drafts.set(&lane("b"), "three");
    drafts.retain(|page| page != &lane("a"));
    assert_eq!(drafts.get(&lane("a")), None);
    assert_eq!(drafts.len(), 2);
}

#[test]
fn each_agent_of_a_lane_has_its_own_draft_apart_from_the_lane_s() {
    let mut drafts = Drafts::default();
    drafts.set_for(&lane("a"), Some("build-1"), "to the builder");
    drafts.set_for(&lane("a"), Some("review-1"), "to the reviewer");
    drafts.set(&lane("a"), "to the lane");
    assert_eq!(
        drafts.get_for(&lane("a"), Some("build-1")),
        Some("to the builder")
    );
    assert_eq!(
        drafts.get_for(&lane("a"), Some("review-1")),
        Some("to the reviewer")
    );
    assert_eq!(drafts.get(&lane("a")), Some("to the lane"));
    assert_eq!(drafts.get_for(&lane("a"), Some("check-1")), None);
    assert_eq!(drafts.len(), 3);
}

#[test]
fn a_blank_text_forgets_the_draft_of_that_agent_only() {
    let mut drafts = Drafts::default();
    drafts.set_for(&lane("a"), Some("build-1"), "one");
    drafts.set_for(&lane("a"), Some("review-1"), "two");
    drafts.set_for(&lane("a"), Some("build-1"), "  ");
    assert_eq!(drafts.get_for(&lane("a"), Some("build-1")), None);
    assert_eq!(drafts.get_for(&lane("a"), Some("review-1")), Some("two"));
}

#[test]
fn the_drafts_of_every_agent_survive_the_preferences_and_name_their_agent() {
    let mut drafts = Drafts::default();
    drafts.set_for(&lane("p/x"), Some("build-1"), "one");
    drafts.set_for(&lane("p/x"), Some("review-1"), "two");
    drafts.set(&Page::Master, "three");
    let restored = Drafts::restore(Some(&drafts.to_json()));
    assert_eq!(restored, drafts);
    assert_eq!(
        restored.get_for(&lane("p/x"), Some("review-1")),
        Some("two")
    );
    assert_eq!(
        restored.get(&lane("p/x")),
        None,
        "an agent's draft is not the lane's"
    );
}

#[test]
fn the_drafts_of_a_lane_the_engine_dropped_go_with_the_lane_whatever_their_agent() {
    let mut drafts = Drafts::default();
    drafts.set_for(&lane("gone"), Some("build-1"), "one");
    drafts.set_for(&lane("kept"), Some("build-1"), "two");
    drafts.retain(|page| *page != lane("gone"));
    assert_eq!(drafts.get_for(&lane("gone"), Some("build-1")), None);
    assert_eq!(drafts.get_for(&lane("kept"), Some("build-1")), Some("two"));
}

#[test]
fn the_agents_of_a_lane_that_have_a_draft_are_listed_with_their_text() {
    let mut drafts = Drafts::default();
    drafts.set_for(&lane("a"), Some("review-1"), "two");
    drafts.set_for(&lane("a"), Some("build-1"), "one");
    drafts.set_for(&lane("b"), Some("build-1"), "other lane");
    drafts.set(&lane("a"), "the lane");
    assert_eq!(
        drafts.agents_of(&lane("a")),
        [
            ("build-1".to_string(), "one".to_string()),
            ("review-1".to_string(), "two".to_string())
        ]
    );
}

#[test]
fn the_names_of_the_files_attached_and_not_sent_are_kept_with_the_drafts_and_read_back() {
    let mut drafts = Drafts::default();
    drafts.set_files(
        &Page::Master,
        &["notes.md".to_string(), "shot.png".to_string()],
    );
    let again = Drafts::restore(Some(&drafts.to_json()));
    assert_eq!(again.files(&Page::Master), ["notes.md", "shot.png"]);
    assert!(again.files(&lane("a")).is_empty());
    let mut cleared = again;
    cleared.set_files(&Page::Master, &[]);
    assert!(cleared.files(&Page::Master).is_empty());
    // Older storage has none.
    assert!(
        Drafts::restore(Some(&json!({"version": STORAGE_VERSION, "drafts": []})))
            .files(&Page::Master)
            .is_empty()
    );
}

#[test]
fn clearing_one_scoped_draft_cannot_restore_the_others_text() {
    let page = Page::Master;
    let mut drafts = Drafts::default();
    assert!(drafts.claim_legacy_owner(&page, "first"));
    drafts.set_for_owner(&page, None, "first", "original");
    drafts.set_for_owner(&page, None, "second", "independent");
    drafts.set_files_for_owner(&page, "second", &["only-second.txt".to_string()]);
    drafts.set_for_owner(&page, None, "second", "");
    let restored = Drafts::restore(Some(&drafts.to_json()));
    assert_eq!(
        restored.get_for_owner(&page, None, "first"),
        Some("original")
    );
    assert_eq!(restored.get_for_owner(&page, None, "second"), None);
    assert!(restored.files_for_owner(&page, "first").is_empty());
    assert_eq!(
        restored.files_for_owner(&page, "second"),
        ["only-second.txt"]
    );
}

#[test]
fn same_lane_split_keeps_agent_drafts_separate_through_close_and_reload() {
    let page = Page::Lane("lane".into());
    let mut drafts = Drafts::default();
    assert!(drafts.claim_legacy_owner(&page, "first"));
    let stored = drafts.to_json();
    let mut drafts = Drafts::restore(Some(&stored));
    assert!(!drafts.claim_legacy_owner(&page, "second"));
    drafts.set_for_owner(&page, Some("agent"), "second", "second text");
    let restored = Drafts::restore(Some(&drafts.to_json()));
    assert!(restored.agents_for_owner(&page, "first").is_empty());
    assert_eq!(
        restored.agents_for_owner(&page, "second"),
        [("agent".into(), "second text".into())]
    );
}
