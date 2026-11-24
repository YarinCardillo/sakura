//! The morning digest's dialog: whether it runs, when, how far back it looks, in which language and on which
//! model and account, and its sources (which come from Settings, Connectors), each on or off with when it was
//! last read. "Run now" runs it at once.

use super::reset;
use super::EnginePage;
use crate::data::engine_facts::EngineFacts;
use crate::icons::ConnectorIcon;
use crate::l10n::{tr, tr_count, tr_with, Key};
use crate::theme::engine_page as sizes;
use crate::theme::{self, Palette, Theme};
use crate::ui::chrome::connector_mark;
use crate::ui::dialog_frame;
use crate::ui::dialog_guard::DialogToken;
use crate::ui::engine_acts::{
    self, on, EngineActs, RUN_DIGEST, SET_AUTOMATION, SET_DIGEST, SET_DIGEST_SOURCE,
};
use crate::ui::kit::toggle;
use crate::ui::settings_modal::{controls, group_button};
use gpui::{
    div, AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, WeakEntity, Window,
};
use gpui_component::WindowExt as _;
use sakura_core::engine::automations::{Automation, Digest, DigestSource};
use sakura_core::engine::snapshot::Snapshot;
use std::rc::Rc;

/// The automation the dialog is about.
const DIGEST_NAME: &str = "morning-digest";
/// The hours of the day the digest can run at, and how far back it can look.
pub(in crate::ui) const RUN_HOURS: [u32; 8] = [5, 6, 7, 8, 9, 10, 11, 12];
pub(in crate::ui) const LOOKBACK_HOURS: [u32; 4] = [12, 24, 48, 72];
/// The languages a digest is written in, as the engine names them (a language code), with the word of each.
pub(in crate::ui) const LANGUAGES: [(&str, Key); 2] = [
    ("en", Key("engine_page.digest.english")),
    ("it", Key("engine_page.digest.italian")),
];
/// The key the settings' presses are kept under.
const SETTINGS_PRESS: &str = "engine-digest";

/// Opens the dialog over the window.
pub(super) fn open(
    facts: Entity<EngineFacts>,
    acts: Entity<EngineActs>,
    page: WeakEntity<EnginePage>,
    window: &mut Window,
    cx: &mut App,
) {
    let sheet = cx.new(|cx| DigestSheet::new(facts, acts, page, cx));
    window.open_dialog(cx, move |dialog, window, _| {
        crate::ui::dialog_frame::placed(dialog, crate::ui::dialog_frame::Width::Medium, window)
            .child(sheet.clone())
    });
}

/// The dialog's view.
struct DigestSheet {
    facts: Entity<EngineFacts>,
    acts: Entity<EngineActs>,
    page: WeakEntity<EnginePage>,
    scroll: ScrollHandle,
    _observers: Vec<gpui::Subscription>,
    /// Says to the tab keys that a dialog is open, for as long as the sheet lives.
    _open: DialogToken,
}

impl DigestSheet {
    fn new(
        facts: Entity<EngineFacts>,
        acts: Entity<EngineActs>,
        page: WeakEntity<EnginePage>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observers = vec![
            cx.observe(&facts, |_, _, cx| cx.notify()),
            cx.observe(&acts, |_, _, cx| cx.notify()),
        ];
        Self {
            facts,
            acts,
            page,
            scroll: ScrollHandle::new(),
            _observers: observers,
            _open: DialogToken::take(cx),
        }
    }

    /// The digest's settings, and its automation (whether it runs each day), when the engine lists them.
    fn digest(&self, cx: &App) -> (Option<Digest>, Option<Automation>) {
        let Some(snapshot) = self.facts.read(cx).snapshot() else {
            return (None, None);
        };
        (snapshot.digest.clone(), morning_digest(snapshot))
    }

    /// The accounts that are not hidden, by id and by the name they are shown under.
    fn accounts(&self, cx: &App) -> Vec<(String, String)> {
        let snapshot = self.facts.read(cx).snapshot();
        let visible = snapshot
            .into_iter()
            .flat_map(|snapshot| &snapshot.accounts)
            .filter(|account| !account.is_hidden());
        visible
            .map(|account| (account.id.clone(), account.name().to_string()))
            .collect()
    }

    /// A setting of the digest as a menu: each choice sends `value` for `rule`.
    fn setting(
        &self,
        id: &'static str,
        shown: SharedString,
        choices: Vec<(SharedString, String, bool)>,
        cx: &App,
    ) -> AnyElement {
        let picks = choices
            .into_iter()
            .map(|(label, value, is_on)| {
                let acts = self.acts.clone();
                controls::Choice {
                    label,
                    is_on,
                    on_pick: Rc::new(move |_, cx: &mut App| {
                        let act = SET_DIGEST.act(&[(id, &value)]);
                        acts.update(cx, |acts, cx| acts.press(SETTINGS_PRESS, act, "", cx));
                    }),
                }
            })
            .collect();
        controls::menu(format!("engine-digest-{id}"), shown, picks, cx)
    }

    fn settings_rows(
        &self,
        settings: &Digest,
        item: Option<&Automation>,
        palette: &Palette,
        cx: &App,
    ) -> Vec<AnyElement> {
        let switch = item.map(|item| {
            let (acts, name) = (self.acts.clone(), item.name.clone());
            toggle("engine-digest-on", item.enabled, move |asked, _, cx| {
                let act = SET_AUTOMATION.act(&[("name", name.as_str()), ("on", on(asked))]);
                acts.update(cx, |acts, cx| acts.press(SETTINGS_PRESS, act, "", cx));
            })
        });
        let hours = RUN_HOURS.map(|hour| {
            let at = format!("{hour:02}:00");
            (
                SharedString::from(at.clone()),
                at.clone(),
                at == settings.at,
            )
        });
        let lookback = LOOKBACK_HOURS.map(|count| {
            let words = tr_count(Key("engine_page.digest.hours"), u64::from(count), &[]);
            (
                words,
                count.to_string(),
                Some(count) == settings.lookback_hours,
            )
        });
        let languages = LANGUAGES.map(|(name, word)| {
            (
                SharedString::from(tr(word)),
                name.to_string(),
                settings.language.eq_ignore_ascii_case(name),
            )
        });
        let mut rows: Vec<AnyElement> = switch
            .map(|switch| {
                line(
                    "engine-digest-row-on",
                    Key("engine_page.digest.on"),
                    switch,
                    palette,
                )
            })
            .into_iter()
            .collect();
        rows.extend([
            line(
                "engine-digest-row-at",
                Key("engine_page.digest.at"),
                self.setting("at", settings.at.clone().into(), hours.to_vec(), cx),
                palette,
            ),
            line(
                "engine-digest-row-lookback",
                Key("engine_page.digest.lookback"),
                self.setting(
                    "lookback_hours",
                    shown_hours(settings.lookback_hours),
                    lookback.to_vec(),
                    cx,
                ),
                palette,
            ),
            line(
                "engine-digest-row-language",
                Key("engine_page.digest.language"),
                self.setting(
                    "language",
                    shown_language(&settings.language),
                    languages.to_vec(),
                    cx,
                ),
                palette,
            ),
        ]);
        let accounts = self.accounts(cx);
        if accounts.len() > 1 {
            let chosen = settings.account.clone().unwrap_or_default();
            let choices = accounts
                .iter()
                .map(|(id, name)| (SharedString::from(name.clone()), id.clone(), *id == chosen))
                .collect();
            let shown = accounts.iter().find(|(id, _)| *id == chosen).map_or_else(
                || tr(Key("engine_page.digest.account_default")).to_string(),
                |(_, name)| name.clone(),
            );
            rows.push(line(
                "engine-digest-row-account",
                Key("engine_page.digest.account"),
                self.setting("account", shown.into(), choices, cx),
                palette,
            ));
        }
        rows
    }

    fn source_row(&self, source: &DigestSource, palette: &Palette, cx: &App) -> AnyElement {
        let (acts, name) = (self.acts.clone(), source.id.clone());
        let key = format!("engine-digest-source-{}", source.id);
        let (press_key, press_key_for_line, key_for_mark) = (key.clone(), key.clone(), key.clone());
        let switch = toggle(key.clone(), source.enabled, move |asked, _, cx| {
            let act = SET_DIGEST_SOURCE.act(&[("name", name.as_str()), ("on", on(asked))]);
            acts.update(cx, |acts, cx| acts.press(&press_key, act, "", cx));
        });
        let read = source
            .last_good_run
            .as_deref()
            .and_then(sakura_core::engine::moment::iso_seconds)
            .map_or_else(
                || tr(Key("engine_page.digest.not_read")).to_string(),
                |epoch| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(epoch, |since| since.as_secs() as i64);
                    let at = reset::clock_words_in(epoch, now, reset::local_offset(epoch));
                    tr_with(Key("engine_page.digest.read_at"), &[("at", &at)]).to_string()
                },
            );
        let (title, reads_selector) = (source.name().to_string(), format!("{key}-reads"));
        let mark = ConnectorIcon::of_source(&source.kind, source.connector.as_deref())
            .map(|icon| connector_mark(icon, format!("{key_for_mark}-mark"), palette));
        let told = engine_acts::line(&self.acts, &press_key_for_line, palette, cx);
        let row = crate::ui::kit::row(crate::ui::kit::RowKind::Settings, key, false, palette)
            .children(mark)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(theme::TAB_GAP)
                    .child(div().text_size(sizes::ROW_TEXT).child(title))
                    .child(
                        div()
                            .debug_selector(move || reads_selector)
                            .text_size(sizes::SMALL_TEXT)
                            .text_color(palette.text_tertiary)
                            .child(reads_or_kind(source)),
                    ),
            )
            .child(
                div()
                    .text_size(sizes::SMALL_TEXT)
                    .text_color(palette.text_tertiary)
                    .child(read),
            )
            .child(switch);
        div().child(row).children(told).into_any_element()
    }

    fn footer(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let page = self.page.clone();
        let open_connectors = Rc::new(move |window: &mut Window, cx: &mut App| {
            window.close_dialog(cx);
            drop(page.update(cx, |page, cx| page.open_connectors(cx)));
        });
        let acts = self.acts.clone();
        let run = Rc::new(move |_: &mut Window, cx: &mut App| {
            let act = RUN_DIGEST.act(&[]);
            acts.update(cx, |acts, cx| acts.press("engine-digest-run", act, "", cx));
        });
        let told = crate::ui::engine_acts::line(&self.acts, "engine-digest-run", palette, cx);
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(
                dialog_frame::footer(palette)
                    .child(group_button(
                        "engine-digest-connectors",
                        tr(Key("engine_page.digest.open_connectors")),
                        open_connectors,
                        cx,
                    ))
                    .child(controls::primary_button(
                        "engine-digest-run",
                        tr(Key("engine_page.digest.run_now")),
                        run,
                        cx,
                    )),
            )
            .children(told)
            .into_any_element()
    }
}

/// The language's word when it is one of ours, else what the engine said.
fn shown_language(language: &str) -> SharedString {
    let known = LANGUAGES
        .iter()
        .find(|(name, _)| language.eq_ignore_ascii_case(name));
    known.map_or_else(|| language.to_string().into(), |(_, word)| tr(*word).into())
}

/// What a source reads, in the engine's phrase; for a source it says nothing of, the word for what it reads
/// through.
fn reads_or_kind(source: &DigestSource) -> SharedString {
    match source.reads.is_empty() {
        true => kind_words(&source.kind),
        false => SharedString::from(source.reads.clone()),
    }
}

/// The word for what a source reads through: a connector, a feed or a program; what the engine said for
/// another kind.
fn kind_words(kind: &str) -> SharedString {
    match kind {
        "connector" => tr(Key("engine_page.digest.kind.connector")).into(),
        "feed" => tr(Key("engine_page.digest.kind.feed")).into(),
        "program" => tr(Key("engine_page.digest.kind.program")).into(),
        other => other.to_string().into(),
    }
}

/// The automation that runs the morning digest each day, when the engine lists it.
pub(super) fn morning_digest(snapshot: &Snapshot) -> Option<Automation> {
    let listed = snapshot.automations.as_ref()?;
    listed.iter().find(|item| item.name == DIGEST_NAME).cloned()
}

/// "24 hours" for a look-back the engine reported; empty for one it did not.
fn shown_hours(hours: Option<u32>) -> SharedString {
    hours.map_or_else(SharedString::default, |count| {
        tr_count(Key("engine_page.digest.hours"), u64::from(count), &[])
    })
}

/// One row of settings: its name at the left, its control at the right, a hairline above.
fn line(id: &'static str, name: Key, control: AnyElement, palette: &Palette) -> AnyElement {
    crate::ui::settings_modal::group::row(id, tr(name), None, control, None, palette)
}

impl Render for DigestSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let (settings, item) = self.digest(cx);
        let (rows, sources, problem) = match &settings {
            Some(settings) => {
                let rows = self.settings_rows(settings, item.as_ref(), &palette, cx);
                let sources: Vec<AnyElement> = settings
                    .sources
                    .iter()
                    .map(|source| self.source_row(source, &palette, cx))
                    .collect();
                (rows, sources, settings.problem.clone())
            }
            None => (Vec::new(), Vec::new(), None),
        };
        let told = engine_acts::line(&self.acts, SETTINGS_PRESS, &palette, cx);
        let footer = self.footer(&palette, cx);
        let body = crate::ui::dialog_frame::scrolling_body("engine-digest-body")
            .track_scroll(&self.scroll)
            .gap(theme::GAP_WIDE)
            .children(problem.map(|problem| {
                div()
                    .text_size(sizes::SMALL_TEXT)
                    .text_color(palette.danger)
                    .child(problem)
            }))
            .children(
                (!rows.is_empty()).then(|| crate::ui::settings_modal::group::group(rows, &palette)),
            )
            .children(told)
            .children((!sources.is_empty()).then(|| {
                dialog_frame::group(
                    "engine-digest-sources",
                    tr(Key("engine_page.digest.sources")),
                    sources,
                    &palette,
                )
            }));
        dialog_frame::surface("engine-digest-dialog", window, &palette)
            .text_color(palette.text)
            .font_family(theme::FONT_UI)
            .text_size(theme::TEXT_SMALL)
            .child(dialog_frame::heading(
                "engine-digest-title",
                tr(Key("engine_page.digest.title")),
                &palette,
            ))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(body)
                    .child(crate::ui::page_scroll::edge_fades(
                        &self.scroll,
                        palette.overlay_surface,
                    )),
            )
            .child(div().flex_none().child(footer))
    }
}
