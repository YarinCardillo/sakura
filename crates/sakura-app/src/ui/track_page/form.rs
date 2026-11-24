use super::{request, Operation};
use crate::data::{engine_facts::EngineFacts, source::Source};
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette, Theme};
use crate::ui::dialog_frame;
use crate::ui::engine_acts::{self, Done, EngineActs};
use crate::ui::kit::{self, ButtonKind};
use crate::ui::settings_modal::controls;
use gpui::{
    div, AnyElement, App, AppContext as _, Context, Entity, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window,
};
use gpui_component::input::{Input, InputState, Textarea, TextareaState};
use gpui_component::WindowExt as _;
use sakura_core::engine::cli::Act;
use sakura_core::engine::tracks::Tracks;
use std::collections::BTreeMap;
use std::rc::Rc;

const SAVE: &str = "track-save";
type Outcome = Rc<dyn Fn(&str, &mut Window, &mut App)>;

#[derive(Clone)]
pub(super) struct Field {
    pub input: &'static str,
    pub label: &'static str,
    pub value: String,
    pub rows: Option<usize>,
}

#[derive(Clone)]
pub(in crate::ui) struct Pick {
    pub label: String,
    pub inputs: BTreeMap<String, String>,
}

#[derive(Clone)]
pub(in crate::ui) struct Picker {
    pub label: &'static str,
    pub options: Vec<Pick>,
    pub allows_empty: bool,
}

#[derive(Clone)]
pub(in crate::ui) struct FormSpec {
    pub heading: &'static str,
    pub action: Act,
    pub(super) fields: Vec<Field>,
    pub picker: Option<Picker>,
    pub outcome: Option<Outcome>,
}

impl FormSpec {
    pub fn action(heading: &'static str, action: Act) -> Self {
        Self {
            heading,
            action,
            fields: Vec::new(),
            picker: None,
            outcome: None,
        }
    }

    fn description(&self) -> Option<&'static str> {
        let key = match self.heading {
            "track.close" => "track.close_description",
            "track.pause" => "track.pause_description",
            "track.reopen" => "track.reopen_description",
            "track.link" => "track.link_description",
            "track.unlink" => "track.unlink_description",
            "track.retry" => "track.retry_description",
            "track.remove_reservation" => "track.remove_reservation_description",
            _ => return None,
        };
        Some(tr(Key(key)))
    }
}

pub(in crate::ui) fn open(
    form: FormSpec,
    facts: Entity<EngineFacts>,
    source: Source,
    window: &mut Window,
    cx: &mut App,
) {
    if window.has_active_dialog(cx) {
        return;
    }
    let acts = cx.new(|cx| EngineActs::new(source, facts.clone(), cx));
    let form = cx.new(|cx| Form::new(form, facts, acts, window, cx));
    crate::ui::dialog_frame::open_guarded(window, cx, move |dialog, window, _| {
        crate::ui::dialog_frame::placed(dialog, crate::ui::dialog_frame::Width::Medium, window)
            .on_ok(|_, _, _| false)
            .child(form.clone())
    });
}

enum Editor {
    Line(Entity<InputState>),
    Text(Entity<TextareaState>),
}

impl Editor {
    fn new(field: &Field, window: &mut Window, cx: &mut Context<Form>) -> Self {
        match field.rows {
            Some(rows) => Self::Text(cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(rows)
                    .default_value(field.value.clone())
            })),
            None => Self::Line(
                cx.new(|cx| InputState::new(window, cx).default_value(field.value.clone())),
            ),
        }
    }

    fn element(&self, field: &Field, palette: &Palette, cx: &App) -> AnyElement {
        let label = tr(Key(field.label));
        let (editor, focus) = match self {
            Self::Line(input) => (
                Input::new(input)
                    .appearance(false)
                    .bordered(false)
                    .aria_label(label)
                    .into_any_element(),
                input.focus_handle(cx),
            ),
            Self::Text(input) => (
                Textarea::new(input)
                    .appearance(false)
                    .bordered(false)
                    .aria_label(label)
                    .into_any_element(),
                input.focus_handle(cx),
            ),
        };
        let id = format!("track-field-{}", field.input);
        let ring = kit::keyboard_ring(palette);
        div()
            .id(SharedString::from(id.clone()))
            .debug_selector(move || id)
            .bg(palette.fill)
            .rounded(theme::RADIUS_CONTROL)
            .track_focus(&focus)
            .focus_visible(move |style| style.shadow(ring))
            .child(editor)
            .into_any_element()
    }

    fn value(&self, cx: &App) -> SharedString {
        match self {
            Self::Line(field) => field.read(cx).value(),
            Self::Text(field) => field.read(cx).value(),
        }
    }
}

struct Form {
    spec: FormSpec,
    facts: Entity<EngineFacts>,
    acts: Entity<EngineActs>,
    editors: Vec<Editor>,
    choice: Option<usize>,
    operation: Operation,
    problem: Option<&'static str>,
    _subscriptions: Vec<Subscription>,
}

impl Form {
    fn new(
        spec: FormSpec,
        facts: Entity<EngineFacts>,
        acts: Entity<EngineActs>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editors = spec
            .fields
            .iter()
            .map(|field| Editor::new(field, window, cx))
            .collect();
        let subscriptions = vec![
            cx.observe(&acts, |_, _, cx| cx.notify()),
            cx.observe(&facts, |_, _, cx| cx.notify()),
            cx.subscribe_in(&acts, window, |form, _, done: &Done, window, cx| {
                window.close_dialog(cx);
                if let Some(outcome) = &form.spec.outcome {
                    outcome(&done.output, window, cx);
                }
            }),
        ];
        Self {
            spec,
            facts,
            acts,
            editors,
            choice: None,
            operation: Operation::default(),
            problem: None,
            _subscriptions: subscriptions,
        }
    }

    fn inputs(&self, cx: &App) -> Option<BTreeMap<String, String>> {
        let mut inputs = self.spec.action.inputs.clone();
        let is_edit = self.spec.action.action.key == "trackset";
        for (field, editor) in self.spec.fields.iter().zip(&self.editors) {
            let value = editor.value(cx);
            if is_edit && value.as_ref() == field.value {
                continue;
            }
            if field.input == "summary" && value.trim().is_empty() {
                inputs.insert("clear".into(), "summary".into());
            } else if value.trim().is_empty() {
                return None;
            } else {
                inputs.insert(field.input.into(), value.to_string());
            }
        }
        if let Some(picker) = &self.spec.picker {
            match self.choice.and_then(|index| picker.options.get(index)) {
                Some(found) => inputs.extend(found.inputs.clone()),
                None if !picker.allows_empty => return None,
                None => {}
            }
        }
        Some(inputs)
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.is_blocked(cx) {
            return;
        }
        let Some(inputs) = self.inputs(cx) else {
            self.problem = Some("track.reason.invalid_input");
            cx.notify();
            return;
        };
        if self.spec.action.action.key == "trackset" && inputs.is_empty() {
            self.problem = Some("track.no_changes");
            cx.notify();
            return;
        }
        self.problem = None;
        let draft = request(
            &self.spec.action.action,
            self.spec.action.target.clone(),
            inputs,
        );
        let act = self.operation.prepare(draft);
        self.acts
            .update(cx, |acts, cx| acts.press(SAVE, act, "", cx));
        cx.notify();
    }

    fn is_blocked(&self, cx: &App) -> bool {
        self.acts.read(cx).is_running(SAVE)
            || self
                .facts
                .read(cx)
                .snapshot()
                .is_none_or(|snapshot| !matches!(snapshot.tracks, Tracks::Available { .. }))
    }

    fn picker(&self, picker: &Picker, cx: &Context<Self>) -> AnyElement {
        let title = self
            .choice
            .and_then(|index| picker.options.get(index))
            .map_or_else(
                || tr(Key("track.none")).into(),
                |pick| SharedString::from(pick.label.clone()),
            );
        let choices = picker
            .options
            .iter()
            .enumerate()
            .map(|(index, pick)| (Some(index), SharedString::from(pick.label.clone())));
        let empty = picker
            .allows_empty
            .then(|| (None, tr(Key("track.none")).into()));
        let choices = empty
            .into_iter()
            .chain(choices)
            .map(|(index, label)| {
                let form = cx.weak_entity();
                controls::Choice {
                    label,
                    is_on: self.choice == index,
                    on_pick: Rc::new(move |_, cx| {
                        drop(form.update(cx, |form, cx| {
                            form.choice = index;
                            cx.notify();
                        }));
                    }),
                }
            })
            .collect();
        controls::menu(picker.label, title, choices, cx)
    }

    fn footer(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let label = tr(Key(if self.spec.heading == "track.new" {
            "track.create"
        } else {
            "track.apply"
        }));
        let save = if self.is_blocked(cx) {
            kit::button_disabled(SAVE, label, ButtonKind::Primary, palette)
        } else {
            kit::button(SAVE, label, ButtonKind::Primary, palette)
                .on_click(cx.listener(|form, _, _, cx| form.save(cx)))
        };
        let cancel = kit::button(
            "track-cancel",
            tr(Key("common.cancel")),
            ButtonKind::Secondary,
            palette,
        )
        .on_click(|_, window, cx| window.close_dialog(cx));
        let problem = self.problem.map(|key| tr(Key(key)).into()).or_else(|| {
            match self.acts.read(cx).step(SAVE) {
                Some(engine_acts::Step::Told(words)) => Some(SharedString::from(words.clone())),
                _ => None,
            }
        });
        let feedback = div()
            .id("track-form-feedback")
            .debug_selector(|| "track-form-feedback".into())
            .flex_1()
            .min_w_0()
            .h(theme::CONTROL_HEIGHT)
            .overflow_y_scroll()
            .text_color(palette.danger)
            .children(problem);
        dialog_frame::footer(palette)
            .child(feedback)
            .child(cancel)
            .child(save)
            .into_any_element()
    }
}

impl Render for Form {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let fields = self
            .spec
            .fields
            .iter()
            .zip(&self.editors)
            .map(|(field, editor)| {
                field_row(field.label, editor.element(field, &palette, cx), &palette)
            });
        let picker = self
            .spec
            .picker
            .as_ref()
            .map(|picker| field_row(picker.label, self.picker(picker, cx), &palette));
        dialog_frame::surface("track-dialog", window, &palette)
            .child(dialog_frame::heading(
                "track-dialog-title",
                tr(Key(self.spec.heading)),
                &palette,
            ))
            .child(
                crate::ui::dialog_frame::scrolling_body("track-dialog-fields")
                    .gap(theme::GAP)
                    .children(self.spec.description().map(|words| {
                        div()
                            .debug_selector(|| "track-dialog-description".into())
                            .text_size(theme::TEXT_BODY)
                            .text_color(palette.text_secondary)
                            .child(words)
                    }))
                    .children(fields)
                    .children(picker),
            )
            .child(self.footer(&palette, cx))
    }
}

fn field_row(label: &'static str, field: AnyElement, palette: &Palette) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(theme::GAP_SMALL)
        .child(
            div()
                .text_color(palette.text_secondary)
                .text_size(theme::TEXT_LABEL)
                .child(tr(Key(label))),
        )
        .child(field)
        .into_any_element()
}
