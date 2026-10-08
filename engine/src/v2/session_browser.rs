//! D-446: engine operations used by the session browser and the developer CLI.
//! Reading history never starts a daemon. Deletion stops only the addressed root,
//! takes its coordinator lock, and retains unrelated data under the default base.
use std::path::{Path, PathBuf};

use serde_json::{json, Value as Json};

use super::sessions::{Registry, DEFAULT_ID};

pub fn directory(registry: &Registry, id: &str) -> Result<PathBuf, String> {
    if id == DEFAULT_ID {
        return Ok(registry.home.clone());
    }
    registry.find(id).map(|entry| entry.dir(&registry.home)).ok_or_else(|| format!("no session {id:?}"))
}

/// Stable backwards paging over all members and epochs; new messages cannot move the cursor.
pub fn history(registry: &Registry, id: &str, before: Option<i64>, limit: u64) -> Result<Json, String> {
    if !(1..=1000).contains(&limit) || before.is_some_and(|cursor| cursor <= 0) {
        return Err("history needs --limit 1..1000 and a positive --before cursor".into());
    }
    let db = directory(registry, id)?.join("session.sqlite");
    let mut entries = Vec::new();
    if db.exists() {
        let conn = teamagents_core::v2::store::open_read_only(&db)?;
        let mut stmt = conn
            .prepare(
                "SELECT rowid, instance_id, epoch, idx, kind, message_json FROM context_entries
             WHERE (?1 IS NULL OR rowid < ?1) ORDER BY rowid DESC LIMIT ?2",
            )
            .map_err(|e| format!("session history: {e}"))?;
        entries = stmt.query_map(rusqlite::params![before, limit as i64 + 1], |row| {
            let message: String = row.get(5)?;
            Ok(json!({"cursor": row.get::<_, i64>(0)?, "instance_id": row.get::<_, String>(1)?,
                "epoch": row.get::<_, i64>(2)?, "idx": row.get::<_, i64>(3)?,
                "kind": row.get::<_, String>(4)?, "message": serde_json::from_str::<Json>(&message).unwrap_or(Json::Null)}))
        }).map_err(|e| format!("session history: {e}"))?
            .collect::<Result<Vec<_>, _>>().map_err(|e| format!("session history: {e}"))?;
    }
    let more = entries.len() > limit as usize;
    entries.truncate(limit as usize);
    let next = if more { entries.last().and_then(|entry| entry["cursor"].as_i64()) } else { None };
    entries.reverse();
    Ok(json!({"id": id, "entries": entries, "next_before": next}))
}

/// Reopen using the stored Leader's workspace/model, without silently adopting the other session's settings.
pub fn resume_settings(dir: &Path) -> Result<(Option<String>, Option<PathBuf>), String> {
    if !dir.join("session.sqlite").exists() {
        return Ok((None, None));
    }
    let conn = teamagents_core::v2::store::open_read_only(&dir.join("session.sqlite"))?;
    use rusqlite::OptionalExtension;
    let row: Option<(String, String)> = conn
        .query_row("SELECT workspace_ref, profile_json FROM instances WHERE id = 'i-leader'", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .map_err(|e| format!("session settings: {e}"))?;
    let Some((workspace, profile)) = row else {
        return Ok((None, None));
    };
    let profile: Json = serde_json::from_str(&profile).map_err(|e| format!("session profile: {e}"))?;
    Ok((profile["model"].as_str().map(str::to_string), (!workspace.is_empty()).then(|| PathBuf::from(workspace))))
}

/// Explicit deletion, unlike age-based pruning. The CLI owns the base registry lock throughout this call.
pub fn delete(registry: &mut Registry, id: &str, stop: bool) -> Result<PathBuf, String> {
    let dir = directory(registry, id)?;
    // Refuse path aliases before any stop or filesystem mutation. Registry paths are generated in this shape.
    if id != DEFAULT_ID {
        let entry = registry.find(id).ok_or_else(|| format!("no session {id:?}"))?;
        let expected = PathBuf::from(if entry.archived { "archive" } else { "sessions" }).join(id);
        if entry.path != expected || id.is_empty() || id.contains(['/', '\\']) || id == "." || id == ".." {
            return Err("session path does not match its registered identity".into());
        }
        if dir.exists() {
            let base = registry.home.canonicalize().map_err(|e| e.to_string())?;
            let actual = dir.canonicalize().map_err(|e| e.to_string())?;
            if actual != base.join(expected) {
                return Err("session deletion refuses a symlinked session directory".into());
            }
        }
    }
    protect_workspaces(&dir)?;
    if !dir.exists() {
        return registry.delete(id);
    }
    if stop {
        stop_daemon(&dir)?;
    } else {
        super::sessions::refuse_if_live(&dir, id)?;
    }
    let _coordinator = crate::jobs::state_lock(&dir.join("coordinator.lock"))?;
    super::runners::quiesce_for_delete(&dir)?;
    // A coordinator could have prepared a member after the first check, before its stop completed.
    protect_workspaces(&dir)?;
    if id != DEFAULT_ID {
        return registry.delete(id);
    }
    // The default root also owns named sessions, memory and registry files. Only session-owned names go.
    for name in [
        "session.sqlite",
        "session.sqlite-wal",
        "session.sqlite-shm",
        "daemon.sock",
        "daemon.log",
        "instances",
        "jobs",
        "artifacts",
        "shell",
        "codemode-store.json",
        "automations.json",
        "verification.json",
    ] {
        let path = dir.join(name);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => std::fs::remove_file(&path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("inspect {}: {e}", path.display())),
        }
        .map_err(|e| format!("remove {}: {e}", path.display()))?;
    }
    Ok(dir)
}

fn protect_workspaces(dir: &Path) -> Result<(), String> {
    for name in ["instances", "jobs", "artifacts", "shell"] {
        if std::fs::symlink_metadata(dir.join(name)).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(format!("session deletion refuses symlinked state at {}", dir.join(name).display()));
        }
    }
    // Member workspaces may hold uncommitted/unmerged user work. Never erase them with a conversation.
    if let Ok(members) = std::fs::read_dir(dir.join("instances")) {
        for member in members {
            let member = member.map_err(|e| e.to_string())?;
            if member.file_type().map_err(|e| e.to_string())?.is_symlink() {
                return Err("session deletion refuses a symlinked member directory".into());
            }
            if member.path().join("work").exists() {
                return Err(format!(
                    "session contains a member workspace at {}; retire or move that workspace before deletion",
                    member.path().join("work").display()
                ));
            }
        }
    }
    Ok(())
}

fn stop_daemon(dir: &Path) -> Result<(), String> {
    let socket = dir.join("daemon.sock");
    if !socket.exists() {
        return Ok(());
    }
    let mut client = match super::exec::Client::connect(&socket) {
        Ok(client) => client,
        // A stale socket is safe only if we can subsequently take the coordinator lock.
        Err(_) => return Ok(()),
    };
    client.call("shutdown", json!({}))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while socket.exists() {
        if std::time::Instant::now() >= deadline {
            return Err("session shutdown did not finish within 5 seconds; history was kept".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Ok(())
}
