//! Bounded SSE decoding. Partial tool arguments never reach the executor.
use crate::gateway::TurnControl;
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};

const MAX_RESPONSE: usize = 16 * 1024 * 1024;
const MAX_FRAME: u64 = 1024 * 1024;

/// The three wire formats a model endpoint can speak. The body this module
/// returns keeps each format's own shape; the caller normalizes it.
#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    /// OpenAI-compatible chat completions (`choices[].message`).
    Chat,
    /// Anthropic Messages (`content` blocks).
    Anthropic,
    /// OpenAI Responses (`output` items, dotted SSE event names).
    Responses,
}

pub(crate) fn response(
    response: ureq::Response, mode: Mode, control: &TurnControl,
    mut emit: impl FnMut(&str),
) -> Result<Json, String> {
    if response.header("content-type").unwrap_or("").contains("text/event-stream") {
        decode(BufReader::new(response.into_reader()), mode, control, emit)
    } else {
        let mut bytes = vec![];
        response.into_reader().take(MAX_RESPONSE as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        control.check()?;
        if bytes.len() > MAX_RESPONSE { return Err("model response exceeds 16 MiB".into()); }
        let data: Json = serde_json::from_slice(&bytes).map_err(|e| format!("chat API: bad json: {e}"))?;
        match mode {
            Mode::Anthropic => {
                for block in data["content"].as_array().into_iter().flatten() {
                    if let Some(text) = block["text"].as_str() { emit(text); }
                }
            }
            Mode::Responses => {
                for item in data["output"].as_array().into_iter().flatten() {
                    for part in item["content"].as_array().into_iter().flatten() {
                        if let Some(text) = part["text"].as_str() { emit(text); }
                    }
                }
            }
            Mode::Chat => {
                if let Some(text) = data["choices"][0]["message"]["content"].as_str() { emit(text); }
            }
        }
        Ok(data)
    }
}

fn append(target: &mut Json, field: &str, part: &Json) {
    if let Some(part) = part.as_str() {
        let mut value = target[field].as_str().unwrap_or("").to_string();
        value.push_str(part);
        target[field] = json!(value);
    }
}

fn decode(
    mut reader: impl BufRead, mode: Mode, control: &TurnControl,
    mut emit: impl FnMut(&str),
) -> Result<Json, String> {
    let mut message = json!({"role":"assistant", "content":""});
    let mut blocks: BTreeMap<usize, Json> = BTreeMap::new();
    let mut arguments: BTreeMap<usize, String> = BTreeMap::new();
    let mut usage = json!({});
    // Responses streams items, not deltas per block
    let mut items: Vec<Json> = vec![];
    let mut frame = String::new();
    let mut total = 0usize;
    let mut complete = false;
    loop {
        control.check()?;
        let mut line = String::new();
        let count = reader.by_ref().take(MAX_FRAME + 1).read_line(&mut line).map_err(|e| format!("model stream: {e}"))?;
        if count == 0 { break; }
        total = total.saturating_add(count);
        if count as u64 > MAX_FRAME || frame.len() + count > MAX_FRAME as usize || total > MAX_RESPONSE {
            return Err("model stream exceeds frame/response limit".into());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(data) = line.strip_prefix("data:") {
            if !frame.is_empty() { frame.push('\n'); }
            frame.push_str(data.strip_prefix(' ').unwrap_or(data));
        }
        if !line.is_empty() || frame.is_empty() { continue; }
        if frame.trim() == "[DONE]" { complete = true; break; }
        let data: Json = serde_json::from_str(&frame).map_err(|e| format!("invalid model stream frame: {e}"))?;
        frame.clear();
        if data.get("error").is_some() || data["type"] == "error" {
            return Err(format!("model stream error: {}", data["error"]));
        }
        if mode == Mode::Responses {
            match data["type"].as_str().unwrap_or("") {
                "response.output_text.delta" => {
                    append(&mut message, "content", &data["delta"]);
                    if let Some(text) = data["delta"].as_str() { emit(text); }
                }
                // reasoning stays private: it is not part of the reply
                "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {}
                "response.output_item.done" => {
                    if !data["item"].is_null() { items.push(data["item"].clone()); }
                }
                "response.completed" | "response.incomplete" => {
                    if let Some(output) = data["response"]["output"].as_array() {
                        if !output.is_empty() { items = output.clone(); }
                    }
                    if data["response"]["usage"].is_object() { usage = data["response"]["usage"].clone(); }
                    complete = true;
                }
                "response.failed" => {
                    return Err(format!("model stream error: {}", data["response"]["error"]));
                }
                "error" => return Err(format!("model stream error: {}", data["error"])),
                _ => {}
            }
        } else if mode == Mode::Anthropic {
            let index = data["index"].as_u64().unwrap_or(0) as usize;
            match data["type"].as_str().unwrap_or("") {
                "message_start" => usage = data["message"]["usage"].clone(),
                "content_block_start" => {
                    blocks.insert(index, data["content_block"].clone());
                }
                "content_block_delta" => {
                    let block = blocks.get_mut(&index).ok_or("delta without content block")?;
                    let delta = &data["delta"];
                    match delta["type"].as_str().unwrap_or("") {
                        "text_delta" => {
                            append(block, "text", &delta["text"]);
                            if let Some(text) = delta["text"].as_str() { emit(text); }
                        }
                        "input_json_delta" => arguments.entry(index).or_default().push_str(delta["partial_json"].as_str().unwrap_or("")),
                        "thinking_delta" => append(block, "thinking", &delta["thinking"]),
                        "signature_delta" => append(block, "signature", &delta["signature"]),
                        _ => {}
                    }
                }
                "message_delta" => {
                    if !usage.is_object() { usage = json!({}); }
                    if let Some(fields) = data["usage"].as_object() {
                        usage.as_object_mut().unwrap().extend(fields.clone());
                    }
                }
                "message_stop" => { complete = true; break; }
                _ => {}
            }
        } else {
            if data["usage"].is_object() { usage = data["usage"].clone(); }
            let choice = data["choices"].as_array().and_then(|choices| choices.iter().find(|c| c["index"].as_u64().unwrap_or(0) == 0));
            let Some(choice) = choice else { continue };
            let delta = &choice["delta"];
            append(&mut message, "content", &delta["content"]);
            append(&mut message, "reasoning_content", &delta["reasoning_content"]);
            if let Some(text) = delta["content"].as_str() { emit(text); }
            for tool in delta["tool_calls"].as_array().into_iter().flatten() {
                let index = tool["index"].as_u64().ok_or("stream tool index missing")? as usize;
                let block = blocks.entry(index).or_insert_with(|| json!({"id":"", "type":"function", "function":{"name":"", "arguments":""}}));
                append(block, "id", &tool["id"]);
                append(&mut block["function"], "name", &tool["function"]["name"]);
                append(&mut block["function"], "arguments", &tool["function"]["arguments"]);
            }
        }
    }
    control.check()?;
    if !complete { return Err("model stream ended before completion; partial tool calls were not executed".into()); }
    if mode == Mode::Responses {
        if items.is_empty() && message["content"].as_str().is_some_and(|text| !text.is_empty()) {
            items.push(json!({"type":"message", "role":"assistant",
                              "content":[{"type":"output_text","text":message["content"]}]}));
        }
        return Ok(json!({"output": items, "usage": usage}));
    }
    if mode == Mode::Anthropic {
        for (index, args) in arguments {
            blocks.get_mut(&index).ok_or("tool block missing")?["input"] = serde_json::from_str(&args).map_err(|e| format!("invalid streamed tool arguments: {e}"))?;
        }
        Ok(json!({"content":blocks.into_values().collect::<Vec<_>>(), "usage":usage}))
    } else {
        if !blocks.is_empty() { message["tool_calls"] = json!(blocks.into_values().collect::<Vec<_>>()); }
        Ok(json!({"choices":[{"message":message}], "usage":usage}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_deltas_join_tools_and_reject_truncation() {
        let frames = [
            json!({"choices":[{"delta":{"content":"你好", "tool_calls":[{"index":0,"id":"a","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a\"}"}}]}}]}),
            json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}),
        ];
        let mut wire = frames.iter().map(|v| format!("data: {v}\r\n\r\n")).collect::<String>();
        assert!(decode(wire.as_bytes(), Mode::Chat, &TurnControl::default(), |_|{}).is_err());
        wire.push_str("data: [DONE]\n\n");
        let mut text = String::new();
        let result = decode(wire.as_bytes(), Mode::Chat, &TurnControl::default(), |s| text.push_str(s)).unwrap();
        assert_eq!(text, "你好");
        assert_eq!(result["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"], "{\"path\":\"a\"}");
        assert_eq!(result["usage"]["prompt_tokens"], 10);
    }

    #[test]
    fn responses_stream_yields_text_calls_and_usage() {
        let frames = [
            json!({"type":"response.created","response":{"id":"r1"}}),
            json!({"type":"response.output_text.delta","delta":"plan "}),
            json!({"type":"response.reasoning_summary_text.delta","delta":"private thinking"}),
            json!({"type":"response.output_text.delta","delta":"now"}),
            json!({"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"shell","arguments":"{\"command\":\"ls\"}"}}),
            json!({"type":"response.completed","response":{"usage":{"input_tokens":11,"output_tokens":7,"total_tokens":18},
                    "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"plan now"}]},
                              {"type":"function_call","call_id":"call_1","name":"shell","arguments":"{\"command\":\"ls\"}"}]}}),
        ];
        let wire = frames.iter().map(|v| format!("event: ignored\ndata: {v}\n\n")).collect::<String>();
        let mut text = String::new();
        let result = decode(wire.as_bytes(), Mode::Responses, &TurnControl::default(), |s| text.push_str(s)).unwrap();
        assert_eq!(text, "plan now", "only visible text is emitted");
        assert_eq!(result["usage"]["input_tokens"], 11);
        let output = result["output"].as_array().unwrap();
        assert_eq!(output.len(), 2, "{result}");
        assert_eq!(output[1]["type"], "function_call");
        assert_eq!(output[1]["call_id"], "call_1");

        // A stream that never completes must not be executed on
        let truncated = frames[..2].iter().map(|v| format!("data: {v}\n\n")).collect::<String>();
        assert!(decode(truncated.as_bytes(), Mode::Responses, &TurnControl::default(), |_|{}).is_err());

        // A failed response surfaces the provider's error, it does not look like an empty reply
        let failed = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"overloaded\"}}}\n\n";
        let error = decode(failed.as_bytes(), Mode::Responses, &TurnControl::default(), |_|{}).unwrap_err();
        assert!(error.contains("overloaded"), "{error}");
    }

    #[test]
    fn anthropic_thinking_signature_and_usage_survive() {
        let frames = [
            json!({"type":"message_start","message":{"usage":{"input_tokens":12,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"think"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed"}}),
            json!({"type":"message_delta","usage":{"output_tokens":8}}),
            json!({"type":"message_stop"}),
        ];
        let wire = frames.iter().map(|v| format!("event: ignored\ndata: {v}\n\n")).collect::<String>();
        let result = decode(wire.as_bytes(), Mode::Anthropic, &TurnControl::default(), |_| panic!("thinking must stay private")).unwrap();
        assert_eq!(result["content"][0]["signature"], "signed");
        assert_eq!(result["usage"], json!({"input_tokens":12,"output_tokens":8}));
        let control = TurnControl::default();
        control.cancel();
        assert!(decode(wire.as_bytes(), Mode::Anthropic, &control, |_|{}).is_err());
    }
}
