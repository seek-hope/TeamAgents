//! R2-P6 performance experiment runner (§13): one trial of one task in one of
//! the three pre-registered groups.
//!
//!   A single-instance reference: same kernel/protocol/tools/context policy, direct loop, no team management
//!   B persistent single instance: A's model-visible content plus the persistent runtime (the driver)
//!   C collaboration on demand: B plus the visible collaboration surface (spawn/delegate/send/wait grants and instructions)
//!
//!   cargo run --offline --manifest-path engine/Cargo.toml --example eval_groups_abc -- \
//!     --group A|B|C --task-file FILE --workdir DIR --state DIR --out FILE \
//!     [--model KEY] [--id TASK_ID] [--timeout S] [--max-steps N] [--web]
//!
//! A/B keep identical instructions, tools, options and window; C adds the collaboration paragraph and
//! grants, which is the experiment's treatment and is billed like everything else. The manifests pin the
//! *harness's* half of that surface (the instruction templates, the offered tool names, the request options
//! and the limits, D-182) and every trial records it; the *product's* half (prompt assembly, the tools a
//! grant adds) is pinned by the batch's recorded commit. `--print-surface` prints the record for one flag
//! set without a model call, which is what `review/eval_surface.py` recomputes from this file's text.

use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use teamagents_core::kernel::{KernelProfile, Usage};
use teamagents_core::v2::{Command, Control};
use teamagents_engine::config::{load_user_config, user_config_path};
use teamagents_engine::providers::build_for_model;
use teamagents_engine::reference::{basic_tool_schemas, run_reference, ReferenceConfig, ReferenceEnd};
use teamagents_engine::v2::driver::{start, DriverConfig};
use teamagents_engine::v2::supervisor::{start as start_supervisor, SupervisorConfig, SupervisorHandle};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const GROUP_A: &str = "A";
const GROUP_B: &str = "B";
const GROUP_C: &str = "C";
/// D (D-254): the same runtime, tools and grants as C, with a **directive** instruction shape: the four recorded
/// rounds measured that C's permissive paragraph never produced a single spawn/delegate on this task set, so
/// "does collaboration pay?" was never separable from "does the model choose it?". D changes only the paragraph.
const GROUP_D: &str = "D";

fn usage() -> ! {
    eprintln!(
        "usage: eval_groups_abc --group A|B|C|D --task-file FILE --workdir DIR --state DIR --out FILE \\
[--model KEY] [--id TASK_ID] [--timeout S] [--max-steps N] [--web]   (or --group G --print-surface)"
    );
    std::process::exit(2);
}

/// A/B instructions: the model-visible surface the manifests pin by digest (D-182).
///
/// It is a *template*: `{workspace}` is substituted per trial and the digest is taken of the template, so
/// every trial of a group reports the same one and a reader can check the treatment from the trial's own
/// record. Changing this text changes the experiment: the manifest pins, the recorded trials' self-reported
/// digests and `review/eval_surface.py` move together or the audit fails.
const AGENT_INSTRUCTIONS: &str = "You are a careful coding agent working in the workspace directory {workspace}.\n\
     - Use the file and shell tools to inspect, modify and verify. Verify claims with real commands before finishing.\n\
     - Long tool outputs are masked with a read_history recipe; page them back instead of re-running blind.\n\
     - Finish by calling `finish` exactly once with the honest status, a summary and evidence (files, commands).\n\
       Work you did not deliver must not be reported as success; list unverified claims in `unverified`.\n\
     - Today's date: 2026-09-24. Platform: linux.";

/// C's treatment: A/B's text plus this paragraph, appended unchanged.
const TEAM_EXTRA: &str = "\n\
     - You may build a team: spawn worker instances, delegate bounded tasks, send messages and wait for results. \
Work directly on small or tightly coupled work; delegate only work that can progress independently, and keep every \
task description specific with acceptance criteria.";

/// D's treatment (D-254): the same collaboration surface with a directive instruction shape instead.
const TEAM_DIRECTIVE_EXTRA: &str = "\n\
     - Build a team for this task. Split it into the independent parts the prompt describes and spawn one worker \
instance per part **before** doing any of the work yourself; delegate each part with its own acceptance check, \
then wait for the members and integrate their results. Work directly only on what cannot be split.";

/// Which collaboration instruction shape a group runs (D-254). All three team styles share the runtime, the tools
/// and the grants; only the paragraph differs, and the trial records which one it ran.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Agent,
    Permissive,
    Directive,
}

fn style_of(group: &str) -> Style {
    match group {
        GROUP_D => Style::Directive,
        GROUP_C => Style::Permissive,
        _ => Style::Agent,
    }
}

impl Style {
    /// The name a trial records, so a reader can tell the treatment from the record alone.
    fn name(self) -> &'static str {
        match self {
            Style::Agent => "agent",
            Style::Permissive => "team",
            Style::Directive => "team-directive",
        }
    }

    fn team(self) -> bool {
        self != Style::Agent
    }
}

/// The instruction text a trial of this style runs with.
fn instructions_for(style: Style, workspace: &str) -> String {
    let text = AGENT_INSTRUCTIONS.replace("{workspace}", workspace);
    match style {
        Style::Agent => text,
        Style::Permissive => format!("{text}{TEAM_EXTRA}"),
        Style::Directive => format!("{text}{TEAM_DIRECTIVE_EXTRA}"),
    }
}

/// The template [`instructions_for`] substitutes into — what the manifests pin and each trial records.
fn instructions_template(style: Style) -> String {
    match style {
        Style::Agent => AGENT_INSTRUCTIONS.to_string(),
        Style::Permissive => format!("{AGENT_INSTRUCTIONS}{TEAM_EXTRA}"),
        Style::Directive => format!("{AGENT_INSTRUCTIONS}{TEAM_DIRECTIVE_EXTRA}"),
    }
}

/// The request options every group runs with (the manifest's `model.reasoning_effort` pins the same value).
///
/// Round 8's treatment adds a **per-response output ceiling** from `TEAMAGENTS_EVAL_MAX_TOKENS`: the manifest
/// carries it, `run.py` exports it, every group gets it, and the trial records it with its own surface. It
/// removes what round 5 measured as the solo arm's mechanism — several units answered inside one response —
/// and it is applied to *both* arms, so it is a treatment, not a handicap. The audit that holds the
/// model-visible surface to one pin reads the `reasoning_effort` value it has always read.
fn request_options() -> Json {
    let mut options = json!({"reasoning_effort": "high"});
    if let Ok(ceiling) = std::env::var("TEAMAGENTS_EVAL_MAX_TOKENS") {
        if let Ok(tokens) = ceiling.parse::<u64>() {
            options["max_tokens"] = json!(tokens);
        }
    }
    options
}

/// Retries inside one turn, for every group.
const MAX_RETRIES: usize = 2;

/// The tool surface this harness offers a trial, in the order the model sees it.
fn tools_for(web: bool) -> (Vec<Json>, Vec<String>) {
    let tools = basic_tool_schemas(web, false);
    let names = tools.iter().filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string)).collect();
    (tools, names)
}

/// Everything this harness decides that the model sees, so a recorded trial's treatment is checkable from
/// its own record instead of from the source (D-182). The product's half of the surface — how a prompt is
/// assembled, which collaboration tools a grant adds — is pinned by the batch's recorded commit instead.
fn surface_json(style: Style, web: bool, timeout_s: u64, max_steps: usize) -> Json {
    let (_, names) = tools_for(web);
    json!({
        "instructions_kind": style.name(),
        "instructions_template_sha256": sha256_hex(&instructions_template(style)),
        "tools": names,
        "request_options": request_options(),
        "timeout_s": timeout_s,
        "max_steps": max_steps,
        "max_retries": MAX_RETRIES,
    })
}

/// Lower-case hex of the SHA-256 of `text`.
fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

struct Args {
    group: String,
    task_file: Option<PathBuf>,
    workdir: Option<PathBuf>,
    state: Option<PathBuf>,
    out: Option<PathBuf>,
    model: String,
    id: String,
    timeout_s: u64,
    max_steps: usize,
    web: bool,
    print_surface: bool,
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
    let mut print_surface = false;
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
            "--print-surface" => print_surface = true,
            _ => usage(),
        }
    }
    let Some(group) = group else { usage() };
    if !matches!(group.as_str(), GROUP_A | GROUP_B | GROUP_C | GROUP_D) {
        usage();
    }
    Args { group, task_file, workdir, state, out, model, id, timeout_s, max_steps, web, print_surface }
}

fn main() -> Fallible<()> {
    let args = parse_args();
    let style = style_of(&args.group);
    if args.print_surface {
        // What a trial with these flags would run under, without a model call: the offline half of the
        // surface audit (`review/eval_surface.py` recomputes the same digests from this file's text).
        let surface = surface_json(style, args.web, args.timeout_s, args.max_steps);
        println!("{}", serde_json::to_string_pretty(&surface)?);
        return Ok(());
    }
    let (Some(task_file), Some(workdir), Some(state), Some(out)) =
        (&args.task_file, &args.workdir, &args.state, &args.out)
    else {
        usage()
    };
    let task = std::fs::read_to_string(task_file)?;
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
    if state.join("session.sqlite").exists() {
        return Err(
            format!("state {} already holds a session; each trial needs a fresh state root", state.display()).into()
        );
    }
    std::fs::create_dir_all(workdir)?;
    std::fs::create_dir_all(state)?;
    let workspace = std::fs::canonicalize(workdir)?;
    let state = std::fs::canonicalize(state)?;
    let bindings: Vec<String> = if args.web {
        vec!["files".into(), "shell".into(), "web".into(), "skills".into()]
    } else {
        vec!["files".into(), "shell".into(), "skills".into()]
    };
    let (tools, _) = tools_for(args.web);
    let profile = KernelProfile {
        images: false,
        model: args.model.clone(),
        instructions: instructions_for(style, &workspace.to_string_lossy()),
        tools,
        // the manifest freezes effort=high for all three groups (and the trial record states it)
        options: request_options(),
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
        // C and D: the supervisor — same runtime and grants, different instruction shape (D-254)
        _ => runtime.block_on(run_driver_trial(
            &catalog,
            &raw_profile,
            &args,
            &task,
            &workspace,
            &state,
            &bindings,
            style.team(),
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
    // D-182: the harness's half of the model-visible surface, in the trial's own record
    report["surface"] = surface_json(style, args.web, args.timeout_s, args.max_steps);
    std::fs::write(out, serde_json::to_vec_pretty(&report)?)?;
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
        sandbox: teamagents_engine::tools::SandboxBackend::bubblewrap(),
        profile: profile.clone(),
        catalog: catalog.clone(),
        bindings: bindings.to_vec(),
        max_steps: args.max_steps,
        max_retries: MAX_RETRIES,
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
            sandbox: teamagents_engine::tools::SandboxBackend::bubblewrap(),
            catalog: catalog.clone(),
            bindings: bindings.to_vec(),
            max_retries: MAX_RETRIES,
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
        let outcome =
            wait_goal(&session_db, session_id, instance, args.timeout_s, &mut events_seen, Some(&handle)).await?;
        handle.shutdown().await?;
        outcome
    } else {
        let provider = build_for_model(catalog, &args.model)?;
        let config = DriverConfig {
            session_db: session_db.to_path_buf(),
            session_id: session_id.into(),
            instance_id: instance.into(),
            state_root: state.to_path_buf(),
            instances_dir: state.to_path_buf().join("instances"),
            workspace: workspace.to_path_buf(),
            permissions: "full_auto".into(),
            sandbox: teamagents_engine::tools::SandboxBackend::bubblewrap(),
            profile: profile.clone(),
            provider,
            catalog: catalog.clone(),
            bindings: bindings.to_vec(),
            max_retries: MAX_RETRIES,
            storage_queue: 256,
            poll: Duration::from_millis(100),
            goal_limits: json!({}),
            require_shell_approval: false,
        };
        let handle = start(config).await?;
        handle.input(task).await?;
        let outcome = wait_goal(&session_db, session_id, instance, args.timeout_s, &mut events_seen, None).await?;
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
///
/// **The user's half of the flow (D-256).** A spawned member holds no
/// `shell@workspace` (§5.1), so it cannot run the acceptance check its task
/// names and can only inspect files — measured in the round-4 pilot, where the
/// leader then re-ran every check itself (20 requests against group B's 8 for
/// the same two-file task). The design's answer is the *user's* grant (A03's
/// live probe issues exactly this by hand), so the team branch does it here,
/// within 100 ms of a member appearing, and records how many it granted. The
/// product's default is untouched; this is the treatment the round-5 arm adds.
async fn wait_goal(
    session_db: &Path,
    session_id: &str,
    instance: &str,
    timeout_s: u64,
    events_seen: &mut i64,
    grant_shell_to_members: Option<&SupervisorHandle>,
) -> Fallible<(String, Usage, usize)> {
    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    let mut granted: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    loop {
        let control = Control::open(session_db, session_id, false)?;
        let conn = control.connection();
        // the members that exist right now: the grant has to land while a
        // worker is still choosing how to work, not after it has finished
        let members: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT id FROM instances WHERE session_id = ?1 AND id != ?2 AND lifecycle != 'TERMINATED'")
                .map_err(|e| format!("members: {e}"))?;
            let rows = stmt.query_map(rusqlite::params![session_id, instance], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
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
        if let Some(handle) = grant_shell_to_members {
            for member in members {
                if !granted.insert(member.clone()) {
                    continue;
                }
                handle
                    .submit_user(Command {
                        command_id: format!("grant-shell-{member}"),
                        method: "issue_grant".into(),
                        params: json!({"subject": member, "action": "shell", "resource_scope": "workspace"}),
                    })
                    .await?;
            }
        }
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
        // the member grant is only useful while the member is still working, so
        // the team branch watches faster than the single-instance one
        let poll = if grant_shell_to_members.is_some() { 100 } else { 500 };
        tokio::time::sleep(Duration::from_millis(poll)).await;
    }
}
