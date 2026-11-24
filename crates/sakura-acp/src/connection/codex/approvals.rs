//! What a Codex master may do. The master runs in Codex's read-only sandbox and nothing it asks beyond that
//! (a command, a change to a file) happens unless the engine's decider lets it: the request is put to the
//! decider in the shape the engine reads for Claude (a `Bash` command, an `Edit` of one file), a refusal is
//! answered at once, and what the engine only asks about goes to the person. With no decider nothing is
//! allowed. Every other request of the server (a permission grant, a form, a tool of the client) is
//! answered with a refusal, so nothing the engine has not seen is ever granted.

use super::items::Change;
use crate::model::{OptionKind, PermissionOption};
use crate::policy::decider::Verdict;
use crate::policy::master_command;
use serde_json::{json, Value};
use std::path::Path;

/// The id of the option that lets a request through once, and of the one that refuses it.
pub(super) const ACCEPT: &str = "accept";
pub(super) const DECLINE: &str = "decline";

/// The two answers the person is offered.
pub(super) fn options() -> Vec<PermissionOption> {
    let option = |id: &str, name: &str, kind| PermissionOption {
        id: id.to_string(),
        name: name.to_string(),
        kind,
    };
    vec![
        option(ACCEPT, "Allow once", OptionKind::AllowOnce),
        option(DECLINE, "Decline", OptionKind::RejectOnce),
    ]
}

fn asked(tool: &str, input: Value) -> Value {
    json!({
        "method": "session/request_permission",
        "params": { "toolCall": { "toolCallId": "codex", "name": tool, "rawInput": input } },
    })
}

/// The fields of a command approval this client understands. Any other field that is not null (a proposed
/// execution policy or network amendment, a managed network context, the id of a subcommand's approval)
/// asks for more than running the one command, which nothing here grants.
const COMMAND_FIELDS: [&str; 8] = [
    "threadId",
    "turnId",
    "itemId",
    "startedAtMs",
    "reason",
    "command",
    "cwd",
    "commandActions",
];
/// The fields of a patch approval this client understands; `grantRoot` and the rest ask for more than the
/// patch.
const PATCH_FIELDS: [&str; 5] = ["threadId", "turnId", "itemId", "startedAtMs", "reason"];

/// Whether every field of `params` that is not null is one of `understood`.
fn understands(params: &Value, understood: &[&str]) -> bool {
    let Some(fields) = params.as_object() else {
        return false;
    };
    fields
        .iter()
        .all(|(name, value)| value.is_null() || understood.contains(&name.as_str()))
}

/// A command approval, as the engine's decider reads a Claude `Bash` request, with everything that shapes how
/// it runs: the command and the folder it runs in. None when it must be declined without asking anyone: it
/// names no command, runs outside the master's folder, or carries a field that is not understood.
pub(super) fn command_request(params: &Value, workspace: &Path) -> Option<Value> {
    if !understands(params, &COMMAND_FIELDS) {
        return None;
    }
    let mut input = json!({ "command": params.get("command")?, "cwd": params.get("cwd")? });
    if let Some(actions) = params
        .get("commandActions")
        .filter(|actions| !actions.is_null())
    {
        input["commandActions"] = actions.clone();
    }
    Some(asked(
        "Bash",
        master_command::approval_input(&input, workspace)?,
    ))
}

/// Whether a patch approval asks for nothing beyond its changes.
pub(super) fn patch_is_plain(params: &Value) -> bool {
    understands(params, &PATCH_FIELDS)
}

/// The requests a patch is judged by, as the engine's decider reads a Claude `Edit`: one for each file it
/// changes and one for the place each move ends at, each with what is done (`add`, `delete`, `update`,
/// `move`).
pub(super) fn edit_requests(changes: &[Change]) -> Vec<Value> {
    let mut requests = Vec::new();
    for change in changes {
        requests.push(edit_request(&change.path, change.kind));
        if let Some(to) = &change.move_to {
            requests.push(edit_request(to, "move"));
        }
    }
    requests
}

fn edit_request(path: &str, kind: &str) -> Value {
    asked("Edit", json!({ "file_path": path, "kind": kind }))
}

/// The verdict on a request that has several parts (the files of one patch): one refusal refuses it all,
/// then one question asks the person, and only a request every part of which is allowed is allowed. A
/// request with no part names nothing the engine could judge and is refused.
pub(super) fn combine(verdicts: &[Verdict]) -> Verdict {
    if verdicts.is_empty() || verdicts.contains(&Verdict::Deny) {
        Verdict::Deny
    } else if verdicts.contains(&Verdict::Ask) {
        Verdict::Ask
    } else {
        Verdict::Allow
    }
}

/// The answer to a command approval.
pub(super) fn command_decision(is_accepted: bool) -> Value {
    json!({ "decision": if is_accepted { "accept" } else { "decline" } })
}

/// The answer to a file change approval.
pub(super) fn file_change_decision(is_accepted: bool) -> Value {
    command_decision(is_accepted)
}

/// The answer to an approval of the older shapes of the protocol: always a refusal.
pub(super) fn legacy_refusal() -> Value {
    json!({ "decision": { "denied": { "rejection": "the master's engine decides what runs" } } })
}

/// How a request of the server that is not an approval of a command or of a change is answered; none
/// when the method is not known, which is an error answer.
pub(super) fn refusal_of(method: &str) -> Option<Value> {
    match method {
        "item/permissions/requestApproval" => Some(json!({ "permissions": {}, "scope": "turn" })),
        "mcpServer/elicitation/request" => {
            Some(json!({ "action": "decline", "content": null, "_meta": null }))
        }
        "applyPatchApproval" | "execCommandApproval" => Some(legacy_refusal()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Value {
        json!({ "threadId": "t", "turnId": "u", "itemId": "i", "startedAtMs": 0, "command": "git status",
            "cwd": "/engine/master/sub", "reason": null, "approvalId": null, "networkApprovalContext": null })
    }

    #[test]
    fn a_command_reaches_the_decider_with_where_it_runs() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().canonicalize().unwrap();
        let cwd = workspace.join("sub");
        std::fs::create_dir(&cwd).unwrap();
        let mut params = plain();
        params["cwd"] = json!(cwd);
        let request = command_request(&params, &workspace).expect("judged");
        assert_eq!(request["method"], "session/request_permission");
        let call = &request["params"]["toolCall"];
        assert_eq!(call["name"], "Bash");
        assert_eq!(
            call["rawInput"],
            json!({ "command": "git status", "cwd": cwd })
        );
    }

    #[test]
    fn a_command_outside_the_masters_folder_or_written_to_escape_it_is_not_judged() {
        let workspace = Path::new("/engine/master");
        for cwd in [
            "/elsewhere",
            "/engine/master-two",
            "/engine/master/../x",
            "relative",
            "",
        ] {
            let mut params = plain();
            params["cwd"] = json!(cwd);
            assert_eq!(command_request(&params, workspace), None, "{cwd:?}");
        }
        let mut no_cwd = plain();
        no_cwd.as_object_mut().expect("object").remove("cwd");
        assert_eq!(command_request(&no_cwd, workspace), None);
    }

    #[test]
    fn a_command_request_with_a_field_that_is_not_understood_is_not_judged() {
        let workspace = Path::new("/engine/master");
        for (field, value) in [
            ("proposedExecpolicyAmendment", json!(["git"])),
            ("proposedNetworkPolicyAmendments", json!([{ "host": "x" }])),
            ("networkApprovalContext", json!({ "host": "x" })),
            ("environmentId", json!("other")),
            ("approvalId", json!("sub")),
            ("surprise", json!(true)),
        ] {
            let mut params = plain();
            params[field] = value;
            assert_eq!(command_request(&params, workspace), None, "{field}");
        }
        let mut no_command = plain();
        no_command["command"] = json!("");
        assert_eq!(command_request(&no_command, workspace), None);
    }

    #[test]
    fn a_command_declines_unverified_context_and_normalizes_plain_words() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let other = temp.path().join("other");
        std::fs::create_dir(&other).unwrap();
        let linked = workspace.join("linked");
        std::os::unix::fs::symlink(other, &linked).unwrap();
        let request = |cwd: &Path, command, actions| {
            command_request(
                &json!({ "cwd": cwd, "command": command, "commandActions": actions }),
                &workspace,
            )
        };
        assert!(request(&linked, "git status", json!([])).is_none());
        assert!(request(&workspace, "git status", json!([{}])).is_none());
        assert!(request(&workspace, "git status\n", json!([])).is_none());
        let approved = request(&workspace, "  git\tstatus  ", json!([])).unwrap();
        assert_eq!(
            approved["params"]["toolCall"]["rawInput"],
            json!({ "command": "git status", "cwd": workspace.canonicalize().unwrap(), "commandActions": [] })
        );
    }

    #[test]
    fn a_patch_is_judged_file_by_file_and_at_the_end_of_each_move_with_what_is_done() {
        let changes = [
            Change {
                path: "/w/a".into(),
                kind: "add",
                move_to: None,
            },
            Change {
                path: "/w/b".into(),
                kind: "update",
                move_to: Some("/out/c".into()),
            },
        ];
        let inputs: Vec<Value> = edit_requests(&changes)
            .iter()
            .map(|request| request["params"]["toolCall"]["rawInput"].clone())
            .collect();
        assert_eq!(
            inputs,
            [
                json!({ "file_path": "/w/a", "kind": "add" }),
                json!({ "file_path": "/w/b", "kind": "update" }),
                json!({ "file_path": "/out/c", "kind": "move" }),
            ]
        );
    }

    #[test]
    fn a_patch_request_that_asks_for_a_root_or_anything_else_is_not_plain() {
        let plain = json!({ "threadId": "t", "turnId": "u", "itemId": "i", "startedAtMs": 0, "reason": null, "grantRoot": null });
        assert!(patch_is_plain(&plain));
        for (field, value) in [("grantRoot", json!("/")), ("surprise", json!(1))] {
            let mut asking = plain.clone();
            asking[field] = value;
            assert!(!patch_is_plain(&asking), "{field}");
        }
    }

    #[test]
    fn one_refusal_refuses_all_one_question_asks_and_nothing_to_judge_is_refused() {
        use Verdict::{Allow, Ask, Deny};
        assert_eq!(combine(&[Allow, Deny, Ask]), Deny);
        assert_eq!(combine(&[Allow, Ask]), Ask);
        assert_eq!(combine(&[Allow, Allow]), Allow);
        assert_eq!(combine(&[]), Deny);
    }

    #[test]
    fn the_person_is_offered_one_allow_once_and_one_decline() {
        let offered = options();
        assert_eq!(
            offered
                .iter()
                .map(|option| (option.id.as_str(), option.kind))
                .collect::<Vec<_>>(),
            [
                (ACCEPT, OptionKind::AllowOnce),
                (DECLINE, OptionKind::RejectOnce)
            ]
        );
    }

    #[test]
    fn grants_forms_and_the_older_approvals_are_all_refused_and_other_methods_are_not_answered() {
        assert_eq!(
            refusal_of("item/permissions/requestApproval"),
            Some(json!({ "permissions": {}, "scope": "turn" }))
        );
        assert_eq!(
            refusal_of("mcpServer/elicitation/request").map(|answer| answer["action"].clone()),
            Some(json!("decline"))
        );
        for old in ["applyPatchApproval", "execCommandApproval"] {
            assert_eq!(refusal_of(old), Some(legacy_refusal()));
        }
        assert_eq!(refusal_of("item/tool/call"), None);
    }
}
