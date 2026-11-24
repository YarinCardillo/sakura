//! The engine's own `flow feed` output with every kind of entry (made by its own code in a throwaway home, see
//! `fixtures/README.md`): the app must read all of it. A shape the app misreads fails here.

use sakura_core::engine::feed::FeedPage;

const REAL_FEED: &str = include_str!("fixtures/engine-real-feed-kinds.json");

fn page() -> FeedPage {
    serde_json::from_str(REAL_FEED).expect("the app reads the engine's feed with every kind")
}

#[test]
fn a_real_feed_with_every_kind_of_entry_parses() {
    let page = page();
    let kinds: Vec<&str> = page.entries.iter().map(|e| e.kind.as_str()).collect();
    for kind in ["text", "tool", "result", "notice", "board"] {
        assert!(kinds.contains(&kind), "no {kind} entry in the real feed");
    }
}

#[test]
fn a_notice_has_no_run_and_no_role_and_a_board_post_names_its_lane_and_role() {
    let page = page();
    let notice = page.entries.iter().find(|e| e.kind == "notice").unwrap();
    assert_eq!((notice.run.as_str(), notice.role.as_str()), ("", ""));
    assert_eq!(notice.tag, "notify");
    assert!(notice.at.is_some());
    let post = page.entries.iter().rfind(|e| e.kind == "board").unwrap();
    assert_eq!(post.from_title, "Fix the signup");
    assert_eq!(post.role, "builder");
}

#[test]
fn a_real_result_entry_carries_its_verdict_and_summary() {
    let page = page();
    let result = page.entries.iter().find(|e| e.kind == "result").unwrap();
    let found = result.result.as_ref().unwrap();
    assert_eq!(
        (found.verdict.as_str(), found.summary.as_str()),
        ("REVISE", "missing test; bad name")
    );
}
