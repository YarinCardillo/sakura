//! The real `codex app-server`, started with a temporary Codex home that holds a server of its own: the
//! thread config the master builds must leave that server switched off. The person's own `~/.codex` is
//! never used. Without a `codex` program on the path the test says so and passes.

use sakura_acp::connection::{AgentConfig, CodexConfig, Deadlines, SessionSetup};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// A server of the temporary home; it ends at once, so no test waits for it.
const EXTRA_SERVER: &str = "extra";

struct AppServer {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
}

impl AppServer {
    fn start(home: &std::path::Path, folder: &std::path::Path) -> Option<Self> {
        let mut child = Command::new("codex")
            .arg("app-server")
            .env("CODEX_HOME", home)
            .current_dir(folder)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let input = child.stdin.take()?;
        let output = BufReader::new(child.stdout.take()?);
        Some(Self {
            child,
            input,
            output,
            next_id: 1,
        })
    }

    fn write(&mut self, message: &Value) {
        writeln!(self.input, "{message}").expect("write to the server");
        self.input.flush().expect("flush");
    }

    /// Sends a request and reads until its answer; the `result`.
    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({ "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            let read = self.output.read_line(&mut line).expect("read");
            assert!(read > 0, "the server closed while {method} was waited for");
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message["id"] == json!(id) && message.get("method").is_none() {
                assert!(message.get("error").is_none(), "{method}: {message}");
                return message["result"].clone();
            }
        }
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

fn masters_config() -> CodexConfig {
    let server = json!({ "name": "sakura-master", "command": "/bin/sakura", "args": [] });
    CodexConfig {
        agent: AgentConfig {
            command: vec!["codex".into(), "app-server".into()],
            env: Vec::new(),
            workspace: "/engine/master".into(),
            deadlines: Deadlines::default(),
            max_unread_bytes: sakura_acp::transport::MAX_UNREAD_BYTES,
            asks_beyond_workspace: false,
            session: SessionSetup {
                mcp_servers: vec![server],
                ..SessionSetup::default()
            },
        },
        instructions: String::new(),
        model: None,
        effort: None,
        collaboration: None,
        allowed_tools: vec!["mcp__sakura-master__lanes".into()],
    }
}

#[test]
fn a_server_of_the_persons_own_codex_home_is_disabled_in_the_masters_thread() {
    let home = tempfile::tempdir().expect("a home");
    let folder = tempfile::tempdir().expect("a folder");
    let config = format!("[mcp_servers.{EXTRA_SERVER}]\ncommand = \"/usr/bin/false\"\nargs = []\n");
    std::fs::write(home.path().join("config.toml"), config).expect("config");
    let Some(mut server) = AppServer::start(home.path(), folder.path()) else {
        return eprintln!("no codex program on the path: the real-binary test did not run");
    };
    server.call(
        "initialize",
        json!({ "clientInfo": { "name": "sakura-test", "title": null, "version": "0" },
                "capabilities": { "experimentalApi": false, "requestAttestation": false } }),
    );
    server.write(&json!({ "method": "initialized" }));
    let folder_text = folder.path().display().to_string();
    let read = server.call(
        "config/read",
        json!({ "cwd": folder_text, "includeLayers": false }),
    );
    let inherited = CodexConfig::inherited_servers(&read);
    assert_eq!(
        inherited,
        [EXTRA_SERVER],
        "the real config/read names the server"
    );
    let started = server.call(
        "thread/start",
        json!({ "cwd": folder_text, "approvalPolicy": "untrusted", "sandbox": "read-only",
                "ephemeral": true, "config": masters_config().thread_config(&inherited) }),
    );
    let thread = started["thread"]["id"].clone();
    let status = server.call(
        "mcpServerStatus/list",
        json!({ "threadId": thread, "detail": "toolsAndAuthOnly" }),
    );
    let extra = status["data"]
        .as_array()
        .and_then(|all| all.iter().find(|entry| entry["name"] == EXTRA_SERVER));
    let state = extra.map(|entry| entry["runtimeStatus"].clone());
    assert_eq!(state, Some(json!("disabled")), "{status}");
}
