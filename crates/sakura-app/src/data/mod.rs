//! Facts from `box`: where they come from, when to refetch, and the entity the views read.

pub mod actions;
pub mod changes;
pub mod facts;
pub mod fixture_changes;
pub mod projects;
pub mod refresh;
pub mod sessions;
pub mod source;

use std::collections::HashMap;
use std::hash::Hash;

/// Drops every entry whose task `exists` no longer accepts; `task_of` names the task of a key.
pub fn retain_tasks<K: Eq + Hash, V>(
    map: &mut HashMap<K, V>,
    task_of: impl Fn(&K) -> &str,
    exists: impl Fn(&str) -> bool,
) {
    map.retain(|key, _| exists(task_of(key)));
}

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
