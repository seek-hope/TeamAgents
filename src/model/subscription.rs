//! Sign in with ChatGPT token sharing. This transport has no configurable origin or billing fallback.
use super::{
    body_with_cache, http_agent, is_context_overflow_payload, is_retryable_status, note_rate_limit,
    providers::WireProtocol,
    read_stream_with_lines,
    responses::{decode_sse, emit_responses_stream_line, StreamDecodeErr},
    retry_delay, send_with_redirects, valid_model_id, wait_rate_limit, ModelError, ModelOutput, ModelSettings,
    StreamCallback, WireOptions, HTTP_MAX_ATTEMPTS, HTTP_RETRY_BASE, LIMIT, STREAM_LIMIT, TIMEOUT,
};
use crate::{protocol::TurnKind, Result};
use ring::digest::{digest, Digest, SHA256};
use serde_json::Value;
use std::{collections::HashSet, io::Read, sync::Mutex, thread, time::Duration};

const MODELS: &str = "https://api.openai.com/v1/models";
const RESPONSES: &str = "https://api.openai.com/v1/responses";
const INVALID: &str = "invalid or ambiguous ChatGPT event stream; no action accepted";
const TEXT_ONLY: &str = "ChatGPT must return a single final assistant text action without tools or refusals";

fn authorize(req: ureq::Request, token: &str) -> Result<ureq::Request> {
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("invalid ChatGPT access token; run teamagents auth login".into());
    }
    Ok(req.set("Authorization", &format!("Bearer {token}")).set("Accept-Encoding", "identity"))
}

fn auth_request(req: ureq::Request, token: &str, send_auth: bool) -> Result<ureq::Request> {
    if send_auth {
        authorize(req, token)
    } else {
        Ok(req.set("Accept-Encoding", "identity"))
    }
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

enum ReadError {
    Io,
    Oversize,
}

fn is_non_retryable_plan_code(code: Option<&str>) -> bool {
    matches!(
        code,
        Some(
            "subscription_sharing_usage_limit_exceeded"
                | "subscription_sharing_unsupported_capability"
                | "subscription_sharing_invalid_user"
                | "subscription_sharing_user_not_eligible"
                | "subscription_sharing_route_not_supported"
                | "chatpass_v2_scope_not_authorized"
                | "chatpass_v2_invalid_authorization_context"
        )
    )
}

fn is_quota_plan_code(code: Option<&str>) -> bool {
    code == Some("subscription_sharing_usage_limit_exceeded")
}

fn is_retryable_plan_sse_code(code: Option<&str>) -> bool {
    matches!(
        code,
        Some(
            "rate_limit_exceeded"
                | "server_error"
                | "timeout"
                | "subscription_sharing_usage_unavailable"
                | "subscription_sharing_user_unavailable"
        )
    )
}

enum RetryErr {
    Http { retryable: bool, retry_after: Option<String>, message: String },
    Compaction(String),
    Quota(String),
}

fn check_or_retry(
    response: std::result::Result<ureq::Response, ureq::Error>,
) -> std::result::Result<ureq::Response, RetryErr> {
    match response {
        Ok(r) if r.status() == 200 => Ok(r),
        Ok(r) | Err(ureq::Error::Status(_, r)) => {
            let status = r.status();
            let retry_after = r.header("Retry-After").map(str::to_owned);
            let raw = bounded(r, 16 * 1024).ok();
            let details = raw.as_deref().and_then(|b| serde_json::from_slice::<Value>(b).ok());
            let code = details.as_ref().and_then(|v| v.pointer("/error/code")).and_then(Value::as_str);
            if status == 413
                || (matches!(status, 400 | 422)
                    && (code == Some("context_length_exceeded")
                        || raw.as_deref().is_some_and(is_context_overflow_payload)))
            {
                return Err(RetryErr::Compaction(format!("ChatGPT HTTP {status}: context overflow")));
            }
            if is_quota_plan_code(code) || status == 402 {
                return Err(RetryErr::Quota(recovery(Some(status), code)));
            }
            let retryable = is_retryable_status(status) && !is_non_retryable_plan_code(code);
            Err(RetryErr::Http { retryable, retry_after, message: recovery(Some(status), code) })
        }
        Err(ureq::Error::Transport(_)) => {
            Err(RetryErr::Http { retryable: true, retry_after: None, message: "ChatGPT transport failed".into() })
        }
    }
}

fn read_bounded(r: ureq::Response, limit: usize) -> std::result::Result<Vec<u8>, ReadError> {
    let mut bytes = Vec::new();
    r.into_reader().take((limit + 1) as u64).read_to_end(&mut bytes).map_err(|_| ReadError::Io)?;
    if bytes.len() > limit {
        return Err(ReadError::Oversize);
    }
    Ok(bytes)
}

fn bounded(r: ureq::Response, limit: usize) -> Result<Vec<u8>> {
    read_bounded(r, limit).map_err(|e| match e {
        ReadError::Io => "ChatGPT response interrupted".into(),
        ReadError::Oversize => "ChatGPT response exceeds the size limit; no action accepted".into(),
    })
}

pub(super) fn models(token: &str) -> Result<Vec<String>> {
    discover_with_retry(
        MODELS,
        |url, send_auth| auth_request(http_agent(TIMEOUT).get(url), token, send_auth),
        HTTP_RETRY_BASE,
    )
}

fn discover_with_retry(
    initial_url: &str,
    mut build: impl FnMut(&str, bool) -> Result<ureq::Request>,
    retry_base: Duration,
) -> Result<Vec<String>> {
    for attempt in 1..=HTTP_MAX_ATTEMPTS {
        wait_rate_limit(retry_base);
        let outcome = send_with_redirects(initial_url, None, |url, send_auth| {
            Ok(build(url, send_auth)?.set("Accept", "application/json"))
        })?;
        let resp = match check_or_retry(outcome) {
            Ok(r) => r,
            Err(RetryErr::Http { retryable: true, retry_after, .. }) if attempt < HTTP_MAX_ATTEMPTS => {
                let delay = retry_delay(attempt, retry_after.as_deref(), retry_base);
                note_rate_limit(delay, retry_base);
                thread::sleep(delay);
                continue;
            }
            Err(RetryErr::Http { message, .. } | RetryErr::Compaction(message) | RetryErr::Quota(message)) => {
                return Err(message)
            }
        };
        let bytes = match read_bounded(resp, LIMIT) {
            Ok(b) => b,
            Err(ReadError::Io) if attempt < HTTP_MAX_ATTEMPTS => {
                thread::sleep(retry_delay(attempt, None, retry_base));
                continue;
            }
            Err(ReadError::Io) => return Err("ChatGPT response interrupted".into()),
            Err(ReadError::Oversize) => {
                return Err("ChatGPT response exceeds the size limit; no action accepted".into())
            }
        };
        let catalog: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid ChatGPT model catalog")?;
        let entries = catalog["models"].as_array().ok_or("invalid ChatGPT model catalog")?;
        let mut seen = HashSet::new();
        return Ok(entries
            .iter()
            .filter(|m| m["visibility"] == "list")
            .filter_map(|m| m["slug"].as_str())
            .filter(|slug| valid_model_id(slug) && seen.insert(*slug))
            .map(str::to_owned)
            .collect());
    }
    unreachable!()
}

#[cfg(test)]
fn discover(req: ureq::Request) -> Result<Vec<String>> {
    let ok = match check_or_retry(req.set("Accept", "application/json").call()) {
        Ok(r) => r,
        Err(RetryErr::Http { message, .. } | RetryErr::Compaction(message) | RetryErr::Quota(message)) => {
            return Err(message)
        }
    };
    let bytes = bounded(ok, LIMIT)?;
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
    on_stream: Option<StreamCallback<'_>>,
) -> std::result::Result<ModelOutput, ModelError> {
    static CACHE: ModelCache = ModelCache(Mutex::new(None));
    let model = CACHE.select(token, settings.model.as_deref(), || models(token)).map_err(ModelError::Fatal)?;
    let bytes = body_with_cache(
        &model,
        settings.effort.as_deref(),
        agent,
        task,
        None,
        history,
        kind,
        WireOptions::for_wire(WireProtocol::ChatGpt),
    )
    .map_err(ModelError::Fatal)?;
    infer_with_retry(
        RESPONSES,
        |url, send_auth| auth_request(http_agent(TIMEOUT).post(url), token, send_auth),
        &bytes,
        kind,
        HTTP_RETRY_BASE,
        on_stream,
    )
}

fn map_stream_err(err: StreamDecodeErr, kind: TurnKind) -> std::result::Result<ModelError, RetryErr> {
    match err {
        StreamDecodeErr::Model(m) => Ok(m),
        StreamDecodeErr::ProviderEvent { code, .. } => {
            if kind != TurnKind::Reporter && code.as_deref() == Some("context_length_exceeded") {
                return Err(RetryErr::Compaction("ChatGPT HTTP 200: context overflow".into()));
            }
            if is_quota_plan_code(code.as_deref()) {
                return Err(RetryErr::Quota(recovery(None, code.as_deref())));
            }
            if is_retryable_plan_sse_code(code.as_deref()) {
                return Err(RetryErr::Http {
                    retryable: true,
                    retry_after: None,
                    message: recovery(None, code.as_deref()),
                });
            }
            Ok(ModelError::Fatal(recovery(None, code.as_deref())))
        }
    }
}

fn infer_response(
    outcome: std::result::Result<ureq::Response, ureq::Error>,
    kind: TurnKind,
    on_stream: Option<StreamCallback<'_>>,
) -> std::result::Result<std::result::Result<ModelOutput, ModelError>, RetryErr> {
    let r = check_or_retry(outcome)?;
    // Some gateways omit the SSE media type. Only a validated terminal event authorizes an action.
    let (raw_media, stream_bytes) = match read_stream_with_lines(r, STREAM_LIMIT, |line| {
        emit_responses_stream_line(line, on_stream);
    }) {
        Ok(pair) => pair,
        Err(true) => {
            return Err(RetryErr::Http {
                retryable: true,
                retry_after: None,
                message: "ChatGPT response interrupted".into(),
            })
        }
        Err(false) => {
            return Ok(Err(ModelError::Fatal("ChatGPT response exceeds the size limit; no action accepted".into())))
        }
    };
    let media = match raw_media.as_deref() {
        Some("text/event-stream") => "text/event-stream",
        Some("application/json") => "application/json",
        Some("text/plain") => "text/plain",
        Some("application/octet-stream") => "application/octet-stream",
        Some("text/html") => "text/html",
        None => "missing Content-Type",
        _ => "other Content-Type",
    };
    let text = std::str::from_utf8(&stream_bytes).ok().map(|s| s.strip_prefix('\u{feff}').unwrap_or(s));
    if let Some(text) = text {
        let first = text.lines().find(|line| !line.is_empty()).unwrap_or("");
        let framed = ["event:", "data:", ":"].iter().any(|prefix| first.starts_with(prefix));
        if media == "text/event-stream" || (media != "text/html" && framed) {
            return match decode_sse(text, kind, TEXT_ONLY, INVALID) {
                Ok(v) => Ok(Ok(v)),
                Err(e) => map_stream_err(e, kind).map(Err),
            };
        }
        if let Ok(json) = serde_json::from_str::<Value>(text) {
            if let Some(error) = json.get("error").filter(|e| !e.is_null()) {
                let code = error.get("code").and_then(Value::as_str);
                if kind != TurnKind::Reporter && code == Some("context_length_exceeded") {
                    return Err(RetryErr::Compaction("ChatGPT HTTP 200: context overflow".into()));
                }
                if is_quota_plan_code(code) {
                    return Err(RetryErr::Quota(recovery(Some(200), code)));
                }
                return Ok(Err(ModelError::Fatal(recovery(Some(200), code))));
            }
            return Ok(Err(ModelError::Fatal(format!(
                "ChatGPT HTTP 200 ({media}): received JSON instead of a completed event stream"
            ))));
        }
    }
    Ok(Err(ModelError::Fatal(format!(
        "ChatGPT HTTP 200 ({media}): expected a ChatGPT event stream; no action accepted"
    ))))
}

fn infer_with_retry(
    initial_url: &str,
    mut build: impl FnMut(&str, bool) -> Result<ureq::Request>,
    bytes: &[u8],
    kind: TurnKind,
    retry_base: Duration,
    on_stream: Option<StreamCallback<'_>>,
) -> std::result::Result<ModelOutput, ModelError> {
    for attempt in 1..=HTTP_MAX_ATTEMPTS {
        wait_rate_limit(retry_base);
        let outcome = send_with_redirects(initial_url, Some(bytes), |url, send_auth| {
            Ok(build(url, send_auth)?.set("Content-Type", "application/json").set("Accept", "text/event-stream"))
        })
        .map_err(ModelError::Fatal)?;
        match infer_response(outcome, kind, on_stream) {
            Ok(outcome) => return outcome,
            Err(RetryErr::Compaction(msg)) => {
                if kind == TurnKind::Reporter {
                    return Err(ModelError::Fatal(msg));
                }
                return Err(ModelError::NeedsCompaction(msg));
            }
            Err(RetryErr::Quota(msg)) => return Err(ModelError::QuotaExceeded(msg)),
            Err(RetryErr::Http { retryable: true, retry_after, .. }) if attempt < HTTP_MAX_ATTEMPTS => {
                let delay = retry_delay(attempt, retry_after.as_deref(), retry_base);
                note_rate_limit(delay, retry_base);
                thread::sleep(delay);
            }
            Err(RetryErr::Http { message, .. }) => return Err(ModelError::Fatal(message)),
        }
    }
    unreachable!()
}

#[cfg(test)]
fn infer(req: ureq::Request, bytes: &[u8], kind: TurnKind) -> std::result::Result<Value, ModelError> {
    let outcome = req.set("Content-Type", "application/json").set("Accept", "text/event-stream").send_bytes(bytes);
    match infer_response(outcome, kind, None) {
        Ok(res) => res.map(|o| o.primary_action().clone()),
        Err(RetryErr::Compaction(msg)) if kind != TurnKind::Reporter => Err(ModelError::NeedsCompaction(msg)),
        Err(RetryErr::Quota(msg)) => Err(ModelError::QuotaExceeded(msg)),
        Err(RetryErr::Http { message, .. } | RetryErr::Compaction(message)) => Err(ModelError::Fatal(message)),
    }
}

#[cfg(test)]
fn decode(stream: &str, kind: TurnKind) -> std::result::Result<Value, ModelError> {
    match decode_sse(stream, kind, TEXT_ONLY, INVALID) {
        Ok(v) => Ok(v.primary_action().clone()),
        Err(e) => match map_stream_err(e, kind) {
            Ok(m) => Err(m),
            Err(RetryErr::Quota(m)) => Err(ModelError::QuotaExceeded(m)),
            Err(RetryErr::Compaction(m)) if kind != TurnKind::Reporter => Err(ModelError::NeedsCompaction(m)),
            Err(RetryErr::Http { message, .. } | RetryErr::Compaction(message)) => Err(ModelError::Fatal(message)),
        },
    }
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
        let bytes = body_with_cache(
            &model,
            None,
            "worker",
            "task-id",
            None,
            &history,
            TurnKind::Node,
            WireOptions::for_wire(WireProtocol::ChatGpt),
        )
        .unwrap();
        let req = authorize(http_agent(TIMEOUT).post(&format!("{url}/v1/responses")), TOKEN).unwrap();
        assert_eq!(infer(req, &bytes, TurnKind::Node).unwrap()["text"], "ok");
        let (line, headers, bytes) = server.join().unwrap();
        assert_eq!(line.trim_end(), "POST /v1/responses HTTP/1.1");
        assert!(headers.contains("Bearer offline-fixture") && headers.contains("text/event-stream"));
        let wire: Value = serde_json::from_slice(&bytes).unwrap();
        let expected_tools: Vec<Value> = super::super::tool_specs(TurnKind::Node)
            .into_iter()
            .map(|s| json!({"type":"function","name":s.name,"description":s.description,"parameters":s.parameters}))
            .collect();
        assert_eq!(
            wire,
            json!({"model":"z-first","input":[history[0],history[1],history[2]],"instructions":super::super::WORKER,"tools":expected_tools,"tool_choice":"required","parallel_tool_calls":true,"store":false,"stream":true})
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
    fn http_limits_redaction_redirect_and_retry_behave_safely() {
        // Redirects (307) and transient 503 retries succeed without exposing secrets.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for idx in 0..3 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut r = BufReader::new(&socket);
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let mut size = 0;
                for line in r.by_ref().lines() {
                    let l = line.unwrap();
                    if l.is_empty() {
                        break;
                    }
                    if let Some((n, v)) = l.split_once(':') {
                        if n.eq_ignore_ascii_case("Content-Length") {
                            size = v.trim().parse().unwrap();
                        }
                    }
                }
                let mut b = vec![0; size];
                r.read_exact(&mut b).unwrap();
                if idx == 0 {
                    write!(
                        socket,
                        "HTTP/1.1 503 Retry\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .unwrap();
                } else if idx == 1 {
                    write!(socket, "HTTP/1.1 307 Redir\r\nLocation: http://{addr}/redir\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                } else {
                    assert_eq!(first.trim_end(), "POST /redir HTTP/1.1");
                    let body = frames(&[terminal(ACTION)]);
                    write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
            }
        });
        let retried = infer_with_retry(
            &format!("http://{addr}/start"),
            |url, send_auth| auth_request(http_agent(TIMEOUT).post(url), TOKEN, send_auth),
            b"{}",
            TurnKind::Root,
            Duration::from_millis(1),
            None,
        )
        .unwrap();
        assert_eq!(retried.primary_action()["text"], "ok");
        server.join().unwrap();

        for (status, headers, payload, delay, expected) in [
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
        let sse_over_2m = format!(":{}\r\n\r\n{}", " ".repeat(LIMIT + 64), frames(&[terminal(ACTION)]));
        let (url, server) = serve(200, "Content-Type: text/event-stream", sse_over_2m, Duration::ZERO);
        assert_eq!(
            infer(authorize(http_agent(TIMEOUT).post(&url), TOKEN).unwrap(), b"{}", TurnKind::Root).unwrap()["text"],
            "ok"
        );
        server.join().unwrap();
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
        let err = decode(&frames(&[delta(ACTION),json!({"type":"response.failed","response":{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"reflected-secret"}}})]),TurnKind::Root).unwrap_err();
        assert!(matches!(err, ModelError::QuotaExceeded(_)));
        let err_str = err.to_string();
        assert!(err_str.contains("https://chatgpt.com/settings/usage") && !err_str.contains("reflected-secret"));
    }

    #[test]
    fn subscription_handles_commentary_and_retries_transient_quota_separately() {
        let commentary = json!({"type":"message","id":"c1","role":"assistant","phase":"commentary","status":"completed","content":[{"type":"output_text","text":"Looking at files"}]});
        let final_msg = json!({"type":"message","id":"msg","role":"assistant","phase":"final_answer","status":"completed","content":[{"type":"output_text","text":ACTION}]});
        let stream = frames(&[
            json!({"type":"response.created","response":{"id":"resp","status":"in_progress","output":[]}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"c1","role":"assistant","phase":"commentary","status":"in_progress","content":[]}}),
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"item_id":"c1","delta":"Looking at files"}),
            json!({"type":"response.output_item.done","output_index":0,"item":commentary}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"message","id":"msg","role":"assistant","phase":"final_answer","status":"in_progress","content":[]}}),
            json!({"type":"response.content_part.added","output_index":1,"content_index":0,"item_id":"msg","part":{"type":"output_text","text":""}}),
            json!({"type":"response.output_text.delta","output_index":1,"content_index":0,"item_id":"msg","delta":ACTION}),
            json!({"type":"response.output_text.done","output_index":1,"content_index":0,"item_id":"msg","text":ACTION}),
            json!({"type":"response.content_part.done","output_index":1,"content_index":0,"item_id":"msg","part":{"type":"output_text","text":ACTION}}),
            json!({"type":"response.output_item.done","output_index":1,"item":final_msg}),
            json!({"type":"response.completed","response":{"id":"resp","status":"completed","output":[commentary.clone(), final_msg]}}),
        ]);
        assert_eq!(decode(&stream, TurnKind::Node).unwrap(), json!({"op":"finish","text":"ok"}));

        let commentary_only = frames(&[
            json!({"type":"response.completed","response":{"id":"resp","status":"completed","output":[commentary]}}),
        ]);
        assert!(matches!(decode(&commentary_only, TurnKind::Node), Err(ModelError::InvalidAction(_))));

        let fc_stream = frames(&[
            json!({"type":"response.created","response":{"id":"resp","status":"in_progress","output":[]}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc1","call_id":"c1","name":"shell","arguments":"","status":"in_progress"}}),
            json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc1","delta":r#"{"command":"echo ok"}"#}),
            json!({"type":"response.function_call_arguments.done","output_index":0,"item_id":"fc1","arguments":r#"{"command":"echo ok"}"#}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc1","call_id":"c1","name":"shell","arguments":r#"{"command":"echo ok"}"#,"status":"completed"}}),
            json!({"type":"response.completed","response":{"id":"resp","status":"completed","output":[{"type":"function_call","id":"fc1","call_id":"c1","name":"shell","arguments":r#"{"command":"echo ok"}"#,"status":"completed"}]}}),
        ]);
        assert_eq!(decode(&fc_stream, TurnKind::Node).unwrap(), json!({"op":"shell","command":"echo ok"}));

        let (url, server) = serve(
            429,
            "Content-Type: application/json",
            json!({"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"secret-usage"}}).to_string(),
            Duration::ZERO,
        );
        let qerr = infer(authorize(http_agent(TIMEOUT).post(&url), TOKEN).unwrap(), b"{}", TurnKind::Root).unwrap_err();
        assert!(matches!(qerr, ModelError::QuotaExceeded(_)));
        assert!(!qerr.to_string().contains("secret-usage"));
        server.join().unwrap();
    }
}
