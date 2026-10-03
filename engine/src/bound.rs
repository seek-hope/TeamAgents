//! Member tool bindings: 'files'/'shell' are native, 'web' unlocks the configured
//! web tools, and a `[tools.<name>] kind = "mcp"` entry of the (trust-filtered)
//! user catalog is an MCP service — declaring it *is* binding it (D-74). A name in
//! the bindings list that is not a built-in is looked up the same way, so an
//! unknown or unsupported one is still refused. Binding a service to a member *is*
//! the authorization for its tools (§5.2), so bound tools skip the approval
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
    /// The binding the tool came from (its MCP service name). Codemode groups tools by it
    /// (`describeNamespace`); it is not part of the model-visible tool name.
    pub namespace: String,
    /// The MCP tool's declared `outputSchema`, when it has one. Codemode returns `structuredContent`
    /// for such a tool instead of its text (D-376, pi's adapter rule).
    pub output_schema: Option<Json>,
    /// (client, remote tool name) for MCP tools; None for the native web tools.
    pub remote: Option<(Arc<McpClient>, String)>,
}

pub struct BoundTools {
    pub tools: Vec<BoundTool>,
}

/// Bindings the product implements natively: binding one of these *is* the
/// authorization for the capability, so they are never looked up in the user
/// catalog as MCP services (§5.2) — and they are exactly what a session
/// binds by default (D-78): the daemon boots with this list, `doctor` reports the
/// surface through the same one, and `web` expands to the configured web bindings.
/// One list means a report can never describe a surface other than the one the
/// session runs with.
pub const DEFAULT_BINDINGS: &[&str] = &["files", "shell", "web", "skills", "memory"];

impl BoundTools {
    pub fn load(catalog: &UserConfig, bindings: &[String]) -> Result<BoundTools, String> {
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        Self::load_in(catalog, bindings, &root)
    }

    pub fn load_in(catalog: &UserConfig, bindings: &[String], root: &Path) -> Result<BoundTools, String> {
        let mut tools: Vec<BoundTool> = vec![];
        let mut selected: Vec<(String, ToolBinding)> = vec![];
        for name in bindings {
            if DEFAULT_BINDINGS.contains(&name.as_str()) {
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
            let declared_now = selected.iter().any(|(n, _)| n == name);
            let web =
                bindings.iter().any(|b| b == "web") && matches!(binding.kind.as_str(), "web_search" | "web_fetch");
            // D-74: a `[tools.<name>] kind = "mcp"` entry *is* the user's binding of
            // that service. It used to be loaded only when its name appeared in the
            // bindings list, and that list is built by the product (the daemon binds
            // files/shell/web/skills), which no user surface could extend — so every
            // configured MCP service was unreachable while the docs promised the
            // `[tools.*]` section as the binding (the web rule below has always
            // worked this way). Only the merged, trust-filtered catalog reaches here:
            // a project file's tools need `[permissions] trust_project = true`.
            let declared_service = binding.kind == "mcp";
            if (web || declared_service) && !declared_now {
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
    ///
    /// D-376: the value is the tool's `structuredContent` when it declared an `outputSchema`, else its
    /// joined text — exactly what a codemode script should receive. An MCP tool that reports failure
    /// (`isError`) is an `Err` carrying its text, so `await tools.x()` rejects in the script.
    pub fn call(&self, name: &str, args: &Json) -> Option<Result<Json, String>> {
        let tool = self.tools.iter().find(|t| t.name == name)?;
        let (client, remote) = tool.remote.as_ref()?;
        Some(render_result(client.call_tool_result(remote, args), tool.output_schema.as_ref(), &tool.name))
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

/// One line of `mcp list`: the service's label and either its `(tool name, description)` pairs or why it could
/// not be asked (D-399).
pub type ServiceListing = (String, Result<Vec<(String, String)>, String>);

/// The configured MCP services and the tools each one offers — the CLI's `mcp list` surface (D-399).
///
/// `doctor` reports the *configured* bindings and runs nothing; this is the verb a user runs to see what a server
/// actually offers. It connects (for a stdio binding that means starting the server) and closes again, and a
/// server that cannot start is reported as its own error instead of failing the whole list: the answer a user
/// wants includes what is unreachable.
pub fn list_services(catalog: &UserConfig, root: &Path) -> Vec<ServiceListing> {
    let mut names: Vec<&String> = catalog.tools.keys().collect();
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let binding = &catalog.tools[name];
        if binding.kind != "mcp" {
            continue;
        }
        let label = binding.mcp_server.clone().unwrap_or_else(|| name.clone());
        let listed = match load_service(name, binding, root) {
            Ok((client, tools)) => {
                let rows = tools.iter().map(|tool| (tool.name.clone(), tool.description.clone())).collect();
                client.close();
                Ok(rows)
            }
            Err(error) => Err(error),
        };
        out.push((label, listed));
    }
    out
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
            namespace: service.clone(),
            output_schema: tool.get("outputSchema").cloned(),
            remote: Some((client.clone(), remote_name)),
        });
    }
    Ok((client, out))
}

/// Turn one `tools/call` result into the value codemode exposes (D-376): `structuredContent` when the
/// tool declared an `outputSchema` (pi's adapter rule), else its joined text. `Err` is the transport
/// failure or an MCP `isError` result, so the script's `await` rejects rather than receiving a
/// failure-shaped value it might use by mistake.
fn render_result(result: Result<Json, String>, output_schema: Option<&Json>, name: &str) -> Result<Json, String> {
    let result = result?;
    let text = McpClient::result_text(&result);
    if result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Err(if text.is_empty() { format!("MCP tool {name} failed") } else { text });
    }
    if output_schema.is_some() {
        if let Some(structured) = result.get("structuredContent").filter(|v| !v.is_null()) {
            return Ok(structured.clone());
        }
    }
    Ok(if text.is_empty() { result } else { Json::String(text) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(value: Json) -> ToolBinding {
        serde_json::from_value(value).unwrap()
    }

    /// D-74: `[tools.<name>] kind = "mcp"` in the user catalog is the binding, and
    /// the session's model surface must carry the service's tools — the service
    /// used to be loadable only through a bindings list the product builds, which
    /// no user surface could extend (the web binding has always worked this way).
    /// The probe's server records that it started, so an unreachable service shows
    /// up as "no tools" rather than as an error.
    #[test]
    fn a_declared_mcp_service_is_bound_without_naming_it_in_the_bindings() {
        let root = std::env::temp_dir().join(format!("ta-mcp-declared-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let script = r#"
import json, os, sys
open(os.path.join(os.path.dirname(__file__) if False else '.', 'started'), 'w').write('1')
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    if request['method'] == 'initialize':
        result = {'protocolVersion': '2025-06-18'}
    elif request['method'] == 'tools/list':
        result = {'tools': [{'name': 'probe_ping', 'description': 'answers pong',
                             'inputSchema': {'type': 'object', 'properties': {}}}]}
    else:
        result = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;
        std::fs::write(root.join("server.py"), script).unwrap();
        let mut catalog = UserConfig::default();
        catalog.tools.insert(
            "probe".into(),
            binding(json!({"kind": "mcp", "mcp_execution": "host", "command": "/usr/bin/python3",
                           "args": ["-u", root.join("server.py").to_string_lossy()]})),
        );
        // the product's own bindings: no "probe" anywhere in the list
        let product_bindings: Vec<String> = ["files", "shell", "web", "skills"].map(str::to_string).to_vec();
        let bound = BoundTools::load_in(&catalog, &product_bindings, &root).unwrap();
        let names: Vec<String> = bound.tools.iter().map(|tool| tool.name.clone()).collect();
        // advertised as <service>_<tool>, so two services cannot collide
        assert_eq!(names, vec!["probe_probe_ping".to_string()], "the declared service is bound: {names:?}");
        // …and it is the schema the member's model call advertises
        let schemas = bound.schemas();
        assert_eq!(schemas[0]["name"], json!("probe_probe_ping"), "{schemas:?}");
        assert!(root.join("started").exists(), "the server really started");
        // naming a service that the catalog does not define is still refused
        assert!(BoundTools::load_in(&catalog, &["ghost".to_string()], &root).is_err());
        std::fs::remove_dir_all(&root).unwrap();
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

        // a required service fails the member start; an optional one only drops the
        // tool. Each case gets its own catalog: declaring a service in `[tools.*]`
        // *is* binding it (D-74), so a broken `required` one is meant to fail the
        // start no matter which service the caller names.
        let mut required_catalog = UserConfig::default();
        required_catalog
            .tools
            .insert("broken".into(), binding(json!({"kind": "mcp", "command": "/nonexistent/mcp", "required": true})));
        assert!(BoundTools::load(&required_catalog, &["broken".to_string()]).is_err());
        let mut optional_catalog = UserConfig::default();
        optional_catalog
            .tools
            .insert("optional".into(), binding(json!({"kind": "mcp", "command": "/nonexistent/mcp"})));
        let ok = BoundTools::load(&optional_catalog, &["optional".to_string()]).unwrap();
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
