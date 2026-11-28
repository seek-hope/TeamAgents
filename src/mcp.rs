//! Model Context Protocol (MCP) server config (`mcp.json`, env-var refs only) and stdio/HTTP JSON-RPC 2.0 client.
use crate::{
    model::{is_local_host, valid_env_var_name},
    Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    env,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const CONFIG_LIMIT: u64 = 256 * 1024;
const MCP_TIMEOUT: Duration = Duration::from_secs(30);
const MCP_RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub(crate) enum McpServerSpec {
    Stdio {
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        env_vars: Vec<String>,
    },
    Http {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bearer_token_env: Option<String>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct McpConfig {
    #[serde(default)]
    pub servers: BTreeMap<String, McpServerSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct McpServerSummary {
    pub name: String,
    pub transport: String,
    pub target: String,
    pub env_refs: Vec<String>,
    pub ready: bool,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct McpToolInfo {
    pub server: String,
    pub name: String,
    pub description: String,
}

pub(crate) fn valid_server_name(name: &str) -> Result<()> {
    let bytes = name.as_bytes();
    let ok = (1..=64).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes[1..].iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.'));
    if !ok {
        return Err(format!("invalid MCP server name {name:?}: use 1..=64 lowercase [a-z0-9._-]"));
    }
    Ok(())
}

fn validate_spec(name: &str, spec: &McpServerSpec) -> Result<()> {
    valid_server_name(name)?;
    match spec {
        McpServerSpec::Stdio { command, env_vars, .. } => {
            if command.trim().is_empty() || command.contains('\0') {
                return Err(format!("MCP server {name:?} requires a nonempty command"));
            }
            for v in env_vars {
                valid_env_var_name(v)?;
            }
        }
        McpServerSpec::Http { url, bearer_token_env } => {
            if url.trim() != url || url.is_empty() {
                return Err(format!("invalid URL for MCP server {name:?}"));
            }
            let parsed = url::Url::parse(url).map_err(|_| format!("invalid URL for MCP server {name:?}"))?;
            if !parsed.username().is_empty() || parsed.password().is_some() || parsed.fragment().is_some() {
                return Err(format!("MCP server {name:?} URL cannot contain userinfo or fragment"));
            }
            let local = parsed.host_str().is_some_and(is_local_host);
            if parsed.scheme() != "https" && !(local && parsed.scheme() == "http") {
                return Err(format!("MCP server {name:?} requires HTTPS except on loopback"));
            }
            if let Some(var) = bearer_token_env.as_deref() {
                valid_env_var_name(var)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn config_path() -> Result<PathBuf> {
    if let Some(explicit) = env::var_os("TEAMAGENTS_MCP_FILE").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()) {
        return Ok(xdg.join("teamagents/mcp.json"));
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or("set HOME, XDG_CONFIG_HOME, or TEAMAGENTS_MCP_FILE to manage MCP servers")?;
    Ok(home.join(".config/teamagents/mcp.json"))
}

pub(crate) fn mcp_config_path_display() -> Option<String> {
    config_path().ok().map(|p| p.display().to_string())
}

pub(crate) fn load_config_at(path: &Path) -> Result<McpConfig> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(McpConfig::default()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let mut raw = Vec::new();
    file.take(CONFIG_LIMIT + 1).read_to_end(&mut raw).map_err(|e| format!("read {}: {e}", path.display()))?;
    if raw.len() as u64 > CONFIG_LIMIT {
        return Err(format!("{} exceeds 256 KiB", path.display()));
    }
    let cfg: McpConfig = serde_json::from_slice(&raw).map_err(|e| format!("invalid {}: {e}", path.display()))?;
    for (name, spec) in &cfg.servers {
        validate_spec(name, spec)?;
    }
    Ok(cfg)
}

pub(crate) fn load_config() -> Result<McpConfig> {
    if let Some(explicit) = env::var_os("TEAMAGENTS_MCP_FILE").filter(|v| !v.is_empty()) {
        return load_config_at(Path::new(&explicit));
    }
    match config_path() {
        Ok(path) => load_config_at(&path),
        Err(_) => Ok(McpConfig::default()),
    }
}

pub(crate) fn save_config_at(path: &Path, cfg: &McpConfig) -> Result<()> {
    for (name, spec) in &cfg.servers {
        validate_spec(name, spec)?;
    }
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or("invalid MCP config path")?;
    DirBuilder::new().recursive(true).mode(0o700).create(parent).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(cfg).map_err(|e| e.to_string())?;
    let tmp = parent.join(format!("mcp.json.{}.tmp", std::process::id()));
    let write_res = (|| -> Result<()> {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        f.write_all(&bytes).map_err(|e| e.to_string())?;
        f.write_all(b"\n").map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        fs::rename(&tmp, path).map_err(|e| format!("save {}: {e}", path.display()))?;
        Ok(())
    })();
    if write_res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write_res
}

pub(crate) fn upsert_server(name: &str, spec: McpServerSpec) -> Result<PathBuf> {
    validate_spec(name, &spec)?;
    let path = config_path()?;
    let mut cfg = load_config_at(&path)?;
    cfg.servers.insert(name.to_owned(), spec);
    save_config_at(&path, &cfg)?;
    Ok(path)
}

pub(crate) fn remove_server(name: &str) -> Result<PathBuf> {
    valid_server_name(name)?;
    let path = config_path()?;
    let mut cfg = load_config_at(&path)?;
    if cfg.servers.remove(name).is_none() {
        return Err(format!("unknown MCP server {name:?}"));
    }
    save_config_at(&path, &cfg)?;
    Ok(path)
}

pub(crate) fn configured_server_names() -> Vec<String> {
    load_config().map(|c| c.servers.keys().cloned().collect()).unwrap_or_default()
}

pub(crate) fn list_servers() -> Result<Vec<McpServerSummary>> {
    let cfg = load_config()?;
    let mut out = Vec::with_capacity(cfg.servers.len());
    for (name, spec) in cfg.servers {
        match spec {
            McpServerSpec::Stdio { command, args, env_vars } => {
                let target = if args.is_empty() { command } else { format!("{command} {}", args.join(" ")) };
                let missing: Vec<&str> = env_vars
                    .iter()
                    .filter(|k| env::var_os(k).filter(|v| !v.is_empty()).is_none())
                    .map(String::as_str)
                    .collect();
                let (ready, status) = if missing.is_empty() {
                    (true, "configured (stdio)".into())
                } else {
                    (false, format!("missing env: {}", missing.join(", ")))
                };
                out.push(McpServerSummary {
                    name,
                    transport: "stdio".into(),
                    target,
                    env_refs: env_vars,
                    ready,
                    status,
                });
            }
            McpServerSpec::Http { url, bearer_token_env } => {
                let (ready, status, env_refs) = match bearer_token_env {
                    Some(k) => {
                        let has = env::var_os(&k).filter(|v| !v.is_empty()).is_some();
                        if has {
                            (true, "ready (http)".into(), vec![k])
                        } else {
                            (false, format!("missing env: {k}"), vec![k])
                        }
                    }
                    None => (true, "ready (http)".into(), Vec::new()),
                };
                out.push(McpServerSummary { name, transport: "http".into(), target: url, env_refs, ready, status });
            }
        }
    }
    Ok(out)
}

struct ChildGuard(Option<Child>);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn extract_rpc_result(resp: Value) -> Result<Value> {
    if let Some(err) = resp.get("error").filter(|v| !v.is_null()) {
        let msg = err["message"].as_str().unwrap_or("MCP RPC error");
        let code = err["code"].as_i64().unwrap_or(-1);
        return Err(format!("MCP error ({code}): {msg}"));
    }
    Ok(resp.get("result").cloned().unwrap_or(Value::Null))
}

fn stdio_rpc(command: &str, args: &[String], env_vars: &[String], method: &str, params: Value) -> Result<Value> {
    let mut cmd = Command::new(command);
    cmd.env_clear()
        .env("PATH", env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // MCP servers are outside bwrap. Do not hand them the daemon's home directory
    // (which contains provider credentials); callers may opt in explicitly via env_vars.
    for var in env_vars {
        if let Some(val) = env::var_os(var) {
            cmd.env(var, val);
        }
    }
    let mut guard = ChildGuard(Some(cmd.spawn().map_err(|e| format!("spawn MCP server `{command}`: {e}"))?));
    let child = guard.0.as_mut().unwrap();
    let mut stdin = child.stdin.take().ok_or("MCP server stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("MCP server stdout unavailable")?;

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if line.len() > MCP_RESPONSE_LIMIT {
                        let _ = tx.send(Err("MCP response line exceeds 2 MiB".into()));
                        break;
                    }
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                        if tx.send(Ok(v)).is_err() {
                            break;
                        }
                    }
                }
            }
        }
    });

    let send_msg = |w: &mut std::process::ChildStdin, val: &Value| -> Result<()> {
        let mut s = serde_json::to_string(val).map_err(|e| e.to_string())?;
        s.push('\n');
        w.write_all(s.as_bytes()).map_err(|e| format!("write MCP stdin: {e}"))?;
        w.flush().map_err(|e| format!("flush MCP stdin: {e}"))
    };

    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "teamagents", "version": env!("CARGO_PKG_VERSION") }
        }
    });
    send_msg(&mut stdin, &init_req)?;

    let deadline = Instant::now() + MCP_TIMEOUT;
    let wait_id = |target_id: i64| -> Result<Value> {
        loop {
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                return Err("MCP server request timed out after 30s".into());
            }
            match rx.recv_timeout(rem) {
                Ok(Ok(val)) => {
                    if val.get("id").and_then(Value::as_i64) == Some(target_id) {
                        return extract_rpc_result(val);
                    }
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err("MCP server closed stdout or timed out before responding".into()),
            }
        }
    };

    wait_id(1)?;
    send_msg(&mut stdin, &json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}))?;
    send_msg(&mut stdin, &json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}))?;
    wait_id(2)
}

fn parse_http_rpc_response(resp: ureq::Response, target_id: Option<i64>) -> Result<(Option<String>, Option<Value>)> {
    let session_id = resp.header("Mcp-Session-Id").or_else(|| resp.header("mcp-session-id")).map(str::to_owned);
    let content_type = resp.content_type().to_ascii_lowercase();
    let mut raw = Vec::new();
    resp.into_reader()
        .take(MCP_RESPONSE_LIMIT as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|e| format!("read MCP HTTP response: {e}"))?;
    if raw.len() > MCP_RESPONSE_LIMIT {
        return Err("MCP HTTP response exceeds 2 MiB".into());
    }
    if raw.is_empty() {
        return Ok((session_id, None));
    }
    let text = String::from_utf8(raw).map_err(|_| "MCP HTTP response is not valid UTF-8")?;
    if content_type.contains("text/event-stream") || text.starts_with("event:") || text.starts_with("data:") {
        let mut last = None;
        for line in text.lines() {
            if let Some(data) = line.strip_prefix("data:").map(str::trim) {
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    if target_id.is_some_and(|id| v.get("id").and_then(Value::as_i64) == Some(id)) {
                        return Ok((session_id, Some(extract_rpc_result(v)?)));
                    }
                    last = Some(v);
                }
            }
        }
        return match last {
            Some(v) => Ok((session_id, Some(extract_rpc_result(v)?))),
            None => Ok((session_id, None)),
        };
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("invalid MCP JSON response: {e}"))?;
    Ok((session_id, Some(extract_rpc_result(v)?)))
}

fn http_rpc(url: &str, bearer_token_env: Option<&str>, method: &str, params: Value) -> Result<Value> {
    let token = if let Some(var) = bearer_token_env {
        Some(
            env::var(var)
                .ok()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("MCP bearer token env {var} is not set"))?,
        )
    } else {
        None
    };
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(MCP_TIMEOUT)
        .redirects(3)
        .build();

    let post = |session_id: Option<&str>, body: &Value| -> Result<ureq::Response> {
        let mut req = agent
            .post(url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream");
        if let Some(tok) = token.as_deref() {
            req = req.set("Authorization", &format!("Bearer {tok}"));
        }
        if let Some(sid) = session_id {
            req = req.set("Mcp-Session-Id", sid);
        }
        req.send_json(body.clone()).map_err(|e| format!("MCP HTTP request failed: {e}"))
    };

    let init_body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "teamagents", "version": env!("CARGO_PKG_VERSION") }
        }
    });
    let (sid, _) = parse_http_rpc_response(post(None, &init_body)?, Some(1))?;
    let _ = post(sid.as_deref(), &json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}));
    let (_, result) = parse_http_rpc_response(
        post(sid.as_deref(), &json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}))?,
        Some(2),
    )?;
    result.ok_or_else(|| "MCP HTTP server returned an empty response".into())
}

fn call_server_rpc(spec: &McpServerSpec, method: &str, params: Value) -> Result<Value> {
    match spec {
        McpServerSpec::Stdio { command, args, env_vars } => stdio_rpc(command, args, env_vars, method, params),
        McpServerSpec::Http { url, bearer_token_env } => http_rpc(url, bearer_token_env.as_deref(), method, params),
    }
}

pub(crate) fn list_tools(server_filter: Option<&str>) -> Result<Vec<McpToolInfo>> {
    let cfg = load_config()?;
    if cfg.servers.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    if let Some(filter) = server_filter {
        let Some(spec) = cfg.servers.get(filter) else {
            return Err(format!("unknown MCP server {filter:?}"));
        };
        let res = call_server_rpc(spec, "tools/list", json!({}))?;
        if let Some(arr) = res.get("tools").and_then(Value::as_array) {
            for t in arr {
                out.push(McpToolInfo {
                    server: filter.to_owned(),
                    name: t["name"].as_str().unwrap_or("?").to_owned(),
                    description: t["description"].as_str().unwrap_or("").to_owned(),
                });
            }
        }
        return Ok(out);
    }

    for (name, spec) in cfg.servers {
        match call_server_rpc(&spec, "tools/list", json!({})) {
            Ok(res) => {
                if let Some(arr) = res.get("tools").and_then(Value::as_array) {
                    for t in arr {
                        out.push(McpToolInfo {
                            server: name.clone(),
                            name: t["name"].as_str().unwrap_or("?").to_owned(),
                            description: t["description"].as_str().unwrap_or("").to_owned(),
                        });
                    }
                }
            }
            Err(e) => {
                out.push(McpToolInfo { server: name, name: "(error)".into(), description: e });
            }
        }
    }
    Ok(out)
}

pub(crate) fn list_tools_text(server_filter: Option<&str>) -> Result<String> {
    let tools = list_tools(server_filter)?;
    if tools.is_empty() {
        return Ok("No MCP tools available (configure servers with `teamagents mcp add`).".into());
    }
    let lines: Vec<String> = tools
        .into_iter()
        .map(|t| {
            if t.description.is_empty() {
                format!("{}/{}", t.server, t.name)
            } else {
                format!("{}/{}: {}", t.server, t.name, t.description)
            }
        })
        .collect();
    Ok(lines.join("\n"))
}

pub(crate) fn call_tool_text(server: &str, tool: &str, arguments: Option<&Value>) -> Result<String> {
    if tool.trim().is_empty() {
        return Err("mcp_call requires nonempty tool name".into());
    }
    let cfg = load_config()?;
    let Some(spec) = cfg.servers.get(server) else {
        return Err(format!("unknown MCP server {server:?}"));
    };
    let args_val = arguments.cloned().unwrap_or_else(|| json!({}));
    let res = call_server_rpc(spec, "tools/call", json!({"name": tool, "arguments": args_val}))?;
    let is_err = res.get("isError").and_then(Value::as_bool) == Some(true);
    let rendered = if let Some(arr) = res.get("content").and_then(Value::as_array) {
        let parts: Vec<String> = arr
            .iter()
            .map(|item| item.get("text").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| item.to_string()))
            .collect();
        parts.join("\n")
    } else {
        res.to_string()
    };
    if is_err {
        Err(format!("MCP tool `{server}/{tool}` failed: {rendered}"))
    } else {
        Ok(rendered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, sync::Mutex};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn mcp_config_enforces_env_refs_and_https_boundaries() {
        assert!(validate_spec(
            "bad-url",
            &McpServerSpec::Http { url: "http://mcp.example.com/rpc".into(), bearer_token_env: None }
        )
        .is_err());
        assert!(validate_spec(
            "bad-secret",
            &McpServerSpec::Http {
                url: "https://mcp.example.com/rpc".into(),
                bearer_token_env: Some("sk-literal-secret".into()),
            }
        )
        .is_err());
        assert!(validate_spec(
            "good-local",
            &McpServerSpec::Http { url: "http://127.0.0.1:9000/mcp".into(), bearer_token_env: None }
        )
        .is_ok());
    }

    #[test]
    fn mcp_stdio_and_http_transports_execute_tools_end_to_end() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = env::temp_dir().join(format!("teamagents-mcp-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let cfg_path = dir.join("mcp.json");
        let prev = env::var_os("TEAMAGENTS_MCP_FILE");
        env::set_var("TEAMAGENTS_MCP_FILE", &cfg_path);

        // Start a minimal loopback HTTP MCP server handling initialize -> notification -> call.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let http_url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            for _ in 0..6 {
                let Ok((mut stream, _)) = listener.accept() else { break };
                let mut reader = BufReader::new(&stream);
                let mut content_len = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" || line == "\n" {
                        break;
                    }
                    if let Some((k, v)) = line.split_once(':') {
                        if k.eq_ignore_ascii_case("content-length") {
                            content_len = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let mut body = vec![0u8; content_len];
                if content_len > 0 {
                    let _ = reader.read_exact(&mut body);
                }
                drop(reader);
                let v: Value = serde_json::from_slice(&body).unwrap_or_default();
                let resp = match v.get("method").and_then(Value::as_str) {
                    Some("initialize") => json!({"jsonrpc":"2.0","id":v["id"],"result":{"protocolVersion":"2024-11-05"}}).to_string(),
                    Some("notifications/initialized") => String::new(),
                    Some("tools/list") => json!({"jsonrpc":"2.0","id":v["id"],"result":{"tools":[{"name":"ping","description":"Ping tool"}]}}).to_string(),
                    Some("tools/call") => json!({"jsonrpc":"2.0","id":v["id"],"result":{"content":[{"type":"text","text":format!("pong:{}", v["params"]["arguments"]["msg"].as_str().unwrap_or(""))}]}}).to_string(),
                    _ => String::new(),
                };
                if resp.is_empty() {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 204 No Content\r\nMcp-Session-Id: sess-1\r\nConnection: close\r\n\r\n"
                    );
                } else {
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: sess-1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp}", resp.len());
                }
            }
        });

        upsert_server("http_mock", McpServerSpec::Http { url: http_url, bearer_token_env: None }).unwrap();
        let stdio_script = r#"while IFS= read -r line; do case "$line" in *'"initialize"'*) printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05"}}\n';; *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"Echo tool"}]}}\n';; *'"tools/call"'*) printf '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"stdio-ok"}]}}\n';; esac; done"#;
        upsert_server(
            "stdio_mock",
            McpServerSpec::Stdio {
                command: "/bin/sh".into(),
                args: vec!["-c".into(), stdio_script.into()],
                env_vars: vec![],
            },
        )
        .unwrap();

        let listed = list_tools_text(None).unwrap();
        assert!(
            listed.contains("http_mock/ping: Ping tool") && listed.contains("stdio_mock/echo: Echo tool"),
            "{listed}"
        );
        assert_eq!(call_tool_text("http_mock", "ping", Some(&json!({"msg": "hi"}))).unwrap(), "pong:hi");
        assert_eq!(call_tool_text("stdio_mock", "echo", None).unwrap(), "stdio-ok");

        remove_server("stdio_mock").unwrap();
        assert_eq!(list_servers().unwrap().len(), 1);
        drop(handle);

        match prev {
            Some(v) => env::set_var("TEAMAGENTS_MCP_FILE", v),
            None => env::remove_var("TEAMAGENTS_MCP_FILE"),
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
