//! Sign in with ChatGPT token sharing. This transport has no configurable origin or billing fallback.
use super::{body, decode_action, http_agent, valid_model_id, ModelError, ModelSettings, Provider, LIMIT, TIMEOUT};
use crate::{protocol::TurnKind, Result};
use ring::digest::{digest, Digest, SHA256};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashSet, io::Read, sync::Mutex};

const MODELS: &str = "https://api.openai.com/v1/models";
const RESPONSES: &str = "https://api.openai.com/v1/responses";
const INVALID: &str = "invalid or ambiguous ChatGPT event stream; no action accepted";
const TEXT_ONLY: &str = "ChatGPT must return a single final assistant text action without tools or refusals";
const STREAM_LIMIT: usize = 16 * 1024 * 1024;

fn authorize(req: ureq::Request, token: &str) -> Result<ureq::Request> {
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("invalid ChatGPT access token; run teamagents auth login".into());
    }
    Ok(req.set("Authorization", &format!("Bearer {token}")).set("Accept-Encoding", "identity"))
}

fn recovery(status: Option<u16>, code: Option<&str>) -> String {
    // Neither error messages, detail, params, unknown codes nor request IDs are trusted for display.
    let help = match code {
        Some("subscription_sharing_usage_limit_exceeded") => {
            "plan limit reached; pause requests and check https://chatgpt.com/settings/usage"
        }
        Some("subscription_sharing_unsupported_capability") => {
            "unsupported plan capability; check the selected model and reasoning effort, or use the provider defaults"
        }
        Some("subscription_sharing_invalid_user") => {
            "check the signed-in account and its plan authorization with teamagents auth status"
        }
        Some("subscription_sharing_user_not_eligible") => {
            "this account or workspace policy is ineligible for plan sharing; check with your administrator"
        }
        Some("subscription_sharing_route_not_supported") => {
            "direct Responses routing is unavailable for this integration; check the OAuth client configuration"
        }
        Some("chatpass_v2_scope_not_authorized" | "chatpass_v2_invalid_authorization_context") => {
            "check the OAuth client grant and authorize ChatGPT plan access with teamagents auth login"
        }
        Some("subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable") => {
            "account or usage checks are temporarily unavailable; keep credentials and try later"
        }
        _ => match status {
            Some(401) => "check the selected account and plan permission with teamagents auth status",
            Some(403) => "check plan eligibility, workspace policy and supported region",
            Some(429) => "pause requests and check https://chatgpt.com/settings/usage",
            Some(500..=599) => "service unavailable; keep credentials and try later",
            Some(400 | 404 | 422) => "check the request and selected model for this account",
            _ => "request failed; no action accepted",
        },
    };
    match status {
        Some(s) => format!("ChatGPT HTTP {s}: {help}"),
        None => format!("ChatGPT inference failed: {help}"),
    }
}

fn checked(response: std::result::Result<ureq::Response, ureq::Error>) -> Result<ureq::Response> {
    match response {
        Ok(r) if r.status() == 200 => Ok(r),
        Ok(r) | Err(ureq::Error::Status(_, r)) => {
            let status = r.status();
            let details = bounded(r, 16 * 1024).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
            let code = details.as_ref().and_then(|v| v.pointer("/error/code")).and_then(Value::as_str);
            Err(recovery(Some(status), code))
        }
        Err(ureq::Error::Transport(_)) => Err("ChatGPT transport failed; request not retried".into()),
    }
}

fn bounded(r: ureq::Response, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    r.into_reader()
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "ChatGPT response interrupted; request not retried")?;
    if bytes.len() > limit {
        return Err("ChatGPT response exceeds the size limit; no action accepted".into());
    }
    Ok(bytes)
}

pub(super) fn models(token: &str) -> Result<Vec<String>> {
    discover(authorize(http_agent(TIMEOUT).get(MODELS), token)?)
}

fn discover(req: ureq::Request) -> Result<Vec<String>> {
    let bytes = bounded(checked(req.set("Accept", "application/json").call())?, LIMIT)?;
    let catalog: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid ChatGPT model catalog")?;
    let entries = catalog["models"].as_array().ok_or("invalid ChatGPT model catalog")?;
    let mut seen = HashSet::new();
    Ok(entries
        .iter()
        .filter(|m| m["visibility"] == "list")
        .filter_map(|m| m["slug"].as_str())
        .filter(|slug| valid_model_id(slug) && seen.insert(*slug))
        .map(str::to_owned)
        .collect())
}

#[derive(Default)]
struct ModelCache(Mutex<Option<(Digest, String)>>);

impl ModelCache {
    fn select(
        &self,
        token: &str,
        model: Option<&str>,
        discover: impl FnOnce() -> Result<Vec<String>>,
    ) -> Result<String> {
        if let Some(model) = model {
            return Ok(model.to_owned());
        }
        // A new token (including after login or refresh) gets a fresh account-specific catalog.
        // Never keep a reusable credential or hold the cache lock during network I/O.
        let key = digest(&SHA256, token.as_bytes());
        if let Some((cached, model)) = &*self.0.lock().map_err(|_| "ChatGPT model cache unavailable")? {
            if cached.as_ref() == key.as_ref() {
                return Ok(model.clone());
            }
        }
        let model = discover()?
            .into_iter()
            .next()
            .ok_or("no visible ChatGPT model; check the signed-in account and its plan permissions")?;
        *self.0.lock().map_err(|_| "ChatGPT model cache unavailable")? = Some((key, model.clone()));
        Ok(model)
    }
}

pub(super) fn complete(
    token: &str,
    settings: &ModelSettings,
    agent: &str,
    task: &str,
    history: &[Value],
    kind: TurnKind,
) -> std::result::Result<Value, ModelError> {
    static CACHE: ModelCache = ModelCache(Mutex::new(None));
    let model = CACHE.select(token, settings.model.as_deref(), || models(token)).map_err(ModelError::Fatal)?;
    let bytes = body(&model, settings.effort.as_deref(), agent, task, history, kind, Provider::ChatGpt)
        .map_err(ModelError::Fatal)?;
    let req = authorize(http_agent(TIMEOUT).post(RESPONSES), token).map_err(ModelError::Fatal)?;
    infer(req, &bytes, kind)
}

fn infer(req: ureq::Request, bytes: &[u8], kind: TurnKind) -> std::result::Result<Value, ModelError> {
    let r = checked(req.set("Content-Type", "application/json").set("Accept", "text/event-stream").send_bytes(bytes))
        .map_err(ModelError::Fatal)?;
    // Some gateways omit the SSE media type. Only a validated terminal event authorizes an action.
    let media = match r.header("Content-Type").map(|_| r.content_type().trim().to_ascii_lowercase()).as_deref() {
        Some("text/event-stream") => "text/event-stream",
        Some("application/json") => "application/json",
        Some("text/plain") => "text/plain",
        Some("application/octet-stream") => "application/octet-stream",
        Some("text/html") => "text/html",
        None => "missing Content-Type",
        _ => "other Content-Type",
    };
    let bytes = bounded(r, STREAM_LIMIT).map_err(ModelError::Fatal)?;
    let text = std::str::from_utf8(&bytes).ok().map(|s| s.strip_prefix('\u{feff}').unwrap_or(s));
    if let Some(text) = text {
        let first = text.lines().find(|line| !line.is_empty()).unwrap_or("");
        let framed = ["event:", "data:", ":"].iter().any(|prefix| first.starts_with(prefix));
        if media == "text/event-stream" || (media != "text/html" && framed) {
            return decode(text, kind);
        }
        if let Ok(json) = serde_json::from_str::<Value>(text) {
            if let Some(error) = json.get("error").filter(|e| !e.is_null()) {
                return Err(ModelError::Fatal(recovery(Some(200), error.get("code").and_then(Value::as_str))));
            }
            return Err(ModelError::Fatal(format!(
                "ChatGPT HTTP 200 ({media}): received JSON instead of a completed event stream"
            )));
        }
    }
    Err(ModelError::Fatal(format!("ChatGPT HTTP 200 ({media}): expected a ChatGPT event stream; no action accepted")))
}

// Typed wire fields reject duplicates. Unknown metadata is never interpreted as an action.
#[derive(Deserialize)]
struct Part {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

#[derive(Deserialize)]
struct Item {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    status: Option<String>,
    role: Option<String>,
    phase: Option<String>,
    #[serde(default)]
    content: Vec<Part>,
}

impl Item {
    fn message(&self) -> bool {
        self.kind == "message"
            && self.role.as_deref() == Some("assistant")
            && matches!(self.phase.as_deref(), None | Some("final_answer"))
    }

    fn text(&self) -> Result<&str> {
        if let [part] = self.content.as_slice() {
            if self.message() && self.status.as_deref() == Some("completed") && part.kind == "output_text" {
                return part.text.as_deref().ok_or_else(|| TEXT_ONLY.into());
            }
        }
        Err(TEXT_ONLY.into())
    }
}

#[derive(Deserialize)]
struct Failure {
    code: Option<String>,
}

#[derive(Deserialize)]
struct Response {
    id: Option<String>,
    status: Option<String>,
    #[serde(default)]
    output: Vec<Item>,
    error: Option<Failure>,
    incomplete_details: Option<serde::de::IgnoredAny>,
}

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    sequence_number: Option<u64>,
    response: Option<Response>,
    response_id: Option<String>,
    output_index: Option<usize>,
    content_index: Option<usize>,
    item_id: Option<String>,
    item: Option<Item>,
    part: Option<Part>,
    delta: Option<String>,
    text: Option<String>,
    code: Option<String>,
    error: Option<Failure>,
}

fn events(stream: &str) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    let mut data = String::new();
    let mut name = None;
    let mut done = false;
    let mut sequence = None;
    for line in stream.lines() {
        if line.is_empty() {
            if data.is_empty() && name.is_some() {
                return Err(INVALID.into());
            }
            if !data.is_empty() {
                if done {
                    return Err(INVALID.into());
                }
                if data == "[DONE]\n" && name.is_none() {
                    done = true;
                } else {
                    let event: Event = serde_json::from_str(&data).map_err(|_| INVALID)?;
                    if name.is_some_and(|s| s != event.kind) {
                        return Err(INVALID.into());
                    }
                    if let Some(n) = event.sequence_number {
                        if sequence.is_some_and(|prev| n <= prev) {
                            return Err(INVALID.into());
                        }
                        sequence = Some(n);
                    }
                    match event.kind.as_str() {
                        "error" | "response.failed" => {
                            let failure =
                                event.response.as_ref().and_then(|r| r.error.as_ref()).or(event.error.as_ref());
                            return Err(recovery(
                                None,
                                event.code.as_deref().or_else(|| failure.and_then(|e| e.code.as_deref())),
                            ));
                        }
                        "response.incomplete" => return Err("ChatGPT response incomplete; no action accepted".into()),
                        _ => events.push(event),
                    }
                }
            }
            data.clear();
            name = None;
        } else {
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "data" => {
                    data.push_str(value);
                    data.push('\n');
                }
                "event" if name.replace(value).is_none() => (),
                "" | "id" | "retry" => (),
                _ => return Err(INVALID.into()),
            }
        }
    }
    if !data.is_empty() || name.is_some() {
        return Err(INVALID.into());
    }
    Ok(events)
}

fn final_text(response: &Response) -> Result<&str> {
    if response.status.as_deref() != Some("completed")
        || response.id.as_deref().is_none_or(str::is_empty)
        || response.error.is_some()
        || response.incomplete_details.is_some()
    {
        return Err(INVALID.into());
    }
    let mut ids = HashSet::new();
    let mut text = None;
    for item in &response.output {
        if item.id.is_empty() || !ids.insert(&item.id) {
            return Err(INVALID.into());
        }
        if item.kind == "reasoning" && matches!(item.status.as_deref(), None | Some("completed")) {
            continue;
        }
        if text.replace(item.text()?).is_some() {
            return Err(TEXT_ONLY.into());
        }
    }
    text.ok_or_else(|| TEXT_ONLY.into())
}

// The terminal response contains the authoritative output. Reconcile all observed deltas and
// snapshots with it. A missing redundant snapshot is fine; contradictory evidence is not.
fn decode(stream: &str, kind: TurnKind) -> std::result::Result<Value, ModelError> {
    let mut events = events(stream).map_err(ModelError::Fatal)?;
    let terminal = events.pop().filter(|e| e.kind == "response.completed").ok_or_else(|| {
        ModelError::Fatal("ChatGPT stream ended without response.completed; no action accepted".into())
    })?;
    let completed = terminal.response.ok_or_else(|| ModelError::Fatal(INVALID.into()))?;
    if terminal.response_id.is_some() && terminal.response_id != completed.id {
        return Err(ModelError::Fatal(INVALID.into()));
    }
    let text = final_text(&completed).map_err(ModelError::Fatal)?;
    if text.len() > LIMIT {
        return Err(ModelError::Fatal("ChatGPT response exceeds the size limit; no action accepted".into()));
    }
    let mut remaining = text;
    let mut saw_delta = false;
    let mut position = None;
    for event in events {
        if event.response_id.is_some() && event.response_id != completed.id {
            return Err(ModelError::Fatal("ChatGPT event refers to a different response ID".into()));
        }
        if matches!(event.kind.as_str(), "response.created" | "response.in_progress" | "response.queued") {
            let start = event.response.as_ref().ok_or_else(|| ModelError::Fatal(INVALID.into()))?;
            if position.is_some()
                || start.id != completed.id
                || !start.output.is_empty()
                || start.error.is_some()
                || start.incomplete_details.is_some()
                || !matches!(start.status.as_deref(), Some("queued" | "in_progress"))
            {
                return Err(ModelError::Fatal(INVALID.into()));
            }
            continue;
        }
        if event.kind == "response.metadata" {
            // Subscription streams attach metadata to the response, not to an output item.
            if event.response_id != completed.id
                || event.output_index.is_some()
                || event.item.is_some()
                || event.error.is_some()
            {
                return Err(ModelError::Fatal("ChatGPT response metadata did not match the completed response".into()));
            }
            continue;
        }
        let index = event.output_index.ok_or_else(|| {
            ModelError::Fatal(
                "ChatGPT stream contained an unrecognized response-level event or a missing output index".into(),
            )
        })?;
        let item = completed.output.get(index).ok_or_else(|| ModelError::Fatal(INVALID.into()))?;
        if event.item_id.as_ref().is_some_and(|id| id != &item.id) {
            return Err(ModelError::Fatal(INVALID.into()));
        }
        let stage = match event.kind.as_str() {
            "response.output_item.added" | "response.output_item.done" => {
                let got = event.item.as_ref().ok_or_else(|| ModelError::Fatal(INVALID.into()))?;
                let added = event.kind.ends_with(".added");
                if got.id != item.id || got.kind != item.kind {
                    return Err(ModelError::Fatal(INVALID.into()));
                }
                if item.kind == "message" {
                    if added {
                        if !got.message() || got.status.as_deref() != Some("in_progress") || !got.content.is_empty() {
                            return Err(ModelError::Fatal(INVALID.into()));
                        }
                    } else if got.text().map_err(ModelError::Fatal)? != text {
                        return Err(ModelError::Fatal(INVALID.into()));
                    }
                } else if !matches!(got.status.as_deref(), None | Some("in_progress" | "completed")) {
                    return Err(ModelError::Fatal(INVALID.into()));
                }
                if added {
                    1
                } else {
                    6
                }
            }
            "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.delta"
            | "response.reasoning_text.done"
                if item.kind == "reasoning" && event.item_id.is_some() =>
            {
                3
            }
            _ if item.kind != "message" || event.content_index != Some(0) || event.item_id.is_none() => {
                return Err(ModelError::Fatal(TEXT_ONLY.into()))
            }
            "response.content_part.added" | "response.content_part.done" => {
                let part = event.part.as_ref().ok_or_else(|| ModelError::Fatal(INVALID.into()))?;
                let added = event.kind.ends_with(".added");
                if part.kind != "output_text" || part.text.as_deref() != Some(if added { "" } else { text }) {
                    return Err(ModelError::Fatal(TEXT_ONLY.into()));
                }
                if added {
                    2
                } else {
                    5
                }
            }
            "response.output_text.delta" => {
                let delta = event.delta.as_deref().ok_or_else(|| ModelError::Fatal(INVALID.into()))?;
                remaining = remaining.strip_prefix(delta).ok_or_else(|| ModelError::Fatal(INVALID.into()))?;
                saw_delta = true;
                3
            }
            "response.output_text.done" if event.text.as_deref() == Some(text) => 4,
            _ => return Err(ModelError::Fatal(TEXT_ONLY.into())),
        };
        let next = (index, stage);
        if position.is_some_and(|last| next < last || (next == last && stage != 3)) {
            return Err(ModelError::Fatal(INVALID.into()));
        }
        position = Some(next);
    }
    if saw_delta && !remaining.is_empty() {
        return Err(ModelError::Fatal(INVALID.into()));
    }
    decode_action(text, kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
        time::Duration,
    };

    const ACTION: &str = r#"{"op":"finish","text":"ok"}"#;
    const TOKEN: &str = "offline-fixture";

    fn message(text: &str) -> Value {
        json!({"type":"message","id":"msg","role":"assistant","status":"completed","content":[{"type":"output_text","text":text}]})
    }

    fn terminal(text: &str) -> Value {
        json!({"type":"response.completed","response":{"id":"resp","status":"completed","output":[message(text)]}})
    }

    fn delta(text: &str) -> Value {
        json!({"type":"response.output_text.delta","response_id":"resp","output_index":0,"content_index":0,"item_id":"msg","delta":text})
    }

    fn frames(events: &[Value]) -> String {
        events.iter().map(|e| format!("event: {}\r\ndata: {e}\r\n\r\n", e["type"].as_str().unwrap())).collect()
    }

    #[test]
    fn response_metadata_is_bound_to_the_stream_and_never_supplies_an_action() {
        let meta =
            json!({"type":"response.metadata","response_id":"resp","sequence_number":2,"metadata":{"text":"ignored"}});
        let end = terminal(ACTION);
        let stream = frames(&[meta.clone(), delta(ACTION), end.clone()]);
        assert_eq!(decode(&stream, TurnKind::Root).unwrap()["text"], "ok");
        assert!(decode(&frames(std::slice::from_ref(&meta)), TurnKind::Root).is_err());
        for id in [Value::Null, json!("other")] {
            let mut wrong = meta.clone();
            wrong["response_id"] = id;
            assert!(decode(&frames(&[wrong, end.clone()]), TurnKind::Root).is_err());
        }
    }

    #[test]
    fn reconciles_deltas_reasoning_and_snapshots_with_one_terminal_action() {
        let reasoning = json!({"type":"reasoning","id":"reason","summary":[]});
        let mut first = delta(&ACTION[..5]);
        first["output_index"] = json!(1);
        let mut second = delta(&ACTION[5..]);
        second["output_index"] = json!(1);
        let part = json!({"type":"output_text","text":ACTION});
        let mut end = terminal(ACTION);
        end["response"]["output"] = json!([reasoning, message(ACTION)]);
        let stream = frames(&[
            json!({"type":"response.created","response":{"id":"resp","status":"in_progress","output":[]}}),
            json!({"type":"response.output_item.added","output_index":0,"item":reasoning}),
            json!({"type":"response.reasoning_summary_text.delta","output_index":0,"item_id":"reason","delta":"not an action"}),
            json!({"type":"response.output_item.done","output_index":0,"item":reasoning}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"message","id":"msg","role":"assistant","status":"in_progress","content":[]}}),
            json!({"type":"response.content_part.added","output_index":1,"item_id":"msg","content_index":0,"part":{"type":"output_text","text":""}}),
            first,
            second,
            json!({"type":"response.output_text.done","output_index":1,"item_id":"msg","content_index":0,"text":ACTION}),
            json!({"type":"response.content_part.done","output_index":1,"item_id":"msg","content_index":0,"part":part}),
            json!({"type":"response.output_item.done","output_index":1,"item":message(ACTION)}),
            end,
        ]);
        let stream = format!(": heartbeat\r\n\r\n{stream}data: [DONE]\r\n\r\n");
        assert_eq!(decode(&stream, TurnKind::Reporter).unwrap(), json!({"op":"finish","text":"ok"}));
        let ask = frames(&[terminal(r#"{"op":"ask","text":"which?"}"#)]);
        assert!(decode(&ask, TurnKind::Root).is_ok());
        assert!(decode(&ask, TurnKind::Node).is_err());
        assert!(decode(&ask, TurnKind::Reporter).is_err());
    }

    // Only these tests substitute loopback requests for the fixed production endpoints.
    fn serve(
        status: u16,
        headers: &str,
        payload: String,
        delay: Duration,
    ) -> (String, thread::JoinHandle<(String, String, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let headers = headers.to_owned();
        let handle = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut r = BufReader::new(&socket);
            let mut path = String::new();
            r.read_line(&mut path).unwrap();
            let mut request_headers = String::new();
            let mut size = 0;
            for line in r.by_ref().lines() {
                let line = line.unwrap();
                if line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("Content-Length") {
                        size = value.trim().parse().unwrap();
                    }
                }
                request_headers.push_str(&line);
                request_headers.push('\n');
            }
            let mut body = vec![0; size];
            r.read_exact(&mut body).unwrap();
            write!(
                socket,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n\r\n",
                payload.len()
            )
            .unwrap();
            thread::sleep(delay);
            let _ = socket.write_all(payload.as_bytes());
            (path, request_headers, body)
        });
        (url, handle)
    }

    #[test]
    fn catalog_and_responses_use_one_bearer_preserve_order_and_roles() {
        assert_eq!(MODELS, "https://api.openai.com/v1/models");
        assert_eq!(RESPONSES, "https://api.openai.com/v1/responses");
        let catalog = json!({"models":[
            {"visibility":"hide","slug":"hidden"}, {"visibility":"list","slug":"bad\nslug"},
            {"visibility":"list","slug":"z-first"}, {"visibility":"list","slug":"a-second"},
            {"visibility":"list","slug":"z-first"}, {"visibility":"list","slug":""},
            {"visibility":"list","slug":"\u{1b}[2J"}, {"visibility":"list","slug":"x".repeat(257)}
        ]});
        let (url, server) = serve(200, "Content-Type: application/json", catalog.to_string(), Duration::ZERO);
        let listed = discover(authorize(http_agent(TIMEOUT).get(&format!("{url}/v1/models")), TOKEN).unwrap()).unwrap();
        assert_eq!(listed, ["z-first", "a-second"]);
        let cache = ModelCache::default();
        let model = cache.select(TOKEN, None, || Ok(listed)).unwrap();
        assert_eq!(model, "z-first");
        let (line, headers, body_bytes) = server.join().unwrap();
        assert_eq!(line.trim_end(), "GET /v1/models HTTP/1.1");
        assert!(headers.contains("Bearer offline-fixture") && body_bytes.is_empty());
        assert_eq!(cache.select(TOKEN, None, || panic!("cached model")).unwrap(), model);
        assert_eq!(cache.select(TOKEN, Some("override"), || panic!("explicit override")).unwrap(), "override");
        assert!(cache.select("other-account", None, || Err("discovery failed".into())).is_err());
        assert_eq!(cache.select("other-account", None, || Ok(vec!["other-model".into()])).unwrap(), "other-model");

        let (url, server) = serve(
            200,
            "Content-Type: text/event-stream; charset=utf-8",
            frames(&[delta(ACTION), terminal(ACTION)]),
            Duration::ZERO,
        );
        let history = [
            json!({"role":"user","content":"task"}),
            json!({"role":"assistant","content":"prior action"}),
            json!({"role":"user","content":"receipt"}),
        ];
        let bytes = body(&model, None, "worker", "task-id", &history, TurnKind::Node, Provider::ChatGpt).unwrap();
        let req = authorize(http_agent(TIMEOUT).post(&format!("{url}/v1/responses")), TOKEN).unwrap();
        assert_eq!(infer(req, &bytes, TurnKind::Node).unwrap()["text"], "ok");
        let (line, headers, bytes) = server.join().unwrap();
        assert_eq!(line.trim_end(), "POST /v1/responses HTTP/1.1");
        assert!(headers.contains("Bearer offline-fixture") && headers.contains("text/event-stream"));
        let wire: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            wire,
            json!({"model":"z-first","input":[{"role":"user","content":"Agent: worker\nTask: task-id"},history[0],history[1],history[2]],"instructions":super::super::WORKER,"store":false,"stream":true})
        );
    }

    #[test]
    fn handles_completed_streams_with_nonstandard_headers_and_reports_non_stream_errors() {
        for header in [
            "X-Fixture: no-content-type",
            "Content-Type: text/plain",
            "Content-Type: text/event-stream ; charset=utf-8",
        ] {
            let (url, server) = serve(200, header, frames(&[terminal(ACTION)]), Duration::ZERO);
            assert_eq!(infer(http_agent(TIMEOUT).post(&url), b"{}", TurnKind::Root).unwrap()["text"], "ok");
            server.join().unwrap();
        }
        for (header, body, expected) in [
            (
                "Content-Type: application/json",
                json!({"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"private"}}).to_string(),
                "plan limit",
            ),
            ("Content-Type: application/json", terminal(ACTION).to_string(), "received JSON"),
            ("Content-Type: text/html", "<html>private</html>".into(), "text/html"),
            ("Content-Type: text/plain", frames(&[delta(ACTION)]), "without response.completed"),
        ] {
            let (url, server) = serve(200, header, body, Duration::ZERO);
            let error = infer(http_agent(TIMEOUT).post(&url), b"{}", TurnKind::Root).unwrap_err().to_string();
            assert!(error.contains(expected) && !error.contains("private"), "{error}");
            server.join().unwrap();
        }
    }

    #[test]
    fn http_limits_redaction_redirect_and_deadline_fail_closed() {
        let trap = TcpListener::bind("127.0.0.1:0").unwrap();
        trap.set_nonblocking(true).unwrap();
        let location = format!("Location: http://{}/secret", trap.local_addr().unwrap());
        for (status, headers, payload, delay, expected) in [
            (307, location.as_str(), String::new(), Duration::ZERO, "307"),
            (
                403,
                "Content-Type: application/json",
                json!({"detail":"secret"}).to_string(),
                Duration::ZERO,
                "workspace policy",
            ),
            (
                400,
                "Content-Type: application/json",
                json!({"error":{"code":"subscription_sharing_unsupported_capability","param":"secret"}}).to_string(),
                Duration::ZERO,
                "selected model and reasoning effort",
            ),
            (
                200,
                "Content-Type: text/event-stream",
                frames(&[
                    json!({"type":"response.failed","response":{"error":{"code":"subscription_sharing_unsupported_capability","message":"secret"}}}),
                ]),
                Duration::ZERO,
                "selected model and reasoning effort",
            ),
            (200, "Content-Type: application/json", String::new(), Duration::ZERO, "event stream"),
            (200, "Content-Type: text/event-stream", "x".repeat(STREAM_LIMIT + 1), Duration::ZERO, "size limit"),
            (
                200,
                "Content-Type: text/event-stream",
                frames(&[terminal(&json!({"op":"finish","text":"y".repeat(LIMIT)}).to_string())]),
                Duration::ZERO,
                "size limit",
            ),
            (
                200,
                "Content-Type: text/event-stream",
                frames(&[terminal(ACTION)]),
                Duration::from_millis(350),
                "interrupted",
            ),
        ] {
            let (url, server) = serve(status, headers, payload, delay);
            let timeout = if delay.is_zero() { TIMEOUT } else { Duration::from_millis(100) };
            let err = infer(authorize(http_agent(timeout).post(&url), TOKEN).unwrap(), b"{}", TurnKind::Root)
                .unwrap_err()
                .to_string();
            assert!(err.contains(expected) && !err.contains("secret") && !err.contains(TOKEN), "{err}");
            server.join().unwrap();
        }
        // SSE framing larger than `LIMIT` (e.g. xhigh reasoning streams) still decodes when action text <= `LIMIT`.
        let sse_over_1m = format!(":{}\r\n\r\n{}", " ".repeat(LIMIT + 64), frames(&[terminal(ACTION)]));
        let (url, server) = serve(200, "Content-Type: text/event-stream", sse_over_1m, Duration::ZERO);
        assert_eq!(
            infer(authorize(http_agent(TIMEOUT).post(&url), TOKEN).unwrap(), b"{}", TurnKind::Root).unwrap()["text"],
            "ok"
        );
        server.join().unwrap();
        assert_eq!(trap.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        for payload in ["{}", r#"{"data":[{"id":"api-model"}]}"#] {
            let (url, server) = serve(200, "Content-Type: application/json", payload.to_owned(), Duration::ZERO);
            assert!(discover(authorize(http_agent(TIMEOUT).get(&url), TOKEN).unwrap()).is_err());
            server.join().unwrap();
        }
        let hidden = json!({"models":[{"slug":"hidden","visibility":"hide"}]}).to_string();
        let (url, server) = serve(200, "Content-Type: application/json", hidden, Duration::ZERO);
        let listed = discover(authorize(http_agent(TIMEOUT).get(&url), TOKEN).unwrap()).unwrap();
        assert!(listed.is_empty());
        assert!(ModelCache::default().select(TOKEN, None, || Ok(listed)).unwrap_err().contains("no visible"));
        server.join().unwrap();
        assert!(authorize(http_agent(TIMEOUT).get(MODELS), "bad\r\ntoken").is_err());
    }

    #[test]
    fn refuses_truncation_errors_conflicts_and_ambiguous_actions() {
        let end = terminal(ACTION);
        let reject = |es: &[Value]| assert!(decode(&frames(es), TurnKind::Root).is_err());
        reject(&[delta(ACTION)]);
        for key in ["item_id", "response_id", "content_index", "output_index"] {
            let mut d = delta(ACTION);
            d[key] = if key.ends_with("index") { json!(1) } else { json!("other") };
            reject(&[d, end.clone()]);
        }
        for (path, value) in [
            ("/response/output/0/type", json!("function_call")),
            ("/response/output/0/role", json!("user")),
            ("/response/output/0/status", json!("incomplete")),
            ("/response/output/0/content/0/type", json!("refusal")),
            ("/response/output/0/content", json!([])),
            ("/response/status", json!("incomplete")),
            ("/response/output", json!([message(ACTION), message(ACTION)])),
        ] {
            let mut bad = end.clone();
            *bad.pointer_mut(path).unwrap() = value;
            reject(&[bad]);
        }
        for event in [
            json!({"type":"response.incomplete"}),
            json!({"type":"error"}),
            json!({"type":"response.failed"}),
            end.clone(),
        ] {
            reject(&[delta(ACTION), end.clone(), event]);
        }
        for event in [
            json!({"type":"response.refusal.delta"}),
            json!({"type":"response.function_call_arguments.delta"}),
            delta("wrong"),
            delta("{"),
            json!({"type":"response.created","response":{"id":"other","status":"in_progress"}}),
        ] {
            reject(&[event, end.clone()]);
        }
        for text in ["{} {}", r#"{"op":"members","op":"members"}"#, r#"{"op":"finish","text":"ok"} trailing"#] {
            reject(&[terminal(text)]);
        }
        let good = frames(std::slice::from_ref(&end));
        for stream in [
            "data: [DONE]\n\n".into(),
            good.trim_end().into(),
            good.replacen("event: response.completed", "event: response.failed", 1),
            good.replace("\"id\":\"resp\"", "\"id\":\"other\",\"id\":\"resp\""),
        ] {
            assert!(decode(&stream, TurnKind::Root).is_err());
        }
        let err = decode(&frames(&[delta(ACTION),json!({"type":"response.failed","response":{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"reflected-secret"}}})]),TurnKind::Root).unwrap_err().to_string();
        assert!(err.contains("https://chatgpt.com/settings/usage") && !err.contains("reflected-secret"));
    }
}
