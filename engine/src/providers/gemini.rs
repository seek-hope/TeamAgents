//! Google AI Studio's native Gemini API. One request/attempt, no SDK retries.
use super::{
    clamp_max_tokens, pump_sse, read_body, stream_failure_msg, stream_stall_bound, AttemptOutcome, Cancel, Provider,
    ProviderError, ProviderEvent, SseEnd, StreamTiming,
};
use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};
use teamagents_core::kernel::{ModelRequest, ModelResponse, Usage};

pub struct Gemini {
    client: reqwest::Client,
    base: url::Url,
    key: String,
    timeout: Duration,
    window: Option<u64>,
}

fn invalid(reason: &str) -> ProviderError {
    ProviderError::permanent(format!("Gemini: {reason}"))
}

impl Gemini {
    pub(super) fn new(base: &str, key: String, timeout: Duration, window: Option<u64>) -> Result<Self, String> {
        let base = url::Url::parse(base).map_err(|_| "invalid Gemini API base URL")?;
        let loopback = base.host_str() == Some("localhost")
            || matches!(base.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
            || matches!(base.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback());
        if !(base.scheme() == "https" || (loopback && base.scheme() == "http"))
            || base.host().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(
                "Gemini requires an HTTPS API root (HTTP only on loopback), without credentials, query or fragment"
                    .into(),
            );
        }
        // Custom key headers must never follow a redirect to another endpoint.
        let client = super::http_client()?;
        Ok(Self { client, base, key, timeout, window })
    }

    fn body(&self, request: &ModelRequest) -> Result<Json, ProviderError> {
        let mut contents: Vec<Json> = Vec::new();
        let mut system = Vec::new();
        let mut ids = HashMap::new();
        for msg in &request.messages {
            match msg["role"].as_str() {
                Some("system" | "developer") => system.push(
                    json!({"text": msg["content"].as_str().ok_or_else(||invalid("system instruction is not text"))?}),
                ),
                Some("user") => contents.push(json!({"role":"user","parts":parts(&msg["content"])?})),
                Some("assistant") => {
                    let native = &msg["gemini_content"];
                    let same = native["model"].as_str() == Some(&request.model)
                        && native["base"].as_str() == Some(self.base.as_str());
                    let mut translated = Vec::new();
                    if let Some(text) = msg["content"].as_str().filter(|s| !s.is_empty()) {
                        translated.push(json!({"text":text}));
                    }
                    let native_calls: Vec<_> = native["parts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| p.get("functionCall"))
                        .collect();
                    for (idx, call) in msg["tool_calls"].as_array().into_iter().flatten().enumerate() {
                        let id = call["id"].as_str().ok_or_else(|| invalid("missing tool call id"))?;
                        let name = call["function"]["name"].as_str().ok_or_else(|| invalid("missing tool name"))?;
                        let wire_id = same.then(|| native_calls.get(idx).and_then(|f| f["id"].as_str())).flatten();
                        ids.insert(id, (name, wire_id));
                        let args: Json = serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or(""))
                            .map_err(|_| invalid("invalid stored tool arguments"))?;
                        translated.push(json!({"functionCall":{"name":name,"args":args}}));
                    }
                    let selected = if same {
                        native["parts"].as_array().ok_or_else(|| invalid("invalid native history"))?
                    } else {
                        &translated
                    };
                    if !selected.is_empty() {
                        contents.push(json!({"role":"model","parts":selected}));
                    }
                }
                Some("tool") => {
                    let (name, wire_id) = ids
                        .remove(msg["tool_call_id"].as_str().unwrap_or(""))
                        .ok_or_else(|| invalid("unpaired tool response"))?;
                    let (images, texts): (Vec<_>, Vec<_>) =
                        parts(&msg["content"])?.into_iter().partition(|p| p.get("inlineData").is_some());
                    let output = texts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n");
                    let mut f = json!({"name":name,"response":{"output":output}});
                    if let Some(id) = wire_id {
                        f["id"] = json!(id);
                    }
                    if !images.is_empty() {
                        f["parts"] = json!(images);
                    }
                    let part = json!({"functionResponse":f});
                    if let Some(prev) = contents
                        .last_mut()
                        .filter(|m| m["role"] == "user" && !m["parts"][0]["functionResponse"].is_null())
                    {
                        prev["parts"].as_array_mut().unwrap().push(part);
                    } else {
                        contents.push(json!({"role":"user","parts":[part]}));
                    }
                }
                _ => return Err(invalid("unknown message role")),
            }
        }
        let mut body = json!({"contents":contents});
        if !system.is_empty() {
            body["systemInstruction"] = json!({"parts":system});
        }
        if !request.tools.is_empty() {
            let mut decls = Vec::new();
            for t in &request.tools {
                let f = &t["function"];
                let name = f["name"].as_str().ok_or_else(|| invalid("invalid function declaration"))?;
                decls.push(json!({"name": name,"description": f["description"].as_str().unwrap_or(""),"parametersJsonSchema":f["parameters"]}));
            }
            body["tools"] = json!([{"functionDeclarations":decls}]);
        }
        let mut config = serde_json::Map::new();
        for (key, value) in request.options.as_object().into_iter().flatten() {
            match key.as_str() {
                "temperature" | "topP" | "topK" | "thinkingConfig" | "stopSequences" => {
                    config.insert(key.clone(), value.clone());
                }
                "top_p" => {
                    config.insert("topP".into(), value.clone());
                }
                "max_tokens" | "max_output_tokens" | "maxOutputTokens" => {
                    let limit = value.as_u64().filter(|n| *n > 0).ok_or_else(|| invalid("invalid maxOutputTokens"))?;
                    config.insert(
                        "maxOutputTokens".into(),
                        json!(clamp_max_tokens(limit, request.est_prompt_tokens, self.window)),
                    );
                }
                "reasoning_effort" => {
                    let level = match value.as_str() {
                        Some("minimal") => "MINIMAL",
                        Some("low") => "LOW",
                        Some("medium") => "MEDIUM",
                        Some("high" | "xhigh" | "max") => "HIGH",
                        _ => return Err(invalid("unknown thinking level")),
                    };
                    if request.options.get("thinkingConfig").is_some() {
                        return Err(invalid("use reasoning_effort or thinkingConfig, not both"));
                    }
                    config.insert("thinkingConfig".into(), json!({"thinkingLevel":level}));
                }
                _ => return Err(invalid("unsupported generation option")),
            }
        }
        if !config.is_empty() {
            body["generationConfig"] = Json::Object(config);
        }
        Ok(body)
    }
}

fn parts(content: &Json) -> Result<Vec<Json>, ProviderError> {
    super::content::parts(content, "gemini")
}

#[derive(Default)]
struct Answer {
    parts: Vec<Json>,
    visible: String,
    usage: Json,
    stopped: bool,
}
impl Answer {
    fn frame(&mut self, frame: Json, event: &mut (dyn FnMut(ProviderEvent) + Send)) -> Result<(), ProviderError> {
        if frame.get("error").is_some() || !frame["promptFeedback"]["blockReason"].is_null() {
            return Err(invalid("provider error or blocked prompt"));
        }
        if let Some(usage) = frame.get("usageMetadata") {
            self.usage = usage.clone();
        }
        let Some(candidates) = frame["candidates"].as_array() else {
            return if frame.get("usageMetadata").is_some() { Ok(()) } else { Err(invalid("missing candidate")) };
        };
        if candidates.is_empty() && frame.get("usageMetadata").is_some() {
            return Ok(());
        }
        let [candidate] = candidates.as_slice() else {
            return Err(invalid("expected exactly one candidate"));
        };
        if self.stopped || candidate.get("index").is_some_and(|index| index.as_u64() != Some(0)) {
            return Err(invalid("unexpected candidate"));
        }
        if let Some(reason) = candidate.get("finishReason") {
            if reason != "STOP" {
                return Err(invalid("candidate did not finish with STOP"));
            }
            self.stopped = true;
        }
        for p in candidate["content"]["parts"].as_array().into_iter().flatten() {
            if let Some(text) = p["text"].as_str() {
                if p["thought"].as_bool() != Some(true) && !text.is_empty() {
                    self.visible.push_str(text);
                    event(ProviderEvent::TextDelta(text.into()));
                }
            } else if let Some(f) = p.get("functionCall") {
                if f["name"].as_str().is_none_or(str::is_empty) || f.get("args").is_some_and(|v| !v.is_object()) {
                    return Err(invalid("malformed function call"));
                }
            } else {
                return Err(invalid("unsupported candidate part"));
            }
            self.parts.push(p.clone());
        }
        Ok(())
    }
}

impl Provider for Gemini {
    fn protocol(&self) -> &str {
        "gemini"
    }
    async fn complete(
        &self,
        request: &ModelRequest,
        cancel: &Cancel,
        event: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<AttemptOutcome, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::interrupted("turn interrupted"));
        }
        let model = request.model.strip_prefix("models/").unwrap_or(&request.model);
        if model.is_empty()
            || !model.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            || matches!(model, "." | "..")
        {
            return Err(invalid("invalid model id"));
        }
        let endpoint =
            format!("{}/models/{model}:streamGenerateContent?alt=sse", self.base.as_str().trim_end_matches('/'));
        let started = Instant::now();
        let mut timing = StreamTiming::new(started);
        let mut req = self.client.post(endpoint).timeout(self.timeout).json(&self.body(request)?);
        if !self.key.is_empty() {
            req = req.header("x-goog-api-key", &self.key);
        }
        let response = tokio::select! { biased; _=cancel.cancelled()=>return Err(ProviderError::interrupted("turn interrupted")), r=req.send()=>r.map_err(|_|ProviderError::transient("Gemini transport failed"))? };
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok())
                .map(Duration::from_secs);
            let bytes = read_body(response, cancel).await?;
            let mut error = ProviderError::status(status, &String::from_utf8_lossy(&bytes), retry);
            error.message = format!("Gemini API returned HTTP {status}");
            return Err(error);
        }
        let mut answer = Answer::default();
        if response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|s| s.starts_with("text/event-stream"))
        {
            let mut frame = |s: &str| {
                answer.frame(serde_json::from_str(s).map_err(|_| invalid("malformed stream JSON"))?, event)?;
                Ok(ControlFlow::Continue(()))
            };
            match pump_sse(response, cancel, stream_stall_bound(self.timeout), &mut frame, &mut timing).await? {
                SseEnd::Transport(s) => return Err(stream_failure_msg(&s, !answer.visible.is_empty())),
                SseEnd::Cancelled => return Err(ProviderError::interrupted("turn interrupted")),
                SseEnd::Closed => {}
            }
        } else {
            let bytes = read_body(response, cancel).await?;
            answer.frame(serde_json::from_slice(&bytes).map_err(|_| invalid("malformed response JSON"))?, event)?;
        }
        if cancel.is_cancelled() {
            return Err(ProviderError::interrupted("turn interrupted"));
        }
        if !answer.stopped {
            return Err(stream_failure_msg("Gemini response ended before STOP", !answer.visible.is_empty()));
        }
        let calls:Vec<_>=answer.parts.iter().filter_map(|p|p.get("functionCall")).enumerate().map(|(i,f)|json!({"id":format!("{}-{i}",request.request_id),"type":"function","function":{"name":f["name"],"arguments":f.get("args").cloned().unwrap_or_else(||json!({})).to_string()}})).collect();
        if calls.is_empty() && answer.visible.is_empty() {
            return Err(invalid("empty candidate"));
        }
        let mut message = json!({"role":"assistant","content":answer.visible,"gemini_content":{"base":self.base.as_str(),"model":request.model,"parts":answer.parts}});
        if !calls.is_empty() {
            message["tool_calls"] = json!(calls);
        }
        let usage = (|| {
            let prompt = answer.usage["promptTokenCount"].as_u64()?;
            let total = answer.usage["totalTokenCount"].as_u64()?;
            let hit = answer.usage["cachedContentTokenCount"].as_u64().unwrap_or(0);
            Some(Usage {
                prompt,
                completion: answer.usage["candidatesTokenCount"]
                    .as_u64()
                    .unwrap_or(0)
                    .checked_add(answer.usage["thoughtsTokenCount"].as_u64().unwrap_or(0))?,
                total,
                cache_hit: hit,
                cache_miss: prompt.checked_sub(hit)?,
                cache_write: 0,
            })
        })();
        Ok(AttemptOutcome {
            timing: timing.report(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            raw: json!({"candidates":[{"content":{"role":"model","parts":answer.parts},"finishReason":"STOP"}],"usageMetadata":answer.usage}),
            response: ModelResponse { message, usage, native: json!({"protocol":"gemini","base":self.base.as_str()}) },
        })
    }
}
