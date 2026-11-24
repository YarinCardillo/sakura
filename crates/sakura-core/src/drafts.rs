//! What the person has typed and not sent, by page: the text of the Master's composer and of each lane's
//! composer, kept so that leaving a page or quitting the app loses nothing. No UI: the preferences store
//! [`Drafts::to_json`] next to the tabs, and a stored value that cannot be read is dropped, never an error.

use crate::tabs::{page_to_json, read_page, Page};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// The version [`Drafts::to_json`] writes.
pub const STORAGE_VERSION: u32 = 1;

const VERSION_KEY: &str = "version";
const DRAFTS_KEY: &str = "drafts";
const PAGE_KEY: &str = "page";
const TEXT_KEY: &str = "text";
const AGENT_KEY: &str = "agent";
const FILES_KEY: &str = "files";
const OWNER_KEY: &str = "owner";
const OWNERS_KEY: &str = "owners";
const NAMES_KEY: &str = "names";

/// The unsent text of the pages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Drafts {
    /// By the page as stored, so that the order of the stored list never depends on the order of writing.
    by_page: BTreeMap<String, Draft>,
    /// The names of the files attached to a page's composer and not sent, by the page as stored: the files
    /// themselves are not kept, so a person is told which ones to attach again.
    files: BTreeMap<String, AttachmentNames>,
    legacy_owners: BTreeMap<String, (Page, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Draft {
    page: Page,
    owner: Option<String>,
    /// The agent of a lane the text is for (its first run's name); none for a page's own composer.
    agent: Option<String>,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttachmentNames {
    page: Page,
    owner: Option<String>,
    names: Vec<String>,
}

impl Drafts {
    /// The drafts a stored value holds; none for a missing value, another version or one that cannot be
    /// read. An entry that cannot be read (a page this app does not know, no text) is left out.
    pub fn restore(stored: Option<&Value>) -> Self {
        let mut drafts = Self::default();
        let Some(stored) = stored else {
            return drafts;
        };
        let is_ours =
            stored.get(VERSION_KEY).and_then(Value::as_u64) == Some(u64::from(STORAGE_VERSION));
        let entries = stored.get(DRAFTS_KEY).and_then(Value::as_array);
        let files = stored.get(FILES_KEY).and_then(Value::as_array);
        for entry in files.filter(|_| is_ours).into_iter().flatten() {
            let page = entry.get(PAGE_KEY).and_then(read_page);
            let names = entry.get(NAMES_KEY).and_then(Value::as_array);
            let names: Vec<String> = names
                .into_iter()
                .flatten()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect();
            if let Some(page) = page {
                drafts.set_scoped_files(&page, stored_owner(entry), &names);
            }
        }
        for entry in entries.filter(|_| is_ours).into_iter().flatten() {
            let page = entry.get(PAGE_KEY).and_then(read_page);
            let text = entry.get(TEXT_KEY).and_then(Value::as_str);
            let agent = entry.get(AGENT_KEY).and_then(Value::as_str);
            if let Some((page, text)) = page.zip(text) {
                drafts.set_scoped(&page, agent, stored_owner(entry), text);
            }
        }
        if is_ours {
            for entry in stored[OWNERS_KEY].as_array().into_iter().flatten() {
                if let (Some(page), Some(owner)) =
                    (read_page(&entry[PAGE_KEY]), stored_owner(entry))
                {
                    drafts.claim_legacy_owner(&page, owner);
                }
            }
        }
        drafts
    }

    /// Records who keeps using the original page slot once there are multiple editors.
    pub fn claim_legacy_owner(&mut self, page: &Page, owner: &str) -> bool {
        use std::collections::btree_map::Entry;
        match self.legacy_owners.entry(page_to_json(page).to_string()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                entry.insert((page.clone(), owner.to_string()));
                true
            }
        }
    }

    fn scope<'a>(&self, page: &Page, owner: &'a str) -> Option<&'a str> {
        self.legacy_owners
            .get(&page_to_json(page).to_string())
            .filter(|(_, kept)| kept != owner)
            .map(|_| owner)
    }

    /// Reads the text for a tab's page and agent.
    pub fn get_for_owner(&self, page: &Page, agent: Option<&str>, owner: &str) -> Option<&str> {
        self.get_scoped(page, agent, self.scope(page, owner))
    }

    /// Writes only the draft belonging to this tab's page and agent.
    pub fn set_for_owner(&mut self, page: &Page, agent: Option<&str>, owner: &str, text: &str) {
        self.set_scoped(page, agent, self.scope(page, owner), text);
    }

    /// Reads the attachment names belonging to this tab's page.
    pub fn files_for_owner(&self, page: &Page, owner: &str) -> &[String] {
        self.scoped_files(page, self.scope(page, owner))
    }

    /// Writes the attachment names belonging to this tab's page.
    pub fn set_files_for_owner(&mut self, page: &Page, owner: &str, names: &[String]) {
        self.set_scoped_files(page, self.scope(page, owner), names);
    }

    /// Lists agent drafts for this tab's lane.
    pub fn agents_for_owner(&self, page: &Page, owner: &str) -> Vec<(String, String)> {
        self.scoped_agents(page, self.scope(page, owner))
    }

    /// The text kept for `page`'s own composer.
    pub fn get(&self, page: &Page) -> Option<&str> {
        self.get_for(page, None)
    }

    /// The text kept for `page`'s composer, or for the one that writes to `agent` of a lane.
    pub fn get_for(&self, page: &Page, agent: Option<&str>) -> Option<&str> {
        self.get_scoped(page, agent, None)
    }

    fn get_scoped(&self, page: &Page, agent: Option<&str>, owner: Option<&str>) -> Option<&str> {
        self.by_page
            .get(&key_of(page, agent, owner))
            .map(|draft| draft.text.as_str())
    }

    /// Keeps `text` as the draft of `page`, as typed; blank text forgets the draft.
    pub fn set(&mut self, page: &Page, text: &str) {
        self.set_for(page, None, text);
    }

    /// Keeps `text` as the draft for `agent` of the lane `page`, as typed; blank text forgets that draft only.
    pub fn set_for(&mut self, page: &Page, agent: Option<&str>, text: &str) {
        self.set_scoped(page, agent, None, text);
    }

    fn set_scoped(&mut self, page: &Page, agent: Option<&str>, owner: Option<&str>, text: &str) {
        let key = key_of(page, agent, owner);
        if text.trim().is_empty() {
            self.by_page.remove(&key);
            return;
        }
        let draft = Draft {
            page: page.clone(),
            agent: agent.map(str::to_string),
            owner: owner.map(str::to_string),
            text: text.to_string(),
        };
        self.by_page.insert(key, draft);
    }

    /// Keeps the names of the files attached to `page`'s composer and not sent; none forgets them.
    pub fn set_files(&mut self, page: &Page, names: &[String]) {
        self.set_scoped_files(page, None, names);
    }

    fn set_scoped_files(&mut self, page: &Page, owner: Option<&str>, names: &[String]) {
        let key = key_of(page, None, owner);
        if names.is_empty() {
            self.files.remove(&key);
        } else {
            self.files.insert(
                key,
                AttachmentNames {
                    page: page.clone(),
                    owner: owner.map(str::to_string),
                    names: names.to_vec(),
                },
            );
        }
    }

    /// The names of the files attached to `page`'s composer when the drafts were kept.
    pub fn files(&self, page: &Page) -> &[String] {
        self.scoped_files(page, None)
    }

    fn scoped_files(&self, page: &Page, owner: Option<&str>) -> &[String] {
        let kept = self.files.get(&key_of(page, None, owner));
        kept.map_or(&[][..], |files| files.names.as_slice())
    }

    /// The agents of the lane `page` that have a draft, with its text, by the agent's name.
    pub fn agents_of(&self, page: &Page) -> Vec<(String, String)> {
        self.scoped_agents(page, None)
    }

    fn scoped_agents(&self, page: &Page, owner: Option<&str>) -> Vec<(String, String)> {
        let mut found: Vec<(String, String)> = self
            .by_page
            .values()
            .filter(|draft| draft.page == *page && draft.owner.as_deref() == owner)
            .filter_map(|draft| Some((draft.agent.clone()?, draft.text.clone())))
            .collect();
        found.sort();
        found
    }

    /// Keeps the drafts of the pages `is_wanted` accepts.
    pub fn retain(&mut self, is_wanted: impl Fn(&Page) -> bool) {
        self.by_page.retain(|_, draft| is_wanted(&draft.page));
        self.files.retain(|_, files| is_wanted(&files.page));
        self.legacy_owners.retain(|_, (page, _)| is_wanted(page));
    }

    /// How many composers have a draft.
    pub fn len(&self) -> usize {
        self.by_page.len()
    }

    /// Whether no page has a draft.
    pub fn is_empty(&self) -> bool {
        self.by_page.is_empty()
    }

    /// The drafts as the preferences store them.
    pub fn to_json(&self) -> Value {
        let drafts: Vec<Value> = self
            .by_page
            .values()
            .map(|draft| {
                let mut entry =
                    json!({ PAGE_KEY: page_to_json(&draft.page), TEXT_KEY: draft.text });
                if let Some(agent) = &draft.agent {
                    entry[AGENT_KEY] = json!(agent);
                }
                with_owner(entry, draft.owner.as_deref())
            })
            .collect();
        let mut stored = json!({ VERSION_KEY: STORAGE_VERSION, DRAFTS_KEY: drafts });
        if !self.files.is_empty() {
            let files: Vec<Value> = self
                .files
                .values()
                .map(|files| {
                    let entry =
                        json!({ PAGE_KEY: page_to_json(&files.page), NAMES_KEY: files.names });
                    with_owner(entry, files.owner.as_deref())
                })
                .collect();
            stored[FILES_KEY] = json!(files);
        }
        if !self.legacy_owners.is_empty() {
            stored[OWNERS_KEY] = self
                .legacy_owners
                .values()
                .map(|(page, owner)| json!({ PAGE_KEY: page_to_json(page), OWNER_KEY: owner }))
                .collect();
        }
        stored
    }
}

fn with_owner(mut entry: Value, owner: Option<&str>) -> Value {
    if let Some(owner) = owner {
        entry[OWNER_KEY] = json!(owner);
    }
    entry
}

fn stored_owner(entry: &Value) -> Option<&str> {
    entry.get(OWNER_KEY)?.as_str()
}

fn key_of(page: &Page, agent: Option<&str>, owner: Option<&str>) -> String {
    format!("{}|{}", page_to_json(page), json!([agent, owner]))
}
