//! Public native-client flow: https://developers.openai.com/siwc/token-sharing-open-source/sign-in
use super::{Grant, Registration};
use crate::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
    signature,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;
use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::{form_urlencoded, Url};

const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const JWKS: &str = "https://auth.openai.com/.well-known/jwks.json";
const ISSUER: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const DYNAMIC: &str = "dynamic_agent_client";
const SCOPES: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const PATH: &str = "/auth/callback";
const BROWSER_WAIT: Duration = Duration::from_secs(300);
const HEADER_LIMIT: usize = 16 * 1024;
const JSON_LIMIT: u64 = 128 * 1024;
const BAD_ID: &str = "ChatGPT ID token failed verification";

fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| "invalid system clock")?.as_secs())
}

fn random_value() -> Result<String> {
    let mut bytes = [0; 32];
    SystemRandom::new().fill(&mut bytes).map_err(|_| "system random generator unavailable")?;
    Ok(B64.encode(bytes))
}

fn challenge(verifier: &str) -> String {
    B64.encode(digest::digest(&digest::SHA256, verifier.as_bytes()))
}

struct Attempt {
    state: String,
    nonce: String,
    verifier: String,
    authority: String,
    redirect: String,
}

impl Attempt {
    fn new(listener: &TcpListener) -> Result<Self> {
        let authority = listener.local_addr().map_err(|_| "cannot inspect ChatGPT callback listener")?.to_string();
        Ok(Self {
            state: random_value()?,
            nonce: random_value()?,
            verifier: random_value()?,
            redirect: format!("http://{authority}{PATH}"),
            authority,
        })
    }

    fn url(&self, host_id: uuid::Uuid, client_id: Option<&str>) -> Url {
        let host_urn = host_id.urn().to_string();
        let mut url = Url::parse(AUTHORIZE).expect("static authorization URL");
        let mut query = url.query_pairs_mut();
        query.extend_pairs([
            ("client_id", client_id.unwrap_or(DYNAMIC)),
            ("ext_agent_host_id", host_urn.as_str()),
            ("response_type", "code"),
            ("redirect_uri", &self.redirect),
            ("scope", SCOPES),
            ("resource", RESOURCE),
            ("state", &self.state),
            ("nonce", &self.nonce),
            ("code_challenge_method", "S256"),
            ("code_challenge", &challenge(&self.verifier)),
        ]);
        if client_id.is_none() {
            query.append_pair("agent_name_hint", "TeamAgents");
        }
        // The URL is printed for manual browser launch. Never put tokens or account hints into it.
        drop(query);
        url
    }
}

fn opaque(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_graphic())
}

struct Callback {
    code: String,
    client_id: String,
}

// None means an unrelated, malformed or unbound request. It cannot end a pending sign-in.
fn callback(headers: &str, attempt: &Attempt, pending: Option<&str>) -> Option<Result<Callback>> {
    let mut lines = headers.split("\r\n");
    let mut request = lines.next()?.split(' ');
    if request.next()? != "GET" {
        return None;
    }
    let (path, query) = request.next()?.split_once('?')?;
    if path != PATH || query.contains('#') || request.next()? != "HTTP/1.1" || request.next().is_some() {
        return None;
    }
    let mut host = None;
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("host") && host.replace(value.trim()).is_some() {
            return None;
        }
    }
    if host != Some(attempt.authority.as_str()) {
        return None;
    }
    let params: Vec<_> = form_urlencoded::parse(query.as_bytes()).collect();
    let mut states = params.iter().filter(|(k, _)| k == "state");
    if states.next()?.1 != attempt.state || states.next().is_some() {
        return None;
    }

    Some((|| {
        let field = |name| -> Result<Option<&str>> {
            let mut values = params.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_ref());
            let first = values.next();
            if values.next().is_some() {
                return Err("duplicate field in ChatGPT callback".into());
            }
            Ok(first)
        };
        if let Some(error) = field("error")? {
            return Err(if error == "access_denied" {
                "ChatGPT authorization declined"
            } else {
                "ChatGPT authorization failed"
            }
            .into());
        }
        let code = field("code")?.filter(|c| opaque(c)).ok_or("missing or invalid ChatGPT authorization code")?;
        let issued = field("client_id")?
            .or(pending)
            .filter(|c| opaque(c) && *c != DYNAMIC)
            .ok_or("missing or invalid issued ChatGPT client ID")?;
        if pending.is_some_and(|id| id != issued) {
            return Err("ChatGPT callback changed the selected client ID".into());
        }
        Ok(Callback { code: code.into(), client_id: issued.into() })
    })())
}

fn read_callback(
    stream: &mut TcpStream,
    deadline: Instant,
    attempt: &Attempt,
    pending: Option<&str>,
) -> Option<Result<Callback>> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 1024];
    while bytes.len() < HEADER_LIMIT {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        stream.set_read_timeout(Some(remaining)).ok()?;
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        let start = bytes.len().saturating_sub(3);
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(pos) = bytes[start..].windows(4).position(|w| w == b"\r\n\r\n") {
            let end = start + pos;
            if end + 4 > HEADER_LIMIT {
                break;
            }
            return callback(std::str::from_utf8(&bytes[..end]).ok()?, attempt, pending);
        }
    }
    None
}

fn wait(listener: TcpListener, attempt: &Attempt, pending: Option<&str>, deadline: Instant) -> Result<Callback> {
    while Instant::now() < deadline {
        let (mut stream, peer) = match listener.accept() {
            Ok(pair) => pair,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(40));
                continue;
            }
            Err(_) => return Err("ChatGPT callback listener failed".into()),
        };
        if !peer.ip().is_loopback() {
            continue;
        }
        let per_connection = deadline.min(Instant::now() + Duration::from_secs(2));
        let result = read_callback(&mut stream, per_connection, attempt, pending);
        let (status, text) = if result.is_some() {
            ("200 OK", "Callback received. Return to TeamAgents for the sign-in result.")
        } else {
            ("400 Bad Request", "Unrecognized callback.")
        };
        if stream.set_write_timeout(Some(Duration::from_millis(250))).is_ok() {
            let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{text}", text.len());
        }
        if let Some(bound) = result {
            return bound;
        }
    }
    Err("ChatGPT sign-in timed out; run `teamagents auth login` again".into())
}

fn http() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .redirects(0)
        .max_idle_connections(0)
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
}

fn json_body<T: DeserializeOwned>(response: ureq::Response) -> Result<T> {
    let mut bytes = Vec::new();
    response.into_reader().take(JSON_LIMIT + 1).read_to_end(&mut bytes).map_err(|_| "incomplete ChatGPT response")?;
    if bytes.len() as u64 > JSON_LIMIT {
        return Err("ChatGPT response too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid ChatGPT response".into())
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    token_type: String,
    expires_in: u64,
    scope: Option<String>,
    id_token: Option<String>,
}

impl Tokens {
    fn grant(
        self,
        client_id: String,
        subject: String,
        prior_scopes: Option<&[String]>,
        received: u64,
    ) -> Result<Grant> {
        if !self.token_type.eq_ignore_ascii_case("Bearer")
            || !opaque(&self.access_token)
            || !opaque(&self.refresh_token)
            || self.expires_in == 0
        {
            return Err("invalid ChatGPT bearer credentials".into());
        }
        let scopes = match self.scope {
            Some(s) => s.split(' ').filter(|part| !part.is_empty()).map(str::to_owned).collect(),
            None => prior_scopes.ok_or("ChatGPT token response omitted granted scopes")?.to_vec(),
        };
        // An explicit reduced grant replaces the old permissions. Storage gates inference on these scopes.
        Ok(Grant {
            client_id,
            subject,
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            expires_at: received.checked_add(self.expires_in).ok_or("invalid ChatGPT token expiry")?,
            scopes,
        })
    }
}

enum ExchangeError {
    InvalidGrant,
    InvalidRefresh,
    Other(String),
}

fn exchange(http: &ureq::Agent, endpoint: &str, fields: &[(&str, &str)]) -> std::result::Result<Tokens, ExchangeError> {
    let response = match http.post(endpoint).send_form(fields) {
        Ok(r) | Err(ureq::Error::Status(_, r)) => r,
        Err(_) => return Err(ExchangeError::Other("could not reach the ChatGPT token endpoint".into())),
    };
    let status = response.status();
    if status == 200 {
        return json_body(response).map_err(ExchangeError::Other);
    }
    if matches!(status, 400 | 401) {
        let body: Value = json_body(response).unwrap_or(Value::Null);
        let code = body.pointer("/error/code").or_else(|| body.get("error")).and_then(Value::as_str);
        match code {
            Some("invalid_grant") => return Err(ExchangeError::InvalidGrant),
            Some(
                "invalid_refresh_token"
                | "token_expired"
                | "refresh_token_expired"
                | "refresh_token_invalidated"
                | "refresh_token_reused",
            ) => return Err(ExchangeError::InvalidRefresh),
            _ => (),
        }
    }
    // Server-provided diagnostics may echo credentials. Expose only the numeric status.
    Err(ExchangeError::Other(format!("ChatGPT token endpoint returned HTTP {status}")))
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: String,
    crit: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Audience,
    exp: u64,
    iat: u64,
    nbf: Option<u64>,
    nonce: String,
    azp: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    Single(String),
    List(Vec<String>),
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Value>,
}

fn verify_id(jwt: &str, keys: &Jwks, issued: &str, nonce: &str, at: u64) -> Result<String> {
    let (signed, sig) = jwt.rsplit_once('.').ok_or(BAD_ID)?;
    let (head, payload) = signed.split_once('.').ok_or(BAD_ID)?;
    let header: Header = serde_json::from_slice(&B64.decode(head).map_err(|_| BAD_ID)?).map_err(|_| BAD_ID)?;
    if header.alg != "RS256" || header.kid.is_empty() || header.crit.is_some() {
        return Err(BAD_ID.into());
    }
    // Only select keys from the fixed HTTPS JWKS. Ignore any key or URL in the JWT itself.
    let mut matching = keys.keys.iter().filter(|k| k["kid"].as_str() == Some(&header.kid));
    let key = matching.next().ok_or(BAD_ID)?;
    if matching.next().is_some()
        || key["kty"] != "RSA"
        || key.get("alg").is_some_and(|v| v != "RS256")
        || key.get("use").is_some_and(|v| v != "sig")
        || key.get("key_ops").is_some_and(|ops| !ops.as_array().is_some_and(|a| a.iter().any(|v| v == "verify")))
    {
        return Err(BAD_ID.into());
    }
    let component = |name: &str| B64.decode(key[name].as_str().ok_or(BAD_ID)?).map_err(|_| BAD_ID);
    signature::RsaPublicKeyComponents { n: component("n")?, e: component("e")? }
        .verify(&signature::RSA_PKCS1_2048_8192_SHA256, signed.as_bytes(), &B64.decode(sig).map_err(|_| BAD_ID)?)
        .map_err(|_| BAD_ID)?;
    let claims: Claims = serde_json::from_slice(&B64.decode(payload).map_err(|_| BAD_ID)?).map_err(|_| BAD_ID)?;
    verify_claims(claims, issued, nonce, at)
}

fn verify_claims(c: Claims, issued: &str, nonce: &str, at: u64) -> Result<String> {
    let audience_ok = match &c.aud {
        Audience::Single(a) => a == issued,
        Audience::List(a) => a.iter().any(|s| s == issued) && (a.len() == 1 || c.azp.as_deref() == Some(issued)),
    };
    if !audience_ok
        || c.azp.as_deref().is_some_and(|azp| azp != issued)
        || c.iss != ISSUER
        || c.sub.trim().is_empty()
        || c.exp <= at
        || c.iat > at.saturating_add(60)
        || c.nbf.is_some_and(|n| n > at)
        || c.nonce != nonce
    {
        return Err(BAD_ID.into());
    }
    Ok(c.sub)
}

pub(super) fn authorize(host_id: &str, previous: Option<&Registration>) -> Result<Grant> {
    // Profiles store the UUID itself; OpenAI identifies the same host by its UUID URN.
    let host_id = uuid::Uuid::parse_str(host_id).map_err(|_| "invalid saved ChatGPT host UUID")?;
    let http = http();
    let mut pending = previous.map(|r| r.client_id.clone());
    if pending.as_deref().is_some_and(|id| !opaque(id) || id == DYNAMIC) {
        return Err("invalid saved ChatGPT client ID".into());
    }
    for attempt_no in 0..2 {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| "cannot bind ChatGPT callback on 127.0.0.1")?;
        listener.set_nonblocking(true).map_err(|_| "cannot configure ChatGPT callback listener")?;
        let attempt = Attempt::new(&listener)?;
        writeln!(
            io::stderr().lock(),
            "Continue with ChatGPT. Open this URL in your browser:\n{}",
            attempt.url(host_id, pending.as_deref())
        )
        .map_err(|_| "cannot display ChatGPT authorization URL")?;
        let callback = wait(listener, &attempt, pending.as_deref(), Instant::now() + BROWSER_WAIT)?;
        let tokens = match exchange(
            &http,
            TOKEN,
            &[
                ("grant_type", "authorization_code"),
                ("client_id", &callback.client_id),
                ("code", &callback.code),
                ("code_verifier", &attempt.verifier),
                ("redirect_uri", &attempt.redirect),
                ("resource", RESOURCE),
            ],
        ) {
            Ok(tokens) => tokens,
            Err(ExchangeError::InvalidGrant) if attempt_no == 0 => {
                pending = Some(callback.client_id);
                writeln!(io::stderr().lock(), "ChatGPT declined the code. Starting a fresh authorization attempt.")
                    .map_err(|_| "cannot display ChatGPT sign-in instructions")?;
                continue;
            }
            Err(ExchangeError::InvalidGrant | ExchangeError::InvalidRefresh) => {
                return Err("ChatGPT rejected the code; run `teamagents auth login` again".into())
            }
            Err(ExchangeError::Other(error)) => return Err(error),
        };
        let received = now()?;
        let id = tokens.id_token.as_deref().ok_or("ChatGPT token response omitted the ID token")?;
        let response = http.get(JWKS).call().map_err(|_| "could not fetch OpenAI signing keys")?;
        if response.status() != 200 {
            return Err("could not fetch OpenAI signing keys".into());
        }
        let subject = verify_id(id, &json_body(response)?, &callback.client_id, &attempt.nonce, now()?)?;
        if previous.is_some_and(|r| r.subject != subject) {
            return Err("ChatGPT identity differs from the selected registration".into());
        }
        return tokens.grant(callback.client_id, subject, None, received);
    }
    unreachable!("only the first code exchange can start another attempt")
}

pub(super) fn refresh(grant: &Grant) -> Result<Grant> {
    refresh_at(&http(), TOKEN, grant)
}

fn refresh_at(http: &ureq::Agent, endpoint: &str, grant: &Grant) -> Result<Grant> {
    if !opaque(&grant.client_id) || grant.client_id == DYNAMIC || !opaque(&grant.refresh_token) {
        return Err(super::RELOGIN.into());
    }
    let tokens = match exchange(
        http,
        endpoint,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", &grant.client_id),
            ("refresh_token", &grant.refresh_token),
            ("resource", RESOURCE),
        ],
    ) {
        Ok(tokens) => tokens,
        Err(ExchangeError::InvalidGrant | ExchangeError::InvalidRefresh) => return Err(super::RELOGIN.into()),
        Err(ExchangeError::Other(error)) => return Err(error),
    };
    // Refresh renews the existing grant. Never consume an optional refresh-time ID token as a new identity.
    tokens.grant(grant.client_id.clone(), grant.subject.clone(), Some(&grant.scopes), now()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        collections::HashMap,
        io::{BufRead, BufReader},
    };

    fn listener() -> TcpListener {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap()
    }

    #[test]
    fn pkce_and_registration_parameters() {
        // RFC 7636 Appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let listener = listener();
        let a = Attempt::new(&listener).unwrap();
        let b = Attempt::new(&listener).unwrap();
        for (x, y) in [(&a.state, &b.state), (&a.nonce, &b.nonce), (&a.verifier, &b.verifier)] {
            assert_eq!(B64.decode(x).unwrap().len(), 32);
            assert_ne!(x, y);
        }
        let host = uuid::Uuid::parse_str("7ba7f286-53eb-4827-91c9-cd2b7e2e0001").unwrap();
        for id in [None, Some("issued+client")] {
            let url = a.url(host, id);
            assert_eq!(url.as_str().split('?').next(), Some(AUTHORIZE));
            let p: HashMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!(p["redirect_uri"], format!("http://{}/auth/callback", listener.local_addr().unwrap()));
            assert_eq!(p["ext_agent_host_id"], "urn:uuid:7ba7f286-53eb-4827-91c9-cd2b7e2e0001");
            assert_eq!(p["resource"], RESOURCE);
            assert_eq!(p["scope"], SCOPES);
            assert_eq!(p["client_id"], id.unwrap_or(DYNAMIC));
            assert_eq!(p.get("agent_name_hint").map(String::as_str), id.is_none().then_some("TeamAgents"));
            assert_eq!(p["code_challenge_method"], "S256");
            assert_eq!(p["code_challenge"], challenge(&a.verifier));
            assert!(!url.as_str().contains(&a.verifier));
            assert!(!p.contains_key("id_token_hint"));
        }
    }

    #[test]
    fn callback_checks_state_before_errors_and_binds_issued_client() {
        let a = Attempt::new(&listener()).unwrap();
        let request = |query: &str| format!("GET {PATH}?{query} HTTP/1.1\r\nHost: {}", a.authority);
        let success = format!("state={}&code=synthetic%2Bcode", a.state);
        let cb = |q: &str, id: Option<&str>| callback(&request(q), &a, id);
        assert!(cb("state=wrong&error=access_denied", None).is_none());
        assert!(cb(&format!("{success}&state={}", a.state), None).is_none());
        assert!(cb(&format!("{success}&error=access_denied"), None).unwrap().is_err());
        assert!(cb(&success, None).unwrap().is_err());
        assert!(cb(&format!("{success}&client_id={DYNAMIC}"), None).unwrap().is_err());
        assert!(cb(&format!("{success}&client_id=wrong"), Some("issued")).unwrap().is_err());
        assert!(cb(&format!("{success}&code=another"), Some("issued")).unwrap().is_err());
        let first = cb(&format!("{success}&client_id=issued"), None).unwrap().unwrap();
        assert_eq!(first.client_id, "issued");
        assert_eq!(first.code, "synthetic+code");
        assert_eq!(cb(&success, Some("issued")).unwrap().unwrap().client_id, "issued");
        for headers in [
            request(&success).replace(PATH, "/callback"),
            request(&success).replace(&a.authority, "localhost"),
            request(&success).replace("GET ", "POST "),
            format!("{}\r\nHost: {}", request(&success), a.authority),
        ] {
            assert!(callback(&headers, &a, Some("issued")).is_none());
        }
    }

    #[test]
    fn listener_ignores_oversized_callback_and_redacts_browser_reply() {
        let listener = listener();
        listener.set_nonblocking(true).unwrap();
        let a = Attempt::new(&listener).unwrap();
        let addr = listener.local_addr().unwrap();
        let request = format!("GET {PATH}?state={}&code=synthetic-secret-code HTTP/1.1\r\nHost: {addr}\r\n", a.state);
        thread::scope(|scope| {
            let worker = scope.spawn(|| wait(listener, &a, Some("issued"), Instant::now() + Duration::from_secs(5)));
            let send = |extra: &str| {
                let mut stream = TcpStream::connect(addr).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                write!(stream, "{request}{extra}\r\n").unwrap();
                let mut reply = String::new();
                // Closing a connection with unread excessive input can end with a TCP reset.
                let _ = stream.read_to_string(&mut reply);
                assert!(!reply.contains("synthetic-secret-code"));
                assert!(!reply.contains(&a.state));
                reply
            };
            assert!(!send(&format!("X-Padding: {}\r\n", "x".repeat(HEADER_LIMIT))).starts_with("HTTP/1.1 200"));
            assert!(send("").starts_with("HTTP/1.1 200"));
            assert_eq!(worker.join().unwrap().unwrap().code, "synthetic-secret-code");
        });
    }

    #[test]
    fn offline_rs256_signature_and_claims() {
        // A synthetic fixture signed by a throwaway test key. Only the public modulus is retained.
        let key = json!({"kid":"test", "kty":"RSA", "use":"sig", "alg":"RS256", "e":"AQAB", "n":
            "x4Io81a-AFORnQap0UfppKr8DImf-wLlz0iKWZCKetPFun-PEENeSHBYdGRR2wTItzTsqZ8SzFeuXbIl-3rxoevXFPuQYaXaNvwfOug8xEFjCSAWG5uhFgBxzEEcli9TsRGahXugFn7mXxbzuPa_4XouNj4ay75QnJFwSO8eygjDnAD6YlkQB6r9AOZC1X8gCCfF6CWe_iJpKSG-Rt86XSnNDzdgqZYwKHFoM1f_yFZlmgzK_bxWXdT-yCi56i7M9IWCCLOQhIkz2ODlXtmKJMyJ97LnPNJmHOdyv2er2JITcl9ZSlCRvOl2mQOQknXf5IKMKQGtY59eU-M0L9zTow"});
        let keys = Jwks { keys: vec![key.clone()] };
        let jwt = concat!("eyJhbGciOiJSUzI1NiIsImtpZCI6InRlc3QifQ.",
            "eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6ImNsaWVudCIsInN1YiI6InN1YmplY3QiLCJub25jZSI6Im5vbmNlIiwiaWF0IjoxMDAwLCJleHAiOjIwMDAsIm5iZiI6MTAwMH0.",
            "myd5QZ9VJFFfzM1qTFVv_a3rz1s0Suga3fBC24qvtmQgrrAiab8_QAlTe8Gbx2F4Mi4GuZTdFCRA31QjWMZZuztH5GAXxjfTzDtr5Gx9hGdOzR_vG3L7PVOj6j3-lWpVdiTmO1GyCKld62B43SybLpm31DknUYdhRaZCfXGAZiwgi9U2ho_i5kDeDNQdraSHEbFuDtnpNbcNkRzQezmIW1OTH2tIIs74Du0MI2zOFJGxI4ZkuA3B9XMqbvVs5-abS4VFlaWVsUmGODDTx94HjfVIG6mWPBLtBVnemMz1ipuqBAEpnHeqU4aadkE6QZ6GJwimDJqTgH4H869kAtiMGA");
        assert_eq!(verify_id(jwt, &keys, "client", "nonce", 1000).unwrap(), "subject");
        for (id, nonce, at) in [
            ("different", "nonce", 1000),
            ("client", "wrong", 1000),
            ("client", "nonce", 2000),
            ("client", "nonce", 999),
        ] {
            assert_eq!(verify_id(jwt, &keys, id, nonce, at).unwrap_err(), BAD_ID);
        }
        let (signed, sig) = jwt.rsplit_once('.').unwrap();
        let mut bytes = B64.decode(sig).unwrap();
        bytes[0] ^= 1;
        let tampered = format!("{signed}.{}", B64.encode(bytes));
        assert_eq!(verify_id(&tampered, &keys, "client", "nonce", 1000).unwrap_err(), BAD_ID);
        let duplicate = Jwks { keys: vec![key.clone(), key] };
        assert_eq!(verify_id(jwt, &duplicate, "client", "nonce", 1000).unwrap_err(), BAD_ID);
        verify_id(jwt, &Jwks { keys: vec![] }, "client", "nonce", 1000).unwrap_err();
    }

    #[test]
    fn verified_claims_require_issuer_subject_and_bound_audience() {
        let claims =
            json!({"iss":ISSUER,"sub":"subject","aud":["client","other"],"azp":"client","iat":1,"exp":6,"nonce":"n"});
        let check = |v| verify_claims(serde_json::from_value(v).unwrap(), "client", "n", 3);
        assert_eq!(check(claims.clone()).unwrap(), "subject");
        for (name, value) in [
            ("iss", json!("https://other.invalid")),
            ("azp", Value::Null),
            ("azp", json!("other")),
            ("sub", json!("")),
            ("aud", json!(["other"])),
        ] {
            let mut bad = claims.clone();
            bad[name] = value;
            assert_eq!(check(bad).unwrap_err(), BAD_ID);
        }
    }

    // One local HTTP exchange per invocation. Read the form by Content-Length rather than waiting for EOF.
    fn fake_token(status: u16, body: String) -> (String, thread::JoinHandle<String>) {
        let listener = listener();
        let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut reader = BufReader::new(&stream);
            let mut lines = (&mut reader).lines();
            assert_eq!(lines.next().unwrap().unwrap(), "POST /token HTTP/1.1");
            let headers: HashMap<_, _> = lines
                .map_while(|s| {
                    let s = s.unwrap();
                    let (k, v) = s.split_once(':')?;
                    Some((k.to_ascii_lowercase(), v.trim().to_string()))
                })
                .collect();
            assert_eq!(headers["content-type"], "application/x-www-form-urlencoded");
            let mut form = vec![0; headers["content-length"].parse().unwrap()];
            reader.read_exact(&mut form).unwrap();
            write!(
                stream,
                "HTTP/1.1 {status} Response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            String::from_utf8(form).unwrap()
        });
        (endpoint, worker)
    }

    fn synthetic_grant() -> Grant {
        Grant {
            client_id: "issued+client".into(),
            subject: "subject".into(),
            scopes: vec!["chatgpt.tokens.use.direct".into()],
            access_token: "synthetic-old-access".into(),
            refresh_token: "synthetic+old/refresh".into(),
            expires_at: 100,
        }
    }

    fn token_reply() -> Value {
        json!({"token_type":"Bearer","access_token":"synthetic-access","refresh_token":"synthetic-rotated","expires_in":3600})
    }

    #[test]
    fn refresh_rotates_and_honors_granted_scopes() {
        let old = synthetic_grant();
        for scope in [None, Some("openid profile")] {
            let mut body = token_reply();
            if let Some(s) = scope {
                body["scope"] = json!(s);
            }
            let (endpoint, worker) = fake_token(200, body.to_string());
            let before = now().unwrap();
            let renewed = refresh_at(&http(), &endpoint, &old).unwrap();
            let form: HashMap<_, _> = form_urlencoded::parse(worker.join().unwrap().as_bytes()).into_owned().collect();
            assert_eq!(form.len(), 4);
            assert_eq!(form["grant_type"], "refresh_token");
            assert_eq!(form["resource"], RESOURCE);
            assert_eq!(form["refresh_token"], old.refresh_token);
            assert_eq!(form["client_id"], old.client_id);
            assert_eq!(renewed.access_token, "synthetic-access");
            assert_eq!(renewed.refresh_token, "synthetic-rotated");
            assert_eq!(renewed.subject, old.subject);
            assert_eq!(renewed.client_id, old.client_id);
            assert!((before + 3600..=now().unwrap() + 3600).contains(&renewed.expires_at));
            if scope.is_some() {
                assert_eq!(renewed.scopes, ["openid", "profile"]);
            } else {
                assert_eq!(renewed.scopes, old.scopes);
            }
        }
    }

    #[test]
    fn missing_initial_scopes_and_incomplete_rotation_fail_closed() {
        let tokens: Tokens = serde_json::from_value(token_reply()).unwrap();
        assert!(tokens.grant("client".into(), "subject".into(), None, 1000).is_err());
        let mut incomplete = token_reply();
        incomplete.as_object_mut().unwrap().remove("refresh_token");
        let (endpoint, worker) = fake_token(200, incomplete.to_string());
        assert!(refresh_at(&http(), &endpoint, &synthetic_grant()).is_err());
        worker.join().unwrap();
    }

    #[test]
    fn refresh_errors_are_classified_and_redacted() {
        let secret = "synthetic-secret-echo";
        for (status, error, expected) in [
            (400, json!("invalid_grant"), super::super::RELOGIN),
            (401, json!({"code":"refresh_token_reused"}), super::super::RELOGIN),
            (401, json!("invalid_client"), "ChatGPT token endpoint returned HTTP 401"),
            (503, json!("invalid_grant"), "ChatGPT token endpoint returned HTTP 503"),
            (200, json!(secret), "invalid ChatGPT response"),
        ] {
            let (endpoint, worker) = fake_token(status, json!({"error":error,"error_description":secret}).to_string());
            let msg = refresh_at(&http(), &endpoint, &synthetic_grant()).err().unwrap();
            worker.join().unwrap();
            assert_eq!(msg, expected);
            assert!(!msg.contains(secret));
            assert!(!msg.contains(&synthetic_grant().refresh_token));
        }
    }
}
