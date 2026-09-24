//! Stream stall detection (§8): a connection that only trickles keep-alive
//! comments is dead for our purposes and must fail as a retryable transport
//! error instead of holding the turn open indefinitely. The bound counts
//! *events*, not bytes.

use serde_json::json;
use std::time::{Duration, Instant};
use teamagents_core::kernel::ModelRequest;
use teamagents_engine::providers::chat_completions::ChatCompletions;
use teamagents_engine::providers::{Cancel, ErrorClass, Provider};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn request() -> ModelRequest {
    ModelRequest {
        request_id: "stall".into(),
        model: "m".into(),
        messages: vec![json!({"role": "user", "content": "hi"})],
        tools: vec![],
        options: json!({}),
        est_prompt_tokens: 4,
    }
}

/// Keep-alive comments only: the stream never produces an event.
#[tokio::test]
async fn a_keep_alive_only_stream_stalls() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else { return };
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;
        if socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
            .await
            .is_err()
        {
            return;
        }
        for _ in 0..200 {
            if socket.write_all(b": keep-alive\n\n").await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    let provider = ChatCompletions::new(&base, "k", Duration::from_secs(30))
        .unwrap()
        .with_stream_stall(Duration::from_millis(250));
    let started = Instant::now();
    let error = provider.complete(&request(), &Cancel::new(), &mut |_| {}).await.expect_err("stalled stream");
    assert_eq!(error.class, ErrorClass::Transient, "a stall before any output is retryable");
    assert!(error.message.contains("stalled"), "{}", error.message);
    assert!(started.elapsed() < Duration::from_secs(5), "stall detected after {:?}", started.elapsed());
    server.abort();
}

/// Events keep the stream alive: a slow but progressing stream is not stalled.
#[tokio::test]
async fn events_reset_the_stall_clock() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else { return };
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;
        let _ =
            socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n").await;
        // five slow deltas, each well inside the bound, then the finish frame
        for index in 0..5 {
            let frame = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{index}\"}}}}]}}\n\n");
            if socket.write_all(frame.as_bytes()).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        let _ = socket
            .write_all(
                b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1,\"total_tokens\":4}}\n\ndata: [DONE]\n\n",
            )
            .await;
    });
    let provider = ChatCompletions::new(&base, "k", Duration::from_secs(30))
        .unwrap()
        .with_stream_stall(Duration::from_millis(250));
    let outcome = provider.complete(&request(), &Cancel::new(), &mut |_| {}).await.expect("progressing stream");
    assert_eq!(outcome.response.message["content"], json!("01234"));
    server.abort();
}
