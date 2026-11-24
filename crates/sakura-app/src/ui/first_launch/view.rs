use super::{Login, Mode, Setup, Stage, STANDARD};
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette, Theme};
use crate::ui::dialog_frame;
use crate::ui::kit::{self, ButtonKind};
use crate::ui::lane_paces::{pace_words, LANE_PACES};
use crate::ui::master_agent::MasterAgent;
use crate::ui::settings_modal::controls::{self, Choice};
use gpui::{
    div, AnyElement, Context, Focusable as _, FontWeight, InteractiveElement as _,
    IntoElement as _, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window,
};
use gpui_component::{
    checkbox::Checkbox,
    input::{Escape, Input},
    Disableable as _,
};
use sakura_core::engine::presets::PresetChoice;
use std::rc::Rc;

type Press = fn(&mut Setup, &mut Window, &mut Context<Setup>);

impl Render for Setup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let palette = Theme::palette(cx);
        let content = match self.stage {
            Stage::Ask => self.ask(&palette, cx),
            Stage::Searching => {
                sentence("setup-searching", Key("first_launch.searching"), &palette)
            }
            Stage::SearchFailed => self.failure(
                Key("first_launch.search_failed"),
                Self::search,
                &palette,
                cx,
            ),
            Stage::Review if self.programs.is_empty() && self.logins.is_empty() => {
                self.empty(&palette, cx)
            }
            Stage::Review => self.review(&palette, cx),
            Stage::Applying => sentence("setup-applying", Key("first_launch.applying"), &palette),
            Stage::ApplyFailed(key) => self.failure(key, Self::apply, &palette, cx),
        };
        dialog_frame::surface("first-launch", window, &palette)
            .track_focus(&self.focus)
            .capture_action(cx.listener(|setup, _: &Escape, window, cx| {
                cx.stop_propagation();
                setup.close(window, cx);
            }))
            .child(dialog_frame::heading_with_close(
                "setup-title",
                tr(Key("first_launch.title")),
                &palette,
                self.stage != Stage::Applying,
            ))
            .child(content)
    }
}

impl Setup {
    fn ask(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        column()
            .child(sentence("setup-ask", Key("first_launch.ask"), palette))
            .child(
                dialog_frame::footer(palette)
                    .child(self.button(
                        "setup-not-now",
                        Key("common.cancel"),
                        ButtonKind::Secondary,
                        Self::close,
                        palette,
                        cx,
                    ))
                    .child(self.button(
                        "setup-look",
                        Key("first_launch.look"),
                        ButtonKind::Primary,
                        Self::search,
                        palette,
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn empty(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        column()
            .child(sentence("setup-empty", Key("first_launch.empty"), palette))
            .child(
                dialog_frame::footer(palette)
                    .child(self.login_button(
                        "setup-sign-in-claude",
                        Key("first_launch.login_claude"),
                        MasterAgent::Claude,
                        None,
                        palette,
                        cx,
                    ))
                    .child(self.login_button(
                        "setup-sign-in-codex",
                        Key("first_launch.login_codex"),
                        MasterAgent::Codex,
                        None,
                        palette,
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn failure(
        &self,
        key: Key,
        retry: Press,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        column()
            .child(sentence("setup-error", key, palette))
            .child(
                dialog_frame::footer(palette)
                    .child(self.button(
                        "setup-cancel",
                        Key("common.cancel"),
                        ButtonKind::Secondary,
                        Self::close,
                        palette,
                        cx,
                    ))
                    .child(self.button(
                        "setup-retry",
                        Key("first_launch.retry"),
                        ButtonKind::Primary,
                        retry,
                        palette,
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_component::WindowExt as _;
        self.dismiss(cx);
        window.close_dialog(cx);
    }

    fn review(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let programs = self.program_rows(palette, cx);
        let accounts: Vec<_> = self
            .logins
            .iter()
            .enumerate()
            .map(|(at, login)| self.login_row(at, login, palette, cx))
            .collect();
        let groups = column()
            .flex_none()
            .children((!programs.is_empty()).then(|| {
                dialog_frame::group(
                    "setup-programs-title",
                    tr(Key("first_launch.programs")),
                    programs,
                    palette,
                )
            }))
            .children((!accounts.is_empty()).then(|| {
                dialog_frame::group(
                    "setup-accounts-title",
                    tr(Key("first_launch.accounts")),
                    accounts,
                    palette,
                )
            }))
            .child(dialog_frame::group(
                "setup-defaults-title",
                tr(Key("first_launch.defaults")),
                self.default_rows(palette, cx),
                palette,
            ));
        column()
            .min_h_0()
            .child(
                div()
                    .debug_selector(|| "setup-intro".into())
                    .flex_none()
                    .text_size(theme::TEXT_SMALL)
                    .text_color(palette.text_secondary)
                    .child(tr(Key("first_launch.review"))),
            )
            .child(
                div()
                    .relative()
                    .flex_auto()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        dialog_frame::scrolling_body("setup-review-list")
                            .track_scroll(&self.review_scroll)
                            .child(groups),
                    )
                    .child(crate::ui::page_scroll::edge_fades(
                        &self.review_scroll,
                        palette.overlay_surface,
                    )),
            )
            .child(self.review_buttons(palette, cx))
            .into_any_element()
    }

    fn program_rows(&self, palette: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.programs
            .iter()
            .filter_map(|program| {
                let agent = super::agent(&program.id)?;
                let id = format!("setup-program-{}", program.id);
                let version = program.version.as_deref();
                let state = if agent == MasterAgent::Codex
                    && program.program_path.is_some()
                    && program.reason.as_deref() == Some("install_adapter")
                {
                    None
                } else {
                    program.readiness.map(|state| {
                        super::super::account_words::readiness_words(
                            state,
                            program.reason.as_deref(),
                        )
                    })
                };
                let needs_sign_in = agent != MasterAgent::Goose
                    && !self.logins.iter().any(|login| login.agent == agent);
                Some(
                    div()
                        .debug_selector({
                            let id = id.clone();
                            move || id.clone()
                        })
                        .flex()
                        .items_center()
                        .gap(theme::GAP_SMALL)
                        .px(theme::kit::scale::SETTINGS_ROW_PADDING)
                        .py(theme::GAP_TIGHT)
                        .min_h(theme::CONTROL_HEIGHT + theme::GAP_TIGHT * 2.)
                        .text_size(theme::TEXT_SMALL)
                        .child(div().font_weight(FontWeight::MEDIUM).child(agent.label()))
                        .child(
                            muted(program_detail(version, state), palette).debug_selector({
                                let id = id.clone();
                                move || format!("{id}-detail")
                            }),
                        )
                        .children(needs_sign_in.then(|| {
                            div().ml_auto().child(self.login_button(
                                &format!("{id}-sign-in"),
                                Key("first_launch.log_in"),
                                agent,
                                None,
                                palette,
                                cx,
                            ))
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn review_buttons(&self, palette: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let is_valid = self
            .logins
            .iter()
            .filter(|login| login.is_checked)
            .all(|login| !login.label(cx).trim().is_empty());
        let confirm = if is_valid {
            self.button(
                "setup-confirm",
                Key("first_launch.confirm"),
                ButtonKind::Primary,
                Self::confirm,
                palette,
                cx,
            )
        } else {
            kit::button_disabled(
                "setup-confirm",
                tr(Key("first_launch.confirm")),
                ButtonKind::Primary,
                palette,
            )
        };
        dialog_frame::footer(palette)
            .child(self.button(
                "setup-later",
                Key("common.cancel"),
                ButtonKind::Secondary,
                Self::close,
                palette,
                cx,
            ))
            .child(confirm)
    }

    fn login_row(
        &self,
        at: usize,
        login: &Login,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let registered = login.detected.in_book_as.is_some();
        let label = login.label(cx);
        let note = login_note(login);
        let caption = div()
            .debug_selector(move || format!("setup-login-{at}-note"))
            .text_size(theme::TEXT_LABEL)
            .text_color(palette.text_secondary)
            .whitespace_normal()
            .child(note)
            .children(login.detected.probe_error.as_ref().map(|_| {
                div()
                    .debug_selector(move || format!("setup-login-{at}-probe-error"))
                    .child(tr(Key("first_launch.could_not_check")))
            }));
        let check = (!registered && (login.name.is_some() || login.detected.probe_error.is_some()))
            .then(|| login_checkbox(at, login, cx).into_any_element());
        let provider = login
            .provider
            .as_ref()
            .filter(|_| !registered)
            .map(|input| provider_line(at, input, palette, cx));
        kit::AccountRow {
            check,
            name: login_name(at, login, label, palette, cx),
            caption: Some(caption.into_any_element()),
            actions: self
                .login_actions(at, login, palette, cx)
                .into_iter()
                .collect(),
            notes: provider.into_iter().collect(),
        }
        .layout()
        .debug_selector(move || format!("setup-login-{at}"))
        .px(theme::kit::scale::SETTINGS_ROW_PADDING)
        .py(theme::GAP_SMALL)
        .into_any_element()
    }

    fn login_actions(
        &self,
        at: usize,
        login: &Login,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if login.detected.in_book_as.is_some() {
            return Some(
                muted(tr(Key("first_launch.in_book")).into(), palette)
                    .debug_selector(move || format!("setup-login-{at}-registered"))
                    .into_any_element(),
            );
        }
        let location = kit::reveal_path_icon(
            &format!("setup-login-{at}-location"),
            login.detected.folder.clone(),
            cx,
        );
        Some(
            div()
                .flex()
                .items_center()
                .gap(theme::GAP_SMALL)
                .children(self.login_action(at, login, palette, cx))
                .child(location)
                .into_any_element(),
        )
    }

    fn login_action(
        &self,
        at: usize,
        login: &Login,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let action = if login.agent == MasterAgent::Goose
            && login.detected.readiness
                != Some(sakura_core::engine::accounts::Readiness::Unavailable)
        {
            ("configure", Key("goose.configure"))
        } else if login.agent != MasterAgent::Goose
            && login.name.is_none()
            && login.detected.probe_error.is_none()
        {
            ("sign-in", Key("first_launch.log_in"))
        } else {
            return None;
        };
        Some(self.login_button(
            &format!("setup-login-{at}-{}", action.0),
            action.1,
            login.agent,
            Some(at),
            palette,
            cx,
        ))
    }

    fn master_row(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let control = match self.master() {
            Some(at) => {
                let choices = self
                    .logins
                    .iter()
                    .enumerate()
                    .filter(|(_, login)| login.can_be_master())
                    .map(|(index, login)| {
                        let setup = cx.weak_entity();
                        Choice {
                            label: login.label(cx),
                            is_on: at == index,
                            on_pick: Rc::new(move |_, cx| {
                                drop(setup.update(cx, |setup, cx| {
                                    setup.chosen_master = Some(index);
                                    cx.notify();
                                }))
                            }),
                        }
                    })
                    .collect();
                controls::menu(
                    format!("setup-master-{at}"),
                    self.logins[at].label(cx),
                    choices,
                    cx,
                )
            }
            None => div()
                .debug_selector(|| "setup-master-unavailable".into())
                .child(tr(Key("first_launch.no_master_login")))
                .into_any_element(),
        };
        default_row("setup-master", Key("first_launch.master"), control, palette)
    }

    fn default_rows(&self, palette: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let engine = self
            .facts
            .read(cx)
            .snapshot()
            .and_then(|snapshot| snapshot.engine.as_ref());
        let current = engine.filter(|_| self.mode == Mode::LookAgain);
        let presets = engine.map_or(&[][..], |engine| engine.presets.as_slice());
        let preset = self.preset_menu(
            presets,
            current.and_then(|engine| engine.preset.as_deref()),
            cx,
        );
        let pace = self.pace_menu(current.and_then(|engine| engine.pace.as_deref()), cx);
        vec![
            self.master_row(palette, cx),
            default_row(
                "setup-preset-row",
                Key("first_launch.preset"),
                preset,
                palette,
            ),
            default_row("setup-pace-row", Key("first_launch.pace"), pace, palette),
        ]
    }

    fn preset_menu(
        &self,
        presets: &[PresetChoice],
        baseline: Option<&str>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let selected = self.preset.as_deref().or(baseline);
        let choices = presets
            .iter()
            .map(|preset| (preset.name.clone(), preset_words(preset)))
            .collect();
        let shown = selected.map_or_else(keep_current, |name| {
            presets
                .iter()
                .find(|preset| preset.name == name)
                .map_or_else(
                    || preset_words(&PresetChoice::named(name, None)),
                    preset_words,
                )
        });
        choice_menu(
            "setup-preset",
            selected,
            baseline,
            shown,
            choices,
            |setup| &mut setup.preset,
            cx,
        )
    }

    fn pace_menu(&self, baseline: Option<&str>, cx: &Context<Self>) -> AnyElement {
        let selected = self.pace.as_deref().or(baseline);
        let choices = LANE_PACES
            .iter()
            .map(|(id, key)| (id.to_string(), tr(*key).into()))
            .collect();
        choice_menu(
            "setup-pace",
            selected,
            baseline,
            selected.map_or_else(keep_current, pace_words),
            choices,
            |setup| &mut setup.pace,
            cx,
        )
    }

    fn button(
        &self,
        id: &'static str,
        key: Key,
        kind: ButtonKind,
        press: Press,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        kit::button(id, tr(key), kind, palette)
            .on_click(cx.listener(move |setup, _, window, cx| press(setup, window, cx)))
    }

    fn login_button(
        &self,
        id: &str,
        key: Key,
        agent: MasterAgent,
        login: Option<usize>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        kit::button(id.to_string(), tr(key), ButtonKind::Secondary, palette)
            .on_click(
                cx.listener(move |setup, _, window, cx| setup.log_in(agent, login, window, cx)),
            )
            .into_any_element()
    }
}

fn choice_menu(
    id: &'static str,
    selected: Option<&str>,
    baseline: Option<&str>,
    shown: SharedString,
    choices: Vec<(String, SharedString)>,
    field: fn(&mut Setup) -> &mut Option<String>,
    cx: &Context<Setup>,
) -> AnyElement {
    if choices.is_empty() {
        return div().child(shown).into_any_element();
    }
    let choices = choices
        .into_iter()
        .map(|(id, label)| {
            let setup = cx.weak_entity();
            let is_on = selected == Some(id.as_str());
            let change = (baseline != Some(id.as_str())).then_some(id);
            Choice {
                label,
                is_on,
                on_pick: Rc::new(move |_, cx| {
                    drop(setup.update(cx, |setup, cx| {
                        *field(setup) = change.clone();
                        cx.notify();
                    }))
                }),
            }
        })
        .collect();
    controls::menu(id, shown, choices, cx)
}

fn keep_current() -> SharedString {
    tr(Key("first_launch.keep_current")).into()
}

fn preset_words(preset: &PresetChoice) -> SharedString {
    if preset.name == STANDARD {
        tr(Key("first_launch.standard")).into()
    } else {
        preset.words().into()
    }
}

fn default_row(id: &'static str, label: Key, value: AnyElement, palette: &Palette) -> AnyElement {
    crate::ui::settings_modal::group::row(id, tr(label), None, value, None, palette)
}

fn program_detail(version: Option<&str>, state: Option<&str>) -> SharedString {
    match (version, state) {
        (Some(version), Some(state)) => format!("{version}, {state}").into(),
        (Some(detail), None) | (None, Some(detail)) => detail.to_string().into(),
        (None, None) => tr(Key("first_launch.found")).into(),
    }
}

fn muted(text: SharedString, palette: &Palette) -> gpui::Div {
    div().text_color(palette.text_secondary).child(text)
}

fn login_status(login: &Login) -> SharedString {
    if let Some(state) = login.detected.readiness {
        return super::super::account_words::readiness_words(
            state,
            login.detected.reason.as_deref(),
        )
        .into();
    }
    tr(if login.detected.logged_in {
        Key("first_launch.logged_in")
    } else {
        Key("first_launch.logged_out")
    })
    .into()
}

fn login_note(login: &Login) -> SharedString {
    if login.detected.probe_error.is_some() {
        login.agent.label().into()
    } else {
        let status = login_status(login);
        format!("{}, {}", login.agent.label(), status.to_lowercase()).into()
    }
}

fn login_name(
    at: usize,
    login: &Login,
    label: SharedString,
    palette: &Palette,
    cx: &gpui::App,
) -> AnyElement {
    if let Some(name) = &login.name {
        login_field(at, "name", Key("account_add.name_label"), name, palette, cx).into_any_element()
    } else {
        div()
            .debug_selector(move || format!("setup-login-{at}-static-name"))
            .pl(gpui_component::Size::Medium.input_px())
            .font_weight(FontWeight::MEDIUM)
            .child(label)
            .into_any_element()
    }
}

fn login_field(
    at: usize,
    field: &'static str,
    label: Key,
    input: &gpui::Entity<gpui_component::input::InputState>,
    palette: &Palette,
    cx: &gpui::App,
) -> gpui::Stateful<gpui::Div> {
    let ring = kit::keyboard_ring(palette);
    div()
        .id((SharedString::from(format!("setup-login-{field}")), at))
        .debug_selector(move || format!("setup-login-{at}-{field}"))
        .w_full()
        .rounded(theme::RADIUS_CONTROL)
        .bg(palette.fill)
        .track_focus(&input.focus_handle(cx))
        .focus_visible(move |style| style.shadow(ring))
        .child(
            Input::new(input)
                .text_size(theme::TEXT_SMALL)
                .font_weight(FontWeight::MEDIUM)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .aria_label(tr(label)),
        )
}

fn provider_line(
    at: usize,
    input: &gpui::Entity<gpui_component::input::InputState>,
    palette: &Palette,
    cx: &gpui::App,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(theme::GAP)
        .child(
            div()
                .debug_selector(move || format!("setup-login-{at}-provider-label"))
                .flex_none()
                .text_size(theme::TEXT_SMALL)
                .text_color(palette.text_secondary)
                .child(tr(Key("goose.provider"))),
        )
        .child(div().flex_1().min_w_0().child(login_field(
            at,
            "provider",
            Key("goose.provider"),
            input,
            palette,
            cx,
        )))
        .into_any_element()
}

fn login_checkbox(at: usize, login: &Login, cx: &mut Context<Setup>) -> gpui::Div {
    let toggle = cx.weak_entity();
    let status = if login.is_checked {
        "checked"
    } else {
        "unchecked"
    };
    let selector = format!("setup-login-{at}-{status}");
    let checkbox = Checkbox::new(format!("setup-login-{at}-check"))
        .checked(login.is_checked)
        .disabled(login.detected.probe_error.is_some())
        .accessibility_label(login.detected.proposed_name.clone())
        .on_change(move |checked, _, cx| {
            drop(toggle.update(cx, |setup, cx| setup.check(at, *checked, cx)))
        });
    div().debug_selector(move || selector).child(checkbox)
}

fn column() -> gpui::Div {
    div().flex().flex_col().gap(theme::GAP)
}

fn sentence(id: &'static str, key: Key, palette: &Palette) -> AnyElement {
    div()
        .debug_selector(move || id.into())
        .text_color(palette.text)
        .child(tr(key))
        .into_any_element()
}

#[cfg(test)]
mod login_copy_tests {
    use super::*;

    #[test]
    fn review_login_status_uses_a_comma() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let login = Login {
            detected: serde_json::from_value(serde_json::json!({
                "harness":"codex", "folder":"/fixture", "logged_in":true, "proposed_name":"Codex"
            }))
            .unwrap(),
            agent: MasterAgent::Codex,
            name: None,
            provider: None,
            is_checked: false,
        };
        assert_eq!(
            login_note(&login),
            format!("Codex, {}", tr(Key("first_launch.logged_in")))
        );
    }
}
