//! The phone page's setup, in the background card under its switch: what the engine says of it in plain words,
//! the link as the engine knows it (copyable and openable), who may open it, and a way to allow a device of the
//! person's overlay network or take one away. The engine does all of it (`sakura cockpit phone --json`, and the actions
//! `F` and `Z`); the app never changes Tailscale.

use super::EnginePage;
use crate::l10n::{tr, Key};
use crate::theme::engine_page as sizes;
use crate::theme::{self, Palette};
use crate::ui::engine_acts::{ALLOW_DEVICE, REMOVE_DEVICE};
use crate::ui::settings_modal::{controls, group_button};
use gpui::{
    div, AnyElement, App, ClipboardItem, Context, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, Styled as _, Window,
};
use sakura_core::engine::phone::{PhoneDevice, PhoneSetup};
use std::rc::Rc;

/// The prefix of the keys the phone setup's presses are kept under.
pub(super) const PHONE_PRESS: &str = "engine-phone";

impl EnginePage {
    /// Reads the setup of the phone page; a failed read keeps what was shown.
    pub(super) fn read_phone(&mut self, cx: &mut Context<Self>) {
        let setup = self.source.phone();
        cx.spawn(async move |page, cx| {
            let setup = setup.await;
            drop(page.update(cx, |page, cx| {
                page.phone_setup = setup.ok().or_else(|| page.phone_setup.take());
                cx.notify();
            }));
        })
        .detach();
    }

    /// The block under the phone page's row, or none before the engine has said anything of the setup.
    pub(super) fn phone_block(
        &self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let setup = self.phone_setup.clone()?;
        if setup.state.is_empty() {
            return None;
        }
        let link = setup.link.as_deref().filter(|_| setup.state == "ready");
        let mut block = div()
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .pb(theme::GAP_WIDE)
            .children(link.is_none().then(|| {
                div()
                    .debug_selector(|| "engine-phone-words".into())
                    .text_size(sizes::SMALL_TEXT)
                    .text_color(palette.text_secondary)
                    .child(SharedString::from(setup.words.clone()))
            }));
        if let Some(link) = link {
            block = block.child(self.link_line(link, palette, cx));
            block = block.child(
                div()
                    .debug_selector(|| "engine-phone-opens-from".into())
                    .text_size(sizes::SMALL_TEXT)
                    .text_color(palette.text_tertiary)
                    .child(tr(Key("engine_page.phone.opens_from"))),
            );
        }
        for device in setup.allowed() {
            block = block.child(self.device_line(device, palette, cx));
        }
        if let Some(allow) = self.allow_menu(&setup, cx) {
            block = block.child(div().flex().child(allow));
        }
        let told = crate::ui::engine_acts::line(&self.acts, PHONE_PRESS, palette, cx);
        Some(block.children(told).into_any_element())
    }

    fn link_line(&self, link: &str, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (copied, opened) = (link.to_string(), link.to_string());
        let copy = Rc::new(move |_: &mut Window, cx: &mut App| {
            cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
        });
        let open = Rc::new(move |_: &mut Window, cx: &mut App| cx.open_url(&opened));
        div()
            .flex()
            .items_center()
            .gap(theme::GAP_WIDE)
            .child(
                div()
                    .debug_selector(|| "engine-phone-link".into())
                    .min_w_0()
                    .text_size(sizes::ROW_TEXT)
                    .child(SharedString::from(link.to_string())),
            )
            .child(group_button(
                "engine-phone-copy",
                tr(Key("engine_page.phone.copy")),
                copy,
                cx,
            ))
            .child(group_button(
                "engine-phone-open",
                tr(Key("engine_page.phone.open")),
                open,
                cx,
            ))
            .text_color(palette.text)
            .into_any_element()
    }

    fn device_line(
        &self,
        device: &PhoneDevice,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (acts, id) = (self.acts.clone(), device.id.clone());
        let remove = Rc::new(move |_: &mut Window, cx: &mut App| {
            let act = REMOVE_DEVICE.act(&[("device", id.as_str())]);
            acts.update(cx, |acts, cx| acts.press(PHONE_PRESS, act, "", cx));
        });
        let selector = format!("engine-phone-device-{}", device.id);
        div()
            .debug_selector(move || selector.clone())
            .flex()
            .items_center()
            .gap(theme::GAP_WIDE)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(sizes::ROW_TEXT)
                    .text_color(palette.text)
                    .child(SharedString::from(device_words(device))),
            )
            .child(group_button(
                format!("engine-phone-remove-{}", device.id),
                tr(Key("engine_page.phone.remove")),
                remove,
                cx,
            ))
            .into_any_element()
    }

    /// The menu of the devices that may not open the page yet; none when there is nobody to allow.
    fn allow_menu(&self, setup: &PhoneSetup, cx: &mut Context<Self>) -> Option<AnyElement> {
        let choices: Vec<controls::Choice> = setup
            .allowable()
            .map(|device| {
                let (acts, id) = (self.acts.clone(), device.id.clone());
                controls::Choice {
                    label: device_words(device).into(),
                    is_on: false,
                    on_pick: Rc::new(move |_: &mut Window, cx: &mut App| {
                        let act = ALLOW_DEVICE.act(&[("device", id.as_str())]);
                        acts.update(cx, |acts, cx| acts.press(PHONE_PRESS, act, "", cx));
                    }),
                }
            })
            .collect();
        if choices.is_empty() {
            return None;
        }
        Some(controls::menu(
            "engine-phone-allow",
            tr(Key("engine_page.phone.allow")).into(),
            choices,
            cx,
        ))
    }
}

/// A device in words: its name, and which one is this Mac.
fn device_words(device: &PhoneDevice) -> String {
    match device.is_this_mac {
        true => format!(
            "{} ({})",
            device.name,
            tr(Key("engine_page.phone.this_mac"))
        ),
        false => format!("{}, {}", device.name, device.os),
    }
}
