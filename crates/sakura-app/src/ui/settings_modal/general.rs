//! "General": the language and the theme, what keeps the lanes and the master running without the window,
//! the path of the engine's command and the version.

use super::controls::{self, Choice};
use super::group::{group, row, row_acting, with_default};
use super::{login_item, SettingsModal};
use crate::data::source::Source;
use crate::l10n::{tr, tr_with, Key, Language, LanguageChoice};
use crate::prefs::Preferences;
use crate::theme::settings_modal as sizes;
use crate::theme::{self, Appearance, Palette, Theme};
use crate::ui::engine_acts::{self, on, KEEPER_SERVICE, SET_SERVICE};
use crate::ui::kit::{segments, toggle_with_label};
use gpui::{
    div, AnyElement, App, AppContext as _, Context, InteractiveElement as _, ParentElement as _,
    SharedString, Styled as _, Window,
};
use std::future::Future;
use std::path::PathBuf;
use std::rc::Rc;
use tokio::sync::oneshot;

/// The key the keeper switch's press is kept under.
const KEEPER_PRESS: &str = "settings-keeper";
/// The themes the segments choose between, in their order: the Mac's own, dark, light.
const THEMES: [Option<Appearance>; 3] = [None, Some(Appearance::Dark), Some(Appearance::Light)];

#[derive(Default)]
struct SettingsWrites {
    path: Option<oneshot::Receiver<()>>,
    login: Option<oneshot::Receiver<()>>,
}

impl gpui::Global for SettingsWrites {}

fn queue_write(
    slot: &mut Option<oneshot::Receiver<()>>,
) -> impl Future<Output = oneshot::Sender<()>> {
    let (finished, waiting) = oneshot::channel();
    let previous = slot.replace(waiting);
    async move {
        if let Some(previous) = previous {
            let _ = previous.await;
        }
        finished
    }
}

fn system_language_label() -> SharedString {
    tr_with(
        Key("settings.appearance.language.system"),
        &[("language", crate::l10n::language().name())],
    )
}

impl SettingsModal {
    pub(super) fn read_general(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.source.clone();
        let current = source.command();
        let discover =
            cx.background_spawn(async move { current.or_else(|| source.default_command()) });
        cx.spawn_in(window, async move |modal, cx| {
            let Some(found) = discover.await else { return };
            drop(modal.update_in(cx, |modal, window, cx| {
                if modal.open.is_some() {
                    modal.path.update(cx, |field, cx| {
                        field.set_placeholder(found.display().to_string(), window, cx)
                    });
                }
            }));
        })
        .detach();
        self.run_login_task(None, cx);
    }

    /// The groups of "General".
    pub(super) fn general(
        &mut self,
        palette: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let look = vec![
            self.language_row(palette, cx),
            self.theme_row(palette, cx),
            self.accent_row(palette, cx),
            self.animated_pixels_row(palette, cx),
            self.login_row(palette, cx),
        ];
        let engine = vec![
            self.engine_row(palette, window, cx),
            self.version_row(palette, cx),
        ];
        let background = vec![self.keeper_row(palette, cx), self.master_row(palette, cx)];
        let new_master = vec![self.new_master_row(palette, cx)];
        [look, engine, background, new_master]
            .into_iter()
            .map(|rows| group(rows, palette))
            .collect()
    }

    fn language_row(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let current = crate::language::choice(cx);
        let this = cx.entity().downgrade();
        let choice = |label: SharedString, asked: LanguageChoice| {
            let this = this.clone();
            Choice {
                label,
                is_on: current == asked,
                on_pick: Rc::new(move |_, cx: &mut App| {
                    crate::language::set_choice(asked, cx);
                    drop(this.update(cx, |_, cx| cx.notify()));
                }),
            }
        };
        let automatic = system_language_label();
        let mut choices = vec![choice(automatic.clone(), LanguageChoice::Automatic)];
        choices.extend(
            Language::ALL
                .map(|language| choice(language.name().into(), LanguageChoice::Is(language))),
        );
        let shown = match current {
            LanguageChoice::Automatic => automatic,
            LanguageChoice::Is(language) => SharedString::from(language.name()),
        };
        let menu = controls::menu("settings-language", shown, choices, cx);
        let is_changed = current != LanguageChoice::Automatic;
        let reset_to = this.clone();
        let reset = Rc::new(move |_: &mut gpui::Window, cx: &mut App| {
            crate::language::set_choice(LanguageChoice::Automatic, cx);
            drop(reset_to.update(cx, |_, cx| cx.notify()));
        });
        let control = with_default("settings-language", is_changed, reset, menu, palette);
        let name = tr(Key("settings.appearance.language"));
        row("settings-row-language", name, None, control, None, palette)
    }

    fn theme_row(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let chosen = Preferences::current(cx).appearance;
        let words = [
            Key("settings.appearance.theme.system"),
            Key("settings.appearance.theme.dark"),
            Key("settings.appearance.theme.light"),
        ];
        let options: Vec<(SharedString, bool)> = words
            .into_iter()
            .zip(THEMES)
            .map(|(word, theme)| (tr(word).into(), chosen == theme))
            .collect();
        let this = cx.entity().downgrade();
        let reset_to = this.clone();
        let track = segments("settings-theme", &options, move |nth, _, cx| {
            choose_theme(THEMES[nth], cx);
            drop(this.update(cx, |_, cx| cx.notify()));
        });
        let reset = Rc::new(move |_: &mut gpui::Window, cx: &mut App| {
            choose_theme(None, cx);
            drop(reset_to.update(cx, |_, cx| cx.notify()));
        });
        let control = with_default("settings-theme", chosen.is_some(), reset, track, palette);
        let name = tr(Key("settings.appearance.theme"));
        row("settings-row-theme", name, None, control, None, palette)
    }

    fn animated_pixels_row(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = "settings-animated-pixels";
        let is_on = Preferences::is_retino_animated(cx);
        let this = cx.entity().downgrade();
        let reset_to = this.clone();
        let (switch, target) = toggle_with_label(id, is_on, move |asked, _, cx| {
            Preferences::change(cx, |preferences| preferences.is_retino_animated = asked);
            drop(this.update(cx, |_, cx| cx.notify()));
        });
        let reset = Rc::new(move |_: &mut Window, cx: &mut App| {
            Preferences::change(cx, |preferences| preferences.is_retino_animated = true);
            drop(reset_to.update(cx, |_, cx| cx.notify()));
        });
        let control = with_default(id, !is_on, reset, switch, palette);
        let name = tr(Key("settings.appearance.animated_pixels"));
        row_acting(
            "settings-row-animated-pixels",
            name,
            None,
            control,
            (None, target),
            palette,
        )
    }

    fn login_row(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let is_on = self.login_enabled;
        let this = cx.entity().downgrade();
        let reset_to = this.clone();
        let (switch, target) = toggle_with_label("settings-login", is_on, move |asked, _, cx| {
            drop(this.update(cx, |modal, cx| modal.set_login(asked, cx)));
        });
        let reset = Rc::new(move |_: &mut gpui::Window, cx: &mut App| {
            drop(reset_to.update(cx, |modal, cx| modal.set_login(false, cx)));
        });
        let control = with_default("settings-login", is_on, reset, switch, palette);
        let name = tr(Key("settings_modal.general.login"));
        row_acting(
            "settings-row-login",
            name,
            None,
            control,
            (None, target),
            palette,
        )
    }

    fn set_login(&mut self, asked: bool, cx: &mut Context<Self>) {
        if self.open.is_none() {
            return;
        }
        self.login_enabled = asked;
        self.run_login_task(Some(asked), cx);
        cx.notify();
    }

    fn run_login_task(&mut self, asked: Option<bool>, cx: &mut Context<Self>) {
        self.login_request += 1;
        let request = self.login_request;
        let home = self.home.clone();
        let turn = queue_write(&mut cx.default_global::<SettingsWrites>().login);
        cx.spawn(async move |modal, cx| {
            let _finished = turn.await;
            let enabled = cx
                .background_executor()
                .spawn(async move {
                    if let Some(asked) = asked {
                        if let Err(error) = login_item::set_enabled(&home, asked) {
                            eprintln!("sakura: the login item could not be changed: {error}");
                        }
                    }
                    login_item::is_enabled(&home)
                })
                .await;
            drop(modal.update(cx, |modal, cx| {
                if modal.open.is_some() && modal.login_request == request {
                    modal.login_enabled = enabled;
                    cx.notify();
                }
            }));
        })
        .detach();
    }

    fn engine_row(&self, palette: &Palette, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let failure = self
            .refused
            .as_ref()
            .map(|error| controls::failed("settings-engine-failed", error, palette));
        let is_written = Preferences::current(cx).engine_bin.is_some();
        let in_use = self.source.command().map(|path| path.display().to_string());
        let field = self.path_field(in_use.as_deref(), palette, window, cx);
        let editor = div()
            .id("settings-engine")
            .debug_selector(|| "settings-engine".into())
            .flex()
            .items_center()
            .gap(theme::GAP_SMALL)
            .child(field)
            .children(in_use.map(|path| {
                crate::ui::kit::reveal_path_icon("settings-engine-location", path, cx)
            }));
        let (this, path) = (cx.entity().downgrade(), self.path.clone());
        let reset = Rc::new(move |window: &mut gpui::Window, cx: &mut App| {
            path.update(cx, |field, cx| field.set_value("", window, cx));
            drop(this.update(cx, |modal, cx| modal.save_engine_path(cx)));
        });
        let control = with_default("settings-engine", is_written, reset, editor, palette);
        let note = tr(Key("settings_modal.general.engine_note")).into();
        let name = tr(Key("settings.engine.title"));
        let target = crate::ui::kit::LabelTarget::Focus(gpui::Focusable::focus_handle(
            self.path.read(cx),
            cx,
        ));
        row_acting(
            "settings-row-engine",
            name,
            Some(note),
            control,
            (failure, target),
            palette,
        )
    }

    fn path_field(
        &self,
        path: Option<&str>,
        palette: &Palette,
        window: &Window,
        cx: &App,
    ) -> AnyElement {
        use gpui::Focusable as _;
        let width = sizes::FIELD_WIDTH - theme::CONTROL_HEIGHT - theme::GAP_SMALL;
        let input = controls::field(&self.path, width, palette).relative();
        let Some(path) = path.filter(|_| !self.path.read(cx).focus_handle(cx).is_focused(window))
        else {
            use gpui::IntoElement as _;
            return input.into_any_element();
        };
        let label = crate::ui::kit::path_label(
            "settings-engine-preview",
            path,
            (palette.text, palette.text_secondary),
        )
        .absolute()
        .inset_0()
        .items_center()
        .px(theme::GAP_SMALL)
        .rounded(theme::RADIUS_CONTROL)
        .bg(palette.fill);
        let input = input.id("settings-engine-field").child(label);
        use gpui::IntoElement as _;
        crate::ui::tip::bind_path(
            input,
            "settings-engine-tooltip",
            crate::ui::tip::Words::plain(path.to_string()),
            cx,
        )
        .into_any_element()
    }

    fn version_row(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let status = crate::updates::status(cx);
        let note = (status.state != crate::updates::UpdateState::Ready)
            .then(|| version_note(&status.state).into());
        let ids = ("settings-check-updates", "settings-version-restart");
        let check =
            super::updates::update_control(ids, Key("settings_modal.general.check_updates"), cx);
        let name = tr(Key("updates.current_version"));
        let control = div()
            .flex()
            .items_center()
            .gap(theme::GAP)
            .child(
                div()
                    .debug_selector(|| "settings-installed-version".into())
                    .child(crate::updates::CURRENT_VERSION),
            )
            .child(check);
        row("settings-row-version", name, note, control, None, palette)
    }

    fn keeper_row(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let keeper = self
            .snapshot(cx)
            .and_then(|snapshot| snapshot.engine.as_ref())
            .and_then(|engine| engine.keeper.as_ref())
            .map(|keeper| keeper.on);
        let name = tr(Key("settings_modal.general.keeper"));
        let Some(is_on) = keeper else {
            let note = tr(Key("settings_modal.general.keeper_unreported")).into();
            return row(
                "settings-row-keeper",
                name,
                Some(note),
                div(),
                None,
                palette,
            );
        };
        let acts = self.acts.clone();
        let (switch, target) = toggle_with_label("settings-keeper", is_on, move |asked, _, cx| {
            let act = SET_SERVICE.act(&[("name", KEEPER_SERVICE), ("on", on(asked))]);
            acts.update(cx, |acts, cx| acts.press(KEEPER_PRESS, act, "", cx));
        });
        let note = tr(Key("settings_modal.general.keeper_note")).into();
        let told = engine_acts::line(&self.acts, KEEPER_PRESS, palette, cx);
        row_acting(
            "settings-row-keeper",
            name,
            Some(note),
            switch,
            (told, target),
            palette,
        )
    }

    fn master_row(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let is_on = Preferences::current(cx).is_master_restored;
        let this = cx.entity().downgrade();
        let reset_to = this.clone();
        let (switch, target) =
            toggle_with_label("settings-master-restore", is_on, move |asked, _, cx| {
                Preferences::change(cx, |preferences| preferences.is_master_restored = asked);
                drop(this.update(cx, |_, cx| cx.notify()));
            });
        let reset = Rc::new(move |_: &mut gpui::Window, cx: &mut App| {
            Preferences::change(cx, |preferences| preferences.is_master_restored = true);
            drop(reset_to.update(cx, |_, cx| cx.notify()));
        });
        let control = with_default("settings-master-restore", !is_on, reset, switch, palette);
        let name = tr(Key("settings_modal.general.master_restore"));
        row_acting(
            "settings-row-master-restore",
            name,
            None,
            control,
            (None, target),
            palette,
        )
    }

    pub(super) fn save_engine_path(&mut self, cx: &mut Context<Self>) {
        if self.open.is_none() {
            return;
        }
        let written = self.path.read(cx).value().trim().to_string();
        if Preferences::current(cx).engine_bin.is_none()
            && self
                .source
                .command()
                .is_some_and(|path| path.to_string_lossy() == written)
        {
            return;
        }
        let configured = (!written.is_empty()).then_some(written);
        let path = configured.as_ref().map(PathBuf::from);
        let (source, facts) = (self.source.clone(), self.facts.clone());
        let turn = queue_write(&mut cx.default_global::<SettingsWrites>().path);
        cx.spawn(async move |modal, cx| {
            let _finished = turn.await;
            let found = cx
                .background_executor()
                .spawn(async move { Source::locate(path) })
                .await;
            let feedback = cx.update(|cx| {
                Preferences::change(cx, move |preferences| preferences.engine_bin = configured);
                let refused = source.replace_command(found).err();
                facts.update(cx, |facts, cx| facts.read(cx));
                refused
            });
            drop(modal.update(cx, |modal, cx| {
                if modal.open.is_some() {
                    modal.refused = feedback;
                    cx.notify();
                }
            }));
        })
        .detach();
    }
}

/// The appearance chosen is kept and in force at once; `None` is the Mac's own.
fn choose_theme(appearance: Option<Appearance>, cx: &mut App) {
    Preferences::change(cx, |preferences| preferences.appearance = appearance);
    let shown = appearance.unwrap_or_else(|| Appearance::from_window(cx.window_appearance()));
    Theme::set(shown, cx);
    cx.refresh_windows();
}

/// What the version row says under its name: the version running, until a check has something to say.
fn version_note(state: &crate::updates::UpdateState) -> String {
    match state {
        crate::updates::UpdateState::Ready => tr_with(
            Key("settings_modal.general.version_note"),
            &[("version", crate::updates::CURRENT_VERSION)],
        )
        .to_string(),
        other => crate::updates::sentence(other),
    }
}

#[cfg(test)]
mod tests {
    use super::version_note;
    use crate::updates::{Release, UpdateState};

    #[gpui::test]
    fn queued_launch_at_login_toggle_survives_settings_close(cx: &mut gpui::TestAppContext) {
        use super::super::tests::open_alone;
        use crate::ui::engine_rig_tests::{wait_until, SNAPSHOT};

        let home = tempfile::tempdir().unwrap();
        let (_rig, modal, cx) = open_alone(cx, SNAPSHOT, home.path().to_path_buf());
        modal.update(cx, |modal, cx| {
            modal.set_login(false, cx);
            modal.set_login(true, cx);
            modal.dismiss(cx);
        });
        wait_until(cx, "the queued toggle is written after dismissal", |_| {
            super::login_item::is_enabled(home.path())
        });
    }

    #[test]
    fn the_version_row_says_the_version_until_a_check_has_something_to_say() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let version = crate::updates::CURRENT_VERSION;
        assert_eq!(
            version_note(&UpdateState::Ready),
            format!("Sakura {version}")
        );
        assert_eq!(version_note(&UpdateState::Checking), "Checking…");
        let latest = format!("Sakura {version} is the latest version");
        assert_eq!(version_note(&UpdateState::UpToDate), latest);
        let found = UpdateState::Available(Release {
            version: "999.0.0".into(),
            url: String::new(),
        });
        assert_eq!(version_note(&found), "Sakura 999.0.0 is available");
        assert!(!version_note(&UpdateState::Failed).contains("://"));
    }
}

#[test]
fn the_system_language_label_names_the_language_in_use() {
    crate::l10n::set_for_test(Language::English);
    assert_eq!(system_language_label(), "English, as the system");
    crate::l10n::set_for_test(Language::Italian);
    assert_eq!(system_language_label(), "Italiano, come il sistema");
}
