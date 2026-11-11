use super::*;

#[test]
fn a_project_without_a_request_of_its_own_gets_the_default_one_for_its_base() {
    let default = Said::new("publish.request.default").text("base", "main");
    assert_eq!(request_for(None, "main"), Request::Default(default.clone()));
    assert_eq!(
        request_for(Some("  \n "), "main"),
        Request::Default(default)
    );
    let own = "Push and open a pull request against develop, in Italian.";
    assert_eq!(
        request_for(Some(&format!("  {own}\n")), "main"),
        Request::Own(own.into())
    );
}

#[test]
fn a_request_typed_in_settings_is_kept_as_typed_or_refused() {
    assert_eq!(
        own_request("  "),
        Ok(None),
        "nothing typed: the default one"
    );
    assert_eq!(
        own_request(" Push it.\nThen open a draft. "),
        Ok(Some("Push it.\nThen open a draft.".into()))
    );
    assert!(own_request(&"x".repeat(REQUEST_MAX_CHARS + 1)).is_err());
    assert!(own_request("push\u{1b}[2J").is_err());
    // A request kept before the bound existed is never sent longer than the bound.
    let long = "y".repeat(REQUEST_MAX_CHARS + 500);
    let Request::Own(sent) = request_for(Some(&long), "main") else {
        panic!("the project's own request");
    };
    assert_eq!(sent.chars().count(), REQUEST_MAX_CHARS);
}

#[test]
fn the_link_of_a_pull_request_is_found_in_what_the_agent_wrote() {
    let found = |text: &str| pull_request(text).map(|link| (link.host, link.number, link.url));
    let github = "I pushed the branch and opened a draft: https://github.com/acme/shop/pull/128.";
    assert_eq!(
        found(github),
        Some((
            Host::GitHub,
            128,
            "https://github.com/acme/shop/pull/128".into()
        ))
    );
    let gitlab = "Merge request: <https://gitlab.com/acme/platform/shop/-/merge_requests/7>";
    assert_eq!(
        found(gitlab),
        Some((
            Host::GitLab,
            7,
            "https://gitlab.com/acme/platform/shop/-/merge_requests/7".into()
        ))
    );
    // A page of the pull request is the pull request; a host of one's own is read the same way.
    let files = "see https://github.com/acme/shop/pull/128/files#diff-1 for the diff";
    assert_eq!(
        found(files).unwrap().2,
        "https://github.com/acme/shop/pull/128"
    );
    let own_host = "(https://git.example.com:8443/team/app/-/merge_requests/41/diffs)";
    assert_eq!(
        found(own_host).unwrap().2,
        "https://git.example.com:8443/team/app/-/merge_requests/41"
    );
    // The newest link is the one that counts.
    let two = "https://github.com/acme/shop/pull/1 was closed; https://github.com/acme/shop/pull/2 is open";
    assert_eq!(found(two).unwrap().1, 2);
}

#[test]
fn what_is_not_the_link_of_a_pull_request_is_not_one() {
    let none = |text: &str| assert_eq!(pull_request(text), None, "{text}");
    none("");
    none("no link here");
    none("http://github.com/acme/shop/pull/128");
    none("https://github.com/acme/shop/pulls");
    none("https://github.com/acme/shop/pull/");
    none("https://github.com/acme/shop/pull/abc");
    none("https://github.com/acme/shop/issues/128");
    none("https://github.com/acme/pull/128");
    none("https://user:secret@github.com/acme/shop/pull/128");
    none("https://github.com/acme/shop/pull/0");
    none("https://github.com/acme/shop/pull/99999999999999999999");
    none("https://gitlab.com/-/merge_requests/7");
    none("https://exa mple.com/acme/shop/pull/1");
    none(&format!(
        "https://github.com/{}/shop/pull/1",
        "a".repeat(SEGMENT_MAX_CHARS + 1)
    ));
}

#[test]
fn only_the_end_of_a_very_long_text_is_looked_at() {
    let link = "https://github.com/acme/shop/pull/128";
    let early = format!("{link} {}", "é".repeat(SCAN_MAX_BYTES));
    assert_eq!(pull_request(&early), None);
    let late = format!("{} {link}", "é".repeat(SCAN_MAX_BYTES));
    assert_eq!(pull_request(&late).unwrap().number, 128);
}

#[test]
fn a_text_of_link_starts_with_no_end_is_looked_at_within_a_bound() {
    // Every start would be read to the end of the text if nothing stopped it: the work would grow
    // with the square of the text.
    let glued = SCHEME.repeat(SCAN_MAX_BYTES / SCHEME.len() * 3);
    let (found, examined) = scan(&glued);
    assert_eq!(found, None);
    let bound = SCAN_MAX_BYTES + CANDIDATES_MAX * (URL_MAX_BYTES + 1);
    assert!(
        examined <= bound,
        "{examined} bytes looked at, over {bound}"
    );
    // One endless link is given up after the longest link's length.
    let endless = format!("{SCHEME}github.com/{}", "a/".repeat(SCAN_MAX_BYTES));
    let (found, examined) = scan(&endless);
    assert_eq!(found, None);
    assert!(examined <= SCAN_MAX_BYTES + URL_MAX_BYTES + 1, "{examined}");
    // Multi-byte text is cut on a character, never inside one.
    let wide = format!("{SCHEME}{}", "é".repeat(URL_MAX_BYTES));
    assert_eq!(scan(&wide).0, None);
}

#[test]
fn only_the_newest_link_starts_are_tried() {
    let link = "https://github.com/acme/shop/pull/5";
    let others = |count: usize| "https://example.com/page ".repeat(count);
    let near = format!("{link} {}", others(CANDIDATES_MAX - 1));
    assert_eq!(pull_request(&near).unwrap().number, 5);
    let far = format!("{link} {}", others(CANDIDATES_MAX));
    assert_eq!(pull_request(&far), None);
}
