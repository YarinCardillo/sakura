//! The tab strip above the panel: one tab per opened lavoro, 36 px, on the canvas. The active tab is fused
//! into the panel with concave fillets at its base, and the panel's top-left corner morphs with the tab's
//! distance from it. With no lavoro opened the
//! strip holds no tab and the panel's top is drawn alone.
//!
//! Entry point: [`Strip`], an entity that owns the tabs ([`sakura_core::tabs::Tabs`]) and draws them.
//! - It needs: the tabs to start from (`Strip::new`), and how far its first tab starts from the panel's
//!   left edge (`Strip::set_leading_inset`, for the traffic lights when the sidebar is hidden).
//! - It emits [`StripEvent::Changed`] whenever the tabs, their order or the one shown changed: the desk
//!   then shows the lavoro of `strip.tabs().active()` (none when no tab is open) and stores
//!   `strip.tabs().to_json()`.
//! - [`framed`] lays the strip over a page as one panel; [`keys::install`] binds Cmd+1 to Cmd+9 and Cmd+W.
//! - Its sizes are the named constants of [`metrics`], to bind to the window's layout tokens; the trackpad
//!   taps when a dragged tab crosses a slot go through [`haptics::Haptics`].

mod drag;
mod element;
mod frame;
pub mod haptics;
pub mod keys;
mod layout;
pub mod metrics;
mod morph;
mod motion;
mod paint;
mod panel;
mod pointer;
mod shape;
mod strip;
mod tab;
mod tips;

#[cfg(test)]
mod tests;

pub use panel::framed;
pub use strip::{Strip, StripEvent};
pub use tab::CLOSE_TAB;
