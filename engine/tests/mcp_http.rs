//! MCP streamable HTTP transport: a hand-rolled std::net fake server covers
//! initialize (with mcp-session-id) → notifications/initialized (202) →
//! tools/list (plain JSON) → tools/call (SSE stream), plus optional/required
//! degradation and malformed-JSON robustness.

use serde_json::{json, Value as Json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use teamagents_core::models::UserConfig;
use teamagents_engine::bound::BoundTools;
use teamagents_engine::mcp::McpClient;

#[derive(Clone, Copy)]
enum Mode {
    Good,
    /// tools/list answers with a body that is not JSON at all.
    BadJson,
    /// the server refuses the GET push stream and session DELETE with 405
    NoPush,
}

#[derive(Default)]
struct Seen {
    http_method: String,
    method: String,
    session: Option<String>,
    authorization: Option<String>,
    protocol: Option<String>,
    body: Json,
}

const SESSION_ID: &str = "ta-test-session";

/// POST /mcp server with `connection: close` on every response, so each RPC is
/// one short-lived connection and the accept loop never has to keep state.
fn spawn_server(mode: Mode) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_in_loop = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let seen = seen_in_loop.clone();
            std::thread::spawn(move || handle(stream, seen, mode));
        }
    });
    (url, seen)
}

fn handle(stream: TcpStream, seen: Arc<Mutex<Vec<Seen>>>, mode: Mode) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut content_length = 0usize;
    let mut record = Seen::default();
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return; // request line
    }
    record.http_method = line.split_whitespace().next().unwrap_or("").to_string();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        // header names are case-insensitive; values are not
        let lower = trimmed.to_ascii_lowercase();
        let value = |name: &str| lower.starts_with(name).then(|| trimmed[name.len()..].trim());
        if let Some(v) = value("content-length:") {
            content_length = v.parse().unwrap_or(0);
        } else if let Some(v) = value("mcp-session-id:") {
            record.session = Some(v.to_string());
        } else if let Some(v) = value("authorization:") {
            record.authorization = Some(v.to_string());
        } else if let Some(v) = value("mcp-protocol-version:") {
            record.protocol = Some(v.to_string());
        }
    }
    let mut body = vec![0u8; content_length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let message: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
    let method = message.get("method").and_then(|v| v.as_str()).unwrap_or("").to_string();
    record.method = method.clone();
    record.body = message.clone();
    seen.lock().unwrap().push(record);
    let id = message.get("id").cloned().unwrap_or(Json::Null);
    let mut stream = stream;
    // the GET push stream carries server -> client messages
    if method.is_empty() {
        let push = || {
            "data: {\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"sampling/createMessage\",\"params\":{}}\r\n\r\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\r\n\r\n".to_string()
        };
        return match mode {
            Mode::NoPush => respond(&mut stream, "405 Method Not Allowed", "", ""),
            _ => respond(&mut stream, "200 OK", "content-type: text/event-stream\r\n", &push()),
        };
    }
    match (mode, method.as_str()) {
        (_, "initialize") => respond(
            &mut stream,
            "200 OK",
            "content-type: application/json\r\nmcp-session-id: ta-test-session\r\n",
            &json!({"jsonrpc": "2.0", "id": id, "result": {
                // Negotiate a different supported version to catch hardcoding.
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake-http-mcp", "version": "0"},
            }})
            .to_string(),
        ),
        (_, m) if m.starts_with("notifications/") => respond(&mut stream, "202 Accepted", "", ""),
        (Mode::BadJson, "tools/list") => respond(&mut stream, "200 OK", "content-type: application/json\r\n", "this is not json"),
        (_, "tools/list") => respond(
            &mut stream,
            "200 OK",
            "content-type: application/json\r\n",
            &json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{
                "name": "echo",
                "description": "Echo the given text N times.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"text": {"type": "string"}, "times": {"type": "integer"}},
                    "required": ["text"],
                },
            }]}})
            .to_string(),
        ),
        (_, "tools/call") => {
            let args = message.pointer("/params/arguments").cloned().unwrap_or(json!({}));
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let times = args.get("times").and_then(|v| v.as_i64()).unwrap_or(1).max(0) as usize;
            let echoed = std::iter::repeat(text).take(times).collect::<Vec<_>>().join(" ");
            let payload = json!({"jsonrpc": "2.0", "id": id,
                "result": {"content": [{"type": "text", "text": echoed}]}});
            respond(
                &mut stream,
                "200 OK",
                "content-type: text/event-stream\r\n",
                &format!("data: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}}\r\n\r\nevent: message\r\n{}\r\n\r\ndata: {{\"id\":999,\"result\":\"unrelated\"}}\r\n\r\n",
                    serde_json::to_string_pretty(&payload).unwrap().lines().map(|line| format!("data: {line}")).collect::<Vec<_>>().join("\r\n")),
            )
        }
        _ => respond(
            &mut stream,
            "200 OK",
            "content-type: application/json\r\n",
            &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "no such method"}}).to_string(),
        ),
    }
}

fn respond(stream: &mut TcpStream, status: &str, headers: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n{headers}\r\n{body}",
        body.len()
    );
}

fn http_binding(url: &str, extra: Json) -> teamagents_core::models::ToolBinding {
    let mut value = json!({"kind": "mcp", "mcp_transport": "http", "url": url});
    value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    serde_json::from_value(value).unwrap()
}

/// Full path: bind by config, tools/list over plain JSON, tools/call over SSE,
/// the session id and bearer token reach the server on every post-initialize RPC.
#[test]
fn http_transport_binds_and_calls_tools() {
    let (url, seen) = spawn_server(Mode::Good);
    std::env::set_var("TA_MCP_HTTP_TEST_TOKEN", "test-secret");
    let mut catalog = UserConfig::default();
    catalog.tools.insert(
        "remote".into(),
        http_binding(
            &url,
            json!({"bearer_token_env_var": "TA_MCP_HTTP_TEST_TOKEN", "startup_timeout_s": 5, "tool_timeout_s": 5}),
        ),
    );
    let bound = BoundTools::load(&catalog, &["remote".to_string()]).expect("http service binds");
    assert!(bound.names().contains("remote_echo"), "{:?}", bound.names());
    let result = bound
        .call("remote_echo", &json!({"text": "hello", "times": 2}))
        .expect("a bound tool")
        .expect("call succeeds");
    assert_eq!(result, json!("hello hello"));
    bound.close();

    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|r| r.method == "initialize"), "{seen:?}");
    assert!(seen.iter().any(|r| r.method == "notifications/initialized"), "{seen:?}");
    for method in ["notifications/initialized", "tools/list", "tools/call"] {
        let request = seen.iter().find(|r| r.method == method).unwrap_or_else(|| panic!("{method} was called"));
        assert_eq!(request.session.as_deref(), Some(SESSION_ID), "{method} carries the session id");
        assert_eq!(request.authorization.as_deref(), Some("Bearer test-secret"), "{method} carries the token");
        assert_eq!(request.protocol.as_deref(), Some("2025-03-26"), "{method} carries the negotiated version");
    }
}

/// Server push: the client opens the GET SSE stream, answers a server-initiated
/// request it does not implement instead of leaving the server hanging, and
/// terminates the session with DELETE.
#[test]
fn http_push_stream_answers_requests_and_deletes_the_session() {
    let (url, seen) = spawn_server(Mode::Good);
    let client = McpClient::connect_http(&url, None, 5, 5).expect("connect");
    assert!(client.tools().is_ok());

    // the reader thread needs a moment to answer the pushed request
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let reply_seen = || {
        seen.lock()
            .unwrap()
            .iter()
            .any(|r| r.body.get("id") == Some(&json!(99)) && r.body.get("error").is_some())
    };
    while !reply_seen() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(reply_seen(), "the pushed sampling request got an error reply: {seen:?}", seen = seen.lock().unwrap().len());

    client.close();
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|r| r.http_method == "GET"), "a GET push stream was opened: {seen:?}");
    assert!(
        seen.iter().any(|r| r.http_method == "GET" && r.session.as_deref() == Some(SESSION_ID)),
        "the stream carries the session id"
    );
    let delete = seen.iter().find(|r| r.http_method == "DELETE").expect("session terminated with DELETE");
    assert_eq!(delete.session.as_deref(), Some(SESSION_ID), "{delete:?}");
}

/// Servers without push support: 405 on GET and DELETE must be a no-op.
#[test]
fn http_transport_tolerates_servers_without_push_or_delete() {
    let (url, seen) = spawn_server(Mode::NoPush);
    let client = McpClient::connect_http(&url, None, 5, 5).expect("connect still succeeds");
    assert!(client.tools().is_ok(), "requests keep working over POST");
    client.close();
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|r| r.http_method == "GET"), "{seen:?}");
    assert!(seen.iter().any(|r| r.http_method == "DELETE"), "{seen:?}");
}

/// An optional http service that is down only drops the capability; a required
/// one fails the load. (Port 1 refuses connections deterministically.)
#[test]
fn optional_http_failure_only_drops_the_capability() {
    let dead = "http://127.0.0.1:1/mcp";
    let mut catalog = UserConfig::default();
    catalog.tools.insert("dead".into(), http_binding(dead, json!({"startup_timeout_s": 2})));
    let bound = BoundTools::load(&catalog, &["dead".to_string()]).expect("optional failure is tolerated");
    assert!(bound.tools.is_empty());

    catalog.tools.insert("dead-required".into(), http_binding(dead, json!({"required": true, "startup_timeout_s": 2})));
    let err = BoundTools::load(&catalog, &["dead-required".to_string()]).err().expect("required failure is fatal");
    assert!(err.contains("unavailable"), "{err}");
}

/// A server that answers tools/list with garbage produces an error, not a panic.
#[test]
fn malformed_server_json_is_an_error_not_a_crash() {
    let (url, _seen) = spawn_server(Mode::BadJson);
    let client = McpClient::connect_http(&url, None, 5, 5).expect("the handshake itself is fine");
    let err = client.tools().err().expect("bad json surfaces as an error");
    assert!(err.contains("bad json"), "{err}");
}

impl std::fmt::Debug for Seen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Seen").field("method", &self.method).field("session", &self.session).finish()
    }
}
