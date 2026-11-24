//! The page model behind the page bar, with no UI: which pages are open in which tabs, which tab is shown,
//! and each tab's own back and forward history. A tab shows one [`Page`]; moving inside a tab pushes the
//! page it leaves onto that tab's history, as in a browser.
//!
//! Version 4 also keeps the split and its members' histories. Standalone strips still write version 3.

use serde_json::{json, Value};

#[cfg(test)]
mod tests;

/// The storage version for a strip without a pair.
pub const STORAGE_VERSION: u32 = 3;
const SPLIT_STORAGE_VERSION: u64 = 4;
/// The version of the pinned strip the older app wrote, which [`Tabs::restore`] still reads.
const PINNED_STORAGE_VERSION: u64 = 2;
/// Cmd+9 shows the last tab, whatever its place.
const LAST_TAB_DIGIT: usize = 9;
const DEFAULT_SPLIT_SHARE: u16 = 5000;
/// The smallest supported left pane share, in basis points.
pub const MIN_SPLIT_SHARE: u16 = 4000;
/// The largest supported left pane share, in basis points.
pub const MAX_SPLIT_SHARE: u16 = 6000;

const TALK_KEY: &str = "talk";
const VERSION_KEY: &str = "version";
const TABS_KEY: &str = "tabs";
const ACTIVE_KEY: &str = "active";
const SPLIT_KEY: &str = "split";
const LEFT_KEY: &str = "left";
const RIGHT_KEY: &str = "right";
const SHARE_KEY: &str = "left_share_bps";
const BACK_KEY: &str = "back";
const FORWARD_KEY: &str = "forward";
const PAGE_KEY: &str = "page";
const LANE_KEY: &str = "lane";
const PAGE_INBOX: &str = "inbox";
const PAGE_MASTER: &str = "master";
const PAGE_LANES: &str = "lanes";
const PAGE_SCHEDULED: &str = "scheduled";
const PAGE_ENGINE: &str = "engine";
const PAGE_LANE: &str = "lane";
const PAGE_PROJECT: &str = "project";
const PAGE_TRACK: &str = "track";
const FOLDER_KEY: &str = "folder";
// The kinds of a version 2 tab.
const KIND_KEY: &str = "kind";
const KIND_WAITING: &str = "waiting";
const KIND_MASTER: &str = "master";
const KIND_LANE: &str = "lane";

/// A tab's identity for as long as it is open: it survives a change of page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(u64);

impl TabId {
    /// The id as a number, for a view that keys its elements by tab.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// A page of the app.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Page {
    /// Every ask of every lane that needs the person.
    Inbox,
    /// The conversation with the engine's master.
    Master,
    /// The list of lanes.
    Lanes,
    Scheduled,
    /// The engine: preset, pace, accounts.
    Engine,
    /// One lane, by its key.
    Lane(String),
    /// One project, by its folder: where it is, its branch, its lanes.
    Project(String),
    Track(String),
}

/// One open tab: the page it shows and the pages it can go back and forward to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    id: TabId,
    /// The conversation of the Master this tab holds, whatever page it shows: made with the tab, kept with it
    /// across launches, and never the one of another tab, open or closed. Empty for the conversation kept
    /// before tabs had their own.
    talk: String,
    page: Page,
    back: Vec<Page>,
    forward: Vec<Page>,
}

impl Tab {
    /// The tab's identity.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The name of the tab's conversation of the Master, which tells it from the other tabs' in what is kept.
    pub fn conversation(&self) -> &str {
        &self.talk
    }

    /// The page the tab shows.
    pub fn page(&self) -> &Page {
        &self.page
    }

    /// Whether [`Tabs::back`] has a page to go to in this tab.
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// Whether [`Tabs::forward`] has a page to go to in this tab.
    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

/// Two adjacent tabs in display order, with the divider position in basis points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitTabs {
    pub left: TabId,
    pub right: TabId,
    pub left_share_bps: u16,
}

impl SplitTabs {
    /// The other member when `id` belongs to this pair.
    pub fn other(self, id: TabId) -> Option<TabId> {
        if id == self.left {
            Some(self.right)
        } else if id == self.right {
            Some(self.left)
        } else {
            None
        }
    }
}

/// The open tabs in strip order, with a selected tab and at most one pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tabs {
    tabs: Vec<Tab>,
    active: TabId,
    next_id: u64,
    split: Option<SplitTabs>,
}

impl Default for Tabs {
    fn default() -> Self {
        Self::new()
    }
}

impl Tabs {
    /// One tab, on the Inbox.
    pub fn new() -> Self {
        let active = TabId(1);
        Self {
            tabs: vec![Tab::on(active, Page::Inbox)],
            active,
            next_id: 2,
            split: None,
        }
    }

    /// Restores stored tabs, falling back to one Inbox tab for unsupported or unreadable strips.
    pub fn restore(stored: Option<&Value>) -> Self {
        stored.and_then(Self::from_json).unwrap_or_default()
    }

    /// The tabs in order.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// How many tabs are open.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Whether no tab is open: never, a strip keeps its last tab.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// The tab shown.
    pub fn active(&self) -> &Tab {
        &self.tabs[self.active_index()]
    }

    /// The place of the tab `id`, when it is open.
    pub fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    /// Looks up a tab by identity, including tabs in the background.
    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.index_of(id).map(|index| &self.tabs[index])
    }

    /// Returns the pair even when it is in the background.
    pub fn split(&self) -> Option<SplitTabs> {
        self.split
    }

    /// Returns the visible tabs in display order, independent of the active side.
    pub fn visible(&self) -> (TabId, Option<TabId>) {
        match self.split.filter(|pair| pair.other(self.active).is_some()) {
            Some(pair) => (pair.left, Some(pair.right)),
            None => (self.active, None),
        }
    }

    /// Returns the visible peer of the selected tab.
    pub fn other_in_visible_split(&self) -> Option<TabId> {
        self.split?.other(self.active)
    }

    /// Replaces the active tab's slot with the ordered pair, keeping it selected.
    pub fn create_split(&mut self, left: TabId, right: TabId) -> bool {
        if self.split.is_some() || left == right {
            return false;
        }
        let (peer, peer_is_left) = if self.active == left {
            (right, false)
        } else if self.active == right {
            (left, true)
        } else {
            return false;
        };
        let Some(index) = self.index_of(peer) else {
            return false;
        };
        let tab = self.tabs.remove(index);
        let slot = self.active_index() + usize::from(!peer_is_left);
        self.tabs.insert(slot, tab);
        self.split = Some(SplitTabs {
            left,
            right,
            left_share_bps: DEFAULT_SPLIT_SHARE,
        });
        true
    }

    /// Reverses the pair without moving the divider or changing the selected tab.
    pub fn swap_split(&mut self, member: TabId) -> bool {
        let Some(mut pair) = self.split.filter(|pair| pair.other(member).is_some()) else {
            return false;
        };
        let left = self.index_of(pair.left).expect("split members are open");
        self.tabs.swap(left, left + 1);
        std::mem::swap(&mut pair.left, &mut pair.right);
        self.split = Some(pair);
        true
    }

    /// Commits a divider change only for the current ordered pair.
    pub fn set_split_share(&mut self, left: TabId, right: TabId, left_share_bps: u16) -> bool {
        let Some(pair) = self
            .split
            .as_mut()
            .filter(|pair| (pair.left, pair.right) == (left, right))
        else {
            return false;
        };
        let share = left_share_bps.clamp(MIN_SPLIT_SHARE, MAX_SPLIT_SHARE);
        if share == pair.left_share_bps {
            return false;
        }
        pair.left_share_bps = share;
        true
    }

    /// Navigates from the selected tab.
    pub fn navigate(&mut self, page: Page) {
        self.navigate_from(self.active, page);
    }

    /// Navigates a visible source, keeping split histories local to each tab.
    pub fn navigate_from(&mut self, id: TabId, page: Page) -> bool {
        let (left, right) = self.visible();
        if left != id && right != Some(id) {
            return false;
        }
        let index = self.index_of(id).expect("visible tabs are open");
        if self.tabs[index].page == page {
            return false;
        }
        if right.is_none() {
            if let Some(open) = self.tabs.iter().find(|tab| tab.page == page) {
                self.active = open.id;
                return true;
            }
        }
        Self::navigate_in(&mut self.tabs[index], page)
    }

    /// Opens a page in the active tab even if another tab already shows that page.
    pub fn navigate_here(&mut self, page: Page) -> bool {
        Self::navigate_in(self.active_mut(), page)
    }

    fn navigate_in(tab: &mut Tab, page: Page) -> bool {
        if tab.page == page {
            return false;
        }
        let left = std::mem::replace(&mut tab.page, page);
        tab.back.push(left);
        tab.forward.clear();
        true
    }

    /// Opens `page` in a new tab at the end and shows it, even when another tab shows the page already (a
    /// new tab on the Inbox is a new tab). Returns the new tab.
    pub fn open_new(&mut self, page: Page) -> TabId {
        let id = self.new_id();
        self.tabs.push(Tab::on(id, page));
        self.active = id;
        id
    }

    /// Goes back one page in the tab shown. False when it has no history behind it.
    pub fn back(&mut self) -> bool {
        let tab = self.active_mut();
        let Some(earlier) = tab.back.pop() else {
            return false;
        };
        let left = std::mem::replace(&mut tab.page, earlier);
        tab.forward.push(left);
        true
    }

    /// Goes forward one page in the tab shown. False when it has no history ahead.
    pub fn forward(&mut self) -> bool {
        let tab = self.active_mut();
        let Some(later) = tab.forward.pop() else {
            return false;
        };
        let left = std::mem::replace(&mut tab.page, later);
        tab.back.push(left);
        true
    }

    /// Shows the tab `id`. False when it is not open or already shown.
    pub fn activate(&mut self, id: TabId) -> bool {
        if self.active == id || self.index_of(id).is_none() {
            return false;
        }
        self.active = id;
        true
    }

    /// Closes the tab `id`; the last tab goes back to the Inbox instead. The tab shown hands over to its right
    /// neighbour, else to its left one. False when nothing changed.
    pub fn close(&mut self, id: TabId) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        if self.tabs.len() == 1 {
            let tab = &mut self.tabs[0];
            let did_close = tab.reset_to(Page::Inbox);
            if did_close {
                tab.talk = new_talk();
            }
            return did_close;
        }
        let survivor = self.split.and_then(|pair| pair.other(id));
        let is_visible = self.other_in_visible_split().is_some();
        self.tabs.remove(index);
        if let Some(survivor) = survivor {
            self.split = None;
            if is_visible {
                self.active = survivor;
            }
        } else if self.active == id {
            self.active = self.tabs[index.min(self.tabs.len() - 1)].id;
        }
        true
    }

    /// Puts the tab `id` at the place `to`, clamped to the strip's ends; the tab shown and every history stay
    /// as they are. False when the tab is not open or did not move.
    pub fn move_to(&mut self, id: TabId, to: usize) -> bool {
        let Some(to) = self.reorder_target(id, to) else {
            return false;
        };
        let from = self.index_of(id).expect("the reordered tab is open");
        if to == from {
            return false;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        true
    }

    /// Finds a standalone tab's destination without inserting it between paired tabs.
    pub fn reorder_target(&self, id: TabId, to: usize) -> Option<usize> {
        let from = self.index_of(id)?;
        let mut to = to.min(self.tabs.len() - 1);
        if let Some(pair) = self.split {
            if pair.other(id).is_some() {
                return None;
            }
            let left = self.index_of(pair.left).expect("split members are open");
            let insertion_between = left + usize::from(from > left);
            if to == insertion_between {
                to = if from < left { to + 1 } else { to - 1 };
            }
        }
        Some(to)
    }

    /// Cmd+`digit`: 1 to 8 show the tab at that place, 9 the last one. False when no tab stands there or it
    /// is shown already.
    pub fn activate_digit(&mut self, digit: usize) -> bool {
        let index = match digit {
            LAST_TAB_DIGIT => self.tabs.len() - 1,
            place if (1..LAST_TAB_DIGIT).contains(&place) => place - 1,
            _ => return false,
        };
        match self.tabs.get(index) {
            Some(tab) => self.activate(tab.id),
            None => false,
        }
    }

    /// Stores pages in strip order, with histories for the two members of a split.
    pub fn to_json(&self) -> Value {
        let tabs: Vec<Value> = self
            .tabs
            .iter()
            .map(|tab| {
                let mut stored = page_to_json(&tab.page);
                stored[TALK_KEY] = json!(tab.talk);
                if self.split.is_some_and(|pair| pair.other(tab.id).is_some()) {
                    stored[BACK_KEY] = tab.back.iter().map(page_to_json).collect();
                    stored[FORWARD_KEY] = tab.forward.iter().map(page_to_json).collect();
                }
                stored
            })
            .collect();
        let mut stored = json!({
            VERSION_KEY: STORAGE_VERSION, TABS_KEY: tabs, ACTIVE_KEY: self.active_index()
        });
        if let Some(pair) = self.split {
            stored[VERSION_KEY] = json!(SPLIT_STORAGE_VERSION);
            stored[SPLIT_KEY] = json!({
                LEFT_KEY: self.index_of(pair.left),
                RIGHT_KEY: self.index_of(pair.right),
                SHARE_KEY: pair.left_share_bps
            });
        }
        stored
    }

    fn from_json(stored: &Value) -> Option<Self> {
        let version = stored.get(VERSION_KEY)?.as_u64()?;
        let read: fn(&Value) -> Option<Page> = match version {
            v if v == u64::from(STORAGE_VERSION) || v == SPLIT_STORAGE_VERSION => read_page,
            PINNED_STORAGE_VERSION => read_pinned,
            _ => return None,
        };
        let entries = stored.get(TABS_KEY)?.as_array()?;
        let mut kept = Vec::with_capacity(entries.len());
        for (place, tab) in entries.iter().enumerate() {
            if let Some(page) = read(tab) {
                kept.push((place, page, talk_of(tab)));
            } else if version != SPLIT_STORAGE_VERSION {
                return None;
            }
        }
        let (mut tabs, places) = Self::from_kept(kept, read_index(stored, ACTIVE_KEY))?;
        if version == SPLIT_STORAGE_VERSION {
            tabs.restore_split(stored, entries, &places);
        }
        Some(tabs)
    }

    /// The tabs of the stored `kept` (each page with the conversation it was kept with, none for a tab stored
    /// before conversations were kept), the tab at the stored place `shown` shown. Two tabs stored with the
    /// same conversation are one; a Master tab kept with a conversation is never taken for another; the first
    /// Master tab kept with none takes up the conversation kept before tabs had their own.
    fn from_kept(
        kept: Vec<(usize, Page, Option<String>)>,
        shown: Option<usize>,
    ) -> Option<(Self, Vec<usize>)> {
        let mut tabs: Vec<Tab> = Vec::with_capacity(kept.len());
        let mut places = Vec::with_capacity(kept.len());
        let mut shown_id = None;
        let mut has_old_talk = false;
        for (place, page, talk) in kept {
            let is_open = tabs.iter().any(|tab| match (&talk, &page) {
                (Some(talk), _) => tab.talk == *talk && !talk.is_empty(),
                (None, page) => tab.page == *page,
            });
            if is_open {
                continue;
            }
            let id = TabId(tabs.len() as u64 + 1);
            let talk = match talk {
                Some(talk) => talk,
                None if page == Page::Master && !has_old_talk => {
                    has_old_talk = true;
                    String::new()
                }
                None => new_talk(),
            };
            tabs.push(Tab::restored(id, page, talk));
            places.push(place);
            if shown == Some(place) {
                shown_id = Some(id);
            }
        }
        let active = shown_id.or(tabs.first().map(|tab| tab.id))?;
        let strip = Self {
            next_id: tabs.len() as u64 + 1,
            tabs,
            active,
            split: None,
        };
        Some((strip, places))
    }

    fn restore_split(&mut self, stored: &Value, entries: &[Value], places: &[usize]) {
        let Some(pair) = stored.get(SPLIT_KEY) else {
            return;
        };
        let (Some(left), Some(right)) = (read_index(pair, LEFT_KEY), read_index(pair, RIGHT_KEY))
        else {
            return;
        };
        let Some(index) = places.windows(2).position(|pair| pair == [left, right]) else {
            return;
        };
        if left.checked_add(1) != Some(right) {
            return;
        }
        self.tabs[index].restore_history(&entries[left]);
        self.tabs[index + 1].restore_history(&entries[right]);
        self.split = Some(SplitTabs {
            left: self.tabs[index].id,
            right: self.tabs[index + 1].id,
            left_share_bps: read_share(pair.get(SHARE_KEY)),
        });
    }

    fn active_index(&self) -> usize {
        self.index_of(self.active)
            .unwrap_or_else(|| unreachable!("the tab shown is always open"))
    }

    fn active_mut(&mut self) -> &mut Tab {
        let index = self.active_index();
        &mut self.tabs[index]
    }

    fn new_id(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        id
    }
}

impl Tab {
    /// Makes the tab show `page` with no history; false when it showed it already with none.
    fn reset_to(&mut self, page: Page) -> bool {
        let is_same = self.page == page && self.back.is_empty() && self.forward.is_empty();
        self.page = page;
        self.back.clear();
        self.forward.clear();
        !is_same
    }

    fn on(id: TabId, page: Page) -> Self {
        Self::restored(id, page, new_talk())
    }

    fn restored(id: TabId, page: Page, talk: String) -> Self {
        Self {
            id,
            talk,
            page,
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    fn restore_history(&mut self, stored: &Value) {
        let read = |key| {
            stored[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(read_page)
                .collect()
        };
        self.back = read(BACK_KEY);
        self.forward = read(FORWARD_KEY);
    }
}

fn read_index(stored: &Value, key: &str) -> Option<usize> {
    usize::try_from(stored.get(key)?.as_u64()?).ok()
}

fn read_share(stored: Option<&Value>) -> u16 {
    let Some(stored) = stored else {
        return DEFAULT_SPLIT_SHARE;
    };
    if let Some(share) = stored.as_u64() {
        return share.clamp(u64::from(MIN_SPLIT_SHARE), u64::from(MAX_SPLIT_SHARE)) as u16;
    }
    if stored.as_i64().is_some() {
        return MIN_SPLIT_SHARE;
    }
    DEFAULT_SPLIT_SHARE
}

/// The conversation a stored tab was kept with; none for a tab stored before they were kept.
fn talk_of(stored: &Value) -> Option<String> {
    stored.get(TALK_KEY)?.as_str().map(str::to_string)
}

/// A name for a new conversation that no other has had: the time it was made and a count of those made since
/// the app started, so two made at once differ too.
fn new_talk() -> String {
    static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let count = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("t{nanos:x}-{count:x}")
}

pub(crate) fn page_to_json(page: &Page) -> Value {
    match page {
        Page::Inbox => json!({ PAGE_KEY: PAGE_INBOX }),
        Page::Master => json!({ PAGE_KEY: PAGE_MASTER }),
        Page::Lanes => json!({ PAGE_KEY: PAGE_LANES }),
        Page::Scheduled => json!({ PAGE_KEY: PAGE_SCHEDULED }),
        Page::Engine => json!({ PAGE_KEY: PAGE_ENGINE }),
        Page::Lane(key) => json!({ PAGE_KEY: PAGE_LANE, LANE_KEY: key }),
        Page::Project(folder) => json!({ PAGE_KEY: PAGE_PROJECT, FOLDER_KEY: folder }),
        Page::Track(id) => json!({ PAGE_KEY: PAGE_TRACK, PAGE_TRACK: id }),
    }
}

/// The page of one stored tab of version 3; `None` for one that cannot be read.
pub(crate) fn read_page(stored: &Value) -> Option<Page> {
    match stored.get(PAGE_KEY)?.as_str()? {
        PAGE_INBOX => Some(Page::Inbox),
        PAGE_MASTER => Some(Page::Master),
        PAGE_LANES => Some(Page::Lanes),
        PAGE_SCHEDULED => Some(Page::Scheduled),
        PAGE_ENGINE => Some(Page::Engine),
        PAGE_LANE => read_lane(stored),
        PAGE_PROJECT => read_project(stored),
        PAGE_TRACK => stored
            .get(PAGE_TRACK)?
            .as_str()
            .filter(|id| !id.is_empty())
            .map(|id| Page::Track(id.into())),
        _ => None,
    }
}

/// The page of one stored tab of version 2: Waiting is the Inbox, the Master stays, a lane keeps its key;
/// `None` for one that cannot be read.
fn read_pinned(stored: &Value) -> Option<Page> {
    match stored.get(KIND_KEY)?.as_str()? {
        KIND_WAITING => Some(Page::Inbox),
        KIND_MASTER => Some(Page::Master),
        KIND_LANE => read_lane(stored),
        _ => None,
    }
}

fn read_project(stored: &Value) -> Option<Page> {
    let folder = stored.get(FOLDER_KEY)?.as_str()?;
    (!folder.is_empty()).then(|| Page::Project(folder.to_string()))
}

fn read_lane(stored: &Value) -> Option<Page> {
    let key = stored.get(LANE_KEY)?.as_str()?;
    (!key.is_empty()).then(|| Page::Lane(key.to_string()))
}

#[cfg(test)]
mod project_page_tests {
    use super::*;

    #[test]
    fn a_project_page_is_kept_by_its_folder_and_read_back() {
        let page = Page::Project("/work/orchard".to_string());
        assert_eq!(read_page(&page_to_json(&page)), Some(page));
        assert_eq!(
            read_page(&json!({ PAGE_KEY: PAGE_PROJECT })),
            None,
            "a folder is needed"
        );
        assert_eq!(
            read_page(&json!({ PAGE_KEY: PAGE_PROJECT, FOLDER_KEY: "" })),
            None
        );
    }
}
