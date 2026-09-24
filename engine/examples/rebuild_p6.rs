//! R2-P6 performance experiment runner (§13): one trial of one task in one of
//! the three pre-registered groups.
//!
//!   A 单实例参考：同一个 kernel/协议/工具/上下文策略，直驱循环，无团队管理
//!   B 持久化单实例：A 的模型可见内容 + 新持久化运行时（v2 驱动）
//!   C 按需协作：B + 可见协作能力（spawn/delegate/send/wait 授权 + 协作指令）
//!
//!   cargo run --offline --manifest-path engine/Cargo.toml --example rebuild_p6 -- \
//!     --group A|B|C --task-file FILE --workdir DIR --state DIR --out FILE \
//!     [--model KEY] [--id TASK_ID] [--timeout S] [--max-steps N] [--web]
//!
//! A/B keep identical instructions, tools, options and window (the manifest
//! freezes them); C adds the collaboration paragraph and grants, which is the
//! experiment's treatment and is billed like everything else.

use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use teamagents_core::kernel::{KernelProfile, Usage};
use teamagents_core::v2::{Command, Control};
use teamagents_engine::config::{load_user_config, user_config_path};
use teamagents_engine::providers::build_for_model;
use teamagents_engine::reference::{basic_tool_schemas, run_reference, ReferenceConfig, ReferenceEnd};
use teamagents_engine::v2::driver::{start, DriverConfig};
use teamagents_engine::v2::supervisor::{start as start_supervisor, SupervisorConfig};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const GROUP_A: &str = "A";
const GROUP_B: &str = "B";
const GROUP_C: &str = "C";

fn usage() -> ! {
    eprintln!(
        "usage: rebuild_p6 --group A|B|C --task-file FILE --workdir DIR --state DIR --out FILE \\
[--model KEY] [--id TASK_ID] [--timeout S] [--max-steps N] [--web]"
    );
    std::process::exit(2);
}

/// A/B instructions: the model-visible surface the manifest freezes.
fn agent_instructions(workspace: &str) -> String {
    format!(
        "You are a careful coding agent working in the workspace directory {workspace}.\n\
         - Use the file and shell tools to inspect, modify and verify. Verify claims with real commands before finishing.\n\
         - Long tool outputs are masked with a read_history recipe; page them back instead of re-running blind.\n\
         - Finish by calling `finish` exactly once with the honest status, a summary and evidence (files, commands).\n\
           Work you did not deliver must not be reported as success; list unverified claims in `unverified`.\n\
         - Today's date: 2026-09-24. Platform: linux.",
        workspace = workspace,
    )
}

/// C's treatment: the same instructions plus the collaboration vocabulary.
fn team_instructions(workspace: &str) -> String {
    format!(
        "{}\n\
         - You may build a team: spawn worker instances, delegate bounded tasks, send messages and wait for results. \
Work directly on small or tightly coupled work; delegate only work that can progress independently, and keep every \
task description specific with acceptance criteria.",
        agent_instructions(workspace)
    )
}

struct Args {
    group: String,
    task_file: PathBuf,
    workdir: PathBuf,
    state: PathBuf,
    out: PathBuf,
    model: String,
    id: String,
    timeout_s: u64,
    max_steps: usize,
    web: bool,
}

fn parse_args() -> Args {
    let mut args = std::env::args().skip(1);
    let mut group = None;
    let mut task_file = None;
    let mut workdir = None;
    let mut state = None;
    let mut out = None;
    let mut model = "leader_main".to_string();
    let mut id = "task".to_string();
    let mut timeout_s = 900u64;
    let mut max_steps = 40usize;
    let mut web = false;
    while let Some(flag) = args.next() {
        let value = |args: &mut std::iter::Skip<std::env::Args>| args.next().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--group" => group = Some(value(&mut args)),
            "--task-file" => task_file = Some(PathBuf::from(value(&mut args))),
            "--workdir" => workdir = Some(PathBuf::from(value(&mut args))),
            "--state" => state = Some(PathBuf::from(value(&mut args))),
            "--out" => out = Some(PathBuf::from(value(&mut args))),
            "--model" => model = value(&mut args),
            "--id" => id = value(&mut args),
            "--timeout" => timeout_s = value(&mut args).parse().unwrap_or_else(|_| usage()),
            "--max-steps" => max_steps = value(&mut args).parse().unwrap_or_else(|_| usage()),
            "--web" => web = true,
            _ => usage(),
        }
    }
    let (Some(group), Some(task_file), Some(workdir), Some(state), Some(out)) = (group, task_file, workdir, state, out)
    else {
        usage()
    };
    if !matches!(group.as_str(), GROUP_A | GROUP_B | GROUP_C) {
        usage();
    }
    Args { group, task_file, workdir, state, out, model, id, timeout_s, max_steps, web }
}

fn main() -> Fallible<()> {
    let args = parse_args();
    let task = std::fs::read_to_string(&args.task_file)?;
    let catalog = load_user_config(&user_config_path())?;
    let entry = catalog
        .models
        .get(&args.model)
        .ok_or_else(|| format!("model key {:?} is not in the user catalog", args.model))?
        .clone();
    if entry.context_window.is_none() {
        return Err(format!("model {:?} has no declared native context window (D-36)", args.model).into());
    }
    // shell jobs run in the production runner (`teamagents` next to this example)
    if std::env::var_os("TEAMAGENTS_RUNNER_BIN").is_none() {
        let runner = std::env::current_exe()?
            .parent()
            .and_then(|dir| dir.parent())
            .map(|dir| dir.join("teamagents"))
            .filter(|path| path.is_file())
            .ok_or("cannot locate the teamagents runner binary; set TEAMAGENTS_RUNNER_BIN")?;
        std::env::set_var("TEAMAGENTS_RUNNER_BIN", runner);
    }
    // the runner prepares the workspace (fixture copied in) and owns the trial
    // bookkeeping; only a *reused* state root would silently mix two trials
    if args.state.join("session.sqlite").exists() {
        return Err(format!(
            "state {} already holds a session; each trial needs a fresh state root",
            args.state.display()
        )
        .into());
    }
    std::fs::create_dir_all(&args.workdir)?;
    std::fs::create_dir_all(&args.state)?;
    let workspace = std::fs::canonicalize(&args.workdir)?;
    let state = std::fs::canonicalize(&args.state)?;
    let bindings: Vec<String> = if args.web {
        vec!["files".into(), "shell".into(), "web".into(), "skills".into()]
    } else {
        vec!["files".into(), "shell".into(), "skills".into()]
    };
    let team = args.group == GROUP_C;
    let profile = KernelProfile {
        model: args.model.clone(),
        instructions: if team {
            team_instructions(&workspace.to_string_lossy())
        } else {
            agent_instructions(&workspace.to_string_lossy())
        },
        tools: basic_tool_schemas(args.web, false),
        // the manifest freezes effort=high for all three groups
        options: json!({"reasoning_effort": "high"}),
        context_window: entry.context_window,
    };
    // A/B use the resolved profile directly (there is no resolution step inside
    // the driver); C hands the *catalog-key* profile to the supervisor, which
    // resolves it per instance and looks the key up again in its factory.
    let raw_profile = profile.clone();
    let profile = teamagents_engine::providers::resolve_profile(profile, &catalog);
    let started = Instant::now();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?;
    let result = match args.group.as_str() {
        GROUP_A => {
            runtime.block_on(run_reference_trial(&catalog, &profile, &args, &task, &workspace, &state, &bindings))?
        }
        GROUP_B => runtime
            .block_on(run_driver_trial(&catalog, &profile, &args, &task, &workspace, &state, &bindings, false))?,
        _ => runtime.block_on(run_driver_trial(
            &catalog,
            &raw_profile,
            &args,
            &task,
            &workspace,
            &state,
            &bindings,
            true,
        ))?,
    };
    let mut report = result;
    report["group"] = json!(args.group);
    report["task_id"] = json!(args.id);
    report["model_key"] = json!(args.model);
    report["model"] = json!(entry.model);
    report["context_window"] = json!(entry.context_window);
    report["permissions"] = json!("full_auto");
    report["web"] = json!(args.web);
    report["wall_ms"] = json!(started.elapsed().as_millis() as u64);
    std::fs::write(&args.out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

/// Group A: the reference loop (same kernel, provider, tools, instructions).
#[allow(clippy::too_many_arguments)]
async fn run_reference_trial(
    catalog: &teamagents_core::models::UserConfig,
    profile: &KernelProfile,
    args: &Args,
    task: &str,
    workspace: &Path,
    state: &Path,
    bindings: &[String],
) -> Fallible<Json> {
    let provider = build_for_model(catalog, &args.model)?;
    let trace_dir = state.join("trace");
    std::fs::create_dir_all(&trace_dir)?;
    let config = ReferenceConfig {
        workspace: workspace.to_path_buf(),
        artifacts: Some(state.join("artifacts")),
        shell_state: Some(state.join("shell")),
        permissions: "full_auto".into(),
        profile: profile.clone(),
        catalog: catalog.clone(),
        bindings: bindings.to_vec(),
        max_steps: args.max_steps,
        max_retries: 2,
        deadline: Some(Duration::from_secs(args.timeout_s)),
        trace_dir: trace_dir.clone(),
        run_id: args.id.clone(),
    };
    let outcome = run_reference(&provider, config, task, |_| {}).await.map_err(|e| format!("reference: {e}"))?;
    let (status, detail) = match &outcome.end {
        ReferenceEnd::Completed(candidate) => ("completed".to_string(), json!(candidate)),
        ReferenceEnd::Reply(text) => ("reply".to_string(), json!(text.chars().take(400).collect::<String>())),
        ReferenceEnd::Failed(reason) => ("failed".to_string(), json!(reason)),
    };
    Ok(json!({
        "status": status,
        "detail": detail,
        "steps": outcome.steps,
        "prompt_tokens": outcome.usage.prompt,
        "completion_tokens": outcome.usage.completion,
        "total_tokens": outcome.usage.total,
        "trace": outcome.trace_path.to_string_lossy(),
    }))
}

/// Groups B/C: the persistent runtime (single instance; C adds collaboration).
#[allow(clippy::too_many_arguments)]
async fn run_driver_trial(
    catalog: &teamagents_core::models::UserConfig,
    profile: &KernelProfile,
    args: &Args,
    task: &str,
    workspace: &Path,
    state: &Path,
    bindings: &[String],
    team: bool,
) -> Fallible<Json> {
    let instance = if team { "i-leader" } else { "i-main" };
    let session_db = state.join("session.sqlite");
    let session_id = "s-p6";
    let mut events_seen: i64 = 0;
    let (status, usage, attempts) = if team {
        let catalog_for_factory = catalog.clone();
        let factory = move |_id: &str, profile: &KernelProfile| {
            // the profile may carry the catalog key or the resolved wire id
            if let Ok(provider) = build_for_model(&catalog_for_factory, &profile.model) {
                return provider;
            }
            let key = catalog_for_factory
                .models
                .iter()
                .find(|(_, entry)| entry.model == profile.model)
                .map(|(key, _)| key.clone())
                .unwrap_or_else(|| panic!("no catalog entry for {}", profile.model));
            build_for_model(&catalog_for_factory, &key).unwrap_or_else(|e| panic!("provider for {key}: {e}"))
        };
        let config = SupervisorConfig {
            marker: std::marker::PhantomData,
            session_db: session_db.to_path_buf(),
            session_id: session_id.into(),
            leader_id: instance.into(),
            leader_profile: profile.clone(),
            state_root: state.to_path_buf(),
            workspace: workspace.to_path_buf(),
            permissions: "full_auto".into(),
            catalog: catalog.clone(),
            bindings: bindings.to_vec(),
            max_retries: 2,
            storage_queue: 256,
            poll: Duration::from_millis(100),
            goal_limits: json!({}),
            require_shell_approval: false,
            provider_factory: factory,
        };
        let handle = start_supervisor(config).await?;
        for (action, scope) in [("manage", "session"), ("delegate", "session"), ("message", "session")] {
            handle
                .submit_user(Command {
                    command_id: format!("grant-{action}"),
                    method: "issue_grant".into(),
                    params: json!({"subject": instance, "action": action, "resource_scope": scope}),
                })
                .await?;
        }
        handle.input(instance, task).await?;
        let outcome = wait_goal(&session_db, session_id, instance, args.timeout_s, &mut events_seen).await?;
        handle.shutdown().await?;
        outcome
    } else {
        let provider = build_for_model(catalog, &args.model)?;
        let config = DriverConfig {
            session_db: session_db.to_path_buf(),
            session_id: session_id.into(),
            instance_id: instance.into(),
            state_root: state.to_path_buf(),
            workspace: workspace.to_path_buf(),
            permissions: "full_auto".into(),
            profile: profile.clone(),
            provider,
            catalog: catalog.clone(),
            bindings: bindings.to_vec(),
            max_retries: 2,
            storage_queue: 256,
            poll: Duration::from_millis(100),
            goal_limits: json!({}),
            require_shell_approval: false,
        };
        let handle = start(config).await?;
        handle.input(task).await?;
        let outcome = wait_goal(&session_db, session_id, instance, args.timeout_s, &mut events_seen).await?;
        handle.shutdown().await?;
        outcome
    };
    Ok(json!({
        "status": status,
        "steps": attempts,
        "prompt_tokens": usage.prompt,
        "completion_tokens": usage.completion,
        "total_tokens": usage.total,
        "events": events_seen,
    }))
}

/// Terminal goal status, idle reply, or the deadline: the same stopping rule
/// for B and C (§13.1 keeps the two groups comparable).
async fn wait_goal(
    session_db: &Path,
    session_id: &str,
    instance: &str,
    timeout_s: u64,
    events_seen: &mut i64,
) -> Fallible<(String, Usage, usize)> {
    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    loop {
        let control = Control::open(session_db, session_id, false)?;
        let conn = control.connection();
        let goal: Option<(String, String)> = conn
            .query_row("SELECT status, known_usage_json FROM goals LIMIT 1", [], |row| Ok((row.get(0)?, row.get(1)?)))
            .ok();
        let attempts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM attempts a JOIN model_requests r ON r.request_id = a.request_id
                 WHERE r.instance_id = ?1 AND a.status = 'COMPLETE'",
                [instance],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let max_sequence: i64 = conn
            .query_row("SELECT COALESCE(MAX(sequence), 0) FROM events WHERE session_id = ?1", [session_id], |row| {
                row.get(0)
            })
            .unwrap_or(0);
        *events_seen = max_sequence;
        let idle: Option<String> = conn
            .query_row(
                "SELECT message_json FROM context_entries WHERE instance_id = ?1 ORDER BY epoch DESC, idx DESC LIMIT 1",
                [instance],
                |row| row.get(0),
            )
            .ok();
        let phase: Option<String> =
            conn.query_row("SELECT phase FROM instances WHERE id = ?1", [instance], |row| row.get(0)).ok();
        drop(control);
        let usage = goal.as_ref().and_then(|(_, known)| serde_json::from_str::<Usage>(known).ok()).unwrap_or_default();
        if let Some((status, _)) = &goal {
            if matches!(status.as_str(), "SUCCEEDED" | "FAILED" | "BLOCKED" | "CANCELLED") {
                return Ok((status.to_ascii_lowercase(), usage, attempts as usize));
            }
        }
        // an assistant reply that ends the turn without finish (the same shape
        // the P2 driver reports) stops the trial as a reply
        if phase.as_deref() == Some("READY") {
            if let Some(raw) = idle {
                let message: Json = serde_json::from_str(&raw).unwrap_or(Json::Null);
                if message["role"] == json!("assistant") && !message["content"].as_str().unwrap_or("").is_empty() {
                    return Ok(("reply".to_string(), usage, attempts as usize));
                }
            }
        }
        if Instant::now() > deadline {
            return Ok(("timeout".to_string(), usage, attempts as usize));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
