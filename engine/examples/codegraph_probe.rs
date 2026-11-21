//! Real TeamAgents -> MCP -> codemode -> CodeGraph integration, without a model.
//!
//! The probe uses the shared CodeGraph daemon when one is available and never
//! initializes an index. Graph output is evidence for navigation, not a
//! substitute for compilation or tests.

use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use teamagents_core::models::UserConfig;
use teamagents_engine::{bound::BoundTools, codemode};

fn usage() {
    println!(
        "Usage: codegraph_probe [WORKSPACE] [--query TEXT]\n\
              Requires an existing WORKSPACE/.codegraph index and the codegraph executable on PATH.\n\
              Does not initialize an index or call a model."
    );
}

fn arguments() -> Result<(PathBuf, String), String> {
    let mut workspace = None;
    let mut query = "web_search parse_search_response".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                usage();
                std::process::exit(0);
            }
            "--query" => {
                query = args.next().ok_or_else(|| "--query needs a value".to_string())?;
                if query.trim().is_empty() {
                    return Err("--query must not be empty".into());
                }
            }
            _ if arg.starts_with('-') => return Err(format!("unknown argument {arg:?}")),
            _ if workspace.is_none() => workspace = Some(PathBuf::from(arg)),
            _ => return Err(format!("unexpected extra argument {arg:?}")),
        }
    }
    let workspace = match workspace {
        Some(path) => path,
        None => std::env::current_dir().map_err(|error| error.to_string())?,
    };
    let workspace = workspace.canonicalize().map_err(|error| format!("{}: {error}", workspace.display()))?;
    if !workspace.join(".codegraph").is_dir() {
        return Err(format!(
            "{} has no .codegraph index; initialize it explicitly with CodeGraph before running this read-only probe",
            workspace.display()
        ));
    }
    Ok((workspace, query))
}

fn catalog() -> UserConfig {
    let mut catalog = UserConfig::default();
    catalog.tools.insert(
        "codegraph".into(),
        serde_json::from_value(json!({
            "kind": "mcp",
            "command": "codegraph",
            "args": ["serve", "--mcp"],
            "codegraph_projects": [],
            "tool_names": ["codegraph_explore", "codegraph_search", "codegraph_callers", "codegraph_callees", "codegraph_impact", "codegraph_status"],
            "required": true,
            "startup_timeout_s": 60,
            "tool_timeout_s": 120
        }))
        .expect("the fixed CodeGraph binding is valid"),
    );
    catalog
}

fn explore(workspace: &Path, query: &str) -> Result<String, String> {
    let bindings = ["files".to_string(), "shell".to_string(), "web".to_string(), "skills".to_string()];
    let bound = Arc::new(BoundTools::load_in(&catalog(), &bindings, workspace)?);
    if !bound.tools.iter().any(|tool| tool.name == "codegraph_codegraph_explore") {
        let names: Vec<_> = bound.tools.iter().map(|tool| tool.name.as_str()).collect();
        return Err(format!("CodeGraph explore was not discovered; bound tools: {names:?}"));
    }
    let call_args = json!({"query": query, "projectPath": workspace, "maxFiles": 2}).to_string();
    let source = r#"
const namespace = describeNamespace("codegraph");
if (!namespace || !namespace.tools.some((tool) => tool.name === "codegraph_codegraph_explore")) {
  throw new Error("CodeGraph namespace is missing explore");
}
const args = __CALL_ARGS__;
const result = await tools.codegraph_codegraph_explore(args);
if (result.project !== args.projectPath || result.freshness.status !== "not_verified") throw new Error("missing project or freshness evidence");
if (!result.result.includes("Source Code")) throw new Error(result.result.slice(0, 1000));
text(result.result.slice(0, 4000));
for (const [suffix, params] of Object.entries({status: {}, search: {query: args.query.split(" ")[0], limit: 3}, callers: {symbol: args.query.split(" ")[0]}, callees: {symbol: args.query.split(" ")[0]}, impact: {symbol: args.query.split(" ")[0], depth: 1}})) {
  const reply = await tools["codegraph_codegraph_" + suffix]({...params, projectPath: args.projectPath});
  if (reply.project !== args.projectPath || !reply.result || reply.freshness.status !== "not_verified") throw new Error("missing scoped " + suffix + " result");
  text(suffix + ": " + reply.result.slice(0, 100));
}
"#
    .replace("__CALL_ARGS__", &call_args);
    let outcome = codemode::run(
        &source,
        codemode::Codemode {
            tools: bound,
            store: Arc::new(Mutex::new(BTreeMap::<String, Json>::new())),
            veto: None,
            timeout_ms: 120_000,
            aborted: false,
        },
    );
    if let Some(error) = outcome.error {
        return Err(format!("codemode failed: {error}\n{}", outcome.output));
    }
    if outcome.calls.len() != 6 || outcome.calls.iter().any(|call| call.status != "ok") {
        return Err(format!("unexpected nested-call receipt: {:?}", outcome.calls));
    }
    Ok(outcome.output)
}

fn run() -> Result<(), String> {
    let (workspace, query) = arguments()?;
    let output = explore(&workspace, &query)?;
    println!("PASS: TeamAgents MCP discovery, codemode namespace lookup, and real CodeGraph query");
    println!("{output}");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("codegraph_probe: {error}");
        std::process::exit(1);
    }
}
