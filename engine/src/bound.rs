//! Member tool bindings: 'files'/'shell' are
//! native, 'web' unlocks the configured web tools, anything else is an MCP
//! service from the user config. Binding a service to a member *is* the
//! authorization for its tools (plan §12.1), so bound tools skip the approval
//! gate — the ToolGateway still audits every other tool call.

use crate::mcp::McpClient;
use serde_json::{json, Value as Json};
use std::collections::HashSet;
use std::path::Path;
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
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        Self::load_in(catalog, bindings, &root)
    }

    pub fn load_in(catalog: &UserConfig, bindings: &[String], root: &Path) -> Result<BoundTools, String> {
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
                "mcp" => match load_service(name.as_str(), &binding, root) {
                    Ok((client, mut service_tools)) => {
                        if service_tools.is_empty() {
                            client.close();
                        } else {
                            clients.push(client);
                        }
                        tools.append(&mut service_tools);
                    }
                    Err(e) if binding.required => {
                        for client in &clients {
                            client.close();
                        }
                        return Err(format!("required tool service {name:?} is unavailable: {e}"));
                    }
                    Err(e) => {
                        // optional service failure only removes that capability
                        eprintln!("tool service {name:?} unavailable: {e}");
                    }
                },
                other => {
                    if binding.required {
                        for client in &clients {
                            client.close();
                        }
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

fn load_service(name: &str, binding: &ToolBinding, root: &Path) -> Result<(Arc<McpClient>, Vec<BoundTool>), String> {
    let transport = binding.mcp_transport.clone().unwrap_or_else(|| "stdio".into());
    let client = match transport.as_str() {
        "stdio" => {
            let command = binding.command.clone().ok_or_else(|| format!("binding {name:?} needs a command"))?;
            // same contract as bearer_token_env_var below: a named variable
            // that is not set is a hard error, never a silent empty secret
            let env: Vec<(String, String)> = binding
                .env
                .iter()
                .map(|(k, v)| {
                    let expanded = if let Some(var) = v.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
                        std::env::var(var).map_err(|_| format!("binding {name:?}: env var {var} is not set"))?
                    } else {
                        v.clone()
                    };
                    Ok((k.clone(), expanded))
                })
                .collect::<Result<_, String>>()?;
            McpClient::connect_stdio_in(
                &command,
                &binding.args,
                &env,
                root,
                binding.mcp_execution.as_deref().unwrap_or("workspace"),
                binding.mcp_network,
                binding.startup_timeout_s.unwrap_or(60),
                binding.tool_timeout_s.unwrap_or(120),
            )?
        }
        "http" => {
            let url =
                binding.url.clone().ok_or_else(|| format!("binding {name:?} needs a url for the http transport"))?;
            // the token lives only in the named environment variable, never in config
            let token = match &binding.bearer_token_env_var {
                Some(var) => Some(
                    std::env::var(var)
                        .map_err(|_| format!("binding {name:?}: bearer token env var {var} is not set"))?,
                ),
                None => None,
            };
            McpClient::connect_http(
                &url,
                token,
                binding.startup_timeout_s.unwrap_or(60),
                binding.tool_timeout_s.unwrap_or(120),
            )?
        }
        "sse" => {
            return Err(format!(
                "binding {name:?}: the MCP \"sse\" transport was removed from the spec; use \"http\" (streamable HTTP)"
            ))
        }
        other => return Err(format!("MCP transport {other:?} is not implemented (use \"stdio\" or \"http\")")),
    };
    // the workspace is what a server gets when it asks for `roots/list`
    client.set_workspace(root);
    let service = binding.mcp_server.clone().unwrap_or_else(|| name.to_string());
    let allowed: HashSet<&str> = binding.tool_names.iter().map(|s| s.as_str()).collect();
    let mut out = vec![];
    let listed = match client.tools() {
        Ok(tools) => tools,
        Err(error) => {
            client.close();
            return Err(error);
        }
    };
    for tool in listed {
        let remote_name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if remote_name.is_empty() {
            continue;
        }
        let prefixed = format!("{service}_{remote_name}");
        // service-name prefix: the model sees `<service>_<tool>`
        if !allowed.is_empty() && !allowed.contains(prefixed.as_str()) && !allowed.contains(remote_name.as_str()) {
            continue;
        }
        out.push(BoundTool {
            name: prefixed,
            description: tool.get("description").and_then(|v| v.as_str()).unwrap_or(&remote_name).to_string(),
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
    fn filtered_and_failed_services_reap_started_processes() {
        let root = std::env::temp_dir().join(format!("ta-mcp-cleanup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let script = r#"
import json, os, sys
with open('pid', 'w') as f: f.write(str(os.getpid()))
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    response = {'jsonrpc': '2.0', 'id': request['id'], 'result': {'protocolVersion': '2025-06-18'}}
    if request['method'] == 'tools/list':
        if sys.argv[1] == 'fail': response = {'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -1, 'message': 'bad list'}}
        else: response['result'] = {'tools': [{'name': 'echo'}]}
    print(json.dumps(response), flush=True)
"#;
        for scenario in ["filtered", "fail", "later-failure"] {
            let mut catalog = UserConfig::default();
            catalog.tools.insert(
                "first".into(),
                binding(json!({
                    "kind": "mcp", "mcp_execution": "host", "command": "/usr/bin/python3",
                    "args": ["-u", "-c", script, scenario], "required": true,
                    "tool_names": if scenario == "filtered" { vec!["absent"] } else { vec![] },
                })),
            );
            let mut bindings = vec!["first".into()];
            if scenario == "later-failure" {
                catalog.tools.insert("broken".into(), binding(json!({"kind": "unsupported", "required": true})));
                bindings.push("broken".into());
            }
            let result = BoundTools::load_in(&catalog, &bindings, &root);
            if scenario == "filtered" {
                assert!(result.unwrap().tools.is_empty());
            } else {
                assert!(result.is_err());
            }
            let pid: u32 = std::fs::read_to_string(root.join("pid")).unwrap().parse().unwrap();
            assert!(!Path::new(&format!("/proc/{pid}")).exists(), "{scenario} left a server process");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unknown_and_unsupported_bindings_are_reported() {
        let mut catalog = UserConfig::default();
        catalog.tools.insert(
            "echo".into(),
            binding(json!({"kind": "mcp", "mcp_transport": "http", "url": "http://127.0.0.1:1/mcp"})),
        );
        let err = BoundTools::load(&catalog, &["ghost".to_string()]).err().expect("unknown binding");
        assert!(err.contains("unknown tool binding"), "{err}");

        // an optional service that cannot load only drops the capability
        // (port 1 refuses connections: the http transport is implemented but down)
        let ok = BoundTools::load(&catalog, &["echo".to_string()]).unwrap();
        assert!(ok.tools.is_empty());
        // …unless it is marked required
        let mut required = UserConfig::default();
        required.tools.insert(
            "echo".into(),
            binding(json!({"kind": "mcp", "mcp_transport": "http", "url": "http://127.0.0.1:1/mcp", "required": true})),
        );
        let err = BoundTools::load(&required, &["echo".to_string()]).err().expect("required http transport");
        assert!(err.contains("unavailable"), "{err}");

        // the removed "sse" transport gets a pointer at "http"
        let mut legacy = UserConfig::default();
        legacy.tools.insert(
            "old".into(),
            binding(json!({"kind": "mcp", "mcp_transport": "sse", "url": "http://x", "required": true})),
        );
        let err = BoundTools::load(&legacy, &["old".to_string()]).err().expect("sse transport");
        assert!(err.contains("sse") && err.contains("http"), "{err}");

        // a named bearer token env var that is not set fails the binding
        let mut auth = UserConfig::default();
        auth.tools.insert(
            "auth".into(),
            binding(json!({"kind": "mcp", "mcp_transport": "http", "url": "http://127.0.0.1:1/mcp",
                "bearer_token_env_var": "TA_MCP_TOKEN_DEFINITELY_UNSET", "required": true})),
        );
        let err = BoundTools::load(&auth, &["auth".to_string()]).err().expect("missing bearer env var");
        assert!(err.contains("TA_MCP_TOKEN_DEFINITELY_UNSET"), "{err}");

        // a required service fails the member start; an optional one only drops the tool
        let mut catalog = UserConfig::default();
        catalog
            .tools
            .insert("broken".into(), binding(json!({"kind": "mcp", "command": "/nonexistent/mcp", "required": true})));
        assert!(BoundTools::load(&catalog, &["broken".to_string()]).is_err());
        catalog.tools.insert("optional".into(), binding(json!({"kind": "mcp", "command": "/nonexistent/mcp"})));
        let ok = BoundTools::load(&catalog, &["optional".to_string()]).unwrap();
        assert!(ok.tools.is_empty());

        // a stdio ${VAR} reference to an unset variable fails the binding,
        // just like the http transport's bearer_token_env_var
        let mut stdio_env = UserConfig::default();
        stdio_env.tools.insert(
            "tok".into(),
            binding(json!({"kind": "mcp", "command": "/nonexistent/mcp", "required": true,
                "env": {"TOKEN": "${TA_MCP_STDIO_DEFINITELY_UNSET}"}})),
        );
        let err = BoundTools::load(&stdio_env, &["tok".to_string()]).err().expect("missing stdio env var");
        assert!(err.contains("TA_MCP_STDIO_DEFINITELY_UNSET"), "{err}");

        // builtins and 'files'/'shell'-kind entries add no bound tools
        let catalog = UserConfig::default();
        let ok = BoundTools::load(&catalog, &["files".to_string(), "shell".to_string(), "web".to_string()]).unwrap();
        assert!(ok.tools.is_empty());
    }
}
