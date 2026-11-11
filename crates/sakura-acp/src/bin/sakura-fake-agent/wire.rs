//! The fake agent's side of the wire: one JSON-RPC message per line on stdout, written whole under a lock.

use serde_json::{json, Value};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Requests the agent sends start here, apart from the client's own ids.
const FIRST_REQUEST_ID: u64 = 9000;

/// Stdout as a line-delimited JSON-RPC writer.
pub struct Wire {
    out: Mutex<std::io::Stdout>,
    next_request: AtomicU64,
    /// Whether a line that is not JSON goes out before every response, as a careless agent's log would.
    is_noisy: bool,
}

impl Wire {
    pub fn stdout(is_noisy: bool) -> Self {
        Self {
            out: Mutex::new(std::io::stdout()),
            next_request: AtomicU64::new(FIRST_REQUEST_ID),
            is_noisy,
        }
    }

    /// A successful response.
    pub fn result(&self, id: &Value, result: Value) {
        if self.is_noisy {
            self.write("debug: answering a request");
        }
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    /// An error response.
    pub fn error(&self, id: &Value, code: i64, message: &str) {
        let error = json!({ "code": code, "message": message });
        self.send(json!({ "jsonrpc": "2.0", "id": id, "error": error }));
    }

    /// A `session/update` notification.
    pub fn update(&self, session: &str, update: Value) {
        let params = json!({ "sessionId": session, "update": update });
        self.send(json!({ "jsonrpc": "2.0", "method": "session/update", "params": params }));
    }

    /// The id the next request to the client will carry.
    pub fn next_id(&self) -> u64 {
        self.next_request.fetch_add(1, Ordering::SeqCst)
    }

    /// A request to the client, under an id taken from [`Wire::next_id`].
    pub fn request(&self, id: u64, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
    }

    fn send(&self, message: Value) {
        self.write(&message.to_string());
    }

    /// Writes the line and its newline; a closed pipe means the client left, so the agent ends too.
    fn write(&self, line: &str) {
        let mut out = self.out.lock().expect("the stdout lock");
        let written = writeln!(out, "{line}").and_then(|()| out.flush());
        if written.is_err() {
            std::process::exit(0);
        }
    }
}
