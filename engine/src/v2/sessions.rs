//! Named sessions beyond the default (D-364): a base directory holds a registry and one directory per named
//! session. "One session per state root" — the coordinator-lock rule of A33/`V2Coordinator` — is untouched:
//! each named session *is* its own state root, so several sessions run side by side without sharing a lock.
//!
//! The default session is the base directory itself (`<base>/session.sqlite`), exactly where a session has
//! always lived, so a user with an existing state root keeps it: `teamagents` with no flags still runs the
//! default session. `sessions new` creates `<base>/sessions/<id>/` and records it in `<base>/sessions.json`;
//! `--session <id>` resolves a record to its directory. The registry is the only new persistent file, and it is
//! validated on load in both directions: two ids may not name one path, a path may not escape the base, and a
//! file this build does not understand is refused rather than guessed (`V2Sessions.tla` states these rules).
//!
//! `list` reads only filesystem facts (does a `session.sqlite` exist, how big, is a daemon listening). It never
//! opens another session's database, so managing sessions has no side effect on the sessions themselves.

use serde_json::{json, Value as Json};
use std::path::{Component, Path, PathBuf};

/// The id that always names the base directory's own session.
pub const DEFAULT_ID: &str = "default";
/// The registry file inside the base directory.
const REGISTRY_FILE: &str = "sessions.json";
/// The registry layout this build understands; a different one is refused, never rewritten on a guess.
const REGISTRY_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub id: String,
    pub name: String,
    /// Relative to the base directory, so the tree can move with it.
    pub path: PathBuf,
    pub created_ms: u64,
    pub archived: bool,
}

impl SessionEntry {
    pub fn dir(&self, home: &Path) -> PathBuf {
        home.join(&self.path)
    }
}

#[derive(Debug, Clone)]
pub struct Registry {
    pub home: PathBuf,
    pub sessions: Vec<SessionEntry>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A registry path must stay inside the base: relative, with no `..` and no root.
fn safe_relative(path: &Path) -> bool {
    !path.is_absolute()
        && !path.as_os_str().is_empty()
        && path.components().all(|part| matches!(part, Component::Normal(_)))
}

impl Registry {
    /// Load the registry of `home`, or an empty one when the file does not exist yet.
    ///
    /// A file this build cannot read *in full* is an error: an unknown version, a duplicate id, two ids naming
    /// one path, or a path that escapes the base all refuse rather than silently dropping a session.
    pub fn load(home: &Path) -> Result<Registry, String> {
        let file = home.join(REGISTRY_FILE);
        if !file.exists() {
            return Ok(Registry { home: home.to_path_buf(), sessions: Vec::new() });
        }
        let text = std::fs::read_to_string(&file).map_err(|e| format!("read {}: {e}", file.display()))?;
        let parsed: Json = serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", file.display()))?;
        let version = parsed["version"].as_i64().unwrap_or(-1);
        if version != REGISTRY_VERSION {
            return Err(format!(
                "{} is version {version}, this build writes version {REGISTRY_VERSION}: refusing to reinterpret it",
                file.display()
            ));
        }
        let mut sessions = Vec::new();
        for raw in parsed["sessions"].as_array().cloned().unwrap_or_default() {
            let id = raw["id"].as_str().unwrap_or("").to_string();
            if id.is_empty() || id == DEFAULT_ID {
                return Err(format!("{}: a session entry has id {id:?}, which is reserved or empty", file.display()));
            }
            let name = raw["name"].as_str().unwrap_or("").to_string();
            let path = PathBuf::from(raw["path"].as_str().unwrap_or(""));
            if !safe_relative(&path) {
                return Err(format!(
                    "{}: session {id} names {path:?}, which is not a path inside the base",
                    file.display()
                ));
            }
            sessions.push(SessionEntry {
                id,
                name,
                path,
                created_ms: raw["created_ms"].as_u64().unwrap_or(0),
                archived: raw["archived"].as_bool().unwrap_or(false),
            });
        }
        // Injective, both ways: two ids may not name one path, and one id may not name two.
        for (index, one) in sessions.iter().enumerate() {
            for other in &sessions[index + 1..] {
                if one.id == other.id {
                    return Err(format!("{}: two sessions share the id {:?}", file.display(), one.id));
                }
                if one.path == other.path {
                    return Err(format!(
                        "{}: sessions {:?} and {:?} share the path {:?} — one state root, two sessions",
                        file.display(),
                        one.id,
                        other.id,
                        one.path
                    ));
                }
            }
        }
        Ok(Registry { home: home.to_path_buf(), sessions })
    }

    /// Write the registry atomically (tmp + fsync + rename), so a crash never leaves a half file.
    pub fn save(&self) -> Result<(), String> {
        let file = self.home.join(REGISTRY_FILE);
        std::fs::create_dir_all(&self.home).map_err(|e| format!("create {}: {e}", self.home.display()))?;
        let body = json!({
            "version": REGISTRY_VERSION,
            "sessions": self.sessions.iter().map(|session| json!({
                "id": session.id,
                "name": session.name,
                "path": session.path.to_string_lossy(),
                "created_ms": session.created_ms,
                "archived": session.archived,
            })).collect::<Vec<_>>(),
        });
        let tmp = self.home.join(format!("{REGISTRY_FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_string_pretty(&body).unwrap_or_default())
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        std::fs::File::open(&tmp)
            .and_then(|file| file.sync_all())
            .map_err(|e| format!("sync {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &file).map_err(|e| format!("publish {}: {e}", file.display()))?;
        Ok(())
    }

    pub fn find(&self, id: &str) -> Option<&SessionEntry> {
        self.sessions.iter().find(|session| session.id == id)
    }

    /// The directory a session id names: the base itself for the default, else the entry's directory.
    /// An archived session is named by the same error as an unknown one — it is not attachable.
    pub fn resolve(&self, id: &str) -> Result<PathBuf, String> {
        if id == DEFAULT_ID {
            return Ok(self.home.clone());
        }
        match self.find(id) {
            Some(entry) if !entry.archived => Ok(entry.dir(&self.home)),
            Some(_) => Err(format!("session {id} is archived; it is not attachable")),
            None => Err(format!("no session {id:?}; `teamagents sessions` lists them")),
        }
    }

    /// All rows for `sessions list`: the synthesized default first, then the registry in creation order.
    pub fn rows(&self) -> Vec<(SessionEntry, PathBuf)> {
        let mut rows = vec![(
            SessionEntry {
                id: DEFAULT_ID.to_string(),
                name: "default (this state root)".to_string(),
                path: PathBuf::new(),
                created_ms: 0,
                archived: false,
            },
            self.home.clone(),
        )];
        for entry in &self.sessions {
            rows.push((entry.clone(), entry.dir(&self.home)));
        }
        rows
    }

    /// Create a new named session directory and record it.
    pub fn new_session(&mut self, name: &str) -> Result<SessionEntry, String> {
        let mut id = String::new();
        for _ in 0..32 {
            let candidate = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
            if self.find(&candidate).is_none() && !self.home.join("sessions").join(&candidate).exists() {
                id = candidate;
                break;
            }
        }
        if id.is_empty() {
            return Err("could not find a free session id after 32 tries".into());
        }
        let path = PathBuf::from("sessions").join(&id);
        let dir = self.home.join(&path);
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let entry = SessionEntry {
            id: id.clone(),
            name: if name.trim().is_empty() { id.clone() } else { name.trim().to_string() },
            path,
            created_ms: now_ms(),
            archived: false,
        };
        self.sessions.push(entry.clone());
        self.save()?;
        Ok(entry)
    }

    /// Fork a session (D-365): snapshot its database, copy its artifacts, reset the copy's execution state, and
    /// register it. Refused while the source has a daemon (the snapshot must not race a writer), and the new
    /// directory is removed again if any step fails, so a failed fork never leaves a half session behind.
    pub fn fork(&mut self, from: &str, name: &str) -> Result<SessionEntry, String> {
        let source_dir = self.resolve(from)?;
        let source_db = source_dir.join("session.sqlite");
        if !source_db.exists() {
            return Err(format!("session {from} has no session.sqlite to fork"));
        }
        refuse_if_live(&source_dir, from)?;
        let mut id = String::new();
        for _ in 0..32 {
            let candidate = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
            if self.find(&candidate).is_none() && !self.home.join("sessions").join(&candidate).exists() {
                id = candidate;
                break;
            }
        }
        if id.is_empty() {
            return Err("could not find a free session id after 32 tries".into());
        }
        let path = PathBuf::from("sessions").join(&id);
        let dir = self.home.join(&path);
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let attempt = (|| -> Result<(), String> {
            teamagents_core::v2::store::fork_database(&source_db, &dir.join("session.sqlite"))?;
            copy_dir(&source_dir.join("artifacts"), &dir.join("artifacts"))?;
            let mut control = teamagents_core::v2::Control::open(&dir.join("session.sqlite"), "s-main", false)?;
            control.submit(
                teamagents_core::v2::Command {
                    command_id: format!("fork-reset-{}", uuid::Uuid::new_v4()),
                    method: "fork_reset".into(),
                    params: json!({
                        "keep_instance": "i-leader",
                        "source_root": source_dir.to_string_lossy(),
                        "target_root": dir.to_string_lossy(),
                    }),
                },
                teamagents_core::v2::Identity::User,
            )?;
            Ok(())
        })();
        if let Err(error) = attempt {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(error);
        }
        let entry = SessionEntry {
            id: id.clone(),
            name: if name.trim().is_empty() { id.clone() } else { name.trim().to_string() },
            path,
            created_ms: now_ms(),
            archived: false,
        };
        self.sessions.push(entry.clone());
        self.save()?;
        Ok(entry)
    }

    /// Move a named session's directory aside. Refused while a daemon holds it, and for the default.
    pub fn archive(&mut self, id: &str) -> Result<PathBuf, String> {
        let Some(index) = self.sessions.iter().position(|session| session.id == id) else {
            return Err(format!("no named session {id:?}; the default state root cannot be archived"));
        };
        let entry = self.sessions[index].clone();
        if entry.archived {
            return Err(format!("session {id} is already archived"));
        }
        let from = entry.dir(&self.home);
        refuse_if_live(&from, id)?;
        let to = self.home.join("archive").join(&entry.id);
        if to.exists() {
            return Err(format!("{} already exists; move it aside before archiving {id}", to.display()));
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        std::fs::rename(&from, &to).map_err(|e| format!("archive {}: {e}", from.display()))?;
        self.sessions[index].archived = true;
        self.sessions[index].path = PathBuf::from("archive").join(&entry.id);
        self.save()?;
        Ok(to)
    }

    /// Remove a named session's directory and its record. Refused while a daemon holds it, and for the default.
    pub fn delete(&mut self, id: &str) -> Result<PathBuf, String> {
        let Some(index) = self.sessions.iter().position(|session| session.id == id) else {
            return Err(format!("no named session {id:?}; the default state root cannot be deleted"));
        };
        let entry = self.sessions[index].clone();
        let dir = entry.dir(&self.home);
        refuse_if_live(&dir, id)?;
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("remove {}: {e}", dir.display()))?;
        }
        self.sessions.remove(index);
        self.save()?;
        Ok(dir)
    }
}

/// Copy a directory tree (immutable artifact blobs). A missing source is not an error: a session that never
/// published an artifact has no directory to copy.
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    if !from.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(to).map_err(|e| format!("create {}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))? {
        let entry = entry.map_err(|e| format!("read {}: {e}", from.display()))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            copy_dir(&source, &target)?;
        } else {
            std::fs::copy(&source, &target).map_err(|e| format!("copy {}: {e}", source.display()))?;
        }
    }
    Ok(())
}

/// Refuse a filesystem move or removal while a daemon is serving the session. The socket is the address (the
/// same rule `daemon --stop` uses), so no pid is recorded, guessed or reused.
pub fn refuse_if_live(dir: &Path, id: &str) -> Result<(), String> {
    if !is_live(dir) {
        return Ok(());
    }
    Err(format!(
        "session {id} still has a daemon serving {}; stop it first with `teamagents --state-root {} daemon --stop`",
        dir.display(),
        dir.display()
    ))
}

/// Whether a daemon is answering this session's socket. Connecting only reads the greeting.
pub fn is_live(dir: &Path) -> bool {
    let socket = dir.join("daemon.sock");
    socket.exists() && crate::v2::exec::Client::connect(&socket).is_ok()
}

/// Filesystem-only facts for `sessions list`: nothing here opens another session's database.
pub fn facts(dir: &Path) -> Json {
    let db = dir.join("session.sqlite");
    let bytes = std::fs::metadata(&db).map(|meta| meta.len()).unwrap_or(0);
    let modified_ms = std::fs::metadata(&db)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    json!({
        "has_database": db.exists(),
        "database_bytes": bytes,
        "modified_ms": modified_ms,
        "live": is_live(dir),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch base directory, removed by the test (the crate's convention for a file-backed unit test).
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("ta-sessions-{tag}-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_new_session_round_trips_and_is_injective() {
        let scratch = Scratch::new("roundtrip");
        let mut registry = Registry::load(scratch.path()).unwrap();
        let entry = registry.new_session("refactor").unwrap();
        assert_eq!(entry.name, "refactor");
        // reload from disk: the record is the only source
        let reloaded = Registry::load(scratch.path()).unwrap();
        assert_eq!(reloaded.find(&entry.id), Some(&entry));
        assert_eq!(reloaded.resolve(&entry.id).unwrap(), scratch.path().join("sessions").join(&entry.id));
        assert_eq!(reloaded.resolve(DEFAULT_ID).unwrap(), scratch.path());
        // a second session gets a different id and path
        let mut registry = reloaded;
        let second = registry.new_session("").unwrap();
        assert_ne!(second.id, entry.id);
        assert_ne!(second.path, entry.path);
        assert_eq!(second.name, second.id, "an unnamed session is named by its id");
    }

    #[test]
    fn a_shared_path_or_escaping_path_is_refused_at_load() {
        let scratch = Scratch::new("validate");
        std::fs::write(
            scratch.path().join(REGISTRY_FILE),
            r#"{"version":1,"sessions":[
                {"id":"a","name":"a","path":"sessions/a"},
                {"id":"b","name":"b","path":"sessions/a"}]}"#,
        )
        .unwrap();
        let error = Registry::load(scratch.path()).unwrap_err();
        assert!(error.contains("share the path"), "{error}");
        std::fs::write(
            scratch.path().join(REGISTRY_FILE),
            r#"{"version":1,"sessions":[{"id":"a","name":"a","path":"../outside"}]}"#,
        )
        .unwrap();
        let error = Registry::load(scratch.path()).unwrap_err();
        assert!(error.contains("not a path inside the base"), "{error}");
    }

    #[test]
    fn an_unknown_registry_version_is_refused_not_reinterpreted() {
        let scratch = Scratch::new("version");
        std::fs::write(scratch.path().join(REGISTRY_FILE), r#"{"version":99,"sessions":[]}"#).unwrap();
        let error = Registry::load(scratch.path()).unwrap_err();
        assert!(error.contains("version 99"), "{error}");
    }

    #[test]
    fn archiving_and_deleting_move_the_directory_and_refuse_the_default() {
        let scratch = Scratch::new("lifecycle");
        let mut registry = Registry::load(scratch.path()).unwrap();
        let entry = registry.new_session("work").unwrap();
        let dir = scratch.path().join(&entry.path);
        std::fs::write(dir.join("session.sqlite"), b"x").unwrap();
        // the default is never a named session
        assert!(registry.archive(DEFAULT_ID).is_err());
        assert!(registry.delete(DEFAULT_ID).is_err());
        let archived = registry.archive(&entry.id).unwrap();
        assert!(archived.exists());
        assert!(!dir.exists(), "the session directory was moved aside: {}", dir.display());
        let reloaded = Registry::load(scratch.path()).unwrap();
        assert!(reloaded.find(&entry.id).unwrap().archived);
        assert!(reloaded.resolve(&entry.id).is_err(), "an archived session is not attachable");
        // deleting is the physical removal of the record and the directory
        let mut registry = reloaded;
        let removed = registry.delete(&entry.id).unwrap();
        assert!(!removed.exists());
        assert!(Registry::load(scratch.path()).unwrap().find(&entry.id).is_none());
    }
}
