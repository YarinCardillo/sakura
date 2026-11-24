//! What each section of Settings holds, as rows: a setting has its title, the sentence that says what
//! changes with it and its control; the projects (their list, one project, the flow that adds one) and
//! the brand's page are parts of their section drawn whole.

use super::controls::{self, Choice, Loaded, Segment};
use super::rows::{self, Group, Row};
use super::{
    project_folder, Deps, Notify, ProjectsPage, Read, Section, Settings, SettingsEvent, Shortcut,
    ThemeChoice,
};
use crate::l10n::{say, tr, Key, Language, LanguageChoice};
use crate::theme::settings as sizes;
use crate::theme::{self, Theme};
use crate::ui::brand;
use crate::ui::chrome::text;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, AnyView, AppContext as _, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, Styled as _, Window,
};
use gpui_component::input::{InputEvent, Textarea, TextareaState};
use gpui_component::switch::Switch;
use gpui_component::FocusableExt as _;
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
    /// The rows of `section`, in their groups.
    pub(super) fn groups(&self, section: Section, cx: &Context<Self>) -> Vec<Group> {
        match section {
            Section::Projects => self.projects_groups(cx),
            Section::Accounts => self.accounts_groups(cx),
            Section::Agent => self.agent_groups(cx),
            Section::Appearance => self.appearance_groups(cx),
            Section::Notifications => vec![self.notify_group(cx)],
            Section::Machine => self.machine_groups(cx),
            Section::Storage => self.storage_groups(cx),
            Section::Updates => vec![super::updates::group(cx)],
            Section::Shortcuts => self.shortcuts_groups(cx),
            Section::About => vec![about_group()],
        }
    }

    /// "Projects": the list, the project that was opened, or the flow that adds one.
    fn projects_groups(&self, cx: &Context<Self>) -> Vec<Group> {
        let words = keywords(Key("settings.projects.keywords"));
        let whole = |page: AnyView, name: Option<String>| {
            let mut words = words.clone();
            words.extend(name.map(SharedString::from));
            let row = Row::custom(words, move |_, _| page.clone());
            vec![Group::new(None, vec![row])]
        };
        match &self.page {
            ProjectsPage::Form(form) => {
                let name = form.read(cx).name().to_string();
                whole(form.clone().into(), Some(name))
            }
            ProjectsPage::Adding(flow) => whole(flow.clone().into(), None),
            ProjectsPage::List => self.projects_list(words, cx),
        }
    }

    fn projects_list(&self, words: Vec<SharedString>, cx: &Context<Self>) -> Vec<Group> {
        let mut rows = match &self.projects {
            Loaded::Reading => vec![quiet_row(words.clone(), tr(Key("settings.reading")))],
            Loaded::Failed(error) => {
                vec![self.unread_row(Read::Projects, error, words.clone(), cx)]
            }
            Loaded::Ready(projects) if projects.is_empty() => {
                vec![quiet_row(words.clone(), tr(Key("settings.projects.none")))]
            }
            Loaded::Ready(projects) => projects
                .iter()
                .map(|listed| self.project_row(listed, cx))
                .collect(),
        };
        let add = Self::on(cx, |settings, window, cx| settings.add_project(window, cx));
        rows.push(Row::setting(
            tr(Key("settings.projects.add")),
            tr(Key("settings.projects.add.description")),
            words,
            move |_, cx| {
                let label = tr(Key("settings.projects.add.choose"));
                controls::button("settings-add-project", label, add.clone(), cx)
            },
        ));
        vec![Group::new(None, rows)]
    }

    /// One project of the list: its folder under its name, and the button that opens it. One that
    /// cannot be used says so, with the service's words as its sentence, and opens nothing.
    fn project_row(&self, listed: &Listed, cx: &Context<Self>) -> Row {
        let name = listed.name().to_string();
        let id = SharedString::from(format!("settings-project-{name}"));
        match listed {
            Listed::Usable(project) => {
                let folder = project_folder(project);
                let opened = name.clone();
                let open = Self::on(cx, move |settings, window, cx| {
                    settings.open_project(&opened, window, cx);
                });
                Row::setting(name, folder, Vec::new(), move |_, cx| {
                    let label = tr(Key("settings.projects.open"));
                    controls::button(id.clone(), label, open.clone(), cx)
                })
            }
            Listed::Unusable { error, .. } => {
                let unusable = BridgeError::failed(Said::new("settings.projects.unusable"));
                Row::setting(name, error.clone(), Vec::new(), move |_, cx| {
                    let said = controls::failed(format!("{id}-unusable"), &unusable, &palette(cx));
                    marked(id.clone(), said)
                })
            }
        }
    }

    /// What could not be read, said once with "Retry": nothing is read again by itself.
    fn unread_row(
        &self,
        what: Read,
        error: &BridgeError,
        words: Vec<SharedString>,
        cx: &Context<Self>,
    ) -> Row {
        let retry = Self::on(cx, move |settings, _, cx| settings.read(what, cx));
        let error = error.clone();
        Row::custom(words, move |_, cx| {
            div()
                .flex()
                .items_center()
                .gap(theme::GAP_WIDE)
                .child(controls::failed("settings-unread", &error, &palette(cx)))
                .child(controls::button(
                    "settings-retry",
                    tr(Key("common.retry")),
                    retry.clone(),
                    cx,
                ))
        })
    }

    /// "Accounts": the accounts of the boxes, signed in or not, and the person's logins on this
    /// Mac, which need nothing.
    fn accounts_groups(&self, cx: &Context<Self>) -> Vec<Group> {
        let words = keywords(Key("settings.accounts.keywords"));
        let accounts = match self.accounts.as_ref().unwrap_or(&Loaded::Reading) {
            Loaded::Reading => {
                return vec![Group::new(
                    None,
                    vec![quiet_row(words, tr(Key("settings.reading")))],
                )]
            }
            Loaded::Failed(error) => {
                let unread = self.unread_row(Read::Accounts, error, words, cx);
                return vec![Group::new(None, vec![unread])];
            }
            Loaded::Ready(accounts) if accounts.is_empty() => {
                let none = quiet_row(words, tr(Key("settings.accounts.none")));
                return vec![Group::new(None, vec![none])];
            }
            Loaded::Ready(accounts) => accounts,
        };
        let of = |place: Place, name: Key| {
            let rows = accounts.iter().filter(|account| account.place == place);
            let rows: Vec<Row> = rows
                .map(|account| account_row(account, &words, cx))
                .collect();
            (!rows.is_empty()).then(|| Group::new(Some(tr(name).into()), rows))
        };
        let boxes = of(Place::Box, Key("settings.accounts.boxes"));
        let mac = of(Place::Mac, Key("settings.accounts.mac"));
        boxes.into_iter().chain(mac).collect()
    }

    /// "Agent": what "Publish" asks the agent in the projects that have no request of their own,
    /// and the one control that brings the app's own request back.
    fn agent_groups(&self, cx: &Context<Self>) -> Vec<Group> {
        let field = self.default_request.clone();
        let refused = self.default_refused.clone();
        let request = Row::setting(
            tr(Key("settings.project.request")),
            tr(Key("settings.agent.request.description")),
            keywords(Key("settings.agent.request.keywords")),
            move |_, cx| request_field(&field, refused.as_ref(), cx),
        )
        .stacked();
        let restore = Self::on(cx, Self::restore_default_request);
        let restore = Row::setting(
            tr(Key("settings.agent.restore.title")),
            tr(Key("settings.agent.restore.description")),
            keywords(Key("settings.agent.request.keywords")),
            move |_, cx| {
                let label = tr(Key("settings.agent.request.restore"));
                controls::button(
                    "settings-default-request-restore",
                    label,
                    restore.clone(),
                    cx,
                )
            },
        );
        let publishing = Some(tr(Key("settings.project.publishing")).into());
        vec![
            Group::new(publishing, vec![request]).footer(|_, cx| request_notes(cx)),
            Group::new(None, vec![restore]),
        ]
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
    fn notify_group(&self, cx: &Context<Self>) -> Group {
        let switch = |id: &'static str, name: Key, is_on: bool, set: fn(&mut Notify, bool)| {
            let settings = cx.weak_entity();
            let control = move |window: &mut Window, _: &mut gpui::App| {
                let settings = settings.clone();
                // A switch pressed with the pointer keeps the focus but shows no ring; one reached
                // with the keyboard shows it.
                let switch = Switch::new(id)
                    .focus_ring(window.last_input_was_keyboard())
                    .checked(is_on)
                    .on_click(move |is_on: &bool, _, cx| {
                        let is_on = *is_on;
                        drop(settings.update(cx, |settings, cx| {
                            set(&mut settings.notify, is_on);
                            cx.emit(SettingsEvent::NotifyChanged(settings.notify));
                            cx.notify();
                        }));
                    });
                let switch = div().debug_selector(move || id.into()).child(switch);
                marked(format!("{id}-row"), switch)
            };
            (name, control)
        };
        let notify = self.notify;
        let (needs_you, needs_you_switch) = switch(
            "settings-notify-needs-you",
            Key("settings.notify.needs_you"),
            notify.needs_you,
            |notify, is_on| notify.needs_you = is_on,
        );
        let (finished, finished_switch) = switch(
            "settings-notify-finished",
            Key("settings.notify.finished"),
            notify.finished,
            |notify, is_on| notify.finished = is_on,
        );
        let words = keywords(Key("settings.notify.keywords"));
        let rows = vec![
            Row::setting(
                tr(needs_you),
                tr(Key("settings.notify.needs_you.description")),
                words.clone(),
                needs_you_switch,
            ),
            Row::setting(
                tr(finished),
                tr(Key("settings.notify.finished.description")),
                words,
                finished_switch,
            ),
        ];
        Group::new(Some(tr(Key("settings.notify.told")).into()), rows)
    }

    /// "Appearance": the theme, the language and the terminal's text, each a choice that applies at
    /// once.
    fn appearance_groups(&self, cx: &Context<Self>) -> Vec<Group> {
        let terminal = Some(tr(Key("settings.appearance.terminal")).into());
        vec![
            Group::new(None, vec![self.theme_row(cx), self.language_row(cx)]),
            Group::new(terminal, vec![self.font_row(cx), self.size_row(cx)]),
        ]
    }

    fn theme_row(&self, cx: &Context<Self>) -> Row {
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
        Row::setting(
            tr(Key("settings.appearance.theme")),
            tr(Key("settings.appearance.theme.description")),
            keywords(Key("settings.appearance.theme.keywords")),
            move |_, cx| segments_in("settings-row-theme", themes.clone(), cx),
        )
    }

    fn language_row(&self, cx: &Context<Self>) -> Row {
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
        Row::setting(
            tr(Key("settings.appearance.language")),
            tr(Key("settings.appearance.language.description")),
            keywords(Key("settings.appearance.language.keywords")),
            move |_, cx| segments_in("settings-row-language", languages.clone(), cx),
        )
    }

    /// The terminal's font, among the installed ones whose letters have one width.
    fn font_row(&self, cx: &Context<Self>) -> Row {
        let own: SharedString = tr(Key("settings.appearance.terminal.font.own")).into();
        let current = self.terminal.family.clone();
        let mut families = self.families.clone().unwrap_or_default();
        // The family in force is always a choice: before the list is read, and when it was removed.
        if let Some(family) = current.clone().filter(|family| !families.contains(family)) {
            families.insert(0, family);
        }
        let names = std::iter::once(None).chain(families.into_iter().map(Some));
        let choices: Vec<Choice> = names
            .map(|name| Choice {
                label: name.clone().map_or(own.clone(), SharedString::from),
                is_on: name == current,
                on_pick: Self::on(cx, move |settings, _, cx| {
                    settings.set_terminal_family(name.clone(), cx);
                }),
            })
            .collect();
        let shown = current.map_or(own, SharedString::from);
        Row::setting(
            tr(Key("settings.appearance.terminal.font")),
            tr(Key("settings.appearance.terminal.font.description")),
            keywords(Key("settings.appearance.terminal.font.keywords")),
            move |_, cx| {
                let menu =
                    controls::menu("settings-terminal-font", shown.clone(), choices.clone(), cx);
                marked("settings-row-terminal-font", menu)
            },
        )
    }

    /// The size of the terminal's text, one of a few, in points.
    fn size_row(&self, cx: &Context<Self>) -> Row {
        let size_now = self.terminal.size;
        let sizes: Vec<Segment> = theme::TERMINAL_TEXT_SIZES
            .into_iter()
            .map(|size| Segment {
                id: size_id(size),
                label: size.to_string().into(),
                is_on: size_now.map_or(f32::from(size) == theme::TERMINAL_TEXT, |now| now == size),
                on_press: Self::on(cx, move |settings, _, cx| {
                    settings.set_terminal_size(size, cx);
                }),
            })
            .collect();
        Row::setting(
            tr(Key("settings.appearance.terminal.size")),
            tr(Key("settings.appearance.terminal.size.description")),
            keywords(Key("settings.appearance.terminal.size.keywords")),
            move |_, cx| segments_in("settings-row-terminal-size", sizes.clone(), cx),
        )
    }

    /// "Storage": the folder the data is in, and what each task in a box has taken of the budget
    /// the service gives it, as `box status` says it. Read only.
    fn storage_groups(&self, cx: &Context<Self>) -> Vec<Group> {
        let words = keywords(Key("settings.storage.keywords"));
        let folder = self.data_folder.clone();
        let folder = Row::setting(
            tr(Key("settings.storage.folder")),
            tr(Key("settings.storage.folder.description")),
            words.clone(),
            move |_, cx| {
                // A long path is cut at the width of a field; it never leaves its row.
                let path = text(sizes::SETTING_TEXT, palette(cx).text_tertiary)
                    .debug_selector(|| "settings-storage-folder-path".into())
                    .max_w(sizes::FIELD_WIDTH)
                    .truncate()
                    .child(folder.clone());
                marked("settings-storage-folder", path)
            },
        );
        let tasks = match self.machine.as_ref().unwrap_or(&Loaded::Reading) {
            Loaded::Reading => vec![quiet_row(words.clone(), tr(Key("settings.reading")))],
            Loaded::Failed(error) => vec![failed_row(error, words.clone())],
            Loaded::Ready(status) => stored_rows(status, &words),
        };
        let refresh = self.refresh_row("settings-storage-refresh", words, cx);
        vec![
            Group::new(Some(tr(Key("settings.storage.data")).into()), vec![folder]),
            Group::new(Some(tr(Key("settings.storage.tasks")).into()), tasks),
            Group::new(None, vec![refresh]),
        ]
    }

    /// "Machine": the runner of the checks, chosen and as `box status` says it, and the agents in boxes.
    fn machine_groups(&self, cx: &Context<Self>) -> Vec<Group> {
        let words = keywords(Key("settings.machine.keywords"));
        let refresh = vec![self.refresh_row("settings-machine-refresh", words.clone(), cx)];
        let checks = Some(tr(Key("settings.machine.checks")).into());
        let agents = Some(tr(Key("settings.machine.agents")).into());
        let mut groups = match self.machine.as_ref().unwrap_or(&Loaded::Reading) {
            Loaded::Reading => {
                vec![Group::new(
                    None,
                    vec![quiet_row(words, tr(Key("settings.reading")))],
                )]
            }
            Loaded::Failed(error) => vec![Group::new(None, vec![failed_row(error, words)])],
            Loaded::Ready(status) => {
                let mut rows: Vec<Row> = Vec::new();
                // The engine switch is built only with the `restate-engine` feature, and only for a
                // service that says it has a choice of runner.
                #[cfg(feature = "restate-engine")]
                rows.extend(
                    status
                        .check_runner
                        .as_ref()
                        .map(|choice| self.engine_row(choice, cx)),
                );
                rows.extend(runner_rows(status, &words));
                vec![
                    Group::new(checks, rows),
                    Group::new(agents, agent_rows(status, &words)),
                ]
            }
        };
        groups.push(Group::new(None, refresh));
        groups
    }

    /// What reads the machine again: the read is done when the section is first shown, and again
    /// only when asked.
    fn refresh_row(&self, id: &'static str, words: Vec<SharedString>, cx: &Context<Self>) -> Row {
        let refresh = Self::on(cx, |settings, _, cx| settings.read(Read::Machine, cx));
        Row::setting(
            tr(Key("settings.machine.status")),
            tr(Key("settings.machine.status.description")),
            words,
            move |_, cx| {
                let label = tr(Key("settings.machine.refresh"));
                controls::button(id, label, refresh.clone(), cx)
            },
        )
    }

    /// "Shortcuts": the menu bar's own table, read only, one group for each menu with a line for each
    /// command and its keys. A search keeps the commands it finds, or every command of a menu whose
    /// name it holds.
    fn shortcuts_groups(&self, cx: &Context<Self>) -> Vec<Group> {
        let words = keywords(Key("settings.shortcuts.keywords"));
        let shortcuts = (self.shortcuts)(cx);
        if shortcuts.is_empty() {
            let none = quiet_row(words, tr(Key("settings.shortcuts.none")));
            return vec![Group::new(None, vec![none])];
        }
        let query = self.search.read(cx).value();
        let is_all = words.iter().any(|word| rows::holds(word, &query));
        let mut menus: Vec<(SharedString, Vec<(usize, Shortcut)>)> = Vec::new();
        for (nth, shortcut) in shortcuts.into_iter().enumerate() {
            match menus.last_mut() {
                Some((menu, listed)) if *menu == shortcut.menu => listed.push((nth, shortcut)),
                _ => menus.push((shortcut.menu.clone(), vec![(nth, shortcut)])),
            }
        }
        let menu_group = |(menu, listed): (SharedString, Vec<(usize, Shortcut)>)| {
            let mut found = words.clone();
            found.push(menu.clone());
            found.extend(listed.iter().map(|(_, shortcut)| shortcut.action.clone()));
            let is_menu = is_all || rows::holds(&menu, &query);
            let shown: Vec<(usize, Shortcut)> = listed
                .into_iter()
                .filter(|(_, shortcut)| is_menu || rows::holds(&shortcut.action, &query))
                .collect();
            let lines = Row::custom(found, move |_, cx| shortcut_lines(&shown, cx));
            Group::new(Some(menu), vec![lines])
        };
        menus.into_iter().map(menu_group).collect()
    }
}

/// The commands of one menu, a line each: its name at the left and its keys at the right, a hairline
/// between two lines.
fn shortcut_lines(shown: &[(usize, Shortcut)], cx: &gpui::App) -> AnyElement {
    let palette = palette(cx);
    let lines = shown.iter().enumerate().map(|(line, (nth, shortcut))| {
        let keys = text(sizes::SETTING_TEXT, palette.text_secondary)
            .px(theme::GAP_SMALL)
            .rounded(theme::RADIUS_CHIP)
            .bg(palette.fill)
            .child(shortcut.keys.clone());
        div()
            .when(line > 0, |row| {
                row.border_t_1().border_color(palette.border)
            })
            .min_h(sizes::SETTING_ROW_HEIGHT)
            .flex()
            .items_center()
            .justify_between()
            .gap(theme::GAP_WIDE)
            .child(text(sizes::SETTING_TEXT, palette.text).child(shortcut.action.clone()))
            .child(marked(format!("settings-shortcut-{nth}"), keys))
    });
    div().flex().flex_col().children(lines).into_any_element()
}

/// The current palette, inside a row's control.
fn palette(cx: &gpui::App) -> theme::Palette {
    Theme::palette(cx)
}

/// The words a person may search a row by, from a list in the locale files (separated by commas).
pub(super) fn keywords(key: Key) -> Vec<SharedString> {
    tr(key)
        .split(',')
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .map(|word| SharedString::from(word.to_string()))
        .collect()
}

/// `control`, under the name `id` in the window: what a test or a script finds the row's control by.
pub(super) fn marked(id: impl Into<SharedString>, control: impl IntoElement) -> AnyElement {
    let id: SharedString = id.into();
    div()
        .debug_selector(move || id.to_string())
        .child(control)
        .into_any_element()
}

/// A choice between a few words, named `id` in the window.
fn segments_in(id: &'static str, options: Vec<Segment>, cx: &gpui::App) -> AnyElement {
    marked(id, controls::segments(options, &palette(cx)))
}

/// A quiet sentence in place of rows: something is being read, a list is empty.
fn quiet_row(words: Vec<SharedString>, sentence: &'static str) -> Row {
    Row::custom(words, move |_, cx| controls::quiet(sentence, &palette(cx)))
}

/// What could not be read, in place of rows; "Refresh" reads it again.
fn failed_row(error: &BridgeError, words: Vec<SharedString>) -> Row {
    let error = error.clone();
    Row::custom(words, move |_, cx| {
        controls::failed("settings-unread", &error, &palette(cx))
    })
}

/// One account: where it is in its sentence, and at the right whether it is signed in, with "Sign in"
/// for an account of the boxes that is not.
fn account_row(account: &Account, words: &[SharedString], cx: &Context<Settings>) -> Row {
    let place = match account.place {
        Place::Box => "box",
        Place::Mac => "mac",
    };
    let id = format!("settings-account-{place}-{}", account.name);
    let description = match account.place {
        Place::Box => tr(Key("settings.accounts.box.description")),
        Place::Mac => tr(Key("settings.accounts.mac.description")),
    };
    let state = match (account.place, account.is_usable()) {
        (Place::Mac, true) => Key("settings.accounts.mac.login"),
        (Place::Mac, false) => Key("settings.accounts.mac.missing"),
        (Place::Box, true) => Key("settings.accounts.signed_in"),
        (Place::Box, false) => Key("settings.accounts.signed_out"),
    };
    let sign_in = (account.place == Place::Box && !account.is_usable()).then(|| {
        let name = account.name.clone();
        Settings::on(cx, move |_, _, cx| {
            cx.emit(SettingsEvent::SignInAsked {
                account: name.clone(),
            });
        })
    });
    // Each account's button has an id of its own: the kit keeps pressed and hovered state by id.
    let button_id = format!("settings-sign-in-{}", account.name);
    let control = move |_: &mut Window, cx: &mut gpui::App| {
        let sign_in = sign_in.clone().map(|ask| {
            let label = tr(Key("settings.accounts.sign_in"));
            controls::button(button_id.clone(), label, ask, cx)
        });
        let state = div()
            .flex()
            .items_center()
            .gap(theme::GAP_WIDE)
            .child(controls::quiet(tr(state), &palette(cx)))
            .children(sign_in);
        marked(id.clone(), state)
    };
    Row::setting(account.name.clone(), description, words.to_vec(), control)
}

/// The brand's page: the product's name, its version and where it comes from.
fn about_group() -> Group {
    let words = keywords(Key("settings.about.keywords"));
    let about = Row::custom(words, |_, cx| brand::about_page(&palette(cx)));
    Group::new(None, vec![about])
}

/// The field of the request for projects with none of their own, the width of the page; under it,
/// what the service refused of the text last typed there.
fn request_field(
    field: &Entity<TextareaState>,
    refused: Option<&BridgeError>,
    cx: &gpui::App,
) -> AnyElement {
    let palette = palette(cx);
    let box_ = div()
        .w_full()
        .rounded(theme::RADIUS_CONTROL)
        .bg(palette.fill)
        .child(Textarea::new(field).appearance(false).bordered(false));
    let failure = refused
        .map(|error| controls::failed("settings-row-default-request-failed", error, &palette));
    div()
        .debug_selector(|| "settings-row-default-request".into())
        .w_full()
        .flex()
        .flex_col()
        .gap(theme::GAP)
        .child(box_)
        .children(failure)
        .into_any_element()
}

/// What an empty request stands for, in full, and that a project's own request comes first.
fn request_notes(cx: &gpui::App) -> AnyElement {
    let palette = palette(cx);
    let default = say(&publish::default_request(tr(Key("settings.agent.base"))));
    let empty = Said::new("settings.agent.request.empty").text("request", default);
    div()
        .debug_selector(|| "settings-agent-notes".into())
        .flex()
        .flex_col()
        .gap(theme::GAP)
        .child(controls::quiet(say(&empty), &palette))
        .child(controls::quiet(
            tr(Key("settings.agent.request.note")),
            &palette,
        ))
        .into_any_element()
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

/// The tasks in boxes with what each has taken of its budget; one that is over says so in its
/// sentence, and its figures are in the colour of a failure.
fn stored_rows(status: &BoxStatus, words: &[SharedString]) -> Vec<Row> {
    let stored = status
        .tasks
        .iter()
        .filter_map(|task| Some((task.name.clone(), task.disk?)));
    let rows: Vec<Row> = stored
        .take(STORED_MAX)
        .enumerate()
        .map(|(nth, (task, disk))| {
            let description = if disk.over_budget {
                tr(Key("settings.storage.task.over"))
            } else {
                tr(Key("settings.storage.task.description"))
            };
            let (said, is_over) = (disk_said(&disk), disk.over_budget);
            Row::setting(task, description, words.to_vec(), move |_, cx| {
                let palette = palette(cx);
                let colour = if is_over {
                    palette.danger
                } else {
                    palette.text_tertiary
                };
                let figure = text(sizes::SETTING_TEXT, colour).child(said.clone());
                marked(format!("settings-storage-task-{nth}"), figure)
            })
        })
        .collect();
    if rows.is_empty() {
        return vec![quiet_row(words.to_vec(), tr(Key("settings.storage.none")))];
    }
    rows
}

/// The runner of the checks: whether it runs, what it runs now, how many wait.
fn runner_rows(status: &BoxStatus, words: &[SharedString]) -> Vec<Row> {
    let said = |id: &'static str, name: Key, description: Key, value: SharedString| {
        Row::setting(tr(name), tr(description), words.to_vec(), move |_, cx| {
            marked(id, controls::quiet(value.clone(), &palette(cx)))
        })
    };
    let state = match status.runner.state {
        RunnerPhase::Running => Key("settings.machine.runner.running"),
        RunnerPhase::Stopped => Key("settings.machine.runner.stopped"),
        RunnerPhase::NotInstalled => Key("settings.machine.runner.not_installed"),
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
    vec![
        said(
            "settings-machine-runner",
            Key("settings.machine.runner"),
            Key("settings.machine.runner.description"),
            tr(state).into(),
        ),
        said(
            "settings-machine-now",
            Key("settings.machine.now"),
            Key("settings.machine.now.description"),
            now,
        ),
        said(
            "settings-machine-queue",
            Key("settings.machine.queue"),
            Key("settings.machine.queue.description"),
            crate::l10n::grouped(waiting).into(),
        ),
    ]
}

/// The agents that run in boxes now: the account each works as, and the processor and the memory
/// it takes.
fn agent_rows(status: &BoxStatus, words: &[SharedString]) -> Vec<Row> {
    let running = status.tasks.iter().filter_map(|task| {
        let agent = task.agent.as_ref()?;
        (agent.state == AgentPhase::Running).then_some((task.name.clone(), agent))
    });
    let rows: Vec<Row> = running
        .take(AGENTS_MAX)
        .enumerate()
        .map(|(nth, (task, agent))| {
            let account =
                Said::new("settings.machine.agent.account").text("account", agent.account.clone());
            let mut uses = Said::new("settings.machine.agent");
            uses = uses.tenths("cpu", agent.cpu_percent.unwrap_or(0.));
            uses = uses.number("memory", agent.memory_mb.unwrap_or(0));
            let uses = say(&uses);
            Row::setting(task, say(&account), words.to_vec(), move |_, cx| {
                let id = format!("settings-machine-agent-{nth}");
                marked(id, controls::quiet(uses.clone(), &palette(cx)))
            })
        })
        .collect();
    if rows.is_empty() {
        return vec![quiet_row(
            words.to_vec(),
            tr(Key("settings.machine.agents.none")),
        )];
    }
    rows
}
