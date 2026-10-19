//! Governed self-update transaction (D-453).
//!
//! The update is a small, journaled transaction that runs outside the sandbox:
//! stage and verify a release, stop the daemon gracefully, switch the versioned
//! install pointer atomically, start the new daemon, health-check it and either
//! keep it or roll back. The append-only journal is hash-chained and protected by
//! one writer lock, so a crash at any step leaves exactly one active version.
//!
//! The agent-facing path is a request, not an execution: `request_self_update`
//! creates a persisted `UpdateRequest` and the control plane requires the user's
//! one-shot approval. Only the trusted runtime spawns the detached updater.

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{symlink, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

pub const DEFAULT_INSTALL_ROOT: &str = "~/.local/lib/teamagents";
pub const DEFAULT_BIN_DIR: &str = "~/.local/bin";
pub const DEFAULT_REPO: &str = "seek-hope/TeamAgents";
pub const PROTOCOL_VERSION: u64 = 1;

fn now() -> f64 {
    teamagents_core::models::now()
}

/// Journal timestamps are strings on purpose: a hash chain over floating-point
/// numbers is not stable across a serialize/parse round trip, and the chain must
/// be verifiable from the bytes it wrote.
fn now_string() -> String {
    format!("{:.6}", now())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn write_private(path: &Path, body: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        file.write_all(body).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        file.sync_all().map_err(|e| format!("cannot sync {}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot publish {}: {e}", path.display()))
}

fn atomic_symlink(target: &Path, link: &Path) -> Result<(), String> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let name = link.file_name().and_then(|n| n.to_str()).ok_or("install link has no name")?;
    let tmp = link.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    symlink(target, &tmp).map_err(|e| format!("cannot link {} -> {}: {e}", link.display(), target.display()))?;
    std::fs::rename(&tmp, link).map_err(|e| format!("cannot replace {}: {e}", link.display()))?;
    Ok(())
}

fn read_link_dir(path: &Path, root: &Path) -> Option<PathBuf> {
    let target = std::fs::read_link(path).ok()?;
    Some(if target.is_absolute() { target } else { root.join(target) })
}

/// A single writer lock for the installation's update journal. `flock` is
/// released by the kernel when the updater dies, so a crash never leaves the
/// install permanently locked (unlike a directory lock a person has to remove).
struct JournalLock {
    file: std::fs::File,
}

impl JournalLock {
    fn acquire(path: &Path) -> Result<JournalLock, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|e| format!("cannot open update lock {}: {e}", path.display()))?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return Err(format!("another updater holds {}", path.display()));
        }
        Ok(JournalLock { file })
    }
}

impl Drop for JournalLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateState {
    pub status: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub from_version: Option<String>,
    #[serde(default)]
    pub to_version: Option<String>,
    #[serde(default)]
    pub previous_dir: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub actor: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub follow_up: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub updated_at: String,
}

impl Default for UpdateState {
    fn default() -> Self {
        UpdateState {
            status: "idle".into(),
            target: None,
            digest: None,
            from_version: None,
            to_version: None,
            previous_dir: None,
            request_id: None,
            actor: "user".into(),
            reason: None,
            follow_up: None,
            error: None,
            updated_at: now_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalEntry {
    revision: u64,
    at: String,
    event: String,
    previous: String,
    state: UpdateState,
    hash: String,
}

#[derive(Serialize)]
struct JournalEntryPayload<'a> {
    revision: u64,
    at: &'a str,
    event: &'a str,
    previous: &'a str,
    state: &'a UpdateState,
}

fn entry_hash(payload: &JournalEntryPayload<'_>) -> Result<String, String> {
    let bytes = serde_json::to_vec(payload).map_err(|e| format!("journal hash: {e}"))?;
    Ok(sha256_bytes(&bytes))
}

pub struct UpdateJournal {
    path: PathBuf,
    _lock: Option<JournalLock>,
    state: UpdateState,
    last_hash: String,
    revision: u64,
    tail: String,
}

impl UpdateJournal {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read-only view for `update --status`; no lock, no write.
    pub fn open_read(path: &Path) -> Result<UpdateJournal, String> {
        UpdateJournal::load(path, None)
    }

    /// The writer used by `update --apply`; holds the install-wide lock.
    pub fn open_locked(path: &Path) -> Result<UpdateJournal, String> {
        let lock = JournalLock::acquire(&path.with_file_name("lock"))?;
        UpdateJournal::load(path, Some(lock))
    }

    fn load(path: &Path, lock: Option<JournalLock>) -> Result<UpdateJournal, String> {
        let mut journal = UpdateJournal {
            path: path.to_path_buf(),
            _lock: lock,
            state: UpdateState::default(),
            last_hash: String::new(),
            revision: 0,
            tail: String::new(),
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(journal),
            Err(error) => return Err(format!("cannot read update journal {}: {error}", path.display())),
        };
        let boundary = text.rfind('\n').map(|index| index + 1).unwrap_or(0);
        journal.tail = text[boundary..].to_string();
        for line in text[..boundary].split('\n') {
            if line.is_empty() {
                continue;
            }
            let entry: JournalEntry = serde_json::from_str(line).map_err(|e| {
                format!("update journal entry is not valid JSON: {e}; preserve the journal for recovery")
            })?;
            let payload = JournalEntryPayload {
                revision: entry.revision,
                at: &entry.at,
                event: &entry.event,
                previous: &entry.previous,
                state: &entry.state,
            };
            let expected = entry_hash(&payload)?;
            if entry.previous != journal.last_hash || entry.revision != journal.revision + 1 || entry.hash != expected {
                return Err(
                    "update journal integrity check failed; writes stopped. Preserve the journal for recovery".into()
                );
            }
            journal.state = entry.state;
            journal.last_hash = entry.hash;
            journal.revision = entry.revision;
        }
        Ok(journal)
    }

    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    pub fn entries(&self) -> Result<Vec<Json>, String> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(format!("cannot read update journal: {error}")),
        };
        let mut out = vec![];
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let entry: JournalEntry = serde_json::from_str(line).map_err(|e| format!("journal entry: {e}"))?;
            out.push(json!({"revision": entry.revision, "at": entry.at, "event": entry.event, "state": entry.state}));
        }
        Ok(out)
    }

    fn append(&mut self, event: &str, mut state: UpdateState) -> Result<(), String> {
        state.updated_at = now_string();
        let at = state.updated_at.clone();
        let payload = JournalEntryPayload {
            revision: self.revision + 1,
            at: &at,
            event,
            previous: &self.last_hash,
            state: &state,
        };
        let hash = entry_hash(&payload)?;
        let entry = json!({
            "revision": payload.revision,
            "at": payload.at,
            "event": event,
            "previous": payload.previous,
            "state": state,
            "hash": hash,
        });
        let line = format!("{entry}\n");
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        // A partial final line is preserved rather than silently discarded; the
        // file is truncated to the last complete entry before the append.
        if !self.tail.is_empty() {
            let preserved = self.path.with_extension(format!("interrupted-{}", std::process::id()));
            write_private(&preserved, self.tail.as_bytes())?;
            let complete = std::fs::read_to_string(&self.path).unwrap_or_default();
            let boundary = complete.rfind('\n').map(|index| index + 1).unwrap_or(0);
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&self.path)
                .map_err(|e| format!("cannot open journal for truncation: {e}"))?;
            file.set_len(boundary as u64).map_err(|e| format!("cannot truncate journal: {e}"))?;
            file.sync_all().map_err(|e| format!("cannot sync journal: {e}"))?;
            self.tail.clear();
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("cannot open update journal: {e}"))?;
        file.write_all(line.as_bytes()).map_err(|e| format!("cannot append update journal: {e}"))?;
        file.sync_all().map_err(|e| format!("cannot sync update journal: {e}"))?;
        self.revision = payload.revision;
        self.last_hash = hash;
        self.state = state;
        Ok(())
    }

    /// Settle an interrupted transaction before a new one starts. The pointer is
    /// the source of truth: a complete `current` target is active; anything else
    /// rolls back to the `previous` target.
    fn settle(&mut self, layout: &Layout) -> Result<(), String> {
        let status = self.state.status.clone();
        if matches!(status.as_str(), "idle" | "healthy" | "rolled_back" | "failed" | "cancelled") {
            return Ok(());
        }
        if matches!(status.as_str(), "staged" | "stopping" | "switching") {
            let mut state = self.state.clone();
            state.status = "failed".into();
            state.error = Some(format!("update was interrupted during {status}; the previous version stays active"));
            return self.append("interrupted", state);
        }
        if matches!(status.as_str(), "switched" | "starting" | "rolling_back") {
            if layout.current_dir().is_some_and(|dir| complete_version_dir(&dir)) {
                let mut state = self.state.clone();
                state.status = "healthy".into();
                state.error = Some(format!("update recovered after an interruption during {status}"));
                return self.append("recovered", state);
            }
            if let Some(previous) = layout.previous_dir().filter(|dir| complete_version_dir(dir)) {
                layout.switch_to(&previous)?;
                let mut state = self.state.clone();
                state.status = "rolled_back".into();
                state.error = Some(format!("update interrupted during {status}; rolled back"));
                return self.append("recovered_rollback", state);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    version: String,
    digest: String,
    installed_at: f64,
    files: Vec<String>,
}

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("manifest.json")
}

fn complete_version_dir(dir: &Path) -> bool {
    manifest_path(dir).is_file()
        && dir.join("teamagents").is_file()
        && dir.join("teamagents").metadata().map(|meta| meta.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

fn read_manifest(dir: &Path) -> Option<Manifest> {
    let text = std::fs::read_to_string(manifest_path(dir)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_manifest(dir: &Path, manifest: &Manifest) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(manifest).map_err(|e| format!("manifest: {e}"))?;
    write_private(&manifest_path(dir), &body)
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub root: PathBuf,
    pub bin_dir: PathBuf,
}

impl Layout {
    pub fn discover(install_root: Option<PathBuf>, bin_dir: Option<PathBuf>) -> Result<Layout, String> {
        let root = install_root
            .or_else(|| std::env::var_os("TEAMAGENTS_INSTALL_ROOT").map(PathBuf::from))
            .unwrap_or_else(|| crate::config::expand_home(DEFAULT_INSTALL_ROOT));
        let bin_dir = bin_dir
            .or_else(|| std::env::var_os("TEAMAGENTS_BIN_DIR").map(PathBuf::from))
            .unwrap_or_else(|| crate::config::expand_home(DEFAULT_BIN_DIR));
        if root.as_os_str().is_empty() || bin_dir.as_os_str().is_empty() {
            return Err("install root and bin directory must not be empty".into());
        }
        Ok(Layout { root, bin_dir })
    }

    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    pub fn updates_dir(&self) -> PathBuf {
        self.root.join("updates")
    }

    pub fn requests_dir(&self) -> PathBuf {
        self.updates_dir().join("requests")
    }

    pub fn journal_path(&self) -> PathBuf {
        self.updates_dir().join("journal.jsonl")
    }

    pub fn current_link(&self) -> PathBuf {
        self.root.join("current")
    }

    pub fn previous_link(&self) -> PathBuf {
        self.root.join("previous")
    }

    pub fn current_dir(&self) -> Option<PathBuf> {
        read_link_dir(&self.current_link(), &self.root)
    }

    pub fn previous_dir(&self) -> Option<PathBuf> {
        read_link_dir(&self.previous_link(), &self.root)
    }

    pub fn current_binary(&self) -> PathBuf {
        self.root.join("current").join("teamagents")
    }

    pub fn ensure(&self) -> Result<(), String> {
        for dir in [self.versions_dir(), self.requests_dir(), self.bin_dir.clone()] {
            std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        self.migrate_flat()
    }

    fn migrate_flat(&self) -> Result<(), String> {
        if self.current_link().exists() {
            return Ok(());
        }
        let binary = self.bin_dir.join("teamagents");
        let meta = match std::fs::symlink_metadata(&binary) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("cannot inspect {}: {error}", binary.display())),
        };
        if !meta.file_type().is_file() {
            return Ok(());
        }
        let version = binary_version(&binary).unwrap_or_else(|| "unknown".into());
        let digest = sha256_file(&binary)?;
        let dir = self.versions_dir().join(format!("{version}-{}-legacy", &digest[..12]));
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let mut files = vec![];
        for name in ["teamagents", "teamagents-tui"] {
            let source = self.bin_dir.join(name);
            if !source.is_file() {
                continue;
            }
            let target = dir.join(name);
            std::fs::copy(&source, &target).map_err(|e| format!("cannot copy {}: {e}", source.display()))?;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("cannot chmod {}: {e}", target.display()))?;
            files.push(name.to_string());
        }
        write_manifest(
            &dir,
            &Manifest { version: version.clone(), digest: digest.clone(), installed_at: now(), files: files.clone() },
        )?;
        self.switch_to(&dir)?;
        Ok(())
    }

    fn point_bin(&self, name: &str) -> Result<(), String> {
        let link = self.bin_dir.join(name);
        let target = self.root.join("current").join(name);
        if !self.root.join("current").join(name).exists() {
            return Ok(());
        }
        atomic_symlink(&target, &link)
    }

    pub fn switch_to(&self, dir: &Path) -> Result<(), String> {
        if !complete_version_dir(dir) {
            return Err(format!("{} is not a complete staged version", dir.display()));
        }
        let old = self.current_dir();
        if let Some(old) = old.filter(|old| old.as_path() != dir) {
            let relative = path_relative(&old, &self.root);
            atomic_symlink(&relative, &self.previous_link())?;
        }
        let relative = path_relative(dir, &self.root);
        atomic_symlink(&relative, &self.current_link())?;
        for name in ["teamagents", "teamagents-tui"] {
            if dir.join(name).is_file() {
                self.point_bin(name)?;
            }
        }
        Ok(())
    }

    pub fn rollback(&self) -> Result<PathBuf, String> {
        let current = self.current_dir().ok_or("there is no current version to roll back from")?;
        let previous = self.previous_dir().ok_or("there is no previous version to roll back to")?;
        if !complete_version_dir(&previous) {
            return Err(format!("previous version {} is incomplete", previous.display()));
        }
        self.switch_to(&previous)?;
        let current_relative = path_relative(&current, &self.root);
        atomic_symlink(&current_relative, &self.previous_link())?;
        Ok(previous)
    }

    pub fn current_version(&self) -> Option<String> {
        self.current_dir()
            .and_then(|dir| read_manifest(&dir).map(|manifest| manifest.version))
            .or_else(|| binary_version(&self.bin_dir.join("teamagents")))
    }

    pub fn prune(&self, keep: usize) {
        let Ok(entries) = std::fs::read_dir(self.versions_dir()) else {
            return;
        };
        let current = self.current_dir();
        let previous = self.previous_dir();
        let mut versions: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| {
                let path = entry.path();
                let modified = entry.metadata().ok()?.modified().ok()?;
                Some((modified, path))
            })
            .collect();
        versions.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        for (index, (_, path)) in versions.into_iter().enumerate() {
            let protected = current.as_deref() == Some(path.as_path()) || previous.as_deref() == Some(path.as_path());
            if !protected && index >= keep {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
}

fn path_relative(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root).map(PathBuf::from).unwrap_or_else(|_| path.to_path_buf())
}

fn binary_version(binary: &Path) -> Option<String> {
    let output = std::process::Command::new(binary).arg("version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace().last().map(|value| value.trim().to_string())
}

fn latest_release(repo: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build();
    let response = agent
        .get(&format!("https://api.github.com/repos/{repo}/releases/latest"))
        .call()
        .map_err(|e| format!("cannot read the latest release: {e}"))?;
    let value: Json = response.into_json().map_err(|e| format!("latest release is not JSON: {e}"))?;
    let tag = value["tag_name"].as_str().ok_or("latest release has no tag_name")?;
    Ok(tag.trim_start_matches('v').to_string())
}

fn download(url: &str, target: &Path) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(300)).build();
    let response = agent.get(url).call().map_err(|e| format!("download {url}: {e}"))?;
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(target).map_err(|e| format!("cannot create {}: {e}", target.display()))?;
    std::io::copy(&mut reader, &mut file).map_err(|e| format!("download {url}: {e}"))?;
    file.sync_all().map_err(|e| format!("sync {}: {e}", target.display()))
}

fn verify_sums(archive: &Path, sums: &Path) -> Result<(), String> {
    let name = archive.file_name().and_then(|n| n.to_str()).ok_or("archive has no file name")?;
    let text = std::fs::read_to_string(sums).map_err(|e| format!("cannot read {}: {e}", sums.display()))?;
    let mut matched = None;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else { continue };
        let Some(file) = parts.next() else { continue };
        let file = file.trim_start_matches('*');
        if file == name {
            if matched.is_some() {
                return Err(format!("SHA256SUMS lists {name} more than once"));
            }
            matched = Some(hash.to_string());
        }
    }
    let expected = matched.ok_or_else(|| format!("SHA256SUMS does not list {name}"))?;
    let actual = sha256_file(archive)?;
    if actual != expected {
        return Err(format!("SHA-256 verification failed for {name}"));
    }
    Ok(())
}

fn safe_entries(archive: &Path) -> Result<(String, String), String> {
    let output = std::process::Command::new("tar")
        .arg("-tzf")
        .arg(archive)
        .output()
        .map_err(|e| format!("cannot list the archive: {e}"))?;
    if !output.status.success() {
        return Err(format!("the archive cannot be listed: {}", String::from_utf8_lossy(&output.stderr)));
    }
    let mut cli = None;
    let mut tui = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let entry = line.trim_end_matches('/');
        if entry.is_empty() {
            continue;
        }
        let path = Path::new(entry);
        if path.is_absolute()
            || path.components().any(|component| matches!(component, Component::ParentDir | Component::RootDir))
        {
            return Err(format!("the archive contains an unsafe path: {entry}"));
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        match name {
            "teamagents" => cli = Some(entry.to_string()),
            "teamagents-tui" => tui = Some(entry.to_string()),
            _ => {}
        }
    }
    let cli = cli.ok_or("the archive has no teamagents binary")?;
    let tui = tui.ok_or("the archive has no teamagents-tui binary")?;
    Ok((cli, tui))
}

fn extract(archive: &Path, dir: &Path) -> Result<(), String> {
    let (cli, tui) = safe_entries(archive)?;
    let output = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .arg(&cli)
        .arg(&tui)
        .output()
        .map_err(|e| format!("cannot extract the archive: {e}"))?;
    if !output.status.success() {
        return Err(format!("the archive cannot be extracted: {}", String::from_utf8_lossy(&output.stderr)));
    }
    for entry in [&cli, &tui] {
        let source = dir.join(entry);
        let name = source.file_name().ok_or("archive entry has no name")?.to_owned();
        let target = dir.join(&name);
        let meta = std::fs::symlink_metadata(&source)
            .map_err(|e| format!("extracted {} cannot be inspected: {e}", source.display()))?;
        if !meta.file_type().is_file() {
            return Err(format!("archive entry {} is not a regular file", entry));
        }
        if source != target {
            std::fs::rename(&source, &target).map_err(|e| format!("cannot stage {}: {e}", name.to_string_lossy()))?;
        }
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("cannot chmod {}: {e}", target.display()))?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Staged {
    pub version: String,
    pub digest: String,
    pub dir: PathBuf,
}

pub fn stage(
    layout: &Layout,
    archive: Option<&Path>,
    target: Option<&str>,
    state_root: Option<&Path>,
    repo: &str,
) -> Result<Staged, String> {
    layout.ensure()?;
    let temp = layout.updates_dir().join(format!("staging-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp).map_err(|e| format!("cannot create {}: {e}", temp.display()))?;
    let cleanup = |temp: &Path| {
        let _ = std::fs::remove_dir_all(temp);
    };
    let archive_path = match archive {
        Some(path) => {
            if !path.is_file() {
                cleanup(&temp);
                return Err(format!("archive not found: {}", path.display()));
            }
            path.to_path_buf()
        }
        None => {
            let version = match target {
                Some("latest") | None => latest_release(repo)?,
                Some(version) => version.trim_start_matches('v').to_string(),
            };
            let name = format!("teamagents-{version}-x86_64-unknown-linux-musl.tar.gz");
            let base = format!("https://github.com/{repo}/releases/download/v{version}");
            let archive_path = temp.join(&name);
            let sums_path = temp.join("SHA256SUMS");
            if let Err(error) = download(&format!("{base}/{name}"), &archive_path) {
                cleanup(&temp);
                return Err(error);
            }
            if let Err(error) = download(&format!("{base}/SHA256SUMS"), &sums_path) {
                cleanup(&temp);
                return Err(error);
            }
            archive_path
        }
    };
    let sums = archive_path.with_file_name("SHA256SUMS");
    if !sums.is_file() {
        cleanup(&temp);
        return Err(format!(
            "SHA256SUMS is missing next to {}; refusing an unverified archive",
            archive_path.display()
        ));
    }
    if let Err(error) = verify_sums(&archive_path, &sums) {
        cleanup(&temp);
        return Err(error);
    }
    if let Err(error) = extract(&archive_path, &temp) {
        cleanup(&temp);
        return Err(error);
    }
    let binary = temp.join("teamagents");
    let version = match target {
        Some(version) if version != "latest" => version.trim_start_matches('v').to_string(),
        _ => binary_version(&binary).unwrap_or_default(),
    };
    if version.is_empty() {
        cleanup(&temp);
        return Err("the staged binary did not report a version".into());
    }
    let reported = binary_version(&binary).unwrap_or_default();
    if reported != version {
        cleanup(&temp);
        return Err(format!("staged binary reports {reported:?}, expected {version:?}"));
    }
    if let Some(root) = state_root {
        let output = std::process::Command::new(&binary)
            .arg("update")
            .arg("--health")
            .arg("--state-root")
            .arg(root)
            .arg("--json")
            .output()
            .map_err(|e| format!("cannot run the staged health check: {e}"))?;
        if !output.status.success() {
            cleanup(&temp);
            return Err(format!(
                "the staged binary refused this state root: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    let digest = sha256_file(&archive_path)?;
    let dir = layout.versions_dir().join(format!("{version}-{}", &digest[..12.min(digest.len())]));
    if dir.exists() {
        cleanup(&temp);
        if read_manifest(&dir).is_some() {
            return Ok(Staged { version, digest, dir });
        }
        return Err(format!("{} exists without a manifest", dir.display()));
    }
    std::fs::rename(&temp, &dir).map_err(|e| format!("cannot publish {}: {e}", dir.display()))?;
    write_manifest(
        &dir,
        &Manifest {
            version: version.clone(),
            digest: digest.clone(),
            installed_at: now(),
            files: vec!["teamagents".into(), "teamagents-tui".into()],
        },
    )?;
    Ok(Staged { version, digest, dir })
}

pub fn health_json(state_root: &Path) -> Result<Json, String> {
    let db = state_root.join("session.sqlite");
    if !db.exists() {
        return Ok(json!({"ok": true, "version": crate::VERSION, "state_root": state_root, "schema": null}));
    }
    let connection = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("cannot open {}: {e}", db.display()))?;
    let value = |key: &str| -> Result<Option<String>, String> {
        connection
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| row.get::<_, String>(0))
            .optional()
            .map_err(|e| format!("cannot read meta.{key}: {e}"))
    };
    let format = value("format_id")?;
    let schema = value("schema_version")?.and_then(|value| value.parse::<i64>().ok());
    if let Some(format) = &format {
        if format != teamagents_core::v2::models::V2_FORMAT_ID {
            return Err(format!(
                "state root {} is format {format:?}, not {}; refusing to update over a foreign database",
                state_root.display(),
                teamagents_core::v2::models::V2_FORMAT_ID
            ));
        }
    }
    if let Some(schema) = schema {
        if schema != teamagents_core::v2::models::V2_SCHEMA_VERSION {
            return Err(format!(
                "state root {} is schema {schema}, this binary requires {}; migrate explicitly before updating",
                state_root.display(),
                teamagents_core::v2::models::V2_SCHEMA_VERSION
            ));
        }
    }
    Ok(json!({"ok": true, "version": crate::VERSION, "state_root": state_root,
              "format": format, "schema": schema}))
}

fn daemon_running(state_root: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(state_root.join("daemon.sock")).is_ok()
}

fn daemon_check(socket: &Path) -> Result<Json, String> {
    let stream = std::os::unix::net::UnixStream::connect(socket)
        .map_err(|e| format!("cannot connect to {}: {e}", socket.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| format!("socket clone: {e}"))?);
    let mut writer = stream;
    let mut greeting = String::new();
    reader.read_line(&mut greeting).map_err(|e| format!("cannot read the greeting: {e}"))?;
    let greeting: Json = serde_json::from_str(greeting.trim()).map_err(|e| format!("bad greeting: {e}"))?;
    if greeting["protocol_version"].as_u64() != Some(PROTOCOL_VERSION) {
        return Err(format!(
            "the daemon speaks protocol {:?}, this updater speaks {PROTOCOL_VERSION}",
            greeting["protocol_version"]
        ));
    }
    let request = json!({"protocol_version": PROTOCOL_VERSION, "request_id": "update-health",
                         "method": "checkpoint", "params": {}});
    writer
        .write_all(format!("{request}\n").as_bytes())
        .map_err(|e| format!("cannot ask the daemon for a checkpoint: {e}"))?;
    writer.flush().ok();
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| format!("cannot read the checkpoint: {e}"))?;
    let response: Json = serde_json::from_str(line.trim()).map_err(|e| format!("bad checkpoint: {e}"))?;
    if response.get("error").is_some_and(|error| !error.is_null()) {
        return Err(format!("the daemon refused the health check: {}", response["error"]));
    }
    Ok(response)
}

fn daemon_boot(state_root: &Path) -> Json {
    std::fs::read_to_string(state_root.join("daemon.boot.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}))
}

/// The daemon records the arguments a restart must preserve. It contains no
/// credentials: only the workspace, model key, permission mode and `-c`
/// overrides the user already put on the command line.
pub fn write_boot_record(
    state_root: &Path,
    cwd: &Path,
    model: &str,
    full_auto: bool,
    overrides: &[String],
    permissions: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(state_root).map_err(|e| format!("cannot create {}: {e}", state_root.display()))?;
    let body = json!({
        "cwd": cwd,
        "model": model,
        "full_auto": full_auto,
        "overrides": overrides,
        "permissions": permissions,
        "version": crate::VERSION,
        "written_at": now(),
    });
    let bytes = serde_json::to_vec_pretty(&body).map_err(|e| format!("boot record: {e}"))?;
    write_private(&state_root.join("daemon.boot.json"), &bytes)
}

/// The model-visible request tool. It states the approval boundary in its own
/// description: the model asks, the user decides, the runtime performs.
pub fn request_tool_schema() -> Json {
    json!({
        "type": "function",
        "function": {
            "name": "request_self_update",
            "description": "Request a governed update of TeamAgents itself when the current version blocks the task. The user must approve the exact target, reason and follow-up; the runtime then stages, verifies, restarts and continues with the follow-up. This tool never executes the update and cannot approve itself.",
            "parameters": {
                "type": "object",
                "properties": {
                    "target": {"type": "string", "description": "Stable release version, for example 0.3.0, or latest."},
                    "reason": {"type": "string", "description": "Why the current version blocks the task."},
                    "follow_up": {"type": "string", "description": "What the agent should continue with after the restart."}
                },
                "required": ["target", "reason", "follow_up"],
                "additionalProperties": false
            }
        }
    })
}

pub fn validate_request(target: &str, reason: &str, follow_up: &str) -> Result<(), String> {
    let target = target.trim();
    let version_ok = target == "latest"
        || (target.split('.').count() == 3
            && target
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '.' || character == '-'));
    if !version_ok {
        return Err(format!("request_self_update.target must be a version like 0.3.0 or latest, got {target:?}"));
    }
    if reason.trim().is_empty() {
        return Err("request_self_update.reason must say why the update is needed".into());
    }
    if follow_up.trim().is_empty() {
        return Err("request_self_update.follow_up must say what to continue after the restart".into());
    }
    Ok(())
}

fn start_daemon(layout: &Layout, state_root: &Path) -> Result<(), String> {
    let binary = layout.current_binary();
    if !binary.is_file() {
        return Err(format!("the new binary is missing: {}", binary.display()));
    }
    let boot = daemon_boot(state_root);
    let mut command = if crate::tools::which("setsid").is_some() {
        let mut command = std::process::Command::new("setsid");
        command.arg(&binary);
        command
    } else {
        std::process::Command::new(&binary)
    };
    command.arg("daemon").arg("--state-root").arg(state_root);
    if let Some(cwd) = boot["cwd"].as_str() {
        if Path::new(cwd).is_dir() {
            command.arg("--cwd").arg(cwd);
        }
    }
    if let Some(model) = boot["model"].as_str() {
        command.arg("--model").arg(model);
    }
    if boot["full_auto"].as_bool().unwrap_or(false) {
        command.arg("--full-auto");
    }
    if let Some(overrides) = boot["overrides"].as_array() {
        for value in overrides.iter().filter_map(|value| value.as_str()) {
            command.arg("-c").arg(value);
        }
    }
    std::fs::create_dir_all(state_root).map_err(|e| format!("cannot create {}: {e}", state_root.display()))?;
    let log_path = state_root.join("daemon.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("cannot open {}: {e}", log_path.display()))?;
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(log);
    let mut child = command.spawn().map_err(|e| format!("cannot start the new daemon: {e}"))?;
    let socket = state_root.join("daemon.sock");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if socket.exists() {
            if let Err(error) = daemon_check(&socket) {
                return Err(format!("the new daemon is not healthy: {error}"));
            }
            return Ok(());
        }
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                let text = std::fs::read_to_string(&log_path).unwrap_or_default();
                let tail: String =
                    text.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                return Err(format!("the new daemon exited while starting ({status}): {tail}"));
            }
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!("the new daemon did not open {}", socket.display()))
}

fn rollback_install(layout: &Layout, state_root: &Path, was_running: bool) -> Result<(), String> {
    let previous = layout.rollback()?;
    if was_running {
        let _ = crate::cli::daemon_stop(Some(state_root.to_path_buf()));
        start_daemon(layout, state_root)?;
    }
    let _ = previous;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRequest {
    pub id: String,
    pub target: String,
    pub reason: String,
    pub follow_up: String,
    pub state_root: PathBuf,
    pub session_id: String,
    pub instance_id: String,
    pub leader_id: String,
    pub operation_id: String,
    pub daemon_pid: u32,
    pub created_at: f64,
}

pub fn request_path(layout: &Layout, id: &str) -> PathBuf {
    layout.requests_dir().join(format!("{id}.json"))
}

pub fn write_request(layout: &Layout, request: &UpdateRequest) -> Result<PathBuf, String> {
    layout.ensure()?;
    let path = request_path(layout, &request.id);
    let body = serde_json::to_vec_pretty(request).map_err(|e| format!("update request: {e}"))?;
    write_private(&path, &body)?;
    Ok(path)
}

pub fn read_request(path: &Path) -> Result<UpdateRequest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

pub fn spawn_updater(layout: &Layout, request_path: &Path) -> Result<u32, String> {
    // In production this is the daemon's own binary (the old version); a test
    // may name a script so the approval path can be exercised without replacing
    // the test runner itself. The updater is spawned by the trusted runtime,
    // never by the sandboxed shell.
    let exe = std::env::var_os("TEAMAGENTS_UPDATER_BIN")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .ok_or_else(|| "current executable is unavailable".to_string())?;
    let mut command = if crate::tools::which("setsid").is_some() {
        let mut command = std::process::Command::new("setsid");
        command.arg(&exe);
        command
    } else {
        std::process::Command::new(&exe)
    };
    command
        .arg("update")
        .arg("--apply")
        .arg("--request")
        .arg(request_path)
        .arg("--install-root")
        .arg(&layout.root)
        .arg("--bin-dir")
        .arg(&layout.bin_dir);
    std::fs::create_dir_all(layout.updates_dir())
        .map_err(|e| format!("cannot create {}: {e}", layout.updates_dir().display()))?;
    let log_path = layout.updates_dir().join("updater.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("cannot open {}: {e}", log_path.display()))?;
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(log);
    let child = command.spawn().map_err(|e| format!("cannot start the updater: {e}"))?;
    Ok(child.id())
}

fn wait_for_turn(request: &UpdateRequest) -> Result<(), String> {
    let db = request.state_root.join("session.sqlite");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(connection) = rusqlite::Connection::open_with_flags(
            &db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        ) {
            let status: Option<String> = connection
                .query_row("SELECT status FROM operations WHERE operation_id = ?1", [&request.operation_id], |row| {
                    row.get(0)
                })
                .ok();
            let phase: Option<String> = connection
                .query_row("SELECT phase FROM instances WHERE id = ?1", [&request.instance_id], |row| row.get(0))
                .ok();
            if status
                .as_deref()
                .is_some_and(|status| matches!(status, "SUCCEEDED" | "FAILED" | "CANCELLED" | "OUTCOME_UNKNOWN"))
                && phase.as_deref() == Some("READY")
            {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

fn submit_follow_up(layout: &Layout, request: &UpdateRequest) -> Result<(), String> {
    let binary = layout.current_binary();
    let status = std::process::Command::new(&binary)
        .arg("instances")
        .arg("send")
        .arg("--id")
        .arg(&request.leader_id)
        .arg("--prompt")
        .arg(&request.follow_up)
        .arg("--state-root")
        .arg(&request.state_root)
        .status()
        .map_err(|e| format!("cannot submit the follow-up: {e}"))?;
    if !status.success() {
        return Err(format!("the new daemon refused the follow-up (exit {status})"));
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    pub state_root: PathBuf,
    pub target: Option<String>,
    pub archive: Option<PathBuf>,
    pub restart: bool,
    pub repo: String,
    pub request: Option<UpdateRequest>,
    pub crash_at: Option<String>,
}

fn crash_point(crash_at: &Option<String>, point: &str) -> Result<(), String> {
    if crash_at.as_deref() == Some(point) {
        return Err(format!("simulated crash at {point}"));
    }
    Ok(())
}

pub fn apply(layout: &Layout, options: &ApplyOptions) -> Result<Json, String> {
    layout.ensure()?;
    let mut journal = UpdateJournal::open_locked(&layout.journal_path())?;
    journal.settle(layout)?;
    let active = journal.state().status.clone();
    if !matches!(active.as_str(), "idle" | "healthy" | "rolled_back" | "failed" | "cancelled") {
        return Err(format!("an update is already {active}; run `teamagents update --status`"));
    }
    let staged = stage(
        layout,
        options.archive.as_deref(),
        options.target.as_deref(),
        Some(&options.state_root),
        if options.repo.is_empty() { DEFAULT_REPO } else { &options.repo },
    )?;
    let from_version = layout.current_version();
    let mut state = UpdateState {
        status: "staged".into(),
        target: Some(options.target.clone().unwrap_or_else(|| staged.version.clone())),
        digest: Some(staged.digest.clone()),
        from_version: from_version.clone(),
        to_version: Some(staged.version.clone()),
        request_id: options.request.as_ref().map(|request| request.id.clone()),
        actor: if options.request.is_some() { "agent+user".into() } else { "user".into() },
        reason: options.request.as_ref().map(|request| request.reason.clone()),
        follow_up: options.request.as_ref().map(|request| request.follow_up.clone()),
        ..UpdateState::default()
    };
    journal.append("staged", state.clone())?;
    crash_point(&options.crash_at, "staged")?;
    let was_running = daemon_running(&options.state_root);
    if options.restart && was_running {
        state.status = "stopping".into();
        journal.append("stopping", state.clone())?;
        crash_point(&options.crash_at, "stopping")?;
        if crate::cli::daemon_stop(Some(options.state_root.clone())) != 0 {
            state.status = "failed".into();
            state.error = Some("the daemon refused to stop; the update was not applied".into());
            journal.append("stop_refused", state)?;
            return Err("the daemon refused to stop; the update was not applied".into());
        }
    }
    state.status = "switching".into();
    journal.append("switching", state.clone())?;
    crash_point(&options.crash_at, "switching")?;
    layout.switch_to(&staged.dir)?;
    state.status = "switched".into();
    journal.append("switched", state.clone())?;
    crash_point(&options.crash_at, "switched")?;
    let health = health_json(&options.state_root);
    if let Err(error) = health {
        state.status = "rolling_back".into();
        state.error = Some(error.clone());
        journal.append("rolling_back", state.clone())?;
        rollback_install(layout, &options.state_root, was_running)?;
        state.status = "rolled_back".into();
        journal.append("rolled_back", state)?;
        return Err(error);
    }
    if was_running {
        state.status = "starting".into();
        journal.append("starting", state.clone())?;
        crash_point(&options.crash_at, "starting")?;
        if let Err(error) = start_daemon(layout, &options.state_root) {
            state.status = "rolling_back".into();
            state.error = Some(error.clone());
            journal.append("rolling_back", state.clone())?;
            rollback_install(layout, &options.state_root, was_running)?;
            state.status = "rolled_back".into();
            journal.append("rolled_back", state)?;
            return Err(error);
        }
    }
    state.status = "healthy".into();
    state.error = None;
    journal.append("healthy", state.clone())?;
    layout.prune(3);
    if let Some(request) = &options.request {
        if let Err(error) = submit_follow_up(layout, request) {
            state.status = "healthy".into();
            state.error = Some(format!("updated, but the follow-up failed: {error}"));
            journal.append("follow_up_failed", state.clone())?;
            return Err(format!("updated, but the follow-up failed: {error}"));
        }
    }
    Ok(json!({"ok": true, "version": staged.version, "digest": staged.digest,
              "from": from_version, "restarted": was_running && options.restart,
              "state": state}))
}

pub fn rollback(layout: &Layout, state_root: &Path, restart: bool) -> Result<Json, String> {
    layout.ensure()?;
    let mut journal = UpdateJournal::open_locked(&layout.journal_path())?;
    journal.settle(layout)?;
    let was_running = daemon_running(state_root);
    if restart && was_running && crate::cli::daemon_stop(Some(state_root.to_path_buf())) != 0 {
        return Err("the daemon refused to stop; the rollback was not applied".into());
    }
    let target = layout.rollback()?;
    health_json(state_root)?;
    if was_running && restart {
        start_daemon(layout, state_root)?;
    }
    let mut state = journal.state().clone();
    state.status = "rolled_back".into();
    state.error = None;
    state.updated_at = now_string();
    journal.append("rolled_back", state.clone())?;
    Ok(json!({"ok": true, "version": read_manifest(&target).map(|manifest| manifest.version),
              "dir": target, "restarted": was_running && restart}))
}

pub fn status_json(layout: &Layout, state_root: Option<&Path>) -> Result<Json, String> {
    let journal = UpdateJournal::open_read(&layout.journal_path())?;
    let current = layout.current_dir();
    let previous = layout.previous_dir();
    let current_info = current.as_ref().and_then(|dir| {
        read_manifest(dir).map(|manifest| json!({"version": manifest.version, "digest": manifest.digest, "dir": dir}))
    });
    let previous_info = previous.as_ref().and_then(|dir| {
        read_manifest(dir).map(|manifest| json!({"version": manifest.version, "digest": manifest.digest, "dir": dir}))
    });
    Ok(json!({
        "install_root": layout.root,
        "bin_dir": layout.bin_dir,
        "current": current_info,
        "previous": previous_info,
        "daemon_running": state_root.is_some_and(daemon_running),
        "state": journal.state(),
        "entries": journal.entries()?,
    }))
}

pub fn doctor_row(layout: &Layout) -> (bool, String) {
    let journal = match UpdateJournal::open_read(&layout.journal_path()) {
        Ok(journal) => journal,
        Err(error) => return (false, error),
    };
    let current = layout
        .current_dir()
        .and_then(|dir| read_manifest(&dir))
        .map(|manifest| manifest.version)
        .unwrap_or_else(|| "not installed in the versioned layout".into());
    let state = &journal.state();
    (
        !matches!(state.status.as_str(), "failed" | "rolling_back"),
        format!("v{current} under {}; last update {}", layout.root.display(), state.status),
    )
}

pub fn apply_request(layout: &Layout, path: &Path) -> Result<Json, String> {
    let request = read_request(path)?;
    // The operation is already committed by the time the updater starts. Wait
    // for the turn to reach a safe boundary so the receipt is not cut off; a
    // timeout is not a failure to stop, because the daemon stop below is the
    // authoritative lever.
    wait_for_turn(&request)?;
    let options = ApplyOptions {
        state_root: request.state_root.clone(),
        target: Some(request.target.clone()),
        archive: None,
        restart: true,
        repo: DEFAULT_REPO.to_string(),
        request: Some(request),
        crash_at: std::env::var("TEAMAGENTS_UPDATE_CRASH_AT").ok(),
    };
    let report = apply(layout, &options)?;
    let _ = std::fs::remove_file(path);
    Ok(report)
}

pub fn run_cli(argv: &[String]) -> i32 {
    match run_cli_inner(argv) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("update: {error}");
            1
        }
    }
}

fn run_cli_inner(argv: &[String]) -> Result<i32, String> {
    let mut install_root = None;
    let mut bin_dir = None;
    let mut state_root = None;
    let mut json = false;
    let mut check = false;
    let mut rollback_flag = false;
    let mut status = false;
    let mut health = false;
    let mut apply_flag = false;
    let mut target = None;
    let mut archive = None;
    let mut request = None;
    let mut restart = true;
    let mut repo = DEFAULT_REPO.to_string();
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        let value = |index: usize| -> Result<String, String> {
            argv.get(index + 1).cloned().ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{}", cli_help());
                return Ok(0);
            }
            "--install-root" => {
                install_root = Some(PathBuf::from(value(index)?));
                index += 2;
            }
            "--bin-dir" => {
                bin_dir = Some(PathBuf::from(value(index)?));
                index += 2;
            }
            "--state-root" => {
                state_root = Some(PathBuf::from(value(index)?));
                index += 2;
            }
            "--to" => {
                target = Some(value(index)?);
                index += 2;
            }
            "--archive" => {
                archive = Some(PathBuf::from(value(index)?));
                index += 2;
            }
            "--request" => {
                request = Some(PathBuf::from(value(index)?));
                index += 2;
            }
            "--repo" => {
                repo = value(index)?;
                index += 2;
            }
            "--json" => {
                json = true;
                index += 1;
            }
            "--check" => {
                check = true;
                index += 1;
            }
            "--rollback" => {
                rollback_flag = true;
                index += 1;
            }
            "--status" => {
                status = true;
                index += 1;
            }
            "--health" => {
                health = true;
                index += 1;
            }
            "--apply" => {
                apply_flag = true;
                index += 1;
            }
            "--no-restart" => {
                restart = false;
                index += 1;
            }
            other => return Err(format!("unknown update argument {other:?}; use `teamagents update --help`")),
        }
    }
    let layout = Layout::discover(install_root, bin_dir)?;
    if apply_flag && request.is_none() {
        return Err("update --apply needs --request PATH".into());
    }
    if health {
        let root = state_root.ok_or("update --health needs --state-root")?;
        let report = health_json(&root)?;
        if json {
            println!("{report}");
        } else {
            println!("update health ok: {} (schema {:?})", report["version"], report["schema"]);
        }
        return Ok(0);
    }
    if check {
        let current = layout.current_version().unwrap_or_else(|| "unknown".into());
        let latest = latest_release(&repo)?;
        let report = json!({"current": current, "latest": latest, "update_available": latest != current});
        if json {
            println!("{report}");
        } else {
            println!("current {current}; latest {latest}");
            if latest == current {
                println!("up to date");
            } else {
                println!("update available: teamagents update --to {latest}");
            }
        }
        return Ok(0);
    }
    if status || (!apply_flag && target.is_none() && archive.is_none() && !rollback_flag && request.is_none()) {
        let report = status_json(&layout, state_root.as_deref())?;
        if json {
            println!("{report}");
        } else {
            println!("install: {}", layout.root.display());
            println!("current: {}", report["current"]["version"].as_str().unwrap_or("none"));
            println!("previous: {}", report["previous"]["version"].as_str().unwrap_or("none"));
            println!("last update: {}", report["state"]["status"].as_str().unwrap_or("idle"));
        }
        return Ok(0);
    }
    if rollback_flag {
        let root = state_root.unwrap_or_else(crate::v2_root);
        let report = rollback(&layout, &root, restart)?;
        if json {
            println!("{report}");
        } else {
            println!("rolled back to {}", report["version"].as_str().unwrap_or("unknown"));
        }
        return Ok(0);
    }
    if let Some(request_path) = request {
        let report = apply_request(&layout, &request_path)?;
        if json {
            println!("{report}");
        } else {
            println!("updated to {}", report["version"].as_str().unwrap_or("unknown"));
        }
        return Ok(0);
    }
    let root = state_root.unwrap_or_else(crate::v2_root);
    let options = ApplyOptions {
        state_root: root,
        target,
        archive,
        restart,
        repo,
        request: None,
        crash_at: std::env::var("TEAMAGENTS_UPDATE_CRASH_AT").ok(),
    };
    let report = apply(&layout, &options)?;
    if json {
        println!("{report}");
    } else {
        println!("updated to {}", report["version"].as_str().unwrap_or("unknown"));
    }
    Ok(0)
}

fn cli_help() -> &'static str {
    "teamagents update: governed self-update\n\
     \n\
     usage:\n\
       teamagents update --status [--json]\n\
       teamagents update --check [--json]\n\
       teamagents update --to VERSION [--no-restart] [--json]\n\
       teamagents update --archive PATH [--json]\n\
       teamagents update --rollback [--json]\n\
       teamagents update --health --state-root DIR [--json]\n\
     \n\
     options:\n\
       --install-root DIR  versioned install root (default ~/.local/lib/teamagents)\n\
       --bin-dir DIR       directory holding the teamagents symlinks (default ~/.local/bin)\n\
       --state-root DIR    session whose daemon is stopped/restarted\n\
       --no-restart        switch the binaries without restarting a running daemon\n\
       --json              machine-readable output\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Temp {
            let path = std::env::temp_dir().join(format!("ta-update-{tag}-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Temp(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn script(path: &Path, version: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n  version) echo \"teamagents {version}\" ;;\n  update) echo '{{\"ok\":true}}' ;;\n  instances) echo accepted ;;\n  *) exit 0 ;;\nesac\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn archive(root: &Path, version: &str) -> PathBuf {
        let package = format!("teamagents-{version}-x86_64-unknown-linux-musl");
        // Each release fixture gets its own directory: SHA256SUMS is a sibling of
        // the archive, so two versions in one directory would overwrite each other.
        let release = root.join(format!("release-{version}"));
        let dir = release.join(&package);
        std::fs::create_dir_all(&dir).unwrap();
        script(&dir.join("teamagents"), version);
        script(&dir.join("teamagents-tui"), version);
        let archive = release.join(format!("{package}.tar.gz"));
        let status = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&release)
            .arg(&package)
            .status()
            .unwrap();
        assert!(status.success(), "tar could not create the fixture archive");
        let digest = sha256_file(&archive).unwrap();
        std::fs::write(
            release.join("SHA256SUMS"),
            format!("{digest}  {}\n", archive.file_name().unwrap().to_string_lossy()),
        )
        .unwrap();
        archive
    }

    fn layout(root: &Path) -> Layout {
        Layout { root: root.join("install"), bin_dir: root.join("bin") }
    }

    fn options(state: PathBuf, target: &str, archive: PathBuf, crash_at: Option<&str>) -> ApplyOptions {
        ApplyOptions {
            state_root: state,
            target: Some(target.into()),
            archive: Some(archive),
            restart: false,
            repo: DEFAULT_REPO.into(),
            request: None,
            crash_at: crash_at.map(str::to_string),
        }
    }

    #[test]
    fn journal_hash_chain_and_middle_corruption() {
        let temp = Temp::new("journal");
        let path = temp.path().join("journal.jsonl");
        let mut journal = UpdateJournal::open_locked(&path).unwrap();
        journal.append("staged", UpdateState { status: "staged".into(), ..Default::default() }).unwrap();
        journal.append("healthy", UpdateState { status: "healthy".into(), ..Default::default() }).unwrap();
        drop(journal);
        let read = UpdateJournal::open_read(&path).unwrap();
        assert_eq!(read.state().status, "healthy");
        let text = std::fs::read_to_string(&path).unwrap();
        let corrupted = text.replacen("\"healthy\"", "\"failed\"", 1);
        std::fs::write(&path, corrupted).unwrap();
        assert!(UpdateJournal::open_read(&path).is_err(), "a changed entry must break the chain");
    }

    #[test]
    fn the_writer_lock_is_exclusive() {
        let temp = Temp::new("lock");
        let path = temp.path().join("journal.jsonl");
        let first = UpdateJournal::open_locked(&path).unwrap();
        assert!(UpdateJournal::open_locked(&path).is_err(), "a second updater must be refused");
        drop(first);
        assert!(UpdateJournal::open_locked(&path).is_ok());
    }

    #[test]
    fn a_flat_install_migrates_to_the_versioned_layout() {
        let temp = Temp::new("migrate");
        let layout = layout(temp.path());
        std::fs::create_dir_all(&layout.bin_dir).unwrap();
        script(&layout.bin_dir.join("teamagents"), "0.2.0");
        script(&layout.bin_dir.join("teamagents-tui"), "0.2.0");
        layout.ensure().unwrap();
        assert!(layout.current_link().exists());
        assert_eq!(layout.current_version().as_deref(), Some("0.2.0"));
        assert!(
            std::fs::symlink_metadata(layout.bin_dir.join("teamagents")).unwrap().file_type().is_symlink(),
            "the flat binary is replaced by a symlink into the versioned layout"
        );
    }

    #[test]
    fn stage_switch_and_rollback_keep_one_active_version() {
        let temp = Temp::new("apply");
        let layout = layout(temp.path());
        let state = temp.path().join("state");
        let first = archive(temp.path(), "1.2.3");
        let second = archive(temp.path(), "1.2.4");
        let report = apply(&layout, &options(state.clone(), "1.2.3", first, None)).unwrap();
        assert_eq!(report["version"], "1.2.3");
        assert_eq!(layout.current_version().as_deref(), Some("1.2.3"));
        let report = apply(&layout, &options(state.clone(), "1.2.4", second, None)).unwrap();
        assert_eq!(report["version"], "1.2.4");
        assert_eq!(layout.current_version().as_deref(), Some("1.2.4"));
        let report = rollback(&layout, &state, false).unwrap();
        assert_eq!(report["version"], "1.2.3");
        assert_eq!(layout.current_version().as_deref(), Some("1.2.3"));
    }

    #[test]
    fn a_crash_after_staging_keeps_the_old_version_and_recovers() {
        let temp = Temp::new("crash");
        let layout = layout(temp.path());
        let state = temp.path().join("state");
        let archive = archive(temp.path(), "2.0.0");
        let error = apply(&layout, &options(state.clone(), "2.0.0", archive.clone(), Some("staged"))).unwrap_err();
        assert!(error.contains("simulated crash"));
        assert_eq!(layout.current_version(), None, "nothing was switched before the crash");
        let report = apply(&layout, &options(state, "2.0.0", archive, None)).unwrap();
        assert_eq!(report["version"], "2.0.0");
        assert_eq!(layout.current_version().as_deref(), Some("2.0.0"));
    }

    #[test]
    fn a_bad_checksum_is_refused_before_anything_is_installed() {
        let temp = Temp::new("sums");
        let layout = layout(temp.path());
        let archive = archive(temp.path(), "3.0.0");
        std::fs::write(archive.with_file_name("SHA256SUMS"), "0000  wrong.tar.gz\n").unwrap();
        let error = stage(&layout, Some(&archive), Some("3.0.0"), None, DEFAULT_REPO).unwrap_err();
        assert!(error.contains("SHA256SUMS") || error.contains("SHA-256"), "{error}");
        assert_eq!(layout.current_version(), None);
    }

    #[test]
    fn health_refuses_a_foreign_or_wrong_schema_database() {
        let temp = Temp::new("health");
        let state = temp.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let db = state.join("session.sqlite");
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO meta VALUES ('format_id', 'someone-else');
                 INSERT INTO meta VALUES ('schema_version', '5');",
            )
            .unwrap();
        assert!(health_json(&state).is_err(), "a foreign format must be refused");
        connection
            .execute("UPDATE meta SET value = ?1 WHERE key = 'format_id'", [teamagents_core::v2::models::V2_FORMAT_ID])
            .unwrap();
        connection
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
                [teamagents_core::v2::models::V2_SCHEMA_VERSION.to_string()],
            )
            .unwrap();
        assert!(health_json(&state).is_ok(), "the current format must be accepted");
    }

    #[test]
    fn request_validation_refuses_urls_paths_and_empty_text() {
        assert!(validate_request("latest", "why", "continue").is_ok());
        assert!(validate_request("1.2.3", "why", "continue").is_ok());
        assert!(validate_request("https://example.invalid", "why", "continue").is_err());
        assert!(validate_request("../../etc/passwd", "why", "continue").is_err());
        assert!(validate_request("1.2.3", " ", "continue").is_err());
        assert!(validate_request("1.2.3", "why", " ").is_err());
    }
}

#[cfg(test)]
mod follow_up_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_follow_up_is_sent_to_the_leader_through_the_new_binary() {
        let root = std::env::temp_dir().join(format!("ta-update-follow-{}", uuid::Uuid::new_v4()));
        let layout = Layout { root: root.join("install"), bin_dir: root.join("bin") };
        let binary = layout.root.join("current/teamagents");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        let log = root.join("follow.args");
        std::fs::write(&binary, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n", log.display())).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let request = UpdateRequest {
            id: "r1".into(),
            target: "1.2.3".into(),
            reason: "test".into(),
            follow_up: "continue the task".into(),
            state_root: root.join("state"),
            session_id: "s1".into(),
            instance_id: "i-main".into(),
            leader_id: "i-leader".into(),
            operation_id: "op1".into(),
            daemon_pid: std::process::id(),
            created_at: 0.0,
        };
        submit_follow_up(&layout, &request).unwrap();
        let args = std::fs::read_to_string(&log).unwrap();
        assert!(args.contains("instances"), "{args}");
        assert!(args.contains("send"), "{args}");
        assert!(args.contains("i-leader"), "{args}");
        assert!(args.contains("continue the task"), "{args}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
