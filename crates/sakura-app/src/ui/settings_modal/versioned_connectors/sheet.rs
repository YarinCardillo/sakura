use super::{controls, guidance, words};
use crate::data::engine_facts::{EngineFacts, Refreshed};
use crate::data::source::Source;
use crate::l10n::{tr, tr_with, Key};
use crate::theme::{self, settings_modal as sizes, Palette, Theme};
use crate::ui::dialog_frame;
use crate::ui::engine_acts::{self, Done, EngineActs, Step};
use gpui::{
    div, AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window,
};
use gpui_component::WindowExt as _;
use sakura_core::engine::accounts::Connector;
use sakura_core::engine::cli::Act;
use sakura_core::engine::connectors::{ConnectorControls, ConnectorReceipt};
use sakura_core::engine::snapshot::Snapshot;
use std::{collections::BTreeMap, rc::Rc};

const ACTION: &str = "connector-action";

pub(super) enum Selection {
    Add(&'static str),
    Manage(String),
}

pub(super) fn open(
    source: Source,
    facts: Entity<EngineFacts>,
    selection: Selection,
    window: &mut Window,
    cx: &mut App,
) {
    let acts = cx.new(|cx| EngineActs::new(source, facts.clone(), cx));
    let connection = cx.new(|cx| Sheet::new(acts, facts, selection, window, cx));
    window.open_dialog(cx, move |dialog, window, _| {
        crate::ui::dialog_frame::placed(dialog, crate::ui::dialog_frame::Width::Medium, window)
            .child(connection.clone())
    });
}

pub(super) enum View {
    Manage(String),
    Edit(Box<super::editor::Editor>),
    Replace(String),
}

pub(super) struct Sheet {
    pub(super) view: View,
    facts: Entity<EngineFacts>,
    acts: Entity<EngineActs>,
    last_request: Option<Act>,
    saved: Option<ConnectorReceipt>,
    problem: Option<&'static str>,
    _subscriptions: Vec<Subscription>,
}

impl Sheet {
    fn new(
        acts: Entity<EngineActs>,
        facts: Entity<EngineFacts>,
        selection: Selection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = match selection {
            Selection::Add(kind) => View::Edit(Box::new(super::editor::Editor::new(
                kind, None, false, window, cx,
            ))),
            Selection::Manage(id) => View::Manage(id),
        };
        let _subscriptions = vec![
            cx.observe(&facts, |_, _, cx| cx.notify()),
            cx.observe(&acts, |_, _, cx| cx.notify()),
            cx.subscribe_in(&facts, window, |sheet, _, _: &Refreshed, window, cx| {
                sheet.close_after_refresh(window, cx);
            }),
            cx.subscribe_in(&acts, window, |sheet, _, done: &Done, window, cx| {
                sheet.saved = done.connector.clone();
                if sheet.saved.is_none() {
                    sheet.problem = Some("connectors.status.unavailable");
                }
                sheet.close_after_refresh(window, cx);
                cx.notify();
            }),
        ];
        Self {
            view,
            facts,
            acts,
            last_request: None,
            saved: None,
            problem: None,
            _subscriptions,
        }
    }

    fn snapshot<'a>(&self, cx: &'a App) -> Option<&'a Snapshot> {
        self.facts.read(cx).snapshot()
    }

    fn controls<'a>(&self, cx: &'a App) -> Option<&'a ConnectorControls> {
        let snapshot = self.snapshot(cx)?;
        snapshot.connectors.as_ref()?;
        if snapshot.connectors_problem.is_some() {
            return None;
        }
        snapshot.connector_controls.as_ref()
    }

    fn rows<'a>(&self, cx: &'a App) -> &'a [Connector] {
        self.snapshot(cx)
            .and_then(|snapshot| snapshot.connectors.as_deref())
            .unwrap_or_default()
    }

    fn close_after_refresh(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(saved) = &self.saved {
            if self.rows(cx).iter().any(|row| {
                row.id == saved.id
                    && row
                        .revision
                        .is_some_and(|revision| revision >= saved.revision)
            }) {
                window.close_dialog(cx);
            }
        }
    }

    fn press(
        &mut self,
        key: &str,
        mut inputs: BTreeMap<String, String>,
        summary: SharedString,
        cx: &mut Context<Self>,
    ) {
        if self.is_pending(cx) {
            return;
        }
        let previous = self.last_request.as_ref().filter(|request| {
            request.action.key == key
                && request
                    .inputs
                    .iter()
                    .filter(|(name, _)| name.as_str() != "op")
                    .eq(inputs.iter())
        });
        let operation = previous
            .and_then(|request| request.inputs.get("op").cloned())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        inputs.insert("op".into(), operation);
        let Some(request) = self
            .controls(cx)
            .and_then(|controls| engine_acts::connector_request(controls, key, inputs))
        else {
            self.problem = Some("connectors.reason.operator");
            cx.notify();
            return;
        };
        self.problem = None;
        self.last_request = Some(request.clone());
        self.acts.update(cx, |acts, cx| {
            acts.press(ACTION, request, summary.to_string(), cx)
        });
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let View::Edit(editor) = &self.view else {
            return;
        };
        let Some(choices) = editor.payload(cx) else {
            self.problem = Some("connectors.reason.invalid_choices");
            cx.notify();
            return;
        };
        let key = if editor.original.is_some() {
            "connectoredit"
        } else {
            "connectoradd"
        };
        let mut inputs = editor
            .original
            .as_ref()
            .and_then(row_inputs)
            .unwrap_or_default();
        inputs.insert("choices".into(), choices);
        self.press(key, inputs, tr(Key("connectors.save_question")).into(), cx);
    }

    fn disconnect(&mut self, row: &Connector, cx: &mut Context<Self>) {
        let Some(inputs) = row_inputs(row) else {
            return;
        };
        let question = if affected(row).next().is_some() {
            "connectors.disconnect_tasks"
        } else {
            "connectors.disconnect_question"
        };
        self.press(
            "connectordisconnect",
            inputs,
            tr_with(Key(question), &[("label", &words::name(row))]),
            cx,
        );
    }

    fn replace(&mut self, row: &Connector, candidate: &Connector, cx: &mut Context<Self>) {
        let (Some(mut inputs), Some(revision)) = (row_inputs(row), candidate.revision) else {
            return;
        };
        inputs.extend([
            ("candidate".into(), candidate.id.clone()),
            ("candidate_revision".into(), revision.to_string()),
        ]);
        self.press(
            "connectorreplace",
            inputs,
            tr(Key("connectors.replacement_ready")).into(),
            cx,
        );
    }

    fn is_pending(&self, cx: &App) -> bool {
        self.saved.is_some()
            || matches!(
                self.acts.read(cx).step(ACTION),
                Some(Step::Running | Step::Confirming { .. })
            )
    }

    fn manage(&self, row: &Connector, palette: &Palette, cx: &Context<Self>) -> Vec<AnyElement> {
        let tools = row
            .operations
            .iter()
            .flatten()
            .map(|key| words::operation(key));
        let events = row
            .events
            .iter()
            .flatten()
            .map(|key| crate::ui::scheduled::words::event(key));
        let properties = vec![
            property("connectors.status_label", words::status(row), palette),
            property("connectors.scope", words::reads(row), palette),
            listed_property("connectors.tools", tools, palette),
            listed_property("connectors.events", events, palette),
            affected_tasks(row, palette),
        ];
        vec![
            controls::quiet(tr(Key("connectors.role_access")), palette),
            controls::group(None, properties, palette),
            self.management_buttons(row, cx),
        ]
    }

    fn management_buttons(&self, row: &Connector, cx: &Context<Self>) -> AnyElement {
        let advertised = self.controls(cx).filter(|_| row.revision.is_some());
        let (edit, disconnect, replace) = (
            super::allows(advertised, "connectoredit"),
            super::allows(advertised, "connectordisconnect")
                && row.status.as_deref() != Some("disconnected"),
            super::allows(advertised, "connectorreplace")
                && row.kind == "program"
                && self.rows(cx).iter().any(is_candidate),
        );
        let mut buttons = Vec::new();
        if let Some(kind) = super::GROUPS
            .iter()
            .find(|kind| **kind == row.kind)
            .filter(|_| edit)
        {
            let row = row.clone();
            let reconnect = !row.connected;
            let key = if reconnect {
                "connectors.reconnect"
            } else {
                "connectors.edit"
            };
            buttons.push(
                self.button("connector-edit", key, cx, move |sheet, window, cx| {
                    sheet.clear(cx);
                    sheet.view = View::Edit(Box::new(super::editor::Editor::new(
                        kind,
                        Some(row.clone()),
                        reconnect,
                        window,
                        cx,
                    )));
                    cx.notify();
                }),
            );
        }
        if disconnect {
            let row = row.clone();
            buttons.push(self.button(
                "connector-disconnect",
                "settings_modal.connectors.disconnect",
                cx,
                move |sheet, _, cx| sheet.disconnect(&row, cx),
            ));
        }
        if replace {
            let id = row.id.clone();
            buttons.push(self.button(
                "connector-replace",
                "connectors.review_replacement",
                cx,
                move |sheet, _, cx| {
                    sheet.clear(cx);
                    sheet.view = View::Replace(id.clone());
                    cx.notify();
                },
            ));
        }
        div()
            .flex()
            .flex_wrap()
            .gap(theme::GAP)
            .children(buttons)
            .into_any_element()
    }

    fn replacement_options(
        &self,
        row: &Connector,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let options = self
            .rows(cx)
            .iter()
            .filter(|row| is_candidate(row))
            .map(|candidate| {
                let (selection, original) = (candidate.clone(), row.clone());
                let control = self.button(
                    format!("connector-candidate-{}", candidate.id),
                    "connectors.review_replacement",
                    cx,
                    move |sheet, _, cx| sheet.replace(&original, &selection, cx),
                );
                super::group::row(
                    format!("connector-option-{}", candidate.id),
                    words::name(candidate),
                    Some(words::reads(candidate)),
                    control,
                    None,
                    palette,
                )
            });
        vec![
            controls::quiet(tr(Key("connectors.replacement_check")), palette),
            super::group::group(options.collect(), palette),
        ]
    }

    fn button(
        &self,
        id: impl Into<SharedString>,
        title: &'static str,
        cx: &Context<Self>,
        press: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let id = id.into();
        if id.as_ref() != "connector-close" && self.is_pending(cx) {
            return crate::ui::kit::button_disabled(
                id,
                tr(Key(title)),
                crate::ui::kit::ButtonKind::Secondary,
                &Theme::palette(cx),
            )
            .into_any_element();
        }
        let view = cx.weak_entity();
        controls::button(
            id,
            tr(Key(title)),
            Rc::new(move |window, cx| {
                drop(view.update(cx, |sheet, cx| press(sheet, window, cx)));
            }),
            cx,
        )
        .into_any_element()
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.acts.update(cx, |acts, cx| acts.cancel(ACTION, cx));
        self.problem = None;
    }

    fn form(
        &self,
        editor: &super::editor::Editor,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        if editor.kind != "feed" {
            rows.push(controls::quiet(guidance(editor.kind), palette));
        }
        rows.extend(editor.fields(palette, cx));
        rows
    }

    fn page(&self, palette: &Palette, cx: &Context<Self>) -> (SharedString, Vec<AnyElement>) {
        let (id, is_replacing) = match &self.view {
            View::Manage(id) => (id, false),
            View::Replace(id) => (id, true),
            View::Edit(editor) => {
                return (
                    tr(Key(editor.title())).into(),
                    self.form(editor, palette, cx),
                )
            }
        };
        let Some(row) = self.rows(cx).iter().find(|row| &row.id == id) else {
            let words = tr(Key("connectors.reason.unknown_connector"));
            return (
                tr(Key("settings_modal.connectors.manage")).into(),
                vec![controls::quiet(words, palette)],
            );
        };
        let body = if is_replacing {
            self.replacement_options(row, palette, cx)
        } else {
            self.manage(row, palette, cx)
        };
        (words::name(row), body)
    }
}

impl gpui::Render for Sheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let (title, body) = self.page(&palette, cx);
        let message = self
            .problem
            .map(|message| controls::quiet(tr(Key(message)), &palette))
            .or_else(|| engine_acts::line(&self.acts, ACTION, &palette, cx))
            .or_else(|| {
                self.is_pending(cx)
                    .then(|| controls::quiet(tr(Key("settings.applying")), &palette))
            });
        let editor = match &self.view {
            View::Edit(editor) => Some(editor),
            _ => None,
        };
        let save = editor.map(|editor| {
            let kind = crate::ui::kit::ButtonKind::Primary;
            let label = tr(Key("connectors.save"));
            if self.is_pending(cx) || editor.payload(cx).is_none() {
                crate::ui::kit::button_disabled("connector-save", label, kind, &palette)
            } else {
                crate::ui::kit::button("connector-save", label, kind, &palette)
                    .on_click(cx.listener(|sheet, _, _, cx| sheet.save(cx)))
            }
        });
        let close = self.button(
            "connector-close",
            if editor.is_some() {
                "common.cancel"
            } else {
                "common.close"
            },
            cx,
            |_, window, cx| window.close_dialog(cx),
        );
        dialog_frame::surface("connector-sheet", window, &palette)
            .child(dialog_frame::heading(
                "connector-sheet-title",
                title,
                &palette,
            ))
            .child(
                dialog_frame::scrolling_body("connector-sheet-scroll")
                    .gap(theme::GAP)
                    .children(body),
            )
            .children(message.map(|message| {
                div()
                    .id("connector-feedback")
                    .max_h(sizes::CONNECTOR_FEEDBACK_HEIGHT)
                    .overflow_y_scroll()
                    .child(message)
            }))
            .child(dialog_frame::footer(&palette).child(close).children(save))
    }
}

fn row_inputs(row: &Connector) -> Option<BTreeMap<String, String>> {
    Some(BTreeMap::from([
        ("name".into(), row.id.clone()),
        ("revision".into(), row.revision?.to_string()),
    ]))
}

fn is_candidate(row: &Connector) -> bool {
    row.kind != "program"
        && row.revision.is_some()
        && row.connected
        && row.scope.as_ref().and_then(|scope| scope.mode.as_deref()) == Some("scoped")
}

fn affected(
    row: &Connector,
) -> impl Iterator<Item = &sakura_core::engine::connectors::DependentTask> {
    row.dependent_tasks
        .iter()
        .flatten()
        .filter(|task| !task.label.is_empty())
}

fn affected_tasks(row: &Connector, palette: &Palette) -> AnyElement {
    let names = affected(row)
        .map(|task| {
            let id = format!("connector-dependent-{}", task.id);
            div()
                .debug_selector(move || id.clone())
                .child(task.label.clone())
        })
        .collect::<Vec<_>>();
    let empty = names.is_empty().then(|| tr(Key("connectors.none")));
    let names = div().flex().flex_col().children(empty).children(names);
    controls::row(
        "connectors.dependent_tasks",
        tr(Key("connectors.dependent_tasks")),
        names,
        None,
        palette,
    )
}

fn property(label: &'static str, value: impl Into<SharedString>, palette: &Palette) -> AnyElement {
    let label = div()
        .text_color(palette.text_secondary)
        .child(tr(Key(label)));
    div()
        .flex()
        .flex_col()
        .gap(theme::GAP_SMALL)
        .child(label)
        .child(value.into())
        .into_any_element()
}

fn listed_property<'a>(
    label: &'static str,
    values: impl Iterator<Item = &'a str>,
    palette: &Palette,
) -> AnyElement {
    let values = values.collect::<Vec<_>>().join(", ");
    let shown = if values.is_empty() {
        tr(Key("connectors.none"))
    } else {
        &values
    };
    property(label, shown.to_string(), palette)
}
