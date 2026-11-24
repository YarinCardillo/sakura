//! Keeps what the person typed and has not sent: the composers tell it, it saves into the preferences after
//! a pause in typing (and at once when asked), and it gives the text back when a page is built again, in
//! this launch or the next.

use crate::prefs::Preferences;
use crate::theme::DRAFT_PAUSE;
use gpui::{Context, Task};
use sakura_core::drafts::Drafts;
use sakura_core::tabs::Page;
use std::collections::HashMap;

/// The unsent text of every composer.
pub struct DraftKeeper {
    drafts: Drafts,
    editors: HashMap<Page, Vec<String>>,
    /// The wait before the next save; a new key replaces it, so a save follows a pause and never a key.
    waiting: Option<Task<()>>,
}

impl DraftKeeper {
    /// A keeper holding the drafts the preferences kept.
    pub fn new(preferences: &Preferences) -> Self {
        Self {
            drafts: Drafts::restore(preferences.drafts.as_ref()),
            waiting: None,
            editors: HashMap::new(),
        }
    }

    /// Assigns the original draft slot to the first editor when another tab opens the page.
    pub fn open_editor(&mut self, page: &Page, owner: &str, cx: &mut Context<Self>) {
        let editors = self.editors.entry(page.clone()).or_default();
        if editors.iter().any(|editor| editor == owner) {
            return;
        }
        if let Some(first) = editors.first() {
            if self.drafts.claim_legacy_owner(page, first) {
                self.waiting = Some(Self::wait_to_save(cx));
            }
        }
        editors.push(owner.to_string());
    }

    /// Drops live editor registrations for closed tabs, preserving their saved drafts.
    pub fn retain_editors(&mut self, owners: &[String]) {
        self.editors.retain(|_, editors| {
            editors.retain(|editor| owners.contains(editor));
            !editors.is_empty()
        });
    }

    /// The text kept for `page`.
    pub fn restored(&self, page: &Page, owner: &str) -> Option<String> {
        self.drafts
            .get_for_owner(page, None, owner)
            .map(str::to_string)
    }

    /// The text kept for each agent of the lane `page` that has some.
    pub fn restored_agents(&self, page: &Page, owner: &str) -> Vec<(String, String)> {
        self.drafts.agents_for_owner(page, owner)
    }

    /// The composer that writes to `agent` of the lane `page` now holds `text`.
    pub fn note_for(
        &mut self,
        page: &Page,
        agent: Option<&str>,
        owner: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let is_same = match self.drafts.get_for_owner(page, agent, owner) {
            Some(kept) => kept == text,
            None => text.trim().is_empty(),
        };
        if is_same {
            return;
        }
        self.drafts.set_for_owner(page, agent, owner, text);
        self.waiting = Some(Self::wait_to_save(cx));
    }

    /// The names of the files that were attached to `page`'s composer and not sent when the drafts were kept.
    pub fn restored_files(&self, page: &Page, owner: &str) -> Vec<String> {
        self.drafts.files_for_owner(page, owner).to_vec()
    }

    /// `page`'s composer now has the files named `names` attached: they are kept with the draft, so that a person
    /// is told which to attach again if the app closes first.
    pub fn note_files(
        &mut self,
        page: &Page,
        owner: &str,
        names: &[String],
        cx: &mut Context<Self>,
    ) {
        if self.drafts.files_for_owner(page, owner) == names {
            return;
        }
        self.drafts.set_files_for_owner(page, owner, names);
        self.waiting = Some(Self::wait_to_save(cx));
    }

    /// Drops the drafts of the lanes whose keys are not in `listed`: the engine no longer has them.
    pub fn forget_lanes_not_in(&mut self, listed: &[String], cx: &mut Context<Self>) {
        let before = self.drafts.clone();
        self.drafts.retain(|page| match page {
            Page::Lane(key) => listed.contains(key),
            _ => true,
        });
        if self.drafts != before {
            self.waiting = Some(Self::wait_to_save(cx));
        }
    }

    /// Writes the drafts down now, when a save is waiting: the person is leaving the page.
    pub fn flush(&mut self, cx: &mut Context<Self>) {
        if self.waiting.take().is_none() {
            return;
        }
        let stored = self.drafts.to_json();
        Preferences::change(cx, move |preferences| preferences.drafts = Some(stored));
    }

    fn wait_to_save(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |keeper, cx| {
            cx.background_executor().timer(DRAFT_PAUSE).await;
            drop(keeper.update(cx, |keeper, cx| keeper.flush(cx)));
        })
    }
}
