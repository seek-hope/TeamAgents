//! A tiny `codex app-server` stand-in for tests.
//! Modes (env FAKE_CODEX_MODE): simple | approval | slow | die | die-once

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::io::{BufRead, Write};

fn send(message: Json) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let _ = writeln!(handle, "{message}");
    let _ = handle.flush();
}

fn main() {
    let mode = std::env::var("FAKE_CODEX_MODE").unwrap_or_else(|_| "simple".into());
    // die-once: the first spawned process crashes like "die"; a marker file
    // (env FAKE_CODEX_DIE_MARKER) makes later respawns behave like "simple",
    // so reconnect tests get a working server on the second process
    let mode = if mode == "die-once" {
        let marker = std::env::var("FAKE_CODEX_DIE_MARKER").unwrap_or_default();
        if marker.is_empty() || std::path::Path::new(&marker).exists() {
            "simple".to_string()
        } else {
            let _ = std::fs::write(&marker, b"1");
            "die".to_string()
        }
    } else {
        mode
    };
    // turn id -> (thread id, status)
    let mut turns: HashMap<String, (String, String)> = HashMap::new();
    let mut thread_count = 0usize;
    let mut turn_count = 0usize;

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Json>(&line) else { continue };
        let method = message.get("method").and_then(|v| v.as_str()).map(str::to_string);
        let id = message.get("id").cloned().unwrap_or(Json::Null);
        let params = message.get("params").cloned().unwrap_or(json!({}));

        // a response to our server->client approval request
        if method.is_none() && (id.as_u64() == Some(9001) || id.as_u64() == Some(9002)) {
            let decision = message
                .get("result")
                .and_then(|r| r.get("decision"))
                .and_then(|v| v.as_str())
                .unwrap_or("decline")
                .to_string();
            if let Ok(log) = std::env::var("FAKE_APPROVAL_LOG") {
                if !log.is_empty() {
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
                        let _ = writeln!(f, "{decision}");
                    }
                }
            }
            if let Some(turn_id) =
                turns.iter().find(|(_, (_, status))| status == "awaitingApproval").map(|(k, _)| k.clone())
            {
                let (thread_id, _) = turns[&turn_id].clone();
                // approval-grant: the first accept is answered by asking for the
                // identical operation again under fresh per-call ids; the turn
                // completes only when that second request is answered
                if mode == "approval-grant" && id.as_u64() == Some(9001) {
                    send(json!({
                        "id": 9002,
                        "method": "item/commandExecution/requestApproval",
                        "params": {"threadId": thread_id, "turnId": turn_id, "itemId": "exec-2",
                                   "startedAtMs": 2, "command": "echo probe", "reason": "fake approval"},
                    }));
                    continue;
                }
                let item = json!({"type": "agentMessage", "text": format!("approval={decision}")});
                send(
                    json!({"method": "item/completed", "params": {"threadId": thread_id, "turnId": turn_id, "item": item}}),
                );
                turns.insert(turn_id.clone(), (thread_id.clone(), "completed".into()));
                send(
                    json!({"method": "turn/completed", "params": {"threadId": thread_id, "turn": {"id": turn_id, "status": "completed"}}}),
                );
            }
            continue;
        }
        let Some(method) = method else { continue };
        match method.as_str() {
            "initialize" => send(json!({"id": id, "result": {"userAgent": "fake-codex/0.0.1"}})),
            "thread/start" => {
                thread_count += 1;
                let thread_id = format!("thr-{thread_count}");
                send(json!({"id": id, "result": {"thread": {"id": thread_id}}}));
            }
            "turn/start" => {
                turn_count += 1;
                let thread_id = params.get("threadId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let turn_id = format!("turn-{turn_count}");
                send(json!({"id": id, "result": {"turn": {"id": turn_id, "status": "inProgress"}}}));
                turns.insert(turn_id.clone(), (thread_id.clone(), "inProgress".into()));
                match mode.as_str() {
                    "approval" | "approval-grant" => {
                        send(json!({
                            "id": 9001,
                            "method": "item/commandExecution/requestApproval",
                            "params": {"threadId": thread_id, "turnId": turn_id, "itemId": "exec-1",
                                       "command": "echo probe", "reason": "fake approval"},
                        }));
                        turns.insert(turn_id.clone(), (thread_id, "awaitingApproval".into()));
                    }
                    // crash mid-turn: the reply is out, no turn/completed follows
                    "die" => std::process::exit(1),
                    "slow" => {}
                    _ => {
                        let item = json!({"type": "agentMessage", "text": "fake work done"});
                        send(
                            json!({"method": "item/completed", "params": {"threadId": thread_id, "turnId": turn_id, "item": item}}),
                        );
                        turns.insert(turn_id.clone(), (thread_id.clone(), "completed".into()));
                        send(
                            json!({"method": "turn/completed", "params": {"threadId": thread_id, "turn": {"id": turn_id, "status": "completed"}}}),
                        );
                    }
                }
            }
            "turn/interrupt" => {
                let turn_id = params.get("turnId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                send(json!({"id": id, "result": {}}));
                if let Some((thread_id, _)) = turns.get(&turn_id).cloned() {
                    turns.insert(turn_id.clone(), (thread_id.clone(), "interrupted".into()));
                    send(
                        json!({"method": "turn/completed", "params": {"threadId": thread_id, "turn": {"id": turn_id, "status": "interrupted"}}}),
                    );
                }
            }
            _ => send(json!({"id": id, "result": {}})),
        }
    }
}
