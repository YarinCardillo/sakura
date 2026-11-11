//! Sakura desk: watch and steer coding agents in their agentbox workspaces.

mod data;
#[cfg(feature = "snapshot")]
mod drive;
mod fonts;
mod icons;
mod l10n;
mod language;
mod menus;
mod prefs;
mod snapshot;
mod theme;
mod theme_brand;
mod ui;
mod window;

use std::sync::Arc;

rust_i18n::i18n!("locales", fallback = "en");

/// Threads for `box` calls; they mostly wait on child processes.
const RUNTIME_THREADS: usize = 2;

fn main() {
    let launched = std::time::Instant::now();
    // Before any thread starts: the data folder exists and every `box` this process starts is given it.
    if let Err(error) = sakura_core::platform::prepare_data_home() {
        eprintln!("sakura: cannot create the data folder: {error}");
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(RUNTIME_THREADS)
        .thread_name("sakura-box")
        .enable_all()
        .build()
    {
        Ok(runtime) => Arc::new(runtime),
        Err(error) => return eprintln!("sakura: cannot start the runtime: {error}"),
    };
    let application = gpui_platform::application().with_assets(icons::Assets);
    // Closing the window does not quit Sakura; opening it again (the Dock icon, a second launch) always
    // brings a window back.
    let reopened = runtime.clone();
    application.on_reopen(move |cx| window::reopen(reopened.clone(), cx));
    application.run(move |cx| {
        gpui_component::init(cx);
        // The name the system shows beside what the app tells it (its notifications).
        cx.set_app_identity(window::APP_ID, window::APP_NAME);
        fonts::register(cx);
        menus::install(cx);
        ui::brand::install(cx);
        ui::keyboard::install(cx);
        window::open(launched, runtime, cx);
    });
}
