use super::*;
use serde_json::json;

fn lane(key: &str) -> Page {
    Page::Lane(key.to_string())
}

fn pages(tabs: &Tabs) -> Vec<Page> {
    tabs.tabs().iter().map(|tab| tab.page().clone()).collect()
}

fn shown(tabs: &Tabs) -> Page {
    tabs.active().page().clone()
}

fn id_of(tabs: &Tabs, page: &Page) -> TabId {
    match tabs.tabs().iter().find(|tab| tab.page() == page) {
        Some(tab) => tab.id(),
        None => panic!("no tab on {page:?} in {:?}", pages(tabs)),
    }
}

/// An Inbox tab, then one tab per page of `rest`, the Inbox shown.
fn strip_of(rest: &[Page]) -> Tabs {
    let mut tabs = Tabs::new();
    for page in rest {
        tabs.open_new(page.clone());
    }
    let first = tabs.tabs()[0].id();
    tabs.activate(first);
    tabs
}

#[test]
fn tabs_new_is_one_inbox_tab_shown_with_no_history() {
    let tabs = Tabs::new();
    assert_eq!(pages(&tabs), [Page::Inbox]);
    assert_eq!(shown(&tabs), Page::Inbox);
    assert!(!tabs.is_empty());
    assert!(!tabs.active().can_go_back());
    assert!(!tabs.active().can_go_forward());
}

#[test]
fn navigate_then_back_returns_to_the_previous_page_and_forward_comes_back() {
    let mut tabs = Tabs::new();
    tabs.navigate(Page::Master);
    tabs.navigate(lane("demo"));
    assert_eq!(shown(&tabs), lane("demo"));
    assert!(tabs.back());
    assert_eq!(shown(&tabs), Page::Master);
    assert!(tabs.back());
    assert_eq!(shown(&tabs), Page::Inbox);
    assert!(!tabs.back());
    assert!(tabs.forward());
    assert_eq!(shown(&tabs), Page::Master);
    assert!(tabs.forward());
    assert_eq!(shown(&tabs), lane("demo"));
    assert!(!tabs.forward());
}

#[test]
fn navigate_after_back_clears_the_forward_history() {
    let mut tabs = Tabs::new();
    tabs.navigate(Page::Master);
    tabs.back();
    assert!(tabs.active().can_go_forward());
    tabs.navigate(Page::Lanes);
    assert!(!tabs.active().can_go_forward());
    assert!(!tabs.forward());
    assert_eq!(shown(&tabs), Page::Lanes);
}

#[test]
fn navigate_to_the_page_already_shown_changes_nothing() {
    let mut tabs = Tabs::new();
    tabs.navigate(Page::Inbox);
    assert!(!tabs.active().can_go_back());
    assert_eq!(pages(&tabs), [Page::Inbox]);
}

#[test]
fn navigate_to_a_page_open_in_another_tab_shows_that_tab_without_a_new_one() {
    let mut tabs = strip_of(&[Page::Master]);
    let master = id_of(&tabs, &Page::Master);
    tabs.navigate(Page::Master);
    assert_eq!(tabs.active().id(), master);
    assert_eq!(tabs.len(), 2);
    assert!(!tabs.tabs()[0].can_go_back(), "the tab left keeps its page");
    assert_eq!(pages(&tabs), [Page::Inbox, Page::Master]);
}

#[test]
fn history_belongs_to_each_tab() {
    let mut tabs = Tabs::new();
    tabs.navigate(Page::Lanes);
    let first = tabs.active().id();
    let second = tabs.open_new(Page::Engine);
    assert_eq!(tabs.active().id(), second);
    assert!(!tabs.active().can_go_back());
    assert!(!tabs.back());
    tabs.activate(first);
    assert!(tabs.active().can_go_back());
    assert!(tabs.back());
    assert_eq!(shown(&tabs), Page::Inbox);
    assert_eq!(pages(&tabs), [Page::Inbox, Page::Engine]);
}

#[test]
fn open_new_adds_a_tab_at_the_end_and_shows_it() {
    let mut tabs = Tabs::new();
    let opened = tabs.open_new(lane("demo"));
    assert_eq!(pages(&tabs), [Page::Inbox, lane("demo")]);
    assert_eq!(tabs.active().id(), opened);
    assert_eq!(tabs.index_of(opened), Some(1));
}

#[test]
fn open_new_on_a_page_already_open_still_opens_a_tab() {
    let mut tabs = Tabs::new();
    let opened = tabs.open_new(Page::Inbox);
    assert_eq!(pages(&tabs), [Page::Inbox, Page::Inbox]);
    assert_eq!(tabs.active().id(), opened);
}

#[test]
fn activate_shows_an_open_tab_and_refuses_a_shown_or_unknown_one() {
    let mut tabs = strip_of(&[Page::Master]);
    let master = id_of(&tabs, &Page::Master);
    assert!(tabs.activate(master));
    assert_eq!(shown(&tabs), Page::Master);
    assert!(!tabs.activate(master), "already shown");
    let gone = tabs.open_new(Page::Lanes);
    assert!(tabs.close(gone));
    assert!(!tabs.activate(gone));
}

#[test]
fn close_on_the_only_tab_returns_false() {
    let mut tabs = Tabs::new();
    let only = tabs.active().id();
    assert!(!tabs.close(only));
    assert_eq!(tabs.len(), 1);
}

#[test]
fn close_of_an_unknown_tab_returns_false() {
    let mut tabs = strip_of(&[Page::Master]);
    let master = id_of(&tabs, &Page::Master);
    assert!(tabs.close(master));
    assert!(!tabs.close(master));
}

#[test]
fn close_of_the_shown_tab_hands_over_to_its_right_neighbour_else_the_left() {
    let mut tabs = strip_of(&[Page::Master, Page::Lanes]);
    let master = id_of(&tabs, &Page::Master);
    tabs.activate(master);
    assert!(tabs.close(master));
    assert_eq!(shown(&tabs), Page::Lanes);
    let lanes = id_of(&tabs, &Page::Lanes);
    assert!(tabs.close(lanes));
    assert_eq!(shown(&tabs), Page::Inbox);
}

#[test]
fn close_of_another_tab_keeps_the_one_shown() {
    let mut tabs = strip_of(&[Page::Master, Page::Lanes]);
    let master = id_of(&tabs, &Page::Master);
    assert!(tabs.close(master));
    assert_eq!(shown(&tabs), Page::Inbox);
    assert_eq!(pages(&tabs), [Page::Inbox, Page::Lanes]);
}

#[test]
fn activate_digit_shows_the_tab_at_that_place() {
    let mut tabs = strip_of(&[Page::Master, Page::Lanes]);
    assert!(tabs.activate_digit(2));
    assert_eq!(shown(&tabs), Page::Master);
    assert!(tabs.activate_digit(3));
    assert_eq!(shown(&tabs), Page::Lanes);
    assert!(tabs.activate_digit(1));
    assert_eq!(shown(&tabs), Page::Inbox);
}

#[test]
fn activate_digit_nine_shows_the_last_tab_whatever_its_place() {
    let mut tabs = strip_of(&[Page::Master, Page::Lanes, Page::Engine]);
    assert!(tabs.activate_digit(9));
    assert_eq!(shown(&tabs), Page::Engine);
    let many: Vec<Page> = (0..12).map(|n| lane(&format!("lane-{n}"))).collect();
    let mut tabs = strip_of(&many);
    assert!(tabs.activate_digit(9));
    assert_eq!(shown(&tabs), lane("lane-11"));
}

#[test]
fn activate_digit_with_no_tab_there_changes_nothing() {
    let mut tabs = strip_of(&[Page::Master]);
    for digit in [0, 3, 8, 10] {
        assert!(!tabs.activate_digit(digit), "Cmd+{digit}");
    }
    assert_eq!(shown(&tabs), Page::Inbox);
}

#[test]
fn to_json_then_restore_keeps_the_pages_and_the_one_shown() {
    let mut tabs = strip_of(&[
        Page::Master,
        Page::Lanes,
        Page::Engine,
        lane("[\"a\",\"b\"]"),
    ]);
    tabs.activate_digit(3);
    let stored = tabs.to_json();
    assert_eq!(stored["version"], json!(3));
    let restored = Tabs::restore(Some(&stored));
    assert_eq!(pages(&restored), pages(&tabs));
    assert_eq!(shown(&restored), Page::Lanes);
}

#[test]
fn restore_starts_every_tab_with_no_history() {
    let mut tabs = Tabs::new();
    tabs.navigate(Page::Master);
    let restored = Tabs::restore(Some(&tabs.to_json()));
    assert_eq!(shown(&restored), Page::Master);
    assert!(!restored.active().can_go_back());
}

#[test]
fn restore_of_a_stored_v2_strip_keeps_the_places_and_the_one_shown() {
    let stored = json!({
        "version": 2,
        "tabs": [
            { "kind": "waiting" },
            { "kind": "master" },
            { "kind": "lane", "lane": "[\"proj\",\"one\"]" },
            { "kind": "lane", "lane": "[\"proj\",\"two\"]" },
        ],
        "active": 3,
    });
    let restored = Tabs::restore(Some(&stored));
    assert_eq!(
        pages(&restored),
        [
            Page::Inbox,
            Page::Master,
            lane("[\"proj\",\"one\"]"),
            lane("[\"proj\",\"two\"]")
        ]
    );
    assert_eq!(shown(&restored), lane("[\"proj\",\"two\"]"));
}

#[test]
fn restore_of_a_stored_v2_strip_with_the_master_first_keeps_that_order() {
    let stored = json!({
        "version": 2,
        "tabs": [{ "kind": "master" }, { "kind": "waiting" }],
        "active": 1,
    });
    let restored = Tabs::restore(Some(&stored));
    assert_eq!(pages(&restored), [Page::Master, Page::Inbox]);
    assert_eq!(shown(&restored), Page::Inbox);
}

#[test]
fn restore_of_a_stored_v2_strip_with_a_place_past_its_end_shows_the_first() {
    let stored = json!({
        "version": 2,
        "tabs": [{ "kind": "waiting" }, { "kind": "master" }],
        "active": 9,
    });
    assert_eq!(shown(&Tabs::restore(Some(&stored))), Page::Inbox);
}

#[test]
fn restore_of_a_stored_v1_strip_of_tasks_gives_one_inbox_tab() {
    let stored = json!({
        "version": 1,
        "tabs": [{ "kind": "task", "task": "m1" }, { "kind": "task", "task": "m2" }],
        "active": 1,
    });
    let restored = Tabs::restore(Some(&stored));
    assert_eq!(pages(&restored), [Page::Inbox]);
}

#[test]
fn restore_of_an_unknown_version_gives_one_inbox_tab() {
    let stored = json!({ "version": 99, "tabs": [{ "page": "master" }], "active": 0 });
    assert_eq!(pages(&Tabs::restore(Some(&stored))), [Page::Inbox]);
}

#[test]
fn restore_of_nothing_or_of_garbage_gives_one_inbox_tab() {
    assert_eq!(pages(&Tabs::restore(None)), [Page::Inbox]);
    for garbage in [
        json!(null),
        json!("tabs"),
        json!({ "version": 3 }),
        json!({ "version": 3, "tabs": [] }),
        json!({ "version": 3, "tabs": [{ "page": "nowhere" }] }),
        json!({ "version": 3, "tabs": [{ "page": "lane" }] }),
        json!({ "version": 2, "tabs": [{ "kind": "lane", "lane": "" }] }),
    ] {
        let restored = Tabs::restore(Some(&garbage));
        assert_eq!(pages(&restored), [Page::Inbox], "{garbage}");
    }
}

#[test]
fn restore_drops_a_page_stored_twice_and_keeps_the_first_tab() {
    let stored = json!({
        "version": 3,
        "tabs": [{ "page": "master" }, { "page": "master" }, { "page": "engine" }],
        "active": 2,
    });
    let restored = Tabs::restore(Some(&stored));
    assert_eq!(pages(&restored), [Page::Master, Page::Engine]);
    assert_eq!(shown(&restored), Page::Engine);
}

#[test]
fn tab_ids_stay_unique_after_a_restore_and_a_new_tab() {
    let mut restored = Tabs::restore(Some(&strip_of(&[Page::Master]).to_json()));
    restored.open_new(Page::Lanes);
    let mut ids: Vec<u64> = restored
        .tabs()
        .iter()
        .map(|tab| tab.id().as_u64())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 3);
}

#[test]
fn move_to_puts_any_tab_at_any_place_the_first_included() {
    let mut tabs = strip_of(&[lane("m1"), lane("m2")]);
    assert!(tabs.move_to(id_of(&tabs, &lane("m2")), 0));
    assert_eq!(pages(&tabs), [lane("m2"), Page::Inbox, lane("m1")]);
    let inbox = id_of(&tabs, &Page::Inbox);
    assert!(tabs.move_to(inbox, 2));
    assert_eq!(pages(&tabs), [lane("m2"), lane("m1"), Page::Inbox]);
}

#[test]
fn move_to_past_the_end_stops_at_the_last_place() {
    let mut tabs = strip_of(&[lane("m1"), lane("m2")]);
    let first = tabs.tabs()[0].id();
    assert!(tabs.move_to(first, 99));
    assert_eq!(pages(&tabs), [lane("m1"), lane("m2"), Page::Inbox]);
}

#[test]
fn move_to_keeps_the_tab_shown_and_its_history() {
    let mut tabs = strip_of(&[lane("m1"), lane("m2")]);
    tabs.navigate(Page::Master);
    let shown_id = tabs.active().id();
    tabs.move_to(id_of(&tabs, &lane("m2")), 0);
    assert_eq!(tabs.active().id(), shown_id);
    assert!(tabs.active().can_go_back());
}

#[test]
fn move_to_its_own_place_or_of_an_unknown_tab_changes_nothing() {
    let mut tabs = strip_of(&[lane("m1")]);
    let own = id_of(&tabs, &lane("m1"));
    assert!(!tabs.move_to(own, 1));
    assert!(!tabs.move_to(TabId(999), 0));
}

fn talks(tabs: &Tabs) -> Vec<String> {
    tabs.tabs()
        .iter()
        .map(|tab| tab.conversation().to_string())
        .collect()
}

#[test]
fn every_tab_has_a_conversation_of_its_own_that_no_other_tab_ever_had() {
    let mut tabs = Tabs::new();
    let first = tabs.open_new(Page::Master);
    let second = tabs.open_new(Page::Master);
    let all = talks(&tabs);
    assert_eq!(all.len(), 3);
    assert!(all.iter().all(|talk| !talk.is_empty()));
    let mut unique = all.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 3, "{all:?}");
    // A tab closed and a new one opened never takes up the closed one's conversation.
    let closed = all[1].clone();
    tabs.close(first);
    tabs.open_new(Page::Master);
    assert!(!talks(&tabs).contains(&closed));
    let _ = second;
}

#[test]
fn three_master_tabs_come_back_as_three_tabs_each_on_its_own_conversation_and_the_one_shown() {
    let mut tabs = Tabs::new();
    tabs.open_new(Page::Master);
    tabs.open_new(Page::Master);
    let third = tabs.open_new(Page::Master);
    assert_eq!(tabs.active().id(), third);
    let stored = tabs.to_json();
    let restored = Tabs::restore(Some(&stored));
    assert_eq!(pages(&restored), pages(&tabs));
    assert_eq!(talks(&restored), talks(&tabs));
    assert_eq!(
        restored.active().conversation(),
        tabs.active().conversation()
    );
    // A tab opened after the restore is a new conversation, never one of the restored ones.
    let mut restored = restored;
    restored.open_new(Page::Master);
    let all = talks(&restored);
    assert_eq!(all.iter().filter(|talk| **talk == all[4]).count(), 1);
}

#[test]
fn tabs_stored_before_conversations_were_kept_give_the_first_master_tab_the_old_conversation() {
    let stored = json!({"version": 3, "active": 0, "tabs": [
        {"page": "inbox"}, {"page": "master"}, {"page": "master"}]});
    let tabs = Tabs::restore(Some(&stored));
    assert_eq!(pages(&tabs), [Page::Inbox, Page::Master]);
    assert_eq!(
        talks(&tabs)[1],
        "",
        "the first conversation keeps the key it was kept under"
    );
}

#[test]
fn split_keeps_existing_tabs_and_their_histories() {
    let mut tabs = Tabs::new();
    let left = tabs.active().id();
    tabs.navigate(Page::Lanes);
    let right = tabs.open_new(Page::Master);
    tabs.navigate(Page::Engine);
    tabs.back();
    let before = [
        tabs.tab(left).unwrap().clone(),
        tabs.tab(right).unwrap().clone(),
    ];

    assert!(tabs.create_split(left, right));
    assert!(tabs.swap_split(left));
    assert_eq!(tabs.tabs(), [before[1].clone(), before[0].clone()]);
    assert_ne!(before[0].conversation(), before[1].conversation());
    assert_eq!(tabs.active().id(), right);
    assert_eq!(tabs.visible(), (right, Some(left)));
}

#[test]
fn split_members_are_adjacent_at_the_previously_active_tabs_slot() {
    for peer_on_left in [false, true] {
        let mut tabs = strip_of(&[Page::Master, Page::Lanes, Page::Engine]);
        let [first, peer, middle, active] = ids(&tabs)[..] else {
            unreachable!()
        };
        tabs.activate(active);
        let (left, right) = if peer_on_left {
            (peer, active)
        } else {
            (active, peer)
        };
        assert!(tabs.create_split(left, right));
        assert_eq!(ids(&tabs), [first, middle, left, right]);
    }
}

#[test]
fn duplicate_or_closed_members_and_second_pair_are_refused() {
    let mut tabs = strip_of(&[Page::Master, Page::Lanes]);
    let left = tabs.active().id();
    let right = tabs.tabs()[1].id();
    let other = tabs.tabs()[2].id();
    let closed = tabs.open_new(Page::Engine);
    tabs.close(closed);
    tabs.activate(left);
    assert!(!tabs.create_split(left, left));
    assert!(!tabs.create_split(left, closed));
    assert!(!tabs.create_split(right, other));
    assert!(tabs.create_split(left, right));
    tabs.activate(other);
    assert!(!tabs.create_split(other, right));
}

#[test]
fn pair_reopens_from_either_member_after_switching_to_an_unrelated_tab() {
    let (mut tabs, left, right) = split_strip();
    let other = tabs.tabs()[2].id();
    for digit in [1, 2] {
        assert!(tabs.activate(other));
        assert_eq!(tabs.visible(), (other, None));
        assert!(tabs.activate_digit(digit));
        assert_eq!(tabs.visible(), (left, Some(right)));
    }
}

#[test]
fn close_visible_member_selects_survivor_and_close_background_member_preserves_selection() {
    let (mut tabs, left, right) = split_strip();
    let other = tabs.tabs()[2].id();
    let mut background = tabs.clone();
    background.activate(other);
    assert!(background.close(left));
    assert_eq!(background.visible(), (other, None));
    assert!(tabs.close(right));
    assert_eq!(tabs.visible(), (left, None));
    assert_eq!(tabs.split(), None);
}

#[test]
fn standalone_reorder_cannot_insert_between_pair_members() {
    let (mut tabs, left, right) = split_strip();
    let other = tabs.tabs()[2].id();
    assert!(!tabs.move_to(left, 2));
    assert!(tabs.move_to(other, 1));
    assert_eq!(ids(&tabs), [other, left, right]);
    assert!(tabs.move_to(other, 1));
    assert_eq!(ids(&tabs), [left, right, other]);
}

#[test]
fn split_navigation_keeps_duplicate_pages_and_history_local() {
    let (mut tabs, left, right) = split_strip();
    let other = tabs.tabs()[2].id();
    assert!(tabs.navigate_from(right, Page::Inbox));
    assert_eq!(tabs.active().id(), left);
    assert_eq!(pages(&tabs), [Page::Inbox, Page::Inbox, Page::Lanes]);
    assert!(tabs.navigate_from(right, Page::Lanes));
    assert!(!tabs.navigate_from(other, Page::Master));
    tabs.activate(right);
    assert!(tabs.back());
    assert_eq!(shown(&tabs), Page::Inbox);
    assert!(tabs.back());
    assert_eq!(shown(&tabs), Page::Master);
    assert!(!tabs.tab(left).unwrap().can_go_back());
    tabs.close(right);
    assert!(!tabs.navigate_from(right, Page::Engine));
}

#[test]
fn stale_resize_does_not_change_the_swapped_or_closed_pair() {
    let (mut tabs, left, right) = split_strip();
    assert!(tabs.set_split_share(left, right, 6500));
    assert_eq!(tabs.split().unwrap().left_share_bps, 6000);
    assert!(tabs.swap_split(right));
    assert!(!tabs.set_split_share(left, right, 4000));
    assert_eq!(tabs.split().unwrap().left_share_bps, 6000);
    assert!(tabs.close(left));
    assert!(!tabs.set_split_share(right, left, 4500));
}

fn ids(tabs: &Tabs) -> Vec<TabId> {
    tabs.tabs().iter().map(Tab::id).collect()
}

fn split_strip() -> (Tabs, TabId, TabId) {
    let mut tabs = strip_of(&[Page::Master, Page::Lanes]);
    let left = tabs.active().id();
    let right = tabs.tabs()[1].id();
    assert!(tabs.create_split(left, right));
    (tabs, left, right)
}

#[test]
fn asymmetric_split_round_trips_both_histories_focus_and_conversations() {
    let (mut tabs, left, right) = split_strip();
    tabs.navigate_from(left, Page::Engine);
    tabs.navigate_from(right, Page::Inbox);
    tabs.activate(right);
    tabs.back();
    tabs.set_split_share(left, right, 5700);
    let stored = tabs.to_json();
    assert_eq!(stored["version"], 4);
    let restored = Tabs::restore(Some(&stored));
    assert_eq!(restored, tabs);
}

#[test]
fn background_pair_round_trips_without_stealing_selection() {
    let (mut tabs, _, _) = split_strip();
    tabs.activate_digit(3);
    assert_eq!(Tabs::restore(Some(&tabs.to_json())), tabs);
}

fn split_storage() -> Value {
    json!({
        "version": 4, "active": 2,
        "tabs": [{"page": "inbox"}, {"page": "master"}, {"page": "engine"}],
        "split": {"left": 1, "right": 2, "left_share_bps": 5500}
    })
}

#[test]
fn split_indices_use_original_positions_before_deduplication() {
    let mut stored = split_storage();
    stored["tabs"]
        .as_array_mut()
        .unwrap()
        .insert(1, json!({"page": "inbox"}));
    stored["split"]["left"] = json!(2);
    stored["split"]["right"] = json!(3);
    stored["active"] = json!(3);
    let tabs = Tabs::restore(Some(&stored));
    assert_eq!(shown(&tabs), Page::Engine);
    assert_eq!(
        tabs.visible(),
        (tabs.tabs()[1].id(), Some(tabs.active().id()))
    );
    stored["split"]["left"] = json!(1);
    assert_eq!(Tabs::restore(Some(&stored)).split(), None);
}

#[test]
fn bad_split_metadata_keeps_the_restored_strip() {
    for (left, right) in [
        (None, Some(1)),
        (Some(1), Some(1)),
        (Some(1), Some(3)),
        (Some(2), Some(1)),
        (Some(0), Some(2)),
    ] {
        let mut stored = split_storage();
        stored["split"] = json!({"left": left, "right": right});
        let tabs = Tabs::restore(Some(&stored));
        assert_eq!(pages(&tabs), [Page::Inbox, Page::Master, Page::Engine]);
        assert_eq!(shown(&tabs), Page::Engine);
        assert_eq!(tabs.split(), None);
    }
}

#[test]
fn bad_active_index_falls_back_without_retargeting_the_pair() {
    let mut stored = split_storage();
    stored["active"] = json!(99);
    let tabs = Tabs::restore(Some(&stored));
    assert_eq!(shown(&tabs), Page::Inbox);
    assert_eq!(tabs.visible(), (tabs.active().id(), None));
    assert_eq!(tabs.split().unwrap().right, tabs.tabs()[2].id());
}

#[test]
fn bad_share_uses_default_without_dropping_pair() {
    for (value, expected) in [
        (json!(null), 5000),
        (json!("5700"), 5000),
        (json!(4.5), 5000),
        (json!(u64::MAX), 6000),
        (json!(-1), 4000),
    ] {
        let mut stored = split_storage();
        stored["split"]["left_share_bps"] = value;
        let tabs = Tabs::restore(Some(&stored));
        assert_eq!(tabs.split().unwrap().left_share_bps, expected);
    }
}

#[test]
fn bad_history_does_not_drop_current_page() {
    let mut stored = split_storage();
    stored["tabs"][2]["back"] = json!([{"page": "unknown"}, {"page": "lanes"}]);
    stored["tabs"][2]["forward"] = json!("damaged");
    let mut tabs = Tabs::restore(Some(&stored));
    assert_eq!(shown(&tabs), Page::Engine);
    assert!(!tabs.forward());
    assert!(tabs.back());
    assert_eq!(shown(&tabs), Page::Lanes);
    assert!(!tabs.back());
}

#[test]
fn close_then_relaunch_cannot_revive_the_pair() {
    let mut tabs = Tabs::restore(Some(&split_storage()));
    assert!(tabs.close(tabs.active().id()));
    let stored = tabs.to_json();
    assert_eq!(stored["version"], 3);
    let restored = Tabs::restore(Some(&stored));
    assert_eq!(restored.split(), None);
    assert_eq!(shown(&restored), Page::Master);
}

#[test]
fn a_kept_track_tab_round_trips_by_id() {
    let mut stored = Tabs::new().to_json();
    stored["tabs"][0]["page"] = "track".into();
    stored["tabs"][0]["track"] = "t-01234567".into();
    let restored = Tabs::restore(Some(&stored));
    assert_eq!(restored.to_json()["tabs"][0]["page"], "track");
    assert_eq!(restored.to_json()["tabs"][0]["track"], "t-01234567");
}
