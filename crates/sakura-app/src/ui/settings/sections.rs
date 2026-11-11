//! What each section of Settings draws: the projects (their list, one project, the flow that adds
//! one), the accounts, the agent, the appearance, the notifications, the machine and the shortcuts.

use super::controls::{self, Choice, Loaded, Segment, Sent};
use super::{Deps, Notify, ProjectsPage, Read, Section, Settings, SettingsEvent, ThemeChoice};
use crate::l10n::{say, tr, Key, Language, LanguageChoice};
use crate::theme::settings as sizes;
use crate::theme::{self, Palette};
use crate::ui::chrome::text;
use gpui::{
    div, AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::input::{InputEvent, Textarea, TextareaState};
use gpui_component::switch::Switch;
use sakura_core::error::BridgeError;
use sakura_core::models::{Account, AgentPhase, BoxStatus, DiskUse, Place, RunnerPhase};
use sakura_core::publish;
use sakura_core::said::Said;
use sakura_core::settings::Listed;

/// The most agents "Machine" lists: more than run at once on one Mac.
const AGENTS_MAX: usize = 50;
/// The most tasks "Storage" lists.
const STORED_MAX: usize = 200;
/// How many bytes a megabyte and a gigabyte are, as the service counts its budget.
const MEGABYTE: u64 = 1024 * 1024;
const GIGABYTE: u64 = 1024 * MEGABYTE;

impl Settings {
    /// "Projects": the list, the project that was opened, or the flow that adds one.
    pub(super) fn projects_section(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        match &self.page {
            ProjectsPage::Form(form) => form.clone().into_any_element(),
            ProjectsPage::Adding(flow) => flow.clone().into_any_element(),
            ProjectsPage::List => self.projects_list(palette, cx),
        }
    }

    fn projects_list(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let title = tr(Section::Projects.title());
        let listed = match &self.projects {
            Loaded::Reading => vec![controls::quiet(tr(Key("settings.reading")), palette)],
            Loaded::Failed(error) => vec![self.unread(Read::Projects, error, palette, cx)],
            Loaded::Ready(projects) if projects.is_empty() => {
                vec![controls::quiet(tr(Key("settings.projects.none")), palette)]
            }
            Loaded::Ready(projects) => {
                let rows = projects
                    .iter()
                    .map(|listed| self.project_row(listed, palette, cx));
                vec![controls::group(None, rows.collect(), palette)]
            }
        };
        let add = Self::on(cx, |settings, window, cx| settings.add_project(window, cx));
        let add = controls::button(
            "settings-add-project",
            tr(Key("settings.projects.add")),
            add,
            cx,
        );
        let groups = listed
            .into_iter()
            .chain([div().flex().child(add).into_any_element()]);
        controls::page(title, groups.collect(), palette).into_any_element()
    }

    /// One project of the list: a press opens it. One that cannot be used says so, with the
    /// service's words as its details, and opens nothing.
    fn project_row(&self, listed: &Listed, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let name = listed.name().to_string();
        let id = SharedString::from(format!("settings-project-{name}"));
        let hover = palette.hover;
        match listed {
            Listed::Usable(project) => {
                let folder = text(theme::TEXT_SMALL, palette.text_tertiary)
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(project.repo.clone());
                let open = cx.listener(move |settings, _: &gpui::ClickEvent, window, cx| {
                    settings.open_project(&name, window, cx);
                });
                div()
                    .id(id.clone())
                    .rounded(theme::RADIUS_SURFACE)
                    .hover(move |style| style.bg(hover))
                    .on_click(open)
                    .child(controls::row(
                        id,
                        project.name.clone(),
                        folder,
                        None,
                        palette,
                    ))
                    .into_any_element()
            }
            Listed::Unusable { error, .. } => {
                let unusable = BridgeError::failed(Said::new("settings.projects.unusable"))
                    .with_details(Some(error.clone()));
                let said = controls::failed(format!("{id}-unusable"), &unusable, palette);
                controls::row(id, name, said, None, palette)
            }
        }
    }

    /// What could not be read, said once with "Retry": nothing is read again by itself.
    fn unread(
        &self,
        what: Read,
        error: &BridgeError,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let retry = Self::on(cx, move |settings, _, cx| settings.read(what, cx));
        div()
            .flex()
            .items_center()
            .gap(theme::GAP_WIDE)
            .child(controls::failed("settings-unread", error, palette))
            .child(controls::button(
                "settings-retry",
                tr(Key("common.retry")),
                retry,
                cx,
            ))
            .into_any_element()
    }

    /// "Accounts": the accounts of the boxes, signed in or not, and the person's logins on this
    /// Mac, which need nothing.
    pub(super) fn accounts_section(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let title = tr(Section::Accounts.title());
        let groups = match self.accounts.as_ref().unwrap_or(&Loaded::Reading) {
            Loaded::Reading => vec![controls::quiet(tr(Key("settings.reading")), palette)],
            Loaded::Failed(error) => vec![self.unread(Read::Accounts, error, palette, cx)],
            Loaded::Ready(accounts) if accounts.is_empty() => {
                vec![controls::quiet(tr(Key("settings.accounts.none")), palette)]
            }
            Loaded::Ready(accounts) => {
                let of = |place: Place, name: Key| {
                    let rows = accounts.iter().filter(|account| account.place == place);
                    let rows: Vec<AnyElement> = rows
                        .map(|account| self.account_row(account, palette, cx))
                        .collect();
                    (!rows.is_empty()).then(|| controls::group(Some(tr(name)), rows, palette))
                };
                let boxes = of(Place::Box, Key("settings.accounts.boxes"));
                let mac = of(Place::Mac, Key("settings.accounts.mac"));
                boxes.into_iter().chain(mac).collect()
            }
        };
        controls::page(title, groups, palette).into_any_element()
    }

    fn account_row(&self, account: &Account, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let id = SharedString::from(format!(
            "settings-account-{}-{}",
            place_id(account.place),
            account.name
        ));
        let state = |key: Key| controls::quiet(tr(key), palette);
        let control = match (account.place, account.is_usable()) {
            (Place::Mac, true) => state(Key("settings.accounts.mac.login")),
            (Place::Mac, false) => state(Key("settings.accounts.mac.missing")),
            (Place::Box, true) => state(Key("settings.accounts.signed_in")),
            (Place::Box, false) => {
                let name = account.name.clone();
                let ask = Self::on(cx, move |_, _, cx| {
                    cx.emit(SettingsEvent::SignInAsked {
                        account: name.clone(),
                    });
                });
                let sign_in = controls::button(
                    "settings-sign-in",
                    tr(Key("settings.accounts.sign_in")),
                    ask,
                    cx,
                );
                div()
                    .flex()
                    .items_center()
                    .gap(theme::GAP_WIDE)
                    .child(state(Key("settings.accounts.signed_out")))
                    .child(sign_in)
                    .into_any_element()
            }
        };
        controls::row(id, account.name.clone(), control, None, palette)
    }

    /// "Agent": what "Publish" asks the agent in the projects that have no request of their own,
    /// and the one control that brings the app's own request back.
    pub(super) fn agent_section(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let field = div()
            .w(sizes::FIELD_WIDTH)
            .my(theme::GAP)
            .rounded(theme::RADIUS_CONTROL)
            .bg(palette.fill)
            .child(
                Textarea::new(&self.default_request)
                    .appearance(false)
                    .bordered(false),
            );
        let refused = self.default_refused.clone().map(Sent::Failed);
        let rows = vec![controls::row(
            "settings-row-default-request",
            tr(Key("settings.project.request")),
            field,
            refused.as_ref(),
            palette,
        )];
        let default = say(&publish::default_request(tr(Key("settings.agent.base"))));
        let empty = Said::new("settings.agent.request.empty").text("request", default);
        let notes = div()
            .debug_selector(|| "settings-agent-notes".into())
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(controls::quiet(say(&empty), palette))
            .child(controls::quiet(
                tr(Key("settings.agent.request.note")),
                palette,
            ));
        let restore = controls::button(
            "settings-default-request-restore",
            tr(Key("settings.agent.request.restore")),
            Self::on(cx, Self::restore_default_request),
            cx,
        );
        let groups = vec![
            controls::group(Some(tr(Key("settings.project.publishing"))), rows, palette),
            notes.into_any_element(),
            div().flex().child(restore).into_any_element(),
        ];
        controls::page(tr(Section::Agent.title()), groups, palette).into_any_element()
    }

    /// Empties the field and takes the app's own request again.
    fn restore_default_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.default_request
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.default_refused = None;
        if self.publish_default.take().is_some() {
            cx.emit(SettingsEvent::PublishDefaultChanged(None));
        }
        cx.notify();
    }

    /// The default request's field was left: the text is kept by the app.
    pub(super) fn default_request_left(
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
            Ok(request) => {
                self.default_refused = None;
                if request != self.publish_default {
                    self.publish_default = request.clone();
                    cx.emit(SettingsEvent::PublishDefaultChanged(request));
                }
            }
            Err(error) => self.default_refused = Some(error),
        }
        cx.notify();
    }

    /// "Notifications": what the system tells the person of while the app is not in front, each a
    /// switch that applies at once.
    pub(super) fn notifications_section(
        &self,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let switch = |id: &'static str, name: Key, is_on: bool, set: fn(&mut Notify, bool)| {
            let settings = cx.weak_entity();
            let control = Switch::new(id)
                .checked(is_on)
                .on_click(move |is_on: &bool, _, cx| {
                    let is_on = *is_on;
                    drop(settings.update(cx, |settings, cx| {
                        set(&mut settings.notify, is_on);
                        cx.emit(SettingsEvent::NotifyChanged(settings.notify));
                        cx.notify();
                    }));
                });
            let control = div().debug_selector(move || id.into()).child(control);
            controls::row(format!("{id}-row"), tr(name), control, None, palette)
        };
        let notify = self.notify;
        let told = vec![
            switch(
                "settings-notify-needs-you",
                Key("settings.notify.needs_you"),
                notify.needs_you,
                |notify, is_on| notify.needs_you = is_on,
            ),
            switch(
                "settings-notify-finished",
                Key("settings.notify.finished"),
                notify.finished,
                |notify, is_on| notify.finished = is_on,
            ),
        ];
        let groups = vec![controls::group(
            Some(tr(Key("settings.notify.told"))),
            told,
            palette,
        )];
        controls::page(tr(Section::Notifications.title()), groups, palette).into_any_element()
    }

    /// "Appearance": the theme, the language and the terminal's text, each a choice that applies at
    /// once.
    pub(super) fn appearance_section(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let groups = vec![
            controls::group(None, self.look_rows(palette, cx), palette),
            controls::group(
                Some(tr(Key("settings.appearance.terminal"))),
                self.terminal_rows(palette, cx),
                palette,
            ),
        ];
        controls::page(tr(Section::Appearance.title()), groups, palette).into_any_element()
    }

    /// The terminal's font, among the installed ones whose letters have one width, and its size.
    fn terminal_rows(&self, palette: &Palette, cx: &Context<Self>) -> Vec<AnyElement> {
        let own: SharedString = tr(Key("settings.appearance.terminal.font.own")).into();
        let current = self.terminal.family.clone();
        let mut families = self.families.clone().unwrap_or_default();
        // The family in force is always a choice: before the list is read, and when it was removed.
        if let Some(family) = current.clone().filter(|family| !families.contains(family)) {
            families.insert(0, family);
        }
        let names = std::iter::once(None).chain(families.into_iter().map(Some));
        let choices = names.map(|name| Choice {
            label: name.clone().map_or(own.clone(), SharedString::from),
            is_on: name == current,
            on_pick: Self::on(cx, move |settings, _, cx| {
                settings.set_terminal_family(name.clone(), cx);
            }),
        });
        let shown = current.clone().map_or(own.clone(), SharedString::from);
        let font = controls::menu("settings-terminal-font", shown, choices.collect(), cx);
        let size_now = self.terminal.size;
        let sizes = theme::TERMINAL_TEXT_SIZES.into_iter().map(|size| Segment {
            id: size_id(size),
            label: size.to_string().into(),
            is_on: size_now.map_or(f32::from(size) == theme::TERMINAL_TEXT, |now| now == size),
            on_press: Self::on(cx, move |settings, _, cx| {
                settings.set_terminal_size(size, cx);
            }),
        });
        vec![
            controls::row(
                "settings-row-terminal-font",
                tr(Key("settings.appearance.terminal.font")),
                font,
                None,
                palette,
            ),
            controls::row(
                "settings-row-terminal-size",
                tr(Key("settings.appearance.terminal.size")),
                controls::segments(sizes.collect(), palette),
                None,
                palette,
            ),
        ]
    }

    /// The theme and the language.
    fn look_rows(&self, palette: &Palette, cx: &Context<Self>) -> Vec<AnyElement> {
        let theme = |id, label, choice: ThemeChoice| Segment {
            id,
            label: tr(label).into(),
            is_on: self.theme == choice,
            on_press: Self::on(cx, move |settings, _, cx| {
                if settings.theme != choice {
                    settings.theme = choice;
                    cx.emit(SettingsEvent::ThemeChosen(choice));
                    cx.notify();
                }
            }),
        };
        let themes = vec![
            theme(
                "settings-theme-system",
                Key("settings.appearance.theme.system"),
                ThemeChoice::System,
            ),
            theme(
                "settings-theme-light",
                Key("settings.appearance.theme.light"),
                ThemeChoice::Light,
            ),
            theme(
                "settings-theme-dark",
                Key("settings.appearance.theme.dark"),
                ThemeChoice::Dark,
            ),
        ];
        let current = (self.language.current)(cx);
        let language = |id, label: &'static str, choice: LanguageChoice| Segment {
            id,
            label: label.into(),
            is_on: current == choice,
            on_press: Self::on(cx, move |settings, _, cx| {
                (settings.language.set)(choice, cx);
                cx.notify();
            }),
        };
        let languages = vec![
            language(
                "settings-language-automatic",
                tr(Key("settings.appearance.language.automatic")),
                LanguageChoice::Automatic,
            ),
            language(
                "settings-language-en",
                Language::English.name(),
                LanguageChoice::Is(Language::English),
            ),
            language(
                "settings-language-it",
                Language::Italian.name(),
                LanguageChoice::Is(Language::Italian),
            ),
        ];
        vec![
            controls::row(
                "settings-row-theme",
                tr(Key("settings.appearance.theme")),
                controls::segments(themes, palette),
                None,
                palette,
            ),
            controls::row(
                "settings-row-language",
                tr(Key("settings.appearance.language")),
                controls::segments(languages, palette),
                None,
                palette,
            ),
        ]
    }

    /// "Storage": the folder the data is in, and what each task in a box has taken of the budget
    /// the service gives it, as `box status` says it. Read only.
    pub(super) fn storage_section(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let refresh = Self::on(cx, |settings, _, cx| settings.read(Read::Machine, cx));
        let refresh = controls::button(
            "settings-storage-refresh",
            tr(Key("settings.machine.refresh")),
            refresh,
            cx,
        );
        // A long path is cut at the width of a field; it never leaves its row.
        let path = text(theme::TEXT_SMALL, palette.text_tertiary)
            .debug_selector(|| "settings-storage-folder-path".into())
            .max_w(sizes::FIELD_WIDTH)
            .truncate()
            .child(self.data_folder.clone());
        let folder = controls::row(
            "settings-storage-folder",
            tr(Key("settings.storage.folder")),
            path,
            None,
            palette,
        );
        let mut groups = vec![controls::group(
            Some(tr(Key("settings.storage.data"))),
            vec![folder],
            palette,
        )];
        groups.push(match self.machine.as_ref().unwrap_or(&Loaded::Reading) {
            Loaded::Reading => controls::quiet(tr(Key("settings.reading")), palette),
            Loaded::Failed(error) => controls::failed("settings-unread", error, palette),
            Loaded::Ready(status) => stored_group(status, palette),
        });
        groups.push(div().flex().child(refresh).into_any_element());
        controls::page(tr(Section::Storage.title()), groups, palette).into_any_element()
    }

    /// "Machine": what `box status` says of the runner of the checks and of the agents in boxes.
    pub(super) fn machine_section(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let refresh = Self::on(cx, |settings, _, cx| settings.read(Read::Machine, cx));
        let refresh = controls::button(
            "settings-machine-refresh",
            tr(Key("settings.machine.refresh")),
            refresh,
            cx,
        );
        let mut groups = match self.machine.as_ref().unwrap_or(&Loaded::Reading) {
            Loaded::Reading => vec![controls::quiet(tr(Key("settings.reading")), palette)],
            Loaded::Failed(error) => vec![controls::failed("settings-unread", error, palette)],
            Loaded::Ready(status) => {
                vec![runner_group(status, palette), agents_group(status, palette)]
            }
        };
        groups.push(div().flex().child(refresh).into_any_element());
        controls::page(tr(Section::Machine.title()), groups, palette).into_any_element()
    }

    /// "Shortcuts": the menu bar's own table, read only, one group for each menu.
    pub(super) fn shortcuts_section(&self, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let mut menus: Vec<(SharedString, Vec<AnyElement>)> = Vec::new();
        for (nth, shortcut) in (self.shortcuts)(cx).into_iter().enumerate() {
            let keys = text(theme::TEXT_SMALL, palette.text_secondary)
                .px(theme::GAP_SMALL)
                .rounded(theme::RADIUS_CHIP)
                .bg(palette.fill)
                .child(shortcut.keys);
            let id = SharedString::from(format!("settings-shortcut-{nth}"));
            let row = controls::row(id, shortcut.action, keys, None, palette);
            match menus.last_mut() {
                Some((menu, rows)) if *menu == shortcut.menu => rows.push(row),
                _ => menus.push((shortcut.menu, vec![row])),
            }
        }
        let groups = if menus.is_empty() {
            vec![controls::quiet(tr(Key("settings.shortcuts.none")), palette)]
        } else {
            let group = |(menu, rows)| controls::named_group(Some(menu), rows, palette);
            menus.into_iter().map(group).collect()
        };
        controls::page(tr(Section::Shortcuts.title()), groups, palette).into_any_element()
    }
}

/// The field of the request for projects with none of their own: the person's text. What an empty
/// field stands for is said in full under it, where it has the room to be read.
pub(super) fn default_request_field(
    deps: &Deps,
    window: &mut Window,
    cx: &mut Context<Settings>,
) -> Entity<TextareaState> {
    let own = deps.publish_default.clone().unwrap_or_default();
    cx.new(|cx| {
        TextareaState::new(window, cx)
            .auto_grow(sizes::REQUEST_ROWS_MIN, sizes::REQUEST_ROWS_MAX)
            .placeholder(tr(Key("settings.request.placeholder")))
            .default_value(own)
    })
}

/// What names the choice of a terminal text size in the window.
fn size_id(size: u8) -> &'static str {
    match size {
        11 => "settings-terminal-size-11",
        12 => "settings-terminal-size-12",
        13 => "settings-terminal-size-13",
        14 => "settings-terminal-size-14",
        15 => "settings-terminal-size-15",
        _ => "settings-terminal-size-16",
    }
}

/// A size as a person reads it: megabytes under a gigabyte, gigabytes to a tenth from there.
fn size_said(bytes: u64) -> SharedString {
    if bytes < GIGABYTE {
        return say(&Said::new("settings.storage.megabytes").number("size", bytes / MEGABYTE));
    }
    let gigabytes = (bytes / MEGABYTE) as f64 / 1024.;
    say(&Said::new("settings.storage.gigabytes").tenths("size", gigabytes))
}

/// What one task has taken of its budget: the size and the number of files, each of its limit.
pub(super) fn disk_said(disk: &DiskUse) -> SharedString {
    let taken = Said::new("settings.storage.use")
        .text("used", size_said(disk.bytes).to_string())
        .text("limit", size_said(disk.limit_bytes).to_string())
        .number("files", disk.entries)
        .number("file_limit", disk.entry_limit);
    say(&taken)
}

/// The tasks in boxes with what each has taken of its budget; one that is over says so in the
/// colour of a failure.
fn stored_group(status: &BoxStatus, palette: &Palette) -> AnyElement {
    let stored = status
        .tasks
        .iter()
        .filter_map(|task| Some((task.name.clone(), task.disk?)));
    let rows = stored
        .take(STORED_MAX)
        .enumerate()
        .map(|(nth, (task, disk))| {
            let colour = if disk.over_budget {
                palette.danger
            } else {
                palette.text_tertiary
            };
            let mut said = disk_said(&disk);
            if disk.over_budget {
                said = format!("{said} · {}", tr(Key("settings.storage.over"))).into();
            }
            let id = SharedString::from(format!("settings-storage-task-{nth}"));
            let figure = text(theme::TEXT_SMALL, colour).child(said);
            controls::row(id, task, figure, None, palette)
        });
    let rows: Vec<AnyElement> = rows.collect();
    let title = tr(Key("settings.storage.tasks"));
    if rows.is_empty() {
        let none = controls::quiet(tr(Key("settings.storage.none")), palette);
        let heading = text(theme::TEXT_LABEL, palette.text_tertiary).child(title);
        return div()
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(heading)
            .child(none)
            .into_any_element();
    }
    controls::group(Some(title), rows, palette)
}

fn place_id(place: Place) -> &'static str {
    match place {
        Place::Mac => "mac",
        Place::Box => "box",
    }
}

/// The runner of the checks: whether it runs, what it runs now, how many wait.
fn runner_group(status: &BoxStatus, palette: &Palette) -> AnyElement {
    let said = |id: &'static str, name: Key, value: SharedString| {
        controls::row(id, tr(name), controls::quiet(value, palette), None, palette)
    };
    let state = match status.runner.state {
        RunnerPhase::Running => Key("settings.machine.runner.running"),
        RunnerPhase::Stopped => Key("settings.machine.runner.stopped"),
    };
    let now: SharedString = match &status.runner.current {
        Some(current) => {
            let running = Said::new("settings.machine.running")
                .text("check", current.verb.clone())
                .text("task", current.task.clone());
            say(&running)
        }
        None => tr(Key("settings.machine.nothing")).into(),
    };
    let waiting = u64::try_from(status.runner.queue.len()).unwrap_or(u64::MAX);
    let rows = vec![
        said(
            "settings-machine-runner",
            Key("settings.machine.runner"),
            tr(state).into(),
        ),
        said("settings-machine-now", Key("settings.machine.now"), now),
        said(
            "settings-machine-queue",
            Key("settings.machine.queue"),
            crate::l10n::grouped(waiting).into(),
        ),
    ];
    controls::group(Some(tr(Key("settings.machine.checks"))), rows, palette)
}

/// The agents that run in boxes now, with the account, the processor and the memory each takes.
fn agents_group(status: &BoxStatus, palette: &Palette) -> AnyElement {
    let running = status.tasks.iter().filter_map(|task| {
        let agent = task.agent.as_ref()?;
        (agent.state == AgentPhase::Running).then_some((task.name.clone(), agent))
    });
    let rows = running
        .take(AGENTS_MAX)
        .enumerate()
        .map(|(nth, (task, agent))| {
            let mut uses =
                Said::new("settings.machine.agent").text("account", agent.account.clone());
            uses = uses.tenths("cpu", agent.cpu_percent.unwrap_or(0.));
            uses = uses.number("memory", agent.memory_mb.unwrap_or(0));
            let id = SharedString::from(format!("settings-machine-agent-{nth}"));
            controls::row(
                id,
                task,
                controls::quiet(say(&uses), palette),
                None,
                palette,
            )
        });
    let rows: Vec<AnyElement> = rows.collect();
    let title = tr(Key("settings.machine.agents"));
    if rows.is_empty() {
        let none = controls::quiet(tr(Key("settings.machine.agents.none")), palette);
        let heading = text(theme::TEXT_LABEL, palette.text_tertiary).child(title);
        return div()
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(heading)
            .child(none)
            .into_any_element();
    }
    controls::group(Some(title), rows, palette)
}
