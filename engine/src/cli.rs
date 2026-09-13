//! CLI entry points (cli.py): doctor / validate / sessions / version / --plain REPL.

use crate::config::{load_user_config, missing_key_envs, sessions_dir, user_config_path};
use crate::core_client::CoreClient;
use crate::session::{open_session, OpenOptions};
use crate::tools::{bwrap_available, shell_run, which};
use crate::VERSION;
use serde_json::{json, Value as Json};
use std::io::{BufRead, Write};
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
    // not just "is it installed": run a probe (cli.py does the same) so a broken
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
/// method sets exist (cli.py::_codex_schema_check parity).
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
    let catalog = match load_user_config(&user_config_path()) {
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
