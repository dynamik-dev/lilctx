//! MCP boundary tests. We spawn the real `lilctx serve` subprocess and speak
//! JSON-RPC 2.0 over stdio (newline-delimited messages, per the MCP spec).
//!
//! What we're testing: the contract Claude Code consumes — handshake, tool
//! discovery, tool call results. If someone reimplemented the server entirely,
//! these tests should still pass as long as the MCP wire protocol is correct.

// Setup `.unwrap()`/`.expect()` are idiomatic in tests; mirrors the precedent
// set on `src/config.rs`'s `#[cfg(test)] mod tests`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::time::timeout;

use crate::common::TestEnv;

/// Per-request timeout. The server is local and small; anything longer than
/// this means a hang, which is a test failure rather than something to wait on.
const RECV_TIMEOUT: Duration = Duration::from_secs(15);
const PROTOCOL_VERSION: &str = "2024-11-05";

struct McpClient {
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl McpClient {
    fn new(child: &mut tokio::process::Child) -> Self {
        let stdin = child.stdin.take().expect("child stdin must be piped");
        let stdout = BufReader::new(child.stdout.take().expect("child stdout must be piped"));
        Self {
            stdin: Some(stdin),
            stdout,
            next_id: 1,
        }
    }

    /// Close stdin so the server detects EOF and exits cleanly, then wait
    /// for it. A clean exit lets LLVM coverage instrumentation flush its
    /// profile data — `kill_on_drop` skips that flush.
    async fn shutdown(mut self, child: &mut tokio::process::Child) {
        drop(self.stdin.take());
        let _ = timeout(RECV_TIMEOUT, child.wait()).await;
    }

    async fn send(&mut self, msg: &Value) {
        let mut line = serde_json::to_string(msg).expect("serialize JSON-RPC message");
        line.push('\n');
        let stdin = self
            .stdin
            .as_mut()
            .expect("McpClient stdin already closed");
        stdin
            .write_all(line.as_bytes())
            .await
            .expect("write to MCP server stdin");
        stdin.flush().await.expect("flush stdin");
    }

    async fn recv(&mut self) -> Value {
        let mut line = String::new();
        let read = timeout(RECV_TIMEOUT, self.stdout.read_line(&mut line))
            .await
            .expect("MCP server response timed out");
        let n = read.expect("read from MCP server stdout");
        assert!(n > 0, "MCP server closed stdout unexpectedly");
        let parsed: Result<Value, _> = serde_json::from_str(&line);
        assert!(
            parsed.is_ok(),
            "MCP server returned non-JSON line: {err:?}\nline: {line:?}",
            err = parsed.as_ref().err()
        );
        parsed.unwrap()
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.send(&msg).await;
        let resp = self.recv().await;
        assert_eq!(
            resp.get("id").and_then(Value::as_i64),
            Some(id),
            "response id mismatch for {method}: got {resp:?}"
        );
        resp
    }

    async fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.send(&msg).await;
    }

    async fn handshake(&mut self) {
        let resp = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "lilctx-test", "version": "0.0.0"}
                }),
            )
            .await;
        assert!(
            resp.get("result").is_some(),
            "initialize must return a result; got {resp:?}"
        );
        self.notify("notifications/initialized", json!({})).await;
    }
}

fn tool_text(call_response: &Value) -> String {
    let arr = call_response
        .pointer("/result/content")
        .and_then(Value::as_array);
    assert!(
        arr.is_some(),
        "tools/call response missing /result/content array: {call_response}"
    );
    arr.unwrap()
        .iter()
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn mcp_handshake_advertises_search_and_list_paths_tools() {
    let env = TestEnv::new().await;
    let mut child = env.spawn_serve();
    let mut client = McpClient::new(&mut child);

    client.handshake().await;

    let resp = client.request("tools/list", json!({})).await;
    let tools = resp.pointer("/result/tools").and_then(Value::as_array);
    assert!(
        tools.is_some(),
        "tools/list missing /result/tools array: {resp}"
    );
    let names: Vec<&str> = tools
        .unwrap()
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .collect();
    assert!(
        names.contains(&"search"),
        "expected `search` tool; advertised: {names:?}"
    );
    assert!(
        names.contains(&"list_paths"),
        "expected `list_paths` tool; advertised: {names:?}"
    );
    client.shutdown(&mut child).await;
}

#[tokio::test]
async fn mcp_search_returns_indexed_content() {
    let env = TestEnv::new().await;
    env.write_source(
        "notes/alpha.md",
        "# Alpha\nThis note talks about alpha topics in detail.\n",
    );
    env.write_source(
        "notes/golf.md",
        "# Golf\nThis note talks about golf topics in detail.\n",
    );

    // Index synchronously *before* the server starts: serve loads its store
    // at startup and we want a populated table to query.
    env.cmd().arg("index").assert().success();

    let mut child = env.spawn_serve();
    let mut client = McpClient::new(&mut child);
    client.handshake().await;

    let resp = client
        .request(
            "tools/call",
            json!({
                "name": "search",
                "arguments": {"query": "alpha", "limit": 5}
            }),
        )
        .await;

    let body = tool_text(&resp);
    assert!(
        body.contains("alpha.md"),
        "search result must mention the file containing the query term; body:\n{body}"
    );
    client.shutdown(&mut child).await;
}

#[tokio::test]
async fn mcp_list_paths_returns_configured_roots() {
    let env = TestEnv::new().await;
    let configured_root = env.source_dir.path().display().to_string();

    let mut child = env.spawn_serve();
    let mut client = McpClient::new(&mut child);
    client.handshake().await;

    let resp = client
        .request("tools/call", json!({"name": "list_paths", "arguments": {}}))
        .await;

    let body = tool_text(&resp);
    assert!(
        body.contains(&configured_root),
        "list_paths must include the configured source dir; expected {configured_root:?}, got:\n{body}"
    );
    client.shutdown(&mut child).await;
}

#[tokio::test]
async fn mcp_search_with_no_indexed_data_returns_no_results_message() {
    // Empty store: the server still starts, search still works, just nothing
    // to return. The contract is "no error, friendly message" — not an exception.
    let env = TestEnv::new().await;
    let mut child = env.spawn_serve();
    let mut client = McpClient::new(&mut child);
    client.handshake().await;

    let resp = client
        .request(
            "tools/call",
            json!({
                "name": "search",
                "arguments": {"query": "anything"}
            }),
        )
        .await;

    let body = tool_text(&resp);
    assert!(
        body.contains("(no results)"),
        "empty store should produce a friendly empty-result message; got:\n{body}"
    );
    client.shutdown(&mut child).await;
}
