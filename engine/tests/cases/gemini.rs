//! Native Gemini transport, strict completion and continuation provenance.
use super::*;
use teamagents_engine::providers::{build_for_profile, AnyProvider};

fn gemini(base: &str) -> AnyProvider {
    let profile = serde_json::from_value(
        json!({"provider":"google","model":"fixture","protocol":"gemini","base_url":base,"context_window":6000}),
    )
    .unwrap();
    build_for_profile(&profile).unwrap()
}

#[test]
fn gemini_native_tool_roundtrip_preserves_signatures_images_and_known_usage() {
    let rt = runtime();
    let parts = json!([{"thought":true,"text":"private"},{"functionCall":{"name":"read","args":{"path":"doc"},"id":"wire-id"},"thoughtSignature":"signed"}]);
    let frame = json!({"candidates":[{"content":{"parts":parts},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":22,"candidatesTokenCount":3,"thoughtsTokenCount":4,"totalTokenCount":29,"cachedContentTokenCount":5}});
    let done = json!({"candidates":[{"content":{"parts":[{"text":"done"}]},"finishReason":"STOP"}]});
    let server = rt
        .block_on(FakeServer::start(vec![sse_response(&format!("data: {frame}\n\n")), json_response("200 OK", &done)]));
    let p = gemini(&server.base);
    let mut r = request();
    r.messages.insert(0, json!({"role":"system","content":"policy"}));
    r.tools = vec![
        json!({"type":"function","function":{"name":"read","parameters":{"type":"object","additionalProperties":false}}}),
    ];
    r.options = json!({"max_tokens":10000});
    let a = run(&rt, &p, &r).unwrap();
    assert_eq!(a.response.message["content"], "");
    let usage = a.response.usage.unwrap();
    assert_eq!((usage.total, usage.completion, usage.cache_hit), (29, 7, 5));
    let call = a.response.message["tool_calls"][0]["id"].clone();
    r.messages.push(a.response.message);
    r.messages.push(json!({"role":"tool","tool_call_id":call,"content":[{"type":"text","text":"a"},{"type":"image_url","image_url":{"url":"data:image/png;base64,YQ=="}}]}));
    let b = run(&rt, &p, &r).unwrap();
    assert_eq!(b.response.message["content"], "done");
    assert!(b.response.usage.is_none());
    assert!(server.last_request().starts_with("POST /models/deepseek-flash:streamGenerateContent?alt=sse "));
    let body = request_body(&server);
    assert_eq!(body["systemInstruction"]["parts"][0]["text"], "policy");
    assert_eq!(body["contents"][1]["parts"], parts);
    assert_eq!(
        body["contents"][2]["parts"][0]["functionResponse"],
        json!({"id":"wire-id","name":"read","response":{"output":"a"},"parts":[{"inlineData":{"mimeType":"image/png","data":"YQ=="}}]})
    );
    assert_eq!(body["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"]["additionalProperties"], false);
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 1899);
    rt.block_on(server.task).unwrap();
}

#[test]
fn gemini_rejects_incomplete_calls_and_does_not_retry_after_visible_text() {
    let rt = runtime();
    let events = [
        (
            json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"shell","args":{}}}]}}]}),
            ErrorClass::Transient,
        ),
        (json!({"candidates":[{"content":{"parts":[{"text":"shown"}]}}]}), ErrorClass::Permanent),
        (
            json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"shell","args":{}}}]},"finishReason":"MAX_TOKENS"}]}),
            ErrorClass::Permanent,
        ),
        (json!({"promptFeedback":{"blockReason":"SAFETY"}}), ErrorClass::Permanent),
    ];
    for (frame, class) in events {
        let server = rt.block_on(FakeServer::start(vec![sse_response(&format!("data: {frame}\n\n"))]));
        assert_eq!(run(&rt, &gemini(&server.base), &request()).unwrap_err().class, class);
        rt.block_on(server.task).unwrap();
    }
}
