//! "New task": one sheet with one field, "What do you want to do?". Enter sends, Shift+Enter breaks the
//! line. Under the text, quiet and already chosen, what the task is made from: the project (the one used
//! last), the branch it starts from (the project's own), where it lives and the account its agent uses;
//! each is changed in place, as wide as what it says, packed at the left; the send button is at the right
//! and, when a choice stops the task from being requested, the reason is said before it. The sheet asks
//! nothing else: the task is named from the sentence.
//!
//! Opened with a key the field has the caret; opened with the pointer the sheet itself has the keyboard
//! and the field takes the caret on a press.

use super::base_branches;
use super::chooser::opens_on_press;
use super::chrome::text;
use super::conversations::box_account;
use super::desk::Desk;
use super::dialog_frame;
use super::sheet_accounts;
use super::sheet_place;
use super::thread::{button, send_face, send_glyph, send_look, Face};
use crate::data::actions::{Accounts, Actions};
use crate::data::facts::Phase;
use crate::data::projects::{Listed, Projects};
use crate::data::source::Pending;
use crate::l10n::{say, tr, tr_with, Key};
use crate::prefs::Preferences;
use crate::theme::sheet as tokens;
use crate::theme::thread as composer;
use crate::theme::{self, Palette, Theme};
use crate::ui::enter::{guard_enter, is_composing};
use gpui::{
    div, px, AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle,
    Focusable as _, InteractiveElement as _, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, Window,
};
use gpui_component::input::{InputEvent, Textarea, TextareaState};
use gpui_component::select::{
    SearchableVec, Select, SelectDelegate, SelectEvent, SelectGroup, SelectItem, SelectState,
};
use gpui_component::{Sizable as _, WindowExt as _};
use sakura_core::display;
use sakura_core::models::{BoxReadiness, Place};
use sakura_core::projects::Project;

/// What the empty field asks.
const QUESTION: Key = Key("new_task.question");
/// The places a task can live in, as the sheet names them.
const ON_THIS_MAC: Key = Key("new_task.place.mac");
const IN_A_BOX: Key = Key("new_task.place.box");
/// An account of the boxes that is not signed in, as its row names it.
const NOT_SIGNED_IN: Key = Key("new_task.account.not_signed_in");

/// What the person requested: the sentence, and what the task is made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub text: String,
    pub project: String,
    pub base: Option<String>,
    pub place: Place,
    pub account: Option<String>,
}

/// What the sheet tells the desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SheetEvent {
    /// Enter: the task is requested.
    Asked(Asked),
    /// There is no project to make a task in: the person wants to add one.
    AddProject,
    /// The account chosen for a box is not signed in: the person signs in to it.
    SignIn(String),
}

/// One thing that can be chosen: what `box` calls it, and what the sheet shows.
#[derive(Debug, Clone)]
pub struct Choice {
    value: String,
    title: SharedString,
}

impl Choice {
    /// A choice shown under its own name.
    fn named(name: &str) -> Self {
        Self {
            value: name.to_string(),
            title: name.to_string().into(),
        }
    }
}

impl SelectItem for Choice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &String {
        &self.value
    }
}

type Chooser = Entity<SelectState<SearchableVec<Choice>>>;
/// The chooser of the base: its branches are in groups.
type Branches = Entity<SelectState<SearchableVec<SelectGroup<Choice>>>>;

/// The content of the sheet.
pub struct NewTaskSheet {
    is_in_place: bool,
    field: Entity<TextareaState>,
    project: Chooser,
    base: Branches,
    place: Chooser,
    account: Chooser,
    /// The keyboard handle each chooser has while it is closed, in the order the sheet shows them: what
    /// tells an open chooser from a closed one.
    triggers: [FocusHandle; 4],
    /// The project whose own base, place and account are the ones chosen now, until the person changes
    /// one: a project is preset once.
    preset_for: Option<String>,
    /// The tasks there are and the branch each works on, `(lavoro, branch)`: their branches are
    /// offered apart from the repository's own.
    tasks: Vec<(String, String)>,
    /// Whether Docker answered when the sheet asked whether a box can be used: a box is then the
    /// place chosen by itself, where an account of the boxes is signed in.
    does_docker_answer: bool,
    /// The person chose the place: it is not chosen again by itself.
    is_place_chosen: bool,
    projects: Entity<Projects>,
    actions: Entity<Actions>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SheetEvent> for NewTaskSheet {}

impl NewTaskSheet {
    /// An empty sheet over `projects` (read again now) and the accounts `actions` know; `tasks` are the
    /// tasks there are with the branch each works on, `boxes` the answer to whether a box can be used.
    pub fn new(
        (projects, actions): (Entity<Projects>, Entity<Actions>),
        tasks: Vec<(String, String)>,
        boxes: Pending<BoxReadiness>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(tokens::FIELD_MIN_ROWS, tokens::FIELD_MAX_ROWS)
                .placeholder(tr(QUESTION))
                .submit_on_enter(true)
        });
        let chooser = |is_searchable: bool, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let none = SearchableVec::new(Vec::<Choice>::new());
                SelectState::new(none, None, window, cx).searchable(is_searchable)
            })
        };
        let project = chooser(false, window, cx);
        let base = cx.new(|cx| {
            let none = SearchableVec::new(Vec::<SelectGroup<Choice>>::new());
            SelectState::new(none, None, window, cx).searchable(true)
        });
        let place = chooser(false, window, cx);
        let account = chooser(false, window, cx);
        projects.update(cx, |projects, cx| projects.load(cx));
        let triggers = [
            project.read(cx).focus_handle(cx),
            base.read(cx).focus_handle(cx),
            place.read(cx).focus_handle(cx),
            account.read(cx).focus_handle(cx),
        ];
        let mut sheet = Self {
            is_in_place: true,
            _subscriptions: Self::watch(&field, &projects, &actions, window, cx),
            field,
            project,
            base,
            place,
            account,
            triggers,
            preset_for: None,
            tasks,
            does_docker_answer: false,
            is_place_chosen: false,
            projects,
            actions,
        };
        sheet.watch_choices(window, cx);
        sheet.follow(window, cx);
        sheet.hear_boxes(boxes, window, cx);
        sheet
    }

    /// Takes the answer to whether a box can be used: while the person has not chosen the place, it is
    /// chosen again with it.
    fn hear_boxes(
        &self,
        boxes: Pending<BoxReadiness>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |sheet, cx| {
            let answer = boxes.await;
            drop(sheet.update_in(cx, |sheet, window, cx| {
                sheet.does_docker_answer = answer.is_ok_and(|boxes| boxes.docker);
                if !sheet.is_place_chosen {
                    sheet.preset_place(window, cx);
                }
            }));
        })
        .detach();
    }

    /// Chooses the place by itself: the one chosen last for the project, else a box where one can be
    /// used, else the Mac; the account follows.
    fn preset_place(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = value(&self.project, cx) else {
            return;
        };
        let kept = Preferences::try_current(cx).and_then(|kept| kept.places.get(&project).cloned());
        let accounts = match self.actions.read(cx).accounts() {
            Accounts::Loaded(accounts) => accounts.clone(),
            Accounts::Loading | Accounts::Failed(_) => Vec::new(),
        };
        let remembered = sheet_place::remembered(kept.as_deref());
        let place = sheet_place::preset(remembered, self.does_docker_answer, &accounts);
        set(&self.place, place.word(), window, cx);
        self.offer_accounts(true, window, cx);
    }

    /// The field sends on Enter; what is offered follows the projects and the accounts as they are read,
    /// and the question follows the language.
    fn watch(
        field: &Entity<TextareaState>,
        projects: &Entity<Projects>,
        actions: &Entity<Actions>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Subscription> {
        vec![
            cx.subscribe_in(
                field,
                window,
                |sheet, field, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } => {
                        if !is_composing(field, window, cx) {
                            sheet.submit(cx);
                        }
                    }
                    _ => cx.notify(),
                },
            ),
            cx.observe_in(projects, window, |sheet, _, window, cx| {
                sheet.follow(window, cx)
            }),
            cx.observe_in(actions, window, |sheet, _, window, cx| {
                sheet.follow(window, cx)
            }),
            cx.observe_global_in::<crate::l10n::Spoken>(window, |sheet, window, cx| {
                let field = sheet.field.clone();
                field.update(cx, |field, cx| {
                    field.set_placeholder(tr(QUESTION), window, cx)
                });
                sheet.offer_places(window, cx);
            }),
        ]
    }

    /// A project or a place chosen in its list changes what the others offer.
    fn watch_choices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        type Chosen = SelectEvent<SearchableVec<Choice>>;
        let on_project = cx.subscribe_in(
            &self.project,
            window,
            |sheet, _, event: &Chosen, window, cx| {
                let SelectEvent::Confirm(Some(project)) = event else {
                    return;
                };
                sheet.choose_project(&project.clone(), window, cx);
            },
        );
        let on_place = cx.subscribe_in(&self.place, window, |sheet, _, _: &Chosen, window, cx| {
            sheet.is_place_chosen = true;
            sheet.offer_accounts(true, window, cx);
        });
        let on_account = cx.subscribe(&self.account, |sheet, _, event: &Chosen, cx| {
            if let SelectEvent::Confirm(Some(account)) = event {
                sheet.account_chosen(account, cx);
            }
        });
        self._subscriptions
            .extend([on_project, on_place, on_account]);
    }

    /// An account that is not signed in was chosen for a box: the desk is told, and starts the app's
    /// sign-in to it.
    fn account_chosen(&mut self, account: &str, cx: &mut Context<Self>) {
        if self.blocker(cx) == Some(sheet_accounts::NOT_SIGNED_IN) {
            cx.emit(SheetEvent::SignIn(account.to_string()));
        }
    }

    /// Gives the caret to the field: the sheet was opened with a key.
    pub fn focus_field(&self, window: &mut Window, cx: &mut App) {
        self.field.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// Whether the field has the caret.
    #[cfg(test)]
    pub fn has_caret(&self, window: &Window, cx: &App) -> bool {
        self.field.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Writes `text` in the field, as if the person had typed it.
    #[cfg(test)]
    pub fn write(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_string();
        self.field
            .update(cx, |field, cx| field.set_value(text, window, cx));
    }

    /// What is written in the field.
    #[cfg(test)]
    pub fn text(&self, cx: &App) -> String {
        self.field.read(cx).value().to_string()
    }

    /// What is chosen now, in the order the sheet shows it: project, base, place, account.
    #[cfg(test)]
    pub fn chosen(&self, cx: &App) -> [Option<String>; 4] {
        [
            value(&self.project, cx),
            value(&self.base, cx),
            value(&self.place, cx),
            value(&self.account, cx),
        ]
    }

    /// Which choosers are open, in the order the sheet shows them.
    #[cfg(test)]
    pub fn open_choosers(&self, cx: &App) -> [bool; 4] {
        let now = [
            self.project.read(cx).focus_handle(cx),
            self.base.read(cx).focus_handle(cx),
            self.place.read(cx).focus_handle(cx),
            self.account.read(cx).focus_handle(cx),
        ];
        std::array::from_fn(|at| super::chooser::is_open(&self.triggers[at], &now[at]))
    }

    /// Makes the task in `project`: the base, the place and the account become the project's own.
    pub fn choose_project(&mut self, project: &str, window: &mut Window, cx: &mut Context<Self>) {
        set(&self.project, project, window, cx);
        self.preset_for = None;
        self.follow(window, cx);
    }

    /// Starts the task from `branch`.
    #[cfg(test)]
    pub fn choose_base(&mut self, branch: &str, window: &mut Window, cx: &mut Context<Self>) {
        set(&self.base, branch, window, cx);
    }

    /// Makes the task live in `place`; the account becomes the one that place uses.
    #[cfg(test)]
    pub fn choose_place(&mut self, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        set(&self.place, place.word(), window, cx);
        self.offer_accounts(true, window, cx);
    }

    /// Starts the agent with `account`.
    #[cfg(test)]
    pub fn choose_account(&mut self, account: &str, window: &mut Window, cx: &mut Context<Self>) {
        set(&self.account, account, window, cx);
        self.account_chosen(account, cx);
    }

    /// Asks for the task when there is a sentence and a project to make it in.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let text = self.field.read(cx).value().trim().to_string();
        let (Some(project), true) = (value(&self.project, cx), self.can_submit(cx)) else {
            return;
        };
        cx.emit(SheetEvent::Asked(Asked {
            text,
            project,
            base: value(&self.base, cx),
            place: self.place_chosen(cx),
            account: value(&self.account, cx),
        }));
    }

    /// Whether Enter would ask for a task.
    fn can_submit(&self, cx: &App) -> bool {
        let has_sentence = !self.field.read(cx).value().trim().is_empty();
        has_sentence && value(&self.project, cx).is_some() && self.blocker(cx).is_none()
    }

    /// Why the task cannot be requested with what is chosen, whatever is written; nothing when it can.
    fn blocker(&self, cx: &App) -> Option<Key> {
        let Accounts::Loaded(accounts) = self.actions.read(cx).accounts() else {
            return None;
        };
        let place = self.place_chosen(cx);
        let offers = sheet_accounts::offered(accounts, place);
        sheet_accounts::blocker(&offers, place, value(&self.account, cx).as_deref())
    }

    fn place_chosen(&self, cx: &App) -> Place {
        match value(&self.place, cx).as_deref() {
            Some("box") => Place::Box,
            _ => Place::Mac,
        }
    }
}

/// What the sheet offers, kept in line with the projects, their branches and the accounts as read.
impl NewTaskSheet {
    /// The projects are offered; the first time one is chosen (the one used last, else the first) the
    /// base, the place and the account become its own.
    fn follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names: Vec<String> = {
            let usable = self.projects.read(cx).usable();
            usable.iter().map(|project| project.name.clone()).collect()
        };
        let offered = names.iter().map(|name| Choice::named(name)).collect();
        offer(&self.project, offered, window, cx);
        if value(&self.project, cx).is_none() {
            let last = Preferences::try_current(cx).and_then(|kept| kept.last_project);
            let first = last
                .filter(|last| names.contains(last))
                .or(names.first().cloned());
            if let Some(first) = first {
                set(&self.project, &first, window, cx);
            }
        }
        let Some(project) = self.project_chosen(cx) else {
            return cx.notify();
        };
        if self.preset_for.as_deref() != Some(&project.name) {
            self.preset(&project, window, cx);
        }
        self.offer_branches(&project, window, cx);
        self.offer_accounts(false, window, cx);
        cx.notify();
    }

    fn project_chosen(&self, cx: &App) -> Option<Project> {
        let name = value(&self.project, cx)?;
        self.projects.read(cx).project(&name).cloned()
    }

    /// The project's own base, place and account are chosen, and its branches requested.
    fn preset(&mut self, project: &Project, window: &mut Window, cx: &mut Context<Self>) {
        self.preset_for = Some(project.name.clone());
        self.offer_places(window, cx);
        self.offer_branches(project, window, cx);
        if let Some(base) = &project.base {
            set(&self.base, base, window, cx);
        }
        self.is_place_chosen = false;
        self.preset_place(window, cx);
        let name = project.name.clone();
        self.projects
            .update(cx, |projects, cx| projects.load_branches(name, cx));
    }

    fn offer_places(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let place = |place: Place, name: Key| Choice {
            value: place.word().to_string(),
            title: tr(name).into(),
        };
        let places = vec![place(Place::Mac, ON_THIS_MAC), place(Place::Box, IN_A_BOX)];
        offer(&self.place, places, window, cx);
    }

    /// The project's own base first, then the branches of its repository once they are read; apart
    /// from them, the branches other tasks work on, each under its task's name. A group with nothing
    /// in it is not shown.
    fn offer_branches(&mut self, project: &Project, window: &mut Window, cx: &mut Context<Self>) {
        let read = self.projects.read(cx).branches(&project.name);
        let read = read.map(<[String]>::to_vec).unwrap_or_default();
        let offered = base_branches::group(project.base.as_deref(), &read, &self.tasks);
        let own = offered.own.iter().map(|name| Choice::named(name));
        let from_tasks = offered.from_tasks.iter().map(|(branch, task)| Choice {
            value: branch.clone(),
            title: display::label(task).into(),
        });
        let groups = [
            SelectGroup::new(tr(Key("new_task.base.own"))).items(own),
            SelectGroup::new(tr(Key("new_task.base.tasks"))).items(from_tasks),
        ];
        let shown: Vec<_> = groups
            .into_iter()
            .filter(|group| !group.items.is_empty())
            .collect();
        self.base.update(cx, |base, cx| {
            base.set_items(SearchableVec::new(shown), window, cx)
        });
    }

    /// The accounts of the place chosen: for the boxes every one, the ones not signed in saying so.
    /// With `is_reset`, or when none is chosen, the one that place uses is: on the Mac the project's
    /// own, in a box the one chosen last that is signed in.
    fn offer_accounts(&mut self, is_reset: bool, window: &mut Window, cx: &mut Context<Self>) {
        let place = self.place_chosen(cx);
        let Accounts::Loaded(accounts) = self.actions.read(cx).accounts().clone() else {
            return;
        };
        let offers = sheet_accounts::offered(&accounts, place);
        let choices = offers.iter().map(|offer| Choice {
            value: offer.name.clone(),
            title: match offer.is_signed_in {
                true => offer.name.clone().into(),
                false => tr_with(NOT_SIGNED_IN, &[("name", offer.name.as_str())]),
            },
        });
        offer(&self.account, choices.collect(), window, cx);
        if !is_reset && value(&self.account, cx).is_some() {
            return;
        }
        let signed_in = offers.iter().filter(|offer| offer.is_signed_in);
        let names: Vec<String> = signed_in.map(|offer| offer.name.clone()).collect();
        let own = match place {
            Place::Mac => self.project_chosen(cx).and_then(|project| project.account),
            Place::Box => {
                let kept = Preferences::try_current(cx).and_then(|kept| kept.box_account);
                box_account(&accounts, kept.as_deref())
            }
        };
        let preset = own
            .filter(|own| names.contains(own))
            .or(names.first().cloned());
        match preset {
            Some(preset) => set(&self.account, &preset, window, cx),
            None => self.account.update(cx, |account, cx| {
                account.set_selected_index(None, window, cx)
            }),
        }
        cx.notify();
    }
}

/// What `chooser` has chosen.
fn value<D>(chooser: &Entity<SelectState<D>>, cx: &App) -> Option<String>
where
    D: SelectDelegate + 'static,
    D::Item: SelectItem<Value = String>,
{
    chooser.read(cx).selected_value().cloned()
}

/// Chooses `value` in `chooser` (nothing, when it does not offer it).
fn set<D>(chooser: &Entity<SelectState<D>>, value: &str, window: &mut Window, cx: &mut App)
where
    D: SelectDelegate + 'static,
    D::Item: SelectItem<Value = String>,
{
    let value = value.to_string();
    chooser.update(cx, |chooser, cx| {
        chooser.set_selected_value(&value, window, cx)
    });
}

/// Replaces what `chooser` offers; what is chosen stays chosen while it is still offered.
fn offer(chooser: &Chooser, choices: Vec<Choice>, window: &mut Window, cx: &mut App) {
    chooser.update(cx, |chooser, cx| {
        chooser.set_items(SearchableVec::new(choices), window, cx)
    });
}

impl Render for NewTaskSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let field = div()
            .debug_selector(|| "new-task-field".into())
            .w_full()
            .text_size(theme::TEXT_BODY)
            .capture_action(guard_enter(self.field.clone()))
            .child(
                Textarea::new(&self.field)
                    .small()
                    .appearance(false)
                    .bordered(false),
            );
        if self.is_in_place {
            return div()
                .debug_selector(|| "new-task-sheet".into())
                .w_full()
                .p(composer::COMPOSER_PADDING)
                .flex()
                .flex_col()
                .gap(theme::GAP)
                .rounded(composer::COMPOSER_RADIUS)
                .bg(palette.fill)
                .child(field)
                .child(self.under_the_field(&palette, cx))
                .children(self.place_note(&palette, cx));
        }
        dialog_frame::surface("new-task-sheet", window, &palette)
            .child(dialog_frame::heading(
                "new-task-heading",
                tr(Key("new_task.heading")),
                &palette,
            ))
            .child(field)
            .children(self.place_note(&palette, cx))
            .child(self.under_the_field(&palette, cx))
    }
}

impl NewTaskSheet {
    /// Under the text: what the task is made from and the send button; or, when no task can be made,
    /// why in one sentence.
    fn under_the_field(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let row = if self.is_in_place {
            div().flex().items_center().gap(theme::GAP)
        } else {
            dialog_frame::footer(palette)
        };
        let row = row.w_full().min_h(composer::SEND_BUTTON_SIZE);
        let quiet = |sentence: SharedString| {
            text(theme::TEXT_SMALL, palette.text_tertiary)
                .debug_selector(|| "new-task-note".into())
                .flex_1()
                .child(sentence)
        };
        let listed = self.projects.read(cx).listed().clone();
        let has_project = !self.projects.read(cx).usable().is_empty();
        match listed {
            Listed::Loaded(_) if has_project => row
                .child(self.choices(palette))
                .child(self.reason(palette, cx))
                .child(self.send_button(palette, cx))
                .into_any_element(),
            Listed::Loaded(_) => row
                .child(quiet(tr(Key("new_task.no_project")).into()))
                .child(self.add_project_button(palette, cx))
                .into_any_element(),
            Listed::Unknown => row
                .child(quiet(tr(Key("projects.reading")).into()))
                .into_any_element(),
            Listed::Unsupported => row
                .child(quiet(tr(Key("projects.unsupported")).into()))
                .into_any_element(),
            Listed::Failed(error) => row
                .child(quiet(say(&error.said).to_string().into()))
                .into_any_element(),
        }
    }

    /// What working on the Mac means, under the choices while the Mac is chosen.
    fn place_note(&self, palette: &Palette, cx: &App) -> Option<gpui::Div> {
        let is_mac = value(&self.place, cx).is_some() && self.place_chosen(cx) == Place::Mac;
        let has_project = !self.projects.read(cx).usable().is_empty();
        (is_mac && has_project).then(|| {
            // On the inset the kit gives the text of the field and of the choices above it.
            text(theme::TEXT_LABEL, palette.text_tertiary)
                .debug_selector(|| "new-task-mac-means".into())
                .px(gpui_component::Size::Small.input_px())
                .child(tr(sheet_place::ON_THE_MAC))
        })
    }

    /// The room between the choices and the send button: why nothing can be sent, when a choice stops
    /// it, said once and ending beside the button, at the size of the choices it stands beside.
    fn reason(&self, palette: &Palette, cx: &App) -> gpui::Div {
        let room = div().flex_1().min_w_0().flex().justify_end();
        let Some(why) = self.blocker(cx) else {
            return room;
        };
        room.child(
            text(theme::TEXT_LABEL, palette.text_secondary)
                .debug_selector(|| "new-task-reason".into())
                .min_w_0()
                .truncate()
                .child(tr(why)),
        )
    }

    /// The four things chosen, small and quiet, packed at the left: each as wide as what it says.
    fn choices(&self, palette: &Palette) -> gpui::Div {
        let [project, base, place, account] = &self.triggers;
        div()
            .flex()
            .items_center()
            .gap(theme::GAP_TIGHT)
            .text_size(theme::TEXT_LABEL)
            .text_color(palette.text_tertiary)
            .child(chooser(
                ("new-task-project", Key("new_task.project")),
                &self.project,
                project,
            ))
            .child(chooser(
                ("new-task-base", Key("new_task.base")),
                &self.base,
                base,
            ))
            .child(chooser(
                ("new-task-place", Key("new_task.place")),
                &self.place,
                place,
            ))
            .child(chooser(
                ("new-task-account", Key("new_task.account")),
                &self.account,
                account,
            ))
    }

    /// The conversation's own send button: the sentence goes to the agent of the task it makes.
    fn send_button(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let can_submit = self.can_submit(cx);
        let (_, mark) = send_look(palette, can_submit);
        let face = send_face("new-task-send", palette, can_submit)
            .debug_selector(|| "new-task-send".into())
            .aria_label(tr(Key("thread.send")))
            .child(send_glyph(mark));
        if !can_submit {
            return face.into_any_element();
        }
        face.active(|style| style.opacity(composer::PRESSED_OPACITY))
            .on_click(cx.listener(|sheet, _, _, cx| sheet.submit(cx)))
            .into_any_element()
    }

    /// "Choose a folder", when there is no project yet.
    fn add_project_button(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        button(
            "new-task-add-project",
            tr(Key("projects.choose_folder")),
            Face::OnFill,
            palette,
        )
        .debug_selector(|| "new-task-add-project".into())
        .on_click(cx.listener(|_, _, _, cx| cx.emit(SheetEvent::AddProject)))
        .into_any_element()
    }
}

/// One chooser of the sheet's row, found by `name` and called `label`: quiet, opened by one press.
fn chooser<D>(
    (name, label): (&'static str, Key),
    state: &Entity<SelectState<D>>,
    trigger: &FocusHandle,
) -> gpui::Div
where
    D: SelectDelegate + 'static,
    D::Item: SelectItem<Value = String>,
{
    let shown = state.clone();
    let shown = move |cx: &App| shown.read(cx).focus_handle(cx);
    let select = Select::new(state)
        .small()
        .appearance(false)
        .accessibility_label(tr(label))
        .placeholder(tr(label))
        .menu_width(px(tokens::MENU_WIDTH))
        // A menu is never an empty box with the kit's icon: it says why.
        .empty(move |_, cx| {
            let nothing = match name {
                "new-task-account" => Key("new_task.none.account"),
                _ => Key("new_task.none"),
            };
            div()
                .p(theme::GAP)
                .text_size(theme::TEXT_SMALL)
                .text_color(Theme::palette(cx).text_tertiary)
                .child(tr(nothing))
        });
    div()
        .debug_selector(move || name.into())
        .flex_none()
        .child(opens_on_press(select, trigger.clone(), shown))
}

/// "New task" on screen: the sheet, where it is shown, and what the desk does with what it asks.
pub struct NewTaskOpen {
    sheet: Entity<NewTaskSheet>,
    /// It is shown in place in the panel, where there is no task at all, and not over the page.
    is_in_place: bool,
    _asked: Subscription,
}

impl NewTaskOpen {
    #[cfg(test)]
    pub fn sheet(&self) -> &Entity<NewTaskSheet> {
        &self.sheet
    }

    /// Whether it is the one the panel shows in place.
    #[cfg(test)]
    pub fn is_in_place(&self) -> bool {
        self.is_in_place
    }
}

impl Desk {
    /// "New task": what Cmd+N, the sidebar's plus, the File menu and the command palette lead to. With
    /// no task at all the panel already shows its field in place, and this gives it the caret; else
    /// it opens as a sheet over the page. Opened with a key the field has the caret; with the pointer
    /// the sheet itself has the keyboard.
    pub fn open_new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_no_task_yet(cx) {
            let sheet = self.new_task_in_place(window, cx);
            return sheet.update(cx, |sheet, cx| sheet.focus_field(window, cx));
        }
        let is_by_key = window.last_input_was_keyboard();
        let sheet = self.new_sheet(false, window, cx);
        let shown = sheet.clone();
        let desk = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, _| {
            let desk = desk.clone();
            dialog_frame::placed(dialog, dialog_frame::Width::Medium, window)
                .on_close(move |_, _, cx| {
                    drop(desk.update(cx, |desk, cx| {
                        desk.new_task = None;
                        cx.notify();
                    }));
                })
                .close_button(false)
                // Enter is the field's: the sheet closes when a task was requested, not before.
                .on_ok(|_, _, _| false)
                .child(shown.clone())
        });
        if is_by_key {
            sheet.update(cx, |sheet, cx| sheet.focus_field(window, cx));
        }
        cx.notify();
    }

    /// Whether the panel has no task to show and a project to make one in: the state in which "New
    /// task" is the page itself.
    pub(super) fn has_no_task_yet(&self, cx: &App) -> bool {
        let facts = self.facts.read(cx);
        let is_empty = facts.phase() == &Phase::Ready && facts.rows().is_empty();
        let has_project = !self.projects.read(cx).usable().is_empty();
        is_empty && has_project && self.creating.is_empty() && !self.is_adding_project
    }

    /// The sheet the panel shows in place when there is no task at all, made once.
    pub(super) fn new_task_in_place(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<NewTaskSheet> {
        match &self.new_task {
            Some(open) if open.is_in_place => open.sheet.clone(),
            _ => self.new_sheet(true, window, cx),
        }
    }

    /// A sheet over the projects and the tasks there are now, kept as the one on screen: what it asks
    /// is done and it is put away.
    fn new_sheet(
        &mut self,
        is_in_place: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<NewTaskSheet> {
        let (projects, actions) = (self.projects.clone(), self.actions.clone());
        let rows = self.facts.read(cx).rows().iter();
        let tasks = rows
            .filter_map(|row| Some((row.task.name.clone(), row.task.branch.clone()?)))
            .collect();
        let boxes = self.source.boxes();
        let sheet = cx.new(|cx| {
            let mut sheet = NewTaskSheet::new((projects, actions), tasks, boxes, window, cx);
            sheet.is_in_place = is_in_place;
            sheet
        });
        let asked = cx.subscribe_in(
            &sheet,
            window,
            move |desk, _, event: &SheetEvent, window, cx| {
                if !is_in_place {
                    window.close_dialog(cx);
                }
                desk.new_task = None;
                cx.notify();
                match event {
                    SheetEvent::Asked(asked) => desk.begin_lavoro(asked.clone(), window, cx),
                    SheetEvent::AddProject => desk.add_project(cx),
                    SheetEvent::SignIn(account) => desk.sign_in_to(account.clone(), window, cx),
                }
            },
        );
        self.new_task = Some(NewTaskOpen {
            sheet: sheet.clone(),
            is_in_place,
            _asked: asked,
        });
        sheet
    }
}
