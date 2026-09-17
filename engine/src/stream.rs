//! Bounded SSE decoding. Partial tool arguments never reach the executor.
use crate::gateway::TurnControl;
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::cell::Cell;
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

/// Why reading a model response failed. The classification drives retry: a
/// transport failure before any visible output is safe to retry — no tool
/// from the response has executed and no text was shown. Protocol and
/// cancellation failures are final for this request.
#[derive(Debug)]
pub(crate) enum StreamError {
    /// Turn cancellation; never retried, message preserved verbatim.
    Interrupted(String),
    /// I/O failure or a truncated stream; carries whether any text was emitted.
    Transport(String, bool),
    /// The endpoint answered but violated the protocol or reported an error.
    Protocol(String),
}

impl StreamError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Interrupted(message) | Self::Transport(message, _) | Self::Protocol(message) => message,
        }
    }
}

pub(crate) fn response(
    response: ureq::Response, mode: Mode, control: &TurnControl,
    mut emit: impl FnMut(&str),
) -> Result<Json, StreamError> {
    if response.header("content-type").unwrap_or("").contains("text/event-stream") {
        decode(BufReader::new(response.into_reader()), mode, control, emit)
    } else {
        let mut bytes = vec![];
        response.into_reader().take(MAX_RESPONSE as u64 + 1).read_to_end(&mut bytes)
            .map_err(|e| StreamError::Transport(format!("model stream: {e}"), false))?;
        control.check().map_err(StreamError::Interrupted)?;
        if bytes.len() > MAX_RESPONSE { return Err(StreamError::Protocol("model response exceeds 16 MiB".into())); }
        let data: Json = serde_json::from_slice(&bytes).map_err(|e| StreamError::Protocol(format!("chat API: bad json: {e}")))?;
        ensure_complete(&data, mode).map_err(StreamError::Protocol)?;
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

// A transport terminator does not mean generation completed successfully.
// Reject explicit truncation before any tool arguments reach the executor.
fn ensure_complete(data: &Json, mode: Mode) -> Result<(), String> {
    if data.get("error").is_some_and(|error| !error.is_null()) {
        return Err(format!("模型服务返回错误：{}", data["error"]));
    }
    let reason = match mode {
        Mode::Chat => data["choices"][0]["finish_reason"].as_str()
            .filter(|reason| matches!(*reason, "length" | "content_filter")),
        Mode::Anthropic => data["stop_reason"].as_str()
            .filter(|reason| matches!(*reason, "max_tokens" | "model_context_window_exceeded" | "pause_turn")),
        Mode::Responses => data["status"].as_str().filter(|status| *status != "completed"),
    };
    if let Some(reason) = reason {
        return Err(format!("模型响应未完成（{reason}，{}）；未执行本次响应中的工具调用", data.get("incomplete_details").unwrap_or(&Json::Null)));
    }
    if mode == Mode::Responses && data["output"].as_array().into_iter().flatten()
        .any(|item| item["status"].as_str().is_some_and(|status| status != "completed")) {
        return Err("模型响应包含未完成的输出；未执行本次响应中的工具调用".into());
    }
    Ok(())
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
) -> Result<Json, StreamError> {
    // Emission doubles as the retry-safety marker: a failed stream that never
    // showed text holds at most partial tool arguments, which never executed.
    let emitted = Cell::new(false);
    let mut emit = |text: &str| {
        emitted.set(true);
        emit(text);
    };
    let mut message = json!({"role":"assistant", "content":""});
    let mut blocks: BTreeMap<usize, Json> = BTreeMap::new();
    let mut arguments: BTreeMap<usize, String> = BTreeMap::new();
    let mut usage = json!({});
    // Responses streams items, not deltas per block
    let mut items: Vec<Json> = vec![];
    let mut frame = String::new();
    let mut total = 0usize;
    let mut complete = false;
    let mut finish_reason = Json::Null;
    loop {
        control.check().map_err(StreamError::Interrupted)?;
        let mut line = String::new();
        let count = reader.by_ref().take(MAX_FRAME + 1).read_line(&mut line)
            .map_err(|e| StreamError::Transport(format!("model stream: {e}"), emitted.get()))?;
        if count == 0 { break; }
        total = total.saturating_add(count);
        if count as u64 > MAX_FRAME || frame.len() + count > MAX_FRAME as usize || total > MAX_RESPONSE {
            return Err(StreamError::Protocol("model stream exceeds frame/response limit".into()));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(data) = line.strip_prefix("data:") {
            if !frame.is_empty() { frame.push('\n'); }
            frame.push_str(data.strip_prefix(' ').unwrap_or(data));
        }
        if !line.is_empty() || frame.is_empty() { continue; }
        if frame.trim() == "[DONE]" {
            if mode != Mode::Chat { return Err(StreamError::Protocol("模型流缺少协议终结事件".into())); }
            complete = true;
            break;
        }
        let data: Json = serde_json::from_str(&frame).map_err(|e| StreamError::Protocol(format!("invalid model stream frame: {e}")))?;
        frame.clear();
        if data.get("error").is_some() || data["type"] == "error" {
            return Err(StreamError::Protocol(format!("model stream error: {}", data["error"])));
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
                "response.incomplete" => {
                    return Err(StreamError::Protocol(format!("模型响应未完成（{}）；未执行本次响应中的工具调用", data["response"]["incomplete_details"])));
                }
                "response.completed" => {
                    ensure_complete(&data["response"], mode).map_err(StreamError::Protocol)?;
                    if let Some(output) = data["response"]["output"].as_array() {
                        if !output.is_empty() { items = output.clone(); }
                    }
                    if data["response"]["usage"].is_object() { usage = data["response"]["usage"].clone(); }
                    complete = true;
                    break;
                }
                "response.failed" => {
                    return Err(StreamError::Protocol(format!("model stream error: {}", data["response"]["error"])));
                }
                "error" => return Err(StreamError::Protocol(format!("model stream error: {}", data["error"]))),
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
                    let block = blocks.get_mut(&index).ok_or_else(|| StreamError::Protocol("delta without content block".into()))?;
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
                    if !data["delta"]["stop_reason"].is_null() { finish_reason = data["delta"]["stop_reason"].clone(); }
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
            if !choice["finish_reason"].is_null() { finish_reason = choice["finish_reason"].clone(); }
            let delta = &choice["delta"];
            append(&mut message, "content", &delta["content"]);
            append(&mut message, "reasoning_content", &delta["reasoning_content"]);
            if let Some(text) = delta["content"].as_str() { emit(text); }
            for tool in delta["tool_calls"].as_array().into_iter().flatten() {
                let index = tool["index"].as_u64().ok_or_else(|| StreamError::Protocol("stream tool index missing".into()))? as usize;
                let block = blocks.entry(index).or_insert_with(|| json!({"id":"", "type":"function", "function":{"name":"", "arguments":""}}));
                append(block, "id", &tool["id"]);
                append(&mut block["function"], "name", &tool["function"]["name"]);
                append(&mut block["function"], "arguments", &tool["function"]["arguments"]);
            }
        }
    }
    control.check().map_err(StreamError::Interrupted)?;
    // Clean EOF without a terminator means the connection gave up mid-turn;
    // with nothing emitted the request is retried by the caller.
    if !complete { return Err(StreamError::Transport("model stream ended before completion; partial tool calls were not executed".into(), emitted.get())); }
    if mode == Mode::Responses {
        if items.is_empty() && message["content"].as_str().is_some_and(|text| !text.is_empty()) {
            items.push(json!({"type":"message", "role":"assistant",
                              "content":[{"type":"output_text","text":message["content"]}]}));
        }
        let data = json!({"output": items, "usage": usage});
        ensure_complete(&data, mode).map_err(StreamError::Protocol)?;
        return Ok(data);
    }
    if mode == Mode::Anthropic {
        for (index, args) in arguments {
            blocks.get_mut(&index).ok_or_else(|| StreamError::Protocol("tool block missing".into()))?["input"] = serde_json::from_str(&args)
                .map_err(|e| StreamError::Protocol(format!("invalid streamed tool arguments: {e}")))?;
        }
        let data = json!({"content":blocks.into_values().collect::<Vec<_>>(), "usage":usage, "stop_reason":finish_reason});
        ensure_complete(&data, mode).map_err(StreamError::Protocol)?;
        Ok(data)
    } else {
        if !blocks.is_empty() { message["tool_calls"] = json!(blocks.into_values().collect::<Vec<_>>()); }
        let data = json!({"choices":[{"message":message,"finish_reason":finish_reason}], "usage":usage});
        ensure_complete(&data, mode).map_err(StreamError::Protocol)?;
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_fallback_items_must_also_be_complete() {
        let frames = [
            json!({"type":"response.output_item.done","item":{"type":"function_call","status":"incomplete",
                "call_id":"partial","name":"shell","arguments":"{}"}}),
            json!({"type":"response.completed","response":{"status":"completed","output":[]}}),
        ];
        let wire: String = frames.iter().map(|frame| format!("data: {frame}\n\n")).collect();
        assert!(decode(wire.as_bytes(), Mode::Responses, &TurnControl::default(), |_| {}).is_err());
        for mode in [Mode::Responses, Mode::Anthropic] {
            assert!(decode("data: [DONE]\n\n".as_bytes(), mode, &TurnControl::default(), |_| {}).is_err());
        }
    }

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
        assert!(error.message().contains("overloaded"), "{}", error.message());
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

    #[test]
    fn truncated_streams_classify_as_transport_and_track_emission() {
        // Tool-only stream truncated before the terminator: nothing visible,
        // safe to retry.
        let tool_only = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"shell\",\"arguments\":\"{\\\"comma\"}}]}}]}\n\n";
        let error = decode(tool_only.as_bytes(), Mode::Chat, &TurnControl::default(), |_| {}).unwrap_err();
        match error {
            StreamError::Transport(_, false) => {}
            other => panic!("tool-only truncation must be retryable transport: {other:?}"),
        }

        // Once text reached the user the same truncation is final.
        let with_text = "data: {\"choices\":[{\"delta\":{\"content\":\"half an answer\"}}]}\n\n";
        let error = decode(with_text.as_bytes(), Mode::Chat, &TurnControl::default(), |_| {}).unwrap_err();
        match error {
            StreamError::Transport(_, true) => {}
            other => panic!("emitted text must mark the failure as final: {other:?}"),
        }

        // Cancellation stays distinguishable from a transport failure.
        let control = TurnControl::default();
        control.cancel();
        let error = decode(tool_only.as_bytes(), Mode::Chat, &control, |_| {}).unwrap_err();
        assert!(matches!(error, StreamError::Interrupted(_)), "{error:?}");
    }
}
