//! R2-P5 real-provider acceptance harness (A27 异构供应商合作 + R23 长程/权限):
//! one v2 session whose leader and worker run on **different real providers**
//! (catalog keys), cooperating over the controlled message plane.
//!
//!   cargo run --offline --manifest-path engine/Cargo.toml --example rebuild_p5_accept -- \
//!     --evidence review/tmp/r2-p5-accept --workspace /tmp/ws \
//!     --lead leader_main --worker <openai-key-in-user-config> \
//!     [--permissions approved_scope|full_auto] [--timeout 600] [--dry-run]
//!
//! Evidence: `run.json` (configuration), `events.json` (full session trace),
//! `history-<instance>.json` (per-instance context) and `report.json` (verdict)
//! land in a fresh `--evidence` directory. `--dry-run` boots the session and
//! the controlled topology but sends no task, so it costs nothing.
//!
//! Real-provider runs always use the model's native context window (D-36): the
//! harness never overrides `context_window`.

use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use teamagents_core::kernel::KernelProfile;
use teamagents_core::v2::Command;
use teamagents_engine::providers::build_for_model;
use teamagents_engine::v2::supervisor::{start, SupervisorConfig, SupervisorHandle};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const CODEWORD: &str = "open-sesame";
const ACCEPT_FILE: &str = "p5-accept.txt";

fn cmd(id: impl Into<String>, method: &str, params: Json) -> Command {
    Command { command_id: id.into(), method: method.into(), params }
}

fn usage() -> ! {
    eprintln!(
        "usage: rebuild_p5_accept --evidence DIR --workspace DIR --lead KEY --worker KEY \\
[--permissions approved_scope|full_auto] [--timeout S] [--dry-run] [--require-file]"
    );
    std::process::exit(2);
}

#[tokio::main(worker_threads = 4)]
async fn main() -> Fallible<()> {
    let mut args = std::env::args().skip(1);
    let (mut evidence, mut workspace, mut lead, mut worker) = (None, None, None, None);
    let mut permissions = "approved_scope".to_string();
    let mut timeout_s = 600u64;
    let mut dry_run = false;
    let mut require_file = false;
    while let Some(flag) = args.next() {
        let value = |args: &mut std::iter::Skip<std::env::Args>| args.next().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--evidence" => evidence = Some(PathBuf::from(value(&mut args))),
            "--workspace" => workspace = Some(PathBuf::from(value(&mut args))),
            "--lead" => lead = Some(value(&mut args)),
            "--worker" => worker = Some(value(&mut args)),
            "--permissions" => permissions = value(&mut args),
            "--timeout" => timeout_s = value(&mut args).parse().unwrap_or_else(|_| usage()),
            "--dry-run" => dry_run = true,
            "--require-file" => require_file = true,
            _ => usage(),
        }
    }
    let (Some(evidence), Some(workspace), Some(lead), Some(worker)) = (evidence, workspace, lead, worker) else {
        usage()
    };
    if !matches!(permissions.as_str(), "approved_scope" | "full_auto") {
        usage();
    }
    if evidence.exists() {
        return Err(format!("{} exists; acceptance runs need a fresh evidence dir", evidence.display()).into());
    }
    if !workspace.is_dir() {
        return Err(format!("workspace {} does not exist", workspace.display()).into());
    }
    std::fs::create_dir_all(&evidence)?;
    let state_root = evidence.join("state");
    // shell jobs run in the production runner, which is the `teamagents`
    // binary next to this example (`jobs-runner` subcommand, §6.2); tests
    // override the runner image explicitly
    if std::env::var_os("TEAMAGENTS_RUNNER_BIN").is_none() {
        let runner = std::env::current_exe()?
            .parent()
            .and_then(|dir| dir.parent())
            .map(|dir| dir.join("teamagents"))
            .filter(|path| path.is_file())
            .ok_or("cannot locate the teamagents runner binary; set TEAMAGENTS_RUNNER_BIN")?;
        std::env::set_var("TEAMAGENTS_RUNNER_BIN", runner);
    }
    let catalog = teamagents_engine::config::load_user_config(&teamagents_engine::config::user_config_path())?;
    for key in [&lead, &worker] {
        if !catalog.models.contains_key(key) {
            return Err(format!("model key {key:?} is not in the user catalog").into());
        }
        // preflight: credentials and protocol resolve before any session starts (§7)
        build_for_model(&catalog, key).map_err(|e| format!("model {key}: {e}"))?;
    }
    let run = json!({
        "probe": "r2-p5-accept",
        "date": "2026-09-24",
        "lead_model_key": lead,
        "worker_model_key": worker,
        "lead_profile": catalog.models[&lead],
        "worker_profile": catalog.models[&worker],
        "permissions": permissions,
        "workspace": workspace.to_string_lossy(),
        "codeword": CODEWORD,
        "dry_run": dry_run,
        "native_context_window": true,
    });
    std::fs::write(evidence.join("run.json"), serde_json::to_vec_pretty(&run)?)?;
    println!("[accept] session: lead={lead} worker={worker} permissions={permissions}");

    let catalog_for_factory = catalog.clone();
    let factory = move |_id: &str, profile: &KernelProfile| {
        build_for_model(&catalog_for_factory, &profile.model)
            .unwrap_or_else(|e| panic!("provider for {}: {e}", profile.model))
    };
    let config = SupervisorConfig {
        marker: std::marker::PhantomData,
        session_db: state_root.join("session.sqlite"),
        session_id: "s-accept".into(),
        leader_id: "i-leader".into(),
        leader_profile: KernelProfile {
            model: lead.clone(),
            instructions: teamagents_engine::v2::daemon::LEADER_INSTRUCTIONS.into(),
            tools: teamagents_engine::reference::basic_tool_schemas(true, true),
            options: json!({}),
            context_window: None, // resolved from the catalog profile (native)
        },
        state_root: state_root.clone(),
        workspace: workspace.clone(),
        permissions: permissions.clone(),
        catalog: catalog.clone(),
        bindings: vec!["files".into(), "shell".into(), "web".into(), "skills".into()],
        max_retries: 2,
        storage_queue: 256,
        poll: Duration::from_millis(100),
        goal_limits: json!({}),
        require_shell_approval: permissions == "approved_scope",
        provider_factory: factory,
    };
    let handle = start(config).await?;
    let run = Run {
        evidence: &evidence,
        state_root: &state_root,
        workspace: &workspace,
        worker_key: &worker,
        timeout_s,
        dry_run,
        require_file,
    };
    let result = run_acceptance(&handle, run).await;
    let shutdown = handle.shutdown().await;
    result?;
    shutdown?;
    Ok(())
}

/// One acceptance run's knobs (the supervisor handle owns the session).
struct Run<'a> {
    evidence: &'a Path,
    state_root: &'a Path,
    workspace: &'a Path,
    worker_key: &'a str,
    timeout_s: u64,
    dry_run: bool,
    require_file: bool,
}

async fn run_acceptance(handle: &SupervisorHandle, run: Run<'_>) -> Fallible<()> {
    let Run { evidence, state_root, workspace, worker_key, timeout_s, dry_run, require_file } = run;
    // the worker joins the session with its own model key: one session, two
    // protocols (R17/A27)
    handle
        .submit_user(cmd(
            "accept-worker",
            "create_instance",
            json!({"id": "i-worker", "workspace_ref": workspace.to_string_lossy(),
                   "profile": {"model": worker_key}}),
        ))
        .await?;
    // the user grants exactly the two directions of the message plane (§5.1)
    for (subject, peer) in [("i-leader", "i-worker"), ("i-worker", "i-leader")] {
        handle
            .submit_user(cmd(
                format!("grant-{subject}"),
                "issue_grant",
                json!({"subject": subject, "action": "message", "resource_scope": format!("instance:{peer}")}),
            ))
            .await?;
    }
    wait_ready(handle, "i-worker", 20_000).await?;
    if dry_run {
        println!("[accept] dry run: session and topology are ready, no task sent");
        std::fs::write(evidence.join("report.json"), serde_json::to_vec_pretty(&json!({"dry_run": true}))?)?;
        return Ok(());
    }
    // the two sides only need the message plane: the worker replies with the
    // codeword, the leader relays it in its completion summary
    handle.input("i-leader", &leader_task()).await?;
    handle.input("i-worker", &worker_task(workspace, require_file)).await?;

    let started = Instant::now();
    let mut approvals: Vec<Json> = Vec::new();
    let mut approved_at: Option<Instant> = None;
    let mut file_seen_before_approval = false;
    let file = workspace.join(ACCEPT_FILE);
    let delivered = |events: &[Json], sender: &str, recipient: &str| {
        // message_sent carries the *recipient* in scope and the sender in the
        // payload (the delivery fact is scoped to the receiving instance)
        events.iter().any(|event| {
            event["kind"] == json!("message_sent")
                && event["scope"] == json!(recipient)
                && event["payload"]["sender"] == json!(sender)
        })
    };
    // the acceptance waits on *facts*, not on one instance's phase: both
    // directions delivered, the worker's approved shell effect in place, and
    // the leader's goal closed
    let goal_status = loop {
        // user-side approval of every pending shell request (§6.2): this is
        // the real approved_scope path, not a fake gateway. The pending list
        // comes from the daemon's own read surface (a separate read-only
        // connection); the decision goes through the single writer.
        for approval in pending_approvals(state_root)? {
            let id = approval["id"].as_str().unwrap_or("").to_string();
            eprintln!("[accept] approving {id}: {}", approval["preview"]);
            approved_at = Some(Instant::now());
            handle.submit_user(cmd(format!("approve-{id}"), "approve", json!({"approval_id": id}))).await?;
            approvals.push(approval);
        }
        if approved_at.is_none() && file.exists() {
            // nothing may run before the user decided (A14/§6.2)
            file_seen_before_approval = true;
        }
        let events = handle.events(0).await?;
        let snapshot = handle.snapshot().await?;
        let closed = snapshot["goal"]["status"]
            .as_str()
            .is_some_and(|status| matches!(status, "SUCCEEDED" | "FAILED" | "BLOCKED" | "CANCELLED"));
        let facts_hold = delivered(&events, "i-leader", "i-worker")
            && delivered(&events, "i-worker", "i-leader")
            && (!require_file || file.exists());
        if closed && facts_hold {
            break snapshot["goal"]["status"].as_str().unwrap_or("UNKNOWN").to_string();
        }
        if started.elapsed() > Duration::from_secs(timeout_s) {
            break "TIMEOUT".to_string();
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    let events = handle.events(0).await?;
    std::fs::write(evidence.join("events.json"), serde_json::to_vec_pretty(&events)?)?;
    for instance in ["i-leader", "i-worker"] {
        let history = handle
            .submit_user(cmd(
                format!("history-{instance}"),
                "read_history",
                json!({"instance_id": instance, "limit": 400}),
            ))
            .await?;
        std::fs::write(evidence.join(format!("history-{instance}.json")), serde_json::to_vec_pretty(&history)?)?;
    }
    let file_text = std::fs::read_to_string(&file).unwrap_or_default();
    let report = json!({
        "goal_status": goal_status,
        "elapsed_s": started.elapsed().as_secs(),
        "codeword_delivered": events.iter().any(|event| event.to_string().contains(CODEWORD)),
        "leader_to_worker": delivered(&events, "i-leader", "i-worker"),
        "worker_to_leader": delivered(&events, "i-worker", "i-leader"),
        "approvals": approvals.len(),
        "file_written": file.exists(),
        "require_file": require_file,
        "file_text": file_text,
        "file_seen_before_approval": file_seen_before_approval,
        "instance_count": handle.snapshot().await?["instances"].as_array().map(Vec::len).unwrap_or(0),
        "event_count": events.len(),
    });
    std::fs::write(evidence.join("report.json"), serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report["goal_status"] != json!("SUCCEEDED") {
        return Err(format!("leader goal ended as {goal_status}").into());
    }
    if report["leader_to_worker"] != json!(true) || report["worker_to_leader"] != json!(true) {
        return Err("the two providers did not exchange a message in both directions".into());
    }
    if report["file_seen_before_approval"] == json!(true) {
        return Err("a shell effect ran before the user approved it (A14)".into());
    }
    if require_file && (!file.exists() || !file_text.contains(CODEWORD)) {
        return Err("the worker's approved shell effect is missing (R23 permission path)".into());
    }
    Ok(())
}

fn leader_task() -> String {
    String::from(
        "Team exercise: ask instance i-worker for the codeword by sending it a message \
(use the send tool with recipient i-worker), then finish with status success and say that you asked.",
    )
}

fn worker_task(workspace: &Path, require_file: bool) -> String {
    if !require_file {
        return format!(
            "You were sent a message by i-leader. Reply to i-leader (send tool, recipient i-leader) with the \
codeword {CODEWORD}, then finish with status success."
        );
    }
    format!(
        "Do exactly this, in order. (1) Run this shell command: printf '{CODEWORD}' > {}. \
The command needs your shell tool; if it is waiting for approval, just call it and wait. \
(2) After it ran, send instance i-leader a message containing only the codeword {CODEWORD} \
(send tool, recipient i-leader). (3) Finish with status success and mention the file you wrote.",
        workspace.join(ACCEPT_FILE).display()
    )
}

async fn wait_ready(handle: &SupervisorHandle, instance: &str, timeout_ms: u64) -> Fallible<()> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let snapshot = handle.snapshot().await?;
        let ready = snapshot["instances"].as_array().into_iter().flatten().any(|entry| {
            entry["id"] == json!(instance) && entry["phase"] == json!("READY") && entry["lifecycle"] == json!("ACTIVE")
        });
        if ready {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("instance {instance} was not READY within {timeout_ms}ms").into())
}

/// Pending approvals with their operation's fixed intent — the same read the
/// daemon exposes (§9). It opens a plain read-only connection: a full
/// `Control::open` runs the schema/pragma setup on every poll and contends
/// with the single writer while the acceptance waits on real model calls.
fn pending_approvals(state_root: &Path) -> Fallible<Vec<Json>> {
    let conn = rusqlite::Connection::open_with_flags(
        state_root.join("session.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut stmt = conn.prepare(
        "SELECT a.id, a.operation_id, o.intent_json FROM approvals a
         JOIN operations o ON a.operation_id = o.operation_id
         WHERE a.status = 'PENDING' ORDER BY a.rowid",
    )?;
    let rows =
        stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?;
    let mut approvals = Vec::new();
    for row in rows {
        let (id, operation_id, intent_json) = row?;
        let intent: Json = serde_json::from_str(&intent_json).unwrap_or(Json::Null);
        let args = intent["args"].clone();
        let preview = args["command"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| args.to_string().chars().take(120).collect());
        approvals.push(json!({"id": id, "operation_id": operation_id,
                              "tool": intent["name"].as_str().unwrap_or(""), "preview": preview}));
    }
    Ok(approvals)
}
