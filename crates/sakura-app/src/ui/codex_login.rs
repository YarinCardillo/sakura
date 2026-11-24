//! One Codex login in the app, for every place that asks for it (the Master's agent menu and the Add account
//! sheet): the program's own app-server makes the login in the Codex home it is given, the app opens the browser
//! when the server gives the address and tells its calling component how it ended. No terminal is involved and the app never
//! sees a credential.

use super::master_agent;
use crate::l10n::told;
use gpui::{Context, Task, WeakEntity};
use sakura_acp::connection::{CodexLogin, LoginEnd, LoginEvent, LoginFailure};
use sakura_core::engine::master::MasterSpec;
use sakura_core::error::BridgeResult;
use std::future::Future;
use std::path::PathBuf;

/// Starts a Codex login for `owner` and drives it to its end: `spec` is the engine's spec, which says where the
/// program is; the login is made in `home`, or in Sakura's own Codex home when none is given. `hold` is given the
/// running login (to cancel it) and `finish` how it ended, which is always told once, a login that could not even
/// start included.
pub fn run<T: 'static>(
    spec: impl Future<Output = BridgeResult<MasterSpec>> + 'static,
    home: Option<PathBuf>,
    hold: fn(&mut T, CodexLogin),
    finish: fn(&mut T, &LoginEnd, &mut Context<T>),
    cx: &mut Context<T>,
) -> Task<()> {
    cx.spawn(async move |owner: WeakEntity<T>, cx| {
        let started = match spec.await {
            Ok(spec) => master_agent::start_codex_login_in(&spec, home.as_deref()),
            Err(error) => Err(told(&error)),
        };
        let (login, mut events) = match started {
            Ok(started) => started,
            Err(reason) => {
                let end = LoginEnd::Failed(LoginFailure::NotStarted(reason));
                owner.update(cx, |owner, cx| finish(owner, &end, cx)).ok();
                return;
            }
        };
        owner.update(cx, |owner, _| hold(owner, login)).ok();
        while let Some(event) = events.recv().await {
            let heard = owner.update(cx, |owner, cx| match event {
                LoginEvent::Browser(address) => cx.open_url(&address),
                LoginEvent::Ended(end) => finish(owner, &end, cx),
            });
            // An owner that is gone has nobody to tell; the login is cancelled by whoever dropped it.
            if heard.is_err() {
                break;
            }
        }
    })
}
