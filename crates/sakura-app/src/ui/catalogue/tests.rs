//! The catalogue shows the components of the lane's agents in every state: the row of the list, and the rows
//! of the conversation.

use super::Catalogue;
use crate::theme::{Appearance, Theme};
use gpui::{AppContext as _, TestAppContext};
use gpui_component::Root;

#[gpui::test]
fn the_catalogue_shows_the_agent_row_and_the_conversation_rows_in_every_state(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
        cx.set_reduce_motion(true);
    });
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let page = cx.new(|cx| Catalogue::new(None, window, cx));
        Root::new(page, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let row_states = [
        "cat-agent-0",
        "cat-agent-1",
        "cat-agent-2",
        "cat-agent-3",
        "cat-agent-4",
        "cat-agent-5",
        "cat-agent-hover",
    ];
    let conversation = [
        "thread-prompt",
        "chat-tool-0",
        "chat-tool-elapsed-0",
        "chat-tool-1",
        "chat-tool-5",
        "chat-engine-2",
        "chat-result-3",
        "chat-result-4",
        "chat-verdict-pass-6",
        "chat-verdict-warn-7",
        "chat-round-8",
        "chat-not-listening",
    ];
    for selector in row_states.into_iter().chain(conversation) {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "{selector} is not in the catalogue"
        );
    }
}
