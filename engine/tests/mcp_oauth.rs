//! D-412 end to end: `mcp login` against a fake authorization server, then the real MCP client using the
//! credential it stored.
//!
//! Everything here is a child process with its own `XDG_CONFIG_HOME`/`XDG_STATE_HOME` (AGENTS.md's rule for
//! suites that isolate the environment), so the credential file cannot touch the developer's own config. The fake
//! server plays both roles a real deployment splits — the authorization server (metadata, dynamic client
//! registration, `/authorize`, `/token`) and the protected MCP endpoint (`/mcp`, which answers 401 without a
//! bearer) — and it checks the two things a sign-in must get right: the PKCE verifier hashes to the challenge that
//! travelled with the authorization URL, and the redirect the client asks for is the one it registered.
//!
//! The test acts as the browser: it reads the authorization URL the CLI printed, fetches it, takes the `Location`
//! and pastes that back, which is exactly the `--paste` path a user over SSH follows.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::TempRoot;
use serde_json::{json, Value};

/// What the fake authorization server remembers between requests.
#[derive(Default)]
struct Server {
    base: String,
    registered_redirect: Option<String>,
    /// authorization code -> PKCE challenge the client sent with it
    challenges: Vec<(String, String)>,
    verifier_mismatches: usize,
    refresh_calls: usize,
    code_exchanges: usize,
    /// tokens the MCP endpoint accepts right now
    valid_tokens: Vec<String>,
    refresh_tokens: Vec<String>,
    issued: usize,
    /// what the code exchange should say about the token's lifetime
    expires_in: u64,
    mcp_calls: usize,
}

fn start_server() -> (u16, Arc<Mutex<Server>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the fake authorization server");
    let port = listener.local_addr().unwrap().port();
    let state = Arc::new(Mutex::new(Server {
        base: format!("http://127.0.0.1:{port}"),
        expires_in: 3600,
        ..Default::default()
    }));
    let shared = Arc::clone(&state);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (method, target, headers, body) = read_request(&mut stream);
            let state = Arc::clone(&shared);
            let mut state = state.lock().unwrap();
            match (method.as_str(), target.split('?').next().unwrap_or("")) {
                ("GET", "/.well-known/oauth-protected-resource") => {
                    let body = json!({
                        "resource": format!("{}/mcp", state.base),
                        "authorization_servers": [state.base.clone()],
                    });
                    respond_json(&mut stream, 200, &body);
                }
                ("GET", "/.well-known/oauth-authorization-server") => {
                    let body = json!({
                        "issuer": state.base,
                        "authorization_endpoint": format!("{}/authorize", state.base),
                        "token_endpoint": format!("{}/token", state.base),
                        "registration_endpoint": format!("{}/register", state.base),
                        "scopes_supported": ["mcp"],
                    });
                    respond_json(&mut stream, 200, &body);
                }
                ("POST", "/register") => {
                    let registered: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    state.registered_redirect = registered["redirect_uris"][0].as_str().map(str::to_string);
                    respond_json(&mut stream, 201, &json!({"client_id": "ta-test-client"}));
                }
                ("GET", "/authorize") => {
                    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
                    let params = parse_query(query);
                    let Some(redirect_uri) = params.get("redirect_uri").cloned() else {
                        respond_json(&mut stream, 400, &json!({"error": "no redirect_uri"}));
                        continue;
                    };
                    let code = format!("code-{}", state.challenges.len());
                    if let Some(challenge) = params.get("code_challenge") {
                        state.challenges.push((code.clone(), challenge.clone()));
                    }
                    assert!(params.contains_key("state"), "the authorization URL carries no state");
                    let location = format!(
                        "{redirect_uri}?code={code}&state={}",
                        params.get("state").cloned().unwrap_or_default()
                    );
                    let response = format!(
                        "HTTP/1.1 302 Found\r\nlocation: {location}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                ("POST", "/token") => {
                    let params = parse_query(&body);
                    let grant = params.get("grant_type").cloned().unwrap_or_default();
                    if grant == "authorization_code" {
                        let code = params.get("code").cloned().unwrap_or_default();
                        let verifier = params.get("code_verifier").cloned().unwrap_or_default();
                        match state.challenges.iter().find(|(issued, _)| issued == &code) {
                            Some((_, challenge)) => {
                                // S256: base64url(sha256(verifier)) must equal the challenge that travelled
                                use base64::Engine as _;
                                use sha2::{Digest, Sha256};
                                let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD
                                    .encode(Sha256::digest(verifier.as_bytes()));
                                if &computed != challenge {
                                    state.verifier_mismatches += 1;
                                }
                            }
                            None => {
                                state.verifier_mismatches += 1;
                            }
                        }
                        state.code_exchanges += 1;
                        let access = issue(&mut state, "access");
                        let refresh = issue(&mut state, "refresh");
                        let lifetime = state.expires_in;
                        respond_json(
                            &mut stream,
                            200,
                            &json!({
                                "access_token": access,
                                "refresh_token": refresh,
                                "token_type": "Bearer",
                                "expires_in": lifetime,
                                "scope": "mcp",
                            }),
                        );
                    } else if grant == "refresh_token" {
                        state.refresh_calls += 1;
                        let refresh = params.get("refresh_token").cloned().unwrap_or_default();
                        if !state.refresh_tokens.contains(&refresh) {
                            respond_json(&mut stream, 400, &json!({"error": "invalid_grant"}));
                            continue;
                        }
                        let access = issue(&mut state, "access");
                        respond_json(
                            &mut stream,
                            200,
                            &json!({"access_token": access, "token_type": "Bearer", "expires_in": 3600}),
                        );
                    } else {
                        respond_json(&mut stream, 400, &json!({"error": "unsupported_grant_type"}));
                    }
                }
                ("POST", "/mcp") => {
                    let bearer = headers
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                        .and_then(|(_, value)| value.strip_prefix("Bearer ").map(str::to_string));
                    if !bearer.as_deref().is_some_and(|token| state.valid_tokens.contains(&token.to_string())) {
                        let response = format!(
                            "HTTP/1.1 401 Unauthorized\r\nwww-authenticate: Bearer resource_metadata=\
                             \"{}/.well-known/oauth-protected-resource\"\r\ncontent-length: 0\r\n\
                             connection: close\r\n\r\n",
                            state.base
                        );
                        let _ = stream.write_all(response.as_bytes());
                        continue;
                    }
                    state.mcp_calls += 1;
                    let call: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    let method = call["method"].as_str().unwrap_or("");
                    match method {
                        "initialize" => respond_json(
                            &mut stream,
                            200,
                            &json!({"jsonrpc": "2.0", "id": call["id"],
                                    "result": {"protocolVersion": "2025-06-18", "capabilities": {},
                                               "serverInfo": {"name": "fake", "version": "1"}}}),
                        ),
                        "tools/list" => respond_json(
                            &mut stream,
                            200,
                            &json!({"jsonrpc": "2.0", "id": call["id"],
                                    "result": {"tools": [{"name": "probe_ping",
                                                          "description": "answers pong",
                                                          "inputSchema": {"type": "object"}}]}}),
                        ),
                        "tools/call" => respond_json(
                            &mut stream,
                            200,
                            &json!({"jsonrpc": "2.0", "id": call["id"],
                                    "result": {"content": [{"type": "text", "text": "pong"}]}}),
                        ),
                        // a notification: accepted with no body
                        _ => {
                            let response = "HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
                            let _ = stream.write_all(response.as_bytes());
                        }
                    }
                }
                // the client opens a GET push stream; 405 is the documented "no push here"
                ("GET", _) => {
                    let response = "HTTP/1.1 405 Method Not Allowed\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
                    let _ = stream.write_all(response.as_bytes());
                }
                _ => {
                    let response = "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        }
    });
    (port, state)
}

fn issue(state: &mut Server, prefix: &str) -> String {
    state.issued += 1;
    let token = format!("{prefix}-{}", state.issued);
    match prefix {
        "access" => state.valid_tokens.push(token.clone()),
        _ => state.refresh_tokens.push(token.clone()),
    }
    token
}

fn read_request(stream: &mut std::net::TcpStream) -> (String, String, Vec<(String, String)>, String) {
    use std::io::Read;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut reader = BufReader::new(stream.try_clone().expect("clone the connection"));
    let mut start = String::new();
    let _ = reader.read_line(&mut start);
    let mut parts = start.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let mut headers = Vec::new();
    let mut length = 0_usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim().to_string();
            if name.eq_ignore_ascii_case("content-length") {
                length = value.parse().unwrap_or(0);
            }
            headers.push((name.to_string(), value));
        }
    }
    let mut body = vec![0_u8; length];
    if length > 0 {
        let _ = reader.read_exact(&mut body);
    }
    (method, target, headers, String::from_utf8_lossy(&body).into_owned())
}

fn parse_query(query: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        out.insert(percent_decode(key), percent_decode(value));
    }
    out
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn respond_json(stream: &mut std::net::TcpStream, status: u16, body: &Value) {
    let body = body.to_string();
    let reason = match status {
        200 => "OK",
        201 => "Created",
        _ => "Bad Request",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

/// The config a test signs in with: one OAuth HTTP binding and a model stub the loader accepts.
fn write_config(root: &TempRoot, port: u16, extra: &str) {
    let dir = root.join("config/teamagents");
    std::fs::create_dir_all(&dir).expect("config dir");
    std::fs::write(
        dir.join("config.toml"),
        format!(
            "[models.leader_main]\nprovider = \"compatible\"\nmodel = \"x\"\napi_key_env = \"TA_OAUTH_KEY\"\n\n\
             [tools.svc]\nkind = \"mcp\"\nmcp_transport = \"http\"\nurl = \"http://127.0.0.1:{port}/mcp\"\n\
             oauth = true\n{extra}"
        ),
    )
    .expect("write the config");
}

fn run(root: &TempRoot, args: &[&str]) -> (Option<i32>, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .args(args)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("TA_OAUTH_KEY", "unused")
        .output()
        .expect("run the cli");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Run `mcp login --paste`, act as the browser, and return what the command printed.
fn login(root: &TempRoot) -> (Option<i32>, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .args(["mcp", "login", "--name", "svc", "--paste", "--no-browser"])
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mcp login");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let mut printed = String::new();
    let mut authorize_url = String::new();
    loop {
        let mut line = String::new();
        if stdout.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        printed.push_str(&line);
        if let Some(url) = line.trim().strip_prefix("http://") {
            if url.contains("/authorize?") {
                authorize_url = line.trim().to_string();
                break;
            }
        }
    }
    assert!(!authorize_url.is_empty(), "the command printed no authorization URL: {printed}");
    // the browser: fetch the authorization URL without following the redirect, then paste what it tried to load
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let response = agent.get(&authorize_url).call();
    let location = match response {
        Ok(response) => response.header("location").map(str::to_string),
        Err(ureq::Error::Status(302, response)) => response.header("location").map(str::to_string),
        Err(error) => panic!("the authorization URL did not redirect: {error}"),
    }
    .expect("the authorization response carries no location");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{location}").expect("paste the redirected URL");
    }
    let status = child.wait().expect("mcp login exits");
    let mut rest = String::new();
    let _ = stdout.read_to_string(&mut rest);
    printed.push_str(&rest);
    let mut errors = String::new();
    if let Some(mut handle) = child.stderr.take() {
        let _ = handle.read_to_string(&mut errors);
    }
    (status.code(), printed, errors)
}

#[test]
fn an_oauth_binding_signs_in_lists_its_tools_and_logs_out() {
    let root = TempRoot::new("mcp-oauth");
    let (port, state) = start_server();
    write_config(&root, port, "");
    let (code, printed, errors) = login(&root);
    assert_eq!(code, Some(0), "{printed}{errors}");
    assert!(printed.contains("signed in svc"), "{printed}");
    // nothing secret is printed, and the verifier is not on the wire that the user sees
    assert!(!printed.contains("code_verifier"), "the verifier reached the printed URL: {printed}");
    assert!(!printed.contains("access-"), "an access token was printed: {printed}");

    let store = root.join("config/teamagents/mcp-auth.json");
    assert!(store.exists(), "the credential file was not written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&store).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the credential file is not private");
    }
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(&store).unwrap()).unwrap();
    assert!(stored["services"]["svc"]["access_token"].as_str().is_some(), "{stored}");

    let state_now = state.lock().unwrap();
    // the redirect the client registered is the loopback one it really listened on
    let registered = state_now.registered_redirect.clone().expect("no dynamic registration happened");
    assert!(registered.starts_with("http://127.0.0.1:") && registered.ends_with("/callback"), "{registered}");
    assert_eq!(state_now.verifier_mismatches, 0, "the PKCE verifier did not match the challenge");
    assert_eq!(state_now.code_exchanges, 1);
    drop(state_now);

    // `mcp list` shows the state and the service's tools, authenticated with the stored token
    let (code, out, err) = run(&root, &["mcp", "list"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("svc: oauth — signed in"), "{out}");
    assert!(out.contains("probe_ping"), "{out}{err}");

    // and a signed-out service is not authenticated any more
    let (code, out, err) = run(&root, &["mcp", "logout", "--name", "svc"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("signed out svc"), "{out}");
    let (code, out, err) = run(&root, &["mcp", "list"]);
    assert_eq!(code, Some(1), "a service that cannot be listed must exit non-zero: {out}{err}");
    assert!(out.contains("svc: oauth — not signed in"), "{out}");
    assert!(out.contains("mcp login"), "the refusal must name the lever: {out}{err}");
}

#[test]
fn an_expired_credential_is_refreshed_without_asking_the_user_again() {
    let root = TempRoot::new("mcp-oauth-refresh");
    let (port, state) = start_server();
    // the authorization server hands out a token that is already past its lifetime: the next use must refresh
    state.lock().unwrap().expires_in = 0;
    write_config(&root, port, "");
    let (code, printed, errors) = login(&root);
    assert_eq!(code, Some(0), "{printed}{errors}");
    let (code, out, err) = run(&root, &["mcp", "list"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("svc: oauth — expired (will refresh)"), "{out}");
    assert!(out.contains("probe_ping"), "the refreshed token did not authenticate: {out}{err}");
    let state_now = state.lock().unwrap();
    assert!(state_now.refresh_calls >= 1, "the client did not refresh an expired token");
    assert_eq!(state_now.verifier_mismatches, 0);
    drop(state_now);
    // the refreshed token is the stored one now, so the next command does not refresh again
    let refreshes = state.lock().unwrap().refresh_calls;
    let (code, out, err) = run(&root, &["mcp", "list"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert_eq!(state.lock().unwrap().refresh_calls, refreshes, "a fresh token was refreshed again");
}

#[test]
fn oauth_and_a_bearer_token_are_one_credential_too_many() {
    let root = TempRoot::new("mcp-oauth-both");
    let (port, _state) = start_server();
    let dir = root.join("config/teamagents");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        format!(
            "[models.leader_main]\nprovider = \"compatible\"\nmodel = \"x\"\napi_key_env = \"TA_OAUTH_KEY\"\n\n\
             [tools.svc]\nkind = \"mcp\"\nmcp_transport = \"http\"\nurl = \"http://127.0.0.1:{port}/mcp\"\n\
             oauth = true\nbearer_token_env_var = \"TA_OAUTH_BEARER\"\n"
        ),
    )
    .unwrap();
    let (code, out, err) = run(&root, &["mcp", "list"]);
    assert_eq!(code, Some(2), "{out}{err}");
    assert!(err.contains("two credentials for one endpoint"), "{err}");
}

#[test]
fn mcp_add_writes_an_oauth_binding_and_refuses_a_stdio_one() {
    let root = TempRoot::new("mcp-oauth-add");
    std::fs::create_dir_all(root.join("config/teamagents")).unwrap();
    let config = root.join("config/teamagents/config.toml");
    std::fs::write(&config, "[models.m]\nprovider = \"compatible\"\nmodel = \"x\"\n").unwrap();
    let (code, out, err) = run(
        &root,
        &[
            "mcp",
            "add",
            "--name",
            "remote",
            "--transport",
            "http",
            "--url",
            "https://mcp.example/mcp",
            "--oauth",
            "--oauth-client-id",
            "abc",
        ],
    );
    assert_eq!(code, Some(0), "{out}{err}");
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("oauth = true") && text.contains("oauth_client_id = \"abc\""), "{text}");
    // an OAuth flag on a stdio binding is refused before the file is touched
    let (code, _out, err) = run(&root, &["mcp", "add", "--name", "local", "--command", "/bin/echo", "--oauth"]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("needs --transport http"), "{err}");
    assert!(!std::fs::read_to_string(&config).unwrap().contains("[tools.local]"));
}
