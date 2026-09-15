//! User config and XDG paths.

use std::collections::HashMap;
use serde_json::Value as Json;
use std::path::{Path, PathBuf};
use teamagents_core::models::UserConfig;

pub const APP: &str = "teamagents";

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
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

pub fn state_dir() -> PathBuf {
    xdg_state_home().join(APP)
}

pub fn sessions_dir() -> PathBuf {
    state_dir().join("sessions")
}

/// TeamSpec import: JSON or YAML.
pub fn parse_spec(text: &str) -> Result<Json, String> {
    if let Ok(spec) = serde_json::from_str::<Json>(text) {
        return Ok(spec);
    }
    serde_yaml::from_str::<Json>(text).map_err(|e| format!("bad spec (JSON/YAML): {e}"))
}

pub fn load_spec_file(path: &std::path::Path) -> Result<Json, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_spec(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Missing file is not an error (a fresh install has no config yet).
pub fn load_user_config(path: &Path) -> Result<UserConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_user_config(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(_) => Ok(UserConfig::default()),
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
        .filter_map(|(name, p)| p.api_key_env.clone().map(|env| (name.clone(), std::env::var(&env).is_ok())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_load_from_json_or_yaml() {
        let yaml = "leader_id: leader\nagents:\n  - id: leader\n    name: L\n    role: leader\n    runtime_kind: deepagents\n    model_profile: m\n";
        let spec = parse_spec(yaml).unwrap();
        assert_eq!(spec["leader_id"], "leader");
        assert_eq!(spec["agents"][0]["id"], "leader");
        let json_spec = parse_spec(r#"{"leader_id": "leader", "agents": []}"#).unwrap();
        assert_eq!(json_spec["agents"].as_array().unwrap().len(), 0);
        assert!(parse_spec("leader_id: [unclosed").is_err());
    }

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
            assert!(CATALOG_KEYS.contains(&key.as_str()), "UserConfig field {key:?} is not in CATALOG_KEYS: {CATALOG_KEYS:?}");
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
                eprintln!("teamagents: 项目配置定义的同名条目 {name:?} 已忽略（用户配置优先）");
                continue;
            }
            if !allow_project {
                eprintln!("teamagents: 项目配置定义了 {name:?}，默认不信任项目工具，已忽略");
                continue;
            }
            out.insert(name, value);
        }
        toml::Value::Table(out)
    };
    merged.insert("models".into(), merge(table(&user, "models"), table(&project, "models"), true));
    merged.insert("tools".into(), merge(table(&user, "tools"), table(&project, "tools"), trusted));
    let list = |v: &toml::Value, key: &str| -> Vec<toml::Value> {
        v.get(key)
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default()
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
                eprintln!("teamagents: 项目配置定义了 {key:?}，默认不信任项目工具，已忽略");
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
            eprintln!("teamagents: 项目配置里的 {key:?} 已忽略（只能在用户配置中设置）");
        }
    }
    let _ = empty;
    let catalog: UserConfig = toml::Value::Table(merged)
        .try_into()
        .map_err(|e| format!("bad config: {e}"))?;
    validate_configured_paths(&catalog)?;
    Ok(catalog)
}

fn project_permissions(user: &toml::Value) -> Result<(bool, String), String> {
    let Some(permissions) = user.get("permissions") else {
        return Ok((false, "approved_scope".into()));
    };
    let table = permissions
        .as_table()
        .ok_or_else(|| "[permissions] must be a table in user config".to_string())?;
    let trusted = match table.get("trust_project_tools") {
        None => false,
        Some(value) => value.as_bool().ok_or_else(|| {
            format!("permissions.trust_project_tools must be true/false, got {value}")
        })?,
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
        write(
            &project_config_path(&project),
            "[tools.sneaky]\nkind = \"mcp\"\ncommand = \"rm\"\n",
        );
        let catalog = load_user_config_for(&project).unwrap();
        assert!(!catalog.tools.contains_key("sneaky"), "untrusted project tools are ignored");
        let _ = std::fs::remove_dir_all(&root);
    }
}
