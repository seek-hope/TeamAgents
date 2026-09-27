//! User config and XDG paths.

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use teamagents_core::models::{ModelProfile, UserConfig};

pub const APP: &str = "teamagents";
pub const INITIAL_CONFIG: &str = include_str!("../../examples/config.minimal.toml");

/// Create only a missing config; never follow or overwrite an existing leaf.
pub fn initialize_config(path: &Path) -> Result<bool, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path.parent().ok_or("the config path has no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|e| config_dir_uncreatable(parent, &e))?;
    let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
            if meta.is_dir() {
                return Err(format!("{} is a directory; point at a config file instead", path.display()));
            }
            return Ok(false);
        }
        Err(e) => return Err(format!("cannot create the config {}: {e}", path.display())),
    };
    file.write_all(INITIAL_CONFIG.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("writing the config {} failed; check that the file is complete: {e}", path.display()))?;
    Ok(true)
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The one wording for a config directory that cannot be created (D-242).
///
/// `init` is the only creator, and it answered the OS: measured 2026-09-27 with `XDG_CONFIG_HOME` under a
/// symlink loop, `init failed: cannot create …/teamagents: Too many levels of symbolic links (os error 40)` —
/// a sentence that names neither the variable to fix nor what to do, while every other config-path failure in
/// this family names its lever (`{path} is a directory; point at a config file instead`, D-236's path
/// statement). The two variables are the whole answer, because the path *is* `<xdg_config_home>/<APP>/config.toml`.
pub fn config_dir_uncreatable(path: &Path, error: &std::io::Error) -> String {
    format!(
        "the config directory {} cannot be created: {error} — point XDG_CONFIG_HOME (or HOME) at a directory you \
         can write, or one whose parents do",
        path.display()
    )
}

pub fn home_dir() -> PathBuf {
    env_path("HOME").unwrap_or_else(|| PathBuf::from("/"))
}

pub fn xdg_config_home() -> PathBuf {
    env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home_dir().join(".config"))
}

pub fn xdg_state_home() -> PathBuf {
    env_path("XDG_STATE_HOME").unwrap_or_else(|| home_dir().join(".local").join("state"))
}

pub fn user_config_path() -> PathBuf {
    xdg_config_home().join(APP).join("config.toml")
}

/// User-entered connection settings. Credentials remain environment references.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomProvider {
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    pub model: String,
    pub api_key_env: Option<String>,
    pub context_window: Option<u64>,
}

impl CustomProvider {
    pub fn profile(&self) -> Result<(String, ModelProfile), String> {
        if [&self.name, &self.model, &self.base_url].iter().any(|value| value.chars().any(char::is_control)) {
            return Err("provider name, model id and base URL must not contain control characters".into());
        }
        let name = self.name.trim();
        let model = self.model.trim();
        for (label, value) in [("provider name", name), ("model id", model)] {
            if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
                return Err(format!("{label} must be non-empty, free of control characters and at most 512 bytes"));
            }
        }
        let protocol = match self.protocol.trim().to_ascii_lowercase().as_str() {
            "response" | "responses" => "responses",
            "anthropic" => "anthropic",
            "chat/completions" => "chat/completions",
            _ => return Err("the API format must be responses, anthropic or chat/completions".into()),
        };
        let base = self.base_url.trim().trim_end_matches('/');
        let url = url::Url::parse(base).map_err(|_| "the base URL must be a valid HTTP(S) URL")?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err("the base URL must be a valid HTTP(S) URL".into());
        }
        if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
            return Err("the base URL must not carry credentials, query parameters or fragments; use an environment variable for the key".into());
        }
        if ["/responses", "/chat/completions", "/messages"].iter().any(|suffix| url.path().ends_with(suffix)) {
            return Err("enter the base URL (e.g. https://example.com/v1) without the concrete call path".into());
        }
        let env = self.api_key_env.as_deref().map(str::trim).filter(|s| !s.is_empty());
        if let Some(env) = env {
            if env.len() > 256
                || !env
                    .bytes()
                    .enumerate()
                    .all(|(i, c)| c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
            {
                return Err("the credential env var must start with a letter or underscore and contain only letters, digits and underscores; never paste the key itself".into());
            }
        }
        if self.context_window == Some(0) {
            return Err("the native context window must be a positive integer; leave it empty when unknown".into());
        }
        let profile = serde_json::from_value(serde_json::json!({
            "provider":name, "protocol":protocol, "base_url":base, "model":model,
            "api_key_env":env, "context_window":self.context_window,
        }))
        .map_err(|e| format!("invalid provider config: {e}"))?;
        Ok((name.into(), profile))
    }
}

pub fn state_dir() -> PathBuf {
    xdg_state_home().join(APP)
}

pub fn sessions_dir() -> PathBuf {
    state_dir().join("sessions")
}

/// The file a user creates in a state root to say "keep this root": the third class DESIGN §9 separates, and the
/// `evidence` guard `verification/tla/V2Retention.tla` pins (D-192's `MC_retention_evicts_evidence` control is the
/// counterfactual "a sweep that ignores the mark"). The design's own evaluation evidence (`review/eval/**`) is
/// never inside a session database, so the mark is how a *user* keeps a session's history: the retention sweep
/// refuses a marked root and `doctor` says so (D-245).
pub const EVIDENCE_MARKER: &str = "EVIDENCE";

/// Is this state root marked as evidence? A *file* named `EVIDENCE` in it — the check is existence, so an empty
/// file is enough, and a directory of that name is refused by the state-root kind check (D-228) like the other
/// reserved paths.
pub fn state_root_marked_as_evidence(root: &Path) -> bool {
    root.join(EVIDENCE_MARKER).is_file()
}

/// Missing file is not an error (a fresh install has no config yet).
pub fn load_user_config(path: &Path) -> Result<UserConfig, String> {
    let text = read_config_file(path)?;
    if text.is_empty() {
        return Ok(UserConfig::default());
    }
    parse_user_config(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Read a config file, distinguishing "not there yet" (a fresh install) from "cannot be read" (D-244).
///
/// The merge loader used `unwrap_or_default()`, which made *every* read error look like an empty file: a config
/// whose path is a **directory** — or one the user cannot read — answered "no model configured yet; run
/// `teamagents init`" instead of naming the file. `doctor_fresh_install_reports_the_missing_requirements` caught
/// it the moment the merge became the product's path, and this is the one wording both loaders use now.
fn read_config_file(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("cannot read config {}: {e}", path.display())),
    }
}

/// Only the documented sections are read; unknown sections (e.g. `[permissions]`)
/// are ignored rather than rejected (tolerant loader).
/// Top-level keys of the user catalog (`UserConfig`). A hand-maintained list,
/// so `every_user_config_field_is_accepted` fails the moment a field is added and
/// forgotten here — a silent drop would make the feature inert, which is exactly
/// what happened to `[retention]` and `[hooks]` before that test existed.
const CATALOG_KEYS: &[&str] =
    &["models", "tools", "skills_paths", "instruction_files", "retention", "hooks", "checks", "limits"];

/// The one wording for a key this build does not serve (D-75's rule, D-161's enforcement):
/// refused with a pointer, never dropped in silence.
fn unknown_key(key: &str) -> String {
    format!("unknown key `{key}` in the user config; docs/CONFIG.md lists every key this build serves")
}

pub fn parse_user_config(text: &str) -> Result<UserConfig, String> {
    let value: toml::Value = text.parse().map_err(|e| format!("bad TOML: {e}"))?;
    let table = value.as_table().ok_or("config root must be a table")?;
    // A key this build does not serve is refused here, with a pointer: the table below
    // is filtered to the catalog keys before serde ever sees it, so without this an
    // unknown *top-level* key was dropped silently — measured 2026-09-27, a typo of
    // `skills_paths` left `doctor` green and the path never loaded (D-161).
    for key in table.keys() {
        if key != "permissions" && !CATALOG_KEYS.contains(&key.as_str()) {
            return Err(unknown_key(key));
        }
    }
    // the permissions section is not a catalog field, but a wrong type there is
    // an error, never a silent default
    project_permissions(&value)?;
    let mut filtered = toml::map::Map::new();
    for key in CATALOG_KEYS {
        if let Some(v) = table.get(*key) {
            filtered.insert(key.to_string(), v.clone());
        }
    }
    let catalog: UserConfig = toml::Value::Table(filtered).try_into().map_err(|e| format!("bad config: {e}"))?;
    validate_shared(&catalog)?;
    Ok(catalog)
}

/// The rules **both** loaders apply (D-232): the profiles, the tool bindings and the checks.
///
/// `parse_user_config` (the product's path, through `load_user_config`) and `load_user_config_for` (the
/// project-merge path, which D-133 records as unread by the product) had drifted: the merge path never ran
/// `validate_tools`, so a `kind`/`mcp_execution`/required-field mistake was refused on one path and accepted on
/// the other — measured 2026-09-27, `[tools.t] kind = "mcp"` with no `command` passed `load_user_config_for`
/// while `parse_user_config` refused it. Which checks a config gets must not depend on the loader.
fn validate_shared(catalog: &UserConfig) -> Result<(), String> {
    validate_checks(catalog)?;
    validate_profiles(catalog)?;
    validate_tools(catalog)?;
    Ok(())
}

/// A model-profile key this release does not serve must not be ignored silently
/// (D-75): `codex_profile` describes an external Codex app-server member, which the
/// confirmed scope excludes (DESIGN Q12: "no external Codex adaptation"), and no
/// code path reads it — a config carrying it would quietly run the shipped provider
/// instead of the profile the user named.
fn validate_profiles(catalog: &UserConfig) -> Result<(), String> {
    for (key, profile) in &catalog.models {
        if let Some(codex) = &profile.codex_profile {
            return Err(format!(
                "models.{key}.codex_profile = {codex:?}: an external Codex profile is not part of this release; \
                 configure the member directly with provider/protocol/base_url/api_key_env"
            ));
        }
        // A typo'd protocol used to fall through the provider dispatch's catch-all and quietly speak the
        // chat-completions wire: `protocol = "openais"` left `doctor` green and the session talking a different
        // protocol to the endpoint (measured 2026-09-27, D-162). An *empty* protocol is not a typo: it is the
        // historical default (chat/completions, with the base URL following `provider`), which several configs
        // in this tree and the doc's `provider`-only example rely on.
        if !profile.protocol.is_empty() && !PROTOCOLS.contains(&profile.protocol.as_str()) {
            return Err(format!(
                "models.{key}.protocol = {:?}: this build serves {} (docs/USER-GUIDE.md §2)",
                profile.protocol,
                PROTOCOLS.join(", ")
            ));
        }
        // D-247: the retry budget is applied per instance now, so a value the build cannot serve is refused at
        // load rather than clamped: a negative count has no meaning, and silently reading it as zero would make
        // the key look applied while changing nothing (D-75's rule).
        if profile.max_retries < 0 {
            return Err(format!(
                "models.{key}.max_retries = {}: the retry budget is a whole number of retries, 0 or more \
                 (0 means one attempt)",
                profile.max_retries
            ));
        }
        // D-229: two more silent misconfigurations, the class D-162 fixed for `protocol`. Measured 2026-09-27:
        // `base_url = "not a url"` loaded fine and failed at the first call as `permanent model error: chat API:
        // builder error` — neither the key nor the URL — and `context_window = 0` loaded fine, though a
        // zero-token window can serve no request (the shape `[limits]` already refuses at load).
        if let Some(url) = profile.base_url.as_deref().filter(|url| !url.is_empty()) {
            let usable = match reqwest::Url::parse(url) {
                Ok(parsed) => matches!(parsed.scheme(), "http" | "https"),
                Err(_) => false,
            };
            if !usable {
                return Err(format!(
                    "models.{key}.base_url = {url:?}: the endpoint must be an absolute http(s) URL \
                     (docs/USER-GUIDE.md §2)"
                ));
            }
        }
        if profile.context_window == Some(0) {
            return Err(format!(
                "models.{key}.context_window = 0: a zero-token window cannot serve a request — leave it unset \
                 to use the reader's own value, or set the model's real window (docs/USER-GUIDE.md §2)"
            ));
        }
    }
    Ok(())
}

/// The wire protocols the provider dispatch can build (`engine/src/providers/mod.rs`): the two named arms and
/// the chat-completions shape three names share.
const PROTOCOLS: &[&str] = &["openai", "chat/completions", "deepseek", "responses", "anthropic"];

/// The tool kinds a `[tools.<name>]` entry may declare (`engine/src/bound.rs`).
const TOOL_KINDS: &[&str] = &["web_search", "web_fetch", "mcp"];

/// A tool binding whose `kind` this build does not serve used to be dropped by the binder in silence: the entry
/// stayed in `doctor`'s tools list and nothing was ever bound (measured 2026-09-27, D-162).
fn validate_tools(catalog: &UserConfig) -> Result<(), String> {
    for (name, binding) in &catalog.tools {
        if !TOOL_KINDS.contains(&binding.kind.as_str()) {
            return Err(format!(
                "tools.{name}.kind = {:?}: this build serves {} (docs/CONFIG.md, `[tools.<name>]`)",
                binding.kind,
                TOOL_KINDS.join(", ")
            ));
        }
        // the execution boundary is a *safety* setting, and it had no check at all: `mcp_execution =
        // "workspac"` silently meant the sandboxed default (measured 2026-09-27, D-162). `mcp_transport` is
        // deliberately not checked here — `doctor` names an unserved transport before a session boots, which is
        // D-74's shape ("where the user can still fix it without reading a daemon log").
        // D-232: the field a binding's kind and transport *need*, refused at load — D-162's class one level in.
        // The binder reports a missing one when the session boots ("binding \"probe\" needs a command",
        // engine/src/bound.rs) while `doctor` still shows the binding as fine, so the user meets it after
        // starting a run. An unserved *transport* stays `doctor`'s row (D-74's shape, see above).
        if binding.kind == "mcp" {
            let url_is_usable = binding
                .url
                .as_deref()
                .filter(|url| !url.is_empty())
                .and_then(|url| reqwest::Url::parse(url).ok())
                .is_some_and(|url| matches!(url.scheme(), "http" | "https"));
            match binding.mcp_transport.as_deref().unwrap_or("stdio") {
                "stdio" if binding.command.as_deref().is_none_or(str::is_empty) => {
                    return Err(format!(
                        "tools.{name}.command is required for kind = \"mcp\" over stdio: the service is started \
                         from that argv (docs/CONFIG.md, `[tools.<name>]`)"
                    ));
                }
                "http" if !url_is_usable => {
                    return Err(format!(
                        "tools.{name}.url is required for kind = \"mcp\" over http and must be an absolute \
                         http(s) URL (docs/CONFIG.md, `[tools.<name>]`)"
                    ));
                }
                _ => {}
            }
        }
        if let Some(execution) = binding.mcp_execution.as_deref() {
            if !MCP_EXECUTIONS.contains(&execution) {
                return Err(format!(
                    "tools.{name}.mcp_execution = {execution:?}: this build serves {} (docs/CONFIG.md)",
                    MCP_EXECUTIONS.join(", ")
                ));
            }
        }
    }
    Ok(())
}

/// Where a workspace-sandboxed MCP service may run (`engine/src/bound.rs`): inside the member's workspace, or
/// explicitly on the host.
const MCP_EXECUTIONS: &[&str] = &["workspace", "host"];

/// The goal limits the runtime boots with (`create_goal limits`): the user's
/// acceptance checks and usage ceiling, or `{}` when neither is configured — the
/// shape travels through the same validation the control plane applies, so the
/// config edge and the `create_goal` gate can never disagree.
///
/// `max_total_tokens` is part of `limits` because the core enforces it from there
/// (`begin_request`'s budget gate, A18). The *deadline* is not: the core takes an
/// absolute timestamp at `create_goal`, so `goal_deadline_minutes` below gives the
/// bootstrap the duration to convert.
pub fn goal_limits(catalog: &UserConfig) -> Result<Json, String> {
    let mut limits = serde_json::Map::new();
    if !catalog.checks.is_empty() {
        let checks = Json::Array(catalog.checks.iter().map(|check| check.to_json()).collect());
        teamagents_core::v2::validate_required_checks(&checks)
            .map_err(|e| format!("configured [[checks]] are not usable: {e}"))?;
        limits.insert("required_checks".into(), checks);
    }
    match catalog.limits.max_total_tokens {
        Some(0) => return Err("[limits] max_total_tokens must be a positive number of tokens".into()),
        Some(max) => {
            limits.insert("max_total_tokens".into(), json!(max));
        }
        None => {}
    }
    // The deadline travels the same way and is consumed by the bootstrap
    // (`driver::bootstrap` removes it and sets the absolute `deadline` the core
    // takes); a key the core does not enforce never reaches the stored goal.
    if let Some(minutes) = goal_deadline_minutes(catalog)? {
        limits.insert("deadline_minutes".into(), json!(minutes));
    }
    Ok(Json::Object(limits))
}

/// The wall-clock ceiling of every goal in minutes (DESIGN §8, A35): a static
/// config cannot hold an absolute timestamp, so the bootstrap converts this into
/// the goal's `deadline` when it creates it.
pub fn goal_deadline_minutes(catalog: &UserConfig) -> Result<Option<u64>, String> {
    match catalog.limits.deadline_minutes {
        Some(0) => Err("[limits] deadline_minutes must be a positive number of minutes".into()),
        other => Ok(other),
    }
}

/// Reject a broken `[[checks]]` entry at load time (doctor and every entry
/// point report it), never at the completion boundary where the goal would
/// just fail to settle.
fn validate_checks(catalog: &UserConfig) -> Result<(), String> {
    for check in &catalog.checks {
        if check.id.trim().is_empty() {
            return Err("[[checks]] needs a non-empty id".into());
        }
        if check.command.trim().is_empty() {
            return Err(format!("[[checks]] {}: command must not be empty", check.id));
        }
        if let Some(timeout) = check.timeout {
            if timeout == 0 {
                return Err(format!("[[checks]] {}: timeout must be a positive number of seconds", check.id));
            }
        }
        for input in &check.inputs {
            let relative = std::path::Path::new(input);
            if relative.is_absolute()
                || relative.components().any(|part| matches!(part, std::path::Component::ParentDir))
            {
                return Err(format!(
                    "[[checks]] {}: input {input:?} must be a workspace-relative path without .. escapes",
                    check.id
                ));
            }
        }
    }
    goal_limits(catalog).map(|_| ())?;
    goal_deadline_minutes(catalog).map(|_| ())
}

/// api_key_env -> present in the environment (doctor output).
pub fn missing_key_envs(catalog: &UserConfig) -> HashMap<String, bool> {
    catalog
        .models
        .iter()
        .filter_map(|(name, p)| {
            p.api_key_env
                .as_ref()
                .map(|env| (name.clone(), std::env::var(env).is_ok_and(|value| !value.trim().is_empty())))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_hooks_and_retention_survive_loading_and_project_ones_are_ignored() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-config-hooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cwd = root.join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        std::fs::create_dir_all(root.join("config/teamagents")).unwrap();
        std::fs::write(
            root.join("config/teamagents/config.toml"),
            "[models.m]\nprovider = \"openai\"\nmodel = \"x\"\n\n[hooks]\nnotify = [\"/bin/sh\", \"hook\"]\n\n[retention]\narchived_days = 30\n\n[[checks]]\nid = \"mine\"\ncommand = \"true\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(cwd.join(".teamagents")).unwrap();
        std::fs::write(
            cwd.join(".teamagents/config.toml"),
            "[hooks]\nnotify = [\"/bin/echo\", \"evil\"]\n\n[retention]\narchived_days = 1\n\n[[checks]]\nid = \"theirs\"\ncommand = \"rm -rf /\"\n",
        )
        .unwrap();

        let catalog = load_user_config_for(&cwd).unwrap().0;
        assert_eq!(catalog.hooks.notify, vec!["/bin/sh".to_string(), "hook".to_string()], "the user's hook is loaded");
        assert_eq!(catalog.retention.archived_days, 30, "and so is the user's retention policy");
        // A project must not be able to install an acceptance command: it runs
        // unattended at every completion boundary.
        let ids: Vec<&str> = catalog.checks.iter().map(|check| check.id.as_str()).collect();
        assert_eq!(ids, vec!["mine"], "only the user's own checks load");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// D-75: a model-profile key the release does not serve is refused, not ignored.
    /// `codex_profile` names an external Codex app-server member that the confirmed
    /// scope excludes (DESIGN Q12) and that no code path reads; a config carrying it
    /// used to run the shipped provider while the user believed Codex owned the
    /// profile.
    /// D-247: the retry budget is applied per instance now, so a value the build cannot serve is refused at load
    /// with the key named (the D-229 shape) rather than clamped to zero, which would look applied while adding no
    /// retry at all.
    #[test]
    fn a_negative_retry_budget_is_refused_at_load() {
        let error = parse_user_config("[models.m]\nprovider = \"openai\"\nmodel = \"x\"\nmax_retries = -1\n")
            .expect_err("a negative budget is refused");
        assert!(error.contains("models.m.max_retries = -1") && error.contains("0 or more"), "{error}");
        assert!(
            parse_user_config("[models.m]\nprovider = \"openai\"\nmodel = \"x\"\nmax_retries = 0\n").is_ok(),
            "0 means one attempt, which is a valid budget"
        );
    }

    #[test]
    fn a_codex_profile_is_refused_instead_of_ignored() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-config-codex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        std::fs::create_dir_all(root.join("config/teamagents")).unwrap();
        std::fs::write(
            root.join("config/teamagents/config.toml"),
            "[models.m]\nprovider = \"openai\"\nmodel = \"x\"\ncodex_profile = \"deepseek\"\n",
        )
        .unwrap();
        let error = load_user_config_for(&root).expect_err("a codex profile is not served");
        assert!(error.contains("codex_profile") && error.contains("not part of this release"), "{error}");
        assert!(error.contains("models.m"), "the message names the profile: {error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// D-229: a malformed `base_url` and a zero `context_window` are refused at load with the key named — the
    /// class D-162 fixed for `protocol`. Measured before this test: the first surfaced at the initial model call
    /// as `permanent model error: chat API: builder error` (neither the key nor the URL), and the second loaded
    /// silently even though a zero-token window can serve no request.
    #[test]
    fn a_malformed_endpoint_or_a_zero_window_is_refused_at_load() {
        for (label, text) in [
            ("a URL with spaces", "[models.m]\nprovider = \"openai\"\nmodel = \"x\"\nbase_url = \"not a url\"\n"),
            ("a bare host", "[models.m]\nprovider = \"openai\"\nmodel = \"x\"\nbase_url = \"api.example.com\"\n"),
            (
                "a non-http scheme",
                "[models.m]\nprovider = \"openai\"\nmodel = \"x\"\nbase_url = \"ftp://example.com\"\n",
            ),
            ("a zero window", "[models.m]\nprovider = \"openai\"\nmodel = \"x\"\ncontext_window = 0\n"),
        ] {
            let error = parse_user_config(text).expect_err(label);
            assert!(error.contains("models.m"), "{label}: {error}");
            assert!(error.contains("docs/USER-GUIDE.md §2"), "{label} points at the guide: {error}");
        }
        // the shapes the documents show still parse: a real endpoint, no endpoint, an empty one (unset), a window
        for text in [
            "[models.m]\nprovider = \"openai\"\nmodel = \"x\"\nbase_url = \"https://api.example.com/v1\"\ncontext_window = 1000000\n",
            "[models.m]\nprovider = \"openai\"\nmodel = \"x\"\n",
            "[models.m]\nprovider = \"openai\"\nmodel = \"x\"\nbase_url = \"\"\n",
        ] {
            parse_user_config(text).expect("the documented shapes parse");
        }
    }

    #[test]
    fn every_user_config_field_is_accepted() {
        let value = serde_json::to_value(UserConfig::default()).unwrap();
        let keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
        for key in &keys {
            assert!(
                CATALOG_KEYS.contains(&key.as_str()),
                "UserConfig field {key:?} is not in CATALOG_KEYS: {CATALOG_KEYS:?}"
            );
        }
        assert_eq!(keys.len(), CATALOG_KEYS.len(), "CATALOG_KEYS and UserConfig drifted: {keys:?} vs {CATALOG_KEYS:?}");
    }

    /// A key this build does not serve is refused with a pointer — at the top level and inside `[permissions]`,
    /// the two places serde's own unknown-field check never saw: the top level was filtered to the catalog keys
    /// *before* parsing, and the permissions table is read by hand. Measured before this test existed
    /// (2026-09-27): `skills_pathes = []` (a typo of `skills_paths`) and `[permissions] mod = "full_auto"` (a typo
    /// of `mode`, a *safety* setting — the session would have run in `approved_scope`) both left `doctor` green,
    /// while unknown keys in `[limits]`, `[retention]`, `[hooks]`, `[tools.*]`, `[models.*]` and `[[checks]]`
    /// were already refused (D-161).
    #[test]
    fn a_key_the_build_does_not_serve_is_refused_with_a_pointer() {
        let model = "[models.m]\nprovider = \"openai\"\nmodel = \"x\"\n";
        for (label, text) in [
            ("the top level", format!("mysterys = 1\n{model}")),
            ("a typo of skills_paths", format!("skills_pathes = []\n{model}")),
            ("[permissions]", format!("{model}\n[permissions]\nmod = \"full_auto\"\n")),
            ("[permissions], a typo of trust_project", format!("{model}\n[permissions]\ntrust_project_tool = true\n")),
        ] {
            let error = parse_user_config(&text).expect_err(label);
            assert!(error.contains("unknown key"), "{label}: {error}");
            assert!(error.contains("docs/CONFIG.md"), "{label} must point at the reference: {error}");
        }
        // and the keys the reference lists still parse, both permission keys included
        let ok = parse_user_config(&format!(
            "skills_paths = [\"/tmp/skills\"]\ninstruction_files = []\n{model}\n\
             [permissions]\nmode = \"full_auto\"\ntrust_project = true\n"
        ))
        .expect("the documented keys parse");
        assert_eq!(ok.skills_paths, vec!["/tmp/skills".to_string()]);
    }

    /// A *value* this build does not serve is refused (D-162), where D-161 refused an unserved *key*: a typo'd
    /// `protocol` used to fall through the provider dispatch's catch-all and speak the chat-completions wire, and
    /// a typo'd tool `kind` was dropped by the binder in silence while `doctor` still listed the tool. An empty
    /// protocol stays legal — it is the historical chat/completions default.
    /// D-232: both loaders run the same rules. They had drifted — `load_user_config_for` (the project-merge
    /// path) never ran `validate_tools`, so a binding mistake was refused on one path and accepted on the other
    /// (measured 2026-09-27). D-232 kept one deliberate difference (a configured path that resolves nowhere was
    /// the merge path's refusal); D-244 dissolved it when the merge path became the product's path, because that
    /// difference would have refused to boot on the shipped config's `~/.agents/skills` — the case D-66/D-168
    /// designed a `doctor` warning for (`doctor_reports_the_skills_registry_and_missing_configured_paths`).
    #[test]
    fn both_loaders_refuse_the_same_shapes() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-config-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        std::fs::create_dir_all(root.join("config/teamagents")).unwrap();
        let model = "[models.m]\nprovider = \"openai\"\nmodel = \"x\"\n";
        for (label, text) in [
            (
                "a typo'd protocol",
                "[models.m]\nprovider = \"openai\"\nprotocol = \"openais\"\nmodel = \"x\"\n".to_string(),
            ),
            ("a typo'd kind", format!("{model}[tools.t]\nkind = \"mcp_transport\"\n")),
            ("an mcp binding without a command", format!("{model}[tools.t]\nkind = \"mcp\"\n")),
            (
                "an mcp http binding without a url",
                format!("{model}[tools.t]\nkind = \"mcp\"\nmcp_transport = \"http\"\n"),
            ),
            (
                "a typo'd mcp_execution",
                format!("{model}[tools.t]\nkind = \"mcp\"\ncommand = \"/bin/true\"\nmcp_execution = \"workspac\"\n"),
            ),
        ] {
            let direct = parse_user_config(&text).expect_err(label);
            std::fs::write(root.join("config/teamagents/config.toml"), &text).unwrap();
            let merged = load_user_config_for(&root).expect_err(label);
            assert!(merged.contains(&direct), "{label}: the loaders disagree — direct: {direct}; merged: {merged}");
        }
        // … and the difference D-232 pinned is gone (D-244): a configured path that resolves nowhere is a
        // `[WARN]` row `doctor` names (D-66/D-168), never a load failure — the merge runs for every session now,
        // and the shipped config's `~/.agents/skills` does not exist on a fresh machine
        let missing = format!("skills_paths = [\"/nonexistent/skills-dir\"]\n{model}");
        parse_user_config(&missing).expect("the product's path leaves a missing root to doctor's row");
        std::fs::write(root.join("config/teamagents/config.toml"), &missing).unwrap();
        load_user_config_for(&root).expect("the merge path is the product's path now");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_value_the_build_does_not_serve_is_refused() {
        let profile =
            |protocol: &str| format!("[models.m]\nprovider = \"openai\"\nprotocol = \"{protocol}\"\nmodel = \"x\"\n");
        for protocol in ["openai", "chat/completions", "deepseek", "responses", "anthropic"] {
            parse_user_config(&profile(protocol)).unwrap_or_else(|e| panic!("{protocol}: {e}"));
        }
        parse_user_config(&format!("{}\n[tools.t]\nkind = \"web_fetch\"\n", profile("openai")))
            .expect("a served kind parses");
        for execution in ["workspace", "host"] {
            // D-232: the fixture needs the field its kind and transport require — the rule it never met
            let text = format!(
                "{}\n[tools.t]\nkind = \"mcp\"\ncommand = \"/bin/true\"\nmcp_execution = \"{execution}\"\n",
                profile("openai")
            );
            parse_user_config(&text).unwrap_or_else(|e| panic!("{execution}: {e}"));
        }
        let error = parse_user_config(&format!(
            "{}\n[tools.t]\nkind = \"mcp\"\ncommand = \"/bin/true\"\nmcp_execution = \"workspac\"\n",
            profile("openai")
        ))
        .expect_err("a typo is refused");
        assert!(error.contains("mcp_execution") && error.contains("workspace"), "{error}");
        let error = parse_user_config(&profile("openais")).expect_err("a typo is refused");
        assert!(error.contains("protocol") && error.contains("chat/completions"), "{error}");
        let error = parse_user_config(&format!("{}\n[tools.t]\nkind = \"web_fetchx\"\n", profile("openai")))
            .expect_err("a typo is refused");
        assert!(error.contains("kind") && error.contains("web_fetch"), "{error}");
        parse_user_config("[models.m]\nprovider = \"openai\"\nmodel = \"x\"\n")
            .expect("an absent protocol keeps the chat/completions default");
    }

    #[test]
    fn parses_retention_and_hooks() {
        let cfg = parse_user_config(
            r#"
[models.m]
provider = "openai"
model = "x"

[retention]
archived_days = 30
history_days = 7

[hooks]
notify = ["/bin/sh", "-c", "echo hi", "hook"]
"#,
        )
        .unwrap();
        assert_eq!(cfg.retention.archived_days, 30);
        assert_eq!(cfg.retention.history_days, 7);
        assert_eq!(cfg.hooks.notify.first().map(String::as_str), Some("/bin/sh"), "hooks configure a command");
    }

    /// The user's acceptance checks (`[[checks]]`) reach the goal exactly in the
    /// shape the runtime's check boundary reads: absent fields stay absent,
    /// because a JSON `null` would be rejected by the `create_goal` gate.
    /// `[limits]` gives every goal a usage ceiling and a wall-clock deadline
    /// (§8/A18/A35, D-64): the ceiling travels inside `limits` because the core
    /// enforces it from there, the deadline as the duration the bootstrap converts
    /// at goal creation. A zero is a config error, never a silent default.
    #[test]
    fn user_limits_bound_every_goal() {
        let cfg = parse_user_config("[models.m]\nprovider = \"openai\"\nmodel = \"x\"\n\n[limits]\nmax_total_tokens = 250000\ndeadline_minutes = 45\n")
            .unwrap();
        let limits = goal_limits(&cfg).unwrap();
        assert_eq!(limits["max_total_tokens"], json!(250000));
        assert_eq!(limits["deadline_minutes"], json!(45));
        assert_eq!(goal_deadline_minutes(&cfg).unwrap(), Some(45));
        // neither ceiling configured: `{}`, and no deadline
        let plain = parse_user_config("[models.m]\nprovider = \"openai\"\nmodel = \"x\"\n").unwrap();
        assert_eq!(goal_limits(&plain).unwrap(), json!({}));
        assert_eq!(goal_deadline_minutes(&plain).unwrap(), None);
        // zero is refused at load time (it would otherwise mean "no request ever")
        for text in ["[limits]\nmax_total_tokens = 0\n", "[limits]\ndeadline_minutes = 0\n"] {
            let error = parse_user_config(text).unwrap_err();
            assert!(error.contains("[limits]"), "{error}");
        }
        // an unknown key in [limits] is rejected instead of ignored
        let error = parse_user_config("[limits]\nmax_tokens = 5\n").unwrap_err();
        assert!(error.contains("unknown field") || error.contains("max_tokens"), "{error}");
    }

    #[test]
    fn user_checks_become_goal_limits() {
        let cfg = parse_user_config(
            r#"
[models.m]
provider = "openai"
model = "x"

[[checks]]
id = "tests"
command = "cargo test --offline"
timeout = 600
inputs = ["src", "Cargo.toml"]

[[checks]]
id = "docs"
command = "test -s README.md"
"#,
        )
        .unwrap();
        assert_eq!(cfg.checks.len(), 2);
        let limits = goal_limits(&cfg).unwrap();
        let checks = limits["required_checks"].as_array().unwrap();
        assert_eq!(checks[0]["id"], json!("tests"));
        assert_eq!(checks[0]["command"], json!("cargo test --offline"));
        assert_eq!(checks[0]["timeout"], json!(600));
        assert_eq!(checks[0]["inputs"], json!(["src", "Cargo.toml"]));
        assert!(checks[0].get("network").is_none(), "an unset flag stays absent: {}", checks[0]);
        assert_eq!(checks[1]["id"], json!("docs"));
        assert!(checks[1].get("timeout").is_none(), "no `null` timeout may reach create_goal: {}", checks[1]);
        // no configured check leaves the goal limits empty (the runtime then
        // settles on the candidate alone)
        assert_eq!(goal_limits(&UserConfig::default()).unwrap(), json!({}));
    }

    /// A broken check fails at load time with the check's own id, not at the
    /// completion boundary where the goal would silently never settle.
    #[test]
    fn a_broken_check_is_rejected_when_the_config_loads() {
        for (label, body) in [
            ("empty command", "[[checks]]\nid = \"c\"\ncommand = \"\"\n"),
            ("zero timeout", "[[checks]]\nid = \"c\"\ncommand = \"true\"\ntimeout = 0\n"),
            ("escaping input", "[[checks]]\nid = \"c\"\ncommand = \"true\"\ninputs = [\"../outside\"]\n"),
            ("absolute input", "[[checks]]\nid = \"c\"\ncommand = \"true\"\ninputs = [\"/etc/passwd\"]\n"),
        ] {
            let error = parse_user_config(&format!("[models.m]\nprovider = \"openai\"\nmodel = \"x\"\n\n{body}"))
                .expect_err(&format!("{label} must be refused"));
            assert!(error.contains("checks") || error.contains("id"), "{label}: the message names the check: {error}");
        }
    }

    #[test]
    fn parses_model_context_window() {
        let cfg = parse_user_config(
            r#"
[models.big]
provider = "openai"
model = "gpt"
context_window = 128000

[models.plain]
provider = "openai"
model = "gpt"
"#,
        )
        .unwrap();
        assert_eq!(cfg.models["big"].context_window, Some(128000));
        assert_eq!(cfg.models["plain"].context_window, None, "missing = tolerant default");
        // wrong type is rejected by serde, not silently dropped
        let bad = "[models.a]\nprovider = \"o\"\nmodel = \"m\"\ncontext_window = \"big\"\n";
        assert!(parse_user_config(bad).is_err());
    }

    #[test]
    fn parses_models_and_validates_the_permissions_section() {
        let cfg = parse_user_config(
            r#"
[permissions]
trust_project = false

[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
generation_options = { reasoning_effort = "max" }

[tools.web]
kind = "web_search"
provider = "anysearch"
"#,
        )
        .unwrap();
        assert_eq!(cfg.models["leader_main"].model, "deepseek-flash");
        assert_eq!(cfg.models["leader_main"].context_window, None);
        assert_eq!(cfg.tools["web"].kind, "web_search");
        assert!(parse_user_config("[models.a]\nmodel = 1\n").is_err());
        // a malformed [permissions] section is an error, never a silent default
        assert!(parse_user_config("[permissions]\ntrust_project = \"yes\"\n")
            .unwrap_err()
            .contains("must be true/false"));
        assert!(parse_user_config("permissions = 1\n").unwrap_err().contains("must be a table"));
        assert!(parse_user_config("[permissions]\nmode = \"yolo\"\n").unwrap_err().contains("invalid permission mode"));
        assert!(parse_user_config("[permissions]\ntrust_project = true\nmode = \"full_auto\"\n").is_ok());
    }
}

// -- project config + permissions --------------------------------------------------

pub fn project_config_path(cwd: &Path) -> PathBuf {
    cwd.join(".teamagents").join("config.toml")
}

/// What the project-config merge decided (D-244).
///
/// The loader used to print its notes to stderr, which for a *detached* daemon means `<state root>/daemon.log` —
/// a place a user does not read. Wiring the merge made that the difference between "the repository's config
/// works" and "the repository's config is silently ignored", so the decision comes back as data: the daemon
/// prints it into its own log and `doctor` shows the same facts where the user looks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectMerge {
    /// The file the merge looked for: `<cwd>/.teamagents/config.toml`.
    pub path: PathBuf,
    /// Was there a non-empty file?
    pub present: bool,
    /// The user's `[permissions] trust_project` answered true — the only source of this decision.
    pub trusted: bool,
    /// Project entries that landed, as `<class>:<name>` (`model:fast`, `tool:web`, `skills_paths`, …).
    pub accepted: Vec<String>,
    /// Project entries that did not, each with the rule that refused it.
    pub refused: Vec<String>,
}

/// User config plus repository-local project config (user-defined names win).
///
/// **One gate** (D-244): the project file contributes nothing until the user opts in with
/// `[permissions] trust_project = true` in *their own* config. It used to be two rules — model profiles merged
/// unconditionally while tools, skills paths and instruction files needed the flag — and a project-sourced
/// profile carries `base_url` and `api_key_env`, so "models always merge" let a cloned repository become the
/// *default* model (both the client's and the daemon's "exactly one configured key" fallback) and receive the
/// user's credential at an endpoint the user never chose. The classes a project may never contribute are
/// unchanged and are refused even when trusted: `[permissions]` (`mode` and the trust flag itself), `hooks`,
/// `checks`, `retention` and `limits` are the user's policy — a check is a command that runs without an approval
/// prompt, and a limit is the user's ceiling, not the repository's.
pub fn load_user_config_for(cwd: &Path) -> Result<(UserConfig, ProjectMerge), String> {
    let mut outcome = ProjectMerge { path: project_config_path(cwd), ..Default::default() };
    let user_text = read_config_file(&user_config_path())?;
    let project_text = read_config_file(&outcome.path)?;
    let user: toml::Value = if user_text.trim().is_empty() {
        toml::Value::Table(Default::default())
    } else {
        user_text.parse().map_err(|e| format!("bad TOML: {e}"))?
    };
    let project: toml::Value = if project_text.trim().is_empty() {
        toml::Value::Table(Default::default())
    } else {
        project_text.parse().map_err(|e| format!("bad TOML: {e}"))?
    };
    // the project file gets the user file's own validity rules (D-232's rule, one file over): an unknown class or
    // a broken `[permissions]` table is refused here rather than dropped in silence. The *decision* it carries is
    // never read — `mode` and `trust_project` are the user's.
    if let Some(table) = project.as_table() {
        for key in table.keys() {
            if key != "permissions" && !CATALOG_KEYS.contains(&key.as_str()) {
                return Err(format!(
                    "unknown key `{key}` in the project config ({}); docs/CONFIG.md lists every key this build \
                     serves",
                    outcome.path.display()
                ));
            }
        }
    }
    project_permissions(&project)?;
    let (trusted, _mode) = project_permissions(&user)?;
    outcome.present = !project_text.trim().is_empty();
    outcome.trusted = trusted;
    let untrusted = "the project config is untrusted: set [permissions] trust_project = true in your own config";
    let mut merged = toml::map::Map::new();
    let table = |v: &toml::Value, key: &str| -> toml::map::Map<String, toml::Value> {
        v.get(key).and_then(|t| t.as_table()).cloned().unwrap_or_default()
    };
    let merge = |user_part: toml::map::Map<String, toml::Value>,
                 project_part: toml::map::Map<String, toml::Value>,
                 class: &str,
                 outcome: &mut ProjectMerge|
     -> toml::Value {
        let mut out = user_part;
        for (name, value) in project_part {
            if out.contains_key(&name) {
                outcome.refused.push(format!("{class}:{name} (the user config defines this name)"));
                continue;
            }
            if !trusted {
                outcome.refused.push(format!("{class}:{name} ({untrusted})"));
                continue;
            }
            outcome.accepted.push(format!("{class}:{name}"));
            out.insert(name, value);
        }
        toml::Value::Table(out)
    };
    let models = merge(table(&user, "models"), table(&project, "models"), "model", &mut outcome);
    let tools = merge(table(&user, "tools"), table(&project, "tools"), "tool", &mut outcome);
    merged.insert("models".into(), models);
    merged.insert("tools".into(), tools);
    let list = |v: &toml::Value, key: &str| -> Vec<toml::Value> {
        v.get(key).and_then(|t| t.as_array()).cloned().unwrap_or_default()
    };
    // instruction_files/skills_paths land in every member's prompt, so project
    // sources pass the same trust gate as project tools (P1-3)
    let mut skills = list(&user, "skills_paths");
    let mut instructions = list(&user, "instruction_files");
    for (key, target) in [("skills_paths", &mut skills), ("instruction_files", &mut instructions)] {
        if list(&project, key).is_empty() {
            continue;
        }
        if trusted {
            target.extend(list(&project, key));
            outcome.accepted.push(key.to_string());
        } else {
            outcome.refused.push(format!("{key} ({untrusted})"));
        }
    }
    merged.insert("skills_paths".into(), toml::Value::Array(skills));
    merged.insert("instruction_files".into(), toml::Value::Array(instructions));
    // hooks and checks run commands, retention deletes data and limits bound the session: all four are the
    // user's own policy, never a cloned project's — a repo must not be able to install a command that runs
    // without an approval prompt, delete data, or raise the ceiling the user set. `limits` is carried from the
    // user config here too: the product's loader kept it, this one dropped it, so wiring the merge would have
    // silently lost every `[limits]` setting (found while writing D-244, and the test
    // `the_two_loaders_agree_on_a_user_only_config` is what keeps the two rule sets equal).
    for key in ["retention", "hooks", "checks", "limits"] {
        if let Some(value) = user.get(key) {
            merged.insert(key.into(), value.clone());
        }
        if project.get(key).is_some() {
            outcome.refused.push(format!("{key} (only the user config may set it)"));
        }
    }
    if project.get("permissions").is_some() {
        outcome.refused.push("permissions (mode and trust_project are your own settings)".to_string());
    }
    let catalog: UserConfig = toml::Value::Table(merged).try_into().map_err(|e| format!("bad config: {e}"))?;
    // the rules both loaders share — and, since D-244 wired this loader, *only* those: the one rule this path
    // used to add was "a configured path that resolves nowhere is refused here" (D-232), and running it for every
    // session would refuse to boot on the shipped config's `~/.agents/skills` (which does not exist on a fresh
    // machine) — the opposite of D-66/D-168's rule, where such a path is a `[WARN]` row `doctor` names and the
    // session goes on. The difference D-232 pinned therefore dissolves in that direction: one behaviour, no
    // loader-shaped surprise.
    validate_shared(&catalog)?;
    Ok((catalog, outcome))
}

fn project_permissions(user: &toml::Value) -> Result<(bool, String), String> {
    let Some(permissions) = user.get("permissions") else {
        return Ok((false, "approved_scope".into()));
    };
    let table = permissions.as_table().ok_or_else(|| "[permissions] must be a table in user config".to_string())?;
    // the same rule as the top level: `mod = "full_auto"` (a typo of `mode`, a *safety* setting) used to leave
    // `doctor` green and the session in `approved_scope` (measured 2026-09-27, D-161)
    for key in table.keys() {
        if key != "mode" && key != "trust_project" {
            return Err(format!("{} ([permissions] takes mode and trust_project)", unknown_key(key)));
        }
    }
    let trusted = match table.get("trust_project") {
        None => false,
        Some(value) => {
            value.as_bool().ok_or_else(|| format!("permissions.trust_project must be true/false, got {value}"))?
        }
    };
    let mode = match table.get("mode") {
        None => "approved_scope".to_string(),
        Some(value) => value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("permissions.mode must be a string, got {value}"))?,
    };
    if mode != "approved_scope" && mode != "full_auto" {
        return Err(format!("invalid permission mode {mode:?} in user config"));
    }
    Ok((trusted, mode))
}

/// Full-auto must be user-chosen in config or CLI; never from a project file.
pub fn permission_mode_from_config() -> Result<String, String> {
    let text = std::fs::read_to_string(user_config_path()).unwrap_or_default();
    if text.trim().is_empty() {
        return Ok("approved_scope".into());
    }
    let user: toml::Value = text.parse().map_err(|e| format!("bad TOML: {e}"))?;
    Ok(project_permissions(&user)?.1)
}

/// The text a member's prompt gets from `instruction_files` (D-102 recorded the promise; D-246 delivers it).
///
/// Every file's content in config order, each under one heading line naming the file as the *config* spells it —
/// a member receiving two files must be able to tell where one ends and the next begins, and a reader of a stored
/// prompt can see which file a rule came from. The caller prints `unreadable` (the daemon's log) and `doctor`
/// counts what it can read: a file that cannot be read is never silently absent from both.
///
/// Read per turn, not once per session: instruction files are policy a user edits, and the useful behaviour is
/// that an edit reaches the *next* turn (the shape D-102 asked for) rather than the next state root.
pub fn instruction_text(files: &[String]) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut unreadable = Vec::new();
    for raw in files {
        match std::fs::read_to_string(expand_home(raw)) {
            Ok(body) => {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(&format!("<!-- instruction file: {raw} -->\n{}", body.trim_end()));
            }
            Err(error) => unreadable.push(format!("{raw}: {error}")),
        }
    }
    (text, unreadable)
}

pub fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod project_config_tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn project_config_merges_with_user_priority_and_trust() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let project = root.join("proj");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("XDG_STATE_HOME", home.join(".state"));
        write(
            &user_config_path(),
            r#"
[permissions]
trust_project = true

[models.user_model]
provider = "openai"
model = "gpt"

[models.shared]
provider = "openai"
model = "user-wins"
"#,
        );
        write(
            &project_config_path(&project),
            r#"
[models.project_model]
provider = "openai"
model = "proj"

[models.shared]
provider = "openai"
model = "project-loses"
"#,
        );
        let catalog = load_user_config_for(&project).unwrap().0;
        assert!(catalog.models.contains_key("project_model"), "project adds models");
        assert_eq!(catalog.models["shared"].model, "user-wins", "user definitions win");
        assert_eq!(permission_mode_from_config().unwrap(), "approved_scope");

        // without the opt-in the project tools stay out
        let mut user_text = std::fs::read_to_string(user_config_path()).unwrap();
        user_text = user_text.replace("trust_project = true", "trust_project = false");
        std::fs::write(user_config_path(), user_text).unwrap();
        write(&project_config_path(&project), "[tools.sneaky]\nkind = \"mcp\"\ncommand = \"rm\"\n");
        let catalog = load_user_config_for(&project).unwrap().0;
        assert!(!catalog.tools.contains_key("sneaky"), "untrusted project tools are ignored");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// D-244: the two loaders must end up with the *same* catalog for a config only the user wrote. The merge
    /// path had drifted from the product path on coverage, not on rules: it carried models, tools, skills,
    /// instruction files, hooks, checks and retention, and silently dropped `[limits]` — so wiring the merge
    /// (D-244) would have lost every goal ceiling and deadline the user set. Found while writing that wiring;
    /// this test is what keeps the two rule sets equal.
    /// D-246: what `instruction_files` delivers to a prompt — content in config order under a heading naming
    /// each file, and every file that could not be read named back to the caller (never dropped in silence).
    #[test]
    fn instruction_text_names_each_file_and_reports_what_it_cannot_read() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-rules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("HOME", &root);
        std::fs::write(root.join("one.md"), "# One\nthe first rule\n").unwrap();
        std::fs::write(root.join("two.md"), "the second rule").unwrap();
        let files = vec!["~/one.md".to_string(), "~/two.md".to_string(), "~/gone.md".to_string()];
        let (text, unreadable) = instruction_text(&files);
        assert!(text.contains("<!-- instruction file: ~/one.md -->"), "{text}");
        assert!(text.contains("the first rule") && text.contains("the second rule"), "{text}");
        assert!(
            text.find("the first rule").unwrap() < text.find("the second rule").unwrap(),
            "the files keep config order: {text}"
        );
        assert_eq!(unreadable.len(), 1, "{unreadable:?}");
        assert!(unreadable[0].starts_with("~/gone.md:"), "{unreadable:?}");
        assert!(!text.contains("gone.md <!--"), "an unreadable file contributes no heading: {text}");
        // an empty list is empty text, not a stray separator
        assert_eq!(instruction_text(&[]).0, "");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_two_loaders_agree_on_a_user_only_config() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-config-agree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (home, project) = (root.join("home"), root.join("proj"));
        std::fs::create_dir_all(&project).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::fs::create_dir_all(home.join(".config/teamagents")).unwrap();
        std::fs::create_dir_all(home.join("skills")).unwrap();
        write(
            &user_config_path(),
            &format!(
                r#"
skills_paths = ["{}"]
instruction_files = ["{}"]

[models.m]
provider = "openai"
model = "m"

[tools.web]
kind = "web_search"

[hooks]
notify = ["/bin/echo", "hook"]

[retention]
archived_days = 30

[[checks]]
id = "mine"
command = "true"

[limits]
max_total_tokens = 1000
deadline_minutes = 5
"#,
                home.join("skills").display(),
                project.join("RULES.md").display()
            ),
        );
        std::fs::write(project.join("RULES.md"), "rules").unwrap();
        let product = load_user_config(&user_config_path()).unwrap();
        let merged = load_user_config_for(&project).unwrap().0;
        assert_eq!(product, merged, "one config, one catalog: the two loaders must not disagree");
        assert!(merged.limits.max_total_tokens.is_some(), "the merge carries the user's goal limits");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// D-244: without the user's opt-in the project file contributes **nothing** — it used to contribute model
    /// profiles unconditionally, and a profile carries `base_url`/`api_key_env`, so a cloned repository could
    /// become the session's default model (the "exactly one configured key" fallback) and be handed the user's
    /// credential. The outcome lists what was refused, with the rule that refused it.
    #[test]
    fn an_untrusted_project_contributes_nothing_and_says_so() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-config-untrusted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (home, project) = (root.join("home"), root.join("proj"));
        std::fs::create_dir_all(&project).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        write(&user_config_path(), "[models.mine]\nprovider = \"openai\"\nmodel = \"m\"\n");
        write(
            &project_config_path(&project),
            "skills_paths = [\"/tmp\"]\n\n[models.theirs]\nprovider = \"openai\"\nmodel = \"evil\"\n\
             base_url = \"https://example.invalid/v1\"\napi_key_env = \"MY_SECRET\"\n\n[tools.sneaky]\n\
             kind = \"mcp\"\ncommand = \"rm\"\n",
        );
        let (catalog, outcome) = load_user_config_for(&project).unwrap();
        assert!(outcome.present && !outcome.trusted, "{outcome:?}");
        assert!(outcome.accepted.is_empty(), "an untrusted project lands nothing: {outcome:?}");
        assert_eq!(catalog.models.len(), 1, "the project's model profile does not land: {:?}", catalog.models.keys());
        assert!(catalog.tools.is_empty() && catalog.skills_paths.is_empty());
        for want in ["model:theirs", "tool:sneaky", "skills_paths"] {
            assert!(
                outcome.refused.iter().any(|entry| entry.starts_with(want) && entry.contains("trust_project")),
                "{want} must be named with the rule: {outcome:?}"
            );
        }

        // with the opt-in the same file lands everything it asked for
        write(
            &user_config_path(),
            "[permissions]\ntrust_project = true\n\n[models.mine]\nprovider = \"openai\"\nmodel = \"m\"\n",
        );
        let (catalog, outcome) = load_user_config_for(&project).unwrap();
        assert!(outcome.trusted);
        assert_eq!(outcome.accepted, vec!["model:theirs", "tool:sneaky", "skills_paths"], "{outcome:?}");
        assert!(catalog.models.contains_key("theirs") && catalog.tools.contains_key("sneaky"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// D-244: `[permissions]` is the user's own — a project cannot set the session mode, and it cannot grant
    /// itself the trust the gate is built on. Both are refused with a note; the mode stays the user's.
    #[test]
    fn a_project_cannot_set_the_mode_or_grant_itself_trust() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-config-selfgrant-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (home, project) = (root.join("home"), root.join("proj"));
        std::fs::create_dir_all(&project).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        write(&user_config_path(), "[models.mine]\nprovider = \"openai\"\nmodel = \"m\"\n");
        write(
            &project_config_path(&project),
            "[permissions]\nmode = \"full_auto\"\ntrust_project = true\n\n[models.theirs]\nprovider = \"openai\"\nmodel = \"evil\"\n",
        );
        let (catalog, outcome) = load_user_config_for(&project).unwrap();
        assert!(!outcome.trusted, "the project cannot grant itself trust: {outcome:?}");
        assert!(!catalog.models.contains_key("theirs"));
        assert!(
            outcome.refused.iter().any(|entry| entry.starts_with("permissions (")),
            "the ignored table is named: {outcome:?}"
        );
        assert_eq!(permission_mode_from_config().unwrap(), "approved_scope", "the mode stays the user's");

        // and a *broken* project `[permissions]` table is refused like the user's own (D-232's rule): a repo
        // shipping a typo there must not be told its config was fine
        write(&project_config_path(&project), "[permissions]\ntrust_project_tool = true\n");
        let error = load_user_config_for(&project).expect_err("a typo in the project's table is refused");
        assert!(error.contains("trust_project_tool") && error.contains("[permissions]"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
