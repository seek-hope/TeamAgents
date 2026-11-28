//! Built-in and user-configured model providers (`providers.json`) without persisted secrets.
use super::{has_anthropic_only_env, has_deepseek_env, has_nonempty_env, is_local_host, valid_model_id};
use crate::{oauth, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fs,
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

const CONFIG_LIMIT: u64 = 256 * 1024;
pub(crate) const BUILTIN_PROVIDERS: &[&str] = &["deepseek", "chatgpt", "responses", "anthropic", "api"];
const RESERVED_NAMES: &[&str] =
    &["list", "add", "remove", "rm", "show", "default", "set", "effort", "provider", "providers", "use", "auto"];

/// Wire protocol family spoken by a provider endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WireProtocol {
    ChatCompletions,
    Responses,
    Anthropic,
    ChatGpt,
}

impl WireProtocol {
    /// Parse a wire protocol allowed for custom user-added providers.
    pub(crate) fn parse_custom(raw: &str) -> Result<Self> {
        match raw.trim() {
            "responses" => Ok(Self::Responses),
            "chat_completions" | "api" => Ok(Self::ChatCompletions),
            "anthropic" => Ok(Self::Anthropic),
            "chatgpt" => Err(
                "custom providers cannot use the chatgpt subscription wire; use responses, chat_completions, or anthropic"
                    .into(),
            ),
            other => {
                Err(format!("unsupported wire protocol {other:?}; expected responses, chat_completions, or anthropic"))
            }
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
            Self::Anthropic => "anthropic",
            Self::ChatGpt => "chatgpt",
        }
    }
}

/// Tool choice constraint sent when tool schemas are attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolChoiceMode {
    Required,
    Auto,
}

impl ToolChoiceMode {
    pub(crate) fn parse(raw: &str) -> Result<Self> {
        match raw.trim() {
            "required" => Ok(Self::Required),
            "auto" => Ok(Self::Auto),
            other => Err(format!("tool_choice must be required or auto (got {other:?})")),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Auto => "auto",
        }
    }
}

/// Per-provider entry stored under `providers` in `providers.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CustomProviderSpec {
    pub wire: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoiceMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_reasoning: Option<bool>,
}

/// Top-level `providers.json` configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProvidersConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    #[serde(default)]
    pub providers: BTreeMap<String, CustomProviderSpec>,
}

/// Runtime parameters resolved for either a built-in or custom provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedProvider {
    pub name: String,
    pub wire: WireProtocol,
    pub builtin: bool,
    pub base_url: Option<String>,
    pub url_label: String,
    pub api_key_env: Option<String>,
    pub default_model: Option<String>,
    pub context_window: Option<u64>,
    pub tool_choice: ToolChoiceMode,
    pub max_tokens: Option<u32>,
    pub replay_reasoning: bool,
}

/// Redacted provider descriptor returned over IPC / CLI / TUI (never contains keys).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ProviderInfo {
    pub name: String,
    pub wire: String,
    pub builtin: bool,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub default_model: Option<String>,
    pub context_window: Option<u64>,
    pub tool_choice: String,
    pub ready: bool,
    pub status: String,
}

/// Validate a custom provider identifier.
pub(crate) fn valid_provider_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=64).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes[1..].iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.'))
        && !RESERVED_NAMES.contains(&name)
}

/// Validate an environment variable name (never a literal secret value).
pub(crate) fn valid_env_var_name(name: &str) -> Result<()> {
    let bytes = name.as_bytes();
    let ok = (1..=128).contains(&bytes.len())
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes[1..].iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_');
    if !ok {
        return Err(format!(
            "invalid api_key_env {name:?}: pass an environment variable NAME (e.g. MY_API_KEY), not a raw API key"
        ));
    }
    Ok(())
}

fn validate_base_url(name: &str, base_url: &str) -> Result<bool> {
    if base_url.trim() != base_url || base_url.is_empty() {
        return Err(format!("invalid base_url for provider {name:?}"));
    }
    let parsed = url::Url::parse(base_url).map_err(|_| format!("invalid base_url for provider {name:?}"))?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(format!("base_url for provider {name:?} cannot contain userinfo, query, or fragment"));
    }
    let Some(host) = parsed.host_str().filter(|h| !h.is_empty()) else {
        return Err(format!("base_url for provider {name:?} must include a host"));
    };
    let local = is_local_host(host);
    if parsed.scheme() != "https" && !(local && parsed.scheme() == "http") {
        return Err(format!("base_url for provider {name:?} requires HTTPS except on loopback"));
    }
    Ok(local)
}

/// Validate a custom provider name and specification before saving or running.
pub(crate) fn validate_custom_spec(name: &str, spec: &CustomProviderSpec) -> Result<()> {
    if BUILTIN_PROVIDERS.contains(&name) {
        return Err(format!("provider {name:?} is built-in and cannot be overridden"));
    }
    if !valid_provider_name(name) {
        return Err(format!(
            "invalid provider name {name:?}: use 1-64 lowercase ASCII letters/digits plus '-', '_' or '.', starting with [a-z0-9]"
        ));
    }
    WireProtocol::parse_custom(&spec.wire)?;
    let local = validate_base_url(name, &spec.base_url)?;
    match spec.api_key_env.as_deref() {
        Some(env_name) => valid_env_var_name(env_name)?,
        None if local => {}
        None => return Err(format!("non-loopback provider {name:?} requires api_key_env")),
    }
    if let Some(model) = spec.default_model.as_deref() {
        if !valid_model_id(model) {
            return Err(format!("invalid default_model for provider {name:?}"));
        }
    }
    if spec.context_window.is_some_and(|n| n < 1024) {
        return Err(format!("context_window for provider {name:?} must be at least 1024"));
    }
    if spec.max_tokens == Some(0) {
        return Err(format!("max_tokens for provider {name:?} must be positive"));
    }
    Ok(())
}

fn validate_config(cfg: &ProvidersConfig) -> Result<()> {
    for (name, spec) in &cfg.providers {
        validate_custom_spec(name, spec)?;
    }
    if let Some(def) = cfg.default_provider.as_deref() {
        if !BUILTIN_PROVIDERS.contains(&def) && !cfg.providers.contains_key(def) {
            return Err(format!("default_provider {def:?} in providers.json is not a known provider"));
        }
    }
    Ok(())
}

/// Resolve the active `providers.json` path (`TEAMAGENTS_PROVIDERS_FILE` or XDG config).
pub(crate) fn config_path() -> Result<PathBuf> {
    if let Some(explicit) = env::var_os("TEAMAGENTS_PROVIDERS_FILE").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()) {
        return Ok(xdg.join("teamagents").join("providers.json"));
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or("set HOME, XDG_CONFIG_HOME, or TEAMAGENTS_PROVIDERS_FILE to manage providers")?;
    Ok(home.join(".config").join("teamagents").join("providers.json"))
}

pub(crate) fn providers_config_path_display() -> Option<String> {
    config_path().ok().map(|p| p.display().to_string())
}

pub(crate) fn load_config_at(path: &Path) -> Result<ProvidersConfig> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ProvidersConfig::default()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let mut raw = Vec::new();
    file.take(CONFIG_LIMIT + 1).read_to_end(&mut raw).map_err(|e| format!("read {}: {e}", path.display()))?;
    if raw.len() as u64 > CONFIG_LIMIT {
        return Err(format!("{} exceeds 256 KiB", path.display()));
    }
    let cfg: ProvidersConfig = serde_json::from_slice(&raw).map_err(|e| format!("invalid {}: {e}", path.display()))?;
    validate_config(&cfg)?;
    Ok(cfg)
}

pub(crate) fn save_config_at(path: &Path, cfg: &ProvidersConfig) -> Result<()> {
    validate_config(cfg)?;
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or("invalid provider config path")?;
    fs::DirBuilder::new().recursive(true).mode(0o700).create(parent).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(cfg).map_err(|e| e.to_string())?;
    let tmp = parent.join(format!("providers.json.{}.tmp", std::process::id()));
    let write_res = (|| -> Result<()> {
        let mut f = fs::OpenOptions::new()
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

pub(crate) fn load_config() -> Result<ProvidersConfig> {
    if let Some(explicit) = env::var_os("TEAMAGENTS_PROVIDERS_FILE").filter(|v| !v.is_empty()) {
        return load_config_at(Path::new(&explicit));
    }
    match config_path() {
        Ok(path) => load_config_at(&path),
        Err(_) => Ok(ProvidersConfig::default()),
    }
}

pub(crate) fn upsert_provider(name: &str, spec: CustomProviderSpec, set_default: bool) -> Result<PathBuf> {
    validate_custom_spec(name, &spec)?;
    let path = config_path()?;
    let mut cfg = load_config_at(&path)?;
    cfg.providers.insert(name.to_owned(), spec);
    if set_default {
        cfg.default_provider = Some(name.to_owned());
    }
    save_config_at(&path, &cfg)?;
    Ok(path)
}

pub(crate) fn remove_provider(name: &str) -> Result<PathBuf> {
    if BUILTIN_PROVIDERS.contains(&name) {
        return Err(format!("cannot remove built-in provider {name:?}"));
    }
    let path = config_path()?;
    let mut cfg = load_config_at(&path)?;
    if cfg.providers.remove(name).is_none() {
        return Err(format!("provider {name:?} is not configured in {}", path.display()));
    }
    if cfg.default_provider.as_deref() == Some(name) {
        cfg.default_provider = None;
    }
    save_config_at(&path, &cfg)?;
    Ok(path)
}

pub(crate) fn set_default_provider(target: Option<&str>) -> Result<PathBuf> {
    let path = config_path()?;
    let mut cfg = load_config_at(&path)?;
    if let Some(name) = target {
        if !BUILTIN_PROVIDERS.contains(&name) && !cfg.providers.contains_key(name) {
            return Err(format!("unknown provider {name:?}"));
        }
    }
    cfg.default_provider = target.map(str::to_owned);
    save_config_at(&path, &cfg)?;
    Ok(path)
}

fn nonempty_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn builtin_provider(name: &str) -> Option<ResolvedProvider> {
    match name {
        "deepseek" => Some(ResolvedProvider {
            name: "deepseek".into(),
            wire: WireProtocol::Responses,
            builtin: true,
            base_url: Some(nonempty_env("DEEPSEEK_BASE_URL").unwrap_or_else(|| "https://api.deepseek.com/v1".into())),
            url_label: "DEEPSEEK_BASE_URL".into(),
            api_key_env: Some("DEEPSEEK_API_KEY".into()),
            default_model: Some(nonempty_env("DEEPSEEK_MODEL").unwrap_or_else(|| "deepseek-flash".into())),
            context_window: None,
            tool_choice: ToolChoiceMode::Auto,
            max_tokens: None,
            replay_reasoning: true,
        }),
        "chatgpt" => Some(ResolvedProvider {
            name: "chatgpt".into(),
            wire: WireProtocol::ChatGpt,
            builtin: true,
            base_url: None,
            url_label: "ChatGPT subscription".into(),
            api_key_env: None,
            default_model: None,
            context_window: None,
            tool_choice: ToolChoiceMode::Required,
            max_tokens: None,
            replay_reasoning: false,
        }),
        "responses" => Some(ResolvedProvider {
            name: "responses".into(),
            wire: WireProtocol::Responses,
            builtin: true,
            base_url: Some(nonempty_env("OPENAI_BASE_URL").unwrap_or_else(|| "https://api.openai.com/v1".into())),
            url_label: "OPENAI_BASE_URL".into(),
            api_key_env: Some("OPENAI_API_KEY".into()),
            default_model: None,
            context_window: None,
            tool_choice: ToolChoiceMode::Required,
            max_tokens: None,
            replay_reasoning: false,
        }),
        "anthropic" => Some(ResolvedProvider {
            name: "anthropic".into(),
            wire: WireProtocol::Anthropic,
            builtin: true,
            base_url: Some(nonempty_env("ANTHROPIC_BASE_URL").unwrap_or_else(|| "https://api.anthropic.com".into())),
            url_label: "ANTHROPIC_BASE_URL".into(),
            api_key_env: Some("ANTHROPIC_API_KEY".into()),
            default_model: nonempty_env("ANTHROPIC_MODEL"),
            context_window: None,
            tool_choice: ToolChoiceMode::Required,
            max_tokens: None,
            replay_reasoning: false,
        }),
        "api" => Some(ResolvedProvider {
            name: "api".into(),
            wire: WireProtocol::ChatCompletions,
            builtin: true,
            base_url: Some(nonempty_env("OPENAI_BASE_URL").unwrap_or_else(|| "https://api.openai.com/v1".into())),
            url_label: "OPENAI_BASE_URL".into(),
            api_key_env: Some("OPENAI_API_KEY".into()),
            default_model: None,
            context_window: None,
            tool_choice: ToolChoiceMode::Required,
            max_tokens: None,
            replay_reasoning: false,
        }),
        _ => None,
    }
}

fn resolved_from_custom(name: &str, spec: CustomProviderSpec) -> Result<ResolvedProvider> {
    let wire = WireProtocol::parse_custom(&spec.wire)?;
    let replay_reasoning = spec.replay_reasoning.unwrap_or(false);
    let tool_choice =
        spec.tool_choice.unwrap_or(if replay_reasoning { ToolChoiceMode::Auto } else { ToolChoiceMode::Required });
    Ok(ResolvedProvider {
        name: name.to_owned(),
        wire,
        builtin: false,
        base_url: Some(spec.base_url),
        url_label: format!("providers.json ({name}.base_url)"),
        api_key_env: spec.api_key_env,
        default_model: spec.default_model,
        context_window: spec.context_window,
        tool_choice,
        max_tokens: spec.max_tokens,
        replay_reasoning,
    })
}

/// Resolve a built-in or custom provider by name (reloading `providers.json` dynamically).
pub(crate) fn resolve_provider(name: &str) -> Result<ResolvedProvider> {
    if let Some(p) = builtin_provider(name) {
        return Ok(p);
    }
    let cfg = load_config()?;
    if let Some(spec) = cfg.providers.get(name) {
        return resolved_from_custom(name, spec.clone());
    }
    let mut known: Vec<String> = BUILTIN_PROVIDERS.iter().map(|s| (*s).to_owned()).collect();
    known.extend(cfg.providers.keys().cloned());
    Err(format!("unknown provider {name:?}; available: {}", known.join(", ")))
}

/// Validate and return explicit `TEAMAGENTS_PROVIDER` if set in environment.
pub(crate) fn explicit_env_provider() -> Result<Option<String>> {
    let Some(raw) = env::var_os("TEAMAGENTS_PROVIDER") else {
        return Ok(None);
    };
    let name = raw
        .to_str()
        .filter(|s| !s.is_empty())
        .ok_or("TEAMAGENTS_PROVIDER must be a valid non-empty UTF-8 provider name")?;
    resolve_provider(name)?;
    Ok(Some(name.to_owned()))
}

pub(crate) fn select_default_provider(
    explicit: Option<&str>,
    config_default: Option<&str>,
    has_deepseek: impl FnOnce() -> bool,
    chatgpt_configured: impl FnOnce() -> Result<bool>,
    anthropic_only: impl FnOnce() -> bool,
) -> Result<String> {
    if let Some(name) = explicit {
        return Ok(name.to_owned());
    }
    if let Some(name) = config_default {
        return Ok(name.to_owned());
    }
    if has_deepseek() {
        return Ok("deepseek".into());
    }
    if chatgpt_configured()? {
        return Ok("chatgpt".into());
    }
    if anthropic_only() {
        return Ok("anthropic".into());
    }
    Ok("api".into())
}

/// Default provider selected when no session override is active.
pub(crate) fn default_provider_name() -> Result<String> {
    let explicit = explicit_env_provider()?;
    let cfg = load_config()?;
    select_default_provider(
        explicit.as_deref(),
        cfg.default_provider.as_deref(),
        has_deepseek_env,
        oauth::configured,
        has_anthropic_only_env,
    )
}

/// Effective default model ID for `provider_name` (only applies `TEAMAGENTS_MODEL` to the startup/default provider).
pub(crate) fn default_model_for(provider_name: &str) -> Option<String> {
    let resolved = resolve_provider(provider_name).ok()?;
    let is_startup_provider = default_provider_name().ok().as_deref() == Some(provider_name);
    if is_startup_provider {
        if let Some(m) = nonempty_env("TEAMAGENTS_MODEL") {
            return Some(m);
        }
    }
    resolved.default_model
}

fn readiness_for(resolved: &ResolvedProvider) -> (bool, String) {
    if resolved.wire == WireProtocol::ChatGpt {
        return match oauth::configured() {
            Ok(true) => (true, "ready (ChatGPT subscription)".into()),
            Ok(false) => (false, "signed out (run `teamagents auth login`)".into()),
            Err(e) => (false, e),
        };
    }
    let local = resolved
        .base_url
        .as_deref()
        .and_then(|u| url::Url::parse(u).ok())
        .and_then(|u| u.host_str().map(is_local_host))
        .unwrap_or(false);
    match resolved.api_key_env.as_deref() {
        Some(env_name) if has_nonempty_env(env_name) => (true, format!("ready (${env_name})")),
        _ if local => (true, "ready (loopback)".into()),
        Some(env_name) => (false, format!("missing ${env_name}")),
        None => (false, "missing api_key_env".into()),
    }
}

fn to_info(resolved: ResolvedProvider) -> ProviderInfo {
    let (ready, status) = readiness_for(&resolved);
    ProviderInfo {
        name: resolved.name,
        wire: resolved.wire.as_str().to_owned(),
        builtin: resolved.builtin,
        base_url: resolved.base_url,
        api_key_env: resolved.api_key_env,
        default_model: resolved.default_model,
        context_window: resolved.context_window,
        tool_choice: resolved.tool_choice.as_str().to_owned(),
        ready,
        status,
    }
}

/// Return all built-in and custom providers in display order with readiness metadata.
pub(crate) fn list_providers() -> Result<Vec<ProviderInfo>> {
    let cfg = load_config()?;
    let mut out = Vec::with_capacity(BUILTIN_PROVIDERS.len() + cfg.providers.len());
    for name in BUILTIN_PROVIDERS {
        if let Some(resolved) = builtin_provider(name) {
            out.push(to_info(resolved));
        }
    }
    for (name, spec) in cfg.providers {
        out.push(to_info(resolved_from_custom(&name, spec)?));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_config_path() -> PathBuf {
        std::env::temp_dir().join(format!("teamagents-providers-{}.json", Uuid::new_v4()))
    }

    #[test]
    fn custom_provider_round_trip_and_atomic_permissions() {
        let path = temp_config_path();
        assert_eq!(load_config_at(&path).unwrap(), ProvidersConfig::default());
        let spec = CustomProviderSpec {
            wire: "responses".into(),
            base_url: "https://tokens.byteseek.ai/v1".into(),
            api_key_env: Some("BYTESEEK_API_KEY".into()),
            default_model: Some("gpt-5".into()),
            context_window: Some(400_000),
            tool_choice: Some(ToolChoiceMode::Auto),
            max_tokens: None,
            replay_reasoning: Some(true),
        };
        let mut cfg = ProvidersConfig { default_provider: Some("byteseek".into()), ..Default::default() };
        cfg.providers.insert("byteseek".into(), spec.clone());
        save_config_at(&path, &cfg).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let loaded = load_config_at(&path).unwrap();
        assert_eq!(loaded, cfg);
        let resolved = resolved_from_custom("byteseek", loaded.providers["byteseek"].clone()).unwrap();
        assert_eq!(resolved.wire, WireProtocol::Responses);
        assert_eq!(resolved.tool_choice, ToolChoiceMode::Auto);
        assert!(resolved.replay_reasoning);
        assert_eq!(resolved.context_window, Some(400_000));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn custom_provider_validation_enforces_security_boundaries() {
        let valid = CustomProviderSpec {
            wire: "chat_completions".into(),
            base_url: "https://api.example.com/v1".into(),
            api_key_env: Some("EXAMPLE_API_KEY".into()),
            default_model: None,
            context_window: None,
            tool_choice: None,
            max_tokens: None,
            replay_reasoning: None,
        };
        // Built-in override rejected.
        for builtin in BUILTIN_PROVIDERS {
            assert!(validate_custom_spec(builtin, &valid).is_err());
        }
        // Reserved keyword or bad name rejected.
        for bad_name in ["list", "default", "BadName", "a/b", "", "a b"] {
            assert!(validate_custom_spec(bad_name, &valid).is_err(), "{bad_name}");
        }
        // Custom provider cannot hijack chatgpt wire.
        let mut cg = valid.clone();
        cg.wire = "chatgpt".into();
        assert!(validate_custom_spec("custom_cg", &cg).is_err());
        // Raw API key in api_key_env rejected.
        for raw_secret in ["sk-abc123", "MY KEY", "KEY=VAL", ""] {
            let mut s = valid.clone();
            s.api_key_env = Some(raw_secret.into());
            assert!(validate_custom_spec("custom_sec", &s).is_err(), "{raw_secret}");
        }
        // Non-loopback requires HTTPS and api_key_env.
        let mut http_remote = valid.clone();
        http_remote.base_url = "http://api.example.com/v1".into();
        assert!(validate_custom_spec("custom_http", &http_remote).is_err());
        let mut keyless_remote = valid.clone();
        keyless_remote.api_key_env = None;
        assert!(validate_custom_spec("custom_keyless", &keyless_remote).is_err());
        // Loopback allows http without api_key_env.
        let loopback = CustomProviderSpec {
            wire: "chat_completions".into(),
            base_url: "http://127.0.0.1:8080/v1".into(),
            api_key_env: None,
            default_model: Some("local-llama".into()),
            context_window: Some(32768),
            tool_choice: None,
            max_tokens: None,
            replay_reasoning: None,
        };
        assert!(validate_custom_spec("local_vllm", &loopback).is_ok());
    }

    #[test]
    fn default_provider_priority_respects_env_and_config_default() {
        assert_eq!(select_default_provider(Some("api"), Some("custom"), || true, || Ok(true), || true).unwrap(), "api");
        assert_eq!(select_default_provider(None, Some("custom"), || true, || Ok(true), || true).unwrap(), "custom");
        assert_eq!(select_default_provider(None, None, || true, || Ok(true), || true).unwrap(), "deepseek");
        assert_eq!(select_default_provider(None, None, || false, || Ok(true), || true).unwrap(), "chatgpt");
        assert_eq!(select_default_provider(None, None, || false, || Ok(false), || true).unwrap(), "anthropic");
        assert_eq!(select_default_provider(None, None, || false, || Ok(false), || false).unwrap(), "api");
    }
}
