use super::*;
use serde_json::json;

/// The lavoro "m0", then one tab per name, the first one shown.
fn strip_of(names: &[&str]) -> Tabs {
    let mut tabs = Tabs::new();
    tabs.open("m0", Placement::Background);
    for name in names {
        tabs.open(name, Placement::Background);
    }
    tabs
}

fn targets(tabs: &Tabs) -> Vec<String> {
    tabs.tabs().iter().map(|tab| tab.task.clone()).collect()
}

/// The lavoro shown.
fn shown(tabs: &Tabs) -> Option<&str> {
    tabs.active().map(|tab| tab.task.as_str())
}

fn id_of(tabs: &Tabs, name: &str) -> TabId {
    match tabs.tabs().iter().find(|tab| tab.task == name) {
        Some(tab) => tab.id,
        None => panic!("no tab on {name} in {:?}", targets(tabs)),
    }
}

#[test]
fn tabs_new_has_no_tab_and_shows_nothing() {
    let tabs = Tabs::new();
    assert!(tabs.is_empty());
    assert_eq!(tabs.len(), 0);
    assert_eq!(tabs.active_id(), None);
    assert_eq!(tabs.active_index(), None);
    assert_eq!(tabs.active(), None);
}

#[test]
fn tabs_with_no_tab_have_nothing_to_select_close_or_move() {
    let mut tabs = Tabs::new();
    for digit in 0..=10 {
        assert_eq!(tabs.select_position(digit), None, "Cmd+{digit}");
    }
    let mut once = strip_of(&[]);
    let gone = id_of(&once, "m0");
    assert!(once.retain_tasks(|_| false));
    assert!(!tabs.close(gone));
    assert!(!tabs.close_others(gone));
    assert!(!tabs.activate(gone));
    assert!(!tabs.move_to(gone, 0));
    assert!(!tabs.retain_tasks(|_| true));
    assert_eq!(tabs, Tabs::new());
}

#[test]
fn tabs_show_with_no_tab_opens_the_first_and_shows_it() {
    let mut tabs = Tabs::new();
    let opened = tabs.show("m1");
    assert_eq!(targets(&tabs), ["m1"]);
    assert_eq!(tabs.active_id(), Some(opened));
}

#[test]
fn tabs_open_in_the_background_with_no_tab_still_shows_the_first() {
    let mut tabs = Tabs::new();
    let opened = tabs.open("m1", Placement::Background);
    assert_eq!(
        tabs.active_id(),
        Some(opened),
        "a strip with a tab shows one"
    );
}

#[test]
fn tabs_show_replaces_the_page_of_the_current_tab() {
    let mut tabs = strip_of(&[]);
    let before = tabs.active_id();
    let shown = tabs.show("m1");
    assert_eq!(Some(shown), before, "the same tab now shows the task");
    assert_eq!(targets(&tabs), ["m1"]);
}

#[test]
fn tabs_show_a_task_already_open_selects_its_tab_instead_of_a_copy() {
    let mut tabs = strip_of(&["m1", "m2"]);
    let shown = tabs.show("m2");
    assert_eq!(targets(&tabs), ["m0", "m1", "m2"]);
    assert_eq!(shown, id_of(&tabs, "m2"));
    assert_eq!(tabs.active_index(), Some(2));
}

#[test]
fn tabs_open_in_the_background_adds_a_tab_at_the_end_and_keeps_the_page() {
    let mut tabs = strip_of(&[]);
    let opened = tabs.open("m1", Placement::Background);
    assert_eq!(targets(&tabs), ["m0", "m1"]);
    assert_eq!(tabs.active_index(), Some(0));
    assert_ne!(Some(opened), tabs.active_id());
}

#[test]
fn tabs_open_in_the_foreground_shows_the_new_tab() {
    let mut tabs = strip_of(&[]);
    let opened = tabs.open("m1", Placement::Foreground);
    assert_eq!(tabs.active_id(), Some(opened));
    assert_eq!(tabs.active_index(), Some(1));
}

#[test]
fn tabs_open_a_task_already_open_reuses_its_tab() {
    let mut tabs = strip_of(&["m1"]);
    let again = tabs.open("m1", Placement::Foreground);
    assert_eq!(targets(&tabs), ["m0", "m1"]);
    assert_eq!(again, id_of(&tabs, "m1"));
    assert_eq!(tabs.active_id(), Some(again));
}

#[test]
fn tabs_activate_an_unknown_tab_changes_nothing() {
    let mut tabs = strip_of(&["m1"]);
    let gone = id_of(&tabs, "m1");
    assert!(tabs.close(gone));
    assert!(!tabs.activate(gone));
    assert_eq!(tabs.active_index(), Some(0));
}

#[test]
fn tabs_close_the_last_tab_left_is_refused() {
    let mut tabs = strip_of(&[]);
    let only = id_of(&tabs, "m0");
    assert!(!tabs.close(only));
    assert_eq!(tabs.len(), 1);
}

#[test]
fn tabs_close_any_tab_while_another_remains_the_first_included() {
    let mut tabs = strip_of(&["m1", "m2"]);
    let first = tabs.tabs()[0].id;
    assert!(tabs.close(first));
    assert_eq!(targets(&tabs), ["m1", "m2"]);
}

#[test]
fn tabs_close_the_active_tab_hands_over_to_its_right_neighbour() {
    let mut tabs = strip_of(&["m1", "m2"]);
    tabs.activate(id_of(&tabs, "m1"));
    tabs.close(id_of(&tabs, "m1"));
    assert_eq!(shown(&tabs), Some("m2"));
}

#[test]
fn tabs_close_the_active_last_tab_hands_over_to_its_left_neighbour() {
    let mut tabs = strip_of(&["m1", "m2"]);
    tabs.activate(id_of(&tabs, "m2"));
    tabs.close(id_of(&tabs, "m2"));
    assert_eq!(shown(&tabs), Some("m1"));
}

#[test]
fn tabs_close_an_inactive_tab_keeps_the_page_shown() {
    let mut tabs = strip_of(&["m1", "m2"]);
    tabs.activate(id_of(&tabs, "m2"));
    tabs.close(id_of(&tabs, "m1"));
    assert_eq!(shown(&tabs), Some("m2"));
    assert_eq!(tabs.active_index(), Some(1));
}

#[test]
fn tabs_close_others_keeps_the_tab_asked_for_and_shows_it() {
    let mut tabs = strip_of(&["m1", "m2", "m3"]);
    let kept = id_of(&tabs, "m2");
    assert!(tabs.close_others(kept));
    assert_eq!(targets(&tabs), ["m2"]);
    assert_eq!(tabs.active_id(), Some(kept));
}

#[test]
fn tabs_close_others_with_one_tab_changes_nothing() {
    let mut tabs = strip_of(&[]);
    let only = id_of(&tabs, "m0");
    assert!(!tabs.close_others(only));
}

#[test]
fn tabs_move_puts_any_tab_at_any_place_the_first_included() {
    let mut tabs = strip_of(&["m1", "m2"]);
    assert!(tabs.move_to(id_of(&tabs, "m2"), 0));
    assert_eq!(targets(&tabs), ["m2", "m0", "m1"]);
    let first_opened = tabs.tabs()[1].id;
    assert!(tabs.move_to(first_opened, 2));
    assert_eq!(targets(&tabs), ["m2", "m1", "m0"]);
}

#[test]
fn tabs_move_past_the_end_stops_at_the_last_place() {
    let mut tabs = strip_of(&["m1", "m2"]);
    let first = tabs.tabs()[0].id;
    assert!(tabs.move_to(first, 99));
    assert_eq!(targets(&tabs), ["m1", "m2", "m0"]);
}

#[test]
fn tabs_move_keeps_the_page_shown() {
    let mut tabs = strip_of(&["m1", "m2"]);
    let active = tabs.active_id();
    tabs.move_to(id_of(&tabs, "m2"), 0);
    assert_eq!(tabs.active_id(), active);
    assert_eq!(tabs.active_index(), Some(1));
}

#[test]
fn tabs_move_to_its_own_place_changes_nothing() {
    let mut tabs = strip_of(&["m1"]);
    assert!(!tabs.move_to(id_of(&tabs, "m1"), 1));
}

#[test]
fn tabs_select_position_one_to_eight_selects_by_place() {
    let mut tabs = strip_of(&["m1", "m2"]);
    assert_eq!(tabs.select_position(3), Some(id_of(&tabs, "m2")));
    assert_eq!(tabs.active_index(), Some(2));
    assert_eq!(tabs.select_position(1), Some(tabs.tabs()[0].id));
    assert_eq!(tabs.active_index(), Some(0));
}

#[test]
fn tabs_select_position_nine_selects_the_last_tab() {
    let mut tabs = strip_of(&["m1", "m2"]);
    assert_eq!(tabs.select_position(9), Some(id_of(&tabs, "m2")));
    assert_eq!(tabs.active_index(), Some(2));
}

#[test]
fn tabs_select_position_with_no_tab_there_changes_nothing() {
    let mut tabs = strip_of(&["m1"]);
    assert_eq!(tabs.select_position(5), None);
    assert_eq!(tabs.select_position(0), None);
    assert_eq!(tabs.active_index(), Some(0));
}

#[test]
fn shortcut_digit_names_the_first_eight_places_and_nine_for_the_last() {
    assert_eq!(shortcut_digit(0, 3), Some(1));
    assert_eq!(shortcut_digit(2, 3), Some(3));
    assert_eq!(shortcut_digit(7, 12), Some(8));
    assert_eq!(
        shortcut_digit(8, 12),
        None,
        "the ninth of twelve has no key"
    );
    assert_eq!(shortcut_digit(11, 12), Some(9));
    assert_eq!(shortcut_digit(8, 9), Some(9), "the ninth and last has nine");
}

#[test]
fn tabs_retain_tasks_closes_the_tabs_of_tasks_that_are_gone() {
    let mut tabs = strip_of(&["m1", "m2"]);
    tabs.activate(id_of(&tabs, "m1"));
    assert!(tabs.retain_tasks(|name| name != "m1"));
    assert_eq!(targets(&tabs), ["m0", "m2"]);
    assert_eq!(shown(&tabs), Some("m2"), "the right neighbour");
}

#[test]
fn tabs_retain_tasks_with_every_task_gone_leaves_no_tab_and_shows_nothing() {
    let mut tabs = strip_of(&["m1"]);
    assert!(tabs.retain_tasks(|_| false));
    assert!(tabs.is_empty());
    assert_eq!(tabs.active_id(), None);
}

#[test]
fn tabs_retain_tasks_hands_a_gone_last_tab_over_to_its_left_neighbour() {
    let mut tabs = strip_of(&["m1", "m2"]);
    tabs.activate(id_of(&tabs, "m2"));
    assert!(tabs.retain_tasks(|name| name == "m0"));
    assert_eq!(shown(&tabs), Some("m0"));
}

#[test]
fn tabs_retain_tasks_with_every_task_alive_changes_nothing() {
    let mut tabs = strip_of(&["m1"]);
    assert!(!tabs.retain_tasks(|_| true));
}

#[test]
fn tabs_to_json_and_back_keeps_order_and_the_page_shown() {
    let mut tabs = strip_of(&["m1", "m2"]);
    tabs.activate(id_of(&tabs, "m1"));
    let stored = tabs.to_json();
    assert_eq!(stored["version"], json!(STORAGE_VERSION));
    let restored = Tabs::from_json(&stored);
    let Some(restored) = restored else {
        panic!("a strip just stored must be readable: {stored}");
    };
    assert_eq!(targets(&restored), ["m0", "m1", "m2"]);
    assert_eq!(shown(&restored), Some("m1"));
}

#[test]
fn tabs_from_json_of_another_version_is_refused() {
    let stored = json!({ "version": 99, "tabs": [{ "kind": "task", "task": "m1" }], "active": 0 });
    assert_eq!(Tabs::from_json(&stored), None);
    let unversioned = json!({ "tabs": [{ "kind": "task", "task": "m1" }], "active": 0 });
    assert_eq!(Tabs::from_json(&unversioned), None);
}

#[test]
fn tabs_from_json_without_a_usable_tab_is_refused() {
    let empty = json!({ "version": STORAGE_VERSION, "tabs": [], "active": 0 });
    assert_eq!(Tabs::from_json(&empty), None);
    let broken = json!({ "version": STORAGE_VERSION, "tabs": [{ "kind": "task" }], "active": 0 });
    assert_eq!(Tabs::from_json(&broken), None, "a task tab needs its name");
    let nameless = json!({ "version": STORAGE_VERSION, "tabs": [{ "kind": "task", "task": "" }] });
    assert_eq!(Tabs::from_json(&nameless), None);
    assert_eq!(Tabs::from_json(&json!("tabs")), None);
}

#[test]
fn tabs_from_json_drops_a_second_tab_on_the_same_page() {
    let stored = json!({
        "version": STORAGE_VERSION,
        "tabs": [
            { "kind": "task", "task": "m1" },
            { "kind": "task", "task": "m0" },
            { "kind": "task", "task": "m1" },
        ],
        "active": 2,
    });
    let Some(tabs) = Tabs::from_json(&stored) else {
        panic!("readable");
    };
    assert_eq!(targets(&tabs), ["m1", "m0"]);
    assert_eq!(shown(&tabs), Some("m1"), "the page shown stays shown");
}

#[test]
fn tabs_from_json_with_no_valid_active_place_shows_the_first_tab() {
    let stored = json!({
        "version": STORAGE_VERSION,
        "tabs": [{ "kind": "task", "task": "m0" }, { "kind": "task", "task": "m1" }],
        "active": 7,
    });
    let Some(tabs) = Tabs::from_json(&stored) else {
        panic!("readable");
    };
    assert_eq!(tabs.active_index(), Some(0));
}

#[test]
fn tabs_restored_open_new_tabs_with_ids_of_their_own() {
    let stored = strip_of(&["m1", "m2"]).to_json();
    let Some(mut tabs) = Tabs::from_json(&stored) else {
        panic!("readable");
    };
    let opened = tabs.open("m3", Placement::Background);
    let same = tabs.tabs().iter().filter(|tab| tab.id == opened).count();
    assert_eq!(same, 1);
}

#[test]
fn tabs_from_json_drops_the_home_view_of_a_strip_stored_before_it_went() {
    let stored = json!({
        "version": STORAGE_VERSION,
        "tabs": [{ "kind": "today" }, { "kind": "task", "task": "m1" }, { "kind": "task", "task": "m2" }],
        "active": 2,
    });
    let Some(tabs) = Tabs::from_json(&stored) else {
        panic!("readable");
    };
    assert_eq!(targets(&tabs), ["m1", "m2"]);
    assert_eq!(shown(&tabs), Some("m2"), "the lavoro shown stays shown");
    let on_home = json!({
        "version": STORAGE_VERSION,
        "tabs": [{ "kind": "today" }, { "kind": "task", "task": "m1" }],
        "active": 0,
    });
    let Some(tabs) = Tabs::from_json(&on_home) else {
        panic!("readable");
    };
    assert_eq!(
        shown(&tabs),
        Some("m1"),
        "the view shown is gone: the first tab is"
    );
    let only_home =
        json!({ "version": STORAGE_VERSION, "tabs": [{ "kind": "today" }], "active": 0 });
    assert_eq!(Tabs::from_json(&only_home), None, "nothing left to restore");
}

#[test]
fn tabs_with_no_tab_are_stored_and_start_over_with_none() {
    let stored = Tabs::new().to_json();
    assert_eq!(stored["tabs"], json!([]));
    assert_eq!(
        Tabs::from_json(&stored),
        None,
        "the caller starts with no tab"
    );
}
