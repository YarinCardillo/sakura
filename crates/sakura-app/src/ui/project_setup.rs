//! The page that adds a project: it asks for the folder (one sentence, one button, the Mac's own folder
//! picker), shows on one screen what `box` recognised in it (the repository, the folder inside it, the
//! branch lavori start from, the checks with why each was proposed) to confirm or correct, and adds it.
//! What `box` refuses is said in the interface's words, with its own line under "Details".

use super::brand::{BrandMark, Ink, MarkSize};
use super::centred::centred;
use super::chrome::{mono, text, DETAILS};
use super::thread::{button, Face};
use crate::data::actions::{Accounts, Actions};
use crate::data::source::Source;
use crate::l10n::{say, tr, Key};
use crate::theme::setup as tokens;
use crate::theme::{self, Palette, Theme};
use gpui::{
    div, px, AnyElement, App, AppContext as _, Context, Div, Entity, EventEmitter,
    InteractiveElement as _, IntoElement, ParentElement, PathPromptOptions, Render, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled, Task, Window,
};
use gpui_component::checkbox::Checkbox;
use gpui_component::select::{SearchableVec, Select, SelectState};
use gpui_component::Sizable as _;
use sakura_core::box_words;
use sakura_core::error::BridgeError;
use sakura_core::models::Place;
use sakura_core::projects::{Detected, NewProject};
use sakura_core::said::Said;

const ASK: Key = Key("projects.ask");
const CHOOSE: Key = Key("projects.choose_folder");
const CHOOSE_ANOTHER: Key = Key("projects.choose_another");
const ADD: Key = Key("projects.add");
const NO_CHECKS: Key = Key("projects.no_checks");

/// What the page tells the desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupEvent {
    /// The project of this name was added.
    Added(String),
    /// The person does not want to add a project now.
    Cancelled,
}

/// What `box` refused, in the interface's words, and its own line.
#[derive(Debug, Clone, PartialEq)]
struct Refusal {
    said: SharedString,
    details: Option<String>,
}

impl Refusal {
    fn of(error: &BridgeError) -> Self {
        Self {
            said: say(&error.said).to_string().into(),
            details: error.details.clone(),
        }
    }
}

/// What was recognised in a folder, and what the person corrected.
struct Recognised {
    detected: Detected,
    base: Entity<SelectState<SearchableVec<String>>>,
    /// Which of the proposed checks are kept, in their order.
    kept: Vec<bool>,
    is_adding: bool,
}

/// Where the page stands.
enum Step {
    /// The folder is requested.
    Ask,
    /// `box` is looking at the folder.
    Looking,
    Recognised(Box<Recognised>),
}

/// The page.
pub struct ProjectSetup {
    source: Source,
    actions: Entity<Actions>,
    step: Step,
    /// What `box` refused last: the folder, or the project as confirmed.
    refusal: Option<Refusal>,
    shows_details: bool,
    /// Whether what the service noted about the folder is open, under its own "Details".
    shows_notes: bool,
    /// Whether there is something to go back to: a page with lavori, where "Cancel" leads.
    can_cancel: bool,
    /// The brand's mark over the page's sentence. It reveals itself once, the first time it is drawn in
    /// a launch, and is whole ever after; made when the page is first drawn.
    mark: Option<Entity<BrandMark>>,
    _work: Option<Task<()>>,
}

impl EventEmitter<SetupEvent> for ProjectSetup {}

impl ProjectSetup {
    /// The page asking for a folder.
    pub fn new(source: Source, actions: Entity<Actions>) -> Self {
        Self {
            source,
            actions,
            step: Step::Ask,
            refusal: None,
            shows_details: false,
            shows_notes: false,
            can_cancel: false,
            mark: None,
            _work: None,
        }
    }

    /// The brand's mark of the page, once the page was drawn.
    #[cfg(test)]
    pub fn mark(&self) -> Option<Entity<BrandMark>> {
        self.mark.clone()
    }

    /// Whether "Cancel" is offered: there is a page to go back to.
    pub fn set_can_cancel(&mut self, can_cancel: bool, cx: &mut Context<Self>) {
        if self.can_cancel != can_cancel {
            self.can_cancel = can_cancel;
            cx.notify();
        }
    }

    /// Opens the Mac's folder picker; the folder chosen is looked at.
    pub fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(tr(CHOOSE).into()),
        });
        self._work = Some(cx.spawn_in(window, async move |page, cx| {
            let folder = match chosen.await {
                Ok(Ok(Some(folders))) => folders.into_iter().next(),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(error)) => {
                    eprintln!("sakura: the folder picker could not open: {error}");
                    None
                }
            };
            let Some(folder) = folder else {
                return;
            };
            let folder = folder.to_string_lossy().into_owned();
            drop(page.update_in(cx, |page, window, cx| page.look_at(folder, window, cx)));
        }));
    }

    /// Asks `box` what it recognises in `folder`.
    pub fn look_at(&mut self, folder: String, window: &mut Window, cx: &mut Context<Self>) {
        self.step = Step::Looking;
        self.refusal = None;
        self.shows_details = false;
        self.shows_notes = false;
        let pending = self.source.project_detect(folder);
        self._work = Some(cx.spawn_in(window, async move |page, cx| {
            let answer = pending.await;
            drop(page.update_in(cx, |page, window, cx| {
                match answer {
                    Ok(detected) => page.recognised(detected, window, cx),
                    Err(error) => {
                        page.step = Step::Ask;
                        page.refusal = Some(Refusal::of(&error));
                    }
                }
                cx.notify();
            }));
        }));
        cx.notify();
    }

    fn recognised(&mut self, detected: Detected, window: &mut Window, cx: &mut Context<Self>) {
        let branches = SearchableVec::new(detected.branches.clone());
        let base = cx.new(|cx| {
            let mut base = SelectState::new(branches, None, window, cx).searchable(true);
            base.set_selected_value(&detected.default_branch, window, cx);
            base
        });
        self.step = Step::Recognised(Box::new(Recognised {
            kept: vec![true; detected.checks.len()],
            detected,
            base,
            is_adding: false,
        }));
    }

    /// Keeps or drops the proposed check at `index`.
    pub fn keep_check(&mut self, index: usize, is_kept: bool, cx: &mut Context<Self>) {
        if let Step::Recognised(recognised) = &mut self.step {
            if let Some(kept) = recognised.kept.get_mut(index) {
                *kept = is_kept;
            }
        }
        cx.notify();
    }

    /// Adds the project as it stands on the page.
    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        let account = self.first_own_account(cx);
        let Step::Recognised(recognised) = &mut self.step else {
            return;
        };
        if recognised.is_adding {
            return;
        }
        let mut project = NewProject::proposed(&recognised.detected);
        if let Some(base) = recognised.base.read(cx).selected_value() {
            project.base = base.clone();
        }
        let kept = recognised.kept.clone();
        let mut index = 0;
        project.checks.retain(|_| {
            index += 1;
            kept.get(index - 1).copied().unwrap_or(true)
        });
        project.account = account;
        recognised.is_adding = true;
        self.refusal = None;
        let name = project.name.clone();
        let pending = self.source.project_add(project);
        self._work = Some(cx.spawn(async move |page, cx| {
            let answer = pending.await;
            drop(page.update(cx, |page, cx| page.added(name, answer, cx)));
        }));
        cx.notify();
    }

    fn added(&mut self, name: String, answer: Result<(), BridgeError>, cx: &mut Context<Self>) {
        match answer {
            Ok(()) => {
                self.step = Step::Ask;
                cx.emit(SetupEvent::Added(name));
            }
            Err(error) => {
                if let Step::Recognised(recognised) = &mut self.step {
                    recognised.is_adding = false;
                }
                self.refusal = Some(Refusal::of(&error));
            }
        }
        cx.notify();
    }

    /// The person's own login on this Mac that an agent can start with, when `box` knows one: the
    /// project's lavori on the Mac use it.
    fn first_own_account(&self, cx: &App) -> Option<String> {
        let Accounts::Loaded(accounts) = self.actions.read(cx).accounts() else {
            return None;
        };
        let own = |account: &&sakura_core::models::Account| {
            account.place == Place::Mac && account.is_usable()
        };
        accounts
            .iter()
            .find(own)
            .map(|account| account.name.clone())
    }

    /// What was recognised, when the page shows it.
    #[cfg(test)]
    pub fn detected(&self) -> Option<&Detected> {
        match &self.step {
            Step::Recognised(recognised) => Some(&recognised.detected),
            _ => None,
        }
    }
}

impl Render for ProjectSetup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let mark = self.mark.get_or_insert_with(|| {
            cx.new(|_| BrandMark::revealing(MarkSize::MEDIUM, Ink::Fuchsia))
        });
        let mark = mark.clone();
        let content = match &self.step {
            Step::Ask => self.ask(mark, &palette, cx),
            Step::Looking => text(theme::TEXT_SMALL, palette.text_tertiary)
                .child(tr(Key("projects.looking")))
                .into_any_element(),
            Step::Recognised(recognised) => self.review(recognised, &palette, cx),
        };
        // A page with one sentence and one button is centred as a Mac empty state is; the review of
        // what was recognised fills its column and keeps its left edge.
        let is_review = matches!(self.step, Step::Recognised(_));
        let column = div()
            .max_w_full()
            .flex()
            .flex_col()
            .gap(px(tokens::SECTION_GAP));
        let column = match is_review {
            true => column.w(px(tokens::COLUMN_WIDTH)),
            false => column.items_center(),
        };
        centred(column.child(content)).debug_selector(|| "project-setup".into())
    }
}

impl ProjectSetup {
    /// One sentence and one button; under them, what `box` refused in the folder chosen before.
    fn ask(
        &self,
        mark: Entity<BrandMark>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_refusal = self.refusal.is_some();
        let word = if has_refusal { CHOOSE_ANOTHER } else { CHOOSE };
        let choose = button("project-choose", tr(word), Face::Primary, palette)
            .debug_selector(|| "project-choose".into())
            .on_click(cx.listener(|page, _, window, cx| page.choose_folder(window, cx)));
        let sentence = match has_refusal {
            true => None,
            false => Some(text(theme::TEXT_BODY, palette.text).child(tr(ASK))),
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(tokens::LINE_GAP))
            .child(
                div()
                    .debug_selector(|| "brand-mark-first".into())
                    .child(mark),
            )
            .children(sentence)
            .children(self.refusal_lines(palette, cx))
            .child(
                div()
                    .flex()
                    .justify_center()
                    .gap(theme::GAP)
                    .child(choose)
                    .children(self.cancel_button(palette, cx)),
            )
            .into_any_element()
    }

    /// What `box` refused, and under "Details" its own line.
    fn refusal_lines(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let refusal = self.refusal.as_ref()?;
        let flip: fn(&mut Self) = |page| page.shows_details = !page.shows_details;
        let more = refusal
            .details
            .as_ref()
            .map(|_| details_word("project-details", flip, palette, cx));
        let raw = refusal.details.as_ref().filter(|_| self.shows_details);
        let raw = raw.map(|details| {
            mono()
                .debug_selector(|| "project-details-text".into())
                .text_size(theme::TEXT_LABEL)
                .text_color(palette.text_tertiary)
                .child(details.clone())
        });
        let lines = div()
            .flex()
            .flex_col()
            .gap(theme::GAP_TIGHT)
            .child(
                text(theme::TEXT_BODY, palette.text)
                    .debug_selector(|| "project-refusal".into())
                    .child(refusal.said.clone()),
            )
            .children(more)
            .children(raw);
        Some(lines.into_any_element())
    }

    fn cancel_button(
        &self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        self.can_cancel.then(|| {
            button(
                "project-cancel",
                tr(Key("common.cancel")),
                Face::Quiet,
                palette,
            )
            .debug_selector(|| "project-cancel".into())
            .on_click(cx.listener(|_, _, _, cx| cx.emit(SetupEvent::Cancelled)))
        })
    }

    /// What was recognised, line by line, and the two buttons.
    fn review(
        &self,
        recognised: &Recognised,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detected = &recognised.detected;
        let title = text(theme::TEXT_BODY, palette.text)
            .font_weight(theme::WEIGHT_MEDIUM)
            .debug_selector(|| "project-name".into())
            .child(detected.name.clone());
        let folder = (!detected.subdir.is_empty())
            .then(|| line(Key("projects.subfolder"), plain(&detected.subdir), palette));
        // A filled field, never an outline: the kit's own edge is off and the fill is the theme's.
        let base = div()
            .debug_selector(|| "project-base".into())
            .w(px(tokens::BRANCH_WIDTH))
            .rounded(theme::RADIUS_CONTROL)
            .bg(palette.fill)
            .child(
                Select::new(&recognised.base)
                    .small()
                    .appearance(false)
                    .accessibility_label(tr(Key("projects.base")))
                    .menu_width(px(tokens::BRANCH_WIDTH)),
            );
        let install = detected.install.as_ref().map(|install| {
            let value = proposal(&install.command, &install.reason, palette);
            line(Key("projects.install"), value, palette)
        });
        let no_checks = detected.checks.is_empty().then(|| {
            text(theme::TEXT_SMALL, palette.text_secondary)
                .debug_selector(|| "project-no-checks".into())
                .child(tr(NO_CHECKS))
        });
        div()
            .debug_selector(|| "project-review".into())
            .flex()
            .flex_col()
            .gap(px(tokens::LINE_GAP))
            .child(title)
            .child(line(
                Key("projects.repository"),
                plain(&detected.repo),
                palette,
            ))
            .children(folder)
            .child(line(Key("projects.base"), base.into_any_element(), palette))
            .children(install)
            .children(self.checks(recognised, palette, cx))
            .children(no_checks)
            .children(self.notes(&service_lines(detected), palette, cx))
            .children(self.refusal_lines(palette, cx))
            .child(self.review_buttons(recognised.is_adding, palette, cx))
            .into_any_element()
    }

    /// What the service noted about the folder. They are its own lines, so they wait under "Details".
    fn notes(
        &self,
        notes: &[String],
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if notes.is_empty() {
            return None;
        }
        let flip: fn(&mut Self) = |page| page.shows_notes = !page.shows_notes;
        let raw = notes.iter().filter(|_| self.shows_notes).map(|note| {
            mono()
                .debug_selector(|| "project-notes-text".into())
                .text_size(theme::TEXT_LABEL)
                .text_color(palette.text_tertiary)
                .child(note.clone())
        });
        let lines = div()
            .flex()
            .flex_col()
            .gap(theme::GAP_TIGHT)
            .child(details_word("project-notes", flip, palette, cx))
            .children(raw);
        Some(lines.into_any_element())
    }

    /// The checks proposed: each kept until it is unticked, with its command and why it was proposed.
    fn checks(
        &self,
        recognised: &Recognised,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if recognised.detected.checks.is_empty() {
            return None;
        }
        let rows = recognised
            .detected
            .checks
            .iter()
            .enumerate()
            .map(|(index, check)| {
                let is_kept = recognised.kept.get(index).copied().unwrap_or(true);
                let page = cx.entity();
                let tick = Checkbox::new(("project-check", index))
                    .checked(is_kept)
                    .label(check.verb.clone())
                    .on_click(move |is_kept, _, cx| {
                        page.update(cx, |page, cx| page.keep_check(index, *is_kept, cx));
                    });
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::GAP_TIGHT)
                    .child(tick)
                    .child(div().pl(theme::CONTROL_HEIGHT).child(proposal(
                        &check.command,
                        &check.reason,
                        palette,
                    )))
            });
        let list = div().flex().flex_col().gap(theme::GAP).children(rows);
        let checks = line(Key("projects.checks"), list.into_any_element(), palette);
        Some(checks.into_any_element())
    }

    fn review_buttons(
        &self,
        is_adding: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // While `box` adds the project the button keeps its word and its place, and takes no press.
        let add = button("project-add", tr(ADD), Face::Primary, palette)
            .debug_selector(|| "project-add".into());
        let add = match is_adding {
            true => add.opacity(crate::theme::thread::SEND_BUTTON_DISABLED_OPACITY),
            false => add.on_click(cx.listener(|page, _, _, cx| page.confirm(cx))),
        };
        let another = button(
            "project-another",
            tr(CHOOSE_ANOTHER),
            Face::Secondary,
            palette,
        )
        .debug_selector(|| "project-another".into())
        .on_click(cx.listener(|page, _, window, cx| page.choose_folder(window, cx)));
        div()
            .pt(theme::GAP)
            .flex()
            .gap(theme::GAP)
            .child(add)
            .child(another)
            .children(self.cancel_button(palette, cx))
    }
}

/// One line of the review: its label in its room, and its value.
/// The service's own lines about the folder, for "Details": its notes, and each reason it gave for a
/// proposal that the interface has no sentence for.
fn service_lines(detected: &Detected) -> Vec<String> {
    let install = detected.install.iter().map(|install| &install.reason);
    let checks = detected.checks.iter().map(|check| &check.reason);
    let unknown = install
        .chain(checks)
        .filter(|reason| box_words::proposal_reason(reason).is_none());
    detected.notes.iter().chain(unknown).cloned().collect()
}

fn line(label: Key, value: AnyElement, palette: &Palette) -> gpui::Div {
    div()
        .flex()
        .items_start()
        .gap(theme::GAP_WIDE)
        .child(
            text(theme::TEXT_SMALL, palette.text_tertiary)
                .w(px(tokens::LABEL_WIDTH))
                .flex_none()
                .child(tr(label)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(theme::TEXT_SMALL)
                .child(value),
        )
}

fn plain(value: &str) -> AnyElement {
    div().child(value.to_string()).into_any_element()
}

/// A command `box` proposes, and under it why, in `box`'s own words (they are its data, shown as they
/// are: a sentence of Sakura's around them would be half in one language and half in another).
/// The quiet word that opens and closes the service's own lines.
fn details_word(
    id: &'static str,
    flip: fn(&mut ProjectSetup),
    palette: &Palette,
    cx: &mut Context<ProjectSetup>,
) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_tertiary)
        .cursor_pointer()
        .child(tr(DETAILS))
        .on_click(cx.listener(move |page, _, _, cx| {
            flip(page);
            cx.notify();
        }))
}

fn proposal(command: &str, reason: &str, palette: &Palette) -> AnyElement {
    let why = box_words::proposal_reason(reason).unwrap_or_else(|| Said::from(box_words::PROPOSED));
    div()
        .flex()
        .flex_col()
        .gap(theme::GAP_TIGHT)
        .child(
            mono()
                .text_size(theme::TEXT_SMALL)
                .child(command.to_string()),
        )
        .child(
            text(theme::TEXT_LABEL, palette.text_tertiary)
                .debug_selector(|| "project-reason".into())
                .child(say(&why)),
        )
        .into_any_element()
}
