//! Session inventory / lock / archive / delete (sessions.py, session paths).
//!
//! ponytail: the lock is a pid file checked against /proc (Python uses flock);
//! the semantic both versions need — "a live process holds this session" —
//! is identical, and this works without libc bindings.

use crate::config::sessions_dir;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub struct SessionPaths {
    pub base: PathBuf,
    pub db: PathBuf,
    pub artifacts: PathBuf,
    pub locks: PathBuf,
    pub lock: PathBuf,
}

pub fn session_paths(session_id: &str) -> SessionPaths {
    let base = sessions_dir().join(session_id);
    SessionPaths {
        db: base.join("team.db"),
        artifacts: base.join("artifacts"),
        locks: base.join("locks"),
        lock: base.join("session.lock"),
        base,
    }
}

pub fn default_session_id(cwd: &Path) -> String {
    let base = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let hex = format!("{:x}", Sha256::digest(base.to_string_lossy().as_bytes()));
    format!("proj_{}", &hex[..12])
}

fn pid_alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Returns a release closure; Err means another live process holds the session.
pub fn acquire_session_lock(session_id: &str) -> Result<impl FnOnce(), String> {
    let paths = session_paths(session_id);
    std::fs::create_dir_all(&paths.base).map_err(|e| e.to_string())?;
    if try_create_lock(&paths.lock).is_err() {
        let holder = std::fs::read_to_string(&paths.lock)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok());
        match holder {
            Some(pid) if pid_alive(pid) => {
                return Err(format!("session {session_id} is already running (pid {pid})"))
            }
            _ => {
                // stale lock: the recorded process is gone
                let _ = std::fs::remove_file(&paths.lock);
                try_create_lock(&paths.lock)
                    .map_err(|e| format!("cannot lock session {session_id}: {e}"))?;
            }
        }
    }
    let lock = paths.lock.clone();
    Ok(move || {
        let _ = std::fs::remove_file(&lock);
    })
}

fn try_create_lock(lock: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(lock)?;
    file.write_all(std::process::id().to_string().as_bytes())
}

pub fn is_session_locked(session_id: &str, base: Option<&Path>) -> bool {
    let lock = base.unwrap_or(&sessions_dir()).join(session_id).join("session.lock");
    std::fs::read_to_string(&lock)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .map(pid_alive)
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub path: String,
    pub cwd: String,
    pub status: String,
    #[serde(rename = "goalState")]
    pub goal_state: String,
    #[serde(rename = "permissionsMode")]
    pub permissions_mode: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: f64,
    pub events: i64,
    pub tasks: i64,
    #[serde(rename = "sizeMb")]
    pub size_mb: f64,
    pub archived: bool,
    pub locked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn read_meta(path: &Path) -> Meta {
    let db = path.join("team.db");
    let open = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY);
    let conn = match open {
        Ok(c) => c,
        Err(e) => return Meta { error: Some(e.to_string()), ..Meta::default() },
    };
    let row = conn.query_row(
        "SELECT status, cwd, permissions_mode, goal_state, updated_at FROM sessions ORDER BY updated_at DESC LIMIT 1",
        [],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, f64>(4)?,
            ))
        },
    );
    let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0) };
    match row {
        Ok((status, cwd, mode, goal, updated)) => Meta {
            status,
            cwd,
            permissions_mode: mode,
            goal_state: goal,
            updated_at: updated,
            events: count("SELECT COUNT(*) FROM events"),
            tasks: count("SELECT COUNT(*) FROM tasks"),
            error: None,
        },
        Err(e) => Meta { error: Some(e.to_string()), ..Meta::default() },
    }
}

#[derive(Default)]
struct Meta {
    status: String,
    cwd: String,
    permissions_mode: String,
    goal_state: String,
    updated_at: f64,
    events: i64,
    tasks: i64,
    error: Option<String>,
}

fn dir_size_mb(path: &Path) -> f64 {
    fn walk(dir: &Path, total: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                walk(&entry.path(), total);
            } else {
                *total += meta.len();
            }
        }
    }
    let mut total = 0u64;
    walk(path, &mut total);
    (total as f64 / 1e5).round() / 10.0
}

pub fn list_sessions(cwd: Option<&Path>, include_archived: bool, base: Option<&Path>) -> Vec<SessionInfo> {
    let root = base.map(Path::to_path_buf).unwrap_or_else(sessions_dir);
    let wanted = cwd.map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    let mut groups = vec![(root.clone(), false)];
    if include_archived {
        groups.push((root.join("archived"), true));
    }
    let mut infos = vec![];
    for (group_root, archived) in groups {
        let Ok(entries) = std::fs::read_dir(&group_root) else { continue };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().join("team.db").exists())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            let path = group_root.join(&name);
            let meta = read_meta(&path);
            let info = SessionInfo {
                session_id: name.clone(),
                path: path.to_string_lossy().into_owned(),
                cwd: meta.cwd.clone(),
                status: if meta.status.is_empty() { "?".into() } else { meta.status.clone() },
                goal_state: if meta.goal_state.is_empty() { "?".into() } else { meta.goal_state.clone() },
                permissions_mode: if meta.permissions_mode.is_empty() { "?".into() } else { meta.permissions_mode.clone() },
                updated_at: meta.updated_at,
                events: meta.events,
                tasks: meta.tasks,
                size_mb: dir_size_mb(&path),
                archived,
                locked: is_session_locked(&name, Some(&group_root)),
                error: meta.error.clone(),
            };
            if !archived {
                if let Some(wanted) = &wanted {
                    let here = std::fs::canonicalize(&info.cwd).unwrap_or_else(|_| PathBuf::from(&info.cwd));
                    if &here != wanted {
                        continue;
                    }
                }
            }
            infos.push(info);
        }
    }
    infos.sort_by(|a, b| {
        (a.archived as i64, -(a.updated_at * 1000.0) as i64)
            .cmp(&(b.archived as i64, -(b.updated_at * 1000.0) as i64))
    });
    infos
}

pub fn new_session_id(cwd: &Path) -> String {
    let root = sessions_dir();
    let mut existing = std::collections::HashSet::new();
    for group in [root.clone(), root.join("archived")] {
        if let Ok(entries) = std::fs::read_dir(group) {
            for name in entries.flatten() {
                existing.insert(name.file_name().to_string_lossy().into_owned());
            }
        }
    }
    let base = default_session_id(cwd);
    if !existing.contains(&base) {
        return base;
    }
    let mut index = 2;
    while existing.contains(&format!("{base}_{index}")) {
        index += 1;
    }
    format!("{base}_{index}")
}

pub fn archive_session(session_id: &str, base: Option<&Path>) -> Result<String, String> {
    let root = base.map(Path::to_path_buf).unwrap_or_else(sessions_dir);
    if is_session_locked(session_id, Some(&root)) {
        return Err(format!("session {session_id} is running"));
    }
    let target_dir = root.join("archived");
    std::fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
    let target = target_dir.join(session_id);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
    }
    std::fs::rename(root.join(session_id), &target).map_err(|e| e.to_string())?;
    Ok(target.to_string_lossy().into_owned())
}

pub fn delete_session(session_id: &str, base: Option<&Path>) -> Result<(), String> {
    let root = base.map(Path::to_path_buf).unwrap_or_else(sessions_dir);
    if is_session_locked(session_id, Some(&root)) {
        return Err(format!("session {session_id} is running"));
    }
    let path = root.join(session_id);
    match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_rejects_live_holder_and_reclaims_stale() {
        let dir = std::env::temp_dir().join(format!("ta-lock-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let session = "proj_test";
        // point the module at a scratch root via a hand-made lock file
        let base = dir.join(session);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("session.lock"), std::process::id().to_string()).unwrap();
        assert!(is_session_locked(session, Some(&dir)));
        std::fs::write(base.join("session.lock"), "999999").unwrap();
        assert!(!is_session_locked(session, Some(&dir)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
