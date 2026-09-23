//! Provider-edge contract tests against a local fake HTTP server (no real
//! model calls): SSE assembly, DeepSeek native fields, usage, retry classes,
//! mid-stream truncation and cancellation.

use serde_json::{json, Value as Json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use teamagents_core::kernel::ModelRequest;
use teamagents_engine::providers::anthropic::Anthropic;
use teamagents_engine::providers::chat_completions::ChatCompletions;
use teamagents_engine::providers::responses::Responses;
use teamagents_engine::providers::{AttemptOutcome, Cancel, ErrorClass, Provider, ProviderError, ProviderEvent};
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
                            "{}\r\n\r\n{}",
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

fn run<P: Provider>(
    rt: &tokio::runtime::Runtime,
    provider: &P,
    request: &ModelRequest,
) -> Result<AttemptOutcome, ProviderError> {
    rt.block_on(async {
        let cancel = Cancel::new();
        provider.complete(request, &cancel, &mut |_| {}).await
    })
}

/// Recorded raw request → parsed JSON body.
fn request_body(server: &FakeServer) -> Json {
    let raw = server.last_request();
    let pos = raw.find("\r\n\r\n").expect("recorded request carries a body");
    serde_json::from_str(&raw[pos + 4..]).expect("request body is JSON")
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

fn responses_request() -> ModelRequest {
    ModelRequest {
        request_id: "req-1".into(),
        model: "gpt-5".into(),
        messages: vec![
            json!({"role":"system","content":"You are lead."}),
            json!({"role":"system","content":"Be brief."}),
            json!({"role":"user","content":"hi"}),
            json!({"role":"assistant","content":"done","tool_calls":[{"id":"c1","type":"function","function":{"name":"shell","arguments":"{}"}}]}),
            json!({"role":"tool","tool_call_id":"c1","content":"{}"}),
            json!({"role":"assistant","responses_output":[{"type":"reasoning","id":"r1"},{"type":"message","role":"assistant","content":[{"type":"output_text","text":"kept"}]}]}),
        ],
        tools: vec![
            json!({"type":"function","function":{"name":"shell","description":"run","parameters":{"type":"object"}}}),
        ],
        options: json!({"max_tokens": 1024, "reasoning_effort": "low", "temperature": 0.2}),
        est_prompt_tokens: 5,
    }
}

const RESPONSES_COMPLETED: &str = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"ok\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n";

#[test]
fn responses_request_body_translates_history_tools_and_options() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![sse_response(RESPONSES_COMPLETED)]));
    let provider = Responses::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    run(&rt, &provider, &responses_request()).unwrap();
    let raw = server.last_request();
    assert!(raw.starts_with("POST /responses "), "request line: {}", raw.lines().next().unwrap_or(""));
    let body = request_body(&server);
    assert_eq!(body["instructions"], "You are lead.\n\nBe brief.");
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_output_tokens"], 1024, "max_tokens maps to max_output_tokens");
    assert!(body.get("max_tokens").is_none());
    assert_eq!(body["reasoning"], json!({"effort": "low"}), "reasoning_effort maps to reasoning.effort");
    assert!(body.get("reasoning_effort").is_none());
    assert_eq!(body["temperature"], 0.2, "other options pass through verbatim");
    assert_eq!(
        body["tools"],
        json!([{"type":"function","name":"shell","description":"run","parameters":{"type":"object"}}]),
        "tools are flattened to the Responses shape"
    );
    let input = body["input"].as_array().unwrap();
    assert_eq!(input[0], json!({"role":"user","content":[{"type":"input_text","text":"hi"}]}));
    assert_eq!(input[1], json!({"role":"assistant","content":[{"type":"output_text","text":"done"}]}));
    assert_eq!(input[2], json!({"type":"function_call","call_id":"c1","name":"shell","arguments":"{}"}));
    assert_eq!(input[3], json!({"type":"function_call_output","call_id":"c1","output":"{}"}));
    // Native continuation: the original items are replayed verbatim (§7).
    assert_eq!(input[4], json!({"type":"reasoning","id":"r1"}));
    assert_eq!(input[5], json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"kept"}]}));
    assert_eq!(input.len(), 6);
    rt.block_on(server.task).unwrap();
}

#[test]
fn responses_sse_assembles_items_usage_and_native() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n\
         data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n\
         data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"secret\"}\n\n\
         data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\",\"status\":\"completed\"}}\n\n\
         data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"reasoning\",\"id\":\"rs_1\",\"status\":\"completed\"},{\"type\":\"message\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\"}]},{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"shell\",\"arguments\":\"{\\\"cmd\\\":\\\"ls\\\"}\",\"status\":\"completed\"}],\"usage\":{\"input_tokens\":10,\"output_tokens\":4,\"total_tokens\":14}}}\n\n",
    )]));
    let provider = Responses::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let collected = events.clone();
    let outcome = rt
        .block_on(async {
            let cancel = Cancel::new();
            provider
                .complete(&responses_request(), &cancel, &mut |event| {
                    let ProviderEvent::TextDelta(text) = event;
                    collected.lock().unwrap().push(text);
                })
                .await
        })
        .unwrap();
    assert_eq!(events.lock().unwrap().join(""), "Hello", "text deltas stream as previews");
    let message = &outcome.response.message;
    assert_eq!(message["content"], "Hello");
    assert_eq!(message["tool_calls"][0]["id"], "call_1");
    assert_eq!(message["tool_calls"][0]["function"]["name"], "shell");
    assert_eq!(message["tool_calls"][0]["function"]["arguments"], "{\"cmd\":\"ls\"}");
    let output = message["responses_output"].as_array().unwrap();
    assert_eq!(output.len(), 3, "the original items survive for continuation");
    assert_eq!(output[0]["type"], "reasoning", "opaque reasoning keeps its place in the ordering");
    let usage = outcome.response.usage.unwrap();
    assert_eq!((usage.prompt, usage.completion, usage.total), (10, 4, 14));
    assert_eq!(outcome.response.native["protocol"], "responses");
    rt.block_on(server.task).unwrap();
}

#[test]
fn responses_text_only_stream_synthesizes_a_message_item() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"plain\"}\n\n\
         data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
    )]));
    let provider = Responses::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let outcome = run(&rt, &provider, &responses_request()).unwrap();
    assert_eq!(outcome.response.message["content"], "plain");
    assert_eq!(outcome.response.usage.unwrap().total, 2);
    rt.block_on(server.task).unwrap();
}

#[test]
fn responses_incomplete_and_failed_events_are_permanent() {
    let rt = runtime();
    for events in [
        "data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
        "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"boom\"}}}\n\n",
        // completed envelope but the payload says otherwise (ported contract)
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"output\":[]}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"function_call\",\"status\":\"incomplete\",\"call_id\":\"p\",\"name\":\"shell\",\"arguments\":\"{}\"}]}}\n\n",
    ] {
        let server = rt.block_on(FakeServer::start(vec![sse_response(events)]));
        let provider = Responses::new(&server.base, "k", Duration::from_secs(5)).unwrap();
        let error = run(&rt, &provider, &responses_request()).unwrap_err();
        assert_eq!(error.class, ErrorClass::Permanent, "events: {events}");
        rt.block_on(server.task).unwrap();
    }
}

#[test]
fn responses_truncated_stream_classes() {
    let rt = runtime();
    // only partial tool items, nothing visible: safe to retry
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"c1\"}}\n\n",
    )]));
    let provider = Responses::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let error = run(&rt, &provider, &responses_request()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Transient);
    assert!(error.message.contains("ended before completion"));
    rt.block_on(server.task).unwrap();
    // visible text already went out: never replay
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
    )]));
    let provider = Responses::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let error = run(&rt, &provider, &responses_request()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Permanent);
    rt.block_on(server.task).unwrap();
}

#[test]
fn responses_non_sse_json_body() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![json_response(
        "200 OK",
        &json!({"status":"completed",
                "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"plain"}]}],
                "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}),
    )]));
    let provider = Responses::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let outcome = run(&rt, &provider, &responses_request()).unwrap();
    assert_eq!(outcome.response.message["content"], "plain");
    assert_eq!(outcome.response.usage.unwrap().total, 5);
    rt.block_on(server.task).unwrap();
}

#[test]
fn responses_empty_output_is_rejected() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{}}}\n\n",
    )]));
    let provider = Responses::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let error = run(&rt, &provider, &responses_request()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Permanent);
    assert!(error.message.contains("empty output"), "{}", error.message);
    rt.block_on(server.task).unwrap();
}

#[test]
fn responses_status_classes_drive_retry_decisions() {
    let rt = runtime();
    for (status, body, class) in [
        ("429 Too Many Requests", json!({"error":{"message":"slow down"}}), ErrorClass::Transient),
        ("400 Bad Request", json!({"error":{"message":"prompt is too long"}}), ErrorClass::ContextOverflow),
        ("401 Unauthorized", json!({"error":{"message":"bad key"}}), ErrorClass::Permanent),
    ] {
        let server = rt.block_on(FakeServer::start(vec![json_response(status, &body)]));
        let provider = Responses::new(&server.base, "k", Duration::from_secs(5)).unwrap();
        let error = run(&rt, &provider, &responses_request()).unwrap_err();
        assert_eq!(error.class, class, "status {status}");
        rt.block_on(server.task).unwrap();
    }
}

fn anthropic_request() -> ModelRequest {
    ModelRequest {
        request_id: "req-1".into(),
        model: "claude-sonnet".into(),
        messages: vec![
            json!({"role":"system","content":"You are lead."}),
            json!({"role":"system","content":"Be brief."}),
            json!({"role":"user","content":"hi"}),
            json!({"role":"assistant","content":"done","tool_calls":[{"id":"t1","type":"function","function":{"name":"shell","arguments":"{\"cmd\":\"ls\"}"}}]}),
            json!({"role":"tool","tool_call_id":"t1","content":"{}"}),
            json!({"role":"tool","tool_call_id":"t2","content":"[]"}),
            json!({"role":"assistant","anthropic_blocks":[{"type":"thinking","thinking":"hmm","signature":"sig"},{"type":"text","text":"kept"}]}),
        ],
        tools: vec![
            json!({"type":"function","function":{"name":"shell","description":"run","parameters":{"type":"object"}}}),
        ],
        options: json!({"max_tokens": 2048, "reasoning_effort": "high", "temperature": 0.2}),
        est_prompt_tokens: 5,
    }
}

const ANTHROPIC_TEXT_DONE: &str =
    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n\
     data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
     data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n\
     data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n\
     data: {\"type\":\"message_stop\"}\n\n";

#[test]
fn anthropic_request_body_translates_history_tools_and_options() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![sse_response(ANTHROPIC_TEXT_DONE)]));
    // the /v1 suffix on the configured base is trimmed, not duplicated
    let provider = Anthropic::new(format!("{}/v1", server.base), "k", Duration::from_secs(5)).unwrap();
    run(&rt, &provider, &anthropic_request()).unwrap();
    let raw = server.last_request();
    assert!(raw.starts_with("POST /v1/messages "), "request line: {}", raw.lines().next().unwrap_or(""));
    assert!(raw.contains("anthropic-version: 2023-06-01"), "headers: {raw}");
    assert!(raw.contains("x-api-key: k"), "headers: {raw}");
    let body = request_body(&server);
    assert_eq!(body["system"], "You are lead.\n\nBe brief.");
    assert_eq!(body["max_tokens"], 2048, "max_tokens is required by this API");
    assert_eq!(body["output_config"], json!({"effort": "high"}), "reasoning_effort maps to output_config.effort");
    assert!(body.get("reasoning_effort").is_none());
    assert_eq!(body["temperature"], 0.2, "other options pass through verbatim");
    assert_eq!(body["stream"], true);
    assert_eq!(
        body["tools"],
        json!([{"name":"shell","description":"run","input_schema":{"type":"object"}}]),
        "tools are flattened to the Anthropic shape"
    );
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0], json!({"role":"user","content":[{"type":"text","text":"hi"}]}));
    assert_eq!(
        messages[1],
        json!({"role":"assistant","content":[
            {"type":"text","text":"done"},
            {"type":"tool_use","id":"t1","name":"shell","input":{"cmd":"ls"}}]})
    );
    // consecutive tool results share one user message (API rule)
    assert_eq!(
        messages[2],
        json!({"role":"user","content":[
            {"type":"tool_result","tool_use_id":"t1","content":"{}"},
            {"type":"tool_result","tool_use_id":"t2","content":"[]"}]})
    );
    // native continuation: signed thinking replays verbatim (§7)
    assert_eq!(
        messages[3],
        json!({"role":"assistant","content":[
            {"type":"thinking","thinking":"hmm","signature":"sig"},
            {"type":"text","text":"kept"}]})
    );
    assert_eq!(messages.len(), 4);
    rt.block_on(server.task).unwrap();
}

#[test]
fn anthropic_sse_assembles_blocks_usage_and_native() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n\
         data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
         data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n\
         data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n\
         data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"shell\",\"input\":{}}}\n\n\
         data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"cmd\\\":\"}}\n\n\
         data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"ls\\\"}\"}}\n\n\
         data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n\n\
         data: {\"type\":\"message_stop\"}\n\n",
    )]));
    let provider = Anthropic::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let collected = events.clone();
    let outcome = rt
        .block_on(async {
            let cancel = Cancel::new();
            provider
                .complete(&anthropic_request(), &cancel, &mut |event| {
                    let ProviderEvent::TextDelta(text) = event;
                    collected.lock().unwrap().push(text);
                })
                .await
        })
        .unwrap();
    assert_eq!(events.lock().unwrap().join(""), "Hello", "text deltas stream as previews");
    let message = &outcome.response.message;
    assert_eq!(message["content"], "Hello");
    assert_eq!(message["tool_calls"][0]["id"], "toolu_1");
    assert_eq!(message["tool_calls"][0]["function"]["name"], "shell");
    assert_eq!(message["tool_calls"][0]["function"]["arguments"], "{\"cmd\":\"ls\"}");
    let blocks = message["anthropic_blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 2, "the original blocks survive for continuation");
    assert_eq!(blocks[1]["input"], json!({"cmd": "ls"}), "streamed arguments parse into the block");
    let usage = outcome.response.usage.unwrap();
    assert_eq!((usage.prompt, usage.completion, usage.total), (10, 7, 17));
    assert_eq!(outcome.response.native["protocol"], "anthropic");
    rt.block_on(server.task).unwrap();
}

#[test]
fn anthropic_thinking_blocks_stay_verbatim() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n\
         data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n\
         data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n\
         data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig1\"}}\n\n\
         data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
         data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"answer\"}}\n\n\
         data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n\
         data: {\"type\":\"message_stop\"}\n\n",
    )]));
    let provider = Anthropic::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let outcome = run(&rt, &provider, &anthropic_request()).unwrap();
    let message = &outcome.response.message;
    assert_eq!(message["content"], "answer");
    assert_eq!(
        message["anthropic_blocks"][0],
        json!({"type":"thinking","thinking":"hmm","signature":"sig1"}),
        "thinking + signature accumulate verbatim for the signed replay"
    );
    rt.block_on(server.task).unwrap();
}

#[test]
fn anthropic_truncated_stream_classes() {
    let rt = runtime();
    // only partial tool arguments, nothing visible: safe to retry
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t\",\"name\":\"shell\"}}\n\n\
         data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\"}}\n\n",
    )]));
    let provider = Anthropic::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let error = run(&rt, &provider, &anthropic_request()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Transient);
    assert!(error.message.contains("ended before completion"));
    rt.block_on(server.task).unwrap();
    // visible text already went out: never replay
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
         data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
    )]));
    let provider = Anthropic::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let error = run(&rt, &provider, &anthropic_request()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Permanent);
    rt.block_on(server.task).unwrap();
}

#[test]
fn anthropic_incomplete_stop_reasons_are_permanent() {
    let rt = runtime();
    for reason in ["max_tokens", "model_context_window_exceeded", "pause_turn"] {
        let events = format!(
            "data: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}}}}}}\n\n\
             data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n\
             data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"cut\"}}}}\n\n\
             data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"{reason}\"}},\"usage\":{{\"output_tokens\":3}}}}\n\n\
             data: {{\"type\":\"message_stop\"}}\n\n"
        );
        let server = rt.block_on(FakeServer::start(vec![sse_response(&events)]));
        let provider = Anthropic::new(&server.base, "k", Duration::from_secs(5)).unwrap();
        let error = run(&rt, &provider, &anthropic_request()).unwrap_err();
        assert_eq!(error.class, ErrorClass::Permanent, "stop_reason {reason}");
        assert!(error.message.contains(reason), "{}", error.message);
        rt.block_on(server.task).unwrap();
    }
}

#[test]
fn anthropic_protocol_violations_are_permanent() {
    let rt = runtime();
    // delta for a block that never started
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"x\"}}\n\n",
    )]));
    let provider = Anthropic::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let error = run(&rt, &provider, &anthropic_request()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Permanent);
    assert!(error.message.contains("delta without content block"), "{}", error.message);
    rt.block_on(server.task).unwrap();
    // streamed tool arguments that do not parse
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t\",\"name\":\"shell\"}}\n\n\
         data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{bad\"}}\n\n\
         data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{}}\n\n\
         data: {\"type\":\"message_stop\"}\n\n",
    )]));
    let provider = Anthropic::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let error = run(&rt, &provider, &anthropic_request()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Permanent);
    assert!(error.message.contains("invalid streamed tool arguments"), "{}", error.message);
    rt.block_on(server.task).unwrap();
}

#[test]
fn anthropic_non_sse_json_body_and_empty_content() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![json_response(
        "200 OK",
        &json!({"content":[{"type":"text","text":"plain"}],"stop_reason":"end_turn",
                "usage":{"input_tokens":3,"output_tokens":2}}),
    )]));
    let provider = Anthropic::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let outcome = run(&rt, &provider, &anthropic_request()).unwrap();
    assert_eq!(outcome.response.message["content"], "plain");
    assert_eq!(outcome.response.usage.unwrap().total, 5);
    rt.block_on(server.task).unwrap();
    // an empty message is not a usable turn
    let server = rt.block_on(FakeServer::start(vec![json_response(
        "200 OK",
        &json!({"content":[],"stop_reason":"end_turn","usage":{}}),
    )]));
    let provider = Anthropic::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let error = run(&rt, &provider, &anthropic_request()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Permanent);
    assert!(error.message.contains("empty content"), "{}", error.message);
    rt.block_on(server.task).unwrap();
}

#[test]
fn chat_completions_body_strips_native_continuation_fields() {
    // §7: a history crossing protocols must not forward old native blocks.
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![sse_response(
        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    )]));
    let provider = ChatCompletions::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    let mut req = request();
    req.messages = vec![
        json!({"role":"assistant","content":"done",
               "responses_output":[{"type":"reasoning","encrypted_content":"opaque"}],
               "anthropic_blocks":[{"type":"thinking","signature":"signed"}]}),
        json!({"role":"user","content":"hi"}),
    ];
    run(&rt, &provider, &req).unwrap();
    let body = request_body(&server);
    assert_eq!(body["messages"][0], json!({"role":"assistant","content":"done"}));
    assert_eq!(body["messages"][1], json!({"role":"user","content":"hi"}));
    let wire = body.to_string();
    assert!(!wire.contains("opaque") && !wire.contains("signed"), "{wire}");
    rt.block_on(server.task).unwrap();
}

// ---- R17 follow-ups ported from pi-ai (transform-messages / simple-options) ----

const CHAT_TEXT_DONE: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\ndata: [DONE]\n\n";

#[test]
fn tool_call_ids_normalize_for_cross_protocol_continuation() {
    let rt = runtime();
    // a Responses-protocol id (long, with '|') stored in the context
    let long_id = format!("fc_{}|{}", "x".repeat(80), "y".repeat(80));
    let make_request = || {
        let long_id = long_id.clone();
        ModelRequest {
            request_id: "req-1".into(),
            model: "claude-sonnet".into(),
            messages: vec![
                json!({"role":"user","content":"hi"}),
                json!({"role":"assistant","content":"done","tool_calls":[
                    {"id": long_id, "type":"function","function":{"name":"shell","arguments":"{}"}},
                    {"id":"toolu_ok","type":"function","function":{"name":"grep","arguments":"{}"}}]}),
                json!({"role":"tool","tool_call_id": long_id,"content":"{}"}),
            ],
            tools: vec![],
            options: json!({}),
            est_prompt_tokens: 5,
        }
    };
    let server =
        rt.block_on(FakeServer::start(vec![sse_response(ANTHROPIC_TEXT_DONE), sse_response(ANTHROPIC_TEXT_DONE)]));
    let provider = Anthropic::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    run(&rt, &provider, &make_request()).unwrap();
    let body = request_body(&server);
    let messages = body["messages"].as_array().unwrap();
    let uses = messages[1]["content"].as_array().unwrap();
    let rewritten = uses[1]["id"].as_str().unwrap().to_string();
    assert!(
        !rewritten.is_empty()
            && rewritten.len() <= 64
            && rewritten.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "Anthropic rejects anything else: {rewritten}"
    );
    assert_ne!(rewritten, long_id);
    assert_eq!(uses[2]["id"], "toolu_ok", "already-valid ids pass through unchanged");
    let results = messages[2]["content"].as_array().unwrap();
    assert_eq!(results[0]["tool_use_id"], rewritten, "call and result stay paired");
    // deterministic: the same original maps identically on the next request
    run(&rt, &provider, &make_request()).unwrap();
    let again = request_body(&server);
    assert_eq!(again["messages"][1]["content"][1]["id"], rewritten);
    rt.block_on(server.task).unwrap();
}

#[test]
fn anthropic_max_tokens_clamps_to_the_remaining_context_window() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![
        sse_response(ANTHROPIC_TEXT_DONE),
        sse_response(ANTHROPIC_TEXT_DONE),
        sse_response(ANTHROPIC_TEXT_DONE),
    ]));
    let provider = Anthropic::new(&server.base, "k", Duration::from_secs(5)).unwrap().with_context_window(Some(10_000));
    let mut req = request();
    req.options = json!({"max_tokens": 8192});
    req.est_prompt_tokens = 3904;
    run(&rt, &provider, &req).unwrap();
    assert_eq!(request_body(&server)["max_tokens"], 2000, "8192 clamps to 10000-3904-4096");
    // floor: a nearly-full window still asks for one token
    req.est_prompt_tokens = 9999;
    run(&rt, &provider, &req).unwrap();
    assert_eq!(request_body(&server)["max_tokens"], 1);
    // no declared window → no clamp
    let provider = Anthropic::new(&server.base, "k", Duration::from_secs(5)).unwrap();
    req.est_prompt_tokens = 3904;
    run(&rt, &provider, &req).unwrap();
    assert_eq!(request_body(&server)["max_tokens"], 8192);
    rt.block_on(server.task).unwrap();
}

#[test]
fn responses_max_output_tokens_clamps_to_the_remaining_context_window() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![sse_response(RESPONSES_COMPLETED)]));
    let provider = Responses::new(&server.base, "k", Duration::from_secs(5)).unwrap().with_context_window(Some(10_000));
    let mut req = request();
    req.options = json!({"max_tokens": 8192});
    req.est_prompt_tokens = 3904;
    run(&rt, &provider, &req).unwrap();
    let body = request_body(&server);
    assert_eq!(body["max_output_tokens"], 2000);
    assert!(body.get("max_tokens").is_none());
    rt.block_on(server.task).unwrap();
}

#[test]
fn chat_completions_max_tokens_clamps_to_the_remaining_context_window() {
    let rt = runtime();
    let server = rt.block_on(FakeServer::start(vec![sse_response(CHAT_TEXT_DONE)]));
    let provider =
        ChatCompletions::new(&server.base, "k", Duration::from_secs(5)).unwrap().with_context_window(Some(10_000));
    let mut req = request();
    req.options = json!({"max_tokens": 8192});
    req.est_prompt_tokens = 3904;
    run(&rt, &provider, &req).unwrap();
    assert_eq!(request_body(&server)["max_tokens"], 2000);
    rt.block_on(server.task).unwrap();
}
