//! CLI entry points: doctor / validate / sessions / version / --plain REPL.

use crate::config::{load_user_config, load_user_config_for, missing_key_envs, sessions_dir, user_config_path};
use crate::core_client::CoreClient;
use crate::session::{open_session, OpenOptions};
use crate::sessions::session_paths;
use crate::tools::{bwrap_available, shell_run, which};
use crate::VERSION;
use serde_json::{json, Value as Json};
use std::io::{BufRead, Read, Write};
use std::time::{Duration, Instant};
use std::path::{Path, PathBuf};

fn check(results: &mut Vec<(String, bool, String)>, name: &str, ok: bool, detail: String) {
    results.push((name.to_string(), ok, detail));
}

pub fn doctor() -> i32 {
    let mut results: Vec<(String, bool, String)> = vec![];
    match CoreClient::open(":memory:", "doctor").and_then(|core| core.call("ping", json!({}))) {
        Ok(info) => {
            let version = info.get("core").and_then(|v| v.as_str()).unwrap_or("?");
            check(&mut results, "rust core", true, format!("teamagents-core {version}"));
        }
        Err(e) => check(&mut results, "rust core", false, e),
    }
    match load_user_config(&user_config_path()) {
        Ok(catalog) => {
            let models: Vec<&String> = catalog.models.keys().collect();
            let tools: Vec<&String> = catalog.tools.keys().collect();
            check(&mut results, "user config", true, format!("models={models:?} tools={tools:?}"));
            for (name, present) in missing_key_envs(&catalog) {
                let profile = &catalog.models[&name];
                let env = profile.api_key_env.clone().unwrap_or_default();
                check(
                    &mut results,
                    &format!("model profile {name}"),
                    present,
                    format!(
                        "{}/{} {}",
                        profile.provider,
                        profile.model,
                        if present { String::new() } else { format!("(missing env {env})") }
                    ),
                );
            }
        }
        Err(e) => check(&mut results, "user config", false, e),
    }
    let bwrap = bwrap_available();
    // not just "is it installed": run a probe so a broken
    // userns/kernel setup is caught here instead of at the first shell call
    let bwrap_probe = bwrap
        && shell_run("test -e /etc/hostname && test ! -e /home", &std::env::temp_dir(), 20, false, None)
            .map(|out| !out.contains("(exit "))
            .unwrap_or(false);
    check(
        &mut results,
        "bubblewrap isolation",
        bwrap_probe,
        if bwrap_probe {
            "system files visible, home blocked".into()
        } else if bwrap {
            "bwrap present but the isolation probe failed".into()
        } else {
            "bwrap not found: out-of-scope commands must ask for approval".into()
        },
    );
    let codex = which("codex");
    match &codex {
        Some(codex) => {
            let codex = codex.to_string_lossy().into_owned();
            let help = std::process::Command::new(&codex).args(["app-server", "--help"]).output();
            let app_server = help.map(|out| String::from_utf8_lossy(&out.stdout).contains("app-server")).unwrap_or(false);
            // `codex --version` already prefixes itself ("codex-cli x.y.z")
            check(&mut results, "codex app-server", app_server, codex_version(&codex));
            let (schema_ok, detail) = codex_schema_check(&codex);
            check(&mut results, "codex protocol schema", schema_ok, detail);
        }
        None => check(&mut results, "codex app-server", false, "codex CLI not found".into()),
    }
    let dir = sessions_dir();
    let probe = dir.join(".doctor-probe");
    let state_ok = std::fs::create_dir_all(&dir).is_ok()
        && std::fs::write(&probe, "ok").is_ok()
        && std::fs::remove_file(&probe).is_ok();
    check(&mut results, "state directory", state_ok, dir.to_string_lossy().into_owned());

    println!("TeamAgents doctor ({VERSION})");
    let mut failed = 0;
    for (name, ok, detail) in &results {
        if !ok {
            failed += 1;
        }
        println!("  [{}] {:24} {}", if *ok { "ok  " } else { "FAIL" }, name, detail);
    }
    if failed > 0 {
        1
    } else {
        0
    }
}

fn codex_version(codex: &str) -> String {
    std::process::Command::new(codex)
        .arg("--version")
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// Methods declared by one generated schema file (ClientRequest.json shape).
fn schema_methods(path: &Path) -> Result<std::collections::HashSet<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: Json = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let methods = doc
        .get("oneOf")
        .and_then(|v| v.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    entry
                        .get("properties")
                        .and_then(|p| p.get("method"))
                        .and_then(|m| m.get("enum"))
                        .and_then(|e| e.as_array())
                        .and_then(|e| e.first())
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(methods)
}

/// D-3: generate the schema from the installed CLI and confirm the required
/// method sets exist.
fn codex_schema_check(codex: &str) -> (bool, String) {
    const NEEDED: &[&str] = &["initialize", "thread/start", "thread/resume", "turn/start", "turn/interrupt"];
    const NEEDED_REQUESTS: &[&str] =
        &["item/commandExecution/requestApproval", "item/fileChange/requestApproval", "item/tool/requestUserInput"];
    let dir = std::env::temp_dir().join(format!("ta-codex-schema-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let run = std::process::Command::new(codex)
        .args(["app-server", "generate-json-schema", "--out"])
        .arg(&dir)
        .output();
    let result = match run {
        Ok(out) if out.status.success() => (|| -> Result<String, String> {
            let client = schema_methods(&dir.join("ClientRequest.json"))?;
            let server = schema_methods(&dir.join("ServerRequest.json"))?;
            let missing: Vec<&str> = NEEDED
                .iter()
                .filter(|method| !client.contains(**method))
                .chain(NEEDED_REQUESTS.iter().filter(|method| !server.contains(**method)))
                .copied()
                .collect();
            if missing.is_empty() {
                Ok(format!("schema generated from installed CLI ({} methods)", client.len()))
            } else {
                Err(format!("schema from this CLI lacks: {missing:?}"))
            }
        })(),
        Ok(out) => Err(format!("schema generation failed: {}", String::from_utf8_lossy(&out.stderr).trim())),
        Err(e) => Err(format!("schema generation failed: {e}")),
    };
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        Ok(detail) => (true, detail),
        Err(e) => (false, e),
    }
}

pub fn validate_spec(path: &str) -> i32 {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            println!("invalid: cannot read {path}: {e}");
            return 1;
        }
    };
    let spec: Json = match crate::config::parse_spec(&text) {
        Ok(spec) => spec,
        Err(e) => {
            println!("invalid: {e}");
            return 1;
        }
    };
    let project_root = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
    let catalog = match load_user_config_for(project_root) {
        Ok(catalog) => catalog,
        Err(e) => {
            println!("invalid: {e}");
            return 1;
        }
    };
    let core = match CoreClient::open(":memory:", "validate") {
        Ok(core) => core,
        Err(e) => {
            println!("invalid: {e}");
            return 1;
        }
    };
    let models: Vec<String> = catalog.models.keys().cloned().collect();
    let tools: Vec<String> = catalog.tools.keys().cloned().collect();
    match core.call("validate_spec", json!({"spec": spec, "models": models, "tools": tools})) {
        Ok(result) => {
            println!(
                "ok: {path} — leader={} members={} channels={} spaces={}",
                result.get("leader").and_then(|v| v.as_str()).unwrap_or(""),
                result.get("members").and_then(|v| v.as_u64()).unwrap_or(0),
                result.get("channels").and_then(|v| v.as_u64()).unwrap_or(0),
                result.get("spaces").and_then(|v| v.as_u64()).unwrap_or(0),
            );
            0
        }
        Err(e) => {
            println!("invalid: {e}");
            1
        }
    }
}

/// `teamagents sessions prune --days N [--dry-run]`: archive retention. The
/// sweep never touches a running session or one with unmerged worktree work.
pub fn prune_sessions_cmd(days: u64, dry_run: bool) -> i32 {
    let report = crate::sessions::prune_archived(days, None, dry_run);
    let session_id = |entry: &Json| entry.get("session_id").and_then(|v| v.as_str()).unwrap_or("?").to_string();
    let removed = report.get("removed").and_then(Json::as_array).cloned().unwrap_or_default();
    let skipped = report.get("skipped").and_then(Json::as_array).cloned().unwrap_or_default();
    println!(
        "归档会话保留策略：{} 天（{}）",
        days,
        if dry_run { "试运行，不删除" } else { "删除超期归档会话" }
    );
    for entry in &removed {
        println!(
            "  {} {}（最后一次更新 {} 天前，{:.1} MB）",
            if dry_run { "将删除" } else { "已删除" },
            session_id(entry),
            entry.get("age_days").and_then(Json::as_u64).unwrap_or(0),
            entry.get("size_mb").and_then(Json::as_f64).unwrap_or(0.0)
        );
    }
    for entry in &skipped {
        println!("  跳过 {}：{}", session_id(entry), entry.get("error").and_then(Json::as_str).unwrap_or(""));
    }
    println!(
        "保留 {} 个未超期会话，{} {} 个，释放 {:.1} MB",
        report.get("kept").and_then(Json::as_u64).unwrap_or(0),
        if dry_run { "预计删除" } else { "删除" },
        removed.len(),
        report.get("bytes_freed").and_then(Json::as_u64).unwrap_or(0) as f64 / (1024.0 * 1024.0)
    );
    if skipped.is_empty() { 0 } else { 1 }
}

pub fn list_sessions_cmd(verbose: bool) -> i32 {
    let rows = crate::sessions::list_sessions(None, true, None);
    if rows.is_empty() {
        println!("没有会话记录（{}）", sessions_dir().display());
        return 0;
    }
    println!("会话记录目录：{}", sessions_dir().display());
    for row in rows {
        let updated = if row.updated_at > 0.0 {
            format_timestamp(row.updated_at)
        } else {
            "?".into()
        };
        let mut flags: Vec<String> = vec![];
        if row.archived {
            flags.push("已归档".into());
        }
        if row.locked {
            flags.push("运行中".into());
        }
        if let Some(error) = &row.error {
            flags.push(format!("读取异常:{}", error.chars().take(40).collect::<String>()));
        }
        println!(
            "  {:24} {:7} 目标 {:7} 事件 {:5} 任务 {:3} {:>6}MB  {}  {}  {}",
            row.session_id,
            row.status,
            row.goal_state,
            row.events,
            row.tasks,
            format!("{:.1}", row.size_mb),
            updated,
            row.cwd,
            flags.join(" ")
        );
        if verbose {
            println!("      {}", row.path);
        }
    }
    println!("\n在 TUI 的“会话”面板可切换/新建/归档/删除；命令行恢复：teamagents --resume <会话 id>");
    0
}

fn format_timestamp(seconds: f64) -> String {
    // UTC "MM-DD HH:MM" without a date library
    let total = seconds as i64;
    let days = total / 86_400;
    let (hour, minute) = ((total % 86_400) / 3600, (total % 3600) / 60);
    // civil-from-days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let _era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{m:02}-{d:02} {hour:02}:{minute:02}")
}

pub fn version() -> i32 {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": VERSION,
            "core": "teamagents-core",
            "config": user_config_path().to_string_lossy(),
        }))
        .unwrap_or_default()
    );
    0
}

fn print_event(event: &Json) {
    let kind = event.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let actor = event.get("actor_id").and_then(|v| v.as_str()).unwrap_or("");
    let payload = event.get("payload").cloned().unwrap_or(json!({}));
    let head = |limit: usize| -> String { payload.to_string().chars().take(limit).collect() };
    match kind {
        "user_message" => {}
        "task_completed" | "task_failed" | "task_blocked" | "task_created" | "goal_done" | "limit_reached" => {
            println!("  [{kind}] {}", head(200))
        }
        "message" => println!(
            "  [message] {actor} -> {}: {}",
            payload.get("target").and_then(|v| v.as_str()).unwrap_or(""),
            payload.get("text").and_then(|v| v.as_str()).unwrap_or("").chars().take(160).collect::<String>()
        ),
        "approval_requested" => println!("  [approval] {}", head(200)),
        "leader_reply" => println!(
            "  [Leader] {}",
            payload.get("text").and_then(|v| v.as_str()).unwrap_or("").chars().take(2000).collect::<String>()
        ),
        "run_failed" => println!(
            "  [运行失败] {actor}: {}",
            payload.get("error").and_then(|v| v.as_str()).unwrap_or("未知错误").chars().take(400).collect::<String>()
        ),
        _ => {}
    }
}

/// `--plain` REPL `status` command: one line per member (Codex /status parity).
pub fn print_usage_table(report: &Json) {
    println!("成员 | 模型 | 上下文窗口 | 累计 tokens (prompt/completion) | 剩余上下文");
    for agent in report.get("agents").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let name = agent.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let model = agent.get("model").and_then(|v| v.as_str()).unwrap_or("未配置");
        let window = agent.get("context_window").and_then(|v| v.as_u64());
        let usage = agent.get("usage").cloned().unwrap_or(json!({}));
        let prompt = usage.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let completion = usage.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let total = usage.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let last = usage.get("last_prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let remaining = match window {
            Some(w) => w.saturating_sub(last).to_string(),
            None => "未配置".into(),
        };
        println!(
            "{name} | {model} | {} | {total} ({prompt}/{completion}) | {remaining}",
            window.map(|w| w.to_string()).unwrap_or_else(|| "未配置".into()),
        );
    }
}

pub fn repl(cwd: Option<String>, resume: Option<String>, full_auto: bool, team: Option<String>) -> i32 {
    let initial_spec = match &team {
        Some(path) => match crate::config::load_spec_file(std::path::Path::new(path)) {
            Ok(spec) => Some(spec),
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        },
        None => None,
    };
    let opened = match open_session(OpenOptions {
        cwd: cwd.map(PathBuf::from),
        session_id: resume,
        full_auto,
        initial_spec,
        catalog: None,
        scripts: None,
    }) {
        Ok(opened) => opened,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    println!("session: {} (Ctrl-D to exit)", opened.session_id);
    opened.runtime.start();
    let stdin = std::io::stdin();
    let mut cursor = 0i64;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if line.trim() == "status" {
            print_usage_table(&opened.usage_report());
            continue;
        }
        // D-26 rewind/fork (pi-style tree history; leader conversation only)
        let cmd = line.trim();
        if cmd == "rewind" || cmd.starts_with("rewind ") {
            match cmd.strip_prefix("rewind").map(|x| x.trim()) {
                Some("") => match opened.rewind_points() {
                    Ok(report) => {
                        let points = report.get("points").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                        if points.is_empty() {
                            println!("  暂无可回退的节点");
                        }
                        for point in points {
                            println!("  {}  {}", point.get("id").and_then(|v| v.as_str()).unwrap_or("?"),
                                point.get("preview").and_then(|v| v.as_str()).unwrap_or(""));
                        }
                    }
                    Err(e) => println!("  获取回退点失败：{e}"),
                },
                Some("0") | None => match opened.rewind(None) {
                    Ok(v) => println!("  已回退（深度 {}）", v.get("depth").and_then(|v| v.as_u64()).unwrap_or(0)),
                    Err(e) => println!("  回退失败：{e}"),
                },
                Some(id) => match opened.rewind(Some(id.to_string())) {
                    Ok(v) => println!("  已回退（深度 {}）", v.get("depth").and_then(|v| v.as_u64()).unwrap_or(0)),
                    Err(e) => println!("  回退失败：{e}"),
                },
            }
            continue;
        }
        if cmd == "fork" {
            println!("  --plain 模式请用 TUI 的 /fork（需要 worker 会话管理）");
            continue;
        }
        // feature 5 (/model): model <member> <model> [effort]；model <member> 清除
        let trimmed = line.trim();
        if trimmed == "model" || trimmed.starts_with("model ") {
            let tokens: Vec<&str> = trimmed.split_whitespace().collect();
            let result = match tokens.as_slice() {
                ["model", member] => opened.set_model_override(member, None, None),
                ["model", member, model] => opened.set_model_override(member, Some(model.to_string()), None),
                ["model", member, model, effort] => {
                    opened.set_model_override(member, Some(model.to_string()), Some(effort.to_string()))
                }
                _ => {
                    println!("  用法：model <成员> <模型> [档位]；model <成员> 恢复 profile 默认");
                    continue;
                }
            };
            match result {
                Ok(v) => println!(
                    "  {}：模型 {} · 档位 {}{}",
                    v.get("agent_id").and_then(|x| x.as_str()).unwrap_or("?"),
                    v.get("model").and_then(|x| x.as_str()).unwrap_or("未配置"),
                    v.get("effort").and_then(|x| x.as_str()).unwrap_or("未配置"),
                    if v.get("overridden").and_then(|x| x.as_bool()).unwrap_or(false) {
                        "（会话覆盖，下一回合生效）"
                    } else {
                        "（profile 默认）"
                    },
                ),
                Err(e) => println!("  [模型切换失败: {e}]"),
            }
            continue;
        }
        match opened.runtime.user_message(&line, false) {
            Ok(receipt) => println!(
                "  [input received: {}]",
                receipt.result.get("goal_id").and_then(|v| v.as_str()).unwrap_or("")
            ),
            Err(e) => println!("  [rejected: {e}]"),
        }
        opened.runtime.settle(600);
        if let Ok(state) = opened.core.call_in_session("state", json!({"after_sequence": cursor})) {
            for event in state.get("events").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                cursor = event.get("sequence").and_then(|v| v.as_i64()).unwrap_or(cursor);
                print_event(&event);
            }
        }
    }
    let _ = std::io::stdout().flush();
    opened.close();
    0
}

/// Structured, non-interactive execution. Every stdout line is JSON and all
/// diagnostics stay on stderr so callers can safely stream and parse it.
pub struct ExecOptions {
    pub cwd: Option<String>, pub resume: Option<String>, pub full_auto: bool,
    pub team: Option<String>, pub timeout: Option<u64>, pub checks: Vec<String>, pub prompt: Option<String>,
}

pub fn exec_json(args: &ExecOptions) -> i32 {
    let started = Instant::now();
    let prompt = match args.prompt.as_deref() {
        Some("-") => { let mut s = String::new(); if std::io::stdin().read_to_string(&mut s).is_err() { eprintln!("读取 stdin 失败"); return 2; } s },
        Some(s) => s.to_string(),
        None => { eprintln!("exec 需要 PROMPT（使用 - 从 stdin 读取）"); return 2; }
    };
    let initial_spec = match &args.team {
        Some(path) => match crate::config::load_spec_file(Path::new(path)) { Ok(s) => Some(s), Err(e) => { eprintln!("{e}"); return 2; } },
        None => None,
    };
    let opened = match open_session(OpenOptions { cwd: args.cwd.clone().map(PathBuf::from), session_id: args.resume.clone(), full_auto: args.full_auto, initial_spec, catalog: None, scripts: None }) {
        Ok(v) => v,
        Err(e) => { eprintln!("打开会话失败：{e}"); return 1; }
    };
    let sid = opened.session_id.clone();
    if !json_line(&json!({"schema_version":1,"type":"session","session_id":sid})) { opened.close(); return 1; }
    // Tool activity as it happens: which file, which command, ok or failed.
    let sink_session = sid.clone();
    opened.runtime.notify.set_tool_sink(Box::new(move |run_id, agent_id, activity| {
        let _ = json_line(&json!({
            "schema_version":1,"type":"tool","session_id":sink_session,"run_id":run_id,"agent_id":agent_id,
            "tool":activity["tool"],"call_id":activity["call_id"],"ok":activity["ok"],
            "error":activity["error"],"arguments":activity["arguments"],
        }));
    }));
    opened.runtime.start();
    if let Err(e) = opened.runtime.user_message(&prompt, false) { eprintln!("提交消息失败：{e}"); opened.close(); return 1; }
    let timeout = args.timeout.unwrap_or(1200);
    let deadline = Instant::now().checked_add(Duration::from_secs(timeout)).unwrap_or_else(|| Instant::now() + Duration::from_secs(1200));
    let mut cursor = 0i64;
    let mut timed_out = false;
    loop {
        let state = match opened.core.call_in_session("state", json!({"after_sequence": cursor})) { Ok(v) => v, Err(e) => { eprintln!("读取状态失败：{e}"); break; } };
        for event in state.get("events").and_then(Json::as_array).cloned().unwrap_or_default() {
            cursor = event.get("sequence").and_then(Json::as_i64).unwrap_or(cursor);
            if !json_line(&json!({"schema_version":1,"type":"event","session_id":sid,"event":event})) { opened.close(); return 1; }
        }
        let runs = state.get("runs").and_then(Json::as_array).cloned().unwrap_or_default();
        let active: Vec<_> = runs.iter().filter(|r| matches!(r.get("status").and_then(Json::as_str), Some("QUEUED"|"RUNNING"|"WAITING_TASK"|"WAITING_APPROVAL"))).collect();
        if active.is_empty() { break; }
        // A parked approval can only be answered from outside, so waiting for it
        // would just burn the timeout and report "timeout" instead of exit 3.
        if has_pending_approvals(&state) { break; }
        if Instant::now() >= deadline { timed_out = true; for run in active { if let Some(id) = run.get("run_id").and_then(Json::as_str) { let _ = opened.runtime.submit(teamagents_core::models::TeamAction { action_id: teamagents_core::models::new_id("cancel"), session_id: sid.clone(), actor_id: "user".into(), run_id: None, kind: teamagents_core::models::ActionKind::CancelRun, payload: json!({"run_id":id}) }); } } break; }
        std::thread::sleep(Duration::from_millis(50));
    }
    let state = opened.core.call_in_session("state", json!({"after_sequence": cursor})).unwrap_or_else(|_| json!({}));
    for event in state.get("events").and_then(Json::as_array).cloned().unwrap_or_default() {
        cursor = event.get("sequence").and_then(Json::as_i64).unwrap_or(cursor);
        if !json_line(&json!({"schema_version":1,"type":"event","session_id":sid,"event":event})) { opened.close(); return 1; }
    }
    let mut verification = Vec::new();
    for command in &args.checks {
        let marker = format!("__TEAMAGENTS_CHECK_RC_{}__", uuid::Uuid::new_v4());
        let wrapped = format!("{{ {command}; rc=$?; printf '\\n{marker}%s\\n' \"$rc\"; exit $rc; }}");
        let result = crate::tools::shell_run(&wrapped, &opened.cwd, timeout.min(120), false, Some(&session_paths(&sid).artifacts));
        let (output, code) = match result { Ok(text) => parse_check_result(&text, &marker), Err(e) => (e, 1) };
        let ok = code == 0;
        verification.push(json!({"command":command,"output":output,"exit_code":code,"ok":ok}));
        if !ok { break; }
    }
    if !verification.is_empty() { let path = session_paths(&sid).base.join("verification.json"); let _ = std::fs::create_dir_all(session_paths(&sid).base); let _ = std::fs::write(path, serde_json::to_vec_pretty(&verification).unwrap_or_default()); }
    let checks_ok = verification.iter().all(|v| v.get("ok").and_then(Json::as_bool).unwrap_or(false));
    let (status, code) = exec_outcome(&state, timed_out, checks_ok);
    // Same accounting the TUI shows in /status: evals and CI can record real
    // token totals instead of guessing them.
    let usage = opened.usage_report();
    let _ = json_line(&json!({
        "schema_version":1,"type":"result","session_id":sid,"status":status,"exit_code":code,
        "duration_ms": started.elapsed().as_millis() as u64,
        "usage": usage.get("agents").cloned().unwrap_or_else(|| json!([])),
        "verification":verification,
    }));
    opened.close();
    code
}

fn has_pending_approvals(state: &Json) -> bool {
    state.get("pending_approvals").and_then(Json::as_array).is_some_and(|queue| !queue.is_empty())
}

/// Documented `exec --json` contract: 0 completed, 1 failed/incomplete,
/// 3 approval required, 124 timeout.
fn exec_outcome(state: &Json, timed_out: bool, checks_ok: bool) -> (&'static str, i32) {
    let run_in = |statuses: &[&str]| {
        state.get("runs").and_then(Json::as_array).is_some_and(|runs| {
            runs.iter().any(|run| run.get("status").and_then(Json::as_str).is_some_and(|s| statuses.contains(&s)))
        })
    };
    let goal_done = state.get("session").and_then(|s| s.get("goal_state")).and_then(Json::as_str) == Some("done");
    if timed_out { ("timeout", 124) }
    else if has_pending_approvals(state) { ("approval_required", 3) }
    else if run_in(&["FAILED", "OUTCOME_UNKNOWN"]) { ("failed", 1) }
    else if goal_done && checks_ok { ("completed", 0) }
    else { ("incomplete", 1) }
}

#[cfg(test)]
fn parse_exit_code(text: &str) -> Option<i32> {
    let line = text.lines().rev().find(|l| l.trim_start().starts_with("(exit "))?;
    line.trim().strip_prefix("(exit ")?.strip_suffix(')')?.parse().ok()
}

fn parse_check_result(text: &str, marker: &str) -> (String, i32) {
    let Some((before, after)) = text.rsplit_once(marker) else { return (text.to_string(), 1) };
    let code = after.lines().next().and_then(|line| line.trim().parse().ok()).unwrap_or(1);
    (before.trim_end().to_string(), code)
}

fn json_line(value: &Json) -> bool {
    let mut out = std::io::stdout().lock();
    out.write_all(serde_json::to_string(value).unwrap_or_else(|_| "{}".into()).as_bytes()).and_then(|_| out.write_all(b"\n")).is_ok()
}

#[cfg(test)]
mod exec_tests {
    use super::{exec_outcome, parse_check_result, parse_exit_code};
    use serde_json::json;

    #[test]
    fn exit_code_parser_uses_only_engine_marker() {
        assert_eq!(parse_exit_code("hello (exit 99)\n(exit 0)"), Some(0));
        assert_eq!(parse_exit_code("failed\n(exit 7)"), Some(7));
        assert_eq!(parse_exit_code("ok\n"), None);
        assert_eq!(parse_check_result("hello\n__marker__0\n", "__marker__"), ("hello".into(), 0));
    }

    #[test]
    fn parked_approval_reports_approval_required_not_timeout() {
        let parked = json!({"runs": [{"status": "WAITING_APPROVAL"}], "pending_approvals": [{"approval_id": "a1"}]});
        assert_eq!(exec_outcome(&parked, false, true), ("approval_required", 3));
        // the timeout branch still wins when the deadline really passed
        assert_eq!(exec_outcome(&parked, true, true), ("timeout", 124));
        let unknown = json!({"runs": [{"status": "OUTCOME_UNKNOWN"}], "pending_approvals": []});
        assert_eq!(exec_outcome(&unknown, false, true), ("failed", 1));
        let done = json!({"runs": [{"status": "SUCCEEDED"}], "pending_approvals": [], "session": {"goal_state": "done"}});
        assert_eq!(exec_outcome(&done, false, true), ("completed", 0));
        assert_eq!(exec_outcome(&done, false, false), ("incomplete", 1));
    }
}
