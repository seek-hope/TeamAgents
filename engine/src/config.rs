//! User config and XDG paths.

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
    std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
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

/// Preserve comments and unrelated settings; serialize concurrent UI writers.
pub fn save_custom_provider(name: &str, profile: &ModelProfile) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = user_config_path();
    let parent = path.parent().ok_or("the config path has no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("cannot create the config directory: {e}"))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(parent.join("config.lock"))
        .map_err(|e| format!("cannot lock the config: {e}"))?;
    lock.try_lock().map_err(|_| "another process is editing the config; retry in a moment")?;
    // Do not replace a symlink's target or silently recover an unreadable config.
    let original = match std::fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() => {
            return Err("the user config must be a regular file; resolve the symlink or directory first".into())
        }
        Ok(_) => std::fs::read_to_string(&path).map_err(|e| format!("cannot read the config: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("cannot read the config: {e}")),
    };
    let existing = parse_user_config(&original)?;
    if existing.models.contains_key(name) || existing.models.values().any(|p| p.provider == profile.provider) {
        return Err("a provider or model with this name already exists; pick another name".into());
    }
    let mut doc = original.parse::<toml_edit::DocumentMut>().map_err(|e| format!("invalid TOML config: {e}"))?;
    let serialized = toml::to_string(&std::collections::BTreeMap::from([(
        "models",
        std::collections::BTreeMap::from([(name, profile)]),
    )]))
    .map_err(|e| format!("cannot encode the provider: {e}"))?;
    let addition = serialized.parse::<toml_edit::DocumentMut>().map_err(|e| e.to_string())?;
    if !doc.contains_key("models") {
        doc["models"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let models = doc["models"].as_table_like_mut().ok_or("the models key must be a table")?;
    models.insert(name, addition["models"][name].clone());
    let updated = doc.to_string();
    parse_user_config(&updated)?;
    let tmp = parent.join(format!(".config-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        file.write_all(updated.as_bytes())?;
        file.sync_all()?;
        // An editor need not take our lock. Refuse a detected concurrent edit.
        if std::fs::read_to_string(&path).unwrap_or_default() != original {
            return Err(std::io::Error::other("another program changed the config; retry"));
        }
        std::fs::rename(&tmp, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| format!("saving the provider failed: {e}"))
}

pub fn state_dir() -> PathBuf {
    xdg_state_home().join(APP)
}

pub fn sessions_dir() -> PathBuf {
    state_dir().join("sessions")
}

/// Missing file is not an error (a fresh install has no config yet).
pub fn load_user_config(path: &Path) -> Result<UserConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_user_config(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UserConfig::default()),
        Err(e) => Err(format!("cannot read config {}: {e}", path.display())),
    }
}

/// Only the documented sections are read; unknown sections (e.g. `[permissions]`)
/// are ignored rather than rejected (tolerant loader).
/// Top-level keys of the user catalog (`UserConfig`). A hand-maintained list,
/// so `every_user_config_field_is_accepted` fails the moment a field is added and
/// forgotten here — a silent drop would make the feature inert, which is exactly
/// what happened to `[retention]` and `[hooks]` before that test existed.
const CATALOG_KEYS: &[&str] = &["models", "tools", "skills_paths", "instruction_files", "retention", "hooks"];

pub fn parse_user_config(text: &str) -> Result<UserConfig, String> {
    let value: toml::Value = text.parse().map_err(|e| format!("bad TOML: {e}"))?;
    let table = value.as_table().ok_or("config root must be a table")?;
    // the permissions section is not a catalog field, but a wrong type there is
    // an error, never a silent default
    project_permissions(&value)?;
    let mut filtered = toml::map::Map::new();
    for key in CATALOG_KEYS {
        if let Some(v) = table.get(*key) {
            filtered.insert(key.to_string(), v.clone());
        }
    }
    toml::Value::Table(filtered).try_into().map_err(|e| format!("bad config: {e}"))
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
            "[models.m]\nprovider = \"openai\"\nmodel = \"x\"\n\n[hooks]\nnotify = [\"/bin/sh\", \"hook\"]\n\n[retention]\narchived_days = 30\n",
        )
        .unwrap();
        std::fs::create_dir_all(cwd.join(".teamagents")).unwrap();
        std::fs::write(
            cwd.join(".teamagents/config.toml"),
            "[hooks]\nnotify = [\"/bin/echo\", \"evil\"]\n\n[retention]\narchived_days = 1\n",
        )
        .unwrap();

        let catalog = load_user_config_for(&cwd).unwrap();
        assert_eq!(catalog.hooks.notify, vec!["/bin/sh".to_string(), "hook".to_string()], "the user's hook is loaded");
        assert_eq!(catalog.retention.archived_days, 30, "and so is the user's retention policy");
        let _ = std::fs::remove_dir_all(&root);
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
trust_project_tools = false

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
        assert!(parse_user_config("[permissions]\ntrust_project_tools = \"yes\"\n")
            .unwrap_err()
            .contains("must be true/false"));
        assert!(parse_user_config("permissions = 1\n").unwrap_err().contains("must be a table"));
        assert!(parse_user_config("[permissions]\nmode = \"yolo\"\n").unwrap_err().contains("invalid permission mode"));
        assert!(parse_user_config("[permissions]\ntrust_project_tools = true\nmode = \"full_auto\"\n").is_ok());
    }
}

// -- project config + permissions --------------------------------------------------

pub fn project_config_path(cwd: &Path) -> PathBuf {
    cwd.join(".teamagents").join("config.toml")
}

/// User config plus repository-local project config (user-defined names win).
/// A project file may add model profiles, but never replace a user-defined name,
/// and its tool bindings only load after the user opts in with
/// `[permissions] trust_project_tools = true` (plan §12.2/14).
pub fn load_user_config_for(cwd: &Path) -> Result<UserConfig, String> {
    let user_text = std::fs::read_to_string(user_config_path()).unwrap_or_default();
    let project_text = std::fs::read_to_string(project_config_path(cwd)).unwrap_or_default();
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
    let trusted = project_permissions(&user).map(|p| p.0).unwrap_or(false);
    let mut merged = toml::map::Map::new();
    let empty = toml::map::Map::new();
    let table = |v: &toml::Value, key: &str| -> toml::map::Map<String, toml::Value> {
        v.get(key).and_then(|t| t.as_table()).cloned().unwrap_or_default()
    };
    let merge = |user_part: toml::map::Map<String, toml::Value>,
                 project_part: toml::map::Map<String, toml::Value>,
                 allow_project: bool|
     -> toml::Value {
        let mut out = user_part;
        for (name, value) in project_part {
            if out.contains_key(&name) {
                eprintln!("teamagents: ignoring the project config entry {name:?} (the user config wins)");
                continue;
            }
            if !allow_project {
                eprintln!("teamagents: the project config defines {name:?}; project tools are untrusted by default, so it was ignored");
                continue;
            }
            out.insert(name, value);
        }
        toml::Value::Table(out)
    };
    merged.insert("models".into(), merge(table(&user, "models"), table(&project, "models"), true));
    merged.insert("tools".into(), merge(table(&user, "tools"), table(&project, "tools"), trusted));
    let list = |v: &toml::Value, key: &str| -> Vec<toml::Value> {
        v.get(key).and_then(|t| t.as_array()).cloned().unwrap_or_default()
    };
    // instruction_files/skills_paths land in every member's prompt, so project
    // sources pass the same trust gate as project tools (P1-3)
    let mut skills = list(&user, "skills_paths");
    let mut instructions = list(&user, "instruction_files");
    if trusted {
        skills.extend(list(&project, "skills_paths"));
        instructions.extend(list(&project, "instruction_files"));
    } else {
        for key in ["skills_paths", "instruction_files"] {
            if !list(&project, key).is_empty() {
                eprintln!("teamagents: the project config defines {key:?}; project tools are untrusted by default, so it was ignored");
            }
        }
    }
    merged.insert("skills_paths".into(), toml::Value::Array(skills));
    merged.insert("instruction_files".into(), toml::Value::Array(instructions));
    // hooks run commands and retention deletes data: both are the user's own
    // policy, never a cloned project's (a repo must not be able to install one)
    for key in ["retention", "hooks"] {
        if let Some(value) = user.get(key) {
            merged.insert(key.into(), value.clone());
        }
        if project.get(key).is_some() {
            eprintln!("teamagents: ignoring {key:?} from the project config (it can only be set in the user config)");
        }
    }
    let _ = empty;
    let catalog: UserConfig = toml::Value::Table(merged).try_into().map_err(|e| format!("bad config: {e}"))?;
    validate_configured_paths(&catalog)?;
    Ok(catalog)
}

fn project_permissions(user: &toml::Value) -> Result<(bool, String), String> {
    let Some(permissions) = user.get("permissions") else {
        return Ok((false, "approved_scope".into()));
    };
    let table = permissions.as_table().ok_or_else(|| "[permissions] must be a table in user config".to_string())?;
    let trusted = match table.get("trust_project_tools") {
        None => false,
        Some(value) => {
            value.as_bool().ok_or_else(|| format!("permissions.trust_project_tools must be true/false, got {value}"))?
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

fn validate_configured_paths(catalog: &UserConfig) -> Result<(), String> {
    for path in catalog.skills_paths.iter().chain(catalog.instruction_files.iter()) {
        let expanded = expand_home(path);
        if !expanded.exists() {
            return Err(format!("configured skills/instruction path does not exist: {path}"));
        }
    }
    Ok(())
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
trust_project_tools = true

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
        let catalog = load_user_config_for(&project).unwrap();
        assert!(catalog.models.contains_key("project_model"), "project adds models");
        assert_eq!(catalog.models["shared"].model, "user-wins", "user definitions win");
        assert_eq!(permission_mode_from_config().unwrap(), "approved_scope");

        // without the opt-in the project tools stay out
        let mut user_text = std::fs::read_to_string(user_config_path()).unwrap();
        user_text = user_text.replace("trust_project_tools = true", "trust_project_tools = false");
        std::fs::write(user_config_path(), user_text).unwrap();
        write(&project_config_path(&project), "[tools.sneaky]\nkind = \"mcp\"\ncommand = \"rm\"\n");
        let catalog = load_user_config_for(&project).unwrap();
        assert!(!catalog.tools.contains_key("sneaky"), "untrusted project tools are ignored");
        let _ = std::fs::remove_dir_all(&root);
    }
}
