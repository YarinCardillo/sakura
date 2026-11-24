//! The lines of the Codex app-server: JSON-RPC as that server speaks it over stdio, one message per
//! line, without the `jsonrpc` member. Everything read from it is untrusted text; a message that does not
//! parse is dropped by the caller, never guessed at.

use serde_json::{json, Value};

/// One message of the server.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Incoming {
    /// The answer to a request of ours.
    Response { id: u64, result: Value },
    /// The error answer to a request of ours.
    Failure {
        id: u64,
        code: Option<i64>,
        message: String,
    },
    /// Something the server tells, which nobody answers.
    Notification { method: String, params: Value },
    /// Something the server asks; `id` is sent back as it came (a number or a string).
    Request {
        id: Value,
        method: String,
        params: Value,
    },
}

/// Reads one line; none when it is not a message of the server.
pub(super) fn parse(line: &str) -> Option<Incoming> {
    let message: Value = serde_json::from_str(line).ok()?;
    let object = message.as_object()?;
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    let method = object.get("method").and_then(Value::as_str);
    let id = object.get("id").cloned();
    match (method, id) {
        (Some(method), Some(id)) => Some(Incoming::Request {
            id,
            method: method.to_string(),
            params,
        }),
        (Some(method), None) => Some(Incoming::Notification {
            method: method.to_string(),
            params,
        }),
        (None, Some(id)) => answer(id.as_u64()?, object),
        (None, None) => None,
    }
}

fn answer(id: u64, object: &serde_json::Map<String, Value>) -> Option<Incoming> {
    if let Some(error) = object.get("error") {
        let message = error.get("message").and_then(Value::as_str);
        let message = message.unwrap_or("the server gave no reason");
        return Some(Incoming::Failure {
            id,
            code: error.get("code").and_then(Value::as_i64),
            message: message.to_string(),
        });
    }
    let result = object.get("result")?.clone();
    Some(Incoming::Response { id, result })
}

/// A request of ours.
pub(super) fn request(id: u64, method: &str, params: Value) -> String {
    json!({ "id": id, "method": method, "params": params }).to_string()
}

/// A notification of ours.
pub(super) fn notification(method: &str) -> String {
    json!({ "method": method }).to_string()
}

/// The answer to a request of the server.
pub(super) fn response(id: &Value, result: Value) -> String {
    json!({ "id": id, "result": result }).to_string()
}

/// The error answer to a request of the server.
pub(super) fn failure(id: &Value, message: &str) -> String {
    json!({ "id": id, "error": { "code": -32601, "message": message } }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_kinds_of_message_are_told_apart() {
        let response = parse(r#"{"id":3,"result":{"a":1}}"#);
        assert_eq!(
            response,
            Some(Incoming::Response {
                id: 3,
                result: json!({ "a": 1 })
            })
        );
        let failure = parse(r#"{"id":4,"error":{"code":-1,"message":"no"}}"#);
        assert_eq!(
            failure,
            Some(Incoming::Failure {
                id: 4,
                code: Some(-1),
                message: "no".into()
            })
        );
        let notification = parse(r#"{"method":"turn/started","params":{"x":1}}"#);
        assert!(matches!(notification, Some(Incoming::Notification { .. })));
        let request = parse(r#"{"id":"abc","method":"item/tool/call","params":{}}"#);
        let Some(Incoming::Request { id, .. }) = request else {
            panic!("a request");
        };
        assert_eq!(id, json!("abc"), "a string id is kept as it came");
    }

    #[test]
    fn a_line_that_is_not_a_message_is_dropped() {
        for line in [
            "",
            "not json",
            "[1]",
            r#"{"x":1}"#,
            r#"{"id":"s","result":1}"#,
        ] {
            assert_eq!(parse(line), None, "{line}");
        }
    }

    #[test]
    fn our_messages_carry_no_jsonrpc_member() {
        let line = request(7, "turn/start", json!({ "threadId": "t" }));
        let value: Value = serde_json::from_str(&line).expect("json");
        assert_eq!(
            value,
            json!({ "id": 7, "method": "turn/start", "params": { "threadId": "t" } })
        );
        assert_eq!(notification("initialized"), r#"{"method":"initialized"}"#);
    }
}
