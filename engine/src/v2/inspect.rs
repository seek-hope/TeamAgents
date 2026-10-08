//! Read-only developer views over the existing daemon protocol (D-445).

use super::exec::{resolve_prefix, Client};
use serde_json::{json, Value as Json};
use std::path::Path;

/// Inspect a running session without starting one or changing its state. History
/// is a bounded tail; the JSON preserves complete stored messages within it.
pub fn run(socket: &Path, history: Option<(&str, u64)>, json_out: bool) -> i32 {
    match inspect(socket, history) {
        Ok(report) => {
            if json_out {
                println!("{report}");
            } else if history.is_some() {
                println!("session {} · instance {}", report["session_id"], report["instance_id"]);
                for entry in report["entries"].as_array().into_iter().flatten() {
                    println!(
                        "\n{}:{} {}\n{}",
                        entry["epoch"],
                        entry["idx"],
                        entry["kind"],
                        serde_json::to_string_pretty(&entry["message"]).unwrap_or_default()
                    );
                }
            } else {
                println!(
                    "session {}\nworkspace {}\npermissions {}\nwatermark {}",
                    report["session_id"], report["session_workspace"], report["permissions"], report["watermark"]
                );
                println!("{}", serde_json::to_string_pretty(&report["snapshot"]).unwrap_or_default());
            }
            0
        }
        Err((code, message)) => {
            eprintln!("{message}");
            code
        }
    }
}

fn inspect(socket: &Path, history: Option<(&str, u64)>) -> Result<Json, (i32, String)> {
    let mut client = Client::connect(socket).map_err(|e| (2, format!("inspect: {e}")))?;
    let checkpoint = client.call("checkpoint", json!({})).map_err(|e| (1, e))?;
    if let Some((id, limit)) = history {
        let rows = checkpoint["snapshot"]["instances"]
            .as_array()
            .ok_or_else(|| (1, "inspect: checkpoint has no instances array".into()))?;
        let row = resolve_prefix(rows, "id", id, "instance", "teamagents instances")?;
        let result = client.call("history", json!({"instance_id": row["id"], "limit": limit})).map_err(|e| (1, e))?;
        Ok(json!({"schema_version": 1, "session_id": client.session_id, "state_root": client.state_root,
            "instance_id": result["instance_id"], "limit": limit, "entries": result["entries"]}))
    } else {
        Ok(json!({"schema_version": 1, "session_id": client.session_id, "state_root": client.state_root,
            "permissions": client.permissions, "session_workspace": client.session_workspace,
            "watermark": checkpoint["watermark"], "snapshot": checkpoint["snapshot"]}))
    }
}
