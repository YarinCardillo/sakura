//! The model and the mode of the master's session: two chips in the composer, each shown only when the agent
//! offers more than one value, each a menu of what it offers. The model is a config option of the session
//! (`session/set_config_option`); the mode is the session's own (`session/set_mode`), or a config option when
//! the agent offers it so.

use super::agent_menu::{chip_groups, grouped_chip_with_footer, OnPick};
use super::master::MenuPick;
use super::master_composer::choice_chip;
use super::{copy, Phase, ThreadView};
use crate::l10n::{tr, Key};
use crate::theme::Theme;
use crate::ui::kit::{arriving, chip_placeholder};
use gpui::{
    div, App, Bounds, Context, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
    ParentElement as _, Pixels, PlatformInput, Window,
};
use sakura_acp::model::Selector;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// The id of the agent chip: the one chip that holds the agent, its model and the mode, in one menu, for the
/// whole conversation.
pub(super) const AGENT_CHIP: &str = "master-agent-chip";
/// What the agent chip is while it has no name to say.
const AGENT_PLACEHOLDER: &str = "master-agent-placeholder";

/// Where each chip of the composer was last drawn, by the chip's id.
#[derive(Clone, Default)]
pub(super) struct ChipFrames(Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>);

/// The modes the master may be put in. The engine decides every permission the master asks and the master
/// never edits code or approves, so the modes that stop asking (accepting edits, bypassing permissions) are
/// not offered here, whatever the agent offers.
const MASTER_MODES: [&str; 2] = ["default", "plan"];

/// What a chip's pick sets.
#[derive(Clone, Copy)]
enum Which {
    Model,
    Mode,
    Effort,
}

impl ThreadView {
    /// The agent chip: the real name of what runs (or will run), and one menu with Claude's models, Codex's
    /// models and the mode, before the first message and after it.
    pub(super) fn agent_chip(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let mut menu = self.agent_menu()?;
        let view = cx.entity().downgrade();
        let mut groups = chip_groups(&menu.groups, |pick| pick_entry(&view, pick.clone()));
        let footer = self
            .master
            .as_ref()?
            .world
            .accounts(cx)
            .map(|(fallback, accounts, add)| {
                if menu.name.is_empty() {
                    menu.name = fallback;
                }
                if !accounts.choices.is_empty() {
                    groups.push(accounts);
                }
                add
            });
        let chip = match menu.name.is_empty() {
            true => chip_placeholder(AGENT_PLACEHOLDER, Theme::palette(cx).fill_hover)
                .into_any_element(),
            false => arriving(
                "master-agent-arrives",
                grouped_chip_with_footer(AGENT_CHIP, menu.name, groups, footer, None, cx),
                cx,
            ),
        };
        let frames = self.chip_frames.clone();
        Some(
            div()
                .on_children_prepainted(move |drawn, _, _cx| {
                    if let Some(bounds) = drawn.first() {
                        frames.0.borrow_mut().insert(AGENT_CHIP, *bounds);
                        #[cfg(not(feature = "boxes"))]
                        crate::ui::tour::track_master_chip(*bounds, _cx);
                    }
                })
                .child(chip),
        )
    }

    pub(in crate::ui) fn agent_menu(&self) -> Option<super::master::AgentMenu> {
        let kit = self.master.as_ref()?;
        let empty = Default::default();
        let choices = if self.has_opened_session {
            &self.choices
        } else {
            &empty
        };
        Some((kit.menu)(choices))
    }

    /// Whether the conversation has no session yet: what is picked now is what its first session starts with.
    fn is_before_start(&self) -> bool {
        self.master.is_some()
            && !self.has_opened_session
            && matches!(self.phase, Phase::Idle | Phase::Ended(_) | Phase::SignIn)
    }

    /// Chooses what an entry of the agent chip's menu says: a model, an effort, an option the agent sends, or
    /// the mode. Before the session it is noted for the first one; after it, the session's own is set, so the
    /// session and its history stay as they are.
    pub fn pick_entry(&mut self, pick: &MenuPick, cx: &mut Context<Self>) {
        match pick {
            MenuPick::Model(id) => self.pick(id, Which::Model, cx),
            MenuPick::Effort(id) => self.pick(id, Which::Effort, cx),
            MenuPick::Option { config_id, value } => self.pick_option(config_id, value, cx),
            MenuPick::Mode(id) => self.pick_mode(id, cx),
        }
    }

    /// Sets the option `config_id` of the agent to `value`.
    fn pick_option(&mut self, config_id: &str, value: &str, cx: &mut Context<Self>) {
        if config_id == super::LOGIN_OPTION {
            return cx.emit(super::ThreadViewEvent::CodexLoginRequested);
        }
        let (config, value) = (config_id.to_string(), value.to_string());
        self.tell_agent(|agent| agent.set_option(config, value));
        cx.notify();
    }

    /// Puts the session on the model `id`.
    pub fn pick_model(&mut self, id: &str, cx: &mut Context<Self>) {
        self.pick(id, Which::Model, cx);
    }

    /// Puts the session in the mode `id`, when the master may be in it.
    pub fn pick_mode(&mut self, id: &str, cx: &mut Context<Self>) {
        if MASTER_MODES.contains(&id) {
            self.pick(id, Which::Mode, cx);
        }
    }

    /// Notes what the person picked before the first message: the first session starts with it.
    fn pick_for_start(&mut self, id: &str, which: Which, cx: &mut Context<Self>) {
        let Some(kit) = self.master.as_ref() else {
            return;
        };
        let id = id.to_string();
        kit.start.update(|start| match which {
            Which::Model => {
                let model = Some(id).filter(|id| !id.is_empty());
                if start.model != model {
                    start.effort = None;
                }
                start.model = model;
            }
            Which::Mode => start.mode = Some(id),
            Which::Effort => {
                start.uses_session_effort = id == "default";
                start.effort = Some(id).filter(|id| !id.is_empty());
            }
        });
        cx.notify();
    }

    fn pick(&mut self, id: &str, which: Which, cx: &mut Context<Self>) {
        if self.is_before_start() {
            return self.pick_for_start(id, which, cx);
        }
        let selector = match which {
            Which::Model => self.choices.models.as_ref(),
            Which::Mode => self.choices.modes.as_ref(),
            Which::Effort => self.choices.efforts.as_ref(),
        };
        let Some(selector) = selector else {
            return;
        };
        let id = id.to_string();
        match selector.config_id.clone() {
            Some(config) => self.tell_agent(|agent| agent.set_option(config, id)),
            None => self.tell_agent(|agent| agent.set_mode(id)),
        }
        cx.notify();
    }

    /// Whether the conversation has the agent chip, which `/model` and `/mode` open.
    pub(super) fn has_agent_chip(&self) -> bool {
        self.master.is_some()
    }

    /// Opens the menu of the chip `id` as a press on it does, once the window is done with what it is
    /// doing now. Nothing happens when the chip has not been drawn.
    pub(super) fn open_chip(&self, id: &'static str, window: &mut Window, cx: &mut App) {
        let Some(bounds) = self.chip_frames.0.borrow().get(id).copied() else {
            return;
        };
        let press = MouseDownEvent {
            button: MouseButton::Left,
            position: bounds.center(),
            modifiers: gpui::Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        };
        window.defer(cx, move |window, cx| {
            window.dispatch_event(PlatformInput::MouseDown(press), cx);
        });
    }

    /// The models, efforts, modes and other options the session offers.
    pub fn session_choices(&self) -> &sakura_acp::model::SessionChoices {
        &self.choices
    }
}

/// What choosing `pick` in the agent chip's menu does.
fn pick_entry(view: &gpui::WeakEntity<ThreadView>, pick: MenuPick) -> OnPick {
    let view = view.clone();
    Rc::new(move |_: &mut Window, cx: &mut App| {
        let pick = pick.clone();
        view.update(cx, |view, cx| view.pick_entry(&pick, cx)).ok();
    })
}
