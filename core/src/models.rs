//! Core data types (DP-1: spec as data).
//! Serde defaults and enum strings are the stable wire format.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident, $case:literal, $($v:ident),+) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = $case)]
        pub enum $name { $($v),+ }
    };
}

str_enum!(RuntimeKind, "snake_case", Deepagents, Codex);

str_enum!(WorkspacePolicy, "snake_case", Shared, Isolated, GitWorktree);

pub type Json = serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSpec {
    pub id: String,
    pub name: String,
    pub role: String,
    pub runtime_kind: RuntimeKind,
    #[serde(default)]
    pub instructions: String,
    pub model_profile: String,
    #[serde(default)]
    pub tool_bindings: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default = "default_workspace_policy")]
    pub workspace_policy: WorkspacePolicy,
}

fn default_workspace_policy() -> WorkspacePolicy {
    WorkspacePolicy::Shared
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProfile {
    pub provider: String,
    #[serde(default = "default_protocol")]
    pub protocol: String, // "openai" (legacy) | "chat/completions" | "responses" | "anthropic" | "deepseek"
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout: i64,
    #[serde(default = "default_retries")]
    pub max_retries: i64,
    #[serde(default)]
    pub generation_options: HashMap<String, Json>,
    /// Model context window in tokens (drives the /status remaining-context
    /// column; None = unknown, shown as "not configured").
    #[serde(default)]
    pub context_window: Option<u64>,
    /// Codex members only: layer `$CODEX_HOME/<name>.config.toml` by running
    /// `codex --profile <name> app-server`. The Codex profile then owns the
    /// provider, model and credentials (e.g. a `deepseek` profile instead of the
    /// official subscription).
    #[serde(default)]
    pub codex_profile: Option<String>,
}

fn default_protocol() -> String {
    "openai".into()
}

fn default_timeout() -> i64 {
    120
}

fn default_retries() -> i64 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ToolBinding {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub mcp_server: Option<String>,
    #[serde(default)]
    pub mcp_transport: Option<String>,
    /// Local MCP execution boundary: workspace (default) or explicit host.
    #[serde(default)]
    pub mcp_execution: Option<String>,
    /// Network access for workspace-sandboxed MCP processes.
    #[serde(default)]
    pub mcp_network: bool,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// Bearer token for the http transport: names the environment variable the
    /// secret is read from — the token itself never lands in this file.
    #[serde(default)]
    pub bearer_token_env_var: Option<String>,
    /// initialize/tools/list timeout in seconds (default 60).
    #[serde(default)]
    pub startup_timeout_s: Option<u64>,
    /// tools/call timeout in seconds (default 120).
    #[serde(default)]
    pub tool_timeout_s: Option<u64>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub tool_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct UserConfig {
    #[serde(default)]
    pub models: HashMap<String, ModelProfile>,
    #[serde(default)]
    pub tools: HashMap<String, ToolBinding>,
    #[serde(default)]
    pub skills_paths: Vec<String>,
    #[serde(default)]
    pub instruction_files: Vec<String>,
    #[serde(default)]
    pub retention: Retention,
    #[serde(default)]
    pub hooks: Hooks,
}
/// Engine event hooks: a user-authored command, never a model-chosen one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hooks {
    /// argv of the command to run (event name is appended as the last argument,
    /// the event JSON arrives on stdin). Empty = no hooks.
    #[serde(default)]
    pub notify: Vec<String>,
    /// argv of a *policy* command run before a native tool executes: exit 0
    /// allows, exit 2 denies (stderr is the reason). Any other outcome allows
    /// and only logs, so a broken hook cannot brick the agent.
    #[serde(default)]
    pub pre_tool: Vec<String>,
}
/// Session housekeeping policy. Nothing is deleted unless a [retention] block
/// asks for it: archiving is the user's own "done with this" marker.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Retention {
    /// Delete archived sessions untouched for this many days when a session is
    /// opened. 0 disables it.
    #[serde(default)]
    pub archived_days: u64,
    /// Drop applied deliveries and events older than this many days from the
    /// session database on open. 0 keeps the full history: events are the audit trail.
    #[serde(default)]
    pub history_days: u64,
}
