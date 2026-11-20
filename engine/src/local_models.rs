//! User-only management of an existing llama.cpp router. No inference, process spawning or file operations.
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, io::Read, time::Duration};
use teamagents_core::models::{ModelProfile, UserConfig};

const MAX_BYTES: u64 = 8 * 1024 * 1024;
const UNCERTAIN: &str = "router operation outcome unknown; inspect the list before another operation";

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-:/".contains(&b))
}

pub(crate) fn root(profile: &ModelProfile) -> Result<String, String> {
    if !matches!(profile.protocol.as_str(), "openai" | "chat/completions") || !valid_id(&profile.model) {
        return Err("local router requires a Chat Completions profile with a valid model ID".into());
    }
    let url = url::Url::parse(profile.base_url.as_deref().unwrap_or("")).map_err(|_| "invalid local router URL")?;
    let local = matches!(url.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        || matches!(url.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback());
    if !local
        || url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "/" | "/v1" | "/v1/")
    {
        return Err(
            "local router requires an HTTP loopback IP with path / or /v1, without credentials, query or fragment"
                .into(),
        );
    }
    Ok(url.origin().ascii_serialization())
}

fn body(response: ureq::Response) -> Result<Value, String> {
    if response.status() != 200 {
        return Err(format!("router HTTP {}", response.status()));
    }
    let mut bytes = Vec::new();
    response.into_reader().take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|_| "unreadable router reply")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("router reply exceeds 8 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid router JSON".into())
}

#[derive(Deserialize)]
struct Row {
    id: String,
    status: Status,
}
#[derive(Deserialize)]
struct Status {
    value: String,
    #[serde(default)]
    failed: bool,
}

fn inventory(reply: Value) -> Result<BTreeMap<String, String>, String> {
    let rows = reply
        .get("data")
        .and_then(Value::as_array)
        .filter(|rows| rows.len() <= 4096)
        .ok_or("invalid router directory")?;
    let mut out = BTreeMap::new();
    for row in rows {
        let Row { id, status } = Row::deserialize(row).map_err(|_| "invalid router model row")?;
        if !valid_id(&id)
            || !matches!(
                status.value.as_str(),
                "loaded" | "unloaded" | "loading" | "unloading" | "downloading" | "sleeping"
            )
        {
            return Err("unsupported model ID or router state".into());
        }
        // The router reports a failed start as unloaded + failed. Do not treat any other
        // combination as an empty slot when deciding whether a peer could be evicted.
        if status.failed && status.value != "unloaded" {
            return Err("inconsistent router model state".into());
        }
        let state = if status.failed { "failed".into() } else { status.value };
        if out.insert(id, state).is_some() {
            return Err("duplicate router model ID".into());
        }
    }
    Ok(out)
}

/// An explicit user command, not a model tool. Never resolve a wire alias or a discovered profile here.
pub fn command(cfg: &UserConfig, verb: &str, key: &str, confirmed: bool) -> Result<Value, String> {
    if !matches!((verb, confirmed), ("list", false) | ("load" | "unload", true)) {
        return Err("choose list or explicitly confirm load/unload with --yes".into());
    }
    if !cfg.local_router_profiles.iter().any(|name| name == key) {
        return Err("profile is not enabled for local router management".into());
    }
    let profile = cfg.models.get(key).ok_or("missing local router profile")?;
    let base = root(profile)?;
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .max_idle_connections(0)
        .try_proxy_from_env(false)
        .timeout(Duration::from_secs(profile.timeout.clamp(1, 120) as u64))
        .build();
    let token = profile
        .api_key_env
        .as_ref()
        .map(|name| std::env::var(name).map_err(|_| "missing local router credential"))
        .transpose()?;
    if token.as_ref().is_some_and(|s| s.is_empty() || !s.bytes().all(|b| (0x21..=0x7e).contains(&b))) {
        return Err("invalid local router credential".into());
    }
    let request = |method, path| {
        let r = agent.request(method, &format!("{base}{path}")).set("accept", "application/json");
        match &token {
            Some(t) => r.set("authorization", &format!("Bearer {t}")),
            None => r,
        }
    };
    let listing = request("GET", "/models").call().map_err(|e| match e {
        ureq::Error::Status(n, _) => format!("router list HTTP {n}"),
        _ => "router list unavailable".into(),
    })?;
    let models = inventory(body(listing)?)?;
    if verb == "list" {
        return Ok(json!({"profile":key,"models":models}));
    }
    let model = &profile.model;
    let state = models.get(model).ok_or("model is not present; no router operation sent")?;
    match (verb, state.as_str()) {
        ("load", "unloaded") if models.iter().all(|(id, s)| id == model || s == "unloaded" || s == "failed") => {}
        ("unload", "loaded" | "sleeping") => {}
        _ => return Err("model state or another resident model prevents this operation; inspect the list".into()),
    }
    let path = if verb == "load" { "/models/load" } else { "/models/unload" };
    // No redirect, retry or fallback after dispatch, even if the router closed before acknowledging.
    let response = request("POST", path).send_json(json!({"model":model})).map_err(|_| UNCERTAIN)?;
    let reply = body(response).map_err(|_| UNCERTAIN)?;
    if reply["success"] != true {
        return Err(UNCERTAIN.into());
    }
    Ok(json!({"profile":key,"model":model,"action":verb,"acknowledged":true,"prior_state":state}))
}
