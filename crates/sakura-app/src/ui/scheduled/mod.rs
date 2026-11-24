mod form;
mod history;
pub(super) mod words;

use super::engine_acts::{self, Done, EngineActs};
use super::kit::{self, OnPress, RowKind};
use super::settings_modal::{self, controls, Section};
use crate::data::engine_facts::{EngineFacts, WatchedRefresh};
use crate::data::source::Source;
use crate::l10n::{self, tr, tr_with, Key};
use crate::theme::{self, inbox as sizes, Palette, Theme};
use crate::ui::chrome::{text, text_width};
use gpui::{
    div, AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window,
};
use sakura_core::engine::cli::Act;
use sakura_core::engine::scheduled::{Definition, Scheduled, ScheduledTask, Trigger};
use sakura_core::engine::snapshot::{Action, Snapshot};
use std::collections::HashMap;
use std::rc::Rc;

pub(super) struct ScheduledPage {
    facts: Entity<EngineFacts>,
    source: Source,
    acts: Entity<EngineActs>,
    selected: Option<String>,
    history: history::History,
    conversation: Option<history::Conversation>,
    list_scroll: ScrollHandle,
    detail_scroll: ScrollHandle,
    conversation_scroll: ScrollHandle,
    is_shown: bool,
    run_operations: HashMap<String, String>,
    _subscriptions: Vec<Subscription>,
}

impl ScheduledPage {
    pub(super) fn new(facts: Entity<EngineFacts>, source: Source, cx: &mut Context<Self>) -> Self {
        let acts = cx.new(|cx| EngineActs::new(source.clone(), facts.clone(), cx));
        let subscriptions = vec![
            cx.observe(&facts, |_, _, cx| cx.notify()),
            cx.observe(&acts, |_, _, cx| cx.notify()),
            cx.subscribe(&facts, |page, _, _: &WatchedRefresh, cx| {
                page.refresh_detail(cx)
            }),
            cx.subscribe(&acts, |page, _, done: &Done, cx| {
                page.run_operations.remove(&done.key);
                page.refresh_detail(cx);
            }),
        ];
        Self {
            facts,
            source,
            acts,
            selected: None,
            history: history::History::default(),
            conversation: None,
            list_scroll: ScrollHandle::new(),
            detail_scroll: ScrollHandle::new(),
            conversation_scroll: ScrollHandle::new(),
            is_shown: false,
            run_operations: HashMap::new(),
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn set_shown(&mut self, is_shown: bool, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.is_shown, is_shown) != is_shown && is_shown {
            self.refresh_detail(cx);
        }
    }

    fn refresh_detail(&mut self, cx: &mut Context<Self>) {
        if self.is_shown {
            if let Some(conversation) = &self.conversation {
                self.read_conversation(conversation.id.clone(), None, cx);
            } else if let Some(id) = self.selected.clone() {
                self.read_history(id, None, cx);
            }
        }
    }

    pub(super) fn controls(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(scheduled) = self
            .facts
            .read(cx)
            .snapshot()
            .and_then(|s| s.scheduled.as_ref())
        else {
            return gpui::Empty.into_any_element();
        };
        let Some(act) = offered(
            &scheduled.actions,
            scheduled.target.as_deref(),
            "scheduledcreate",
        ) else {
            return gpui::Empty.into_any_element();
        };
        let template = scheduled
            .templates
            .iter()
            .find(|template| template.id == "morning-summary")
            .and_then(|template| {
                let locale = l10n::language().code();
                Some((
                    template.labels.get(locale)?.clone(),
                    template.definitions.get(locale)?.clone(),
                ))
            });
        div()
            .flex()
            .gap(theme::GAP_SMALL)
            .child(form_button(
                "scheduled-create",
                tr(Key("scheduled.create")),
                act.clone(),
                draft(),
                cx,
            ))
            .children(template.map(|(label, definition)| {
                form_button("scheduled-template", label, act, definition, cx)
            }))
            .into_any_element()
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        self.conversation = None;
        if self.selected.as_ref() != Some(&id) {
            self.history = history::History::default();
            self.detail_scroll = ScrollHandle::new();
        }
        self.selected = Some(id.clone());
        self.read_history(id, None, cx);
        cx.notify();
    }

    fn task_list(
        &self,
        scheduled: &Scheduled,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let rows = scheduled.tasks.iter().map(|task| {
            let id = task.id.clone();
            let row = list_row(
                format!("scheduled-task-{id}"),
                words::task_title(task).to_string(),
                words::trigger(&task.definition.when, self.facts.read(cx).snapshot()),
                words::state(&task.state),
                self.selected.as_ref() == Some(&id),
                palette,
            );
            kit::press_row(
                row,
                palette,
                cx.listener(move |page, _, _, cx| page.select(id.clone(), cx)),
            )
        });
        div()
            .id("scheduled-list")
            .debug_selector(|| "scheduled-list".into())
            .w(gpui::relative(0.34))
            .max_w(sizes::LIST_WIDTH - theme::PANEL_RING_WIDTH)
            .flex_none()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.list_scroll)
            .p(sizes::LIST_PADDING)
            .children(rows)
            .into_any_element()
    }

    fn task_detail(
        &self,
        task: &ScheduledTask,
        snapshot: &Snapshot,
        palette: &Palette,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut properties = words::properties(task, snapshot);
        properties.insert(0, ("scheduled.status", words::state(&task.state).into()));
        let label_width = properties
            .iter()
            .map(|(label, _)| {
                text_width(
                    window,
                    tr(Key(label)),
                    theme::TEXT_BODY,
                    gpui::FontWeight::NORMAL,
                )
            })
            .fold(0., f32::max)
            .ceil();
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(
                text(theme::kit::scale::TEXT_TITLE.size, palette.text)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(words::task_title(task).to_string()),
            )
            .child(controls::quiet(
                words::trigger(&task.definition.when, Some(snapshot)),
                palette,
            ))
            .children(task.reason.as_deref().map(|code| {
                controls::quiet(
                    words::task_reason(code, task.connector_label.as_deref()),
                    palette,
                )
            }))
            .child(text(theme::TEXT_BODY, palette.text).child(task.definition.prompt.clone()))
            .child(self.task_actions(task, palette, cx))
            .child(
                text(theme::TEXT_BODY, palette.text)
                    .mt(theme::CONTENT_PADDING + theme::GAP_SMALL)
                    .font_weight(theme::WEIGHT_MEDIUM)
                    .child(tr(Key("scheduled.properties"))),
            )
            .children(properties.into_iter().map(|(label, value)| {
                super::inbox::property_with_label_width(
                    label,
                    div().child(value),
                    gpui::px(label_width),
                    palette,
                )
            }))
            .children(
                (task.state == "connector_unavailable" && snapshot.connectors.is_some())
                    .then(|| self.connectors_button(cx)),
            )
            .child(
                div()
                    .mt(theme::CONTENT_PADDING + theme::GAP_SMALL)
                    .child(self.history_view(palette, cx)),
            )
            .into_any_element()
    }

    fn task_actions(
        &self,
        task: &ScheduledTask,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let labels = [
            ("scheduledupdate", "scheduled.edit"),
            ("scheduledrun", "scheduled.run"),
            ("scheduledpause", "scheduled.pause"),
            ("scheduledresume", "scheduled.resume"),
            ("scheduleddelete", "scheduled.delete"),
        ];
        let actions: Vec<_> = labels
            .into_iter()
            .filter_map(|(key, label)| {
                let act = offered(&task.actions, task.target.as_deref(), key)?;
                let id = format!("{key}-{}", task.id);
                let label = tr(Key(label));
                let title = words::task_title(task);
                let button = if key == "scheduledupdate" {
                    let mut draft = task.definition.clone();
                    draft.title = title.to_string();
                    form_button(id.clone(), label, act, draft, cx)
                } else {
                    self.task_button(id.clone(), label, act, title, palette, cx)
                };
                Some((id, button))
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_SMALL)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme::GAP_SMALL)
                    .children(actions.into_iter().map(|(_, button)| button)),
            )
            .children(labels.into_iter().filter_map(|(key, _)| {
                engine_acts::line(&self.acts, &format!("{key}-{}", task.id), palette, cx)
            }))
            .into_any_element()
    }

    fn task_button(
        &self,
        id: String,
        label: &'static str,
        act: Act,
        title: &str,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let is_run = act.action.key == "scheduledrun";
        let kind = if is_run {
            kit::ButtonKind::Primary
        } else {
            kit::ButtonKind::Secondary
        };
        if self.acts.read(cx).is_running(&id) {
            return kit::button_disabled(id, label, kind, palette).into_any_element();
        }
        let page = cx.weak_entity();
        let words = tr_with(
            Key("scheduled.delete_question"),
            &[("task", title), ("inbox", tr(Key("page.inbox")))],
        );
        kit::button(id.clone(), label, kind, palette)
            .on_click(move |_, _, cx| {
                drop(page.update(cx, |page, cx| {
                    let mut request = act.clone();
                    if request.action.key == "scheduledrun" {
                        let op = page
                            .run_operations
                            .entry(id.clone())
                            .or_insert_with(|| uuid::Uuid::new_v4().to_string());
                        request.inputs.insert("op".into(), op.clone());
                    }
                    page.acts.update(cx, |acts, cx| {
                        acts.press(&id, request, words.to_string(), cx)
                    });
                }));
            })
            .into_any_element()
    }

    fn connectors_button(&self, cx: &App) -> AnyElement {
        let (source, facts) = (self.source.clone(), self.facts.clone());
        controls::button(
            "scheduled-connectors",
            tr(Key("scheduled.connectors_settings")),
            Rc::new(move |window, cx| {
                settings_modal::open(
                    source.clone(),
                    facts.clone(),
                    Section::Connectors,
                    window,
                    cx,
                );
            }),
            cx,
        )
        .into_any_element()
    }
}

impl Render for ScheduledPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let snapshot = self.facts.read(cx).shared_snapshot();
        let Some((snapshot, scheduled)) = snapshot
            .as_deref()
            .and_then(|snapshot| Some((snapshot, snapshot.scheduled.as_ref()?)))
        else {
            return empty("scheduled.unsupported", &palette);
        };
        let selected = self
            .selected
            .as_deref()
            .and_then(|id| scheduled.tasks.iter().find(|task| task.id == id));
        let detail = match (&self.conversation, selected) {
            (Some(conversation), _) => self.conversation_view(conversation, &palette, cx),
            (None, Some(task)) => self.task_detail(task, snapshot, &palette, window, cx),
            (None, None) => empty(
                if scheduled.tasks.is_empty() {
                    "scheduled.empty"
                } else {
                    "scheduled.select"
                },
                &palette,
            ),
        };
        let problem = scheduled.problem.as_deref().map(|code| {
            div()
                .px(sizes::DETAIL_PADDING_X)
                .py(theme::GAP)
                .child(controls::quiet(words::reason(code), &palette))
        });
        let reading = div()
            .id("scheduled-detail")
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .track_scroll(if self.conversation.is_some() {
                &self.conversation_scroll
            } else {
                &self.detail_scroll
            })
            .px(sizes::DETAIL_PADDING_X)
            .py(sizes::DETAIL_PADDING_TOP)
            .child(detail)
            .into_any_element();
        let panes = if scheduled.tasks.is_empty() {
            reading
        } else {
            let list = self.task_list(scheduled, &palette, cx);
            super::split::columns(list, reading, false, &palette).into_any_element()
        };
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .children(problem)
            .child(div().flex_1().min_h_0().overflow_hidden().child(panes))
            .into_any_element()
    }
}

fn empty(key: &'static str, palette: &Palette) -> AnyElement {
    div()
        .w_full()
        .p(theme::GAP_WIDE)
        .child(controls::quiet(tr(Key(key)), palette))
        .into_any_element()
}

fn list_row(
    id: String,
    title: String,
    note: SharedString,
    state: &'static str,
    is_selected: bool,
    palette: &Palette,
) -> gpui::Stateful<gpui::Div> {
    kit::row(RowKind::Inbox, id, is_selected, palette)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(text(theme::TEXT_BODY, palette.text).child(title))
                .child(text(theme::TEXT_LABEL, palette.text_secondary).child(note)),
        )
        .child(
            text(theme::TEXT_LABEL, palette.text_secondary)
                .flex_none()
                .child(state),
        )
}

fn offered(actions: &[Action], target: Option<&str>, key: &str) -> Option<Act> {
    Some(Act {
        target: target?.to_string(),
        action: actions.iter().find(|action| action.key == key)?.clone(),
        inputs: Default::default(),
        confirm: None,
    })
}

fn draft() -> Definition {
    Definition {
        title: String::new(),
        prompt: String::new(),
        connector_ids: Vec::new(),
        when: Trigger::Interval { minutes: 60 },
        project: None,
        preset: None,
        account: None,
        may_start_lanes: false,
        enabled: true,
    }
}

fn form_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    act: Act,
    definition: Definition,
    cx: &Context<ScheduledPage>,
) -> AnyElement {
    let page = cx.weak_entity();
    let press: OnPress = Rc::new(move |window, cx| {
        if let Some(page) = page.upgrade() {
            let state = page.read(cx);
            form::open(
                state.facts.clone(),
                state.source.clone(),
                act.clone(),
                definition.clone(),
                window,
                cx,
            );
        }
    });
    settings_modal::group_button(id, label, press, cx).into_any_element()
}

#[cfg(test)]
mod tests;
