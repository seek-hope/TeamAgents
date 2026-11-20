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
    pub(crate) projects: Option<Arc<CodeGraphRoots>>,
}

/// Canonical exact roots, not a list of parent directories under which discovery is allowed.
#[derive(Debug)]
pub(crate) struct CodeGraphRoots {
    workspace: std::path::PathBuf,
    extra: Vec<std::path::PathBuf>,
}

impl CodeGraphRoots {
    fn new(root: &Path, extras: &[String]) -> Result<Self, String> {
        let canonical = |p: &Path| p.canonicalize().map_err(|e| format!("CodeGraph project {}: {e}", p.display()));
        let workspace = canonical(root)?;
        let extra = extras
            .iter()
            .map(|s| {
                let p = Path::new(s);
                if !p.is_absolute() {
                    return Err("CodeGraph roots must be absolute".into());
                }
                canonical(p)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { workspace, extra })
    }
    fn start_root(&self) -> Result<&Path, String> {
        std::iter::once(&self.workspace)
            .chain(&self.extra)
            .map(|p| p.as_path())
            .find(|p| Self::index(p).is_ok())
            .ok_or_else(|| "no local CodeGraph index in an authorized root; no index was created".into())
    }

    /// Match the native server's `nodes` schema test before it is allowed to search upwards.
    fn index(root: &Path) -> Result<Json, String> {
        let dir = root.join(".codegraph");
        let db = dir.join("codegraph.db");
        if !db.is_file() || db.canonicalize().ok().as_ref() != Some(&db) {
            return Err("CodeGraph requires a local index in the selected project; no index was created".into());
        }
        for sidecar in ["codegraph.db-shm", "codegraph.db-wal", "codegraph.db-journal"] {
            if dir.join(sidecar).symlink_metadata().is_ok_and(|m| !m.is_file()) {
                return Err("CodeGraph needs local SQLite sidecars".into());
            }
        }
        let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "CodeGraph index is unreadable")?;
        conn.query_row("SELECT 1 FROM sqlite_schema WHERE type='table' AND name='nodes'", [], |_| Ok(()))
            .map_err(|_| "CodeGraph index is invalid; no ancestor fallback")?;
        let mtime = |p: &Path| {
            p.metadata().ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|t| t.as_secs())
        };
        Ok(
            json!({"status":"not_verified", "database_mtime_unix":mtime(&db), "wal_mtime_unix":mtime(&dir.join("codegraph.db-wal"))}),
        )
    }
    fn arguments(&self, args: &Json) -> Result<(String, Json, Json), String> {
        let mut obj = args.as_object().cloned().ok_or("CodeGraph needs an argument object")?;
        if obj.keys().any(|k| !matches!(k.as_str(), "query" | "projectPath" | "maxFiles"))
            || obj.get("query").and_then(Json::as_str).is_none_or(|q| q.trim().is_empty())
        {
            return Err("invalid CodeGraph explore arguments".into());
        }
        if obj.get("maxFiles").is_some_and(|n| !n.as_u64().is_some_and(|v| (1..=20).contains(&v))) {
            return Err("CodeGraph maxFiles must be in 1..20".into());
        }
        let root = match obj.get("projectPath") {
            None => self.workspace.clone(),
            Some(v) => {
                let p = Path::new(v.as_str().ok_or("CodeGraph projectPath must be a string")?);
                if !p.is_absolute() {
                    return Err("CodeGraph projectPath must be absolute".into());
                }
                p.canonicalize().map_err(|_| "CodeGraph project is unavailable")?
            }
        };
        if root != self.workspace && !self.extra.contains(&root) {
            return Err("CodeGraph project is outside the allowed roots".into());
        }
        // Upstream walks ancestors for a missing index. Never silently query an enclosing project.
        let freshness = Self::index(&root)?;
        let project = root.to_str().ok_or("CodeGraph requires a UTF-8 root")?.to_string();
        obj.insert("projectPath".into(), json!(project));
        Ok((project, Json::Object(obj), freshness))
    }
}

pub struct BoundTools {
    pub tools: Vec<BoundTool>,
    pub(crate) services: std::collections::BTreeMap<String, Arc<McpClient>>,
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

    /// Explicit user-side discovery and local probes may use the configured extra roots.
    pub fn load_in(catalog: &UserConfig, bindings: &[String], root: &Path) -> Result<BoundTools, String> {
        Self::for_member(catalog, bindings, root, true)
    }

    pub(crate) fn for_member(
        catalog: &UserConfig,
        bindings: &[String],
        root: &Path,
        extra_roots: bool,
    ) -> Result<BoundTools, String> {
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

        selected.sort_by(|a, b| a.0.cmp(&b.0));
        let mut clients: Vec<Arc<McpClient>> = vec![];
        let mut services = std::collections::BTreeMap::new();
        for (name, binding) in selected {
            match binding.kind.as_str() {
                // web tools are served by the member's tool executor (web_search/web_fetch)
                "web_search" | "web_fetch" => {}
                "mcp" => match load_service(name.as_str(), &binding, root, extra_roots) {
                    Ok((client, mut service_tools)) => {
                        if service_tools.is_empty() && !client.supports("resources") {
                            client.close();
                        } else {
                            let namespace = binding.mcp_server.clone().unwrap_or_else(|| name.clone());
                            if services.contains_key(&namespace) {
                                client.close();
                                for client in &clients {
                                    client.close();
                                }
                                return Err(format!("duplicate MCP service name {namespace:?}"));
                            }
                            clients.push(client.clone());
                            services.insert(namespace, client);
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
        Ok(BoundTools { tools, services })
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
        if name == "mcp_resource" {
            return Some(self.call_resource(args));
        }
        let tool = self.tools.iter().find(|t| t.name == name)?;
        let (client, remote) = tool.remote.as_ref()?;
        if let Some(roots) = &tool.projects {
            return Some(roots.arguments(args).and_then(|(project, bound, freshness)| {
                let value = render_result(client.call_tool_result(remote, &bound), None, &tool.name)?;
                Ok(json!({"project":project,"result":value,"freshness":freshness}))
            }));
        }
        Some(render_result(client.call_tool_result(remote, args), tool.output_schema.as_ref(), &tool.name))
    }

    fn call_resource(&self, args: &Json) -> Result<Json, String> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let mut services = self.services.clone();
        services.retain(|_, client| client.supports("resources"));
        if services.is_empty() {
            return Err("mcp_resource needs a bound MCP service".into());
        }
        let requested = args.get("service").and_then(|v| v.as_str()).unwrap_or("");
        let (namespace, client) = if requested.is_empty() && services.len() == 1 {
            services.into_iter().next().expect("one bound MCP service")
        } else if requested.is_empty() {
            return Err("mcp_resource.service is required when multiple MCP services are bound".into());
        } else {
            let Some(service) = services.remove(requested) else {
                let mut known: Vec<&str> = services.keys().map(String::as_str).collect();
                known.sort_unstable();
                return Err(format!("unknown MCP service {requested:?}; known: {}", known.join(", ")));
            };
            (requested.to_string(), service)
        };
        let result = match action {
            "list" => client.resources(args.get("cursor").and_then(|v| v.as_str())),
            "read" => {
                let Some(uri) = args.get("uri").and_then(|v| v.as_str()).filter(|uri| !uri.is_empty()) else {
                    return Err("mcp_resource.uri must be a non-empty string for action read".into());
                };
                client.read_resource(uri)
            }
            other => Err(format!("unknown mcp_resource action {other:?} (use list|read)")),
        };
        render_result(result.map(|value| json!({"service": namespace, "result": value})), None, "mcp_resource")
    }

    /// Ready-to-advertise function schemas for the member's model call.
    pub fn schemas(&self) -> Vec<Json> {
        let mut schemas: Vec<Json> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect();
        if self.services.values().any(|s| s.supports("resources")) {
            schemas.push(json!({
                "name": "mcp_resource",
                "description": "List or read resources from a bound MCP service (binding is the authorization). Use action list with an optional cursor, or action read with a non-empty uri. service is required only when multiple services are bound.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["list", "read"]},
                        "service": {"type": "string"},
                        "uri": {"type": "string"},
                        "cursor": {"type": "string"},
                    },
                    "required": ["action"],
                },
            }));
        }
        schemas
    }

    pub fn docs(&self) -> Vec<(String, String)> {
        self.tools.iter().map(|t| (t.name.clone(), t.description.clone())).collect()
    }

    pub fn close(&self) {
        for client in self.services.values() {
            client.close();
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
        let listed = match load_service(name, binding, root, true) {
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

fn load_service(
    name: &str,
    binding: &ToolBinding,
    root: &Path,
    extra_roots: bool,
) -> Result<(Arc<McpClient>, Vec<BoundTool>), String> {
    if binding.mcp_execution.as_deref().is_some_and(|mode| mode != "host") {
        return Err(format!(
            "binding {name:?}: MCP workspace isolation was removed (D-480); remove mcp_execution to use the host"
        ));
    }
    let transport = binding.mcp_transport.clone().unwrap_or_else(|| "stdio".into());
    let projects = binding
        .codegraph_projects
        .as_ref()
        .map(|extra| CodeGraphRoots::new(root, if extra_roots { extra } else { &[] }).map(Arc::new))
        .transpose()?;
    let client = match (transport.as_str(), projects.as_ref()) {
        ("stdio", Some(roots)) => McpClient::connect_codegraph(
            binding.command.as_deref().ok_or("CodeGraph needs a command")?,
            roots.start_root()?,
            binding.startup_timeout_s.unwrap_or(60),
            binding.tool_timeout_s.unwrap_or(120),
        )?,
        ("stdio", None) => {
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
                binding.startup_timeout_s.unwrap_or(60),
                binding.tool_timeout_s.unwrap_or(120),
            )?
        }
        ("http", _) => {
            let url =
                binding.url.clone().ok_or_else(|| format!("binding {name:?} needs a url for the http transport"))?;
            // The credential is resolved *before* the client connects: `connect_http` performs the initialize
            // handshake, so a token read any later would arrive after the request that needs it (D-412).
            //
            // * `bearer_token_env_var` names the variable, and a variable that is not set is a hard error, never a
            //   silent empty secret.
            // * `oauth = true` reads the credential `teamagents mcp login` stored beside the config, refreshing it
            //   if the server issued a refresh token. No credential yet is *not* an error here: the client
            //   connects unauthenticated on purpose, and the server's own 401 carries the `mcp login` sentence —
            //   a better place to learn it than a load-time guess. An expired credential with nothing to refresh
            //   *is* an error, and `access_token` states it.
            let (token, refresh) = match (&binding.bearer_token_env_var, binding.oauth) {
                (Some(var), _) => (
                    Some(
                        std::env::var(var)
                            .map_err(|_| format!("binding {name:?}: bearer token env var {var} is not set"))?,
                    ),
                    None,
                ),
                (None, true) => {
                    // Login records the exact resource string from this binding; do not normalize it into
                    // another audience here (including path, query or trailing slash changes).
                    let expected_resource = crate::mcp_auth::params(binding)?.resource;
                    let store = crate::mcp_auth::Store::at(&crate::mcp_auth::Store::default_path());
                    let token = store
                        .access_token_for(
                            name,
                            &expected_resource,
                            std::time::Duration::from_secs(binding.startup_timeout_s.unwrap_or(60)),
                        )
                        .map_err(|error| format!("binding {name:?}: {error}"))?;
                    let service = name.to_string();
                    let refresher: crate::mcp::TokenRefresh = std::sync::Arc::new(move |timeout| {
                        store.refresh_access_token_for(&service, &expected_resource, timeout)
                    });
                    (token, Some(refresher))
                }
                (None, false) => (None, None),
            };
            McpClient::connect_http_auth(
                &url,
                token,
                refresh,
                binding.startup_timeout_s.unwrap_or(60),
                binding.tool_timeout_s.unwrap_or(120),
            )?
        }
        ("sse", _) => {
            return Err(format!(
                "binding {name:?}: the MCP \"sse\" transport was removed from the spec; use \"http\" (streamable HTTP)"
            ))
        }
        (other, _) => return Err(format!("MCP transport {other:?} is not implemented (use \"stdio\" or \"http\")")),
    };
    // the workspace is what a server gets when it asks for `roots/list`
    if projects.is_none() {
        client.set_workspace(root);
    }
    let service = binding.mcp_server.clone().unwrap_or_else(|| name.to_string());
    let allowed: HashSet<&str> = binding.tool_names.iter().map(|s| s.as_str()).collect();
    let mut out = vec![];
    let listing = if client.supports("resources") && !client.supports("tools") { Ok(vec![]) } else { client.tools() };
    if projects.is_some() && client.supports("resources") {
        client.close();
        return Err("unexpected CodeGraph resources capability".into());
    }
    let listed = match listing {
        Ok(tools) => tools,
        Err(error) => {
            client.close();
            return Err(error);
        }
    };
    for tool in listed {
        let remote_name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if remote_name.is_empty() || (projects.is_some() && remote_name != "codegraph_explore") {
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
            projects: projects.clone(),
            remote: Some((client.clone(), remote_name)),
        });
    }
    if projects.is_some() && out.is_empty() {
        client.close();
        return Err("native CodeGraph did not offer the selected explore tool".into());
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

    #[test]
    fn codegraph_uses_exact_index_roots_and_never_falls_back_to_an_ancestor() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("ta-codegraph-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("work/.codegraph")).unwrap();
        crate::scratch_owner::record_ownership(&root);
        let work = root.join("work");
        let extra = root.join("extra");
        let child = work.join("child");
        std::fs::create_dir_all(extra.join(".codegraph")).unwrap();
        std::fs::create_dir(&child).unwrap();
        for p in [&work, &extra] {
            rusqlite::Connection::open(p.join(".codegraph/codegraph.db"))
                .unwrap()
                .execute_batch("CREATE TABLE nodes(id INTEGER)")
                .unwrap();
        }
        let roots = CodeGraphRoots::new(&work, &[extra.display().to_string(), child.display().to_string()]).unwrap();
        assert_eq!(roots.arguments(&json!({"query":"test"})).unwrap().1["projectPath"], work.to_str().unwrap());
        assert!(roots.arguments(&json!({"query":"q","projectPath":extra})).is_ok());
        for denied in [&child, &root] {
            assert!(roots.arguments(&json!({"query":"q","projectPath":denied})).is_err());
        }
        assert!(CodeGraphRoots::new(&work, &[]).unwrap().arguments(&json!({"query":"q","projectPath":extra})).is_err());
        std::fs::create_dir(child.join(".codegraph")).unwrap();
        std::fs::write(child.join(".codegraph/codegraph.db"), b"not a database").unwrap();
        assert!(roots.arguments(&json!({"query":"q","projectPath":child})).is_err());
        std::fs::remove_dir_all(child.join(".codegraph")).unwrap();
        symlink(work.join(".codegraph"), child.join(".codegraph")).unwrap();
        assert!(roots.arguments(&json!({"query":"q","projectPath":child})).is_err());
        std::fs::remove_dir_all(root).unwrap();
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
        crate::scratch_owner::record_ownership(&root);
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
    fn resource_only_and_paged_services_keep_their_declared_contract() {
        let root = std::env::temp_dir().join(format!("ta-mcp-contract-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        crate::scratch_owner::record_ownership(&root);
        let script = r#"
import json, os, sys, time
mode=sys.argv[1]
page=0
open(mode+'.pid','w').write(str(os.getpid()))
for line in sys.stdin:
    request=json.loads(line)
    if 'id' not in request: continue
    if request.get('method') is None:
        assert request['id']=='server-roots'
        open(mode+'.roots','w').write('answered')
        continue
    method=request['method']
    if method=='initialize':
        result={'protocolVersion':'2099-01-01' if mode=='future' else '2025-06-18',
                'capabilities':{'resources':{}} if mode in ('resources','future') else {'tools':{}}}
    elif method=='tools/list':
        assert mode!='resources', 'resource-only servers have no tools/list'
        if mode=='slow':
            time.sleep(0.4)
            page+=1
            result={'tools':[], 'nextCursor':str(page)}
        elif mode=='loop': result={'tools':[], 'nextCursor':'same'}
        elif request.get('params',{}).get('cursor')=='page-2': result={'tools':[{'name':'second'}]}
        else: result={'tools':[{'name':'first'}], 'nextCursor':'page-2'}
    elif method=='resources/list': result={'resources':[{'uri':'memo://one'}], 'nextCursor':'next'}
    elif method=='resources/read': result={'contents':[{'uri':'memo://one','text':'one'}]}
    else: raise AssertionError(method)
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
    if method=='initialize': print(json.dumps({'jsonrpc':'2.0','id':'server-roots','method':'roots/list'}),flush=True)
"#;
        for mode in ["resources", "pages", "loop", "future", "slow"] {
            let mut catalog = UserConfig::default();
            catalog.tools.insert(
                "probe".into(),
                binding(json!({"kind":"mcp","command":"/usr/bin/python3",
                "args":["-u","-c",script,mode],"required":true,"startup_timeout_s":if mode=="slow" {1} else {3}})),
            );
            let began = std::time::Instant::now();
            let loaded = BoundTools::load_in(&catalog, &[], &root);
            if matches!(mode, "loop" | "future" | "slow") {
                let error = loaded.err().expect("invalid peer must fail");
                let expected = match mode {
                    "loop" => "repeated a cursor",
                    "future" => "unsupported MCP protocolVersion",
                    _ => "timed out",
                };
                assert!(error.contains(expected), "{error}");
                if mode == "slow" {
                    assert!(began.elapsed() < std::time::Duration::from_secs(5));
                }
            } else {
                let bound = loaded.unwrap();
                if mode == "resources" {
                    assert!(bound.tools.is_empty());
                    assert!(bound.schemas().iter().any(|schema| schema["name"] == "mcp_resource"));
                    let page = bound.call("mcp_resource", &json!({"action":"list"})).unwrap().unwrap();
                    assert_eq!(page["result"]["nextCursor"], "next");
                    let read =
                        bound.call("mcp_resource", &json!({"action":"read","uri":"memo://one"})).unwrap().unwrap();
                    assert_eq!(read["result"]["contents"][0]["text"], "one");
                } else {
                    assert_eq!(bound.names(), HashSet::from(["probe_first", "probe_second"]));
                }
                assert!(root.join(format!("{mode}.roots")).exists(), "string request ID was not answered");
                bound.close();
            }
            let pid = std::fs::read_to_string(root.join(format!("{mode}.pid"))).unwrap();
            assert!(!Path::new(&format!("/proc/{pid}")).exists(), "{mode} leaked its service");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn filtered_and_failed_services_reap_started_processes() {
        let root = std::env::temp_dir().join(format!("ta-mcp-cleanup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        crate::scratch_owner::record_ownership(&root);
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
