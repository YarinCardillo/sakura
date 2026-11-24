//! A row of links shown by their titles ("Issue #1783" and the host it goes to, muted), each opening in the
//! browser. The title is the engine's, so the host beside it is read from the address that opens: a title can
//! name one place and the address go to another, and the person sees both. The full address is in the tooltip.

use crate::theme::{self, Palette};
use crate::ui::inert_markdown;
use crate::ui::tip::{self, Words};
use gpui::{
    div, AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _,
};
use sakura_core::engine::snapshot::Link;

/// The only kind of address a link may open: the engine takes no other, and the page checks again.
const LINK_SCHEME: &str = "https://";

/// A link of the engine that may be listed: its title, the address the markdown validator allows and the
/// host that address goes to, in the form the browser reads it (letters of another alphabet in ASCII).
#[derive(Debug, PartialEq, Eq)]
struct Accepted {
    label: String,
    address: String,
    host: String,
}

/// The link as it may be listed, or none when its address is not a web one the markdown validator opens (no user
/// name or password, no other kind of address) or is not `https`.
fn accepted(link: &Link) -> Option<Accepted> {
    let address =
        inert_markdown::opens(&link.url).filter(|address| address.starts_with(LINK_SCHEME))?;
    let host = sakura_core::links::web_host(&address)?;
    Some(Accepted {
        label: link.label.clone(),
        address,
        host,
    })
}

/// The links of `links` that may be listed, as titles with their hosts in a row that wraps; none when no link
/// may. `id` names the row in the window and each title `{id}-{index}`.
pub(super) fn titled_links(
    id: &str,
    links: &[Link],
    palette: &Palette,
    cx: &App,
) -> Option<AnyElement> {
    let titles: Vec<AnyElement> = links
        .iter()
        .filter_map(accepted)
        .enumerate()
        .map(|(index, link)| title(format!("{id}-{index}"), link, palette, cx))
        .collect();
    if titles.is_empty() {
        return None;
    }
    let selector = id.to_string();
    Some(
        div()
            .debug_selector(move || selector)
            .flex()
            .flex_wrap()
            .gap(theme::GAP)
            .children(titles)
            .into_any_element(),
    )
}

fn title(selector: String, link: Accepted, palette: &Palette, cx: &App) -> AnyElement {
    let hover = palette.hover;
    let host_selector = format!("{selector}-host");
    let Accepted {
        label,
        address,
        host,
    } = link;
    let words = Words::plain(SharedString::from(address.clone()));
    let target = div()
        .id(SharedString::from(selector.clone()))
        .debug_selector({
            let selector = selector.clone();
            move || selector
        })
        .h(theme::CONTROL_HEIGHT)
        .px(theme::CONTROL_PADDING_X)
        .flex()
        .items_center()
        .gap(theme::GAP_SMALL)
        .rounded(theme::RADIUS_CONTROL)
        .bg(palette.fill)
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_secondary)
        .cursor_pointer()
        .hover(move |style| style.bg(hover));
    tip::bind(target, selector, words, cx)
        .on_click(move |_, _, cx| inert_markdown::open(&address, cx))
        .child(label)
        .child(
            div()
                .debug_selector(move || host_selector)
                .text_color(palette.text_tertiary)
                .child(host),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(label: &str, url: &str) -> Link {
        Link {
            label: label.into(),
            url: url.into(),
        }
    }

    #[test]
    fn a_label_naming_one_host_over_an_address_on_another_shows_the_real_host() {
        let accepted = accepted(&link("github.com/acme/app", "https://evil.example/x"));
        assert_eq!(accepted.expect("a web link").host, "evil.example");
    }

    #[test]
    fn an_address_the_markdown_validator_refuses_is_not_listed() {
        for url in [
            "https://github.com@evil.example/",
            "https://user:secret@example.com/",
            "http://example.com/plain",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,<b>x</b>",
            "ftp://example.com/file",
            "https://",
            "not an address",
        ] {
            assert!(accepted(&link("Issue #1", url)).is_none(), "{url}");
        }
    }

    #[test]
    fn an_address_with_letters_of_another_alphabet_shows_its_ascii_host() {
        let url = "https://\u{0430}pple.com/login";
        let accepted = accepted(&link("apple.com", url)).expect("the browser reads it");
        assert_eq!(accepted.host, "xn--pple-43d.com");
        assert_eq!(Some(accepted.address), inert_markdown::opens(url));
    }

    #[test]
    fn an_accepted_link_keeps_its_label_and_opens_the_validated_address() {
        let accepted = accepted(&link(
            "Issue #4746",
            "https://github.com/acme/app/issues/4746",
        ));
        let accepted = accepted.expect("a web link");
        assert_eq!(accepted.label, "Issue #4746");
        assert_eq!(accepted.host, "github.com");
        assert_eq!(
            Some(accepted.address),
            inert_markdown::opens("https://github.com/acme/app/issues/4746")
        );
    }
}
