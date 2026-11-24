//! The facts the window shows: from the engine (`engine_facts`, over `source`), and with the `boxes` feature
//! from `box` as well.

#[cfg(feature = "boxes")]
pub mod actions;
#[cfg(feature = "boxes")]
pub mod changes;
#[cfg(not(feature = "boxes"))]
pub mod engine_facts;
#[cfg(feature = "boxes")]
pub mod facts;
#[cfg(feature = "boxes")]
pub mod fixture_changes;
pub mod latest_write;
#[cfg(not(feature = "boxes"))]
pub mod master_history;
#[cfg(feature = "boxes")]
pub mod projects;
#[cfg(feature = "boxes")]
pub mod refresh;
#[cfg_attr(not(feature = "boxes"), allow(dead_code))]
pub mod sessions;
#[cfg(feature = "boxes")]
pub mod source;
#[cfg(not(feature = "boxes"))]
#[path = "engine_source.rs"]
pub mod source;

#[cfg(feature = "boxes")]
use std::collections::HashMap;
#[cfg(feature = "boxes")]
use std::hash::Hash;

#[cfg(feature = "boxes")]
/// Drops every entry whose task `exists` no longer accepts; `task_of` names the task of a key.
pub fn retain_tasks<K: Eq + Hash, V>(
    map: &mut HashMap<K, V>,
    task_of: impl Fn(&K) -> &str,
    exists: impl Fn(&str) -> bool,
) {
    map.retain(|key, _| exists(task_of(key)));
}

#[cfg(feature = "boxes")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_of_tasks_that_no_longer_exist_are_dropped() {
        let mut map: HashMap<(String, String), u8> = HashMap::new();
        map.insert(("m1".into(), "test".into()), 1);
        map.insert(("gone".into(), "test".into()), 2);
        retain_tasks(&mut map, |(task, _)| task.as_str(), |task| task == "m1");
        assert_eq!(map.len(), 1);
        assert!(map.contains_key(&("m1".to_string(), "test".to_string())));
    }
}
