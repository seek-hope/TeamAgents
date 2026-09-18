//! Session inventory / lock / archive / delete.

use crate::config::sessions_dir;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub struct SessionPaths {
    pub base: PathBuf,
    pub db: PathBuf,
    pub artifacts: PathBuf,
    pub locks: PathBuf,
    pub lock: PathBuf,
}

/// Same alphabet as agent ids (core models.rs::TeamSpec::validate). A session
/// id becomes a directory name; anything outside the whitelist never reaches
/// join(), so ".." cannot walk out of the sessions root.
pub(crate) fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty() || !session_id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("invalid session id {session_id:?}: use letters, digits, '-', '_'"));
    }
    Ok(())
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

/// An flock held for as long as the session runs.
/// The kernel releases it when the process dies — kill -9 included — so a
/// crashed run never leaves the session "already running".
pub struct SessionLock {
    file: std::fs::File,
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Err means another process holds the session lock.
pub fn acquire_session_lock(session_id: &str) -> Result<SessionLock, String> {
    validate_session_id(session_id)?;
    let paths = session_paths(session_id);
    std::fs::create_dir_all(&paths.base).map_err(|e| e.to_string())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&paths.lock)
        .map_err(|e| format!("cannot lock session {session_id}: {e}"))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            let holder = std::fs::read_to_string(&paths.lock).unwrap_or_default();
            let holder = holder.trim();
            return Err(if holder.is_empty() {
                format!("session {session_id} is already running in another process")
            } else {
                format!("session {session_id} is already running (pid {holder})")
            });
        }
        Err(std::fs::TryLockError::Error(e)) => return Err(format!("cannot lock session {session_id}: {e}")),
    }
    // diagnostic only: the pid is not what makes the lock exclusive
    use std::io::Write;
    let _ = file.set_len(0);
    let _ = write!(&file, "{}", std::process::id());
    Ok(SessionLock { file })
}

pub fn is_session_locked(session_id: &str, base: Option<&Path>) -> bool {
    let lock = base.unwrap_or(&sessions_dir()).join(session_id).join("session.lock");
    let Ok(file) = std::fs::File::open(&lock) else { return false };
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
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

/// Size of a session directory. The walk is O(files) and the TUI polls
/// list_sessions every second, so the result is cached for a short TTL.
/// ponytail: sizes lag up to 30s; push updates from the writer side if the
/// panel must be live.
fn dir_size_mb(path: &Path) -> f64 {
    const TTL: Duration = Duration::from_secs(30);
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (Instant, f64)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((computed_at, size)) = cache.lock().unwrap().get(path) {
        if computed_at.elapsed() < TTL {
            return *size;
        }
    }
    let size = compute_dir_size_mb(path);
    cache.lock().unwrap().insert(path.to_path_buf(), (Instant::now(), size));
    size
}

fn compute_dir_size_mb(path: &Path) -> f64 {
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
                permissions_mode: if meta.permissions_mode.is_empty() {
                    "?".into()
                } else {
                    meta.permissions_mode.clone()
                },
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
        (a.archived as i64, -(a.updated_at * 1000.0) as i64).cmp(&(b.archived as i64, -(b.updated_at * 1000.0) as i64))
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
    validate_session_id(session_id)?;
    let root = base.map(Path::to_path_buf).unwrap_or_else(sessions_dir);
    if is_session_locked(session_id, Some(&root)) {
        return Err(format!("session {session_id} is running"));
    }
    let source = root.join(session_id);
    // checked before touching any old archive: a failed rename must never
    // leave the previously archived copy destroyed
    // team.db specifically, not the bare dir: a ghost left by a failed open
    // (artifacts/ + session.lock only) is not a session and must not be
    // archived over the real copy (same filter as list_sessions)
    if !source.join("team.db").exists() {
        return Err(format!("session {session_id} does not exist"));
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

/// Refuses while it runs elsewhere, and refuses to delete member worktrees
/// that still hold uncommitted or unmerged work.
/// One session's database housekeeping: applied deliveries and events older
/// than `days` go, pending deliveries (and the events they still need) stay.
pub fn prune_session_history(session_id: &str, days: u64, dry_run: bool) -> Result<serde_json::Value, String> {
    validate_session_id(session_id)?;
    let root = sessions_dir();
    for base in [root.clone(), root.join("archived")] {
        let path = base.join(session_id);
        if !path.join("team.db").exists() {
            continue;
        }
        if !dry_run && is_session_locked(session_id, Some(&base)) {
            return Err(format!("session {session_id} is running"));
        }
        let store = teamagents_core::storage::Store::open(&path.join("team.db")).map_err(|e| e.to_string())?;
        let (deliveries, events, vacuumed) =
            store.prune_history(session_id, days, dry_run).map_err(|e| e.to_string())?;
        return Ok(json!({
            "session_id": session_id,
            "days": days,
            "dry_run": dry_run,
            "deliveries": deliveries,
            "events": events,
            "vacuumed": vacuumed,
            "size_mb": dir_size_mb(&path),
        }));
    }
    Err(format!("unknown session {session_id}"))
}

/// Retention sweep: archived sessions untouched for `days` are removed through
/// the same guarded path the UI uses (never a running session, never a worktree
/// with unmerged work). Errors are collected per session instead of aborting the
/// sweep, and `dry_run` reports without deleting.
pub fn prune_archived(days: u64, base: Option<&Path>, dry_run: bool) -> Json {
    let archived = base.map(Path::to_path_buf).unwrap_or_else(sessions_dir).join("archived");
    let mut removed: Vec<Json> = vec![];
    let mut kept = 0usize;
    let mut skipped: Vec<Json> = vec![];
    let mut bytes_freed: u64 = 0;
    let now = std::time::SystemTime::now();
    if let Ok(entries) = std::fs::read_dir(&archived) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.join("team.db").exists() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            let age_days = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .map(|age| age.as_secs() / 86_400)
                .unwrap_or(0);
            if age_days < days {
                kept += 1;
                continue;
            }
            let size = dir_size_mb(&path);
            if dry_run {
                removed.push(json!({"session_id": id, "age_days": age_days, "size_mb": size}));
                continue;
            }
            match delete_session(&id, Some(&archived)) {
                Ok(()) => {
                    bytes_freed = bytes_freed.saturating_add((size * 1024.0 * 1024.0) as u64);
                    removed.push(json!({"session_id": id, "age_days": age_days, "size_mb": size}));
                }
                Err(error) => skipped.push(json!({"session_id": id, "error": error})),
            }
        }
    }
    json!({
        "archived_dir": archived.to_string_lossy(),
        "days": days,
        "dry_run": dry_run,
        "removed": removed,
        "kept": kept,
        "skipped": skipped,
        "bytes_freed": bytes_freed,
    })
}

pub fn delete_session(session_id: &str, base: Option<&Path>) -> Result<(), String> {
    validate_session_id(session_id)?;
    let root = base.map(Path::to_path_buf).unwrap_or_else(sessions_dir);
    if is_session_locked(session_id, Some(&root)) {
        return Err(format!("session {session_id} is running"));
    }
    let path = root.join(session_id);
    let worktrees = crate::workspace::member_worktrees(&path);
    if !worktrees.is_empty() {
        let project_cwd = read_meta(&path).cwd;
        if project_cwd.is_empty() {
            return Err(
                "session has member worktrees but its project directory is unknown; remove them manually first".into(),
            );
        }
        for work in worktrees {
            let branch = std::process::Command::new("git")
                .args(["-C", &work.to_string_lossy(), "rev-parse", "--abbrev-ref", "HEAD"])
                .output()
                .ok()
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .filter(|b| !b.is_empty());
            let workspace = crate::workspace::Workspace {
                path: work.clone(),
                policy: teamagents_core::models::WorkspacePolicy::GitWorktree,
                note: None,
                branch,
                base_commit: None,
            };
            let (ok, reason) = crate::workspace::cleanup(&workspace, Path::new(&project_cwd), false);
            if !ok {
                return Err(format!("member worktree {} keeps unmerged or uncommitted work: {reason}", work.display()));
            }
        }
    }
    std::fs::remove_dir_all(&path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_removes_only_old_archived_sessions() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-retention-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let archived = root.join("archived");
        let make = |id: &str| {
            let path = archived.join(id);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("team.db"), b"x").unwrap();
            path
        };
        let old = make("proj_old");
        let fresh = make("proj_fresh");
        // age the directory entry itself: pruning reads mtime, not db contents
        let stale = filetime_days_ago(&old, 40);
        assert!(stale, "could not age the archived session");

        let dry = prune_archived(30, Some(&root), true);
        assert_eq!(dry["removed"].as_array().unwrap().len(), 1);
        assert!(old.exists() && fresh.exists(), "dry run deletes nothing");

        let report = prune_archived(30, Some(&root), false);
        assert_eq!(report["removed"][0]["session_id"], "proj_old", "{report}");
        assert_eq!(report["kept"], 1, "{report}");
        assert!(!old.exists(), "the stale archive is gone");
        assert!(fresh.exists(), "a recent archive stays");

        // a live session directory (not archived) is never a candidate
        let live = root.join("proj_live");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join("team.db"), b"x").unwrap();
        assert_eq!(filetime_days_ago(&live, 90), true);
        let report = prune_archived(30, Some(&root), false);
        assert!(live.exists(), "pruning only walks the archive");
        assert_eq!(report["removed"].as_array().unwrap().len(), 0, "{report}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Backdate a directory's mtime with `touch -d`, so the test does not need a
    /// file-metadata dependency.
    fn filetime_days_ago(path: &Path, days: u64) -> bool {
        std::process::Command::new("touch")
            .arg("-d")
            .arg(format!("{days} days ago"))
            .arg(path)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[test]
    fn lock_is_held_by_the_live_holder_not_by_file_content() {
        let _env = crate::env_lock();
        let dir = std::env::temp_dir().join(format!("ta-lock-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("XDG_STATE_HOME", dir.join("state"));
        let session = "proj_test";
        assert!(!is_session_locked(session, None), "no lock file yet");
        let lock = acquire_session_lock(session).expect("first lock");
        assert!(is_session_locked(session, None));
        // the old pid-file window: an emptied lock file must not free the session
        let paths = session_paths(session);
        std::fs::write(&paths.lock, "").unwrap();
        assert!(is_session_locked(session, None), "content is not the lock");
        assert!(acquire_session_lock(session).is_err(), "second holder is refused");
        drop(lock);
        assert!(!is_session_locked(session, None), "dropping the handle releases it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_ids_outside_the_whitelist_are_refused_before_touching_disk() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-sid-test-{}", std::process::id()));
        let state = std::env::temp_dir().join(format!("ta-sid-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);
        std::env::set_var("XDG_STATE_HOME", &state);
        for bad in ["../../../x", "..", "a/b", ""] {
            assert!(delete_session(bad, Some(&root)).is_err(), "delete {bad:?}");
            assert!(archive_session(bad, Some(&root)).is_err(), "archive {bad:?}");
            assert!(acquire_session_lock(bad).is_err(), "lock {bad:?}");
            let opened = crate::session::open_session(crate::session::OpenOptions {
                session_id: Some((*bad).into()),
                ..Default::default()
            });
            assert!(opened.is_err(), "open {bad:?}");
        }
        assert!(!state.exists(), "open_session created nothing outside the sessions root");
        assert!(validate_session_id("proj_ok-1").is_ok());
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn archiving_an_id_that_only_exists_in_the_archive_keeps_the_archive() {
        let root = std::env::temp_dir().join(format!("ta-arch-only-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let marker = root.join("archived/s-only/marker");
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, "data").unwrap();
        assert!(archive_session("s-only", Some(&root)).is_err(), "no live session to archive");
        assert!(marker.exists(), "the archived copy survived the failed rename");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deleting_a_missing_session_is_an_error_not_a_success() {
        let root = std::env::temp_dir().join(format!("ta-del-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(delete_session("ghost", Some(&root)).is_err(), "nothing to delete");
        std::fs::create_dir_all(root.join("present")).unwrap();
        assert!(delete_session("present", Some(&root)).is_ok());
        assert!(!root.join("present").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dir_size_is_cached_between_calls() {
        let dir = std::env::temp_dir().join(format!("ta-size-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.bin"), vec![0u8; 100_000]).unwrap();
        let first = dir_size_mb(&dir);
        assert!((first - 0.1).abs() < 0.001, "{first}");
        std::fs::write(dir.join("b.bin"), vec![0u8; 100_000]).unwrap();
        assert_eq!(dir_size_mb(&dir), first, "second call inside the TTL is cached");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
