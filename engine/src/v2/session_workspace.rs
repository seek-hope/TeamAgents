//! Workspace-bound session selection and successful-open recency (D-526).
//! These client hints never replace the Leader's authoritative workspace in SQLite.
use std::path::{Path, PathBuf};

use serde_json::{json, Value as Json};

use super::sessions::{base_of, Registry};

pub const FILE: &str = "session.workspace.json";

pub struct Metadata {
    pub workspace: PathBuf,
    pub last_used_ms: u64,
}

pub fn canonical_workspace(path: &Path) -> Result<PathBuf, String> {
    crate::cli::require_workspace_dir(path)?;
    path.canonicalize().map_err(|error| format!("workspace {}: {error}", path.display()))
}

pub fn metadata(dir: &Path) -> Result<Option<Metadata>, String> {
    let path = dir.join(FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let raw: Json = serde_json::from_slice(&bytes).map_err(|error| format!("read {}: {error}", path.display()))?;
    let workspace = PathBuf::from(raw["workspace"].as_str().unwrap_or_default());
    let last_used_ms = raw["last_used_ms"].as_u64();
    if raw["version"] != 1 || !workspace.is_absolute() || last_used_ms.is_none() {
        return Err(format!("invalid workspace record at {}", path.display()));
    }
    Ok(Some(Metadata { workspace, last_used_ms: last_used_ms.unwrap_or(0) }))
}

fn write(dir: &Path, workspace: &Path, last_used_ms: u64) -> Result<(), String> {
    let path = dir.join(FILE);
    let temporary = dir.join(format!(".{FILE}.{}", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec(&json!({"version":1,"workspace":workspace,"last_used_ms":last_used_ms}))
        .map_err(|error| error.to_string())?;
    let result = (|| {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        std::fs::File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|error: std::io::Error| format!("write {}: {error}", path.display()))
}

/// Bind an empty session at creation, before any daemon or model call. Caller holds the registry lock.
pub fn bind(dir: &Path, workspace: &Path) -> Result<(), String> {
    write(dir, &canonical_workspace(workspace)?, 0)
}

fn recency(dir: &Path) -> u64 {
    match metadata(dir) {
        Ok(record) => record.map_or(0, |record| record.last_used_ms),
        Err(error) => {
            eprintln!("note: ignoring unavailable session recency: {error}");
            0
        }
    }
}

/// Caller holds the registry lock. Equal/rolled-back wall-clock values still produce a strict use order.
pub fn touch(registry: &Registry, dir: &Path, workspace: &Path) -> Result<(), String> {
    if !workspace.is_absolute() {
        return Err(
            "the daemon reported a relative workspace; its original directory cannot be recovered safely".into()
        );
    }
    let workspace = canonical_workspace(workspace)?;
    let (_, saved) = super::session_browser::resume_settings(dir)?;
    if let Some(saved) = saved {
        if canonical_workspace(&saved)? != workspace {
            return Err(format!(
                "daemon workspace does not match the saved workspace at {}; restart that session explicitly",
                dir.display()
            ));
        }
    }
    let mut latest = 0;
    for (_, candidate) in registry.rows() {
        latest = latest.max(recency(&candidate));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis() as u64;
    let next = latest.checked_add(1).ok_or("session recency counter exhausted")?.max(now);
    write(dir, &workspace, next)
}

/// Record a successful client connection, including explicit state-root/ID opens.
pub fn opened(dir: &Path, socket: &Path) -> Result<(), String> {
    let client = super::exec::Client::connect(socket)?;
    let home = base_of(dir);
    let _lock = crate::jobs::state_lock(&home.join("sessions.lock"))?;
    touch(&Registry::load(&home)?, dir, Path::new(&client.session_workspace))
}

/// Select only nonarchived sessions in this exact canonical directory, or create an empty bound session.
pub fn select(home: &Path, workspace: &Path, create: bool) -> Result<Option<PathBuf>, String> {
    let workspace = canonical_workspace(workspace)?;
    if !home.exists() && !create {
        return Ok(None);
    }
    if create {
        std::fs::create_dir_all(home).map_err(|error| crate::cli::state_root_uncreatable(home, &error))?;
    }
    let home = home.canonicalize().map_err(|error| error.to_string())?;
    crate::cli::require_state_paths_kind(&home)?;
    let _lock = if create { Some(crate::jobs::state_lock(&home.join("sessions.lock"))?) } else { None };
    let mut registry = Registry::load(&home)?;
    let mut candidates = Vec::new();
    // ponytail: selection scans read-only session settings; add a validated workspace index if session counts
    // make startup expensive. The index must never override an existing Leader's persisted workspace.
    for (entry, dir) in registry.rows().into_iter().filter(|(entry, dir)| !entry.archived && dir.is_dir()) {
        let settings =
            crate::cli::require_state_paths_kind(&dir).and_then(|()| super::session_browser::resume_settings(&dir));
        let saved = match settings {
            Ok((_, saved)) => saved,
            Err(error) => {
                eprintln!("note: skipping session {} during workspace selection: {error}", entry.id);
                continue;
            }
        };
        if let Some(saved) = saved {
            if saved.canonicalize().ok().as_ref() == Some(&workspace) {
                candidates.push(((recency(&dir), entry.created_ms, entry.id), dir));
            }
        }
    }
    if let Some((_, dir)) = candidates.into_iter().max_by(|a, b| a.0.cmp(&b.0)) {
        return Ok(Some(dir));
    }
    if !create {
        return Ok(None);
    }
    let name = workspace.file_name().unwrap_or(workspace.as_os_str()).to_string_lossy();
    let entry = registry.new_session(&name)?;
    let dir = entry.dir(&home);
    bind(&dir, &workspace)?;
    Ok(Some(dir))
}
