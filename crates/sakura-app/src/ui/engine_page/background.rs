//! The third card: what works in the background. The services that are always on (the keeper that holds
//! lanes alive and the phone page, which says its port while it is on), then the scheduled automations, the morning digest among them with a
//! dialog of its own. Automations are added by asking the master.

use super::digest;
use super::reset;
use super::{EnginePage, EnginePageEvent};
use crate::l10n::{say, tr, tr_with, Key};
use crate::theme::engine_page as sizes;
use crate::theme::{self, Palette};
use crate::ui::engine_acts::{on, SET_AUTOMATION, SET_SERVICE};
use crate::ui::kit::toggle;
use crate::ui::settings_modal::group_button;
use gpui::{
    div, AnyElement, App, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::{Icon, Sizable as _};
use sakura_core::engine::automations::{Automation, Digest};
use sakura_core::engine::state::{EngineState, Restart};
use sakura_core::said::Said;
use std::rc::Rc;

/// The automation that is the morning digest.
const DIGEST_NAME: &str = "morning-digest";
/// The service that serves the phone page.
const PHONE_PAGE_NAME: &str = "phone-page";
/// The schedules the engine writes: `daily 07:00`, `every 60 minutes`, `always on`.
const DAILY: &str = "daily ";
const EVERY: &str = "every ";
const MINUTES: &str = " minutes";

impl EnginePage {
    pub(super) fn background_card(
        &mut self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let snapshot = self.snapshot(cx);
        let has_scheduled = snapshot.is_some_and(|snapshot| snapshot.scheduled.is_some());
        let automations = snapshot.and_then(|snapshot| snapshot.automations.clone());
        let engine = snapshot.and_then(|snapshot| snapshot.engine.clone());
        let digest = snapshot
            .filter(|_| !has_scheduled)
            .and_then(|snapshot| snapshot.digest.clone());
        let page = cx.entity().downgrade();
        let ask = Rc::new(move |_: &mut Window, cx: &mut App| {
            drop(page.update(cx, |_, cx| {
                cx.emit(EnginePageEvent::AskMaster(
                    tr(Key("engine_page.ask_master_text")).to_string(),
                ))
            }));
        });
        let ask = group_button(
            "engine-ask-master",
            tr(Key("engine_page.ask_master")),
            ask,
            cx,
        )
        .into_any_element();
        let body = match automations {
            None => vec![self.unreported(
                Key("engine_page.automations_unreported"),
                "engine-automations-unreported",
                palette,
            )],
            Some(list) => self.automation_rows(
                &list,
                engine.as_ref(),
                (!has_scheduled, digest.as_ref()),
                palette,
                cx,
            ),
        };
        self.card(
            Key("engine_page.background.title"),
            Key("engine_page.background.note"),
            (!has_scheduled).then_some(ask),
            body,
            palette,
        )
    }

    fn automation_rows(
        &self,
        list: &[Automation],
        engine: Option<&EngineState>,
        (show_scheduled, digest): (bool, Option<&Digest>),
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let (always, mut scheduled): (Vec<&Automation>, Vec<&Automation>) =
            list.iter().partition(|item| item.is_service());
        let unlisted = unlisted_digest(list, digest);
        scheduled.extend(unlisted.as_ref());
        let heading = |key| {
            div()
                .text_size(sizes::SMALL_TEXT)
                .text_color(palette.text_tertiary)
                .child(tr(Key(key)))
                .into_any_element()
        };
        let mut rows = vec![heading("engine_page.always_on")];
        for item in always {
            rows.push(self.service_row(item, engine, palette, cx));
            if item.name == PHONE_PAGE_NAME {
                rows.extend(self.phone_block(palette, cx));
            }
        }
        if !show_scheduled {
            return rows;
        }
        rows.push(heading("engine_page.scheduled"));
        for item in scheduled {
            rows.push(self.scheduled_row(item, digest, palette, cx));
        }
        rows
    }

    /// The keeper or the phone page: a switch, when it last did something in words, and for the phone page
    /// the port it listens on while it is on.
    fn service_row(
        &self,
        item: &Automation,
        engine: Option<&EngineState>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let when = service_words(item, engine);
        let (acts, name) = (self.acts.clone(), item.name.clone());
        let key = format!("engine-service-{}", item.name);
        let press_key = key.clone();
        let switch = toggle(key.clone(), item.enabled, move |asked, _, cx| {
            let act = SET_SERVICE.act(&[("name", name.as_str()), ("on", on(asked))]);
            acts.update(cx, |acts, cx| acts.press(&press_key, act, "", cx));
        });
        let told = crate::ui::engine_acts::line(&self.acts, &key, palette, cx);
        let detail = (item.name == PHONE_PAGE_NAME && item.enabled)
            .then(|| self.phone_setup.as_ref()?.port)
            .flatten()
            .map(|port| {
                (
                    format!("engine-service-{}-address", item.name),
                    tr_with(
                        Key("engine_page.phone.address"),
                        &[("port", &port.to_string())],
                    ),
                )
            });
        automation_line(&key, item, when, detail, Some(switch), told, palette)
    }

    fn scheduled_row(
        &self,
        item: &Automation,
        digest: Option<&Digest>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = format!("engine-automation-{}", item.name);
        if item.name == DIGEST_NAME {
            return self.digest_row(item, digest, palette, cx);
        }
        let (acts, name, press_key) = (self.acts.clone(), item.name.clone(), key.clone());
        let switch = toggle(key.clone(), item.enabled, move |asked, _, cx| {
            let act = SET_AUTOMATION.act(&[("name", name.as_str()), ("on", on(asked))]);
            acts.update(cx, |acts, cx| acts.press(&press_key, act, "", cx));
        });
        let told = crate::ui::engine_acts::line(&self.acts, &key, palette, cx);
        automation_line(
            &key,
            item,
            Some(schedule_words(&item.schedule_text)),
            None,
            Some(switch),
            told,
            palette,
        )
    }

    /// The morning digest's row opens its own dialog.
    fn digest_row(
        &self,
        item: &Automation,
        digest: Option<&Digest>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sources = digest.map_or(0, |digest| digest.sources.len());
        let schedule = digest.map_or_else(
            || schedule_words(&item.schedule_text),
            |digest| tr_with(Key("engine_page.schedule.daily"), &[("at", &digest.at)]),
        );
        let when = if sources > 0 {
            let count = say(&Said::new("engine_page.sources").count(sources as u64));
            format!("{schedule}, {count}").into()
        } else {
            schedule
        };
        let (facts, acts, page) = (
            self.facts.clone(),
            self.acts.clone(),
            cx.entity().downgrade(),
        );
        let chevron = Icon::new(crate::icons::CHEVRON_CLOSED)
            .with_size(theme::MARK_SIZE)
            .text_color(palette.text_tertiary);
        let line = automation_line(
            "engine-digest-row",
            item,
            Some(when),
            None,
            Some(chevron.into_any_element()),
            None,
            palette,
        );
        div()
            .id("engine-digest-open")
            .on_click(move |_, window, cx| {
                digest::open(facts.clone(), acts.clone(), page.clone(), window, cx)
            })
            .child(line)
            .into_any_element()
    }

    /// Asks the desk to show Settings on the connectors, from the digest's dialog.
    pub(super) fn open_connectors(&mut self, cx: &mut Context<Self>) {
        cx.emit(EnginePageEvent::OpenSettings(
            crate::ui::settings_modal::Section::Connectors,
        ));
    }
}

/// One row: the name and what it does at the left (and, when there is one, a line of detail under it with
/// the name that identifies it in the window), when it runs, then its control.
fn automation_line(
    id: &str,
    item: &Automation,
    when: Option<SharedString>,
    detail: Option<(String, SharedString)>,
    control: Option<AnyElement>,
    told: Option<AnyElement>,
    palette: &Palette,
) -> AnyElement {
    let selector = id.to_string();
    let title = pretty(&item.name);
    let words = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(theme::TAB_GAP)
        .child(div().text_size(sizes::ROW_TEXT).child(title))
        .child(
            div()
                .text_size(sizes::SMALL_TEXT)
                .text_color(palette.text_tertiary)
                .child(match item.name.as_str() {
                    PHONE_PAGE_NAME => tr(Key("engine_page.phone.description")).to_string(),
                    DIGEST_NAME => tr(Key("engine_page.digest.description")).to_string(),
                    _ => item.description.clone(),
                }),
        )
        .children(detail.map(|(selector, words)| {
            div()
                .debug_selector(move || selector.clone())
                .text_size(sizes::SMALL_TEXT)
                .text_color(palette.text_tertiary)
                .child(words)
        }));
    let line = div()
        .min_h(sizes::ROW_MIN_HEIGHT)
        .py(theme::GAP_SMALL)
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .child(words)
        .children(when.map(|when| {
            div()
                .w(sizes::WHEN_WIDTH)
                .flex_none()
                .text_size(sizes::SMALL_TEXT)
                .text_color(palette.text_tertiary)
                .child(when)
        }))
        .children(control.map(|control| {
            div()
                .w(theme::CONTROL_HEIGHT)
                .flex_none()
                .flex()
                .justify_end()
                .child(control)
        }));
    div()
        .debug_selector(move || selector.clone())
        .border_t_1()
        .border_color(palette.border)
        .child(line)
        .children(told)
        .into_any_element()
}

/// A name the engine gave as words with dashes, said as words: "red-ci-watch" is "Red ci watch".
fn pretty(name: &str) -> String {
    let spaced = name
        .split('-')
        .map(|word| if word == "ci" { "CI" } else { word })
        .collect::<Vec<_>>()
        .join(" ");
    let mut letters = spaced.chars();
    match letters.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), letters.as_str()),
        None => String::new(),
    }
}

/// What a service row says under its name: that it is off when its switch is off, else what the keeper did at its
/// last restart, else that it is always on.
fn service_words(item: &Automation, engine: Option<&EngineState>) -> Option<SharedString> {
    if !item.enabled {
        return None;
    }
    let restart = engine
        .and_then(|e| e.keeper.as_ref())
        .and_then(|k| k.last_restart.as_ref());
    match (item.name.as_str(), restart) {
        ("keeper", Some(restart)) => Some(restart_words(restart)),
        _ => None,
    }
}

/// What the keeper did at its last restart: when, how many lanes it resumed and how many wait for the person.
fn restart_words(restart: &Restart) -> SharedString {
    let at = restart
        .at
        .as_deref()
        .and_then(sakura_core::engine::moment::iso_seconds)
        .map_or_else(String::new, |epoch| {
            let (_, hour, minute) = reset::weekday_and_clock(epoch, reset::local_offset(epoch));
            format!("{hour:02}:{minute:02}")
        });
    let resumed = say(&Said::new("engine_page.resumed").count(restart.resumed.len() as u64));
    let waiting = say(&Said::new("engine_page.waiting").count(restart.waiting.len() as u64));
    tr_with(
        Key("engine_page.restart"),
        &[("at", &at), ("resumed", &resumed), ("waiting", &waiting)],
    )
}

/// The morning digest as a row of its own, for an engine that has its settings but no definition of it yet
/// (it was never installed): it opens the dialog all the same.
fn unlisted_digest(list: &[Automation], digest: Option<&Digest>) -> Option<Automation> {
    let digest = digest?;
    if list.iter().any(|item| item.name == DIGEST_NAME) {
        return None;
    }
    Some(Automation {
        name: DIGEST_NAME.to_string(),
        description: String::new(),
        schedule_text: format!("{DAILY}{}", digest.at),
        enabled: false,
        installed: false,
        last_run_at: None,
        last_result: None,
    })
}

/// A schedule as the engine writes it, said in words: "daily 06:30" is "Daily at 06:30". One that is not
/// of a known form is shown as it came.
fn schedule_words(text: &str) -> SharedString {
    if let Some(at) = text.strip_prefix(DAILY) {
        return tr_with(Key("engine_page.schedule.daily"), &[("at", at)]);
    }
    let minutes = text
        .strip_prefix(EVERY)
        .and_then(|rest| rest.strip_suffix(MINUTES))
        .and_then(|count| count.parse::<u64>().ok());
    match minutes {
        Some(count) => say(&Said::new("engine_page.schedule.every").count(count)),
        None => text.to_string().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &str, is_enabled: bool) -> Automation {
        Automation {
            name: name.to_string(),
            description: String::new(),
            schedule_text: "always on".to_string(),
            enabled: is_enabled,
            installed: is_enabled,
            last_run_at: None,
            last_result: None,
        }
    }

    #[test]
    fn a_service_row_says_nothing_beside_its_switch_unless_the_keeper_restarted_lanes() {
        assert_eq!(service_words(&service("keeper", false), None), None);
        assert_eq!(service_words(&service("keeper", true), None), None);
        assert_eq!(service_words(&service("phone-page", true), None), None);
    }

    #[test]
    fn the_schedules_the_engine_writes_are_said_in_words() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        assert_eq!(schedule_words("daily 06:30"), "Daily at 06:30");
        assert_eq!(schedule_words("every 60 minutes"), "Every 60 minutes");
        assert_eq!(schedule_words("whenever"), "whenever");
    }
}
