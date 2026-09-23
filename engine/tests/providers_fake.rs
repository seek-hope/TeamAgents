//! Provider-edge contract tests against a local fake HTTP server (no real
//! model calls): SSE assembly, DeepSeek native fields, usage, retry classes,
//! mid-stream truncation and cancellation.

use serde_json::{json, Value as Json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use teamagents_core::kernel::ModelRequest;
use teamagents_engine::providers::chat_completions::ChatCompletions;
use teamagents_engine::providers::{Cancel, ErrorClass, Provider, ProviderEvent};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct FakeServer {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeServer {
    /// `responses` are raw HTTP responses replayed one per request.
    async fn start(responses: Vec<String>) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(vec![]));
        let seen = requests.clone();
        let task = tokio::spawn(async move {
            for response in responses {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let mut buf = vec![0u8; 65536];
                let mut head = Vec::new();
                // Read headers, then the advertised body.
                loop {
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    head.extend_from_slice(&buf[..n]);
                    if let Some(pos) = find(&head, b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&head[..pos]).to_string();
                        let content_length: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length: ").or_else(|| line.strip_prefix("Content-Length: "))
                            })
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        let mut body = head[pos + 4..].to_vec();
                        while body.len() < content_length {
                            let n = socket.read(&mut buf).await.unwrap_or(0);
                            if n == 0 {
                                break;
                            }
                            body.extend_from_slice(&buf[..n]);
                        }
                        seen.lock().unwrap().push(format!(
                            "{}{}",
                            String::from_utf8_lossy(&head[..pos]),
                            String::from_utf8_lossy(&body)
                        ));
                        break;
                    }
                }
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                drop(socket);
            }
        });
        FakeServer { base: format!("http://{addr}"), requests, task }
    }

    fn last_request(&self) -> String {
        self.requests.lock().unwrap().last().cloned().unwrap_or_default()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn sse_response(events: &str) -> String {
    format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{events}")
}

fn json_response(status: &str, body: &Json) -> String {
    let text = body.to_string();
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
        text.len()
    )
}

fn request() -> ModelRequest {
    ModelRequest {
        request_id: "req-1".into(),
        model: "deepseek-flash".into(),
        messages: vec![json!({"role":"user","content":"hi"})],
        tools: vec![],
        options: json!({}),
        est_prompt_tokens: 5,
    }
}

fn run(
    rt: &tokio::runtime::Runtime,
    provider: &ChatCompletions,
    request: &ModelRequest,
) -> Result<teamagents_engine::providers::AttemptOutcome, teamagents_engine::providers::ProviderError> {
    rt.block_on(async {
        let cancel = Cancel::new();
        provider.complete(request, &cancel, &mut |_| {}).await
    })
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
}

#[test]
fn sse_assembles_reasoning_tool_calls_and_usage() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\",\"reasoning_content\":\"think\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"comma\"}}]}}]}\n\n\
         data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"nd\\\":\\\"ls\\\"}\"}}]}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":4,\"total_tokens\":14}}\n\n\
         data: [DONE]\n\n",
    )]));
    let provider = ChatCompletions::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let outcome = run(&rt, &provider, &request()).unwrap();
    assert_eq!(outcome.response.message["content"], "Hello");
    assert_eq!(outcome.response.message["reasoning_content"], "think");
    assert_eq!(outcome.response.message["tool_calls"][0]["function"]["arguments"], "{\"command\":\"ls\"}");
    let usage = outcome.response.usage.unwrap();
    assert_eq!((usage.prompt, usage.completion, usage.total), (10, 4, 14));
    // request body carried the stream contract
    let sent = server.last_request();
    assert!(sent.contains("\"stream\":true"), "body was: {sent}");
    assert!(sent.contains("/chat/completions"));
    rt.block_on(server.task).unwrap();
}

#[test]
fn non_sse_json_body_is_accepted() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![json_response(
        "200 OK",
        &json!({"choices":[{"message":{"role":"assistant","content":"plain"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}),
    )]));
    let provider = ChatCompletions::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let outcome = run(&rt, &provider, &request()).unwrap();
    assert_eq!(outcome.response.message["content"], "plain");
    assert_eq!(outcome.response.usage.unwrap().total, 5);
    rt.block_on(server.task).unwrap();
}

#[test]
fn status_classes_drive_retry_decisions() {
    let rt = runtime();
    for (status, body, class) in [
        ("429 Too Many Requests", json!({"error":{"message":"slow down"}}), ErrorClass::Transient),
        ("500 Internal Server Error", json!({"error":{"message":"boom"}}), ErrorClass::Transient),
        (
            "400 Bad Request",
            json!({"error":{"message":"context_length_exceeded: too many tokens"}}),
            ErrorClass::ContextOverflow,
        ),
        ("401 Unauthorized", json!({"error":{"message":"bad key"}}), ErrorClass::Permanent),
    ] {
        let server = rt.block_on(FakeServer::start(vec![json_response(status, &body)]));
        let provider = ChatCompletions::new(&server.base, "k", Duration::from_secs(5)).unwrap();
        let error = run(&rt, &provider, &request()).unwrap_err();
        assert_eq!(error.class, class, "status {status}");
        rt.block_on(server.task).unwrap();
    }
}

#[test]
fn truncated_stream_before_output_is_transient() {
    let rt = runtime();
    // connection closes without [DONE] and without any visible text
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\"}]}}]}\n\n",
    )]));
    let provider = ChatCompletions::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let error = run(&rt, &provider, &request()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Transient);
    assert!(error.message.contains("ended before completion"));
    rt.block_on(server.task).unwrap();
}

#[test]
fn truncated_stream_after_visible_output_is_permanent() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
    )]));
    let provider = ChatCompletions::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let error = run(&rt, &provider, &request()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Permanent);
    rt.block_on(server.task).unwrap();
}

#[test]
fn cancellation_abandons_the_read() {
    let rt = runtime();
    // a stalled server: accepts then never answers within the test
    let server = rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = socket.read(&mut buf).await;
            tokio::time::sleep(Duration::from_secs(30)).await;
            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}").await;
        });
        (format!("http://{addr}"), task)
    });
    let provider = ChatCompletions::new(&server.0, "k", Duration::from_secs(60)).unwrap();
    let cancel = Cancel::new();
    let token = cancel.clone();
    let started = std::time::Instant::now();
    let error = rt.block_on(async move {
        let trigger = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            token.cancel();
        });
        let result = provider.complete(&request(), &cancel, &mut |_| {}).await.unwrap_err();
        trigger.abort();
        result
    });
    assert_eq!(error.class, ErrorClass::Interrupted);
    assert!(started.elapsed() < Duration::from_secs(5), "cancel took {:?}", started.elapsed());
    server.1.abort();
}

#[test]
fn text_deltas_arrive_as_preview_events() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    )]));
    let provider = ChatCompletions::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let collected = events.clone();
    rt.block_on(async {
        let cancel = Cancel::new();
        provider
            .complete(&request(), &cancel, &mut |event| {
                let ProviderEvent::TextDelta(text) = event;
                collected.lock().unwrap().push(text);
            })
            .await
            .unwrap();
    });
    assert_eq!(events.lock().unwrap().join(""), "ab");
    rt.block_on(server.task).unwrap();
}
