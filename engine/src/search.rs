//! Provider-neutral web search: DeepSeek and Exa, ranked and deduplicated.
//!
//! A `[tools.*] kind = "web_search"` binding authorizes one provider. The
//! model-facing `provider` argument selects among those bindings; `auto` uses
//! every bound provider and `both` requires DeepSeek and Exa to be bound.
//! Fetch remains in `tools.rs` and is intentionally not part of this module.

use serde_json::{json, Value as Json};
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use teamagents_core::models::ToolBinding;

const DEEPSEEK_SEARCH_URL: &str = "https://api.deepseek.com/v1/tools/search";
const EXA_SEARCH_URL: &str = "https://api.exa.ai/search";
const MAX_QUERY_CHARS: usize = 128;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 64 * 1024;
const MAX_CONTENT_CHARS: usize = 200_000;
const MAX_SNIPPET_CHARS: usize = 800;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(20);
const SEARCH_RETRIES: usize = 2;
const CACHE_TTL: Duration = Duration::from_secs(300);
const CACHE_MAX_ENTRIES: usize = 64;
const RRF_K: f64 = 60.0;

// ponytail: retry/cache timings are measured defaults, not user configuration.
// Add typed binding fields only if a real workload shows that one global policy
// is insufficient; the untyped binding.env map is not a second config surface.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum SearchProvider {
    DeepSeek,
    Exa,
}

impl SearchProvider {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "deepseek" => Some(Self::DeepSeek),
            "exa" => Some(Self::Exa),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::DeepSeek => "deepseek",
            Self::Exa => "exa",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::DeepSeek => "DeepSeek",
            Self::Exa => "Exa",
        }
    }

    fn default_url(self) -> &'static str {
        match self {
            Self::DeepSeek => DEEPSEEK_SEARCH_URL,
            Self::Exa => EXA_SEARCH_URL,
        }
    }

    fn default_key_env(self) -> &'static str {
        match self {
            Self::DeepSeek => "DEEPSEEK_API_KEY",
            Self::Exa => "EXA_API_KEY",
        }
    }
}

#[derive(Debug, Clone)]
struct SearchError {
    kind: &'static str,
    status: Option<u16>,
    retryable: bool,
    message: String,
}

impl SearchError {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self { kind, status: None, retryable: false, message: message.into() }
    }

    fn retryable(kind: &'static str, message: impl Into<String>) -> Self {
        Self { kind, status: None, retryable: true, message: message.into() }
    }

    fn status(provider: SearchProvider, status: u16, body: &str) -> Self {
        let detail = error_detail(body);
        let suffix = if detail.is_empty() { String::new() } else { format!(": {detail}") };
        let (kind, retryable, message) = match status {
            401 | 403 => (
                "auth",
                false,
                format!(
                    "{} rejected the credentials{suffix}; check the binding's api_key_env (normally {})",
                    provider.label(),
                    provider.default_key_env()
                ),
            ),
            402 => ("rate_limit", false, format!("{} credits or spending budget exhausted{suffix}", provider.label())),
            429 => ("rate_limit", true, format!("{} rate limit reached{suffix}", provider.label())),
            408 => ("timeout", true, format!("{} request timed out{suffix}", provider.label())),
            400 | 404 | 413 | 422 => {
                ("invalid_input", false, format!("{} rejected the search request{suffix}", provider.label()))
            }
            500..=599 => ("http", true, format!("{} server error {status}{suffix}", provider.label())),
            _ => ("http", false, format!("{} returned HTTP {status}{suffix}", provider.label())),
        };
        Self { kind, status: Some(status), retryable, message }
    }

    fn render(&self, provider: SearchProvider) -> String {
        let status = self.status.map(|code| format!(" (HTTP {code})")).unwrap_or_default();
        let retry = if self.retryable { "; the failure looks transient" } else { "" };
        format!("{}: [{}]{status} {}{retry}", provider.name(), self.kind, self.message)
    }
}

fn error_detail(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "empty response body".into();
    }
    if let Ok(payload) = serde_json::from_str::<Json>(trimmed) {
        let nested = payload.get("error");
        let message = nested
            .and_then(|error| error.get("message"))
            .and_then(Json::as_str)
            .or_else(|| payload.get("message").and_then(Json::as_str));
        if let Some(message) = message {
            return message.chars().take(300).collect();
        }
    }
    trimmed.chars().take(300).collect()
}

#[derive(Debug, Clone)]
struct SearchInput {
    query: String,
    count: usize,
    include_content: bool,
    ori_question: Option<String>,
    selection: String,
}

impl SearchInput {
    fn from_args(args: &Json) -> Result<Self, SearchError> {
        let query = args.get("query").and_then(Json::as_str).unwrap_or("").trim().to_string();
        let query_chars = query.chars().count();
        if query_chars == 0 {
            return Err(SearchError::new("invalid_input", "query must not be empty"));
        }
        if query_chars > MAX_QUERY_CHARS {
            return Err(SearchError::new(
                "invalid_input",
                format!("query must be at most {MAX_QUERY_CHARS} characters (got {query_chars})"),
            ));
        }
        let count = match args.get("max_results") {
            None | Some(Json::Null) => 10,
            Some(value) => {
                let number = value.as_f64().filter(|number| number.is_finite()).ok_or_else(|| {
                    SearchError::new("invalid_input", "max_results must be a number between 1 and 50")
                })?;
                number.trunc().clamp(1.0, 50.0) as usize
            }
        };
        let include_content = match args.get("include_content") {
            None | Some(Json::Null) => false,
            Some(Json::Bool(value)) => *value,
            Some(_) => return Err(SearchError::new("invalid_input", "include_content must be a boolean")),
        };
        let ori_question = match args.get("ori_question") {
            None | Some(Json::Null) => None,
            Some(Json::String(value)) => {
                let trimmed = value.trim();
                if trimmed.chars().count() > MAX_QUERY_CHARS {
                    return Err(SearchError::new(
                        "invalid_input",
                        format!("ori_question must be at most {MAX_QUERY_CHARS} characters"),
                    ));
                }
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            Some(_) => return Err(SearchError::new("invalid_input", "ori_question must be a string")),
        };
        let selection = match args.get("provider") {
            None | Some(Json::Null) => "auto".to_string(),
            Some(Json::String(value)) => value.clone(),
            Some(_) => return Err(SearchError::new("invalid_input", "provider must be a string")),
        };
        if !matches!(selection.as_str(), "auto" | "both" | "deepseek" | "exa") {
            return Err(SearchError::new("invalid_input", "provider must be auto, both, deepseek, or exa"));
        }
        Ok(Self { query, count, include_content, ori_question, selection })
    }
}

#[derive(Debug, Clone)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
    summary: String,
    content: Option<String>,
    site_name: Json,
    date: Json,
    authority: Json,
    rerank_score: Json,
    providers: Vec<SearchProvider>,
}

impl SearchResult {
    fn into_json(self) -> Json {
        let providers: Vec<Json> = self.providers.iter().map(|provider| json!(provider.name())).collect();
        let provider = self.providers.first().map(|provider| provider.name()).unwrap_or("deepseek");
        let mut entry = json!({
            "title": self.title,
            "url": self.url,
            "snippet": self.snippet,
            "summary": self.summary,
            "site_name": self.site_name,
            "date": self.date,
            "authority": self.authority,
            "rerank_score": self.rerank_score,
            "provider": provider,
            "providers": providers,
            "fetched_at": crate::tools::iso_now(),
        });
        if let Some(content) = self.content {
            entry["content"] = Json::String(content);
        }
        entry
    }
}

fn string_field(item: &Json, name: &str) -> String {
    item.get(name).and_then(Json::as_str).unwrap_or("").to_string()
}

fn parse_deepseek_response(payload: &Json, include_content: bool) -> Result<Vec<SearchResult>, SearchError> {
    let rows = payload
        .get("results")
        .and_then(Json::as_array)
        .ok_or_else(|| SearchError::new("invalid_response", "DeepSeek search response is missing a results array"))?;
    let mut results = Vec::new();
    for item in rows {
        let url = string_field(item, "url");
        if url.trim().is_empty() {
            continue;
        }
        let title = {
            let title = string_field(item, "title");
            if title.is_empty() {
                url.clone()
            } else {
                title
            }
        };
        results.push(SearchResult {
            title,
            url,
            snippet: string_field(item, "snippet"),
            summary: string_field(item, "summary"),
            content: include_content.then(|| string_field(item, "content")),
            site_name: item.get("site_name").cloned().unwrap_or(Json::Null),
            date: item.get("date").cloned().unwrap_or(Json::Null),
            authority: item.get("authority").cloned().unwrap_or(Json::Null),
            rerank_score: item.get("rerank_score").cloned().unwrap_or(Json::Null),
            providers: vec![SearchProvider::DeepSeek],
        });
    }
    Ok(results)
}

fn parse_exa_response(payload: &Json, include_content: bool) -> Result<Vec<SearchResult>, SearchError> {
    let rows = payload
        .get("results")
        .and_then(Json::as_array)
        .ok_or_else(|| SearchError::new("invalid_response", "Exa search response is missing a results array"))?;
    let mut results = Vec::new();
    for item in rows {
        let url = string_field(item, "url");
        if url.trim().is_empty() {
            continue;
        }
        let title = {
            let title = string_field(item, "title");
            if title.is_empty() {
                url.clone()
            } else {
                title
            }
        };
        let summary = string_field(item, "summary");
        let highlights = item
            .get("highlights")
            .and_then(Json::as_array)
            .map(|rows| rows.iter().filter_map(Json::as_str).collect::<Vec<_>>().join("\n\n"))
            .unwrap_or_default();
        let snippet = if summary.is_empty() { highlights } else { summary.clone() };
        let site_name = url::Url::parse(&url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_string))
            .map(Json::String)
            .unwrap_or(Json::Null);
        results.push(SearchResult {
            title,
            url,
            snippet,
            summary,
            content: include_content.then(|| string_field(item, "text")),
            site_name,
            date: item.get("publishedDate").cloned().unwrap_or(Json::Null),
            authority: Json::Null,
            rerank_score: Json::Null,
            providers: vec![SearchProvider::Exa],
        });
    }
    Ok(results)
}

fn read_body(response: ureq::Response, limit: usize) -> Result<Vec<u8>, SearchError> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| SearchError::retryable("network", format!("could not read search response: {error}")))?;
    if bytes.len() > limit {
        return Err(SearchError::new("invalid_response", format!("search response exceeded {limit} bytes")));
    }
    Ok(bytes)
}

fn retry_delay(attempt: usize, retry_after: Option<Duration>) -> Duration {
    let base = Duration::from_millis(350u64.saturating_mul(1u64 << attempt.min(4))).min(Duration::from_secs(5));
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos();
    let jitter = Duration::from_millis(u64::from(nanos % 250));
    retry_after.unwrap_or(base).min(Duration::from_secs(5)) + jitter
}

fn post_search(provider: SearchProvider, endpoint: &str, credential: &str, body: &Json) -> Result<Json, SearchError> {
    let payload = body.to_string();
    let mut last_error = None;
    for attempt in 0..=SEARCH_RETRIES {
        let agent = ureq::AgentBuilder::new().redirects(0).build();
        let request = agent.post(endpoint).timeout(SEARCH_TIMEOUT).set("content-type", "application/json");
        let request = match provider {
            SearchProvider::DeepSeek => request.set("authorization", &format!("Bearer {credential}")),
            SearchProvider::Exa => request.set("x-api-key", credential),
        };
        match request.send_string(&payload) {
            Ok(response) => {
                let bytes = match read_body(response, MAX_RESPONSE_BYTES) {
                    Ok(bytes) => bytes,
                    Err(error) if error.retryable && attempt < SEARCH_RETRIES => {
                        last_error = Some(error);
                        std::thread::sleep(retry_delay(attempt, None));
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                return serde_json::from_slice(&bytes).map_err(|error| {
                    SearchError::new(
                        "invalid_response",
                        format!("{} returned malformed JSON: {error}", provider.label()),
                    )
                });
            }
            Err(ureq::Error::Status(status, response)) => {
                let retry_after = response
                    .header("retry-after")
                    .and_then(|value| value.trim().parse::<u64>().ok())
                    .map(Duration::from_secs);
                // ponytail: Retry-After's HTTP-date form needs a date parser;
                // numeric seconds are honored now, without another dependency.
                let body = read_body(response, MAX_ERROR_BYTES).unwrap_or_default();
                let error = SearchError::status(provider, status, &String::from_utf8_lossy(&body));
                if !error.retryable || attempt == SEARCH_RETRIES {
                    return Err(error);
                }
                last_error = Some(error);
                std::thread::sleep(retry_delay(attempt, retry_after));
            }
            Err(ureq::Error::Transport(transport)) => {
                use std::error::Error as _;
                let timed_out = transport
                    .source()
                    .and_then(|source| source.downcast_ref::<std::io::Error>())
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::TimedOut);
                let kind = if timed_out { "timeout" } else { "network" };
                let permanent = matches!(
                    transport.kind(),
                    ureq::ErrorKind::InvalidUrl
                        | ureq::ErrorKind::UnknownScheme
                        | ureq::ErrorKind::InsecureRequestHttpsOnly
                        | ureq::ErrorKind::TooManyRedirects
                        | ureq::ErrorKind::InvalidProxyUrl
                        | ureq::ErrorKind::ProxyUnauthorized
                );
                let error = if permanent {
                    SearchError::new("config", format!("invalid search endpoint or proxy: {transport}"))
                } else {
                    SearchError::retryable(kind, format!("could not contact {}: {transport}", provider.label()))
                };
                if permanent || attempt == SEARCH_RETRIES {
                    return Err(error);
                }
                last_error = Some(error);
                std::thread::sleep(retry_delay(attempt, None));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| SearchError::new("network", "search failed without a response")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheSource {
    Cache,
    Coalesced,
    Network,
}

impl CacheSource {
    fn name(self) -> &'static str {
        match self {
            Self::Cache => "cache",
            Self::Coalesced => "coalesced",
            Self::Network => "network",
        }
    }
}

#[derive(Debug, Clone)]
struct ProviderOutcome {
    results: Vec<SearchResult>,
    source: CacheSource,
}

type FlightResult = Result<Vec<SearchResult>, SearchError>;

#[derive(Debug, Default)]
struct Flight {
    result: Mutex<Option<FlightResult>>,
    ready: Condvar,
}

#[derive(Debug)]
struct CacheEntry {
    results: Vec<SearchResult>,
    expires_at: Instant,
}

#[derive(Debug, Default)]
struct SearchCache {
    entries: HashMap<String, CacheEntry>,
    lru: VecDeque<String>,
    inflight: HashMap<String, std::sync::Arc<Flight>>,
}

enum CacheLookup {
    Hit(Vec<SearchResult>),
    Wait(std::sync::Arc<Flight>),
    Load(std::sync::Arc<Flight>),
}

fn cache() -> &'static Mutex<SearchCache> {
    static CACHE: OnceLock<Mutex<SearchCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(SearchCache::default()))
}

fn begin_cached(key: &str) -> CacheLookup {
    let mut cache = cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    if cache.entries.get(key).is_some_and(|entry| entry.expires_at <= now) {
        cache.entries.remove(key);
        cache.lru.retain(|candidate| candidate != key);
    }
    if let Some(entry) = cache.entries.get(key) {
        let results = entry.results.clone();
        cache.lru.retain(|candidate| candidate != key);
        cache.lru.push_back(key.to_string());
        return CacheLookup::Hit(results);
    }
    if let Some(flight) = cache.inflight.get(key) {
        return CacheLookup::Wait(flight.clone());
    }
    let flight = std::sync::Arc::new(Flight::default());
    cache.inflight.insert(key.to_string(), flight.clone());
    CacheLookup::Load(flight)
}

fn finish_cached(key: &str, flight: &std::sync::Arc<Flight>, result: FlightResult) {
    let mut cache = cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if cache.inflight.get(key).is_some_and(|current| std::sync::Arc::ptr_eq(current, flight)) {
        cache.inflight.remove(key);
    }
    if let Ok(results) = &result {
        cache.lru.retain(|candidate| candidate != key);
        cache.lru.push_back(key.to_string());
        cache
            .entries
            .insert(key.to_string(), CacheEntry { results: results.clone(), expires_at: Instant::now() + CACHE_TTL });
        while cache.entries.len() > CACHE_MAX_ENTRIES {
            let Some(oldest) = cache.lru.pop_front() else { break };
            cache.entries.remove(&oldest);
        }
    }
    *flight.result.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
    flight.ready.notify_all();
}

fn wait_cached(flight: &std::sync::Arc<Flight>) -> FlightResult {
    let mut result = flight.result.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    while result.is_none() {
        result = flight.ready.wait(result).unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    result.clone().expect("a completed flight has a result")
}

fn search_binding(name: &str, binding: &ToolBinding, input: &SearchInput) -> Result<ProviderOutcome, SearchError> {
    let provider = SearchProvider::parse(binding.provider.as_deref().unwrap_or("deepseek"))
        .ok_or_else(|| SearchError::new("config", format!("binding {name:?} has an unsupported search provider")))?;
    let credential =
        crate::tools::web_credential(provider.name(), binding.api_key_env.as_deref()).map_err(|reason| {
            SearchError::new(
                "config",
                format!(
                    "{reason}; configure provider = \"{}\" and api_key_env = \"{}\"",
                    provider.name(),
                    provider.default_key_env()
                ),
            )
        })?;
    let endpoint = binding.url.as_deref().unwrap_or_else(|| provider.default_url());
    let key = json!({
        "binding": name,
        "provider": provider.name(),
        "endpoint": endpoint,
        "query": input.query,
        "count": input.count,
        "include_content": input.include_content,
        "ori_question": input.ori_question,
    })
    .to_string();
    match begin_cached(&key) {
        CacheLookup::Hit(results) => Ok(ProviderOutcome { results, source: CacheSource::Cache }),
        CacheLookup::Wait(flight) => {
            wait_cached(&flight).map(|results| ProviderOutcome { results, source: CacheSource::Coalesced })
        }
        CacheLookup::Load(flight) => {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let body = match provider {
                    SearchProvider::DeepSeek => {
                        let mut body = json!({
                            "query": input.query,
                            "count": input.count,
                            "need_content": input.include_content,
                        });
                        if let Some(original) = &input.ori_question {
                            body["ori_question"] = Json::String(original.clone());
                        }
                        body
                    }
                    SearchProvider::Exa => {
                        let mut highlights = json!({ "maxCharacters": MAX_SNIPPET_CHARS });
                        if let Some(original) = &input.ori_question {
                            highlights["query"] = Json::String(original.clone());
                        }
                        let mut contents = json!({ "highlights": highlights });
                        if input.include_content {
                            contents["text"] = json!({ "maxCharacters": MAX_CONTENT_CHARS });
                        }
                        json!({
                            "query": input.query,
                            "numResults": input.count,
                            "type": binding.env.get("search_type").map(String::as_str).unwrap_or("auto"),
                            "contents": contents,
                        })
                    }
                };
                let payload = post_search(provider, endpoint, &credential, &body)?;
                match provider {
                    SearchProvider::DeepSeek => parse_deepseek_response(&payload, input.include_content),
                    SearchProvider::Exa => parse_exa_response(&payload, input.include_content),
                }
            }))
            .unwrap_or_else(|_| Err(SearchError::new("invalid_response", "search worker failed unexpectedly")));
            finish_cached(&key, &flight, result.clone());
            result.map(|results| ProviderOutcome { results, source: CacheSource::Network })
        }
    }
}

fn canonical_result_url(raw: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(raw) else { return raw.to_string() };
    parsed.set_fragment(None);
    let path = parsed.path().strip_suffix('/').unwrap_or(parsed.path()).to_string();
    parsed.set_path(&path);
    let rendered = parsed.to_string();
    if parsed.path() == "/" && parsed.query().is_none() {
        rendered.strip_suffix('/').unwrap_or(&rendered).to_string()
    } else {
        rendered
    }
}

fn merge_results(outcomes: &[(SearchProvider, ProviderOutcome)], limit: usize) -> Vec<SearchResult> {
    struct Entry {
        result: SearchResult,
        score: f64,
        providers: Vec<SearchProvider>,
        first_seen: usize,
    }
    let mut entries: Vec<Entry> = Vec::new();
    let mut indices: HashMap<String, usize> = HashMap::new();
    for (provider, outcome) in outcomes {
        for (rank, result) in outcome.results.iter().enumerate() {
            let key = canonical_result_url(&result.url);
            let points = 1.0 / (RRF_K + rank as f64 + 1.0);
            if let Some(index) = indices.get(&key).copied() {
                let entry = &mut entries[index];
                if !entry.providers.contains(provider) {
                    entry.score += points;
                    entry.providers.push(*provider);
                    entry.providers.sort();
                }
                if entry.result.snippet.is_empty() {
                    entry.result.snippet = result.snippet.clone();
                }
                if entry.result.summary.is_empty() {
                    entry.result.summary = result.summary.clone();
                }
                if entry.result.content.as_deref().unwrap_or("").is_empty() {
                    entry.result.content = result.content.clone();
                }
                if entry.result.date.is_null() {
                    entry.result.date = result.date.clone();
                }
                if entry.result.authority.is_null() {
                    entry.result.authority = result.authority.clone();
                }
                if entry.result.rerank_score.is_null() {
                    entry.result.rerank_score = result.rerank_score.clone();
                }
                entry.result.providers = entry.providers.clone();
            } else {
                let mut result = result.clone();
                result.providers = vec![*provider];
                indices.insert(key, entries.len());
                entries.push(Entry { result, score: points, providers: vec![*provider], first_seen: entries.len() });
            }
        }
    }
    entries
        .sort_by(|left, right| right.score.total_cmp(&left.score).then_with(|| left.first_seen.cmp(&right.first_seen)));
    entries.truncate(limit.max(1));
    entries.into_iter().map(|entry| entry.result).collect()
}

/// Execute one model-facing `web_search` call against its authorized bindings.
pub fn web_search(bindings: &[(String, ToolBinding)], args: &Json) -> Result<Json, String> {
    let input = SearchInput::from_args(args).map_err(|error| format!("web_search: {}", error.message))?;
    let mut available: Vec<(String, ToolBinding, SearchProvider)> = Vec::new();
    for (name, binding) in bindings {
        let provider = SearchProvider::parse(binding.provider.as_deref().unwrap_or("deepseek"))
            .ok_or_else(|| format!("tool service {name:?}: unsupported web provider {:?}", binding.provider))?;
        if !available.iter().any(|(_, _, current)| *current == provider) {
            available.push((name.clone(), binding.clone(), provider));
        }
    }
    available.sort_by_key(|(_, _, provider)| *provider);
    let selected: Vec<(String, ToolBinding, SearchProvider)> = match input.selection.as_str() {
        "auto" => available,
        "both" => {
            for required in [SearchProvider::DeepSeek, SearchProvider::Exa] {
                if !available.iter().any(|(_, _, provider)| *provider == required) {
                    return Err(format!(
                        "web_search: provider=\"both\" needs DeepSeek and Exa bindings; missing {} \
                         (api_key_env = \"{}\")",
                        required.name(),
                        required.default_key_env()
                    ));
                }
            }
            available
        }
        explicit => {
            let provider = SearchProvider::parse(explicit).expect("the selection was validated");
            available.into_iter().filter(|(_, _, current)| *current == provider).collect()
        }
    };
    if selected.is_empty() {
        return Err(format!(
            "web_search is unavailable: no {} binding is configured; add a [tools.*] web_search binding and its api_key_env",
            input.selection
        ));
    }

    let run = |(name, binding, provider): &(String, ToolBinding, SearchProvider)| {
        let outcome = search_binding(name, binding, &input);
        (*provider, outcome)
    };
    let settled: Vec<(SearchProvider, Result<ProviderOutcome, SearchError>)> = if selected.len() == 1 {
        vec![run(&selected[0])]
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = selected
                .iter()
                .map(|binding| {
                    let provider = binding.2;
                    (provider, scope.spawn(move || run(binding)))
                })
                .collect();
            handles
                .into_iter()
                .map(|(provider, handle)| {
                    handle.join().unwrap_or_else(|_| {
                        (provider, Err(SearchError::new("invalid_response", "search worker failed unexpectedly")))
                    })
                })
                .collect()
        })
    };

    let mut successes: Vec<(SearchProvider, ProviderOutcome)> = Vec::new();
    let mut failures: Vec<(SearchProvider, SearchError)> = Vec::new();
    for (provider, result) in settled {
        match result {
            Ok(outcome) => successes.push((provider, outcome)),
            Err(error) => failures.push((provider, error)),
        }
    }
    if successes.is_empty() {
        let details = failures.iter().map(|(provider, error)| error.render(*provider)).collect::<Vec<_>>().join("; ");
        return Err(format!("web_search failed: {details}"));
    }
    successes.sort_by_key(|(provider, _)| *provider);
    let providers: Vec<Json> = successes.iter().map(|(provider, _)| json!(provider.name())).collect();
    let legacy_provider = successes.iter().map(|(provider, _)| provider.name()).collect::<Vec<_>>().join("+");
    let from_cache = successes.iter().all(|(_, outcome)| outcome.source == CacheSource::Cache);
    let cache_sources: Json = successes
        .iter()
        .map(|(provider, outcome)| (provider.name().to_string(), json!(outcome.source.name())))
        .collect();
    let results = merge_results(&successes, input.count).into_iter().map(SearchResult::into_json).collect::<Vec<_>>();
    let mut answer = json!({
        "query": input.query,
        "provider": legacy_provider,
        "providers": providers,
        "from_cache": from_cache,
        "cache_sources": cache_sources,
        "results": results,
    });
    if !failures.is_empty() {
        answer["warnings"] = Json::Array(
            failures
                .iter()
                .map(|(provider, error)| json!({"provider": provider.name(), "error": error.render(*provider)}))
                .collect(),
        );
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Barrier};

    fn binding(name: &str, provider: &str, url: &str, key_env: Option<&str>) -> (String, ToolBinding) {
        let mut value = json!({"kind": "web_search", "provider": provider, "url": url});
        if let Some(key_env) = key_env {
            value["api_key_env"] = Json::String(key_env.into());
        }
        (name.into(), serde_json::from_value(value).unwrap())
    }

    fn args(query: &str) -> Json {
        json!({"query": query, "max_results": 10})
    }

    fn request_body(request: &str) -> Json {
        let body = request.split("\r\n\r\n").nth(1).unwrap_or("{}");
        serde_json::from_str(body)
            .unwrap_or_else(|error| panic!("bad request body {body:?}: {error}; request={request:?}"))
    }

    /// Serve a fixed number of local responses and capture complete request heads/bodies.
    fn server(responses: Vec<(u16, Json)>, delay: Duration) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        std::thread::spawn(move || {
            for (status, payload) in responses {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0u8; 4096];
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let complete = request.windows(4).position(|window| window == b"\r\n\r\n").is_some_and(|head| {
                        let headers = String::from_utf8_lossy(&request[..head]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        request.len() >= head + 4 + length
                    });
                    if complete || request.len() >= 64 * 1024 {
                        break;
                    }
                }
                record.lock().unwrap().push(String::from_utf8_lossy(&request).into_owned());
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                let body = payload.to_string();
                let reason = if status == 200 { "OK" } else { "ERROR" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\nretry-after: 0\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (url, seen)
    }

    #[test]
    fn input_validation_is_bounded_before_any_provider_is_called() {
        for args in [
            json!({"query": "   "}),
            json!({"query": "x".repeat(129)}),
            json!({"query": "😀".repeat(129)}),
            json!({"query": "x", "ori_question": "q".repeat(129)}),
            json!({"query": "x", "provider": "unknown"}),
            json!({"query": "x", "max_results": "many"}),
            json!({"query": "x", "include_content": "yes"}),
        ] {
            assert!(web_search(&[], &args).unwrap_err().contains("web_search:"), "args={args}");
        }
        let normalized = SearchInput::from_args(&json!({
            "query": "  hello  ", "max_results": 7.9, "ori_question": "   "
        }))
        .unwrap();
        assert_eq!(normalized.query, "hello");
        assert_eq!(normalized.count, 7);
        assert_eq!(normalized.ori_question, None);
        assert_eq!(SearchInput::from_args(&json!({"query": "x", "max_results": 0})).unwrap().count, 1);
        assert_eq!(SearchInput::from_args(&json!({"query": "x", "max_results": 99})).unwrap().count, 50);
    }

    #[test]
    fn auto_search_posts_both_shapes_and_merges_with_reciprocal_rank_fusion() {
        let _env = crate::env_lock();
        std::env::set_var("TA_D482_DS", "ds-test-key");
        std::env::set_var("TA_D482_EXA", "exa-test-key");
        let (deepseek_url, deepseek_seen) = server(
            vec![(
                200,
                json!({"results": [
                    {"title": "A", "url": "https://a.example/", "snippet": "deepseek A", "authority": 4},
                    {"title": "B", "url": "https://b.example/"}
                ]}),
            )],
            Duration::ZERO,
        );
        let (exa_url, exa_seen) = server(
            vec![(
                200,
                json!({"results": [
                    {"title": "A from Exa", "url": "https://a.example", "highlights": ["exa A"], "text": "BODY"},
                    {"title": "C", "url": "https://c.example/", "summary": "exa C", "publishedDate": "2026-01-02"}
                ]}),
            )],
            Duration::ZERO,
        );
        let bindings = vec![
            binding("ds-auto", "deepseek", &deepseek_url, Some("TA_D482_DS")),
            binding("exa-auto", "exa", &exa_url, Some("TA_D482_EXA")),
        ];
        let result = web_search(
            &bindings,
            &json!({"query": "pi", "max_results": 10, "include_content": true,
                    "ori_question": "original question", "provider": "auto"}),
        )
        .unwrap();
        let both = web_search(
            &bindings,
            &json!({"query": "pi", "max_results": 10, "include_content": true,
                    "ori_question": "original question", "provider": "both"}),
        )
        .unwrap();
        assert_eq!(both["from_cache"], true, "both reuses the two provider caches without new upstream calls");
        std::env::remove_var("TA_D482_DS");
        std::env::remove_var("TA_D482_EXA");
        assert_eq!(result["provider"], "deepseek+exa");
        assert_eq!(result["providers"], json!(["deepseek", "exa"]));
        assert_eq!(result["results"].as_array().unwrap().len(), 3);
        assert_eq!(result["results"][0]["url"], "https://a.example/");
        assert_eq!(result["results"][0]["providers"], json!(["deepseek", "exa"]));
        assert_eq!(result["results"][0]["snippet"], "deepseek A");
        assert_eq!(result["results"][0]["content"], "BODY");
        assert_eq!(result["results"][1]["url"], "https://b.example/");
        assert_eq!(result["results"][2]["site_name"], "c.example");

        let deepseek_requests = deepseek_seen.lock().unwrap();
        assert_eq!(deepseek_requests.len(), 1);
        assert!(deepseek_requests[0].to_lowercase().contains("authorization: bearer ds-test-key"));
        assert_eq!(
            request_body(&deepseek_requests[0]),
            json!({"query": "pi", "count": 10, "need_content": true, "ori_question": "original question"})
        );
        let exa_requests = exa_seen.lock().unwrap();
        assert_eq!(exa_requests.len(), 1);
        assert!(exa_requests[0].to_lowercase().contains("x-api-key: exa-test-key"));
        assert_eq!(
            request_body(&exa_requests[0]),
            json!({"query": "pi", "numResults": 10, "type": "auto", "contents": {
                "highlights": {"maxCharacters": 800, "query": "original question"},
                "text": {"maxCharacters": 200000}
            }})
        );
    }

    #[test]
    fn auto_keeps_partial_results_and_explicit_selection_fails_closed() {
        let _env = crate::env_lock();
        std::env::set_var("TA_D482_FALLBACK", "test-key");
        let (url, seen) = server(
            vec![(200, json!({"results": [{"title": "available", "url": "https://ok.example/"}]}))],
            Duration::ZERO,
        );
        let deepseek = binding("ds-fallback", "deepseek", &url, Some("TA_D482_FALLBACK"));
        let exa = binding("exa-fallback", "exa", "http://127.0.0.1:9", Some("TA_D482_UNSET_EXA"));
        let bindings = vec![deepseek.clone(), exa];
        let result = web_search(&bindings, &args("fallback")).unwrap();
        assert_eq!(result["providers"], json!(["deepseek"]));
        assert_eq!(result["results"][0]["title"], "available");
        assert_eq!(result["warnings"][0]["provider"], "exa");
        assert!(result["warnings"][0]["error"].as_str().unwrap().contains("TA_D482_UNSET_EXA"));
        assert_eq!(seen.lock().unwrap().len(), 1);

        let error =
            web_search(std::slice::from_ref(&deepseek), &json!({"query": "fallback", "provider": "both"})).unwrap_err();
        assert!(error.contains("missing exa"), "{error}");
        let error =
            web_search(std::slice::from_ref(&deepseek), &json!({"query": "fallback", "provider": "exa"})).unwrap_err();
        assert!(error.contains("no exa binding"), "{error}");
        std::env::remove_var("TA_D482_FALLBACK");
    }

    #[test]
    fn explicit_exa_does_not_call_deepseek() {
        let _env = crate::env_lock();
        std::env::set_var("TA_D482_ONLY_DS", "test-key");
        std::env::set_var("TA_D482_ONLY_EXA", "test-key");
        let (deepseek_url, deepseek_seen) = server(vec![], Duration::ZERO);
        let (exa_url, exa_seen) = server(
            vec![(200, json!({"results": [{"title": "Exa only", "url": "https://exa-only.example/"}]}))],
            Duration::ZERO,
        );
        let bindings = vec![
            binding("ds-only", "deepseek", &deepseek_url, Some("TA_D482_ONLY_DS")),
            binding("exa-only", "exa", &exa_url, Some("TA_D482_ONLY_EXA")),
        ];
        let result = web_search(&bindings, &json!({"query": "only-exa", "provider": "exa"})).unwrap();
        assert_eq!(result["providers"], json!(["exa"]));
        assert_eq!(result["results"][0]["title"], "Exa only");
        assert_eq!(deepseek_seen.lock().unwrap().len(), 0);
        assert_eq!(exa_seen.lock().unwrap().len(), 1);
        std::env::remove_var("TA_D482_ONLY_DS");
        std::env::remove_var("TA_D482_ONLY_EXA");
    }

    #[test]
    fn transient_status_is_retried_and_success_is_cached() {
        let _env = crate::env_lock();
        std::env::set_var("TA_D482_RETRY", "test-key");
        let (url, seen) = server(
            vec![
                (429, json!({"error": {"message": "slow down"}})),
                (200, json!({"results": [{"title": "after retry", "url": "https://retry.example/"}]})),
            ],
            Duration::ZERO,
        );
        let bindings = vec![binding("ds-retry", "deepseek", &url, Some("TA_D482_RETRY"))];
        let first = web_search(&bindings, &args("retry-cache")).unwrap();
        assert_eq!(first["cache_sources"]["deepseek"], "network");
        assert_eq!(first["results"][0]["title"], "after retry");
        let second = web_search(&bindings, &args("retry-cache")).unwrap();
        assert_eq!(second["from_cache"], true);
        assert_eq!(second["cache_sources"]["deepseek"], "cache");
        assert_eq!(seen.lock().unwrap().len(), 2, "one retry, then a cache hit");
        std::env::remove_var("TA_D482_RETRY");
    }

    #[test]
    fn concurrent_identical_searches_share_one_upstream_request() {
        let _env = crate::env_lock();
        std::env::set_var("TA_D482_FLIGHT", "test-key");
        let (url, seen) = server(
            vec![(200, json!({"results": [{"title": "shared", "url": "https://shared.example/"}]}))],
            Duration::from_millis(150),
        );
        let bindings = Arc::new(vec![binding("ds-flight", "deepseek", &url, Some("TA_D482_FLIGHT"))]);
        let barrier = Arc::new(Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let bindings = bindings.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    web_search(&bindings, &args("single-flight")).unwrap()
                })
            })
            .collect();
        let results: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
        assert_eq!(seen.lock().unwrap().len(), 1);
        let sources: Vec<_> =
            results.iter().map(|result| result["cache_sources"]["deepseek"].as_str().unwrap()).collect();
        assert!(sources.contains(&"network"), "{sources:?}");
        assert!(sources.contains(&"coalesced"), "{sources:?}");
        std::env::remove_var("TA_D482_FLIGHT");
    }

    #[test]
    fn auth_and_malformed_responses_are_classified_without_retrying() {
        let _env = crate::env_lock();
        std::env::set_var("TA_D482_AUTH", "bad-key");
        let (auth_url, auth_seen) = server(vec![(401, json!({"error": {"message": "invalid key"}}))], Duration::ZERO);
        let auth = vec![binding("ds-auth", "deepseek", &auth_url, Some("TA_D482_AUTH"))];
        let error = web_search(&auth, &args("auth-error")).unwrap_err();
        assert!(error.contains("[auth]") && error.contains("HTTP 401"), "{error}");
        assert_eq!(auth_seen.lock().unwrap().len(), 1);

        let (bad_url, bad_seen) = server(vec![(200, json!({"results": "not-an-array"}))], Duration::ZERO);
        let malformed = vec![binding("ds-malformed", "deepseek", &bad_url, Some("TA_D482_AUTH"))];
        let error = web_search(&malformed, &args("malformed-error")).unwrap_err();
        assert!(error.contains("[invalid_response]"), "{error}");
        assert_eq!(bad_seen.lock().unwrap().len(), 1);
        std::env::remove_var("TA_D482_AUTH");
    }

    #[test]
    fn response_normalization_drops_unusable_rows_and_honors_content_choice() {
        let payload = json!({"results": [
            {"url": "https://a.example/", "content": "BODY"},
            {"title": "missing URL"},
            "garbage",
            {"title": "B", "url": "https://b.example/"}
        ]});
        let parsed = parse_deepseek_response(&payload, false).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].title, "https://a.example/");
        assert_eq!(parsed[0].content, None);
        let parsed = parse_deepseek_response(&payload, true).unwrap();
        assert_eq!(parsed[0].content.as_deref(), Some("BODY"));
    }

    #[test]
    fn duplicate_urls_from_one_provider_cannot_outweigh_provider_agreement() {
        let item = |url: &str, provider: SearchProvider| SearchResult {
            title: url.into(),
            url: url.into(),
            snippet: String::new(),
            summary: String::new(),
            content: None,
            site_name: Json::Null,
            date: Json::Null,
            authority: Json::Null,
            rerank_score: Json::Null,
            providers: vec![provider],
        };
        let outcomes = vec![
            (
                SearchProvider::DeepSeek,
                ProviderOutcome {
                    results: vec![
                        item("https://duplicate.example/", SearchProvider::DeepSeek),
                        item("https://duplicate.example/#part", SearchProvider::DeepSeek),
                        item("https://shared.example/", SearchProvider::DeepSeek),
                    ],
                    source: CacheSource::Network,
                },
            ),
            (
                SearchProvider::Exa,
                ProviderOutcome {
                    results: vec![item("https://shared.example", SearchProvider::Exa)],
                    source: CacheSource::Network,
                },
            ),
        ];
        let merged = merge_results(&outcomes, 10);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].url, "https://shared.example/");
        assert_eq!(merged[0].providers, vec![SearchProvider::DeepSeek, SearchProvider::Exa]);
        assert_eq!(canonical_result_url("https://Example.com/path/?x=1#part"), "https://example.com/path?x=1");
        assert_ne!(
            canonical_result_url("https://example.com/?path=/"),
            canonical_result_url("https://example.com/?path=")
        );
    }
}
