//! The proof behind the choice of `agent-client-protocol`: the SDK, used directly, holds a session with a
//! child process over stdio on a tokio runtime shaped like the one the app gives `sakura-core` (two worker
//! threads): `initialize`, `session/new`, a prompt with streamed updates, and a cancel in the middle of a turn.

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    CancelNotification, ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest,
    SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::{Agent, Client, Lines};
use sakura_acp::transport;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::BufReader;

const DEADLINE: Duration = Duration::from_secs(20);

fn prompt(session: &agent_client_protocol::v1::schema::SessionId, text: &str) -> PromptRequest {
    PromptRequest::new(
        session.clone(),
        vec![ContentBlock::Text(TextContent::new(text.to_string()))],
    )
}

async fn hold_a_session() -> agent_client_protocol::Result<(Vec<String>, StopReason, StopReason)> {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_sakura-fake-agent"))
        .args(["--delay-ms", "1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("the fake agent starts");
    let stdin = child.stdin.take().expect("a piped stdin");
    let stdout = child.stdout.take().expect("a piped stdout");
    let lines = Lines::new(
        Box::pin(transport::outgoing(stdin)),
        Box::pin(transport::incoming(
            BufReader::new(stdout),
            transport::MAX_LINE_BYTES,
        )),
    );
    let chunks = Arc::new(Mutex::new(Vec::new()));
    let seen = chunks.clone();
    let (first_chunk, mut first_chunks) = tokio::sync::mpsc::unbounded_channel();
    let outcome = Client
        .v1()
        .name("sakura-desk-proof")
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                if let SessionUpdate::AgentMessageChunk(chunk) = notification.update {
                    if let ContentBlock::Text(text) = chunk.content {
                        seen.lock().expect("the chunks lock").push(text.text);
                        first_chunk.send(()).ok();
                    }
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(lines, async |connection: ConnectionTo<Agent>| {
            let initialized = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            assert_eq!(initialized.protocol_version, ProtocolVersion::V1);
            let session = connection
                .send_request(NewSessionRequest::new("/work/demo"))
                .block_task()
                .await?
                .session_id;
            let plain = connection
                .send_request(prompt(&session, "plain"))
                .block_task()
                .await?
                .stop_reason;
            while first_chunks.try_recv().is_ok() {}
            let slow = connection.send_request(prompt(&session, "slow"));
            first_chunks.recv().await;
            connection.send_notification(CancelNotification::new(session.clone()))?;
            let cancelled = slow.block_task().await?.stop_reason;
            Ok((plain, cancelled))
        })
        .await?;
    let chunks = chunks.lock().expect("the chunks lock").clone();
    Ok((chunks, outcome.0, outcome.1))
}

#[test]
fn sdk_on_the_core_runtime_holds_a_session_with_a_child_over_stdio() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    let (chunks, plain, cancelled) = runtime
        .block_on(async { tokio::time::timeout(DEADLINE, hold_a_session()).await })
        .expect("the session ends before the deadline")
        .expect("the session runs without a protocol error");
    assert_eq!(plain, StopReason::EndTurn);
    assert_eq!(cancelled, StopReason::Cancelled);
    assert!(chunks.concat().starts_with("I read the module"));
    assert!(
        chunks.len() > 10,
        "the answer arrived as a stream of chunks"
    );
    assert!(
        chunks.len() < 400,
        "the slow turn was cut short by the cancel"
    );
}
