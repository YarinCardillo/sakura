//! The tab strip's model, with no UI: which lavori are open, in what order, and which one is shown. As in a
//! browser every tab is equal: none is pinned, any tab goes to any place (the first included) and any tab
//! closes while another remains. A tab shows one lavoro, and a lavoro is open in one tab at most. A strip
//! with no tab is a state like any other: nothing is shown and there is nothing to close. The strip is
//! stored with a version, so a later format can tell an older file apart.

use serde_json::{json, Value};

#[cfg(test)]
mod tests;

/// The version [`Tabs::to_json`] writes and the only one [`Tabs::from_json`] reads.
pub const STORAGE_VERSION: u32 = 1;
/// Cmd+9 selects the last tab, whatever its place.
pub const LAST_TAB_DIGIT: usize = 9;

const VERSION_KEY: &str = "version";
const TABS_KEY: &str = "tabs";
const ACTIVE_KEY: &str = "active";
const KIND_KEY: &str = "kind";
const TASK_KEY: &str = "task";
const KIND_TASK: &str = "task";
/// The kind of a tab on the home view, which a strip stored before the view was dropped (2026-11-10) may
/// hold: such an entry is skipped when the strip is read.
const KIND_FORMER_HOME: &str = "today";

/// A tab's identity for as long as it is open: it survives a move and a change of page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(u64);

impl TabId {
    /// The id as a number, for a view that keys its elements by tab.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// One open tab, and the lavoro it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub id: TabId,
    pub task: String,
}

/// Whether a tab opened beside the current one is shown at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// The new tab is shown.
    Foreground,
    /// The page on screen stays; the new tab waits at the end of the strip (a browser's Cmd+click).
    Background,
}

/// The open tabs in strip order and the one shown. While a tab is open one is shown; with none open,
/// none is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tabs {
    tabs: Vec<Tab>,
    active: Option<TabId>,
    next_id: u64,
}

impl Default for Tabs {
    fn default() -> Self {
        Self::new()
    }
}

impl Tabs {
    /// A strip with no tab.
    pub fn new() -> Self {
        Self::from_tasks(Vec::new(), 0)
    }

    /// The tabs in strip order.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// How many tabs are open.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Whether no tab is open.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// The tab shown, when one is open.
    pub fn active_id(&self) -> Option<TabId> {
        self.active
    }

    /// The place of the tab shown.
    pub fn active_index(&self) -> Option<usize> {
        self.index_of(self.active?)
    }

    /// The tab shown.
    pub fn active(&self) -> Option<&Tab> {
        self.tabs.get(self.active_index()?)
    }

    /// The place of the tab `id`, when it is open.
    pub fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    /// Shows `task` in the current tab (a click on a sidebar row). A lavoro already open in another tab is
    /// shown there instead, so none is open twice; with no tab open it gets the first. Returns the tab now
    /// shown.
    pub fn show(&mut self, task: &str) -> TabId {
        if let Some(open) = self.tab_on(task) {
            self.active = Some(open);
            return open;
        }
        match self.active_index() {
            Some(index) => {
                self.tabs[index].task = task.to_string();
                self.tabs[index].id
            }
            None => self.open(task, Placement::Foreground),
        }
    }

    /// Opens `task` in a tab of its own at the end of the strip (Cmd+click on a sidebar row); a lavoro
    /// already open keeps its tab. The first tab of a strip is shown wherever it was asked to open.
    /// Returns that tab.
    pub fn open(&mut self, task: &str, placement: Placement) -> TabId {
        let id = match self.tab_on(task) {
            Some(open) => open,
            None => {
                let id = self.new_id();
                let task = task.to_string();
                self.tabs.push(Tab { id, task });
                id
            }
        };
        if placement == Placement::Foreground || self.active.is_none() {
            self.active = Some(id);
        }
        id
    }

    /// Shows the tab `id`. False when it is not open or already shown.
    pub fn activate(&mut self, id: TabId) -> bool {
        if self.active == Some(id) || self.index_of(id).is_none() {
            return false;
        }
        self.active = Some(id);
        true
    }

    /// Closes the tab `id` while another remains. The active tab hands over to its right neighbour, else
    /// to its left one. False when nothing closed.
    pub fn close(&mut self, id: TabId) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        if self.tabs.len() == 1 {
            return false;
        }
        self.tabs.remove(index);
        if self.active == Some(id) {
            let heir = index.min(self.tabs.len() - 1);
            self.active = Some(self.tabs[heir].id);
        }
        true
    }

    /// "Close other tabs": keeps the tab `id` only, and shows it. False when nothing closed.
    pub fn close_others(&mut self, id: TabId) -> bool {
        if self.index_of(id).is_none() || self.tabs.len() == 1 {
            return false;
        }
        self.tabs.retain(|tab| tab.id == id);
        self.active = Some(id);
        true
    }

    /// Puts the tab `id` at the place `to`, clamped to the strip's ends; the lavoro shown does not change.
    /// False when the tab did not move.
    pub fn move_to(&mut self, id: TabId, to: usize) -> bool {
        let Some(from) = self.index_of(id) else {
            return false;
        };
        let to = to.min(self.tabs.len() - 1);
        if to == from {
            return false;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        true
    }

    /// Cmd+`digit`: 1 to 8 select the tab at that place, 9 the last one. Returns the tab selected, or
    /// `None` when no tab stands there.
    pub fn select_position(&mut self, digit: usize) -> Option<TabId> {
        let index = match digit {
            LAST_TAB_DIGIT => self.tabs.len().checked_sub(1)?,
            place if (1..LAST_TAB_DIGIT).contains(&place) => place - 1,
            _ => return None,
        };
        let id = self.tabs.get(index)?.id;
        self.active = Some(id);
        Some(id)
    }

    /// Closes the tabs of lavori `exists` no longer knows (a lavoro removed, a stored strip older than the
    /// lavori), the last one too. A shown tab that goes hands over to its right neighbour that stays,
    /// else to its left one; a strip left with no tab shows nothing. False when nothing closed.
    pub fn retain_tasks(&mut self, exists: impl Fn(&str) -> bool) -> bool {
        let stays = |tab: &&Tab| exists(&tab.task);
        let heir = self.active_index().and_then(|shown| {
            let (before, from_shown) = self.tabs.split_at(shown);
            let right = from_shown.iter().find(stays);
            right.or_else(|| before.iter().rev().find(stays))
        });
        let heir = heir.map(|tab| tab.id);
        let open = self.tabs.len();
        self.tabs.retain(|tab| exists(&tab.task));
        self.active = heir;
        self.tabs.len() != open
    }

    /// The strip as stored in the preferences: its version, the lavori in order and the place shown.
    pub fn to_json(&self) -> Value {
        let tabs: Vec<Value> = self
            .tabs
            .iter()
            .map(|tab| json!({ KIND_KEY: KIND_TASK, TASK_KEY: tab.task }))
            .collect();
        let shown = self.active_index().unwrap_or(0);
        json!({ VERSION_KEY: STORAGE_VERSION, TABS_KEY: tabs, ACTIVE_KEY: shown })
    }

    /// The strip a stored value describes, or `None` when the value is of another version, holds no tab or
    /// holds one that cannot be read: the caller then starts with [`Tabs::new`]. A lavoro stored twice
    /// keeps its first tab, a tab on the home view of before is dropped, and a place shown that no longer
    /// holds a tab becomes the first.
    pub fn from_json(stored: &Value) -> Option<Self> {
        if stored.get(VERSION_KEY)?.as_u64()? != u64::from(STORAGE_VERSION) {
            return None;
        }
        let read: Option<Vec<Option<String>>> = stored
            .get(TABS_KEY)?
            .as_array()?
            .iter()
            .map(read_task)
            .collect();
        let read = read?;
        let shown = stored
            .get(ACTIVE_KEY)
            .and_then(Value::as_u64)
            .and_then(|place| read.get(usize::try_from(place).ok()?)?.as_ref());
        let mut tasks: Vec<String> = Vec::with_capacity(read.len());
        for task in read.iter().flatten() {
            if !tasks.contains(task) {
                tasks.push(task.clone());
            }
        }
        let active = shown.and_then(|task| tasks.iter().position(|open| open == task));
        (!tasks.is_empty()).then(|| Self::from_tasks(tasks, active.unwrap_or(0)))
    }

    /// `tasks` (no lavoro twice) as tabs with fresh ids, the one at `active` shown.
    fn from_tasks(tasks: Vec<String>, active: usize) -> Self {
        let tabs: Vec<Tab> = (1..)
            .zip(tasks)
            .map(|(id, task)| Tab {
                id: TabId(id),
                task,
            })
            .collect();
        Self {
            active: tabs.get(active).map(|tab| tab.id),
            next_id: tabs.len() as u64 + 1,
            tabs,
        }
    }

    fn tab_on(&self, task: &str) -> Option<TabId> {
        self.tabs
            .iter()
            .find(|tab| tab.task == task)
            .map(|tab| tab.id)
    }

    fn new_id(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        id
    }
}

/// The digit of the shortcut that selects the tab at `index` of `count`: 1 to 8 for the first eight places,
/// 9 for the last tab wherever it stands, none for a tab in between.
pub fn shortcut_digit(index: usize, count: usize) -> Option<usize> {
    if index + 1 < LAST_TAB_DIGIT {
        return Some(index + 1);
    }
    (index + 1 == count).then_some(LAST_TAB_DIGIT)
}

/// The lavoro of one stored tab: `Some(None)` for an entry to skip (the home view of before), `None` for
/// one that cannot be read.
fn read_task(stored: &Value) -> Option<Option<String>> {
    match stored.get(KIND_KEY)?.as_str()? {
        KIND_FORMER_HOME => Some(None),
        KIND_TASK => {
            let name = stored.get(TASK_KEY)?.as_str()?;
            (!name.is_empty()).then(|| Some(name.to_string()))
        }
        _ => None,
    }
}
