//! Weekly account use from the local Leeway service (`GET /v1/status`). Leeway is optional: when it is not
//! running, does not answer within a second or answers something unexpected, the result is an empty list
//! and the window simply shows nothing about usage. That degrade is the contract, not an error.

use crate::models::AccountUsage;
use serde::Deserialize;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Address of the Leeway service.
pub const LEEWAY_ADDR: &str = "127.0.0.1:8799";
/// Overall deadline for one fetch.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
struct LeewayStatus {
    accounts: Vec<LeewayAccount>,
}

#[derive(Deserialize)]
struct LeewayAccount {
    label: String,
    seven_day: LeewayWindow,
}

#[derive(Deserialize)]
struct LeewayWindow {
    pct: Option<f64>,
    resets_at: Option<String>,
}

/// Fetches the weekly use of every account Leeway can read. Empty on any failure.
pub async fn fetch(addr: &str, deadline: Duration) -> Vec<AccountUsage> {
    let body = tokio::time::timeout(deadline, get_status(addr)).await;
    match body {
        Ok(Some(body)) => parse(&body).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn parse(body: &[u8]) -> Option<Vec<AccountUsage>> {
    let status: LeewayStatus = serde_json::from_slice(body).ok()?;
    let usage = status
        .accounts
        .into_iter()
        .filter_map(|account| {
            Some(AccountUsage {
                label: account.label,
                weekly_percent: account.seven_day.pct?,
                resets_at: account.seven_day.resets_at,
            })
        })
        .collect();
    Some(usage)
}

async fn get_status(addr: &str) -> Option<Vec<u8>> {
    let mut stream = TcpStream::connect(addr).await.ok()?;
    let request = format!(
        "GET /v1/status HTTP/1.0\r\nHost: {addr}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut raw = Vec::new();
    (&mut stream)
        .take(MAX_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut raw)
        .await
        .ok()?;
    if raw.len() > MAX_RESPONSE_BYTES {
        return None;
    }
    body_of_ok_response(&raw)
}

/// Returns the body when the status line says 200, decoding chunked transfer if the server used it.
fn body_of_ok_response(raw: &[u8]) -> Option<Vec<u8>> {
    let split = raw.windows(4).position(|window| window == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_lowercase();
    let body = &raw[split + 4..];
    let is_ok = head.lines().next()?.split_whitespace().nth(1) == Some("200");
    if !is_ok {
        return None;
    }
    if head.contains("transfer-encoding: chunked") {
        return dechunk(body);
    }
    Some(body.to_vec())
}

fn dechunk(mut rest: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = rest.windows(2).position(|window| window == b"\r\n")?;
        let size_text = std::str::from_utf8(&rest[..line_end]).ok()?;
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(rest.get(..size)?);
        rest = rest.get(size + 2..)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    const BODY: &str = r#"{"schema_version":3,"generated_at":"2026-11-10T07:00:00Z","history":{},"accounts":[
        {"id":"claude:a","label":"personal","seven_day":{"pct":42.5,"resets_at":"2026-11-14T00:00:00Z"},"extra":1},
        {"id":"claude:b","label":"work","seven_day":{"pct":null,"resets_at":null}},
        {"id":"codex:c","label":"codex","seven_day":{"pct":7,"resets_at":null}}]}"#;

    async fn serve_once(response: Vec<u8>, stall: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let read = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..read])
                .starts_with("GET /v1/status HTTP/1.0\r\nHost: 127.0.0.1:"));
            if stall {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
            socket.write_all(&response).await.unwrap();
        });
        addr
    }

    fn ok(body: &str) -> Vec<u8> {
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{body}").into_bytes()
    }

    #[tokio::test]
    async fn maps_weekly_use_and_skips_unreadable_accounts() {
        let addr = serve_once(ok(BODY), false).await;
        let usage = fetch(&addr, Duration::from_secs(1)).await;
        assert_eq!(usage.len(), 2);
        assert_eq!(usage[0].label, "personal");
        assert_eq!(usage[0].weekly_percent, 42.5);
        assert_eq!(usage[0].resets_at.as_deref(), Some("2026-11-14T00:00:00Z"));
        assert_eq!(usage[1].resets_at, None);
    }

    #[tokio::test]
    async fn decodes_chunked_responses() {
        let (first, second) = BODY.split_at(40);
        let chunked = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{first}\r\n{:x}\r\n{second}\r\n0\r\n\r\n",
            first.len(),
            second.len()
        );
        let addr = serve_once(chunked.into_bytes(), false).await;
        assert_eq!(fetch(&addr, Duration::from_secs(1)).await.len(), 2);
    }

    #[tokio::test]
    async fn server_errors_give_an_empty_list() {
        let response = b"HTTP/1.1 500 Internal Server Error\r\n\r\n{}".to_vec();
        let addr = serve_once(response, false).await;
        assert!(fetch(&addr, Duration::from_secs(1)).await.is_empty());
    }

    #[tokio::test]
    async fn garbage_body_gives_an_empty_list() {
        let addr = serve_once(ok("<html>"), false).await;
        assert!(fetch(&addr, Duration::from_secs(1)).await.is_empty());
    }

    #[tokio::test]
    async fn a_stalled_service_is_abandoned_at_the_deadline() {
        let addr = serve_once(ok(BODY), true).await;
        let started = std::time::Instant::now();
        assert!(fetch(&addr, Duration::from_millis(300)).await.is_empty());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn nothing_listening_gives_an_empty_list() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        drop(listener);
        assert!(fetch(&addr, Duration::from_secs(1)).await.is_empty());
    }
}
