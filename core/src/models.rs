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

str_enum!(WorkspacePolicy, "snake_case", Shared, Isolated, GitWorktree);

pub type Json = serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProfile {
    /// The vendor hint: `deepseek` selects that service's defaults (the reasoning echo, the deepseek
    /// protocol) while `protocol` is unset, and any other value is a label for a compatible service — the
    /// wire is decided by `protocol`/`base_url`, never by this name (D-40, D-229).
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
    /// Transport retries the driver may add to one request. **Accepted but not applied in this release** (D-240):
    /// the session's retry budget is its own constant, so this key changes nothing yet — `docs/CONFIG.md` lists it
    /// with that reason and `docs/ACCEPTANCE.md` carries the open question (wire it, refuse it, or report it).
    #[serde(default = "default_retries")]
    pub max_retries: i64,
    #[serde(default)]
    pub generation_options: HashMap<String, Json>,
    /// Model context window in tokens (drives the /status remaining-context
    /// column; None = unknown, shown as "not configured").
    #[serde(default)]
    pub context_window: Option<u64>,
    /// Codex members only: layer `$CODEX_HOME/<name>.config.toml` by running
    /// `codex --profile <name> app-server`. **Not implemented in this release**
    /// (DESIGN Q12 excludes an external Codex adaptation): no code path reads it,
    /// and a config that sets it is refused at load instead of being ignored
    /// (D-75) — configure the member directly with
    /// `provider`/`protocol`/`base_url`/`api_key_env`.
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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
    /// The `kind = "mcp"` service's argv over stdio (the default transport) — required for that kind and
    /// transport, refused at load otherwise (D-232).
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// The `kind = "mcp"` service's endpoint over the `http` transport — required there, and an absolute
    /// http(s) URL (D-232).
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct UserConfig {
    #[serde(default)]
    pub models: HashMap<String, ModelProfile>,
    #[serde(default)]
    pub tools: HashMap<String, ToolBinding>,
    /// Directories the `skill` tool searches for `SKILL.md` entries. An entry must exist and be a directory;
    /// the default (`~/.agents/skills`) is used only when the key is unset (D-232).
    #[serde(default)]
    pub skills_paths: Vec<String>,
    #[serde(default)]
    pub instruction_files: Vec<String>,
    #[serde(default)]
    pub retention: Retention,
    #[serde(default)]
    pub hooks: Hooks,
    /// Acceptance checks the user predefines for every goal (DESIGN §8, Q11):
    /// the runtime runs them in the isolated shell at the completion boundary,
    /// so a goal cannot be reported as done while a check fails. They are the
    /// user's own machine contracts, never conditions a model extracted.
    #[serde(default)]
    pub checks: Vec<CheckSpec>,
    /// Usage and wall-clock ceilings every goal this session creates carries
    /// (§8, A18/A35): the goal's `max_total_tokens` refuses a new request once
    /// the settled usage would pass it, and its deadline refuses one past that
    /// moment. Both are the user's own bounds; a session with neither runs until
    /// the user stops it (see `docs/USER-GUIDE.md` §2.2).
    #[serde(default)]
    pub limits: GoalLimits,
}

/// User-config ceilings for every goal (`[limits]` in the config).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct GoalLimits {
    /// Usage ceiling in tokens (provider-reported and unknown usage included).
    #[serde(default)]
    pub max_total_tokens: Option<u64>,
    /// Wall-clock ceiling in minutes, counted from the moment the goal is created.
    #[serde(default)]
    pub deadline_minutes: Option<u64>,
}

/// One user-defined acceptance check (`[[checks]]` in the config).
///
/// The four fields mirror what the runtime's check boundary understands:
/// `timeout`, `network` and `inputs` are optional, and `inputs` are
/// workspace-relative paths whose hashes bind the result to the artifact
/// versions observed when the check ran (A17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CheckSpec {
    /// Stable id, used in failures, receipts and repair feedback.
    pub id: String,
    /// The command, executed through the same shell tool the model uses.
    pub command: String,
    /// Seconds; absent means the shell tool's own default.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Run with network access (the sandbox is offline by default).
    #[serde(default)]
    pub network: bool,
    /// Workspace-relative inputs the check reads.
    #[serde(default)]
    pub inputs: Vec<String>,
}

impl CheckSpec {
    /// The check in the shape `create_goal limits.required_checks` stores and
    /// the runtime reads: absent fields stay absent (a JSON `null` would not
    /// survive the create_goal gate).
    pub fn to_json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({"id": self.id, "command": self.command});
        if let Some(timeout) = self.timeout {
            value["timeout"] = serde_json::json!(timeout);
        }
        if self.network {
            value["network"] = serde_json::json!(true);
        }
        if !self.inputs.is_empty() {
            value["inputs"] = serde_json::json!(self.inputs);
        }
        value
    }
}

/// Engine event hooks: a user-authored command, never a model-chosen one.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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
