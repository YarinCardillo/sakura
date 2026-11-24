use super::{controls, words};
use crate::data::{engine_facts::EngineFacts, source::Source};
use crate::l10n::{tr, tr_with, Key};
use crate::theme::{self, Palette, Theme};
use crate::ui::dialog_frame;
use crate::ui::engine_acts::{self, Done, EngineActs};
use crate::ui::kit::{self, ButtonKind};
use gpui::{
    div, AnyElement, App, AppContext as _, Context, Entity, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window,
};
use gpui_base::{TimeField, TimeFieldState};
use gpui_component::input::{InputState, NumberInput, Textarea, TextareaState};
use gpui_component::scroll::{Scrollbar, ScrollbarMode};
use gpui_component::{Sizable as _, Size, WindowExt as _};
use sakura_core::engine::cli::Act;
use sakura_core::engine::scheduled::{Definition, Trigger};
use sakura_core::engine::snapshot::Snapshot;
use std::rc::Rc;

const SAVE: &str = "scheduled-save";
const MIN_INTERVAL: u16 = 5;
const MAX_INTERVAL: u16 = 1440;
const FREQUENCIES: [When; 3] = [When::Daily, When::Interval, When::Event];

pub(super) fn open(
    facts: Entity<EngineFacts>,
    source: Source,
    act: Act,
    draft: Definition,
    window: &mut Window,
    cx: &mut App,
) {
    if window.has_active_dialog(cx) {
        return;
    }
    let acts = cx.new(|cx| EngineActs::new(source, facts.clone(), cx));
    let form = cx.new(|cx| Form::new(facts, acts, act, draft, window, cx));
    window.open_dialog(cx, move |dialog, window, _| {
        crate::ui::dialog_frame::placed(dialog, crate::ui::dialog_frame::Width::Medium, window)
            .child(form.clone())
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum When {
    Daily,
    Interval,
    Event,
}

impl When {
    fn key(self) -> &'static str {
        match self {
            Self::Daily => "scheduled.daily",
            Self::Interval => "scheduled.interval",
            Self::Event => "scheduled.event",
        }
    }
}

struct Form {
    facts: Entity<EngineFacts>,
    acts: Entity<EngineActs>,
    action: Act,
    draft: Definition,
    title: Entity<InputState>,
    prompt: Entity<TextareaState>,
    interval: Entity<InputState>,
    time: Entity<TimeFieldState>,
    timezone: Entity<InputState>,
    when: When,
    event: Option<(String, String)>,
    op: Option<(String, String)>,
    problem: Option<&'static str>,
    fields_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl Form {
    fn new(
        facts: Entity<EngineFacts>,
        acts: Entity<EngineActs>,
        action: Act,
        draft: Definition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mode = match &draft.when {
            Trigger::Daily { .. } => When::Daily,
            Trigger::Interval { .. } => When::Interval,
            Trigger::ConnectorEvent { .. } => When::Event,
        };
        let (interval, time, timezone, event) = trigger_fields(&draft.when);
        let title = input("scheduled.name_hint", draft.title.clone(), window, cx);
        let interval = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(interval)
                .validate(|value, _| {
                    value.is_empty()
                        || value
                            .parse::<u16>()
                            .is_ok_and(|value| value <= MAX_INTERVAL)
                })
                .min(f64::from(MIN_INTERVAL))
                .max(f64::from(MAX_INTERVAL))
        });
        let time = time_editor(&time, window, cx);
        let timezone = input("scheduled.timezone_hint", timezone, window, cx);
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 5)
                .placeholder(tr(Key("scheduled.prompt_hint")))
                .default_value(draft.prompt.clone())
        });
        let subscriptions = vec![
            cx.observe(&facts, |_, _, cx| cx.notify()),
            cx.observe(&acts, |_, _, cx| cx.notify()),
            cx.observe(&interval, |_, _, cx| cx.notify()),
            cx.observe(&time, |_, _, cx| cx.notify()),
            cx.observe(&timezone, |_, _, cx| cx.notify()),
            cx.subscribe_in(&acts, window, |_, _, _: &Done, window, cx| {
                window.close_dialog(cx)
            }),
        ];
        Self {
            facts,
            acts,
            action,
            draft,
            title,
            prompt,
            when: mode,
            event,
            interval,
            time,
            timezone,
            op: None,
            problem: None,
            fields_scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn trigger(&self, cx: &App) -> Option<Trigger> {
        match self.when {
            When::Daily => Some(Trigger::Daily {
                time: self.time.read(cx).time().format("%H:%M").to_string(),
                timezone: self.timezone.read(cx).value().trim().into(),
            }),
            When::Interval => Some(Trigger::Interval {
                minutes: interval_minutes(&self.interval.read(cx).value())?,
            }),
            When::Event => {
                let (connector_id, event_kind) = self.event.clone()?;
                Some(Trigger::ConnectorEvent {
                    connector_id,
                    event_kind,
                })
            }
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        self.problem = None;
        let Some(when) = self.trigger(cx) else {
            self.problem = Some("invalid_when");
            cx.notify();
            return;
        };
        let mut definition = self.draft.clone();
        definition.title = self.title.read(cx).value().to_string();
        definition.prompt = self.prompt.read(cx).value().to_string();
        definition.when = when;
        let Ok(payload) = serde_json::to_string(&definition) else {
            return;
        };
        let mut request = self.action.clone();
        if request.action.needs.iter().any(|name| name == "op") {
            let op = self
                .op
                .get_or_insert_with(|| (payload.clone(), uuid::Uuid::new_v4().to_string()));
            if op.0 != payload {
                *op = (payload.clone(), uuid::Uuid::new_v4().to_string());
            }
            request.inputs.insert("op".into(), op.1.clone());
        }
        request.inputs.insert("definition".into(), payload);
        self.acts
            .update(cx, |acts, cx| acts.press(SAVE, request, "", cx));
        cx.notify();
    }

    fn when_fields(
        &self,
        snapshot: Option<&Snapshot>,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let view = cx.weak_entity();
        let options = FREQUENCIES.map(|when| (tr(Key(when.key())).into(), when == self.when));
        let modes = kit::segments("scheduled-when", &options, move |index, _, cx| {
            drop(view.update(cx, |form, cx| {
                form.when = FREQUENCIES[index];
                cx.notify();
            }));
        });
        let control = match self.when {
            When::Daily => self.daily_fields(palette, cx),
            When::Interval => self.interval_fields(palette),
            When::Event => self.event_menu(snapshot, palette, cx),
        };
        let picker = div()
            .debug_selector(|| "scheduled-when-row".into())
            .flex()
            .flex_col()
            .gap(theme::GAP_TIGHT)
            .child(controls::field_label(
                tr(Key("scheduled.schedule")).into(),
                palette,
            ))
            .child(modes);
        if self.when == When::Daily {
            return vec![div()
                .flex()
                .items_end()
                .gap(theme::GAP)
                .child(picker.flex_none())
                .child(control)
                .into_any_element()];
        }
        vec![picker.into_any_element(), control]
    }

    fn daily_fields(&self, palette: &Palette, cx: &App) -> AnyElement {
        use gpui_component::ActiveTheme as _;
        let selection = cx.theme().selection;
        let time = div()
            .flex_none()
            .debug_selector(|| "scheduled-time".into())
            .flex()
            .flex_col()
            .gap(theme::GAP_SMALL)
            .child(controls::field_label(
                tr(Key("scheduled.time")).into(),
                palette,
            ))
            .child(
                div().debug_selector(|| "scheduled-clock".into()).child(
                    TimeField::new("scheduled-time-field", &self.time)
                        .flex()
                        .items_center()
                        .h(theme::CONTROL_HEIGHT)
                        .px(theme::CONTROL_PADDING_X)
                        .font_features(crate::ui::chrome::tabular())
                        .rounded(theme::RADIUS_CONTROL)
                        .text_color(palette.text)
                        .text_size(theme::TEXT_BODY)
                        .bg(palette.fill)
                        .render_segment(move |segment, state, _, _| {
                            use gpui::prelude::FluentBuilder as _;
                            segment
                                .when(state.is_selected(), |segment| segment.bg(selection))
                                .into_any_element()
                        }),
                ),
            );
        let zone = div()
            .flex()
            .flex_col()
            .gap(theme::GAP_SMALL)
            .child(
                controls::field_label(tr(Key("scheduled.timezone")).into(), palette)
                    .debug_selector(|| "scheduled-zone-label".into()),
            )
            .child(
                div()
                    .debug_selector(|| "scheduled-zone".into())
                    .h(theme::CONTROL_HEIGHT)
                    .flex()
                    .items_center()
                    .rounded(theme::RADIUS_CONTROL)
                    .bg(palette.fill)
                    .child(
                        controls::styled_input(&self.timezone, theme::kit::scale::TEXT_BODY)
                            .w_full()
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false)
                            .aria_label(tr(Key("scheduled.timezone"))),
                    ),
            );
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_start()
            .gap(theme::GAP)
            .pr(theme::GAP_SMALL)
            .child(time)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(theme::DIALOG_WIDTH_SMALL / 2.)
                    .child(zone),
            )
            .into_any_element()
    }

    fn interval_fields(&self, palette: &Palette) -> AnyElement {
        div()
            .debug_selector(|| "scheduled-minutes".into())
            .flex()
            .items_center()
            .gap(theme::GAP)
            .child(
                div()
                    .w(theme::DIALOG_WIDTH_SMALL / 3.)
                    .flex_none()
                    .rounded(theme::RADIUS_CONTROL)
                    .bg(palette.fill)
                    .child(
                        NumberInput::new(&self.interval)
                            .h(theme::CONTROL_HEIGHT)
                            .appearance(false)
                            .suffix(tr(Key("scheduled.minutes_unit")))
                            .accessibility_label(tr(Key("scheduled.minutes"))),
                    ),
            )
            .into_any_element()
    }

    fn event_menu(
        &self,
        snapshot: Option<&Snapshot>,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let connectors = snapshot
            .and_then(|s| s.connectors.as_deref())
            .unwrap_or_default();
        let mut shown = tr(Key("scheduled.pick_event")).into();
        let mut choices = Vec::new();
        for connector in connectors
            .iter()
            .filter(|c| c.connected && !c.label.is_empty())
        {
            for event in connector
                .events
                .as_deref()
                .or(connector.event_kinds.as_deref())
                .unwrap_or_default()
            {
                let key = (connector.id.clone(), event.clone());
                let label: SharedString =
                    format!("{}, {}", connector.label, words::event(event)).into();
                let is_on = self.event.as_ref() == Some(&key);
                if is_on {
                    shown = label.clone();
                }
                let form = cx.weak_entity();
                choices.push(controls::Choice {
                    label,
                    is_on,
                    on_pick: Rc::new(move |_, cx| {
                        drop(form.update(cx, |form, cx| {
                            form.event = Some(key.clone());
                            cx.notify();
                        }))
                    }),
                });
            }
        }
        if choices.is_empty() {
            return controls::quiet(tr(Key("scheduled.no_events")), palette);
        }
        let menu = controls::menu("scheduled-event", shown, choices, cx);
        controls::row(
            "scheduled-event-row",
            tr(Key("scheduled.event")),
            menu,
            None,
            palette,
        )
    }

    fn selection_fields(
        &self,
        snapshot: &Snapshot,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (projects, presets, accounts) = choices(snapshot);
        let defaults = words::defaults(snapshot);
        let options = [
            (
                "scheduled.project",
                projects,
                &self.draft.project,
                (|draft: &mut Definition| &mut draft.project) as Selection,
            ),
            (
                "scheduled.preset",
                presets,
                &self.draft.preset,
                (|draft: &mut Definition| &mut draft.preset) as Selection,
            ),
            (
                "scheduled.account",
                accounts,
                &self.draft.account,
                (|draft: &mut Definition| &mut draft.account) as Selection,
            ),
        ];
        let rows =
            options
                .into_iter()
                .zip(defaults)
                .map(|((label, items, selected, assign), default)| {
                    let menu = self.choice(label, items, selected, assign, default.as_deref(), cx);
                    crate::ui::settings_modal::group::row(
                        format!("{label}-row"),
                        tr(Key(label)),
                        None,
                        menu,
                        None,
                        palette,
                    )
                });
        div()
            .debug_selector(|| "scheduled-properties".into())
            .flex_none()
            .child(crate::ui::settings_modal::group::group(
                rows.collect(),
                palette,
            ))
            .into_any_element()
    }

    fn choice(
        &self,
        label: &'static str,
        options: Options,
        selected: &Option<String>,
        assign: Selection,
        default: Option<&str>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let resolved = default.unwrap_or_else(|| words::unresolved_default(label));
        let current = selected.as_deref().map_or(resolved, |id| {
            option_label(&options, id).unwrap_or_else(|| tr(Key("scheduled.unknown")))
        });
        let current = SharedString::from(current);
        let default = tr_with(Key("scheduled.default_value"), &[("value", resolved)]);
        let choices = std::iter::once((None, default))
            .chain(
                options
                    .into_iter()
                    .map(|(id, label)| (Some(id), label.into())),
            )
            .map(|(id, label)| {
                let form = cx.weak_entity();
                controls::Choice {
                    label,
                    is_on: &id == selected,
                    on_pick: Rc::new(move |_, cx| {
                        drop(form.update(cx, |form, cx| {
                            *assign(&mut form.draft) = id.clone();
                            cx.notify();
                        }))
                    }),
                }
            })
            .collect();
        controls::menu(label, current, choices, cx)
    }

    fn connectors(
        &self,
        snapshot: &Snapshot,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let rows = snapshot
            .connectors
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|c| !c.label.is_empty())
            .filter(|c| c.connected || self.draft.connector_ids.contains(&c.id))
            .map(|connector| {
                let (id, view) = (connector.id.clone(), cx.weak_entity());
                let is_on = self.draft.connector_ids.contains(&id);
                let control = kit::toggle(
                    format!("scheduled-connector-{id}"),
                    is_on,
                    move |is_on, _, cx| {
                        drop(view.update(cx, |form, cx| {
                            if is_on && !form.draft.connector_ids.contains(&id) {
                                form.draft.connector_ids.push(id.clone());
                            } else if !is_on {
                                form.draft.connector_ids.retain(|key| key != &id);
                            }
                            cx.notify();
                        }));
                    },
                );
                crate::ui::settings_modal::group::row(
                    format!("scheduled-connector-row-{}", connector.id),
                    connector.label.clone(),
                    None,
                    control,
                    None,
                    palette,
                )
            })
            .collect::<Vec<_>>();
        (!rows.is_empty()).then(|| {
            div()
                .debug_selector(|| "scheduled-connectors-group".into())
                .child(dialog_frame::group(
                    "scheduled-connectors-title",
                    tr(Key("scheduled.connectors")),
                    rows,
                    palette,
                ))
                .into_any_element()
        })
    }

    fn footer(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let cancel = controls::button(
            "scheduled-form-cancel",
            tr(Key("common.cancel")),
            Rc::new(|window, cx| window.close_dialog(cx)),
            cx,
        );
        let save = if self.acts.read(cx).is_running(SAVE) {
            kit::button_disabled(
                SAVE,
                tr(Key("scheduled.save")),
                ButtonKind::Primary,
                palette,
            )
            .into_any_element()
        } else {
            let view = cx.weak_entity();
            controls::primary_button(
                SAVE,
                tr(Key("scheduled.save")),
                Rc::new(move |_, cx| {
                    drop(view.update(cx, |form, cx| form.save(cx)));
                }),
                cx,
            )
        };
        dialog_frame::footer(palette)
            .child(cancel)
            .child(save)
            .into_any_element()
    }

    fn permission(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let view = cx.weak_entity();
        let (toggle, label) = kit::toggle_with_label(
            "scheduled-may-start",
            self.draft.may_start_lanes,
            move |is_on, _, cx| {
                drop(view.update(cx, |form, cx| {
                    form.draft.may_start_lanes = is_on;
                    cx.notify();
                }));
            },
        );
        let row = kit::settings_row_acting(
            "scheduled-permission-row",
            None,
            tr(Key("scheduled.may_start")),
            None,
            toggle,
            (None, Some(label)),
            palette,
        )
        .debug_selector(|| "scheduled-may-start-row".into());
        crate::ui::tip::bind(
            div().id("scheduled-permission-tip-target").child(
                crate::ui::settings_modal::group::group(vec![row.into_any_element()], palette),
            ),
            "scheduled-permission-tip",
            crate::ui::tip::Words::plain(tr(Key("scheduled.may_start_hint"))),
            cx,
        )
        .into_any_element()
    }
}

impl Render for Form {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let snapshot = self.facts.read(cx).shared_snapshot();
        let heading = if self.action.action.key == "scheduledcreate" {
            "scheduled.create"
        } else {
            "scheduled.edit"
        };
        let fields = self.fields(snapshot.as_deref(), &palette, cx);
        let feedback = self
            .problem
            .map(|code| controls::quiet(words::reason(code), &palette))
            .or_else(|| engine_acts::line(&self.acts, SAVE, &palette, cx));
        dialog_frame::surface("scheduled-dialog", window, &palette)
            .child(dialog_frame::heading(
                "scheduled-title",
                tr(Key(heading)),
                &palette,
            ))
            .child(fields)
            .children(feedback)
            .child(self.footer(&palette, cx))
    }
}

impl Form {
    fn fields(
        &self,
        snapshot: Option<&Snapshot>,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let connectors = snapshot.and_then(|snapshot| self.connectors(snapshot, palette, cx));
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                dialog_frame::scrolling_body("scheduled-form-fields")
                    .track_scroll(&self.fields_scroll)
                    .gap(theme::GAP)
                    .child(self.title_field(palette, cx))
                    .child(self.prompt_field(palette, cx))
                    .children(snapshot.map(|snapshot| self.selection_fields(snapshot, palette, cx)))
                    .children(self.when_fields(snapshot, palette, cx))
                    .children(connectors)
                    .child(self.permission(palette, cx)),
            )
            .child(crate::ui::page_scroll::edge_fades(
                &self.fields_scroll,
                palette.overlay_surface,
            ))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(gpui::relative(1.))
                    .w(theme::GAP_WIDE)
                    .child(
                        Scrollbar::vertical(&self.fields_scroll)
                            .viewport_from_layout()
                            .mode(ScrollbarMode::Scrolling),
                    ),
            )
            .into_any_element()
    }

    fn title_field(&self, palette: &Palette, cx: &App) -> AnyElement {
        let ring = kit::keyboard_ring(palette);
        dialog_frame::title_field(
            ("scheduled-name-label", "scheduled-name-caption"),
            tr(Key("scheduled.name")),
            &self.title,
            palette,
        )
        .id("scheduled-name")
        .track_focus(&self.title.focus_handle(cx))
        .focus_visible(move |style| style.shadow(ring))
        .rounded(theme::RADIUS_CONTROL)
        .into_any_element()
    }

    fn prompt_field(&self, palette: &Palette, cx: &App) -> AnyElement {
        let ring = kit::keyboard_ring(palette);
        dialog_frame::filled_field(
            ("scheduled-prompt", "scheduled-prompt-caption"),
            tr(Key("scheduled.prompt")),
            Textarea::new(&self.prompt)
                .appearance(false)
                .bordered(false)
                .with_size(Size::Medium)
                .text_size(theme::new_lane::DESCRIPTION_TEXT)
                .aria_label(tr(Key("scheduled.prompt"))),
            palette,
        )
        .id("scheduled-prompt")
        .track_focus(&self.prompt.focus_handle(cx))
        .focus_visible(move |style| style.shadow(ring))
        .rounded(theme::RADIUS_CONTROL)
        .into_any_element()
    }
}

type Options = Vec<(String, String)>;
type Selection = fn(&mut Definition) -> &mut Option<String>;

fn interval_minutes(value: &str) -> Option<u16> {
    value
        .trim()
        .parse()
        .ok()
        .filter(|n| (MIN_INTERVAL..=MAX_INTERVAL).contains(n))
}

fn time_editor(time: &str, window: &mut Window, cx: &mut Context<Form>) -> Entity<TimeFieldState> {
    cx.new(|cx| {
        let mut editor = TimeFieldState::new(window, cx);
        if let Ok(time) = chrono::NaiveTime::parse_from_str(time, "%H:%M") {
            editor.set_time(time, window, cx);
        }
        editor
    })
}

fn option_label<'a>(options: &'a Options, id: &str) -> Option<&'a str> {
    options
        .iter()
        .find(|(key, _)| key == id)
        .map(|(_, label)| label.as_str())
}

fn choices(snapshot: &Snapshot) -> (Options, Options, Options) {
    let projects = snapshot
        .projects
        .iter()
        .filter(|p| !p.folder_missing)
        .map(|p| (p.folder.clone(), p.name.clone()))
        .collect();
    let presets = snapshot
        .engine
        .as_ref()
        .into_iter()
        .flat_map(|e| &e.presets)
        .map(|p| (p.name.clone(), p.words()))
        .collect();
    let accounts = snapshot
        .accounts
        .iter()
        .filter(|a| !a.is_hidden())
        .map(|a| (a.id.clone(), a.name().into()))
        .collect();
    (projects, presets, accounts)
}

fn input(
    key: &'static str,
    value: String,
    window: &mut Window,
    cx: &mut Context<Form>,
) -> Entity<InputState> {
    cx.new(|cx| controls::state(Key(key), window, cx).default_value(value))
}

pub(super) fn trigger_fields(when: &Trigger) -> (String, String, String, Option<(String, String)>) {
    let local_zone = || iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into());
    match when {
        Trigger::Daily { time, timezone } => ("60".into(), time.clone(), timezone.clone(), None),
        Trigger::Interval { minutes } => (minutes.to_string(), "07:30".into(), local_zone(), None),
        Trigger::ConnectorEvent {
            connector_id,
            event_kind,
        } => (
            "60".into(),
            "07:30".into(),
            local_zone(),
            Some((connector_id.clone(), event_kind.clone())),
        ),
    }
}

#[cfg(test)]
mod title_tests {
    use super::*;
    use crate::ui::{engine_desk_tests, engine_rig_tests};

    #[gpui::test]
    fn polish_y_task_title_fits_its_placeholder_and_typed_descenders(
        cx: &mut gpui::TestAppContext,
    ) {
        let fixture = include_str!("../../../../sakura-core/tests/fixtures/scheduled.json");
        let (rig, _, cx) = engine_desk_tests::open_unread(cx, fixture);
        engine_rig_tests::wait_until(cx, "the scheduled fixture", |cx| {
            rig.facts
                .read_with(cx, |facts, _| facts.snapshot().is_some())
        });
        let form = cx.update(|window, cx| {
            let facts = rig.facts.clone();
            let scheduled = facts
                .read(cx)
                .snapshot()
                .unwrap()
                .scheduled
                .as_ref()
                .unwrap();
            let act = super::super::offered(
                &scheduled.actions,
                scheduled.target.as_deref(),
                "scheduledcreate",
            )
            .unwrap();
            let acts = cx.new(|cx| EngineActs::new(rig.source.clone(), facts.clone(), cx));
            let form = cx.new(|cx| Form::new(facts, acts, act, super::super::draft(), window, cx));
            window.open_dialog(cx, {
                let form = form.clone();
                move |dialog, window, _| {
                    dialog_frame::placed(dialog, dialog_frame::Width::Medium, window)
                        .child(form.clone())
                }
            });
            form
        });
        engine_rig_tests::wait_until(cx, "the title field", |cx| {
            cx.debug_bounds("scheduled-name-label").is_some()
        });
        let title = form.read_with(cx, |form, _| form.title.clone());
        let line_height = theme::kit::scale::TEXT_TITLE.line_height;
        for text in ["", "Typography gjpqy"] {
            cx.update(|window, cx| {
                title.update(cx, |input, cx| input.set_value(text, window, cx));
                window.draw(cx).clear(cx);
            });
            let field = cx.debug_bounds("scheduled-name-label").unwrap();
            title.read_with(cx, |input, _| {
                let viewport = input.input_bounds();
                assert!(
                    viewport.size.height >= line_height,
                    "{text:?}: {viewport:?}"
                );
                let line = input.range_to_bounds(&(0..text.len())).unwrap();
                assert!(line.top() >= viewport.top() && line.bottom() <= viewport.bottom());
                assert!(line.left() >= field.left() && line.right() <= field.right());
            });
        }
    }
}
