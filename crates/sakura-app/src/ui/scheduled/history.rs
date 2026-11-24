use super::{controls, list_row, words, ScheduledPage};
use crate::l10n::{say, tr, Key};
use crate::theme::{self, Palette};
use crate::ui::{chrome::text, thread};
use gpui::{
    div, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    ScrollHandle, SharedString, Styled as _,
};
use sakura_core::engine::scheduled::{ConversationPage, HistoryPage};
use std::rc::Rc;

#[derive(Default)]
pub(super) struct History {
    page: HistoryPage,
    request: Option<Rc<()>>,
    failure: Option<SharedString>,
}

pub(super) struct Conversation {
    pub(super) id: String,
    page: ConversationPage,
    request: Option<Rc<()>>,
    failure: Option<SharedString>,
}

impl ScheduledPage {
    pub(super) fn read_history(&mut self, id: String, before: Option<u64>, cx: &mut Context<Self>) {
        let pending = self.source.scheduled_history(id.clone(), before);
        let request = Rc::new(());
        self.history.request = Some(request.clone());
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            drop(this.update(cx, |view, cx| {
                if view.selected.as_ref() != Some(&id)
                    || !is_current(&view.history.request, &request)
                {
                    return;
                }
                view.history.request = None;
                view.history.failure = None;
                match result {
                    Ok(page) => view.history.apply(page, before),
                    Err(error) => view.history.failure = Some(say(&error.said)),
                }
                cx.notify();
            }));
        })
        .detach();
        cx.notify();
    }

    pub(in crate::ui) fn open_run(&mut self, id: String, cx: &mut Context<Self>) {
        if self
            .conversation
            .as_ref()
            .is_none_or(|conversation| conversation.id != id)
        {
            self.conversation_scroll = ScrollHandle::new();
            self.conversation = Some(Conversation {
                id: id.clone(),
                page: ConversationPage::default(),
                request: None,
                failure: None,
            });
        }
        self.read_conversation(id, None, cx);
    }

    pub(super) fn read_conversation(
        &mut self,
        id: String,
        after: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let Some(conversation) = &mut self.conversation else {
            return;
        };
        let pending = self.source.scheduled_conversation(id.clone(), after);
        let request = Rc::new(());
        conversation.request = Some(request.clone());
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            drop(this.update(cx, |view, cx| {
                let Some(conversation) = view
                    .conversation
                    .as_mut()
                    .filter(|saved| saved.id == id && is_current(&saved.request, &request))
                else {
                    return;
                };
                conversation.request = None;
                conversation.failure = None;
                match result {
                    Ok(page) => conversation.apply(page, after),
                    Err(error) => conversation.failure = Some(say(&error.said)),
                }
                cx.notify();
            }));
        })
        .detach();
        cx.notify();
    }

    pub(super) fn history_view(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let history = &self.history;
        let rows = history.page.runs.iter().map(|run| {
            let detail = words::run_note(run);
            let state = words::state(&run.state);
            let when = run
                .started_at
                .as_ref()
                .or(run.queued_at.as_ref())
                .or(run.ended_at.as_ref());
            let title = when.map_or_else(
                || tr(Key("scheduled.run_label")).into(),
                |at| words::time(at),
            );
            let row = list_row(
                format!("scheduled-run-{}", run.id),
                title,
                detail,
                state,
                false,
                palette,
            );
            match run.conversation.clone() {
                Some(id) => super::kit::press_row(
                    row,
                    palette,
                    cx.listener(move |view, _, _, cx| view.open_run(id.clone(), cx)),
                )
                .into_any_element(),
                None => row.into_any_element(),
            }
        });
        let notice = notice(
            history.failure.as_ref(),
            history.page.problem.as_deref(),
            history.page.runs.is_empty(),
            history.request.is_some(),
            "scheduled.no_runs",
            palette,
        );
        let more = history
            .page
            .before
            .zip(self.selected.as_ref())
            .map(|(before, id)| {
                let (view, id) = (cx.weak_entity(), id.clone());
                controls::button(
                    "scheduled-older-runs",
                    tr(Key("scheduled.older_runs")),
                    Rc::new(move |_, cx| {
                        drop(view.update(cx, |view, cx| {
                            view.read_history(id.clone(), Some(before), cx)
                        }));
                    }),
                    cx,
                )
            });
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_SMALL)
            .child(
                text(theme::TEXT_BODY, palette.text)
                    .font_weight(theme::WEIGHT_MEDIUM)
                    .child(tr(Key("scheduled.history"))),
            )
            .children(notice)
            .children(rows)
            .children(more)
            .into_any_element()
    }

    pub(super) fn conversation_view(
        &self,
        conversation: &Conversation,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let view = cx.weak_entity();
        let back = controls::button(
            "scheduled-run-back",
            tr(Key("scheduled.back")),
            Rc::new(move |_, cx| {
                drop(view.update(cx, |view, cx| {
                    view.conversation = None;
                    view.refresh_detail(cx);
                    cx.notify();
                }));
            }),
            cx,
        );
        let notice = notice(
            conversation.failure.as_ref(),
            conversation.page.problem.as_deref(),
            conversation.page.entries.is_empty(),
            conversation.request.is_some(),
            "scheduled.no_entries",
            palette,
        );
        let entries = conversation
            .page
            .entries
            .iter()
            .map(|entry| thread::saved_message(entry, palette));
        let more = conversation.page.after.map(|after| {
            let (view, id) = (cx.weak_entity(), conversation.id.clone());
            controls::button(
                "scheduled-more-messages",
                tr(Key("scheduled.more_messages")),
                Rc::new(move |_, cx| {
                    drop(view.update(cx, |view, cx| {
                        view.read_conversation(id.clone(), Some(after), cx)
                    }));
                }),
                cx,
            )
        });
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_WIDE)
            .child(div().flex().child(back))
            .child(
                text(theme::settings::PAGE_TITLE, palette.text)
                    .child(tr(Key("scheduled.conversation"))),
            )
            .children(notice)
            .children(entries)
            .children(more)
            .into_any_element()
    }
}

impl History {
    fn apply(&mut self, page: HistoryPage, before: Option<u64>) {
        if page.problem.is_some() {
            self.page.problem = page.problem;
            return;
        }
        if before.is_some() {
            self.page.runs.extend(page.runs);
            self.page.before = page.before;
        } else if let Some(last) = page.runs.last() {
            let retained = self.page.runs.iter().position(|run| run.id == last.id);
            if let Some(index) = retained.filter(|&index| index + 1 < self.page.runs.len()) {
                let cursor = self.page.before;
                let earlier = self.page.runs.split_off(index + 1);
                self.page = page;
                self.page.runs.extend(earlier);
                self.page.before = cursor;
            } else {
                self.page = page;
            }
        } else {
            self.page = page;
        }
        self.page.problem = None;
    }
}

impl Conversation {
    fn apply(&mut self, page: ConversationPage, after: Option<u64>) {
        if page.problem.is_some() {
            self.page = page;
        } else if after.is_some() {
            self.page.entries.extend(page.entries);
            self.page.after = page.after;
            self.page.problem = None;
        } else if self.page.entries.last().map(|entry| entry.sequence)
            > page.entries.last().map(|entry| entry.sequence)
        {
            for (old, new) in self.page.entries.iter_mut().zip(page.entries) {
                *old = new;
            }
        } else {
            self.page = page;
        }
    }
}

fn is_current(active: &Option<Rc<()>>, request: &Rc<()>) -> bool {
    active
        .as_ref()
        .is_some_and(|active| Rc::ptr_eq(active, request))
}

fn notice(
    failure: Option<&SharedString>,
    problem: Option<&str>,
    is_empty: bool,
    is_loading: bool,
    empty: &'static str,
    palette: &Palette,
) -> Option<AnyElement> {
    let content = failure
        .cloned()
        .or_else(|| problem.map(|code| words::reason(code).into()))
        .or_else(|| {
            is_empty.then(|| {
                tr(Key(if is_loading {
                    "scheduled.loading"
                } else {
                    empty
                }))
                .into()
            })
        })?;
    let words = controls::quiet(content, palette);
    if problem.is_some() {
        Some(
            div()
                .debug_selector(|| "scheduled-read-problem".into())
                .child(words)
                .into_any_element(),
        )
    } else {
        Some(words)
    }
}
