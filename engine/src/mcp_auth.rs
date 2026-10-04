//! MCP OAuth sign-in (D-412): the authorization-code flow a remote MCP server with no token to paste needs.
//!
//! Portfolio: the user has a config binding for an HTTP MCP service that answers 401 until the client presents a
//! bearer token it can only get from the server's own authorization server. Pi's `pi mcp login` does this; this
//! module is the same shape, with the parts this product insists on:
//!
//! * **Nothing secret in the repository, in a prompt, in an event or in a log.** Tokens live in one file beside
//!   the config (`<xdg_config_home>/<APP>/mcp-auth.json`), mode 0600, written atomically. The file holds tokens,
//!   never a password: the client secret is read from the environment variable the config names.
//! * **PKCE (S256) always**, with a `state` the callback must echo. The verifier never leaves this process — it is
//!   sent to the *token* endpoint only, never to the authorization endpoint.
//! * **Fail-closed and explanatory.** A server without discoverable metadata, a callback whose `state` does not
//!   match, a token the server refuses: each is a refusal naming what to do next.
//!
//! The flow, end to end: [`discover`] the authorization server, [`register`] a client if the config does not name
//! one, [`begin`] a pending login (PKCE + `state` + a loopback listener), send the user to the printed URL, take
//! the redirected code in [`PendingLogin::wait`] (or a pasted URL in [`PendingLogin::paste`]), and exchange it in
//! [`PendingLogin::complete`]. [`Store`] is the credential file; [`Store::access_token`] is what the MCP client
//! calls before a request, and it refreshes an expired token when the server gave it a refresh token.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The window before expiry in which a token is treated as expired (a request that starts now must not fail
/// halfway because the token dies).
const EXPIRY_SKEW_SECS: f64 = 60.0;
/// How long the callback listener waits for the browser by default.
pub const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

// ------------------------------------------------------------------ the store

/// One service's credential. Everything the client needs to authenticate again later is here, so a restart never
/// needs the user's browser.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Unix seconds at which `access_token` stops being valid, when the server said. **Integer seconds on
    /// purpose**: this file is JSON, and an `f64` here loses its last bit on a round trip (measured: a value
    /// written as `…8308105` read back as `…8308103`), which is a flake in every test that compares the file and
    /// no useful precision for an expiry the client only compares against a 60-second skew.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// The token endpoint to refresh against, kept so a refresh does not need metadata discovery again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// The *name* of the environment variable holding the client secret, never the secret itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_env: Option<String>,
    /// The resource this token was issued for (RFC 8707): it is sent to this endpoint and no other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
}

impl Tokens {
    /// An access token with no stated lifetime is treated as valid: the server is the authority, and a 401 is
    /// answered by a refresh or by asking the user to sign in again — never by guessing here.
    pub fn expired(&self, now: f64) -> bool {
        self.expires_at.is_some_and(|at| now + EXPIRY_SKEW_SECS >= at as f64)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default = "one")]
    version: u32,
    #[serde(default)]
    services: BTreeMap<String, Tokens>,
}

fn one() -> u32 {
    1
}

/// The credential file for one config root.
#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    /// The file beside the config the bindings come from: a credential belongs to the binding that names the
    /// service, so it follows the same resolution (and the same test isolation) as `config.toml`.
    pub fn default_path() -> PathBuf {
        crate::config::xdg_config_home().join(crate::config::APP).join("mcp-auth.json")
    }

    pub fn at(path: &Path) -> Store {
        Store { path: path.to_path_buf() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Result<File, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("{} is not a credential file this build wrote: {e}", self.path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(File { version: 1, services: BTreeMap::new() })
            }
            Err(error) => Err(format!("cannot read {}: {error}", self.path.display())),
        }
    }

    /// Tokens, rewritten atomically and chmod 0600 before the bytes are visible under the real name.
    fn write(&self, file: &File) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let tmp = self.path.with_extension(format!("json.tmp-{}", std::process::id()));
        let text = serde_json::to_string_pretty(file).map_err(|e| format!("credential encode: {e}"))?;
        {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut handle = options.open(&tmp).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
            handle.write_all(text.as_bytes()).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
            handle.sync_all().map_err(|e| format!("cannot flush {}: {e}", tmp.display()))?;
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("cannot replace {}: {e}", self.path.display())
        })
    }

    pub fn get(&self, service: &str) -> Result<Option<Tokens>, String> {
        Ok(self.read()?.services.get(service).cloned())
    }

    pub fn put(&self, service: &str, tokens: &Tokens) -> Result<(), String> {
        let mut file = self.read()?;
        file.version = 1;
        file.services.insert(service.to_string(), tokens.clone());
        self.write(&file)
    }

    /// Signing out deletes the credential; the file itself stays, so a later sign-in does not need to create it.
    pub fn remove(&self, service: &str) -> Result<bool, String> {
        let mut file = self.read()?;
        let removed = file.services.remove(service).is_some();
        if removed {
            self.write(&file)?;
        }
        Ok(removed)
    }

    /// What a client may send right now: the stored access token, refreshed once if it is expired and the server
    /// issued a refresh token. `sign_in` is the sentence a user acts on when there is nothing usable.
    pub fn access_token(&self, service: &str) -> Result<Option<String>, String> {
        let Some(tokens) = self.get(service)? else { return Ok(None) };
        if !tokens.expired(now()) {
            return Ok(Some(tokens.access_token));
        }
        match (&tokens.refresh_token, &tokens.token_endpoint) {
            (Some(refresh), Some(endpoint)) => {
                let refreshed = refresh_tokens(endpoint, refresh, &tokens).map_err(|error| {
                    format!("{error}; run `teamagents mcp login --name {service}` to sign in again")
                })?;
                let mut merged = tokens.clone();
                merged.access_token = refreshed.access_token;
                merged.expires_at = refreshed.expires_at;
                if refreshed.refresh_token.is_some() {
                    merged.refresh_token = refreshed.refresh_token;
                }
                if !refreshed.scopes.is_empty() {
                    merged.scopes = refreshed.scopes;
                }
                self.put(service, &merged)?;
                Ok(Some(merged.access_token))
            }
            _ => Err(format!(
                "the stored credential for MCP service {service:?} expired and carries no refresh token; run \
                 `teamagents mcp login --name {service}` to sign in again"
            )),
        }
    }

    /// The state `mcp list` prints, without ever touching a token's value.
    pub fn state(&self, service: &str) -> Result<&'static str, String> {
        match self.get(service)? {
            None => Ok("not signed in"),
            Some(tokens) if tokens.expired(now()) && tokens.refresh_token.is_none() => Ok("expired (sign in again)"),
            Some(tokens) if tokens.expired(now()) => Ok("expired (will refresh)"),
            Some(_) => Ok("signed in"),
        }
    }
}

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

// -------------------------------------------------------------- the wire flow

/// What the authorization server's metadata gives us (§RFC 8414, discovered through the protected-resource
/// metadata of RFC 9728).
#[derive(Debug, Clone)]
pub struct AsMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub scopes_supported: Vec<String>,
}

fn get_json(url: &str) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new().timeout(HTTP_TIMEOUT).build();
    let response = agent.get(url).set("accept", "application/json").call().map_err(|e| format!("{url}: {e}"))?;
    response.into_json::<Value>().map_err(|e| format!("{url}: response is not JSON: {e}"))
}

fn origin_of(url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("{url} is not a URL: {e}"))?;
    match (parsed.scheme(), parsed.host_str()) {
        ("http" | "https", Some(host)) => {
            let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
            Ok(format!("{}://{host}{port}", parsed.scheme()))
        }
        _ => Err(format!("{url} is not an absolute http(s) URL")),
    }
}

/// Find the authorization server for an MCP endpoint: protected-resource metadata first (which is where an MCP
/// server announces its authorization server), then the authorization server's own metadata, then the endpoint's
/// origin as a last resort — the shape most servers deploy. Every failure names the URL that did not answer.
pub fn discover(resource_url: &str) -> Result<AsMetadata, String> {
    let origin = origin_of(resource_url)?;
    let mut authorization_servers: Vec<String> = Vec::new();
    let mut attempts: Vec<(String, String)> = Vec::new();
    for candidate in [
        format!("{resource_url}/.well-known/oauth-protected-resource"),
        format!("{origin}/.well-known/oauth-protected-resource"),
    ] {
        match get_json(&candidate) {
            Ok(metadata) => {
                if let Some(list) = metadata["authorization_servers"].as_array() {
                    authorization_servers = list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
                }
                if !authorization_servers.is_empty() {
                    break;
                }
            }
            Err(error) => attempts.push((candidate, error)),
        }
    }
    let bases = if authorization_servers.is_empty() { vec![origin] } else { authorization_servers };
    let mut last = String::new();
    for base in bases {
        let base_origin = origin_of(&base).unwrap_or_else(|_| base.clone());
        for candidate in [
            format!("{base}/.well-known/oauth-authorization-server"),
            format!("{base_origin}/.well-known/oauth-authorization-server"),
            format!("{base}/.well-known/openid-configuration"),
            format!("{base_origin}/.well-known/openid-configuration"),
        ] {
            match get_json(&candidate) {
                Ok(metadata) => {
                    let (Some(authorization_endpoint), Some(token_endpoint)) =
                        (metadata["authorization_endpoint"].as_str(), metadata["token_endpoint"].as_str())
                    else {
                        last = format!("{candidate} names no authorization_endpoint/token_endpoint");
                        continue;
                    };
                    return Ok(AsMetadata {
                        authorization_endpoint: authorization_endpoint.to_string(),
                        token_endpoint: token_endpoint.to_string(),
                        registration_endpoint: metadata["registration_endpoint"].as_str().map(str::to_string),
                        scopes_supported: metadata["scopes_supported"]
                            .as_array()
                            .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                            .unwrap_or_default(),
                    });
                }
                Err(error) => last = error,
            }
        }
    }
    let tried: Vec<String> = attempts.into_iter().map(|(url, error)| format!("{url} ({error})")).collect();
    Err(format!(
        "no OAuth metadata for {resource_url}: {last}{}",
        if tried.is_empty() {
            String::new()
        } else {
            format!("; protected-resource metadata also failed: {}", tried.join(", "))
        }
    ))
}

/// Register this client with the authorization server (RFC 7591). Servers that need a pre-registered client say so
/// by having no `registration_endpoint` — the config's `oauth_client_id` is the answer then.
pub fn register(
    metadata: &AsMetadata,
    redirect_uri: &str,
    client_name: &str,
    secret_env: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let endpoint = metadata
        .registration_endpoint
        .as_ref()
        .ok_or("the authorization server has no registration_endpoint: set oauth_client_id in the binding")?;
    let body = json!({
        "client_name": client_name,
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": if secret_env.is_some() { "client_secret_post" } else { "none" },
    });
    let agent = ureq::AgentBuilder::new().timeout(HTTP_TIMEOUT).build();
    let response = agent
        .post(endpoint)
        .set("content-type", "application/json")
        .set("accept", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| format!("client registration at {endpoint} failed: {e}"))?;
    let registered: Value =
        response.into_json().map_err(|e| format!("client registration at {endpoint} did not answer JSON: {e}"))?;
    let client_id = registered["client_id"]
        .as_str()
        .ok_or_else(|| format!("client registration at {endpoint} returned no client_id"))?
        .to_string();
    let secret = match secret_env {
        Some(var) => Some(std::env::var(var).map_err(|_| format!("{var} is not set"))?),
        None => registered["client_secret"].as_str().map(str::to_string),
    };
    Ok((client_id, secret))
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

fn form_encode(pairs: &[(&str, String)]) -> String {
    pairs.iter().map(|(key, value)| format!("{}={}", urlencode(key), urlencode(value))).collect::<Vec<_>>().join("&")
}

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn post_form(endpoint: &str, pairs: &[(&str, String)]) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new().timeout(HTTP_TIMEOUT).build();
    let response = agent
        .post(endpoint)
        .set("content-type", "application/x-www-form-urlencoded")
        .set("accept", "application/json")
        .send_string(&form_encode(pairs))
        .map_err(|error| match error {
            // a refusal carries the authorization server's own words (invalid_grant, invalid_client, …), and they
            // are the difference between "sign in again" and "your client id is wrong"
            ureq::Error::Status(code, response) => {
                let body = response.into_string().unwrap_or_default();
                let description = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| {
                        let error = v["error"].as_str().map(str::to_string);
                        let text = v["error_description"].as_str().map(str::to_string);
                        error.map(|e| text.map(|t| format!("{e}: {t}")).unwrap_or(e))
                    })
                    .unwrap_or_else(|| body.chars().take(200).collect());
                format!("{endpoint} refused the token request with {code} ({description})")
            }
            other => format!("{endpoint} is unreachable: {other}"),
        })?;
    response.into_json::<Value>().map_err(|e| format!("{endpoint} did not answer JSON: {e}"))
}

fn tokens_from_response(response: &Value, base: Tokens) -> Result<Tokens, String> {
    let access_token = response["access_token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "the token response carries no access_token".to_string())?;
    let expires_at = response["expires_in"].as_f64().map(|seconds| (now() + seconds) as i64);
    let scopes: Vec<String> = response["scope"]
        .as_str()
        .map(|list| list.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let mut tokens = Tokens {
        access_token: access_token.to_string(),
        refresh_token: response["refresh_token"].as_str().map(str::to_string).or(base.refresh_token),
        expires_at,
        ..base
    };
    if !scopes.is_empty() {
        tokens.scopes = scopes;
    }
    Ok(tokens)
}

/// Exchange a refresh token for a new access token.
pub fn refresh_tokens(token_endpoint: &str, refresh: &str, tokens: &Tokens) -> Result<Tokens, String> {
    let mut pairs = vec![("grant_type", "refresh_token".to_string()), ("refresh_token", refresh.to_string())];
    if let Some(client_id) = &tokens.client_id {
        pairs.push(("client_id", client_id.clone()));
    }
    if let Some(var) = &tokens.client_secret_env {
        if let Ok(secret) = std::env::var(var) {
            pairs.push(("client_secret", secret));
        }
    }
    if let Some(resource) = &tokens.resource {
        pairs.push(("resource", resource.clone()));
    }
    let response = post_form(token_endpoint, &pairs)?;
    tokens_from_response(&response, tokens.clone())
}

/// A sign-in in progress: the listener is already bound (so the redirect URI is known before the user leaves), the
/// PKCE verifier and `state` stay in this process, and `complete` is the only place the verifier travels.
pub struct PendingLogin {
    pub authorize_url: String,
    pub redirect_uri: String,
    metadata: AsMetadata,
    client_id: String,
    client_secret: Option<String>,
    client_secret_env: Option<String>,
    resource: String,
    verifier: String,
    state: String,
    listener: Option<TcpListener>,
    code: Option<String>,
}

/// The listener a login's redirect URI points at. Binding it **before** the client registration is what lets a
/// dynamically registered client name the exact redirect URI the authorization server will later be asked to
/// return to — a placeholder port is refused by servers that check it strictly.
pub struct Callback {
    listener: TcpListener,
    pub redirect_uri: String,
    pub port: u16,
}

/// Bind the loopback callback listener (`port` 0 picks a free one).
pub fn bind_callback(port: u16) -> Result<Callback, String> {
    let listener =
        TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("cannot listen on 127.0.0.1:{port}: {e}"))?;
    let port = listener.local_addr().map_err(|e| format!("loopback listener: {e}"))?.port();
    Ok(Callback { listener, redirect_uri: format!("http://127.0.0.1:{port}/callback"), port })
}

/// The OAuth parameters one binding declares, resolved in **one** place (D-412): the CLI prints the URL and
/// waits, the engine decides what to send. Keeping the resolution here rather than in `main.rs` is what makes
/// these config keys *served* — the audit that holds every accepted key to a reader treats the CLI as plumbing,
/// and it is right to: a key the CLI alone reads is a key the engine never enforces.
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    pub resource: String,
    pub client_id: Option<String>,
    pub client_secret_env: Option<String>,
    pub callback_port: u16,
    pub client_name: String,
    pub scopes: Vec<String>,
}

/// Resolve a binding's OAuth keys, refusing the combinations the flow cannot run with. The loader validates these
/// too (so a bad config is caught before a session starts); this is the same rule at the point of use, where the
/// sentence can name what the sign-in needs.
pub fn params(binding: &teamagents_core::models::ToolBinding) -> Result<Params, String> {
    if !binding.oauth {
        return Err("this binding is not an OAuth binding: set oauth = true (docs/CONFIG.md)".into());
    }
    if binding.mcp_transport.as_deref().unwrap_or("stdio") != "http" {
        return Err("an OAuth binding needs mcp_transport = \"http\" (docs/CONFIG.md)".into());
    }
    let resource = binding
        .url
        .clone()
        .filter(|url| !url.is_empty())
        .ok_or("an OAuth binding needs a url: that endpoint is the resource the credential is issued for")?;
    if binding.bearer_token_env_var.is_some() {
        return Err(
            "this binding names both oauth = true and a bearer_token_env_var: keep one credential (docs/CONFIG.md)"
                .into(),
        );
    }
    Ok(Params {
        resource,
        client_id: binding.oauth_client_id.clone(),
        client_secret_env: binding.oauth_client_secret_env_var.clone(),
        callback_port: binding.oauth_callback_port.unwrap_or(0),
        client_name: binding.oauth_client_name.clone().unwrap_or_else(|| "TeamAgents".to_string()),
        scopes: binding.oauth_scopes.clone(),
    })
}

impl Params {
    /// The client secret, from the environment variable the binding named — never from the config file.
    pub fn client_secret(&self) -> Result<Option<String>, String> {
        match &self.client_secret_env {
            Some(var) => std::env::var(var)
                .map(Some)
                .map_err(|_| format!("{var} is not set (it holds the secret of oauth_client_id)")),
            None => Ok(None),
        }
    }
}

pub struct AuthorizeRequest<'a> {
    pub metadata: &'a AsMetadata,
    pub resource: &'a str,
    pub client_id: &'a str,
    pub client_secret: Option<String>,
    pub client_secret_env: Option<&'a str>,
    pub callback: Callback,
    pub scopes: &'a [String],
}

/// Build the authorization URL for a bound callback. Nothing is sent anywhere yet: the user sees the URL, and the
/// code comes back to the listener `bind_callback` bound (or is pasted, for a browser on another machine).
pub fn authorize(request: AuthorizeRequest) -> Result<PendingLogin, String> {
    let Callback { listener, redirect_uri, .. } = request.callback;
    let verifier = random_token();
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    let state = random_token();
    let mut pairs: Vec<(&str, String)> = vec![
        ("response_type", "code".into()),
        ("client_id", request.client_id.to_string()),
        ("redirect_uri", redirect_uri.clone()),
        ("state", state.clone()),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256".into()),
        ("resource", request.resource.to_string()),
    ];
    if !request.scopes.is_empty() {
        pairs.push(("scope", request.scopes.join(" ")));
    }
    let query = pairs
        .iter()
        .map(|(key, value)| format!("{}={}", urlencode(key), urlencode(value)))
        .collect::<Vec<_>>()
        .join("&");
    Ok(PendingLogin {
        authorize_url: format!("{}?{query}", request.metadata.authorization_endpoint),
        redirect_uri,
        metadata: request.metadata.clone(),
        client_id: request.client_id.to_string(),
        client_secret: request.client_secret,
        client_secret_env: request.client_secret_env.map(str::to_string),
        resource: request.resource.to_string(),
        verifier,
        state,
        listener: Some(listener),
        code: None,
    })
}

impl PendingLogin {
    /// Read one callback from the loopback listener and take the code out of it. Requests for anything else (a
    /// favicon, a second callback) are answered and ignored, and the `state` must match or the code is refused:
    /// the callback is the one place an attacker who guessed the port could inject a code.
    pub fn wait(&mut self, timeout: Duration) -> Result<String, String> {
        if let Some(code) = &self.code {
            return Ok(code.clone());
        }
        let listener = self.listener.take().ok_or("this sign-in has no callback listener")?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "the browser did not reach {redirect} within {timeout:?}; run the sign-in again, or pass \
                     `--paste` and paste the redirected URL",
                    redirect = self.redirect_uri
                ));
            }
            listener.set_nonblocking(true).map_err(|e| format!("loopback listener: {e}"))?;
            let (mut stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(error) => return Err(format!("loopback listener: {error}")),
            };
            let request_line = read_request_line(&mut stream);
            let Some((code, state, error)) = callback_params(&request_line) else {
                respond(&mut stream, 404, "Not this callback");
                continue;
            };
            if let Some(error) = error {
                respond(&mut stream, 200, "Sign-in was refused. You can close this window.");
                return Err(format!("the authorization server refused the sign-in: {error}"));
            }
            if state.as_deref() != Some(self.state.as_str()) {
                respond(&mut stream, 400, "State mismatch.");
                return Err("the callback's `state` does not match this sign-in: refusing the code".into());
            }
            respond(&mut stream, 200, "Signed in. You can close this window and return to the terminal.");
            self.code = code.clone();
            return code.ok_or_else(|| "the callback carried no code".to_string());
        }
    }

    /// The redirected URL pasted by the user, for a browser that cannot reach this machine's loopback (SSH). The
    /// `state` is checked the same way — a paste is not a reason to accept a URL this sign-in did not ask for.
    pub fn paste(&mut self, pasted: &str) -> Result<String, String> {
        let (code, state, error) = callback_params(&format!("GET {pasted} HTTP/1.1"))
            .ok_or_else(|| "the pasted text carries no `code=` or `error=`".to_string())?;
        if let Some(error) = error {
            return Err(format!("the authorization server refused the sign-in: {error}"));
        }
        if state.as_deref() != Some(self.state.as_str()) {
            return Err("the pasted URL's `state` does not match this sign-in: refusing the code".into());
        }
        let code = code.ok_or_else(|| "the pasted URL carries no code".to_string())?;
        self.code = Some(code.clone());
        Ok(code)
    }

    /// Exchange the code for tokens. The PKCE verifier travels here and nowhere else; the resource is repeated so a
    /// token is bound to the MCP endpoint it is for (RFC 8707).
    pub fn complete(&self, code: &str) -> Result<Tokens, String> {
        let mut pairs = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", self.redirect_uri.clone()),
            ("client_id", self.client_id.clone()),
            ("code_verifier", self.verifier.clone()),
            ("resource", self.resource.clone()),
        ];
        if let Some(secret) = &self.client_secret {
            pairs.push(("client_secret", secret.clone()));
        }
        let response = post_form(&self.metadata.token_endpoint, &pairs)?;
        let base = Tokens {
            access_token: String::new(),
            refresh_token: None,
            expires_at: None,
            token_endpoint: Some(self.metadata.token_endpoint.clone()),
            client_id: Some(self.client_id.clone()),
            client_secret_env: self.client_secret_env.clone(),
            resource: Some(self.resource.clone()),
            scopes: Vec::new(),
        };
        tokens_from_response(&response, base)
    }
}

/// The `code`/`state`/`error` a redirect carries, from a request line or a pasted URL.
fn callback_params(input: &str) -> Option<(Option<String>, Option<String>, Option<String>)> {
    // two shapes arrive here: the loopback listener's request line (`GET /callback?… HTTP/1.1`) and a URL the user
    // pasted from a browser on another machine (`http://127.0.0.1:PORT/callback?…`), so a URL is normalised to its
    // path-and-query first
    let raw = input.trim();
    let raw = raw.strip_prefix("GET ").unwrap_or(raw);
    let raw = raw.split_whitespace().next()?;
    let path = match url::Url::parse(raw) {
        Ok(url) => match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_string(),
        },
        Err(_) => raw.to_string(),
    };
    let query = path.split_once('?')?.1;
    if !path.starts_with("/callback") {
        return None;
    }
    let mut params = BTreeMap::new();
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(key.to_string(), urldecode(value));
    }
    let code = params.get("code").cloned().filter(|c| !c.is_empty());
    let error = params.get("error").map(|e| match params.get("error_description") {
        Some(description) if !description.is_empty() => format!("{} ({description})", e),
        _ => e.clone(),
    });
    Some((code, params.get("state").cloned(), error))
}

fn urldecode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_request_line(stream: &mut TcpStream) -> String {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let mut reader = BufReader::new(stream.try_clone().expect("clone loopback stream"));
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    line.trim_end().to_string()
}

fn respond(stream: &mut TcpStream, status: u16, message: &str) {
    let body = format!("<!doctype html><meta charset=\"utf-8\"><title>TeamAgents</title><p>{message}");
    let reason = if status == 200 { "OK" } else { "Bad Request" };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

/// Best-effort browser opening: a machine with no browser (a server, a container) is a normal case, and the URL is
/// printed either way.
pub fn open_browser(url: &str) -> bool {
    let opened = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    match opened {
        Ok(mut child) => {
            let _ = child.wait();
            true
        }
        Err(_) => false,
    }
}

/// Read a line from stdin for the paste path, so a headless sign-in needs no argument quoting.
pub fn read_pasted_url() -> Result<String, String> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| format!("stdin: {e}"))?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err("no URL pasted".into());
    }
    Ok(trimmed.to_string())
}

/// The whole sign-in, minus the browser: used by `mcp login` (which prints the URL and waits) and by tests.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_file_is_private_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("ta-mcp-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::at(&dir.join("mcp-auth.json"));
        assert!(store.get("sentry").unwrap().is_none());
        let tokens = Tokens {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_at: Some((now() + 3600.0) as i64),
            token_endpoint: Some("https://as.example/token".into()),
            client_id: Some("c".into()),
            client_secret_env: Some("SENTRY_SECRET".into()),
            resource: Some("https://mcp.example/mcp".into()),
            scopes: vec!["read".into()],
        };
        store.put("sentry", &tokens).unwrap();
        assert_eq!(store.get("sentry").unwrap().as_ref(), Some(&tokens));
        assert_eq!(store.access_token("sentry").unwrap().as_deref(), Some("a"));
        assert_eq!(store.state("sentry").unwrap(), "signed in");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the credential file is not private");
        }
        // a token with no stated lifetime is used as-is; an expired one with no refresh is refused, not guessed
        let mut eternal = tokens.clone();
        eternal.expires_at = None;
        store.put("eternal", &eternal).unwrap();
        assert_eq!(store.access_token("eternal").unwrap().as_deref(), Some("a"));
        let mut stale = tokens.clone();
        stale.expires_at = Some((now() - 10.0) as i64);
        stale.refresh_token = None;
        store.put("stale", &stale).unwrap();
        let refused = store.access_token("stale").unwrap_err();
        assert!(refused.contains("mcp login"), "{refused}");
        assert_eq!(store.state("stale").unwrap(), "expired (sign in again)");
        assert!(store.remove("sentry").unwrap());
        assert!(!store.remove("sentry").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_callback_is_only_read_for_the_code_it_brought() {
        assert!(callback_params("GET /favicon.ico HTTP/1.1").is_none());
        assert!(callback_params("GET / HTTP/1.1").is_none());
        let (code, state, error) = callback_params("GET /callback?code=abc&state=xyz HTTP/1.1").unwrap();
        assert_eq!(code.as_deref(), Some("abc"));
        assert_eq!(state.as_deref(), Some("xyz"));
        assert!(error.is_none());
        let (code, _, error) =
            callback_params("GET /callback?error=access_denied&error_description=nope HTTP/1.1").unwrap();
        assert!(code.is_none());
        assert_eq!(error.as_deref(), Some("access_denied (nope)"));
        // a percent-encoded code survives the round trip
        let (code, _, _) = callback_params("GET /callback?code=a%2Fb%2Bc&state=s HTTP/1.1").unwrap();
        assert_eq!(code.as_deref(), Some("a/b+c"));
    }

    #[test]
    fn a_signed_in_url_knows_its_resource_and_never_leaks_the_verifier() {
        let metadata = AsMetadata {
            authorization_endpoint: "https://as.example/authorize".into(),
            token_endpoint: "https://as.example/token".into(),
            registration_endpoint: None,
            scopes_supported: vec!["read".into()],
        };
        let callback = bind_callback(0).unwrap();
        assert!(callback.redirect_uri.starts_with("http://127.0.0.1:"));
        let pending = authorize(AuthorizeRequest {
            metadata: &metadata,
            resource: "https://mcp.example/mcp",
            client_id: "client-1",
            client_secret: None,
            client_secret_env: None,
            callback,
            scopes: &["read".to_string()],
        })
        .unwrap();
        assert!(pending.authorize_url.starts_with("https://as.example/authorize?"));
        assert!(pending.authorize_url.contains("code_challenge_method=S256"));
        assert!(pending.authorize_url.contains("resource=https%3A%2F%2Fmcp.example%2Fmcp"));
        assert!(pending.authorize_url.contains("state="));
        assert!(!pending.authorize_url.contains(&pending.verifier), "the verifier must not travel with the URL");
        assert!(pending.redirect_uri.starts_with("http://127.0.0.1:"));
        // the state check is the same for a paste as for the listener
        let mut pending = pending;
        let refused = pending.paste(&format!("{}/callback?code=abc&state=wrong", pending.redirect_uri)).unwrap_err();
        assert!(refused.contains("state"), "{refused}");
        let pasted = format!("{}/callback?code=abc&state={}", pending.redirect_uri, pending.state);
        assert_eq!(pending.paste(&pasted).unwrap(), "abc");
        // and the same URL with a `state` this sign-in did not ask for is refused
        let pasted = format!("{}/callback?code=abc&state=other", pending.redirect_uri);
        assert!(pending.paste(&pasted).unwrap_err().contains("state"));
    }
}
