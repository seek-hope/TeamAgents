//! User config and XDG paths (config.py, minus project-local config merging).

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

/// TeamSpec import: JSON or YAML (config.py::load_team_spec parity).
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
/// are ignored rather than rejected, matching config.py's tolerant loader.
pub fn parse_user_config(text: &str) -> Result<UserConfig, String> {
    let value: toml::Value = text.parse().map_err(|e| format!("bad TOML: {e}"))?;
    let table = value.as_table().ok_or("config root must be a table")?;
    let mut filtered = toml::map::Map::new();
    for key in ["models", "tools", "skills_paths", "instruction_files"] {
        if let Some(v) = table.get(key) {
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
    fn parses_models_and_ignores_other_sections() {
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
        assert_eq!(cfg.tools["web"].kind, "web_search");
        assert!(parse_user_config("[models.a]\nmodel = 1\n").is_err());
    }
}
