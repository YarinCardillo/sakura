pub(super) mod form;
mod membership;
pub(super) use membership::{membership_control, Membership};

use self::form::{Field, FormSpec, Pick, Picker};
use super::engine_desk::Desk;
use super::sidebar_pages::go_from;
use crate::data::{engine_facts::EngineFacts, source::Source};
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette, Theme};
use crate::ui::settings_modal::controls;
use gpui::{
    div, AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, WeakEntity, Window,
};
use sakura_core::engine::cli::Act;
use sakura_core::engine::snapshot::{Action, Item, Snapshot};
use sakura_core::engine::tracks::{
    Track, TrackConversation, TrackDecision, TrackDecisionState, TrackLane, TrackLaneState,
    TrackStatus, Tracks, COLLECTION,
};
use sakura_core::tabs::{Page, TabId};
use std::collections::BTreeMap;
use std::rc::Rc;

pub(super) struct TrackPage {
    desk: WeakEntity<Desk>,
    tab: TabId,
    id: String,
    facts: Entity<EngineFacts>,
    source: Source,
    _observer: Subscription,
}

impl TrackPage {
    pub(super) fn new(
        desk: WeakEntity<Desk>,
        tab: TabId,
        facts: Entity<EngineFacts>,
        source: Source,
        id: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let observer = cx.observe(&facts, |_, _, cx| cx.notify());
        Self {
            desk,
            tab,
            id,
            facts,
            source,
            _observer: observer,
        }
    }

    fn body(&self, track: &Track, snapshot: &Snapshot, palette: &Palette, cx: &App) -> AnyElement {
        let projects = track.projects.iter().enumerate().map(|(index, project)| {
            let page = snapshot
                .projects
                .iter()
                .any(|candidate| candidate.folder == project.folder)
                .then(|| Page::Project(project.folder.clone()));
            let status = project
                .folder_missing
                .then(|| tr(super::lanes::FOLDER_MISSING));
            self.member_row(
                ("track-project", index),
                &project.name,
                status,
                page,
                palette,
            )
        });
        let lanes = track
            .lanes
            .iter()
            .enumerate()
            .map(|(index, lane)| self.lane_row(index, track, lane, snapshot, palette, cx));
        let conversations = track
            .conversations
            .iter()
            .enumerate()
            .map(|(index, member)| self.conversation_row(index, track, member, palette, cx));
        let decisions = track
            .decisions
            .iter()
            .map(|decision| self.decision(track, decision, palette, cx));
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_WIDE)
            .child(self.track_controls(track, snapshot, cx))
            .child(property(
                "track.status",
                tr(Key(status_word(track.status))),
                palette,
            ))
            .child(property("track.goal", &track.goal, palette))
            .children(
                (!track.summary.is_empty())
                    .then(|| property("track.summary", &track.summary, palette)),
            )
            .children(section("track.projects", projects, palette))
            .children(section("track.lanes", lanes, palette))
            .children(section("track.conversations", conversations, palette))
            .children(section("track.decisions", decisions, palette))
            .into_any_element()
    }

    fn track_controls(&self, track: &Track, snapshot: &Snapshot, cx: &App) -> AnyElement {
        let lifecycle: &[(&str, &str)] = match track.status {
            TrackStatus::Open => &[("track.pause", "paused"), ("track.close", "closed")],
            TrackStatus::Paused => &[("track.reopen", "open"), ("track.close", "closed")],
            TrackStatus::Closed => &[("track.reopen", "open")],
        };
        let mut buttons = Vec::new();
        if let Some(action) = track.action("trackset") {
            let mut edit = FormSpec::action("track.edit", request(action, track.target(), []));
            edit.fields = vec![
                field("name", "track.name", &track.name, None),
                field("goal", "track.goal", &track.goal, Some(3)),
                field("summary", "track.summary", &track.summary, Some(5)),
            ];
            buttons.push(self.form_button("track-edit", edit, cx));
            buttons.extend(lifecycle.iter().map(|&(label, status)| {
                self.form_button(
                    label,
                    FormSpec::action(
                        label,
                        request(action, track.target(), [("status".into(), status.into())]),
                    ),
                    cx,
                )
            }));
        }
        if let Some(action) = track.action("trackdecision") {
            let mut decision = FormSpec::action("track.raise", request(action, track.target(), []));
            decision.fields = vec![field("text", "track.decision", "", Some(3))];
            buttons.push(self.form_button("track-raise", decision, cx));
        }
        if let Some(link) = self.link_lane_form(track, snapshot) {
            buttons.push(self.form_button("track-link-lane", link, cx));
        }
        if snapshot
            .projects
            .iter()
            .any(|project| super::new_lane::can_start_on_track(project, &track.id))
        {
            let (desk, tab, id) = (self.desk.clone(), self.tab, track.id.clone());
            buttons.push(
                controls::button(
                    "track-new-lane",
                    tr(Key("sidebar.new_lane")),
                    Rc::new(move |window, cx| {
                        drop(desk.update(cx, |desk, cx| {
                            if desk.is_visible_page(tab, &Page::Track(id.clone()), cx) {
                                desk.open_track_lane(&id, window, cx);
                            }
                        }));
                    }),
                    cx,
                )
                .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_wrap()
            .gap(theme::GAP_SMALL)
            .children(buttons)
            .into_any_element()
    }

    fn lane_row(
        &self,
        index: usize,
        track: &Track,
        lane: &TrackLane,
        snapshot: &Snapshot,
        palette: &Palette,
        cx: &App,
    ) -> AnyElement {
        let item = lane.key.as_ref().and_then(|key| {
            snapshot
                .items
                .iter()
                .find(|item| item.key.as_slice() == key.as_slice())
        });
        let row = self.member_row(
            ("track-lane", index),
            &lane.title,
            Some(lane_status(lane, item)),
            item.map(|item| Page::Lane(item.lane_key())),
            palette,
        );
        let project = track
            .projects
            .iter()
            .find(|project| project.id == lane.project_id);
        let controls = project.into_iter().flat_map(|project| {
            let recovery = lane.startop.as_ref().into_iter().flat_map(move |startop| {
                lane.actions.iter().filter_map(move |action| {
                    self.recovery_form(track, action, startop, &project.folder)
                })
            });
            let unlink = (!lane.did_not_start)
                .then(|| self.unlink_form(track, lane_inputs(&project.folder, &lane.task)))
                .flatten();
            recovery.chain(unlink)
        });
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme::GAP_SMALL)
            .child(row)
            .children(controls.enumerate().map(|(action, form)| {
                self.form_button(format!("track-lane-{index}-action-{action}"), form, cx)
            }))
            .into_any_element()
    }

    fn recovery_form(
        &self,
        track: &Track,
        action: &Action,
        startop: &str,
        folder: &str,
    ) -> Option<FormSpec> {
        let label = match action.key.as_str() {
            "trackretry" => "track.retry",
            "trackremove" => "track.remove_reservation",
            _ => return None,
        };
        let mut form = FormSpec::action(
            label,
            request(action, track.target(), [("startop".into(), startop.into())]),
        );
        if action.key == "trackretry" {
            let (desk, tab, page, folder) = (
                self.desk.clone(),
                self.tab,
                Page::Track(self.id.clone()),
                folder.to_owned(),
            );
            form.outcome = Some(Rc::new(move |output, _, cx| {
                drop(desk.update(cx, |desk, cx| {
                    desk.open_started_lane(tab, page.clone(), &folder, output.trim(), cx)
                }));
            }));
        }
        Some(form)
    }

    fn link_lane_form(&self, track: &Track, snapshot: &Snapshot) -> Option<FormSpec> {
        let action = track.action("tracklink")?;
        let options: Vec<_> = snapshot
            .items
            .iter()
            .filter(|item| {
                can_link_lane(item, snapshot)
                    && item
                        .track
                        .as_ref()
                        .is_none_or(|linked| linked.id != track.id)
            })
            .map(|item| Pick {
                label: safe_title(&item.title, "track.member"),
                inputs: lane_inputs(&item.repo, item.task()),
            })
            .collect();
        if options.is_empty() {
            return None;
        }
        let mut form = FormSpec::action("track.link_lane", request(action, track.target(), []));
        form.picker = Some(Picker {
            label: "track.lanes",
            options,
            allows_empty: false,
        });
        Some(form)
    }

    fn unlink_form(&self, track: &Track, inputs: BTreeMap<String, String>) -> Option<FormSpec> {
        let action = track.action("trackunlink")?;
        Some(FormSpec::action(
            "track.unlink",
            request(action, track.target(), inputs),
        ))
    }

    fn conversation_row(
        &self,
        index: usize,
        track: &Track,
        member: &TrackConversation,
        palette: &Palette,
        cx: &App,
    ) -> AnyElement {
        let can_open = self
            .desk
            .upgrade()
            .is_some_and(|desk| desk.read(cx).can_open_track_conversation(&member.key, cx));
        let title = safe_title(&member.title, "track.conversation");
        let row = if can_open {
            let (desk, tab, key) = (self.desk.clone(), self.tab, member.key.clone());
            super::kit::button(
                format!("track-conversation-{index}"),
                title,
                super::kit::ButtonKind::Quiet,
                palette,
            )
            .min_w_0()
            .truncate()
            .on_click(move |event, window, cx| {
                drop(desk.update(cx, |desk, cx| {
                    desk.open_track_conversation(tab, &key, event.modifiers().platform, window, cx)
                }));
            })
            .into_any_element()
        } else {
            self.member_row(
                ("track-conversation", index),
                &title,
                Some(tr(Key("track.conversation_unavailable"))),
                None,
                palette,
            )
        };
        let unlink = self.unlink_form(track, conversation_inputs(&member.key));
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme::GAP_SMALL)
            .child(row)
            .children(unlink.map(|form| {
                self.form_button(format!("track-conversation-unlink-{index}"), form, cx)
            }))
            .into_any_element()
    }

    fn decision(
        &self,
        track: &Track,
        decision: &TrackDecision,
        palette: &Palette,
        cx: &App,
    ) -> AnyElement {
        let answer = (decision.state == TrackDecisionState::Open)
            .then(|| track.action("tracksettle"))
            .flatten()
            .map(|action| {
                let mut form = FormSpec::action(
                    "track.answer",
                    request(
                        action,
                        track.target(),
                        [("decision".into(), decision.id.to_string())],
                    ),
                );
                form.fields = vec![field("answer", "track.answer", "", Some(4))];
                self.form_button(format!("track-answer-{}", decision.id), form, cx)
            });
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_SMALL)
            .min_w_0()
            .child(property(
                if decision.state == TrackDecisionState::Open {
                    "track.decision.open"
                } else {
                    "track.decision.settled"
                },
                &decision.text,
                palette,
            ))
            .children(
                decision
                    .answer
                    .as_ref()
                    .map(|answer| property("track.answer", answer, palette)),
            )
            .children(answer.map(|answer| div().flex().child(answer)))
            .into_any_element()
    }

    fn member_row(
        &self,
        (kind, index): (&'static str, usize),
        title: &str,
        status: Option<&str>,
        page: Option<Page>,
        palette: &Palette,
    ) -> AnyElement {
        let title = safe_title(title, "track.member");
        let row = div().flex().items_center().min_w_0().gap(theme::GAP_SMALL);
        let row = if let Some(page) = page {
            let from = Page::Track(self.id.clone());
            row.child(
                super::kit::button(
                    format!("{kind}-{index}"),
                    title,
                    super::kit::ButtonKind::Quiet,
                    palette,
                )
                .min_w_0()
                .max_w_full()
                .truncate()
                .on_click(go_from(
                    self.desk.clone(),
                    self.tab,
                    from.clone(),
                    page.clone(),
                    false,
                ))
                .on_aux_click(go_from(
                    self.desk.clone(),
                    self.tab,
                    from,
                    page,
                    true,
                )),
            )
        } else {
            row.child(div().min_w_0().truncate().child(title))
        };
        row.children(status.map(|status| {
            div()
                .debug_selector({
                    let status = status.to_owned();
                    move || format!("{kind}-{index}-status-{status}")
                })
                .flex_none()
                .text_size(theme::TEXT_LABEL)
                .text_color(palette.text_secondary)
                .child(status.to_string())
        }))
        .into_any_element()
    }

    fn form_button(&self, id: impl Into<SharedString>, spec: FormSpec, cx: &App) -> AnyElement {
        let (facts, source) = (self.facts.clone(), self.source.clone());
        let label = tr(Key(spec.heading));
        let form = Rc::new(spec);
        controls::button(
            id,
            label,
            Rc::new(move |window, cx| {
                form::open((*form).clone(), facts.clone(), source.clone(), window, cx);
            }),
            cx,
        )
        .into_any_element()
    }
}

impl Render for TrackPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let facts = self.facts.read(cx);
        let body = match facts
            .snapshot()
            .map(|snapshot| (&snapshot.tracks, snapshot))
        {
            Some((Tracks::Unavailable(problem), _)) => message(problem.clone(), &palette),
            Some((Tracks::Available { .. }, snapshot)) => snapshot.track(&self.id).map_or_else(
                || message(tr(Key("track.unavailable")), &palette),
                |track| self.body(track, snapshot, &palette, cx),
            ),
            Some((Tracks::Unsupported, _)) => message(tr(Key("track.unsupported")), &palette),
            None => message(tr(Key("track.unavailable")), &palette),
        };
        div()
            .id("track-page")
            .debug_selector(|| "track-page".into())
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .py(theme::PANEL_INSET)
            .px(theme::CONTENT_PADDING)
            .child(body)
    }
}

pub(super) fn can_link_lane(item: &Item, snapshot: &Snapshot) -> bool {
    item.kind == "flow"
        && item.agent_state != "cleaned"
        && !snapshot.is_project_folder_missing(&item.repo)
}

fn lane_status(lane: &TrackLane, item: Option<&Item>) -> &'static str {
    if lane.folder_missing {
        return tr(super::lanes::FOLDER_MISSING);
    }
    let agent_state = item.map(|item| item.agent_state.as_str());
    tr(Key(match lane.state {
        TrackLaneState::Live | TrackLaneState::Finished if agent_state == Some("cleaned") => {
            "lane.state.cleaned"
        }
        TrackLaneState::Live | TrackLaneState::Finished if agent_state == Some("stopped") => {
            "lane.state.stopped"
        }
        TrackLaneState::Live => "track.lane.live",
        TrackLaneState::Finished => "track.lane.finished",
        TrackLaneState::Unreadable => "track.lane.unavailable",
        TrackLaneState::Gone => "track.lane.gone",
        TrackLaneState::Pending => "track.lane.pending",
        TrackLaneState::Interrupted => "track.lane.interrupted",
    }))
}

impl Desk {
    pub(super) fn open_new_track(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.facts.read(cx).snapshot() else {
            return;
        };
        let Some(action) = snapshot.tracks.action("tracknew") else {
            return;
        };
        let mut spec = FormSpec::action("track.new", request(action, COLLECTION.into(), []));
        spec.fields = vec![
            field("name", "track.name", "", None),
            field("goal", "track.goal", "", Some(3)),
        ];
        if action.optional.iter().any(|input| input == "project") {
            let choices: Vec<_> = snapshot
                .projects
                .iter()
                .filter(|project| !project.folder_missing)
                .map(|project| Pick {
                    label: project.name.clone(),
                    inputs: BTreeMap::from([("project".into(), project.folder.clone())]),
                })
                .collect();
            if !choices.is_empty() {
                spec.picker = Some(Picker {
                    label: "track.initial_project",
                    options: choices,
                    allows_empty: true,
                });
            }
        }
        let (desk, tab) = (cx.weak_entity(), self.bar.read(cx).tabs().active().id());
        spec.outcome = Some(Rc::new(move |output, _, cx| {
            let id = serde_json::from_str::<serde_json::Value>(output)
                .ok()
                .and_then(|receipt| receipt.get("id")?.as_str().map(str::to_owned));
            if let Some(id) = id {
                drop(desk.update(cx, |desk, cx| {
                    desk.navigate_from_tab(tab, Page::Track(id), cx)
                }));
            }
        }));
        form::open(spec, self.facts.clone(), self.source.clone(), window, cx);
    }
}

#[derive(Default)]
pub(super) struct Operation(Option<Act>);

impl Operation {
    pub(super) fn prepare(&mut self, mut draft: Act) -> Act {
        let op = self
            .0
            .as_ref()
            .filter(|last| {
                last.target == draft.target
                    && last.action.key == draft.action.key
                    && last
                        .inputs
                        .iter()
                        .filter(|(name, _)| name.as_str() != "op")
                        .eq(draft.inputs.iter())
            })
            .and_then(|last| last.inputs.get("op"))
            .cloned()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        draft.inputs.insert("op".into(), op);
        self.0 = Some(draft.clone());
        draft
    }
}

pub(super) fn request(
    action: &Action,
    target: String,
    inputs: impl IntoIterator<Item = (String, String)>,
) -> Act {
    Act {
        action: action.clone(),
        target,
        inputs: inputs.into_iter().collect(),
        confirm: None,
    }
}

pub(super) fn lane_inputs(folder: &str, task: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("project".into(), folder.into()),
        ("task".into(), task.into()),
    ])
}

pub(super) fn conversation_inputs(
    key: &sakura_core::engine::tracks::ConversationKey,
) -> BTreeMap<String, String> {
    BTreeMap::from([("conversation".into(), serde_json::json!(key).to_string())])
}

fn field(input: &'static str, label: &'static str, value: &str, rows: Option<usize>) -> Field {
    Field {
        input,
        label,
        value: value.into(),
        rows,
    }
}

pub(super) fn name(track: &Track) -> String {
    safe_title(&track.name, "track.one")
}

fn safe_title(title: &str, fallback: &'static str) -> String {
    if title.trim().is_empty() {
        tr(Key(fallback)).to_string()
    } else {
        title.into()
    }
}

fn status_word(status: TrackStatus) -> &'static str {
    match status {
        TrackStatus::Open => "track.open",
        TrackStatus::Paused => "track.paused",
        TrackStatus::Closed => "track.closed",
    }
}

fn property(label: &'static str, value: &str, palette: &Palette) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(theme::GAP_SMALL)
        .child(
            div()
                .text_size(theme::TEXT_LABEL)
                .text_color(palette.text_secondary)
                .child(tr(Key(label))),
        )
        .child(div().min_w_0().whitespace_normal().child(value.to_string()))
        .into_any_element()
}

fn section(
    label: &'static str,
    rows: impl Iterator<Item = AnyElement>,
    palette: &Palette,
) -> Option<AnyElement> {
    let rows: Vec<_> = rows.collect();
    (!rows.is_empty()).then(|| {
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(
                div()
                    .text_size(theme::TEXT_LABEL)
                    .text_color(palette.text_secondary)
                    .child(tr(Key(label))),
            )
            .children(rows)
            .into_any_element()
    })
}

fn message(words: impl Into<SharedString>, palette: &Palette) -> AnyElement {
    div()
        .min_w_0()
        .whitespace_normal()
        .text_color(palette.text_secondary)
        .child(words.into())
        .into_any_element()
}

#[cfg(test)]
#[path = "track_page_tests.rs"]
mod tests;
