//! Markdown an agent wrote, made inert before it is drawn. The agent's words are not the person's: an
//! image would be fetched from an address the agent chose, a link could hide where it goes behind its label,
//! and raw HTML would be drawn as markup. So every image, reference, definition and HTML tag is rewritten as
//! plain text, and a link is its full address, written out (an `http` or `https` one can be clicked and opens
//! in the browser, any other is only text) with its own label, when it says something else, as plain text
//! before it: what a click opens is always what is read.

use markdown::mdast::{Link, Node};
use markdown::{to_mdast, ParseOptions};
use sakura_core::links::web_url;

/// The options the kit's Markdown view reads with, for a given choice of the two constructs its own setting
/// can turn on or off (frontmatter) and the one it reads twice (math: it reads with math, then reads again
/// without it any text no plugin claimed). The kit does not let a caller hand it options, so these mirror
/// `gpui-base`'s `parse_options` (GFM, `math_text` and `math_flow` together); one constructor serves the
/// rewriting and the final check, so the two never read differently from each other.
fn reading(has_math: bool, has_frontmatter: bool) -> ParseOptions {
    let mut options = ParseOptions::gfm();
    options.constructs.frontmatter = has_frontmatter;
    options.constructs.math_text = has_math;
    options.constructs.math_flow = has_math;
    options
}

/// Every way the view may read a text. The rewriting reads it with the constructs that turn the most text into
/// markup (no math, no frontmatter: nothing is left unread inside either), the check reads it all four ways.
const READINGS: [(bool, bool); 4] = [(false, false), (false, true), (true, false), (true, true)];

/// What Markdown needs to hold an image, a link, a reference, a bare address, an address in an email or a tag; text
/// with none of these is left as it is, without being read (an answer is made inert again with every chunk).
const MAY_HOLD_LIVE_PARTS: [&str; 5] = ["[", "<", "://", "www.", "@"];

/// One part of the source and what stands there instead.
struct Replacement {
    start: usize,
    end: usize,
    text: String,
}

/// `source` with every image, link, reference, definition and HTML tag written as plain text.
pub fn inert(source: &str) -> String {
    if !MAY_HOLD_LIVE_PARTS.iter().any(|mark| source.contains(mark)) {
        return source.to_string();
    }
    guarded(rewritten_source(source), source)
}

/// `candidate`, the rewriting of `source`, once it has been read again every way the view reads it and holds
/// nothing live; `source` as plain text otherwise, which is the worst the view ever shows.
fn guarded(candidate: String, source: &str) -> String {
    match holds_live_parts(&candidate) {
        false => candidate,
        true => escaped(source),
    }
}

/// Whether `text`, read any way the view reads it, holds an image, a link, a reference, a definition or markup;
/// a text that cannot be read at all counts as holding some.
fn holds_live_parts(text: &str) -> bool {
    READINGS.iter().any(|&(has_math, has_frontmatter)| {
        match to_mdast(text, &reading(has_math, has_frontmatter)) {
            Ok(tree) => is_live(&tree),
            Err(_) => true,
        }
    })
}

fn is_live(node: &Node) -> bool {
    let is_node_live = match node {
        Node::Link(link) => !is_plain_address(link),
        _ => matches!(
            node,
            Node::Image(_)
                | Node::ImageReference(_)
                | Node::LinkReference(_)
                | Node::Definition(_)
                | Node::Html(_)
        ),
    };
    is_node_live || node.children().into_iter().flatten().any(is_live)
}

/// Whether `link` is an `http` or `https` address whose text is that very address: it hides nothing.
fn is_plain_address(link: &Link) -> bool {
    let is_its_own_text =
        matches!(link.children.as_slice(), [Node::Text(text)] if text.value == link.url);
    is_its_own_text && opens(&link.url).is_some()
}

/// `source` with every live part written as plain text.
fn rewritten_source(source: &str) -> String {
    let Ok(tree) = to_mdast(source, &reading(false, false)) else {
        return escaped(source);
    };
    let mut found = Vec::new();
    collect(&tree, source, &mut found);
    replace(source, found)
}

fn replace(source: &str, mut found: Vec<Replacement>) -> String {
    found.sort_by_key(|replacement| replacement.start);
    let mut out = String::with_capacity(source.len());
    let mut at = 0;
    for replacement in &found {
        if replacement.start < at {
            continue;
        }
        out.push_str(&source[at..replacement.start]);
        out.push_str(&replacement.text);
        at = replacement.end;
    }
    out.push_str(&source[at..]);
    out
}

#[cfg(not(feature = "boxes"))]
pub(super) fn spec(source: &str) -> String {
    let source = inert(source);
    let Ok(tree) = to_mdast(&source, &reading(false, false)) else {
        return source;
    };
    let mut breaks = Vec::new();
    collect_code_breaks(&tree, &mut breaks);
    replace(&source, breaks)
}

#[cfg(not(feature = "boxes"))]
fn collect_code_breaks(node: &Node, breaks: &mut Vec<Replacement>) {
    if let Node::InlineCode(code) = node {
        if let Some(position) = &code.position {
            if let Some(text) = code_with_breaks(&code.value) {
                breaks.push(Replacement {
                    start: position.start.offset,
                    end: position.end.offset,
                    text,
                });
            }
        }
    }
    for child in node.children().into_iter().flatten() {
        collect_code_breaks(child, breaks);
    }
}

#[cfg(not(feature = "boxes"))]
fn code_with_breaks(code: &str) -> Option<String> {
    if code.trim() != code
        || !code.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'/' | b':' | b'.' | b'-' | b' ')
        })
    {
        return None;
    }
    let parts: Vec<&str> = code.split_inclusive(['_', '/']).collect();
    // A fixed HTML comment separates Markdown code spans without adding a character to copied text.
    (parts.len() > 1).then(|| format!("`{}`", parts.join("`<!-- -->`")))
}

/// Walks the tree; a node that is rewritten is not walked into, its words are taken whole.
fn collect(node: &Node, source: &str, found: &mut Vec<Replacement>) {
    if let Some(text) = rewritten(node, source) {
        if let Some(position) = node.position() {
            found.push(Replacement {
                start: position.start.offset,
                end: position.end.offset,
                text,
            });
        }
        return;
    }
    for child in node.children().into_iter().flatten() {
        collect(child, source, found);
    }
}

/// What a node that must not be drawn as it is says instead; `None` for one that is fine.
fn rewritten(node: &Node, source: &str) -> Option<String> {
    match node {
        Node::Image(image) => Some(with_address(&escaped(&image.alt), &image.url)),
        Node::ImageReference(image) => Some(escaped(&image.alt)),
        Node::Link(link) => Some(link_words(link, source)),
        Node::LinkReference(reference) => Some(escaped(&plain_text(&reference.children))),
        Node::Definition(definition) => {
            let label = definition
                .label
                .as_deref()
                .unwrap_or(&definition.identifier);
            Some(with_address(
                &format!("{}:", escaped(label)),
                &definition.url,
            ))
        }
        Node::Html(html) => Some(escaped(&html.value)),
        _ => None,
    }
}

/// A link: its label, as plain text when it says something else than the address, and the address in full, which
/// is clickable when it is an `http` or `https` one and text otherwise.
fn link_words(link: &Link, source: &str) -> String {
    let text = plain_text(&link.children);
    let written = link
        .position
        .as_ref()
        .map(|p| &source[p.start.offset..p.end.offset]);
    let is_bracketed = written.is_some_and(|written| written.starts_with('['));
    let Some(address) = clickable(&link.url) else {
        return match (is_bracketed, written) {
            (false, Some(written)) => escaped(written),
            _ => with_address(&escaped(&text), &link.url),
        };
    };
    let says_more = is_bracketed && !text.is_empty() && text != link.url && text != address;
    match says_more {
        true => format!("{} (<{address}>)", escaped(&text)),
        false => format!("<{address}>"),
    }
}

/// `address` written to be an autolink's text, which draws as the address and goes to the very same one, when it is
/// a web address:
/// written the way the browser reads it (a host with letters of another alphabet is its ASCII form), with
/// every character that could end the link, split a table cell or hide in it written as `%` and its code.
fn clickable(address: &str) -> Option<String> {
    let web = opens(address)?;
    let mut out = String::with_capacity(web.len() + 2);
    for byte in web.bytes() {
        let is_safe = byte.is_ascii_graphic() && !matches!(byte, b'<' | b'>' | b'|' | b'\\');
        match is_safe {
            true => out.push(char::from(byte)),
            false => out.push_str(&format!("%{byte:02X}")),
        }
    }
    Some(out)
}

/// Allow HTTP and HTTPS links without embedded credentials.
pub fn opens(address: &str) -> Option<String> {
    let web = web_url(address)?;
    let parsed = url::Url::parse(&web).ok()?;
    let has_userinfo = !parsed.username().is_empty() || parsed.password().is_some();
    (!has_userinfo).then_some(web)
}

/// The click on an address: opens it in the browser when `opens` allows it, and says on stderr when it does not.
pub fn open(address: &str, cx: &mut gpui::App) {
    match opens(address) {
        Some(address) => cx.open_url(&address),
        None => eprintln!(
            "sakura: an address in an agent's words is not a web address and was not opened"
        ),
    }
}

/// The words under a list of nodes, without any of their markup.
fn plain_text(nodes: &[Node]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Node::Text(text) => text.value.clone(),
            Node::InlineCode(code) => code.value.clone(),
            Node::Image(image) => image.alt.clone(),
            other => plain_text(other.children().map_or(&[][..], Vec::as_slice)),
        })
        .collect()
}

/// `words (` + the address + `)`, the address written like any other words.
fn with_address(words: &str, address: &str) -> String {
    match words.is_empty() {
        true => escaped(&on_one_line(address)),
        false => format!("{words} ({})", escaped(&on_one_line(address))),
    }
}

/// `address` with every line break and control character made a visible mark.
fn on_one_line(address: &str) -> String {
    let mark = |c: char| {
        if c.is_control() {
            char::REPLACEMENT_CHARACTER
        } else {
            c
        }
    };
    address.chars().map(mark).collect()
}

/// `text` with every punctuation mark escaped, so it is read as the words it is (a pipe stays inside its table
/// cell, a dollar opens no formula), and every control character, a line break too, made a visible mark, so a
/// line cannot end or a block begin inside it.
pub fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' => out.push('\n'),
            c if c.is_control() && c != '\t' => out.push(char::REPLACEMENT_CHARACTER),
            c if c.is_ascii_punctuation() => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the view would make live in `source`, read every way it reads.
    fn live_parts(source: &str) -> Vec<&'static str> {
        let mut found = Vec::new();
        for (has_math, has_frontmatter) in READINGS {
            let tree =
                to_mdast(source, &reading(has_math, has_frontmatter)).expect("markdown reads");
            walk(&tree, &mut found);
        }
        found
    }

    /// A link the reader sees as the address it goes to, which is a web one: the only live link there may be.
    fn is_visible_web_address(link: &Link) -> bool {
        let web = link.url.starts_with("http://") || link.url.starts_with("https://");
        let shown =
            matches!(link.children.as_slice(), [Node::Text(text)] if text.value == link.url);
        web && shown && link.url.is_ascii() && !link.url.contains(['|', '\\', '<', '>', ' ', '@'])
    }

    fn walk(node: &Node, found: &mut Vec<&'static str>) {
        match node {
            Node::Image(_) | Node::ImageReference(_) => found.push("image"),
            Node::Link(link) if !is_visible_web_address(link) => found.push("link"),
            Node::LinkReference(_) => found.push("link"),
            Node::Definition(_) => found.push("definition"),
            Node::Html(_) => found.push("html"),
            _ => {}
        }
        for child in node.children().into_iter().flatten() {
            walk(child, found);
        }
    }

    fn assert_inert(source: &str) -> String {
        assert!(
            !live_parts(source).is_empty(),
            "{source:?} holds nothing live"
        );
        let made = inert(source);
        assert_eq!(
            live_parts(&made),
            Vec::<&str>::new(),
            "{source:?} gave {made:?}"
        );
        made
    }

    #[test]
    fn spec_view_remote_image_not_fetched_shown_as_text() {
        let made = assert_inert("Before ![the logo](https://tracker.example/pixel.png?id=7) after");
        assert!(made.contains("the logo"), "{made}");
        assert!(
            made.replace('\\', "")
                .contains("https://tracker.example/pixel.png?id=7"),
            "{made}"
        );
        assert_inert("![local](/home/someone/.ssh/id_rsa)");
        assert_inert("![by reference][pic]\n\n[pic]: https://tracker.example/p.png\n");
    }

    #[test]
    fn spec_view_link_shown_with_address() {
        let made = assert_inert("Read [the guide](https://example.com/guide \"title\") first");
        assert!(made.contains("the guide"), "{made}");
        assert!(made.contains("<https://example.com/guide>"), "{made}");
        assert_inert("[by reference][1] and [shortcut]\n\n[1]: https://example.com\n[shortcut]: file:///etc/passwd\n");
        assert_inert(
            "A bare https://example.com/x address and <https://example.com/y> and a@b.example",
        );
        assert_inert("[text with `code` and *emphasis*](x-apple-systempreferences:com.apple.x)");
    }

    #[test]
    fn inert_link_shows_full_address_and_opens_only_http() {
        let made = assert_inert("Read [the guide](https://example.com/guide) first");
        assert_eq!(made, "Read the guide (<https://example.com/guide>) first");
        assert_eq!(
            inert("See https://example.com/x and <https://example.com/y>."),
            "See <https://example.com/x> and <https://example.com/y>."
        );
        // Not a web address: written out as text, never a link.
        for other in [
            "[a file](file:///etc/passwd)",
            "[a program](x-apple-systempreferences:com.apple.x)",
            "[a script](javascript:alert(1))",
            "<file:///etc/passwd>",
        ] {
            let made = assert_inert(other);
            assert!(!made.contains("]("), "{made}");
        }
        assert_eq!(
            opens("https://example.com/guide").as_deref(),
            Some("https://example.com/guide")
        );
        assert_eq!(
            opens("http://localhost:3000/").as_deref(),
            Some("http://localhost:3000/")
        );
        for refused in [
            "file:///etc/passwd",
            "x-apple-systempreferences:com.apple.x",
            "javascript:alert(1)",
            "mailto:a@b.example",
            "/etc/passwd",
        ] {
            assert_eq!(opens(refused), None, "{refused}");
        }
    }

    #[test]
    fn inert_link_with_userinfo_shown_as_text_never_opens() {
        for address in [
            "https://github.com@evil.example/x",
            "http://user:secret@example.com/",
            "https://:pass@example.com/",
            "https://github.com:443@evil.example/",
        ] {
            assert_eq!(opens(address), None, "{address}");
            let made = assert_inert(&format!("[the project]({address})"));
            assert!(
                !made.contains("(<") && made.contains("the project"),
                "{made}"
            );
            assert!(made.replace('\\', "").contains(address), "{made}");
            let bare = assert_inert(&format!("<{address}>"));
            assert!(!bare.starts_with('<'), "{bare}");
        }
        assert!(opens("https://example.com@/x").is_none());
        assert!(opens("https://example.com/path@with-at").is_some());
    }

    #[test]
    fn inert_link_label_never_hides_target() {
        let made = assert_inert("[https://good.example](https://evil.example/login)");
        assert!(
            made.contains("good\\.example") && made.contains("<https://evil.example/login>"),
            "{made}"
        );
        let made = assert_inert("[click here](https://evil.example/a)");
        let (label, address) = (
            made.find("click here"),
            made.find("<https://evil.example/a>"),
        );
        assert!(
            label.is_some() && address.is_some() && label < address,
            "the label comes first: {made}"
        );
        // Letters of another alphabet and invisible marks never hide in the address: it is written as the browser reads it.
        let made = assert_inert("[docs](https://exаmple.com/\u{202e}x)");
        let inside =
            &made[made.find('<').expect("an address")..made.rfind('>').expect("an address")];
        assert!(inside.is_ascii() && inside.contains("xn--"), "{made}");
        // A pipe stays inside its table cell.
        let made = assert_inert("| a |\n|---|\n| [t](https://example.com/a\\|b) |\n");
        assert!(made.contains("%7C") && made.lines().count() == 3, "{made}");
    }

    #[test]
    fn spec_view_raw_html_shown_as_text() {
        let made = assert_inert("<img src=\"https://tracker.example/p.png\"> and <a href=\"x\">link</a>\n\n<div>block</div>\n");
        assert!(made.contains("img src"), "{made}");
        assert_inert("<script>alert(1)</script>");
    }

    #[test]
    fn code_blocks_and_ordinary_markdown_are_left_as_they_are() {
        let source = "# Goal\n\n- one\n- **two**\n\n```\n[not a link](https://x.example)\n```\n\nInline `![x](y)` stays.\n";
        assert_eq!(inert(source), source);
    }

    #[test]
    fn an_address_with_backticks_is_plain_words() {
        assert_inert("[a](https://example.com/`x`)");
    }

    #[test]
    fn nested_markup_in_a_link_text_is_flattened() {
        assert_inert("[![inner](https://tracker.example/i.png)](https://example.com)");
    }

    #[test]
    fn inert_markdown_table_pipe_in_address_stays_inert() {
        let source = "| name | link |\n|---|---|\n| a | [t](https://example.com/a\\|b) |\n| b | ![i](https://tracker.example/p\\|q.png) |\n";
        let made = assert_inert(source);
        assert!(
            made.contains("a%7Cb") && made.contains("p\\|q"),
            "a pipe is encoded in a link and escaped in an image's address: {made}"
        );
        assert_eq!(
            made.lines().count(),
            4,
            "the address stays in its cell: {made}"
        );
        assert_inert("| a | [t](<https://example.com/a|b|c>) |\n|---|---|\n");
        assert_inert("[t](<https://example.com/a\u{85}b>)");
    }

    #[test]
    fn inert_markdown_math_or_frontmatter_construct_stays_inert() {
        assert_inert("$a [x](https://example.com) b$");
        assert_inert("$$\n![i](https://tracker.example/p.png)\n$$\n");
        assert_inert("Cost $5 and [x](https://example.com) and $10");
        assert_inert(
            "---\ntitle: [x](https://example.com)\n---\n![i](https://tracker.example/p.png)\n",
        );
        assert_inert("---\n<img src=x>\n---\n");
    }

    #[test]
    fn inert_markdown_fallback_when_live_node_survives() {
        let source = "Plain words and [a link](https://example.com).";
        let kept = guarded("still [a link](https://example.com)".to_string(), source);
        assert_eq!(live_parts(&kept), Vec::<&str>::new(), "{kept}");
        assert!(
            kept.contains("Plain words"),
            "the whole source is shown as plain text: {kept}"
        );
        let clean = guarded("only words".to_string(), source);
        assert_eq!(clean, "only words", "a clean candidate is kept");
    }
}
