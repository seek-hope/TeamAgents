//! R2-P2 persistent-driver validation CLI: one task through the v2 phase
//! machine (SQLite-backed, crash-safe) with the real DeepSeek edge.
//!
//!   cargo run --offline --manifest-path engine/Cargo.toml --example eval_group_b -- \
//!     --task "..." --workdir /tmp/task --trace /tmp/trace
//!
//! This is the eval group-B counterpart to eval_group_a (RV-38): same kernel,
//! same tools, same model configuration — but every transition is persisted
//! through the v2 control plane. Credentials come from the environment.

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use teamagents_core::kernel::KernelProfile;
use teamagents_core::models::{ToolBinding, UserConfig};
use teamagents_engine::providers::chat_completions::ChatCompletions;
use teamagents_engine::reference::basic_tool_schemas;
use teamagents_engine::v2::driver::{start, DriverConfig};

fn usage() -> ! {
    eprintln!(
        "usage: eval_group_b --task TEXT|--task-file PATH --workdir DIR --trace DIR \\
[--model deepseek-flash] [--base URL] [--api-key-env DEEPSEEK_API_KEY] [--window N] \\
[--effort max] [--timeout S] [--retries N] [--full-auto] [--web]"
    );
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut task = None;
    let mut task_file = None;
    let mut workdir: Option<PathBuf> = None;
    let mut trace_dir: Option<PathBuf> = None;
    let mut model = "deepseek-flash".to_string();
    let mut base = "https://api.deepseek.com/v1".to_string();
    let mut key_env = "DEEPSEEK_API_KEY".to_string();
    let mut window = 1_000_000u64;
    let mut effort = "max".to_string();
    let mut timeout_s = 900u64;
    let mut retries = 5usize;
    let mut full_auto = false;
    let mut web = false;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let value = |i: &mut usize| -> String {
            *i += 1;
            args.get(*i).cloned().unwrap_or_else(|| usage())
        };
        match flag {
            "--task" => task = Some(value(&mut i)),
            "--task-file" => task_file = Some(PathBuf::from(value(&mut i))),
            "--workdir" => workdir = Some(PathBuf::from(value(&mut i))),
            "--trace" => trace_dir = Some(PathBuf::from(value(&mut i))),
            "--model" => model = value(&mut i),
            "--base" => base = value(&mut i),
            "--api-key-env" => key_env = value(&mut i),
            "--window" => window = value(&mut i).parse().unwrap_or_else(|_| usage()),
            "--effort" => effort = value(&mut i),
            "--timeout" => timeout_s = value(&mut i).parse().unwrap_or_else(|_| usage()),
            "--retries" => retries = value(&mut i).parse().unwrap_or_else(|_| usage()),
            "--full-auto" => full_auto = true,
            "--web" => web = true,
            _ => usage(),
        }
        i += 1;
    }
    let (Some(workdir), Some(trace_dir)) = (workdir, trace_dir) else { usage() };
    let task = match (task, task_file) {
        (Some(text), None) => text,
        (None, Some(path)) => std::fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("cannot read {}: {e}", path.display());
            std::process::exit(2);
        }),
        _ => usage(),
    };
    let api_key = std::env::var(&key_env).unwrap_or_else(|_| {
        eprintln!("missing API key env {key_env}");
        std::process::exit(2);
    });
    std::fs::create_dir_all(&workdir).expect("workdir");
    std::fs::create_dir_all(&trace_dir).expect("trace");
    let workspace = std::fs::canonicalize(&workdir).expect("canonical workdir");
    let state = workspace.join(".teamagents-v2");
    let session_db = state.join("session.sqlite");

    let mut catalog = UserConfig::default();
    if web {
        let mut tools: HashMap<String, ToolBinding> = HashMap::new();
        let search = ToolBinding {
            kind: "web_search".into(),
            provider: Some("anysearch".into()),
            url: Some("https://api.anysearch.com/v1/search".into()),
            api_key_env: Some("ANYSEARCH_API_KEY".into()),
            ..Default::default()
        };
        let fetch = ToolBinding { kind: "web_fetch".into(), provider: Some("anysearch".into()), ..Default::default() };
        tools.insert("web".into(), search);
        tools.insert("fetch".into(), fetch);
        catalog.tools = tools;
    }
    let bindings = if web { vec!["web".to_string()] } else { vec![] };
    let date = "2026-09-23";
    let instructions = format!(
        "You are a careful coding agent working in the workspace directory {workspace}.\n\
         - Use the file and shell tools to inspect, modify and verify. Verify claims with real commands before finishing.\n\
         - Long tool outputs are masked with a read_history recipe; page them back instead of re-running blind.\n\
         - Finish by calling `finish` exactly once with the honest status, a summary and evidence (files, commands).\n\
           Work you did not deliver must not be reported as success; list unverified claims in `unverified`.\n\
         - Today's date: {date}. Platform: linux.",
        workspace = workspace.display(),
    );

    // the jobs runner is a hidden subcommand of the teamagents binary (§6.2);
    // this example's own exe cannot serve it, so point the spawn hook at the
    // sibling teamagents binary unless the caller already chose one
    if std::env::var_os("TEAMAGENTS_RUNNER_BIN").is_none() {
        let candidate = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().and_then(|examples| examples.parent()).map(|debug| debug.join("teamagents")));
        match candidate {
            Some(path) if path.is_file() => std::env::set_var("TEAMAGENTS_RUNNER_BIN", &path),
            _ => {
                eprintln!("teamagents binary not found next to the example; build it first (cargo build --manifest-path engine/Cargo.toml)");
                std::process::exit(2);
            }
        }
    }

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    let provider = ChatCompletions::new(base, api_key, Duration::from_secs(timeout_s.max(120))).expect("provider");
    let config = DriverConfig {
        session_db: session_db.clone(),
        session_id: "s-p2".into(),
        instance_id: "i-main".into(),
        state_root: state.clone(),
        instances_dir: state.clone().join("instances"),
        workspace: workspace.clone(),
        permissions: if full_auto { "full_auto".into() } else { "approved_scope".into() },
        profile: KernelProfile {
            model: model.clone(),
            instructions,
            tools: basic_tool_schemas(web, false),
            options: json!({"reasoning_effort": effort}),
            context_window: Some(window),
        },
        provider,
        catalog,
        bindings,
        max_retries: retries,
        storage_queue: 256,
        poll: Duration::from_millis(100),
        goal_limits: json!({}),
        require_shell_approval: !full_auto,
    };

    let report = runtime.block_on(run(config, &task, &session_db, timeout_s));
    let events = runtime.block_on(dump_events(&session_db));
    let mut log = String::new();
    for event in &events {
        log.push_str(&event.to_string());
        log.push('\n');
    }
    std::fs::write(trace_dir.join("events.jsonl"), &log).expect("events.jsonl");
    let mut out = report.as_object().unwrap().clone();
    out.insert("events".into(), json!(events.len()));
    out.insert("state".into(), json!(state.to_string_lossy()));
    let rendered = Json::Object(out.clone());
    std::fs::write(trace_dir.join("report.json"), rendered.to_string()).expect("report.json");
    println!("{rendered}");
    let failed = out.get("end").and_then(|e| e.as_str()) != Some("completed")
        || out.get("goal_status").and_then(|s| s.as_str()) != Some("SUCCEEDED");
    if failed {
        std::process::exit(1);
    }
}

/// Drive one task to a terminal state: goal completion (finish tool), an
/// idle assistant reply (no finish), or the overall deadline.
async fn run(config: DriverConfig<ChatCompletions>, task: &str, session_db: &std::path::Path, timeout_s: u64) -> Json {
    let handle = match start(config).await {
        Ok(handle) => handle,
        Err(error) => return json!({"end": "failed", "reason": format!("driver start: {error}")}),
    };
    if let Err(error) = handle.input(task).await {
        return json!({"end": "failed", "reason": format!("input: {error}")});
    }
    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    let mut last_seq = 0i64;
    let report = loop {
        if Instant::now() > deadline {
            break json!({"end": "timeout"});
        }
        if let Ok(events) = handle.events(last_seq).await {
            for event in &events {
                last_seq = last_seq.max(event["sequence"].as_i64().unwrap_or(last_seq));
                let kind = event["kind"].as_str().unwrap_or("");
                if kind != "output" {
                    eprintln!("[event] {kind} {}", event["payload"]);
                }
                if kind == "goal_completed" {
                    let _ = std::io::stderr().flush();
                    return json!({"end": "completed",
                                  "goal_status": event["payload"]["status"],
                                  "completion": event["payload"]["completion"]});
                }
            }
        }
        // idle check: READY with the assistant holding the last word and no
        // active request means the model replied without calling finish
        if let Some(idle) = idle_reply(session_db) {
            let _ = handle.shutdown().await;
            return json!({"end": "reply", "text": idle});
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    let _ = handle.shutdown().await;
    report
}

/// Last context entry is the assistant's while the instance is READY: the
/// turn ended without a finish call. Reads go through a second short-lived
/// connection (WAL readers do not block the storage worker).
fn idle_reply(session_db: &std::path::Path) -> Option<String> {
    let control = teamagents_core::v2::Control::open(session_db, "s-p2", false).ok()?;
    let conn = control.connection();
    let phase: String = conn.query_row("SELECT phase FROM instances WHERE id = 'i-main'", [], |row| row.get(0)).ok()?;
    if phase != "READY" {
        return None;
    }
    let last: Option<(String, String)> = conn
        .query_row(
            "SELECT kind, message_json FROM context_entries WHERE instance_id = 'i-main'
             ORDER BY epoch DESC, idx DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();
    let (kind, message) = last?;
    if kind != "assistant" {
        return None;
    }
    let message: Json = serde_json::from_str(&message).ok()?;
    // a real reply has visible text and no tool calls
    if message.get("tool_calls").is_some() {
        return None;
    }
    Some(message["content"].as_str().unwrap_or("").to_string())
}

async fn dump_events(session_db: &std::path::Path) -> Vec<Json> {
    let path = session_db.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let Ok(control) = teamagents_core::v2::Control::open(&path, "s-p2", false) else { return vec![] };
        let mut stmt = match control
            .connection()
            .prepare("SELECT sequence, kind, scope, payload_json, created FROM events ORDER BY sequence")
        {
            Ok(stmt) => stmt,
            Err(_) => return vec![],
        };
        let rows = stmt.query_map([], |row| {
            Ok(json!({"sequence": row.get::<_, i64>(0)?, "kind": row.get::<_, String>(1)?,
                      "scope": row.get::<_, String>(2)?,
                      "payload": serde_json::from_str::<Json>(&row.get::<_, String>(3)?).unwrap_or(Json::Null),
                      "created": row.get::<_, f64>(4)?}))
        });
        match rows {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => vec![],
        }
    })
    .await
    .unwrap_or_default()
}
