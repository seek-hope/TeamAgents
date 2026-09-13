//! Member tool bindings (tools.py::build_bound_tools): 'files'/'shell' are
//! native, 'web' unlocks the configured web tools, anything else is an MCP
//! service from the user config. Binding a service to a member *is* the
//! authorization for its tools (plan §12.1), so bound tools skip the approval
//! gate — the ToolGateway still audits every other tool call.

use crate::mcp::McpClient;
use serde_json::{json, Value as Json};
use std::collections::HashSet;
use std::sync::Arc;
use teamagents_core::models::{ToolBinding, UserConfig};

pub struct BoundTool {
    pub name: String,
    pub description: String,
    pub parameters: Json,
    /// (client, remote tool name) for MCP tools; None for the native web tools.
    pub remote: Option<(Arc<McpClient>, String)>,
}

pub struct BoundTools {
    pub tools: Vec<BoundTool>,
}

impl BoundTools {
    pub fn load(catalog: &UserConfig, bindings: &[String]) -> Result<BoundTools, String> {
        let mut tools: Vec<BoundTool> = vec![];
        let mut selected: Vec<(String, ToolBinding)> = vec![];
        for name in bindings {
            if teamagents_core::control::BUILTIN_TOOL_BINDINGS.contains(&name.as_str()) {
                continue; // built-in capabilities, not catalog services
            }
            let Some(binding) = catalog.tools.get(name) else {
                return Err(format!("unknown tool binding {name:?}"));
            };
            if binding.kind == "files" || binding.kind == "shell" {
                continue;
            }
            selected.push((name.clone(), binding.clone()));
        }
        for (name, binding) in &catalog.tools {
            if bindings.iter().any(|b| b == "web")
                && matches!(binding.kind.as_str(), "web_search" | "web_fetch")
                && !selected.iter().any(|(n, _)| n == name)
            {
                selected.push((name.clone(), binding.clone()));
            }
        }

        let mut clients: Vec<Arc<McpClient>> = vec![];
        for (name, binding) in selected {
            match binding.kind.as_str() {
                // web tools are served by the member's tool executor (web_search/web_fetch)
                "web_search" | "web_fetch" => {}
                "mcp" => match load_service(name.as_str(), &binding) {
                    Ok((client, mut service_tools)) => {
                        clients.push(client);
                        tools.append(&mut service_tools);
                    }
                    Err(e) if binding.required => {
                        return Err(format!("required tool service {name:?} is unavailable: {e}"))
                    }
                    Err(e) => {
                        // optional service failure only removes that capability
                        eprintln!("tool service {name:?} unavailable: {e}");
                    }
                },
                other => {
                    if binding.required {
                        return Err(format!("tool binding {name:?} has unsupported kind {other:?}"));
                    }
                }
            }
        }
        Ok(BoundTools { tools })
    }

    pub fn names(&self) -> HashSet<&str> {
        self.tools.iter().map(|t| t.name.as_str()).collect()
    }

    /// Some(_) when this name is a bound MCP tool (binding = authorization).
    pub fn call(&self, name: &str, args: &Json) -> Option<Result<Json, String>> {
        let tool = self.tools.iter().find(|t| t.name == name)?;
        let (client, remote) = tool.remote.as_ref()?;
        Some(client.call_tool(remote, args))
    }

    /// Ready-to-advertise function schemas for the member's model call.
    pub fn schemas(&self) -> Vec<Json> {
        self.tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect()
    }

    pub fn docs(&self) -> Vec<(String, String)> {
        self.tools.iter().map(|t| (t.name.clone(), t.description.clone())).collect()
    }

    pub fn close(&self) {
        for tool in &self.tools {
            if let Some((client, _)) = &tool.remote {
                client.close();
            }
        }
    }
}

fn load_service(name: &str, binding: &ToolBinding) -> Result<(Arc<McpClient>, Vec<BoundTool>), String> {
    let transport = binding.mcp_transport.clone().unwrap_or_else(|| "stdio".into());
    if transport != "stdio" {
        return Err(format!("MCP transport {transport:?} is not implemented in the Rust build (use stdio)"));
    }
    let command = binding.command.clone().ok_or_else(|| format!("binding {name:?} needs a command"))?;
    let env: Vec<(String, String)> = binding
        .env
        .iter()
        .map(|(k, v)| {
            let expanded = if let Some(var) = v.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
                std::env::var(var).unwrap_or_default()
            } else {
                v.clone()
            };
            (k.clone(), expanded)
        })
        .collect();
    let client = McpClient::connect_stdio(&command, &binding.args, &env)?;
    let service = binding.mcp_server.clone().unwrap_or_else(|| name.to_string());
    let allowed: HashSet<&str> = binding.tool_names.iter().map(|s| s.as_str()).collect();
    let mut out = vec![];
    for tool in client.tools()? {
        let remote_name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if remote_name.is_empty() {
            continue;
        }
        let prefixed = format!("{service}_{remote_name}");
        // langchain-mcp's tool_name_prefix: the model sees `<service>_<tool>`
        if !allowed.is_empty() && !allowed.contains(prefixed.as_str()) && !allowed.contains(remote_name.as_str()) {
            continue;
        }
        out.push(BoundTool {
            name: prefixed,
            description: tool
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or(&remote_name)
                .to_string(),
            parameters: tool.get("inputSchema").cloned().unwrap_or(json!({"type": "object"})),
            remote: Some((client.clone(), remote_name)),
        });
    }
    Ok((client, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(value: Json) -> ToolBinding {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn unknown_and_unsupported_bindings_are_reported() {
        let mut catalog = UserConfig::default();
        catalog.tools.insert("echo".into(), binding(json!({"kind": "mcp", "mcp_transport": "http", "url": "https://x"})));
        let err = BoundTools::load(&catalog, &["ghost".to_string()]).err().expect("unknown binding");
        assert!(err.contains("unknown tool binding"), "{err}");

        // an optional service that cannot load only drops the capability
        let ok = BoundTools::load(&catalog, &["echo".to_string()]).unwrap();
        assert!(ok.tools.is_empty());
        // …unless it is marked required
        let mut required = UserConfig::default();
        required.tools.insert(
            "echo".into(),
            binding(json!({"kind": "mcp", "mcp_transport": "http", "url": "https://x", "required": true})),
        );
        let err = BoundTools::load(&required, &["echo".to_string()]).err().expect("required http transport");
        assert!(err.contains("not implemented"), "{err}");

        // a required service fails the member start; an optional one only drops the tool
        let mut catalog = UserConfig::default();
        catalog.tools.insert(
            "broken".into(),
            binding(json!({"kind": "mcp", "command": "/nonexistent/mcp", "required": true})),
        );
        assert!(BoundTools::load(&catalog, &["broken".to_string()]).is_err());
        catalog.tools.insert(
            "optional".into(),
            binding(json!({"kind": "mcp", "command": "/nonexistent/mcp"})),
        );
        let ok = BoundTools::load(&catalog, &["optional".to_string()]).unwrap();
        assert!(ok.tools.is_empty());

        // builtins and 'files'/'shell'-kind entries add no bound tools
        let catalog = UserConfig::default();
        let ok = BoundTools::load(&catalog, &["files".to_string(), "shell".to_string(), "web".to_string()]).unwrap();
        assert!(ok.tools.is_empty());
    }
}
