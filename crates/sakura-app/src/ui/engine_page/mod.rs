//! The Engine page: how a lane runs (the pipeline drawn, the preset, the rules of each step), the pace (where
//! each account lands at its reset, how many lanes run at once, the running lanes' own pace) and what works
//! in the background (the keeper, the phone page, the scheduled automations).
//!
//! It reads the engine's `engine`, `accounts` and `automations` parts of the snapshot and its rules; every
//! part an older engine does not give is replaced by a plain sentence, and every change is an action sent to
//! the engine, which decides. Nothing here is the source of truth: after each change the engine is read again.

mod background;
mod book;
pub(super) mod digest;
mod inspector;
pub(super) mod pace;
mod phone_setup;
mod presets;
pub(super) mod reset;
mod schema;
pub(super) mod table;
mod words;

use super::engine_acts::{self, Done, EngineActs};
use super::kit::{self, Toast};
use super::page_bar::PageControls;
use super::settings_modal::{controls, Section};
use crate::data::engine_facts::{EngineFacts, Refreshed};
use crate::data::source::Source;
use crate::l10n::{say, tr, Key};
use crate::theme::engine_page as sizes;
use crate::theme::kit::scale::{Typeset as _, TEXT_HEADLINE};
use crate::theme::{self, Palette, Theme};
use gpui::{
    div, AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window,
};
use gpui_component::input::InputState;
use gpui_component::slider::{SliderEvent, SliderState};
use sakura_core::engine::rules::Rules;
use sakura_core::engine::snapshot::Snapshot;
use sakura_core::engine::watch;
use sakura_core::said::Said;
use schema::Node;
use std::path::PathBuf;
use std::time::SystemTime;

type RuleStamp = Vec<Option<(SystemTime, u64)>>;

fn pending_setting(
    acts: &Entity<EngineActs>,
    key: &str,
    palette: &Palette,
    cx: &App,
) -> Option<AnyElement> {
    acts.read(cx).shown(key)?;
    let selector = format!("{key}-pending");
    Some(
        crate::ui::chrome::text(theme::TEXT_LABEL, palette.text_secondary)
            .debug_selector(move || selector)
            .child(tr(Key("thread.status.pending")))
            .into_any_element(),
    )
}

/// What the page asks of the desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnginePageEvent {
    /// Show Settings on this section.
    OpenSettings(Section),
    /// Open the Master with this text in its composer.
    AskMaster(String),
}

/// The Engine page's view.
pub struct EnginePage {
    facts: Entity<EngineFacts>,
    source: Source,
    /// The engine's controls this page presses, and what the engine said of them.
    acts: Entity<EngineActs>,
    /// The rules last read, and whether a read was requested.
    rules: Option<Rules>,
    has_asked: bool,
    /// A read of the rules is on its way, and another was requested meanwhile.
    is_reading_rules: bool,
    is_reading_again: bool,
    /// The programs were requested their models, because a list was empty: asked once for the page.
    has_asked_models: bool,
    engine_home: PathBuf,
    rules_stamp: RuleStamp,
    config_read: Option<gpui::Task<()>>,
    /// The node whose rules are open beside the drawing.
    selected: Node,
    /// Saving the rules in force as a new preset: whether its name is being asked, and the field.
    is_saving: bool,
    preset_name: Entity<InputState>,
    /// The pace slider, and the level it is dragged to while the person holds it.
    pace: Entity<SliderState>,
    dragging: Option<usize>,
    /// The glide of the thumb to its level after the slider is let go.
    pace_glide: Option<gpui::Task<()>>,
    /// Whether each node was on when the drawing was last made, and how often it has flipped since the page opened.
    drawn_on: std::collections::HashMap<schema::Node, bool>,
    /// What the engine says of the phone page's setup.
    phone_setup: Option<sakura_core::engine::phone::PhoneSetup>,
    /// The requested preset while the engine applies it and the confirmed rules reload.
    asked_preset: Option<String>,
    is_awaiting_preset: bool,
    node_flips: schema::Flips,
    /// How far the page is scrolled, and the keys that scroll it.
    scroll: super::page_scroll::PageScroll,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<EnginePageEvent> for EnginePage {}

impl EnginePage {
    /// The page over `source`, its facts and a window to draw its fields in.
    pub fn new(
        facts: Entity<EngineFacts>,
        source: Source,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let acts = cx.new(|cx| EngineActs::new(source.clone(), facts.clone(), cx));
        let preset_name =
            cx.new(|cx| controls::state(Key("engine_page.preset_name_hint"), window, cx));
        let pace = cx.new(|_| pace::slider());
        let subscriptions = vec![
            cx.observe_in(&facts, window, |page, _, window, cx| {
                page.facts_changed(window, cx)
            }),
            cx.subscribe(&facts, |page, _, _: &Refreshed, cx| page.refreshed(cx)),
            cx.observe_in(&acts, window, |page, _, window, cx| {
                page.resync_pace(window, cx);
                page.resync_preset(cx);
                cx.notify();
            }),
            cx.subscribe_in(&acts, window, |page, _, done: &Done, window, cx| {
                page.pressed(&done.key, window, cx)
            }),
            cx.subscribe_in(&pace, window, |page, _, event: &SliderEvent, window, cx| {
                page.pace_moved(event, window, cx)
            }),
        ];
        let mut page = Self {
            facts,
            source,
            acts,
            rules: None,
            has_asked: false,
            is_reading_rules: false,
            is_reading_again: false,
            has_asked_models: false,
            engine_home: watch::home(),
            rules_stamp: Vec::new(),
            config_read: None,
            selected: Node::Build,
            is_saving: false,
            preset_name,
            pace,
            dragging: None,
            pace_glide: None,
            drawn_on: std::collections::HashMap::new(),
            phone_setup: None,
            asked_preset: None,
            is_awaiting_preset: false,
            node_flips: schema::Flips::new(),
            scroll: super::page_scroll::PageScroll::new(cx),
            _subscriptions: subscriptions,
        };
        page.facts_changed(window, cx);
        page.refreshed(cx);
        page.read_phone(cx);
        page
    }

    /// What the box of `node` says it runs on, for the tests.
    #[cfg(test)]
    pub(super) fn agent_line_for_test(&self, node: schema::Node) -> Option<String> {
        let line = schema::Pipeline::from_rules(self.rules.as_ref()).agent_line(node);
        line.map(|line| line.to_string())
    }

    /// Reads the rule files of another folder than the engine's own home: a test's.
    #[cfg(test)]
    pub(super) fn use_engine_home(&mut self, home: PathBuf) {
        self.config_read = None;
        self.engine_home = home;
    }

    /// Shows the rules of `node` beside the drawing.
    pub fn select(&mut self, node: Node, cx: &mut Context<Self>) {
        self.selected = node;
        cx.notify();
    }

    /// The snapshot the engine last gave.
    fn snapshot<'a>(&self, cx: &'a gpui::App) -> Option<&'a Snapshot> {
        self.facts.read(cx).snapshot()
    }

    /// The engine said something new: the slider follows its pace, and the rules are read when the preset
    /// changed under the page.
    fn facts_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let engine = self
            .snapshot(cx)
            .and_then(|snapshot| snapshot.engine.clone());
        if let Some(level) = engine
            .as_ref()
            .and_then(|engine| pace::level_of(engine.pace.as_deref()))
        {
            if self.dragging.is_none() && self.pace_glide.is_none() {
                self.pace
                    .update(cx, |slider, cx| slider.set_value(level as f32, window, cx));
            }
        }
        self.complete_the_book(cx);
        let stale = engine.as_ref().is_some_and(|engine| {
            self.rules.as_ref().is_some_and(|rules| {
                Some(&rules.preset) != engine.preset.as_ref() || rules.edited != engine.edited
            })
        });
        if engine.is_some() && (!self.has_asked || stale) {
            self.read_rules(cx);
        }
        cx.notify();
    }

    /// Reads the rules. A read made while one is under way is made again
    /// when that one ends; a failed read keeps what was shown and is tried again at the next look.
    pub(super) fn read_rules(&mut self, cx: &mut Context<Self>) {
        if self.is_reading_rules {
            self.is_reading_again = true;
            return;
        }
        self.is_reading_rules = true;
        self.has_asked = true;
        let engine_home = self.engine_home.clone();
        let stamp = cx.background_spawn(async move { watch::rules_stamp(&engine_home) });
        let source = self.source.clone();
        cx.spawn(async move |page, cx| {
            let stamp = stamp.await;
            let rules = source.rules().await;
            drop(page.update(cx, |page, cx| {
                page.rules_stamp = stamp;
                page.take_rules(rules.ok(), cx);
            }));
        })
        .detach();
    }

    fn take_rules(&mut self, rules: Option<Rules>, cx: &mut Context<Self>) {
        self.is_reading_rules = false;
        self.has_asked = rules.is_some();
        if std::mem::take(&mut self.is_awaiting_preset) {
            self.drop_asked_preset();
        }
        self.rules = rules.or_else(|| self.rules.take());
        self.complete_the_book(cx);
        if std::mem::take(&mut self.is_reading_again) {
            self.read_rules(cx);
        }
        self.ask_for_empty_models(cx);
        cx.notify();
    }

    /// The accounts of the book the rules do not list yet are added to them (see `book`).
    fn complete_the_book(&mut self, cx: &gpui::App) {
        let accounts = self.snapshot(cx).map(|snapshot| snapshot.accounts.clone());
        if let (Some(rules), Some(accounts)) = (self.rules.as_mut(), accounts) {
            book::complete_agents(rules, &accounts);
        }
    }

    /// When an account's program has listed no models yet, asks the engine to ask for them (once), and reads the
    /// rules again when it did, so the step menus show the lists as they arrive.
    fn ask_for_empty_models(&mut self, cx: &mut Context<Self>) {
        let is_empty = self.rules.as_ref().is_some_and(|rules| {
            let own = rules
                .agents
                .iter()
                .filter(|a| a.models_state != book::FROM_THE_BOOK);
            own.into_iter().any(|account| account.models.is_empty())
        });
        if !is_empty || self.has_asked_models {
            return;
        }
        self.has_asked_models = true;
        let asked = self.source.act(engine_acts::REFRESH_MODELS.act(&[]));
        cx.spawn(async move |page, cx| {
            drop(asked.await);
            drop(page.update(cx, |page, cx| page.read_rules(cx)));
        })
        .detach();
    }

    /// Whether every account of the rules read has models listed, for the tests.
    #[cfg(test)]
    pub(super) fn has_models_for_test(&self) -> bool {
        self.rules.as_ref().is_some_and(|rules| {
            !rules.agents.is_empty()
                && rules
                    .agents
                    .iter()
                    .all(|account| !account.models.is_empty())
        })
    }

    fn refreshed(&mut self, cx: &mut Context<Self>) {
        let engine_home = self.engine_home.clone();
        let read = cx.background_spawn(async move { watch::rules_stamp(&engine_home) });
        self.config_read = Some(cx.spawn(async move |page, cx| {
            let stamp = read.await;
            drop(page.update(cx, |page, cx| page.take_rules_stamp(stamp, cx)));
        }));
    }

    fn take_rules_stamp(&mut self, stamp: RuleStamp, cx: &mut Context<Self>) {
        let has_engine = self
            .snapshot(cx)
            .is_some_and(|snapshot| snapshot.engine.is_some());
        if has_engine && (!self.has_asked || stamp != self.rules_stamp) {
            self.read_rules(cx);
        }
    }

    /// The engine did what a control asked: a rule or a preset says "Saved" and is read again.
    fn pressed(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if key.starts_with("engine-rule") || key.starts_with("engine-preset") {
            Toast::show(window, cx, tr(Key("engine_page.saved")), None);
            self.is_awaiting_preset =
                key.starts_with("engine-preset") && self.asked_preset.is_some();
            self.read_rules(cx);
        }
        if key.starts_with(phone_setup::PHONE_PRESS) || key == "engine-service-phone-page" {
            self.read_phone(cx);
        }
        if key == presets::SAVE_PRESS {
            self.is_saving = false;
            self.preset_name
                .update(cx, |field, cx| field.set_value("", window, cx));
        }
    }

    fn card(
        &self,
        title: Key,
        note: Key,
        right: Option<AnyElement>,
        body: Vec<AnyElement>,
        palette: &Palette,
    ) -> AnyElement {
        let head = div()
            .flex()
            .items_baseline()
            .gap(theme::GAP_WIDE)
            .child(div().text_style(TEXT_HEADLINE).child(tr(title)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(sizes::CARD_NOTE_TEXT)
                    .text_color(palette.text_tertiary)
                    .child(tr(note)),
            )
            .children(right.map(|right| div().ml_auto().child(right)));
        kit::card(format!("engine-card-{}", title.0), palette)
            .child(head)
            .children(body)
            .into_any_element()
    }

    /// What an engine that does not say says, in a card.
    fn unreported(&self, words: Key, selector: &'static str, palette: &Palette) -> AnyElement {
        div()
            .debug_selector(move || selector.into())
            .text_size(theme::TEXT_SMALL)
            .text_color(palette.text_tertiary)
            .child(tr(words))
            .into_any_element()
    }

    /// The header's line: how many lanes are open and how many of them run, and between which numbers the engine
    /// runs them; nothing before the first read.
    fn running_words(&self, cx: &gpui::App) -> Option<SharedString> {
        let snapshot = self.snapshot(cx)?;
        let count = open_words(&snapshot.items);
        let engine = snapshot.engine.as_ref()?;
        let (Some(from), Some(to)) = (engine.lanes_min, engine.lanes) else {
            return Some(count);
        };
        let range = crate::l10n::tr_with(
            Key("engine_page.header.at_once"),
            &[("from", &from.to_string()), ("to", &to.to_string())],
        );
        Some(format!("{count}, {range}").into())
    }
}

/// "5 open lanes, 1 running": the lanes the table lists, and how many of them are at work. A stuck lane, one at
/// a gate and one that waits are open, and none of them runs.
fn open_words(items: &[sakura_core::engine::snapshot::Item]) -> SharedString {
    let open = super::lanes::open_count(items);
    if open == 0 {
        return tr(Key("engine_page.header.none")).into();
    }
    let running = super::lanes::running_count(items);
    let running = match running {
        0 => tr(Key("engine_page.header.none_running")).to_string(),
        _ => crate::l10n::tr_with(
            Key("engine_page.header.some_running"),
            &[("running", &running.to_string())],
        )
        .to_string(),
    };
    let open = say(&Said::new("engine_page.header.open").count(open as u64));
    format!("{open}, {running}").into()
}

impl PageControls for EnginePage {
    fn controls(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = Theme::palette(cx);
        let words = self.running_words(cx);
        div()
            .debug_selector(|| "engine-header".into())
            .text_size(sizes::CARD_NOTE_TEXT)
            .text_color(palette.text_tertiary)
            .children(words)
            .into_any_element()
    }
}

impl EnginePage {
    pub(super) fn scroll_for(&self, key: &gpui::Keystroke) -> bool {
        self.scroll.scroll_for(key)
    }

    pub(super) fn scroll_to_rules(&self) {
        self.scroll.to_top();
    }
}

impl Render for EnginePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let cards = vec![
            self.runs_card(&palette, cx),
            self.pace_card(&palette, cx),
            self.background_card(&palette, cx),
        ];
        let page = div()
            .id("engine-page")
            .debug_selector(|| "engine-page".into())
            .size_full()
            .px(sizes::PAGE_PADDING_X)
            .pt(sizes::PAGE_PADDING_TOP)
            .pb(sizes::PAGE_PADDING_BOTTOM)
            .flex()
            .flex_col()
            .gap(sizes::CARD_GAP)
            .text_color(palette.text)
            .children(cards);
        self.scroll.wrap(page)
    }
}

#[cfg(test)]
mod layout_tests;
#[cfg(test)]
mod tests;

/// The ids of the nodes of the pipeline, for the tests that look at each.
#[cfg(test)]
pub(super) fn all_nodes() -> Vec<&'static str> {
    schema::Node::ALL.iter().map(|node| node.id()).collect()
}
