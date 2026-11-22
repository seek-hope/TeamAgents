//! Fetch routing is member-local. Search bindings do not authorize fetching.
use super::{guard_url, iso_now, web_credential, OutputLocation, TurnControl, PAGE_BYTES};
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use teamagents_core::models::ToolBinding;

const BODY_LIMIT: usize = 8 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(600);

struct Page {
    title: String,
    content: String,
    fetched_at: String,
}
struct Cached {
    key: [u8; 32],
    expires: Instant,
    page: Arc<Page>,
}
impl Cached {
    fn bytes(&self) -> usize {
        self.page.title.len() + self.page.content.len() + self.page.fetched_at.len()
    }
}

// Small, member-local cache. Keep the authenticated route in the digest, not in cache diagnostics.
#[derive(Default)]
pub(super) struct Fetcher(Mutex<VecDeque<Cached>>);

impl Fetcher {
    fn lookup(&self, key: &[u8; 32], now: Instant) -> Result<Option<Arc<Page>>, String> {
        let mut pages = self.0.lock().map_err(|_| "fetch cache poisoned")?;
        pages.retain(|entry| entry.expires > now);
        let Some(ix) = pages.iter().position(|entry| entry.key == *key) else { return Ok(None) };
        let entry = pages.remove(ix).expect("index from this queue");
        let page = entry.page.clone();
        pages.push_back(entry);
        Ok(Some(page))
    }

    fn remember(&self, entry: Cached) -> Result<(), String> {
        if entry.bytes() > BODY_LIMIT {
            return Ok(());
        }
        let mut pages = self.0.lock().map_err(|_| "fetch cache poisoned")?;
        let now = Instant::now();
        pages.retain(|old| old.key != entry.key && old.expires > now);
        let mut bytes = entry.bytes() + pages.iter().map(Cached::bytes).sum::<usize>();
        while pages.len() >= 32 || bytes > BODY_LIMIT {
            bytes -= pages.pop_front().expect("a cache entry fits by itself").bytes();
        }
        pages.push_back(entry);
        Ok(())
    }
}

struct Failure {
    detail: String,
    transient: bool,
}
impl Failure {
    fn permanent(detail: impl Into<String>) -> Self {
        Self { detail: detail.into(), transient: false }
    }
}

fn request(provider: &str, endpoint: &str, key: &str, url: &str) -> Result<Page, Failure> {
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let req = agent.post(endpoint).timeout(Duration::from_secs(30));
    let (req, body) = if provider == "exa" {
        (req.set("x-api-key", key), json!({"urls":[url], "text":{"maxCharacters":200_000}}))
    } else {
        (req.set("authorization", &format!("Bearer {key}")), json!({"url":url}))
    };
    // No response/error bodies or credential-bearing endpoint strings in diagnostics.
    let resp = match req.send_json(body) {
        Ok(resp) if resp.status() == 200 => resp,
        Ok(resp) => return Err(Failure::permanent(format!("unexpected HTTP {}", resp.status()))),
        Err(ureq::Error::Status(status, _)) => {
            return Err(Failure {
                detail: format!("HTTP {status}"),
                transient: matches!(status, 408 | 429 | 500..=599),
            })
        }
        Err(ureq::Error::Transport(error)) => {
            return Err(Failure {
                detail: format!("transport: {:?}", error.kind()),
                transient: matches!(
                    error.kind(),
                    ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed | ureq::ErrorKind::Io
                ),
            })
        }
    };
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(BODY_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure { detail: "response transport interrupted".into(), transient: true })?;
    if bytes.len() > BODY_LIMIT {
        return Err(Failure::permanent("response exceeds 8 MiB"));
    }
    let payload: Json = serde_json::from_slice(&bytes).map_err(|_| Failure::permanent("malformed JSON"))?;
    let (row, field) = if provider == "exa" {
        let rows = payload["results"].as_array().ok_or_else(|| Failure::permanent("missing results"))?;
        let [row] = rows.as_slice() else {
            return Err(Failure::permanent("expected one page"));
        };
        (row, "text")
    } else {
        (&payload, "content")
    };
    let content = row[field]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| Failure::permanent("missing readable content"))?;
    let title = row["title"].as_str().filter(|s| !s.is_empty()).unwrap_or(url);
    Ok(Page { title: title.chars().take(2048).collect(), content: content.into(), fetched_at: iso_now() })
}

impl Fetcher {
    pub(super) fn fetch(
        &self,
        bindings: &[ToolBinding],
        args: &Json,
        control: &TurnControl,
        output: OutputLocation<'_>,
    ) -> Result<Json, String> {
        if bindings.is_empty() {
            return Err("tool web_fetch is not bound to this member".into());
        }
        let selected = args
            .get("provider")
            .map_or(Some("auto"), Json::as_str)
            .filter(|s| matches!(*s, "auto" | "exa" | "deepseek"))
            .ok_or("provider must be auto, exa or deepseek")?;
        let content_limit = if control.script_output { BODY_LIMIT } else { PAGE_BYTES } as u64;
        let max = args
            .get("max_bytes")
            .map_or(Some(content_limit), Json::as_u64)
            .ok_or("max_bytes must be a nonnegative integer")?
            .min(content_limit) as usize;
        let raw = args["url"].as_str().filter(|s| s.len() <= 8192).ok_or("expected a URL of at most 8192 bytes")?;
        let mut url = url::Url::parse(raw).map_err(|_| "unsupported URL")?;
        if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
            return Err("only HTTP(S) URLs without embedded credentials are allowed".into());
        }
        url.set_fragment(None);
        let host = url.host_str().ok_or("URL has no host")?;
        // §5.1: `web` is a capability. Refuse outright when the member holds no web grant, and
        // otherwise require the host to be covered. Neither branch can widen access.
        if !control.web_permitted() {
            return Err("web_fetch: this member holds no web grant".into());
        }
        if !control.domain_allowed(host) {
            return Err(format!("web_fetch: {host} is not in this member's allowed domains"));
        }
        let mut warnings = Vec::new();
        let mut verified = false;
        for provider in ["exa", "deepseek"] {
            if selected != "auto" && selected != provider {
                continue;
            }
            let Some(binding) =
                bindings.iter().find(|binding| binding.provider.as_deref().unwrap_or("deepseek") == provider)
            else {
                continue;
            };
            control.check()?;
            if !verified && !binding.env.get("allow_private").is_some_and(|value| value == "1") {
                guard_url(url.as_str())?;
                verified = true;
            }
            let credential = web_credential(provider, binding.api_key_env.as_deref())?;
            let endpoint = binding.url.as_deref().unwrap_or(if provider == "exa" {
                "https://api.exa.ai/contents"
            } else {
                "https://api.deepseek.com/v1/tools/fetch"
            });
            let mut hasher = Sha256::new();
            for field in [provider, endpoint, url.as_str(), &credential] {
                hasher.update((field.len() as u64).to_be_bytes());
                hasher.update(field);
            }
            let key = hasher.finalize().into();
            let cached = self.lookup(&key, Instant::now())?;
            control.check()?;
            let hit = cached.is_some();
            let page = if let Some(page) = cached {
                page
            } else {
                match request(provider, endpoint, &credential, url.as_str()) {
                    Ok(page) => {
                        let page = Arc::new(page);
                        self.remember(Cached { key, expires: Instant::now() + TTL, page: page.clone() })?;
                        page
                    }
                    Err(failure) => {
                        warnings.push(format!("{provider}: {}", failure.detail));
                        if selected == "auto" && failure.transient && provider == "exa" {
                            continue;
                        }
                        return Err(warnings.join("; "));
                    }
                }
            };
            control.check()?;
            let end = page.content.floor_char_boundary(max.min(page.content.len()));
            let mut result = json!({
                "url": url.as_str(),
                "provider": provider,
                "title": page.title,
                "fetched_at": page.fetched_at,
                "content": &page.content[..end],
                "bytes": page.content.len(),
                "truncated": end < page.content.len(),
                "from_cache": hit,
                "warnings": warnings,
            });
            if end < page.content.len() && output.root.is_some() {
                let (mut file, reference) = super::create_output_file(output)?;
                file.write_all(page.content.as_bytes())
                    .and_then(|_| file.sync_all())
                    .map_err(|error| format!("write fetch output: {error}"))?;
                result["full_path"] = json!(reference);
                result["next_byte_offset"] = json!(end);
            }
            return Ok(result);
        }
        if warnings.is_empty() {
            Err(format!("no {selected} web_fetch binding for this member"))
        } else {
            Err(warnings.join("; "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{member_executor_with_control, ArtifactPaths};
    use super::*;
    use crate::web_url::tests::serve;
    use std::os::unix::fs::PermissionsExt;
    use teamagents_core::models::UserConfig;

    const URL: &str = "https://93.184.216.34/path";
    const EXA: &str = "TA_TEST_FETCH_ROUTING_EXA_KEY";
    const DS: &str = "TA_TEST_FETCH_ROUTING_DS_KEY";
    struct Keys(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Keys {
        fn set() -> Self {
            Self(
                [(EXA, "fixture-exa"), (DS, "fixture-deepseek")]
                    .into_iter()
                    .map(|(key, value)| {
                        let old = std::env::var_os(key);
                        std::env::set_var(key, value);
                        (key, old)
                    })
                    .collect(),
            )
        }
    }
    impl Drop for Keys {
        fn drop(&mut self) {
            for (key, old) in self.0.drain(..) {
                if let Some(old) = old {
                    std::env::set_var(key, old)
                } else {
                    std::env::remove_var(key)
                }
            }
        }
    }
    fn executor(
        endpoint: &str,
        artifacts: Option<std::path::PathBuf>,
    ) -> impl Fn(&str, &Json, &TurnControl) -> Result<Json, String> {
        let mut catalog = UserConfig::default();
        for (provider, env) in [("exa", EXA), ("deepseek", DS)] {
            catalog.tools.insert(
                provider.into(),
                ToolBinding {
                    kind: "web_fetch".into(),
                    provider: Some(provider.into()),
                    url: Some(format!("{endpoint}{provider}")),
                    api_key_env: Some(env.into()),
                    ..Default::default()
                },
            );
        }
        member_executor_with_control(
            std::env::temp_dir(),
            catalog,
            vec!["deepseek".into(), "exa".into(), "files".into()],
            ArtifactPaths::own(artifacts),
            None,
            None,
            None,
        )
    }

    #[test]
    fn fetch_exa_priority_cache_and_private_utf8_continuation() {
        let _lock = crate::env_lock();
        let _keys = Keys::set();
        let content = "\u{00e9}".repeat(32_001);
        let (endpoint, server) = serve(vec![(200, json!({"results":[{"title":"sample","text":content}]}).to_string())]);
        let root = std::env::temp_dir().join(format!("ta-fetch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        crate::scratch_owner::record_ownership(&root);
        let exec = executor(&endpoint, Some(root.join("member")));
        let control = TurnControl::with_web_scopes(vec!["session".to_string()]);
        let args = json!({"url":format!("{URL}#fragment"),"max_bytes":3});
        let first = exec("web_fetch", &args, &control).unwrap();
        let requests = server.join().unwrap();
        let wire = &requests[0];
        assert!(
            wire.starts_with("POST /exa ")
                && wire.to_lowercase().contains("x-api-key: fixture-exa")
                && !wire.contains("fixture-deepseek")
        );
        assert_eq!(
            serde_json::from_str::<Json>(wire.split_once("\r\n\r\n").unwrap().1).unwrap(),
            json!({"urls":[URL],"text":{"maxCharacters":200_000}})
        );
        assert_eq!(first["from_cache"], false);
        assert_eq!(first["provider"], "exa");
        assert_eq!(first["next_byte_offset"], 2);
        let path = first["full_path"].as_str().unwrap();
        assert_eq!(
            std::fs::metadata(root.join("member").join(path.strip_prefix("/artifacts/").unwrap()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let mut actual = first["content"].as_str().unwrap().to_owned();
        let mut offset = first["next_byte_offset"].clone();
        loop {
            let page = exec("read_file", &json!({"path":path,"byte_offset":offset}), &control).unwrap();
            actual.push_str(page["content"].as_str().unwrap());
            if page["eof"] == true {
                break;
            }
            offset = page["next_byte_offset"].clone();
        }
        assert_eq!(actual, content);
        let script = TurnControl { script_output: true, ..control.clone() };
        let full = exec("web_fetch", &json!({"url":URL}), &script).unwrap();
        assert_eq!(full["content"], content);
        assert_eq!(full["truncated"], false);
        assert_eq!(full["from_cache"], true);
        assert!(full.get("full_path").is_none());
        assert_eq!(exec("web_fetch", &args, &script).unwrap()["next_byte_offset"], 2);
        assert!(executor(&endpoint, Some(root.join("peer")))("read_file", &json!({"path":path}), &control).is_err());
        assert_eq!(exec("web_fetch", &args, &control).unwrap()["from_cache"], true);
        assert!(exec("web_fetch", &args, &TurnControl::with_web_scopes(vec!["domain:other.example".into()]))
            .unwrap_err()
            .contains("allowed domains"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn member_page_cache_evicts_by_recency_payload_bytes_and_expiration() {
        let cache = Fetcher::default();
        let now = Instant::now();
        let entry = |id, size| Cached {
            key: [id; 32],
            expires: now + TTL,
            page: Arc::new(Page { title: String::new(), content: "a".repeat(size), fetched_at: String::new() }),
        };
        for id in 0..32 {
            cache.remember(entry(id, 1)).unwrap();
        }
        assert!(cache.lookup(&[0; 32], now).unwrap().is_some());
        cache.remember(entry(32, 1)).unwrap();
        assert!(cache.lookup(&[1; 32], now).unwrap().is_none());
        assert!(cache.lookup(&[0; 32], now).unwrap().is_some());
        cache.remember(entry(33, BODY_LIMIT)).unwrap();
        assert!(cache.lookup(&[0; 32], now).unwrap().is_none());
        assert!(cache.lookup(&[33; 32], now + TTL).unwrap().is_none());
    }

    #[test]
    fn fetch_fallback_is_transient_only_and_explicit_selection_is_final() {
        let _lock = crate::env_lock();
        let _keys = Keys::set();
        let control = TurnControl::with_web_scopes(vec!["session".to_string()]);
        let body = json!({"content":"result"}).to_string();
        for status in [408, 429, 503] {
            let (endpoint, server) = serve(vec![(status, "reason".into()), (200, body.clone())]);
            let result = executor(&endpoint, None)("web_fetch", &json!({"url":URL}), &control).unwrap();
            let wire = server.join().unwrap();
            assert!(wire[0].starts_with("POST /exa "));
            assert!(wire[1].starts_with("POST /deepseek "));
            assert!(
                wire[1].to_lowercase().contains("authorization: bearer fixture-deepseek")
                    && !wire[1].contains("fixture-exa")
            );
            assert_eq!(
                serde_json::from_str::<Json>(wire[1].split_once("\r\n\r\n").unwrap().1).unwrap(),
                json!({"url":URL})
            );
            assert_eq!(result["provider"], "deepseek");
            assert_eq!(result["warnings"], json!([format!("exa: HTTP {status}")]));
        }
        for (selection, status) in
            [("exa", 503), ("auto", 401), ("auto", 403), ("auto", 302), ("auto", 400), ("auto", 200)]
        {
            let (endpoint, server) = serve(vec![(status, "invalid".into())]);
            let error =
                executor(&endpoint, None)("web_fetch", &json!({"url":URL,"provider":selection}), &control).unwrap_err();
            assert!(error.contains("exa:") && !error.contains("deepseek"), "{error}");
            assert!(server.join().unwrap()[0].starts_with("POST /exa "));
        }
        let (endpoint, server) = serve(vec![(200, body)]);
        let exec = executor(&endpoint, None);
        assert!(exec("web_fetch", &json!({"url":"http://127.0.0.1/"}), &control).unwrap_err().contains("private"));
        assert!(exec("web_fetch", &json!({"url":URL,"provider":"both"}), &control).is_err());
        assert_eq!(
            exec("web_fetch", &json!({"url":URL,"provider":"deepseek"}), &control).unwrap()["provider"],
            "deepseek"
        );
        assert!(server.join().unwrap()[0].starts_with("POST /deepseek "));
    }
}
