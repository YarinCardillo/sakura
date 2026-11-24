//! The ask selected, at the right of the list: its title, where it comes from, what it says rendered as
//! markdown, what the lane is doing about it, and one row of actions at the bottom. The note field stands
//! over the row, only after a decline or a reply. A decided ask shows the answer given instead of actions.

use super::decide::NoteFor;
use super::face::{action, Face, State};
use super::list::decided_title;
use super::model::{self, Verb, Verbs};
use super::{InboxEvent, InboxPage, Shown};
use crate::l10n::{tr, tr_with, Key};
use crate::theme::{self, inbox as sizes, Palette};
use crate::ui::answers::{lane_key, step_view, Step};
use crate::ui::chrome::{tabular, text};
use crate::ui::enter::is_composing;
use crate::ui::inert_markdown::{inert, open};
use crate::ui::kit::progress_line;
use crate::ui::lanes;
use crate::ui::tip::{self, Words};
use crate::ui::titled_links::titled_links;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::text::{TextView, TextViewState};
use sakura_core::engine::resume_note::{self, Problem, CHARACTER_LIMIT};
use sakura_core::engine::snapshot::{Item, Snapshot, Wait};

/// The gate where a lane waits for its spec to be read and approved.
const SPEC_GATE: &str = "spec";
const KIND_APPROVAL: &str = "approval";
const RESUME_NOTE_COUNT_FROM: usize = 1800;

/// Whether `wait` asks the person to read and approve a lane's spec.
pub(crate) fn is_spec_approval(wait: &Wait) -> bool {
    wait.kind == KIND_APPROVAL && wait.key_name() == SPEC_GATE
}

/// What an ask is called: the engine's words for it, or for a spec's approval a sentence that asks to read it.
pub(super) fn ask_title(wait: &Wait, snapshot: &Snapshot) -> String {
    if is_spec_approval(wait) {
        let lane = model::lane_of(wait, snapshot).map(|item| lanes::lane_words(item, false));
        let title = lane.as_deref().unwrap_or_else(|| {
            if wait.lane_title.trim().is_empty() {
                tr(Key("lane.untitled"))
            } else {
                &wait.lane_title
            }
        });
        return spec_ask_title(title);
    }
    if wait.kind == model::KIND_BLOCKED && wait.title.trim().eq_ignore_ascii_case("blocked") {
        return model::lane_of(wait, snapshot).map_or_else(
            || wait.lane_name().to_string(),
            |item| lanes::lane_words(item, false),
        );
    }
    model::ask_title_words(wait)
}

pub(super) fn spec_ask_title(lane: &str) -> String {
    tr_with(Key("waiting.spec_ask"), &[("task", lane)]).to_string()
}

impl InboxPage {
    /// The selected ask, or the sentence that says there is none.
    pub(super) fn detail(
        &mut self,
        snapshot: &Snapshot,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.shown {
            Shown::Open => match self.selected_ask(snapshot) {
                Some(wait) => self.open_detail(wait, snapshot, palette, cx),
                None => div().into_any_element(),
            },
            Shown::Done => self.done_detail(snapshot, palette, cx),
        }
    }

    fn open_detail(
        &mut self,
        wait: &Wait,
        snapshot: &Snapshot,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lane = model::lane_of(wait, snapshot);
        let body = self.body_of(wait, &wait.id, palette, cx);
        let is_missing = snapshot.is_project_folder_missing(&wait.repo);
        let findings = crate::ui::ask_findings::findings_view(
            "ask-findings",
            &wait.id,
            &wait.findings,
            palette,
        )
        .map(|findings| {
            div()
                .max_w(sizes::BODY_MAX_WIDTH)
                .mt(theme::GAP_WIDE)
                .child(findings)
        });
        let source = source_line(wait, palette);
        let project_name = snapshot
            .projects
            .iter()
            .find(|project| project.folder == wait.repo)
            .map_or(wait.project.as_str(), |project| project.name.as_str());
        let properties = Properties {
            kind: model::inbox_kind_words(model::wait_kind(wait)),
            state: State::of_wait(wait),
            snoozed: self.filter.snoozed.then(|| model::snoozed_status(wait)),
            lane,
            lane_name: if wait.task == "master" && wait.lane_title.is_empty() {
                tr(Key("page.master"))
            } else {
                wait.lane_name()
            },
            location: wait
                .track
                .as_ref()
                .map_or(("inbox.filter.project", project_name), |track| {
                    ("inbox.property.track", track.name.as_str())
                }),
            since: wait.since.as_deref(),
            resolved_by: None,
        };
        let properties = properties.render(palette, cx);
        let actions = if self.filter.snoozed {
            self.snoozed_actions(wait, palette, cx)
        } else {
            self.actions(wait, snapshot, palette, cx)
        };
        let links = titled_links("ask-links", &wait.links, palette, cx).map(|links| {
            div()
                .max_w(sizes::BODY_MAX_WIDTH)
                .mt(theme::GAP_WIDE)
                .id("inbox-open-link")
                .on_click(cx.listener(|page, _, _, cx| page.mark_opened(cx)))
                .child(links)
        });
        // An outside agent's lane has no phase to report: what its asks are is all the page says.
        let context = lane
            .filter(|lane| !lane.is_outside())
            .and_then(|lane| context(lane, palette));
        let content = div()
            .child(title(&ask_title(wait, snapshot), &wait.id))
            .children(source)
            .children(self.scheduled_run_link(wait, snapshot, cx))
            .children(is_missing.then(|| {
                text(theme::TEXT_SMALL, palette.text_secondary)
                    .debug_selector(|| "inbox-folder-missing".into())
                    .mb(theme::GAP)
                    .child(tr(Key(sakura_core::engine::cli::PROJECT_FOLDER_MISSING)))
            }))
            .children(self.resume_note_row(&wait.id, is_missing, palette, cx))
            .children(
                self.filter
                    .snoozed
                    .then(|| model::wake_words(wait))
                    .flatten()
                    .map(|words| {
                        text(theme::TEXT_SMALL, palette.text_secondary)
                            .debug_selector(|| "inbox-snooze-wake".into())
                            .mb(theme::GAP)
                            .child(words)
                    }),
            )
            .children(body)
            .children(
                (snapshot.connectors.is_some() && model::needs_github(wait))
                    .then(|| self.github_settings(palette, cx)),
            )
            .children(findings)
            .children(links)
            .children(context);
        pane(
            columns(content, properties),
            actions,
            &self.detail_scroll,
            palette,
        )
    }

    fn scheduled_run_link(
        &self,
        wait: &Wait,
        snapshot: &Snapshot,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        snapshot.scheduled.as_ref()?;
        let run = wait.proposal.as_ref()?.run_id.clone()?;
        let page = cx.weak_entity();
        Some(
            crate::ui::settings_modal::controls::button(
                "inbox-scheduled-run",
                tr(Key("scheduled.open_run")),
                std::rc::Rc::new(move |_, cx| {
                    drop(page.update(cx, |_, cx| {
                        cx.emit(InboxEvent::OpenScheduledRun(run.clone()))
                    }));
                }),
                cx,
            )
            .into_any_element(),
        )
    }

    fn github_settings(&self, palette: &Palette, cx: &App) -> AnyElement {
        use crate::ui::settings_modal::{self, Section};

        let source = self.source.clone();
        let facts = self.facts.clone();
        let id = "inbox-connect-github";
        let open = action(
            id,
            tr(Key("inbox.github_settings")).into(),
            None,
            Face::Plain,
            false,
            palette,
            cx,
        )
        .debug_selector(move || id.into())
        .on_click(move |_, window, cx| {
            if let Some(modal) = settings_modal::open(
                source.clone(),
                facts.clone(),
                Section::Connectors,
                window,
                cx,
            ) {
                modal.update(cx, |modal, cx| modal.show_connector("github", cx));
            }
        });
        div().flex().mt(theme::GAP).child(open).into_any_element()
    }

    /// The text of the ask as markdown, inert since an agent wrote it; kept while the text is the same.
    fn body_of(
        &mut self,
        wait: &Wait,
        id: &str,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let plain = model::plain_reason(wait);
        let body = model::plain_text(&plain.words);
        if let Some((_, commands)) = model::checks_commands(wait, &body) {
            let summary =
                sakura_core::said::Said::new("inbox.checks_summary").count(commands.len() as u64);
            return Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::GAP)
                    .child(
                        text(sizes::BODY_TEXT, palette.text)
                            .debug_selector(|| "inbox-checks-summary".into())
                            .child(crate::l10n::say(&summary)),
                    )
                    .children(commands.into_iter().map(|command| {
                        let selector = format!("inbox-check-command-{command}");
                        div()
                            .debug_selector(move || selector.clone())
                            .text_size(sizes::BODY_TEXT)
                            .child(crate::ui::path_text::plain_with_paths(
                                command,
                                palette.text,
                                palette.text_secondary,
                            ))
                    }))
                    .into_any_element(),
            );
        }
        let text = if model::is_engine_text(wait)
            && crate::ui::plain_error::shows_command_or_path(&body)
        {
            tr(Key("error.plain.generic"))
        } else {
            body.as_str()
        };
        if text.is_empty() || text == wait.title {
            return None;
        }
        let is_plain = model::is_engine_text(wait) && !has_planner_bold(text);
        let check = (wait.kind == "owner_row" && text.contains("\nExpected:"))
            .then(|| text.replace("\nExpected:", "\n\nExpected:"));
        let text = check.as_deref().unwrap_or(text);
        let block = match is_plain {
            true => {
                self.plain_bodies.insert(id.to_string(), text.to_string());
                words_block(text, id, palette)
            }
            false => self.markdown_body(text, id, is_spec_approval(wait), palette, cx),
        };
        let details = plain.raw.as_deref().map(|raw| {
            crate::ui::plain_error::details_view("ask-details", id, raw, &self.answers, palette, cx)
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap(theme::GAP)
                .child(block)
                .children(details)
                .into_any_element(),
        )
    }

    fn markdown_body(
        &mut self,
        body: &str,
        id: &str,
        is_spec: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self
            .bodies
            .get(id)
            .filter(|(shown, _)| shown == body)
            .map(|(_, state)| state.clone())
            .unwrap_or_else(|| self.read_markdown(id, body, is_spec, cx));
        let code = div()
            .font_family(theme::FONT_MONO)
            .px(theme::GAP_TIGHT)
            .rounded_sm()
            .bg(palette.raised)
            .style()
            .clone();
        let reading = TextView::new(&state)
            .inline_code_style(code)
            .list_style(
                gpui::StyleRefinement::default()
                    .w(theme::CONTROL_HEIGHT)
                    .text_right(),
            )
            .selectable(true)
            .on_link_click(|address, _, _, cx| open(address, cx));
        markdown_block(reading, id)
    }

    fn read_markdown(
        &mut self,
        id: &str,
        text: &str,
        is_spec: bool,
        cx: &mut Context<Self>,
    ) -> Entity<TextViewState> {
        let shown = if is_spec {
            crate::ui::inert_markdown::spec(text)
        } else {
            inert(text)
        };
        let state = cx.new(|cx| TextViewState::markdown(&shown, cx));
        self.bodies
            .insert(id.to_string(), (text.to_string(), state.clone()));
        state
    }

    fn done_detail(
        &mut self,
        snapshot: &Snapshot,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected.as_deref();
        let decided = snapshot.decided.iter().flatten();
        let Some(decided) = decided
            .into_iter()
            .find(|d| Some(model::decided_key(d).as_str()) == selected)
        else {
            return div().into_any_element();
        };
        let lane = model::decided_lane(decided, snapshot);
        let age = model::clock_words(decided.at.as_deref(), super::list::now());
        let said = [Some(model::answer_words(decided)), age]
            .into_iter()
            .flatten();
        let said = said
            .map(|part| part.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let original = self.facts.read(cx).decided_wait(decided);
        let title_words = decided_title(decided, snapshot);
        let id = model::decided_key(decided);
        let is_track = matches!(decided.key.as_slice(), [_, source, _, kind, _] if source == "track" && kind == "track_decision");
        let reply = decided
            .text
            .as_deref()
            .filter(|body| decided.answer == "reply" && !body.trim().is_empty());
        let content = div()
            .child(title(&title_words, &id))
            .children(
                original
                    .as_deref()
                    .and_then(|wait| self.body_of(wait, &id, palette, cx)),
            )
            .children(reply.map(|reply| reply_block(reply, is_track, palette)));
        let row = done_row(lane.is_some(), said, palette, cx);
        let properties = Properties {
            kind: model::inbox_kind_words(model::history_kind(decided)),
            state: State::Done,
            snoozed: None,
            lane,
            lane_name: "",
            location: (
                if is_track {
                    "inbox.property.track"
                } else {
                    "inbox.filter.project"
                },
                &decided.project,
            ),
            since: original.as_deref().and_then(|wait| wait.since.as_deref()),
            resolved_by: model::resolved_by(decided),
        };
        pane(
            columns(content, properties.render(palette, cx)),
            row,
            &self.detail_scroll,
            palette,
        )
    }
}

fn has_planner_bold(source: &str) -> bool {
    use markdown::mdast::Node;

    fn has_strong(node: &Node, source: &str) -> bool {
        matches!(node, Node::Strong(strong) if strong.position.as_ref().is_some_and(|position| {
            source[position.start.offset..].starts_with("**")
        })) || node
            .children()
            .into_iter()
            .flatten()
            .any(|child| has_strong(child, source))
    }

    source.contains("**")
        && markdown::to_mdast(source, &markdown::ParseOptions::gfm())
            .is_ok_and(|root| has_strong(&root, source))
}

fn title(words: &str, id: &str) -> AnyElement {
    let selector = format!("title-{id}");
    div()
        .debug_selector(move || selector)
        .max_w(sizes::TITLE_MAX_WIDTH)
        .min_w_0()
        .overflow_hidden()
        .mb(theme::GAP)
        .text_size(sizes::TITLE_TEXT)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(words.to_string())
        .into_any_element()
}

fn source_line(wait: &Wait, palette: &Palette) -> Option<AnyElement> {
    let from = model::from_words(wait).filter(|_| !wait.is_digest())?;
    let selector = format!("from-{}", wait.id);
    Some(
        text(theme::TEXT_LABEL, palette.text_tertiary)
            .debug_selector(move || selector)
            .mb(theme::GAP)
            .child(from)
            .into_any_element(),
    )
}

struct Properties<'a> {
    kind: SharedString,
    state: State,
    snoozed: Option<SharedString>,
    lane: Option<&'a Item>,
    lane_name: &'a str,
    location: (&'static str, &'a str),
    since: Option<&'a str>,
    resolved_by: Option<SharedString>,
}

impl Properties<'_> {
    fn render(self, palette: &Palette, cx: &mut Context<InboxPage>) -> AnyElement {
        let state = div()
            .debug_selector(|| "inbox-property-state".into())
            .flex()
            .gap(theme::GAP_SMALL)
            .items_center()
            .child(div().flex_none().child(self.state.glyph(palette)))
            .child(match self.snoozed {
                Some(words) => div()
                    .debug_selector(|| "inbox-property-snoozed".into())
                    .flex_1()
                    .min_w_0()
                    .whitespace_normal()
                    .child(words),
                None => div().child(self.state.label()),
            });
        let lane = self.lane.map(|lane| lane_link(lane, palette, cx));
        let lane = lane.or_else(|| property_text(self.lane_name));
        let since = model::clock_words(self.since, super::list::now()).map(|words| {
            div()
                .debug_selector(|| "inbox-property-since".into())
                .child(words)
        });
        div()
            .debug_selector(|| "inbox-properties".into())
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .flex_1()
            .min_w_0()
            .flex_basis(sizes::PROPERTIES_WIDTH)
            .text_size(theme::TEXT_LABEL)
            .child(
                div()
                    .font_weight(theme::WEIGHT_MEDIUM)
                    .child(tr(Key("inbox.properties"))),
            )
            .child(property("inbox.property.status", state, palette))
            .child(property(
                "inbox.filter.kind",
                div().child(self.kind),
                palette,
            ))
            .children(lane.map(|value| property("inbox.property.lane", value, palette)))
            .children(
                property_text(self.location.1)
                    .map(|value| property(self.location.0, value, palette)),
            )
            .children(since.map(|value| property("inbox.property.since", value, palette)))
            .children(self.resolved_by.map(|value| {
                property(
                    "inbox.property.resolved_by",
                    div()
                        .debug_selector(|| "inbox-resolved-by".into())
                        .child(value),
                    palette,
                )
            }))
            .into_any_element()
    }
}

fn property_text(value: &str) -> Option<AnyElement> {
    (!value.is_empty()).then(|| div().child(value.to_string()).into_any_element())
}

pub(in crate::ui) fn property(
    label: &'static str,
    value: impl IntoElement,
    palette: &Palette,
) -> AnyElement {
    property_with_label_width(label, value, sizes::PROPERTIES_WIDTH * 0.35, palette)
}

pub(in crate::ui) fn property_with_label_width(
    label: &'static str,
    value: impl IntoElement,
    label_width: gpui::Pixels,
    palette: &Palette,
) -> AnyElement {
    div()
        .debug_selector(move || label.into())
        .w_full()
        .flex()
        .items_start()
        .gap(theme::GAP)
        .child(
            div()
                .w(label_width)
                .flex_none()
                .min_w_0()
                .text_color(palette.text_tertiary)
                .child(tr(Key(label))),
        )
        .child(
            div()
                .debug_selector(move || format!("{label}-value"))
                .min_w_0()
                .flex_1()
                .child(value),
        )
        .into_any_element()
}

fn columns(content: gpui::Div, properties: AnyElement) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_wrap()
        .gap(theme::GAP_WIDE)
        .child(
            content
                .flex_1()
                .flex_basis(sizes::DETAIL_COLUMN_WIDTH)
                .min_w_0(),
        )
        .child(properties)
}

/// The lane's name as a link: a press opens the lane.
fn lane_link(lane: &Item, palette: &Palette, cx: &mut Context<InboxPage>) -> AnyElement {
    let key = lane.lane_key();
    let name: SharedString = lanes::lane_words(lane, false).into();
    let tooltip = name.clone();
    div()
        .id("source-lane")
        .debug_selector(|| "source-lane".into())
        .text_color(palette.text)
        .whitespace_normal()
        .cursor_pointer()
        .tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
        })
        .child(name)
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(InboxEvent::OpenLane(key.clone()))))
        .into_any_element()
}

/// Plain body copy for an engine-authored ask.
fn words_block(words: &str, id: &str, palette: &Palette) -> AnyElement {
    let selector = format!("body-{id}");
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector)
        .w_full()
        .max_w(sizes::BODY_MAX_WIDTH)
        .overflow_x_scroll()
        .text_size(sizes::BODY_TEXT)
        .child(crate::ui::path_text::plain_with_paths(
            &crate::ui::thread::shown(words),
            palette.text,
            palette.text_secondary,
        ))
        .into_any_element()
}

fn markdown_block(reading: TextView, id: &str) -> AnyElement {
    let selector = format!("body-{id}");
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector)
        .w_full()
        .max_w(sizes::BODY_MAX_WIDTH)
        .overflow_x_scroll()
        .text_size(sizes::BODY_TEXT)
        .child(reading)
        .into_any_element()
}

fn reply_block(reply: &str, is_track: bool, palette: &Palette) -> AnyElement {
    let label = if is_track {
        "inbox.note.track_answer"
    } else {
        "inbox.note.reply"
    };
    div()
        .max_w(sizes::BODY_MAX_WIDTH)
        .child(
            text(theme::TEXT_LABEL, palette.text_secondary)
                .debug_selector(move || label.into())
                .child(tr(Key(label))),
        )
        .child(
            text(sizes::BODY_TEXT, palette.text)
                .debug_selector(|| "inbox-note-text".into())
                .child(reply.to_owned()),
        )
        .into_any_element()
}

/// What the lane is doing: its phase and how far, with the progress line at the right. A lane that has nothing
/// to count has only a phase word to say, which the ask above already makes clear: no panel for it.
fn context(lane: &Item, palette: &Palette) -> Option<AnyElement> {
    let progress = lane
        .progress
        .as_ref()
        .filter(|progress| progress.total > 0)?;
    let phase = lanes::phase_word(progress.phase_id());
    let words = tr_with(
        Key("inbox.context"),
        &[
            ("phase", &phase),
            ("progress", &lanes::done_words(progress)),
        ],
    )
    .to_string();
    let line = progress_line(progress.done, progress.total, sizes::CONTEXT_BAR_WIDTH);
    let block = div()
        .debug_selector(|| "inbox-context".into())
        .max_w(sizes::BODY_MAX_WIDTH)
        .mt(sizes::CONTEXT_GAP_ABOVE)
        .px(sizes::CONTEXT_PADDING_X)
        .py(sizes::CONTEXT_PADDING_Y)
        .rounded(sizes::CONTEXT_RADIUS)
        .bg(palette.fill)
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_secondary)
        .child(div().font_features(tabular()).child(words))
        .child(div().flex_1())
        .child(line);
    Some(block.into_any_element())
}

/// The scrolling part over the fixed row of actions.
fn pane(
    content: impl IntoElement,
    actions: AnyElement,
    scroll: &gpui::ScrollHandle,
    palette: &Palette,
) -> AnyElement {
    let scrolling = div()
        .id("inbox-detail-scroll")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(scroll)
        .px(sizes::DETAIL_PADDING_X)
        .pt(sizes::DETAIL_PADDING_TOP)
        .pb(sizes::DETAIL_PADDING_BOTTOM)
        .child(content);
    let actions = div()
        .debug_selector(|| "inbox-actions".into())
        .flex_none()
        .child(crate::ui::split::footer_line(palette))
        .child(
            div()
                .px(sizes::DETAIL_PADDING_X)
                .pt(sizes::ACTIONS_PADDING_TOP)
                .pb(sizes::ACTIONS_PADDING_BOTTOM)
                .flex()
                .flex_col()
                .gap(theme::GAP_WIDE)
                .child(actions),
        );
    div()
        .debug_selector(|| "inbox-detail".into())
        .size_full()
        .flex()
        .flex_col()
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(scrolling)
                .child(crate::ui::page_scroll::end_fade(scroll, palette.panel)),
        )
        .child(actions)
        .into_any_element()
}

/// The row under a decided ask: the answer and when, and the way to the lane.
fn done_row(
    has_lane: bool,
    said: String,
    palette: &Palette,
    cx: &mut Context<InboxPage>,
) -> AnyElement {
    let open = has_lane.then(|| open_lane_button(true, palette, cx));
    div()
        .min_h(theme::CONTROL_HEIGHT)
        .flex()
        .items_center()
        .gap(sizes::ACTIONS_GAP)
        .child(
            text(theme::TEXT_LABEL, palette.text_tertiary)
                .debug_selector(|| "inbox-decided".into())
                .child(said),
        )
        .child(div().flex_1())
        .children(open)
        .into_any_element()
}

fn open_lane_button(
    has_shortcut: bool,
    palette: &Palette,
    cx: &mut Context<InboxPage>,
) -> AnyElement {
    let (label, key) = (tr(Key("inbox.open_lane")), tr(Key("inbox.key.return")));
    action(
        "open-lane",
        label.into(),
        has_shortcut.then(|| key.into()),
        Face::Quiet,
        false,
        palette,
        cx,
    )
    .on_click(cx.listener(|this, _, window, cx| {
        this.open_selected_lane(cx);
        window.focus(&this.focus, cx);
    }))
    .into_any_element()
}

// The row of actions of an open ask.
impl InboxPage {
    fn snoozed_actions(
        &self,
        wait: &Wait,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use super::snooze::{is_engine_snooze, UNSNOOZE};
        if !is_engine_snooze(wait) {
            return div().into_any_element();
        }
        let key = lane_key(&wait.id, "attention");
        let is_running = self.answers.read(cx).is_running(&key);
        let button = super::face::action(
            "inbox-unsnooze",
            tr(Key("inbox.unsnooze")).into(),
            None,
            super::face::Face::Plain,
            is_running,
            palette,
            cx,
        )
        .when(!is_running, |button| {
            button.on_click(cx.listener(|page, _, _, cx| page.attention(UNSNOOZE, None, cx)))
        });
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_WIDE)
            .children(self.steps_of(wait, None, palette, cx))
            .child(
                div()
                    .flex()
                    .gap(theme::GAP)
                    .children(attention_pending(is_running, palette))
                    .child(button),
            )
            .into_any_element()
    }

    fn actions(
        &mut self,
        wait: &Wait,
        snapshot: &Snapshot,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let verbs = Self::verbs_of(wait, snapshot);
        let lane = model::lane_of(wait, snapshot);
        let is_running =
            self.answers.read(cx).is_running(&wait.id) || self.is_answered(&wait.id, cx);
        let is_missing = snapshot.is_project_folder_missing(&wait.repo);
        let steps = self.steps_of(wait, lane, palette, cx);
        let note = self.note_row(&wait.id, palette, cx);
        let attention = lane_key(&wait.id, "attention");
        let is_attention_pending = self.answers.read(cx).is_running(&attention);
        let line = div()
            .flex()
            .items_center()
            .gap(sizes::ACTIONS_GAP)
            .children(self.verb_buttons(wait, &verbs, (is_running, is_missing), palette, cx))
            .child(div().flex_1())
            .children(attention_pending(is_attention_pending, palette))
            .children(self.attention_buttons(wait, palette, cx))
            .children(lane.map(|_| open_lane_button(false, palette, cx)))
            .children(
                wait.track
                    .as_ref()
                    .and_then(|reference| snapshot.track(&reference.id))
                    .map(|track| {
                        let id = track.id.clone();
                        action(
                            "inbox-open-track",
                            tr(Key("track.open_link")).into(),
                            None,
                            Face::Quiet,
                            false,
                            palette,
                            cx,
                        )
                        .on_click(cx.listener(
                            move |_, event: &gpui::ClickEvent, _, cx| {
                                cx.emit(InboxEvent::OpenTrack(
                                    id.clone(),
                                    event.modifiers().platform,
                                ));
                            },
                        ))
                    }),
            );
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_WIDE)
            .children(steps)
            .children(note)
            .child(line)
            .into_any_element()
    }

    /// What came of the person's last answer to `wait`: under the ask, or for a blocked lane under the lane's
    /// own actions.
    fn steps_of(
        &self,
        wait: &Wait,
        lane: Option<&Item>,
        palette: &Palette,
        cx: &App,
    ) -> Option<AnyElement> {
        let answers = self.answers.read(cx);
        let lane_keys = lane
            .into_iter()
            .flat_map(|lane| ["R", "s"].map(|action| lane_key(&lane.id, action)));
        let attention = lane_key(&wait.id, "attention");
        let keys = [wait.id.clone(), attention.clone()]
            .into_iter()
            .chain(lane_keys);
        let is_blocked = wait.kind == "blocked";
        let (key, step) = keys
            .filter_map(|key| Some((key.clone(), answers.step(&key)?.clone())))
            .find(|(key, step)| match step {
                Step::Told {
                    is_problem: false, ..
                } if is_blocked => false,
                Step::Told {
                    is_problem: true,
                    text,
                } => {
                    !self.resume_note_owns_failure(&wait.id, key)
                        && !(is_blocked
                            && key.ends_with("#R")
                            && text == tr(Key("inbox.note.resume_failed")))
                        && text != tr(Key("inbox.note.lane_changed"))
                }
                _ => key != &attention,
            })?;
        Some(step_view(&self.answers, &key, &step, palette, cx))
    }

    fn verb_buttons(
        &self,
        wait: &Wait,
        verbs: &Verbs,
        (is_running, is_missing): (bool, bool),
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut buttons = Vec::new();
        let blocked = |verb: &Verb| {
            is_missing
                && verb.say != model::Say::Start
                && model::is_folder_action(wait, &verb.action.key)
        };
        if let Some(verb) = &verbs.primary {
            buttons.push(self.verb_button(verb, true, (is_running, blocked(verb)), palette, cx));
        }
        for verb in &verbs.secondary {
            buttons.push(self.verb_button(verb, false, (is_running, blocked(verb)), palette, cx));
        }
        if is_missing {
            buttons.extend(verbs.missing_blocked_actions(wait).map(|(say, label)| {
                let is_primary = say == model::Say::Resume;
                let (name, _) = verb_names(say, is_primary);
                let face = if is_primary {
                    Face::Primary
                } else {
                    Face::Plain
                };
                let button = action(name, tr(label).into(), None, face, true, palette, cx);
                missing_folder_tip(button, name, cx).into_any_element()
            }));
        }
        for (index, choice) in verbs.choices.iter().enumerate() {
            buttons.push(self.choice_button(index, choice, is_running, palette, cx));
        }
        if verbs.reply.is_some() {
            buttons.push(self.reply_button(wait.original_kind() == "track_decision", palette, cx));
        }
        buttons
    }

    fn verb_button(
        &self,
        verb: &Verb,
        is_primary: bool,
        (is_running, is_folder_missing): (bool, bool),
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (name, key) = verb_names(verb.say, is_primary);
        let face = if is_primary {
            Face::Primary
        } else {
            Face::Plain
        };
        let is_disabled = is_running || is_folder_missing;
        let button = action(
            name,
            verb.label(),
            key.map(Into::into),
            face,
            is_disabled,
            palette,
            cx,
        );
        let verb = verb.clone();
        button
            .when(is_folder_missing, |button| {
                missing_folder_tip(button, name, cx)
            })
            .when(!is_disabled, |button| {
                button.on_click(
                    cx.listener(move |this, _, window, cx| this.press(verb.clone(), window, cx)),
                )
            })
            .into_any_element()
    }

    fn choice_button(
        &self,
        index: usize,
        choice: &str,
        is_running: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chosen = choice.to_string();
        let label = SharedString::from(choice.to_string());
        let button = action(
            &format!("choice-{index}"),
            label,
            None,
            Face::Plain,
            is_running,
            palette,
            cx,
        );
        button
            .when(!is_running, |button| {
                button.on_click(cx.listener(move |this, _, window, cx| {
                    this.press_choice(chosen.clone(), window, cx)
                }))
            })
            .into_any_element()
    }

    fn reply_button(
        &self,
        is_track: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = SharedString::from(tr(Key(if is_track {
            "track.answer"
        } else {
            "inbox.reply"
        })));
        action(
            "verb-reply",
            label,
            Some("R".into()),
            if is_track { Face::Primary } else { Face::Quiet },
            false,
            palette,
            cx,
        )
        .on_click(cx.listener(|this, _, window, cx| this.press_reply(window, cx)))
        .into_any_element()
    }

    pub(in crate::ui) fn resume_note_row(
        &self,
        ask: &str,
        is_folder_missing: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let note = self.note.as_ref()?;
        if note.ask != ask || note.purpose != NoteFor::Resume {
            return None;
        }
        let field = self.resume_field()?;
        let draft = field.read(cx).value();
        let problem = resume_note::problem(&draft);
        let is_invalid = problem.is_some();
        let is_pending = self.resume_note_pending(note, cx);
        let is_disabled = is_pending || is_folder_missing;
        let field = gpui_component::input::Textarea::new(field)
            .appearance(false)
            .bordered(false)
            .readonly(is_pending);
        Some(
            self.note_frame(cx)
                .w_full()
                .flex_col()
                .child(note_field(field.into_any_element(), palette).w_full())
                .children(
                    problem
                        .map(|problem| resume_note_problem(problem, palette))
                        .or_else(|| self.resume_note_refusal(ask, is_pending, palette, cx)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap(sizes::ACTIONS_GAP)
                        .child(self.note_send(is_disabled || is_invalid, palette, cx))
                        .child(self.note_skip(is_disabled, palette, cx))
                        .children(resume_note_counter(draft.chars().count(), palette)),
                )
                .into_any_element(),
        )
    }

    fn resume_note_owns_failure(&self, ask: &str, key: &str) -> bool {
        self.note
            .as_ref()
            .is_some_and(|note| note.ask == ask && self.resume_answer_key(note) == Some(key))
    }

    fn resume_note_refusal(
        &self,
        ask: &str,
        is_pending: bool,
        palette: &Palette,
        cx: &App,
    ) -> Option<AnyElement> {
        let key = self.resume_answer_key(self.note.as_ref()?)?;
        if is_pending || !self.resume_note_owns_failure(ask, key) {
            return None;
        }
        let step = self.answers.read(cx).step(key)?;
        matches!(
            step,
            Step::Told {
                is_problem: true,
                ..
            }
        )
        .then(|| step_view(&self.answers, key, step, palette, cx))
    }

    fn note_row(&self, ask: &str, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let note = self.note.as_ref()?;
        if note.ask != ask || note.purpose == NoteFor::Resume {
            return None;
        }
        if note.purpose == NoteFor::Track {
            return self.track_answer_row(note, palette, cx);
        }
        let field = Input::new(self.field.as_ref()?)
            .appearance(false)
            .bordered(false);
        Some(
            self.note_frame(cx)
                .items_center()
                .child(note_field(field.into_any_element(), palette).flex_1())
                .child(self.note_send(false, palette, cx))
                .into_any_element(),
        )
    }

    fn track_answer_row(
        &self,
        note: &super::decide::Note,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let draft = self.resume_field()?;
        let is_pending = self.resume_note_pending(note, cx);
        let is_empty = draft.read(cx).value().trim().is_empty();
        let field = gpui_component::input::Textarea::new(draft)
            .appearance(false)
            .bordered(false)
            .readonly(is_pending);
        let feedback = self.resume_note_refusal(&note.ask, is_pending, palette, cx);
        Some(
            self.note_frame(cx)
                .flex_col()
                .min_w_0()
                .child(note_field(field.into_any_element(), palette))
                .child(
                    div()
                        .min_h(theme::CONTROL_HEIGHT)
                        .min_w_0()
                        .truncate()
                        .children(feedback),
                )
                .child(
                    div()
                        .flex()
                        .child(self.note_send(is_empty || is_pending, palette, cx)),
                )
                .into_any_element(),
        )
    }

    fn note_frame(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .debug_selector(|| "inbox-note".into())
            .flex()
            .gap(sizes::ACTIONS_GAP)
            .on_action(
                cx.listener(|this, _: &gpui_component::input::Escape, window, cx| {
                    this.leave_note(window, cx)
                }),
            )
    }

    fn note_send(
        &self,
        is_disabled: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        action(
            "note-send",
            tr(Key("waiting.send")).into(),
            None,
            Face::Plain,
            is_disabled,
            palette,
            cx,
        )
        .when(!is_disabled, |button| {
            button.on_click(cx.listener(|this, _, window, cx| this.send_note(window, cx)))
        })
        .into_any_element()
    }

    fn note_skip(
        &self,
        is_disabled: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        action(
            "resume-without-note",
            tr(Key("inbox.note.skip")).into(),
            None,
            Face::Quiet,
            is_disabled,
            palette,
            cx,
        )
        .when(!is_disabled, |button| {
            button.on_click(cx.listener(|this, _, window, cx| this.resume_without_note(window, cx)))
        })
        .into_any_element()
    }

    /// Sends the note when Return is pressed in its field.
    pub(super) fn watch_field(
        field: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Subscription {
        cx.subscribe_in(
            field,
            window,
            |this, field, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. })
                    && !is_composing(field, window, cx)
                {
                    this.send_note(window, cx);
                }
            },
        )
    }
}

fn attention_pending(is_pending: bool, palette: &Palette) -> Option<gpui::Div> {
    is_pending.then(|| {
        text(theme::TEXT_LABEL, palette.text_secondary)
            .debug_selector(|| "inbox-attention-pending".into())
            .child(tr(Key("thread.status.pending")))
    })
}

fn note_field(field: AnyElement, palette: &Palette) -> gpui::Div {
    div()
        .debug_selector(|| "inbox-note-field".into())
        .min_w_0()
        .px(sizes::NOTE_PADDING_X)
        .rounded(sizes::NOTE_RADIUS)
        .bg(palette.raised)
        .child(field)
}

fn resume_note_problem(problem: Problem, palette: &Palette) -> AnyElement {
    let words = match problem {
        Problem::TooLong => tr_with(
            Key("inbox.note.too_long"),
            &[("limit", &CHARACTER_LIMIT.to_string())],
        ),
        Problem::HiddenCharacter => tr(Key("inbox.note.hidden_character")).into(),
        Problem::ControlCharacter => tr(Key("inbox.note.control_character")).into(),
    };
    text(theme::TEXT_SMALL, palette.danger)
        .debug_selector(|| "resume-note-problem".into())
        .child(words)
        .into_any_element()
}

fn resume_note_counter(count: usize, palette: &Palette) -> Option<gpui::Div> {
    (count >= RESUME_NOTE_COUNT_FROM).then(|| {
        let color = if count > CHARACTER_LIMIT {
            palette.danger
        } else {
            palette.text_tertiary
        };
        text(theme::TEXT_LABEL, color)
            .debug_selector(|| "resume-note-count".into())
            .child(format!("{count} / {CHARACTER_LIMIT}"))
    })
}

fn missing_folder_tip(
    button: gpui::Stateful<gpui::Div>,
    name: &str,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    tip::bind(
        button,
        format!("tip-{name}"),
        Words::plain(tr(Key(sakura_core::engine::cli::PROJECT_FOLDER_MISSING))),
        cx,
    )
}

/// The name a verb's button has in the window, and the key it shows.
fn verb_names(say: model::Say, is_primary: bool) -> (&'static str, Option<&'static str>) {
    match say {
        model::Say::Approve | model::Say::Resume => ("verb-approve", is_primary.then_some("A")),
        model::Say::AlwaysApprove => ("verb-always", None),
        model::Say::Decline => ("verb-decline", Some("D")),
        model::Say::Stop => ("verb-stop", None),
        model::Say::Close => ("verb-close", None),
        model::Say::Restore => ("verb-restore", None),
        model::Say::Review => ("verb-mark-reviewed", Some("↩")),
        model::Say::Start => ("verb-start", Some("↩")),
        model::Say::Dismiss => ("verb-dismiss", Some("D")),
    }
}
