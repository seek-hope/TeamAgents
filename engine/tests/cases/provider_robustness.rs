use super::*;
use teamagents_engine::providers::AnyProvider;

fn provider(index: usize, base: &str) -> AnyProvider {
    let timeout = Duration::from_secs(2);
    match index {
        0 => AnyProvider::ChatCompletions(ChatCompletions::new(base, "", timeout).unwrap()),
        1 => AnyProvider::Responses(Responses::new(base, "", timeout).unwrap()),
        _ => AnyProvider::Anthropic(Anthropic::new(base, "", timeout).unwrap()),
    }
}

#[tokio::test]
async fn sse_crlf_and_multiline_data_preserve_native_text() {
    let cases = [
        "data: {\r\ndata: \"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":\"stop\"}]}\r\n\r\ndata: [DONE]\r\n\r\n",
        "data: {\r\ndata: \"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}]}}\r\n\r\n",
        "data: {\r\ndata: \"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"hello\"}}\r\n\r\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\r\n\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n",
    ];
    for (index, events) in cases.into_iter().enumerate() {
        let server = FakeServer::start(vec![sse_response(events)]).await;
        let provider = provider(index, &server.base);
        let result = provider.complete(&request(), &Cancel::new(), &mut |_| {}).await.unwrap();
        assert_eq!(result.response.message["content"], "hello", "adapter {index}");
    }
}

#[tokio::test]
async fn malformed_anthropic_blocks_fail_without_panicking() {
    for block in [json!([]), json!("bad"), json!(42), json!(true)] {
        let events = format!(
            "data: {}\n\ndata: {}\n\n",
            json!({"type":"content_block_start","index":0,"content_block":block}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}),
        );
        let server = FakeServer::start(vec![sse_response(&events)]).await;
        let provider = Anthropic::new(&server.base, "", Duration::from_secs(2)).unwrap();
        let result = tokio::spawn(async move { provider.complete(&request(), &Cancel::new(), &mut |_| {}).await })
            .await
            .expect("malformed remote input must not panic");
        assert_eq!(result.unwrap_err().class, ErrorClass::Permanent);
    }
}

#[tokio::test]
async fn oversized_model_bodies_are_refused_from_headers() {
    for status in ["200 OK", "500 Internal Server Error"] {
        for index in 0..3 {
            let server = FakeServer::start(vec![format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: 67108865\r\nconnection: close\r\n\r\n"
            )]).await;
            let provider = provider(index, &server.base);
            let error = provider.complete(&request(), &Cancel::new(), &mut |_| {}).await.unwrap_err();
            assert_eq!(error.class, ErrorClass::Permanent, "{index} {status}: {error:?}");
            assert!(error.message.contains("too large"), "{error:?}");
        }
    }
}
