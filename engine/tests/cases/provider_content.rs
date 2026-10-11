//! D-451: inspect real HTTP request bodies; these tests make no external model call.
use super::*;

#[test]
fn native_images_survive_every_protocol_and_unsupported_parts_fail() {
    let rt = runtime();
    for protocol in ["chat", "responses", "anthropic"] {
        // A server rejection still gives us the complete request that was sent.
        let server =
            rt.block_on(FakeServer::start(vec![json_response("400 Bad Request", &json!({"error":"fixture"}))]));
        let mut req = request();
        let image = json!({"type":"image_url","image_url":{"url":"data:image/png;base64,aGVsbG8="}});
        let content = json!([{"type":"text","text":"caption"}, image]);
        req.messages = vec![
            json!({"role":"user","content":content}),
            json!({"role":"assistant","content":"","tool_calls":[{"id":"view","type":"function","function":{"name":"view_image","arguments":"{}"}}]}),
            json!({"role":"tool","tool_call_id":"view","content":content}),
        ];
        let result = match protocol {
            "chat" => run(&rt, &ChatCompletions::new(&server.base, "fixture", Duration::from_secs(5)).unwrap(), &req),
            "responses" => run(&rt, &Responses::new(&server.base, "fixture", Duration::from_secs(5)).unwrap(), &req),
            _ => run(&rt, &Anthropic::new(&server.base, "fixture", Duration::from_secs(5)).unwrap(), &req),
        };
        assert!(result.is_err());
        let body = request_body(&server);
        match protocol {
            "chat" => {
                assert_eq!(body["messages"][0]["content"], content);
                assert_eq!(body["messages"][2]["content"], content);
            }
            "responses" => {
                assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
                assert_eq!(body["input"][0]["content"][1]["type"], "input_image");
                assert_eq!(body["input"][2]["output"][1]["image_url"], image["image_url"]["url"]);
            }
            _ => {
                assert_eq!(body["messages"][0]["content"][1]["type"], "image");
                assert_eq!(body["messages"][2]["content"][0]["content"][1]["source"]["data"], "aGVsbG8=");
            }
        }
        if protocol != "chat" {
            for bad in [
                json!([{"type":"unknown","text":"do not drop me"}]),
                json!([{"type":"image_url","image_url":{"url":"https://example.test/a.png"}}]),
            ] {
                req.messages = vec![json!({"role":"user","content":bad})];
                let result = if protocol == "responses" {
                    run(&rt, &Responses::new(&server.base, "fixture", Duration::from_secs(5)).unwrap(), &req)
                } else {
                    run(&rt, &Anthropic::new(&server.base, "fixture", Duration::from_secs(5)).unwrap(), &req)
                };
                assert_eq!(result.unwrap_err().class, ErrorClass::Permanent);
            }
        }
        server.task.abort();
    }
}
