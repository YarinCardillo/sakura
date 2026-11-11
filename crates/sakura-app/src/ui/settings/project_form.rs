//! One project in Settings: where it is, where its tasks start and live, its commands, its checks
//! and its publishing request. There is no Save: a choice is sent when it is made and a text when
//! its field is left or Enter is pressed, each as one `box project set`. The row says while its
//! change is on its way and says a failure in place; nothing is sent again by itself. An answer
//! for an earlier sending of the same row is dropped.

use super::controls::{self, Choice, Loaded, OnPress, Segment, Sent};
use super::source::SettingsSource;
use crate::l10n::{tr, tr_with, Key};
use crate::theme::settings as sizes;
use crate::theme::{self, Palette, Theme};
use gpui::{
    div, AnyElement, AppContext as _, Context, Entity, EventEmitter, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window,
};
use gpui_component::input::{InputEvent, InputState, Textarea, TextareaState};
use sakura_core::error::{BridgeError, BridgeResult};
use sakura_core::models::{Account, Place};
use sakura_core::publish;
use sakura_core::said::Said;
use sakura_core::settings::{Branches, Change, Check, Project, RunMode};
use std::collections::HashMap;
use std::rc::Rc;

/// What the form tells Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormEvent {
    /// The service took a change: the list of projects is to be read again.
    Changed,
    /// The person goes back to the list.
    Back,
    /// The project was removed.
    Removed,
    /// The publishing request was changed: the project's own text, or none (the default one).
    RequestChanged(Option<String>),
}

/// What names a row of the form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Row {
    Base,
    Place,
    Account,
    BranchPrefix,
    Setup,
    Run,
    Archive,
    Ports,
    RunMode,
    Check(String),
    NewCheck,
    Request,
    Remove,
}

impl Row {
    /// What names the row in the window whatever the language: for its elements, never shown.
    fn id(&self) -> SharedString {
        let name = match self {
            Row::Base => "base",
            Row::Place => "place",
            Row::Account => "account",
            Row::BranchPrefix => "branch-prefix",
            Row::Setup => "setup",
            Row::Run => "run",
            Row::Archive => "archive",
            Row::Ports => "ports",
            Row::RunMode => "run-mode",
            Row::Check(verb) => return format!("settings-row-check-{verb}").into(),
            Row::NewCheck => "new-check",
            Row::Request => "request",
            Row::Remove => "remove",
        };
        format!("settings-row-{name}").into()
    }
}

/// The form's text fields.
pub(super) struct Fields {
    pub(super) prefix: Entity<InputState>,
    pub(super) setup: Entity<InputState>,
    pub(super) run: Entity<InputState>,
    pub(super) archive: Entity<InputState>,
    pub(super) ports: Entity<InputState>,
    pub(super) new_verb: Entity<InputState>,
    pub(super) new_command: Entity<InputState>,
    pub(super) request: Entity<TextareaState>,
    /// One per check of the project, by its verb.
    pub(super) checks: Vec<(String, Entity<InputState>)>,
}

/// The form of one project.
pub struct ProjectForm {
    source: Rc<dyn SettingsSource>,
    pub(super) project: Project,
    /// The accounts the project can be given, as Settings read them.
    accounts: Vec<Account>,
    branches: Loaded<Branches>,
    /// The read of the branches in flight, so an older answer is dropped.
    branches_read: u64,
    /// The project's own publishing request, none when it uses the default one.
    request: Option<String>,
    pub(super) fields: Fields,
    pub(super) sent: HashMap<Row, Sent>,
    sendings: u64,
    is_removal_asked: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FormEvent> for ProjectForm {}

impl ProjectForm {
    /// The form of `project`, whose branches are read once now.
    pub fn new(
        source: Rc<dyn SettingsSource>,
        project: Project,
        accounts: Vec<Account>,
        request: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = Vec::new();
        let mut field = |row: Row, value: &str, placeholder: Key, cx: &mut Context<Self>| {
            let input = cx.new(|cx| {
                let state = InputState::new(window, cx).placeholder(tr(placeholder));
                state.default_value(value.to_string())
            });
            subscriptions.push(left_or_entered(&input, row, window, cx));
            input
        };
        let command = |command: &Option<String>| command.clone().unwrap_or_default();
        let none = Key("settings.project.command.none");
        let fields = Fields {
            prefix: field(
                Row::BranchPrefix,
                &project.branch_prefix,
                Key("settings.project.prefix.default"),
                cx,
            ),
            setup: field(Row::Setup, &command(&project.setup), none, cx),
            run: field(Row::Run, &command(&project.run), none, cx),
            archive: field(Row::Archive, &command(&project.archive), none, cx),
            ports: field(
                Row::Ports,
                &project.ports.to_string(),
                Key("settings.project.ports"),
                cx,
            ),
            new_verb: field(Row::NewCheck, "", Key("settings.project.check.verb"), cx),
            new_command: field(Row::NewCheck, "", Key("settings.project.check.command"), cx),
            request: request_field(request.as_deref(), window, cx),
            checks: Vec::new(),
        };
        subscriptions.push(cx.subscribe_in(&fields.request, window, Self::request_left));
        let mut form = Self {
            source,
            project,
            accounts,
            branches: Loaded::Reading,
            branches_read: 0,
            request,
            fields,
            sent: HashMap::new(),
            sendings: 0,
            is_removal_asked: false,
            _subscriptions: subscriptions,
        };
        form.sync_checks(window, cx);
        form.read_branches(cx);
        form
    }

    pub fn name(&self) -> &str {
        &self.project.name
    }

    /// The project as the service has it now (read again after a change): what is shown follows
    /// it, except the field the person is typing in.
    pub fn set_project(&mut self, project: Project, window: &mut Window, cx: &mut Context<Self>) {
        self.project = project;
        self.sync_checks(window, cx);
        let command = |command: &Option<String>| command.clone().unwrap_or_default();
        let shown = [
            (&self.fields.prefix, self.project.branch_prefix.clone()),
            (&self.fields.setup, command(&self.project.setup)),
            (&self.fields.run, command(&self.project.run)),
            (&self.fields.archive, command(&self.project.archive)),
            (&self.fields.ports, self.project.ports.to_string()),
        ];
        let checks = self.project.checks.iter().zip(&self.fields.checks);
        let checks = checks.map(|(check, (_, input))| (input, check.command.clone()));
        for (input, value) in shown.into_iter().chain(checks.collect::<Vec<_>>()) {
            let is_typed_in = input.read(cx).focus_handle(cx).is_focused(window);
            if !is_typed_in && input.read(cx).value() != value {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
        }
        cx.notify();
    }

    /// The accounts as Settings read them now.
    pub fn set_accounts(&mut self, accounts: Vec<Account>, cx: &mut Context<Self>) {
        self.accounts = accounts;
        cx.notify();
    }

    /// One field per check of the project, in the project's order.
    fn sync_checks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut kept: HashMap<String, Entity<InputState>> = self.fields.checks.drain(..).collect();
        for check in &self.project.checks {
            let input = kept.remove(&check.verb).unwrap_or_else(|| {
                let input = cx.new(|cx| {
                    let state = InputState::new(window, cx);
                    state.default_value(check.command.clone())
                });
                let row = Row::Check(check.verb.clone());
                self._subscriptions
                    .push(left_or_entered(&input, row, window, cx));
                input
            });
            self.fields.checks.push((check.verb.clone(), input));
        }
    }

    /// Reads the project's branches: once when the form opens, again only when the person asks.
    fn read_branches(&mut self, cx: &mut Context<Self>) {
        self.branches_read += 1;
        let read = self.branches_read;
        self.branches = Loaded::Reading;
        let answer = self.source.branches(&self.project.name);
        cx.spawn(async move |form, cx| {
            let answer = answer.await;
            drop(form.update(cx, |form, cx| {
                if form.branches_read == read {
                    form.branches = answer.map_or_else(Loaded::Failed, Loaded::Ready);
                    cx.notify();
                }
            }));
        })
        .detach();
    }

    /// A text field was left or Enter was pressed in it: what it holds is sent when it differs
    /// from what the project has.
    fn commit(&mut self, row: &Row, window: &mut Window, cx: &mut Context<Self>) {
        let typed = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        let or_empty = |value: &Option<String>| value.clone().unwrap_or_default();
        let (typed, current, change): (String, String, fn(String) -> Change) = match row {
            Row::BranchPrefix => (
                typed(&self.fields.prefix),
                self.project.branch_prefix.clone(),
                Change::BranchPrefix,
            ),
            Row::Setup => (
                typed(&self.fields.setup),
                or_empty(&self.project.setup),
                Change::Setup,
            ),
            Row::Run => (
                typed(&self.fields.run),
                or_empty(&self.project.run),
                Change::Run,
            ),
            Row::Archive => (
                typed(&self.fields.archive),
                or_empty(&self.project.archive),
                Change::Archive,
            ),
            Row::Ports => return self.commit_ports(&typed(&self.fields.ports), window, cx),
            Row::Check(verb) => return self.commit_check(verb, window, cx),
            Row::NewCheck => return self.add_check(window, cx),
            _ => return,
        };
        if typed != current {
            self.send(row.clone(), change(typed), window, cx);
        }
    }

    fn commit_ports(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        match typed.parse::<u8>() {
            Ok(ports) if ports == self.project.ports => {}
            Ok(ports) => self.send(Row::Ports, Change::Ports(ports), window, cx),
            Err(_) => self.refuse(Row::Ports, not_a_value(), cx),
        }
    }

    fn commit_check(&mut self, verb: &str, window: &mut Window, cx: &mut Context<Self>) {
        let field = self.fields.checks.iter().find(|(name, _)| name == verb);
        let check = self.project.checks.iter().find(|check| check.verb == verb);
        let (Some((_, field)), Some(check)) = (field, check) else {
            return;
        };
        let command = field.read(cx).value().trim().to_string();
        if command != check.command {
            let change = Change::Check {
                verb: verb.to_string(),
                command,
            };
            self.send(Row::Check(verb.to_string()), change, window, cx);
        }
    }

    /// "Add": the verb and the command typed become a check, unless the project has that verb.
    fn add_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let verb = self.fields.new_verb.read(cx).value().trim().to_string();
        let command = self.fields.new_command.read(cx).value().trim().to_string();
        let is_on_its_way = matches!(self.sent.get(&Row::NewCheck), Some(Sent::Applying(_)));
        if verb.is_empty() || command.is_empty() || is_on_its_way {
            return;
        }
        if self.project.checks.iter().any(|check| check.verb == verb) {
            let exists = BridgeError::invalid(Said::new("settings.project.check.exists"));
            return self.refuse(Row::NewCheck, exists, cx);
        }
        self.send(Row::NewCheck, Change::Check { verb, command }, window, cx);
    }

    /// The publishing request's field was left: the text is kept by the app, not by `box`.
    fn request_left(
        &mut self,
        field: &Entity<TextareaState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Blur) {
            return;
        }
        match publish::own_request(&field.read(cx).value()) {
            Ok(request) if request == self.request => {}
            Ok(request) => {
                self.request = request.clone();
                self.sent.remove(&Row::Request);
                cx.emit(FormEvent::RequestChanged(request));
                cx.notify();
            }
            Err(error) => self.refuse(Row::Request, error, cx),
        }
    }

    fn refuse(&mut self, row: Row, error: BridgeError, cx: &mut Context<Self>) {
        self.sent.insert(row, Sent::Failed(error));
        cx.notify();
    }

    /// Sends one change. A value `box` would refuse is said in place and never sent.
    fn send(&mut self, row: Row, change: Change, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = change.option() {
            return self.refuse(row, error, cx);
        }
        self.sendings += 1;
        let sending = self.sendings;
        self.sent.insert(row.clone(), Sent::Applying(sending));
        let answer = self.source.set(&self.project.name, change.clone());
        cx.spawn_in(window, async move |form, cx| {
            let answer = answer.await;
            drop(form.update_in(cx, |form, window, cx| {
                form.answered(&row, sending, &change, answer, window, cx);
            }));
        })
        .detach();
        cx.notify();
    }

    fn answered(
        &mut self,
        row: &Row,
        sending: u64,
        change: &Change,
        answer: BridgeResult<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sent.get(row) != Some(&Sent::Applying(sending)) {
            return;
        }
        match answer {
            Ok(()) => {
                self.sent.remove(row);
                apply(&mut self.project, change);
                self.sync_checks(window, cx);
                if *row == Row::NewCheck {
                    for field in [&self.fields.new_verb, &self.fields.new_command] {
                        field.update(cx, |field, cx| field.set_value("", window, cx));
                    }
                }
                cx.emit(FormEvent::Changed);
            }
            Err(error) => drop(self.sent.insert(row.clone(), Sent::Failed(error))),
        }
        cx.notify();
    }

    /// "Remove" was confirmed: `box project rm`. `box` refuses while a task uses the project.
    fn remove(&mut self, cx: &mut Context<Self>) {
        self.sendings += 1;
        let sending = self.sendings;
        self.sent.insert(Row::Remove, Sent::Applying(sending));
        let answer = self.source.remove(&self.project.name);
        cx.spawn(async move |form, cx| {
            let answer = answer.await;
            drop(form.update(cx, |form, cx| {
                if form.sent.get(&Row::Remove) != Some(&Sent::Applying(sending)) {
                    return;
                }
                form.is_removal_asked = false;
                match answer {
                    Ok(()) => cx.emit(FormEvent::Removed),
                    Err(error) => drop(form.sent.insert(Row::Remove, Sent::Failed(error))),
                }
                cx.notify();
            }));
        })
        .detach();
        cx.notify();
    }

    /// A handler that calls `act` on this form.
    fn on(
        cx: &Context<Self>,
        act: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> OnPress {
        let form = cx.weak_entity();
        Rc::new(move |window, cx| drop(form.update(cx, |form, cx| act(form, window, cx))))
    }

    fn row(&self, row: Row, name: Key, control: impl IntoElement, palette: &Palette) -> AnyElement {
        controls::row(row.id(), tr(name), control, self.sent.get(&row), palette)
    }

    fn place_group(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let folder = match self.project.subdir.as_str() {
            "" => self.project.repo.clone(),
            subdir => format!("{}/{subdir}", self.project.repo),
        };
        let folder = controls::quiet(folder, palette);
        let place = |id, label, place: Place| Segment {
            id,
            label: tr(label).into(),
            is_on: self.project.place == place,
            on_press: Self::on(cx, move |form, window, cx| {
                if form.project.place != place {
                    form.send(Row::Place, Change::Place(place), window, cx);
                }
            }),
        };
        let places = vec![
            place(
                "settings-place-mac",
                Key("settings.project.place.mac"),
                Place::Mac,
            ),
            place(
                "settings-place-box",
                Key("settings.project.place.box"),
                Place::Box,
            ),
        ];
        let prefix = controls::field(&self.fields.prefix, sizes::FIELD_WIDTH, palette);
        let rows = vec![
            controls::row(
                "settings-row-folder",
                tr(Key("settings.project.folder")),
                folder,
                None,
                palette,
            ),
            self.row(
                Row::Base,
                Key("settings.project.base"),
                self.base_choice(cx),
                palette,
            ),
            self.row(
                Row::Place,
                Key("settings.project.place"),
                controls::segments(places, palette),
                palette,
            ),
            self.row(
                Row::Account,
                Key("settings.project.account"),
                self.account_choice(cx),
                palette,
            ),
            self.row(
                Row::BranchPrefix,
                Key("settings.project.prefix"),
                prefix,
                palette,
            ),
        ];
        controls::group(None, rows, palette)
    }

    /// The base branch: the project's, in a menu of the repository's branches once they are read.
    fn base_choice(&self, cx: &Context<Self>) -> AnyElement {
        let palette = Theme::palette(cx);
        let names = match &self.branches {
            Loaded::Ready(branches) => branches.names.clone(),
            Loaded::Reading => vec![self.project.base.clone()],
            Loaded::Failed(error) => {
                let retry = Self::on(cx, |form, _, cx| form.read_branches(cx));
                return div()
                    .flex()
                    .items_center()
                    .gap(theme::GAP)
                    .child(controls::failed(
                        "settings-branches-failed",
                        error,
                        &palette,
                    ))
                    .child(controls::button(
                        "settings-branches-retry",
                        tr(Key("common.retry")),
                        retry,
                        cx,
                    ))
                    .into_any_element();
            }
        };
        let choices = names.into_iter().map(|name| Choice {
            is_on: name == self.project.base,
            on_pick: {
                let branch = name.clone();
                Self::on(cx, move |form, window, cx| {
                    if form.project.base != branch {
                        form.send(Row::Base, Change::Base(branch.clone()), window, cx);
                    }
                })
            },
            label: name.into(),
        });
        controls::menu(
            "settings-base",
            self.project.base.clone().into(),
            choices.collect(),
            cx,
        )
    }

    /// The account: none, or one of those that serve where the project's tasks live.
    fn account_choice(&self, cx: &Context<Self>) -> AnyElement {
        let none: SharedString = tr(Key("settings.project.account.none")).into();
        let current = self.project.account.clone();
        let serving = self
            .accounts
            .iter()
            .filter(|account| account.place == self.project.place);
        let names = std::iter::once(None).chain(serving.map(|account| Some(account.name.clone())));
        let choices = names.map(|name| Choice {
            label: name.clone().map_or(none.clone(), SharedString::from),
            is_on: name == current,
            on_pick: {
                let (name, current) = (name.clone().unwrap_or_default(), current.clone());
                Self::on(cx, move |form, window, cx| {
                    if current.as_deref().unwrap_or_default() != name {
                        form.send(Row::Account, Change::Account(name.clone()), window, cx);
                    }
                })
            },
        });
        let shown = current.clone().map_or(none.clone(), SharedString::from);
        controls::menu("settings-account", shown, choices.collect(), cx)
    }

    fn commands_group(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let wide = |input| controls::field(input, sizes::FIELD_WIDTH, palette);
        let mode = |id, label, mode: RunMode| Segment {
            id,
            label: tr(label).into(),
            is_on: self.project.run_mode == mode,
            on_press: Self::on(cx, move |form, window, cx| {
                if form.project.run_mode != mode {
                    form.send(Row::RunMode, Change::RunMode(mode), window, cx);
                }
            }),
        };
        let modes = vec![
            mode(
                "settings-run-mode-one",
                Key("settings.project.run_mode.one"),
                RunMode::One,
            ),
            mode(
                "settings-run-mode-many",
                Key("settings.project.run_mode.many"),
                RunMode::Many,
            ),
        ];
        let ports = controls::field(&self.fields.ports, sizes::FIELD_SHORT_WIDTH, palette);
        let rows = vec![
            self.row(
                Row::Setup,
                Key("settings.project.setup"),
                wide(&self.fields.setup),
                palette,
            ),
            self.row(
                Row::Run,
                Key("settings.project.run"),
                wide(&self.fields.run),
                palette,
            ),
            self.row(
                Row::Archive,
                Key("settings.project.archive"),
                wide(&self.fields.archive),
                palette,
            ),
            self.row(Row::Ports, Key("settings.project.ports"), ports, palette),
            self.row(
                Row::RunMode,
                Key("settings.project.run_mode"),
                controls::segments(modes, palette),
                palette,
            ),
        ];
        controls::group(Some(tr(Key("settings.project.commands"))), rows, palette)
    }

    /// The checks, a verb and its command each; an emptied command takes the check away. Under
    /// them, the row that adds one.
    fn checks_group(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let checks = self.fields.checks.iter().map(|(verb, input)| {
            let row = Row::Check(verb.clone());
            let field = controls::field(input, sizes::FIELD_WIDTH, palette);
            controls::row(row.id(), verb.clone(), field, self.sent.get(&row), palette)
        });
        let add = Self::on(cx, |form, window, cx| form.add_check(window, cx));
        let new = div()
            .flex()
            .items_center()
            .gap(theme::GAP)
            .child(controls::field(
                &self.fields.new_command,
                sizes::FIELD_WIDTH,
                palette,
            ))
            .child(controls::button(
                "settings-check-add",
                tr(Key("settings.project.check.add")),
                add,
                cx,
            ));
        let verb = controls::field(&self.fields.new_verb, sizes::FIELD_SHORT_WIDTH, palette);
        let new = div()
            .debug_selector(|| "settings-row-new-check".into())
            .px(sizes::SETTING_ROW_PADDING_X)
            .flex()
            .flex_col()
            .child(
                div()
                    .min_h(sizes::SETTING_ROW_HEIGHT)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme::GAP_WIDE)
                    .child(verb)
                    .child(new),
            )
            .children(match self.sent.get(&Row::NewCheck) {
                Some(Sent::Failed(error)) => {
                    let failed = controls::failed("settings-row-new-check-failed", error, palette);
                    Some(div().pb(theme::GAP).child(failed))
                }
                _ => None,
            });
        let rows = checks.chain([new.into_any_element()]).collect();
        controls::group(Some(tr(Key("settings.project.checks"))), rows, palette)
    }

    fn publishing_group(&self, palette: &Palette) -> AnyElement {
        let field = div()
            .w(sizes::FIELD_WIDTH)
            .my(theme::GAP)
            .rounded(theme::RADIUS_CONTROL)
            .bg(palette.fill)
            .child(
                Textarea::new(&self.fields.request)
                    .appearance(false)
                    .bordered(false),
            );
        let rows = vec![self.row(
            Row::Request,
            Key("settings.project.request"),
            field,
            palette,
        )];
        controls::group(Some(tr(Key("settings.project.publishing"))), rows, palette)
    }

    /// "Remove project", asked twice: the second press removes; a refusal is said in place.
    fn removal(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let ask = Self::on(cx, |form, _, cx| {
            form.is_removal_asked = true;
            form.sent.remove(&Row::Remove);
            cx.notify();
        });
        let cancel = Self::on(cx, |form, _, cx| {
            form.is_removal_asked = false;
            cx.notify();
        });
        let confirm = Self::on(cx, |form, _, cx| form.remove(cx));
        let buttons = if self.is_removal_asked {
            let sure = tr_with(
                Key("settings.project.remove.sure"),
                &[("name", &self.project.name)],
            );
            div()
                .flex()
                .items_center()
                .gap(theme::GAP)
                .child(controls::quiet(sure, palette))
                .child(controls::button(
                    "settings-remove-cancel",
                    tr(Key("common.cancel")),
                    cancel,
                    cx,
                ))
                .child(
                    controls::button(
                        "settings-remove-confirm",
                        tr(Key("settings.project.remove.confirm")),
                        confirm,
                        cx,
                    )
                    .text_color(palette.danger),
                )
        } else {
            let remove = controls::button(
                "settings-remove",
                tr(Key("settings.project.remove")),
                ask,
                cx,
            );
            div().flex().child(remove)
        };
        let rows = vec![self.row(
            Row::Remove,
            Key("settings.project.remove.name"),
            buttons,
            palette,
        )];
        controls::group(None, rows, palette)
    }
}

impl Render for ProjectForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let back = cx.listener(|_, _: &gpui::ClickEvent, _, cx| cx.emit(FormEvent::Back));
        let back = div()
            .id("settings-project-back")
            .debug_selector(|| "settings-project-back".into())
            .text_size(theme::TEXT_SMALL)
            .text_color(palette.text_secondary)
            .hover(move |style| style.text_color(palette.text))
            .on_click(back)
            .child(tr(Key("settings.projects.back")));
        let groups = vec![
            self.place_group(&palette, cx),
            self.commands_group(&palette, cx),
            self.checks_group(&palette, cx),
            self.publishing_group(&palette),
            self.removal(&palette, cx),
        ];
        div()
            .w_full()
            .max_w(sizes::PAGE_MAX_WIDTH)
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(div().flex().child(back))
            .child(controls::page(self.project.name.clone(), groups, &palette))
    }
}

/// The publishing request's field: the project's own text. An empty field stands for the request
/// of every project, which "Agent" holds.
fn request_field(
    request: Option<&str>,
    window: &mut Window,
    cx: &mut Context<ProjectForm>,
) -> Entity<TextareaState> {
    let own = request.unwrap_or_default().to_string();
    cx.new(|cx| {
        TextareaState::new(window, cx)
            .auto_grow(sizes::REQUEST_ROWS_MIN, sizes::REQUEST_ROWS_MAX)
            .placeholder(tr(Key("settings.request.placeholder")))
            .default_value(own)
    })
}

/// Commits `row` when its field is left or Enter is pressed in it.
fn left_or_entered(
    input: &Entity<InputState>,
    row: Row,
    window: &Window,
    cx: &mut Context<ProjectForm>,
) -> Subscription {
    cx.subscribe_in(
        input,
        window,
        move |form, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                form.commit(&row, window, cx);
            }
        },
    )
}

fn not_a_value() -> BridgeError {
    BridgeError::invalid(Said::new("settings.invalid.value"))
}

/// `project` as it is once the service took `change`.
pub(super) fn apply(project: &mut Project, change: &Change) {
    let optional = |text: &String| (!text.trim().is_empty()).then(|| text.trim().to_string());
    match change {
        Change::Base(branch) => project.base = branch.clone(),
        Change::Place(place) => project.place = *place,
        Change::Account(name) => project.account = optional(name),
        Change::BranchPrefix(prefix) => project.branch_prefix = prefix.clone(),
        Change::Setup(command) => project.setup = optional(command),
        Change::Run(command) => project.run = optional(command),
        Change::Archive(command) => project.archive = optional(command),
        Change::Ports(ports) => project.ports = *ports,
        Change::RunMode(mode) => project.run_mode = *mode,
        Change::Check { verb, command } => {
            project.checks.retain(|check| check.verb != *verb);
            if let Some(command) = optional(command) {
                project.checks.push(Check {
                    verb: verb.clone(),
                    command,
                });
            }
        }
    }
}
