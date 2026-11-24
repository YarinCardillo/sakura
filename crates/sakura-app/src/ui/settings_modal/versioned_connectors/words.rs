use crate::l10n::{tr, tr_with, Key};
use gpui::SharedString;
use sakura_core::engine::accounts::Connector;

pub(super) fn reads(row: &Connector) -> SharedString {
    let Some(reads) = &row.reads else {
        return tr(Key("connectors.review_scope")).into();
    };
    if let Some(text) = reads.text.as_ref().filter(|words| !words.is_empty()) {
        return text.clone().into();
    }
    let args = &reads.args;
    let phrase = match reads.key.as_deref() {
        Some("connector_reads_github") => {
            args.get("resource")
                .filter(|repo| !repo.is_empty())
                .map(|repo| {
                    if let Some(branch) = args.get("branch").filter(|branch| !branch.is_empty()) {
                        tr_with(
                            Key("connectors.reads.github_branch"),
                            &[("resource", repo), ("branch", branch)],
                        )
                    } else {
                        tr_with(Key("connectors.reads.github"), &[("resource", repo)])
                    }
                })
        }
        Some("connector_reads_linear") => resource("connectors.reads.linear", args.get("resource")),
        Some("connector_reads_notion") => resource("connectors.reads.notion", args.get("resource")),
        Some("connector_reads_feed") => resource("connectors.reads.feed", args.get("host")),
        Some("connector_reads_program") => Some(tr(Key("connectors.reads.program")).into()),
        _ => None,
    };
    phrase.unwrap_or_else(|| tr(Key("connectors.review_scope")).into())
}

fn resource(key: &'static str, value: Option<&String>) -> Option<SharedString> {
    Some(tr_with(
        Key(key),
        &[("resource", value.filter(|value| !value.is_empty())?)],
    ))
}

pub(super) fn name(row: &Connector) -> SharedString {
    if row.label.is_empty() {
        tr(Key("connectors.unnamed")).into()
    } else {
        row.label.clone().into()
    }
}

pub(super) fn status(row: &Connector) -> &'static str {
    tr(Key(match row.status.as_deref() {
        Some("connected") => "connectors.status.connected",
        Some("disconnected") => "connectors.status.disconnected",
        Some("connector_unavailable") => "connectors.status.unavailable",
        Some("reconnect_required") => "connectors.status.reconnect",
        Some("scope_unavailable") => "connectors.status.scope",
        Some("approval_required") => "connectors.status.approval",
        None if row.connected => "connectors.status.connected",
        _ => "connectors.status.unavailable",
    }))
}

pub(super) fn operation(code: &str) -> &'static str {
    tr(Key(match code {
        "read" => "connectors.tools.read",
        "comment_pr" => "connectors.tools.comment_pr",
        "comment_issue" => "connectors.tools.comment_issue",
        "append_page" => "connectors.tools.append_page",
        _ => "connectors.tools.unknown",
    }))
}

pub(in crate::ui) fn refusal(code: &str) -> &'static str {
    tr(Key(match code {
        "stale_revision" => "connectors.reason.stale_revision",
        "invalid_request" => "connectors.reason.invalid_request",
        "invalid_choices" => "connectors.reason.invalid_choices",
        "unknown_connector" => "connectors.reason.unknown_connector",
        "operation_conflict" => "connectors.reason.operation_conflict",
        "reconnect_required" => "connectors.status.reconnect",
        "scope_unavailable" => "connectors.status.scope",
        "approval_required" => "connectors.status.approval",
        "replacement_unproven" => "connectors.reason.replacement_unproven",
        "replacement_incompatible" => "connectors.reason.replacement_incompatible",
        "local_only" | "operator_only" => "connectors.reason.operator",
        _ => "connectors.status.unavailable",
    }))
}
