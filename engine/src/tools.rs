//! Sandboxed tool executors: file tools confined to
//! the member workspace, shell via bwrap when present, web fetch with an SSRF
//! guard.

use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use crate::gateway::TurnControl;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_OUTPUT: usize = 200_000;
/// A runaway command must not fill the disk: the artifact keeps arrival order
/// up to this many bytes, and `finish` says so when the tail was dropped.
const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
/// Long sessions accumulate one artifact per oversized command, so the
/// directory keeps only its newest files within this budget (oldest dropped
/// first). Their /artifacts/ references stay valid only while the file is kept;
/// a missing artifact reports itself when read.
const ARTIFACT_DIR_BYTES: u64 = 512 * 1024 * 1024;

/// Drop the oldest `exec-*.log` files until the directory fits the budget.
/// Returns the number of files removed; failures are ignored (pruning is
/// best-effort housekeeping, never a reason to fail a command).
fn prune_artifacts(dir: &Path, budget: u64) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut files: Vec<(std::time::SystemTime, PathBuf, u64)> = entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("exec-") && entry.file_name().to_string_lossy().ends_with(".log"))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if !meta.is_file() { return None; }
            Some((meta.modified().ok()?, entry.path(), meta.len()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, _, len)| len).sum();
    if total <= budget { return 0; }
    files.sort_by_key(|(modified, _, _)| *modified);
    let mut removed = 0;
    for (_, path, len) in files {
        if total <= budget { break; }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
            removed += 1;
        }
    }
    removed
}
const PAGE_BYTES: usize = 32_000;
// ponytail: serialize native mutations in this process; use per-path locks if
// unrelated writes contend. Shell/external editors still need hash checks.
static FILE_WRITES: Mutex<()> = Mutex::new(());
/// Virtual prefix members use to read long shell output back (`/artifacts/`
/// routes to the session artifact directory).
const ARTIFACTS_PREFIX: &str = "/artifacts/";

/// Resolve `key` inside root; reject traversal and symlinks escaping root.
pub fn resolve_in_root(root: &Path, key: &str) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let joined = if Path::new(key).is_absolute() { PathBuf::from(key) } else { root.join(key) };
    let normalized = normalize(&joined);
    if !normalized.starts_with(&root) {
        return Err(format!("path escapes workspace: {key}"));
    }
    if let Ok(real) = std::fs::canonicalize(&normalized) {
        if !real.starts_with(&root) {
            return Err(format!("path escapes workspace: {key}"));
        }
        return Ok(real);
    }
    // the file may not exist yet: the deepest existing ancestor must stay inside
    let mut probe = normalized.as_path();
    loop {
        // canonicalize also fails for dangling/cyclic links. They are not a
        // missing ordinary path: a later create would follow them on the host.
        match std::fs::symlink_metadata(probe) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(format!("unresolved symlink: {key}")),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
            _ => {}
        }
        let Some(parent) = probe.parent() else { break };
        if let Ok(real) = std::fs::canonicalize(parent) {
            if !real.starts_with(&root) {
                return Err(format!("path escapes workspace: {key}"));
            }
            break;
        }
        probe = parent;
    }
    Ok(normalized)
}

/// Pin the parent directory before opening a member file. Linux /proc fd paths
/// keep a concurrent symlink replacement from redirecting creates or writes.
fn open_member_file(root: &Path, path: &Path) -> Result<std::fs::File, String> {
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let parent = member_parent(&root, path, false)?;
    let leaf = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(path.file_name().ok_or("file path required")?);
    let file = std::fs::File::open(&leaf).map_err(|e| e.to_string())?;
    check_member_fd(&root, &file)?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() { return Err("not a regular file".into()); }
    Ok(file)
}

fn check_member_fd(root: &Path, file: &std::fs::File) -> Result<(), String> {
    let real = std::fs::canonicalize(format!("/proc/self/fd/{}", file.as_raw_fd())).map_err(|e| e.to_string())?;
    if !real.starts_with(root) { return Err("path escapes workspace".into()); }
    Ok(())
}

fn member_parent(root: &Path, path: &Path, create: bool) -> Result<std::fs::File, String> {
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let relative = path.strip_prefix(&root).map_err(|_| "path escapes workspace")?;
    relative.file_name().ok_or("file path required")?;
    let mut parent = std::fs::File::open(&root).map_err(|e| e.to_string())?;
    check_member_fd(&root, &parent)?;
    for part in relative.parent().unwrap_or(Path::new("")).components() {
        let Component::Normal(part) = part else { return Err("invalid file path".into()) };
        let next = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(part);
        if create && !next.exists() {
            // create_dir never follows/replaces an existing dangling symlink.
            match std::fs::create_dir(&next) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        parent = std::fs::File::open(next).map_err(|e| e.to_string())?;
        check_member_fd(&root, &parent)?;
    }
    Ok(parent)
}

fn file_hash(file: &mut std::fs::File, control: &TurnControl) -> Result<String, String> {
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 8192];
    loop {
        control.check()?;
        let count = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if count == 0 { break; }
        hash.update(&bytes[..count]);
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", hash.finalize()))
}

fn expected_hash(args: &Json) -> Result<Option<&str>, String> {
    let Some(value) = args.get("expected_sha256") else { return Ok(None) };
    let hash = value.as_str().filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or("expected_sha256 must contain 64 hexadecimal characters")?;
    Ok(Some(hash))
}

/// Lock file name for one target: a hash of its absolute path, so the lock is
/// stable across renames and is never written into the user's project.
fn lock_file(lock_dir: &Path, target: &Path) -> PathBuf {
    let key = format!("{:x}", Sha256::digest(target.to_string_lossy().as_bytes()));
    lock_dir.join(format!("{key}.lock"))
}

/// Cross-process write exclusion. The in-process mutex cannot see a second
/// teamagents process (or a `--resume` in another terminal) editing the same
/// workspace, and the file itself cannot carry the lock because it gets replaced
/// by rename. Locks live beside the session state instead.
/// ponytail: one lock per file, held for the whole write; no reader locks.
fn with_path_lock<T>(lock_dir: Option<&Path>, target: &Path, body: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let Some(lock_dir) = lock_dir else { return body() };
    std::fs::create_dir_all(lock_dir).map_err(|e| format!("cannot create lock directory: {e}"))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_file(lock_dir, target))
        .map_err(|e| format!("cannot open write lock: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match file.try_lock() {
            Ok(()) => break,
            // contention: give the other writer time to finish its rename
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err("another teamagents process is writing this file; retry".into());
            }
            // a filesystem without advisory locks must not fail every write
            Err(std::fs::TryLockError::Error(_)) => return body(),
        }
    }
    let result = body();
    let _ = file.unlock();
    result
}

fn atomic_write(root: &Path, lock_dir: Option<&Path>, path: &Path, content: &[u8], expected: Option<&str>, control: &TurnControl) -> Result<(), String> {
    if content.len() as u64 > MAX_FILE_BYTES { return Err("content too large".into()); }
    with_path_lock(lock_dir, path, || atomic_write_locked(root, path, content, expected, control))
}

fn atomic_write_locked(root: &Path, path: &Path, content: &[u8], expected: Option<&str>, control: &TurnControl) -> Result<(), String> {
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let parent = member_parent(&root, path, true)?;
    let parent_path = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
    let leaf = parent_path.join(path.file_name().ok_or("file path required")?);
    let permissions = match std::fs::symlink_metadata(&leaf) {
        Ok(meta) if meta.file_type().is_file() => Some(meta.permissions()),
        Ok(_) => return Err("not a regular file (or path changed to a symlink)".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let temp = parent_path.join(format!(".teamagents-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|e| e.to_string())?;
        // Restrict the staging inode before it contains any old private data.
        if let Some(permissions) = permissions.as_ref() { file.set_permissions(permissions.clone()).map_err(|e| e.to_string())?; }
        file.write_all(content).map_err(|e| e.to_string())?;
        if let Some(permissions) = permissions { file.set_permissions(permissions).map_err(|e| e.to_string())?; }
        file.sync_all().map_err(|e| e.to_string())?;
        control.check()?;
        check_member_fd(&root, &parent)?;
        // Do not follow a leaf swapped by an external writer while staging.
        if std::fs::symlink_metadata(&leaf).is_ok_and(|meta| !meta.is_file()) {
            return Err("file changed while writing".into());
        }
        if let Some(expected) = expected {
            let mut current = open_member_file(&root, path).map_err(|e| format!("file conflict: {e}"))?;
            if !file_hash(&mut current, control)?.eq_ignore_ascii_case(expected) {
                return Err("file conflict: expected_sha256 no longer matches; read the file again".into());
            }
        }
        std::fs::rename(&temp, &leaf).map_err(|e| e.to_string())?;
        parent.sync_all().map_err(|e| format!("file replaced but directory sync failed: {e}"))
    })();
    if result.is_err() { let _ = std::fs::remove_file(&temp); }
    result
}

fn positive_arg(args: &Json, key: &str, default: u64) -> Result<u64, String> {
    match args.get(key) {
        Some(value) => value.as_u64().filter(|n| *n > 0).ok_or_else(|| format!("{key} must be a positive integer")),
        None => Ok(default),
    }
}

/// Byte continuation covers even one enormous line; line reads never allocate
/// the skipped prefix or the remainder of an unbounded tool-output artifact.
fn read_page(mut file: std::fs::File, args: &Json, control: &TurnControl) -> Result<Json, String> {
    let offset = positive_arg(args, "offset", 1)?;
    let limit = positive_arg(args, "limit", 2000)?.min(2000);
    let byte_offset = args.get("byte_offset").map(|v| v.as_u64().ok_or("byte_offset must be a nonnegative integer")).transpose()?;
    if byte_offset.is_some() && offset != 1 { return Err("use offset or byte_offset, not both".into()); }
    let hash = if args.get("include_sha256").and_then(Json::as_bool).unwrap_or(false) {
        Some(file_hash(&mut file, control)?)
    } else { None };
    if let Some(pos) = byte_offset { file.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?; }
    let mut reader = BufReader::new(file);
    let mut position = byte_offset.unwrap_or(0);
    let mut skipped = 1;
    while byte_offset.is_none() && skipped < offset {
        control.check()?;
        let chunk = reader.fill_buf().map_err(|e| e.to_string())?;
        if chunk.is_empty() { break; }
        let count = chunk.iter().position(|b| *b == b'\n').map(|i| i + 1).unwrap_or(chunk.len());
        if chunk[count - 1] == b'\n' { skipped += 1; }
        reader.consume(count);
        position += count as u64;
    }
    let start = position;
    let mut bytes = Vec::with_capacity(PAGE_BYTES);
    let mut lines = 0;
    while bytes.len() < PAGE_BYTES && lines < limit {
        control.check()?;
        let chunk = reader.fill_buf().map_err(|e| e.to_string())?;
        if chunk.is_empty() { break; }
        let mut count = 0;
        for byte in chunk.iter().take(PAGE_BYTES - bytes.len()) {
            count += 1;
            if *byte == b'\n' { lines += 1; }
            if lines == limit { break; }
        }
        bytes.extend_from_slice(&chunk[..count]);
        reader.consume(count);
    }
    let valid = match std::str::from_utf8(&bytes) {
        Ok(_) => bytes.len(),
        Err(e) if e.error_len().is_none() && bytes.len() == PAGE_BYTES => e.valid_up_to(),
        Err(_) => return Err("file is not UTF-8 text (or byte_offset splits a character)".into()),
    };
    let more = valid < bytes.len() || !reader.fill_buf().map_err(|e| e.to_string())?.is_empty();
    bytes.truncate(valid);
    position += bytes.len() as u64;
    let content = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    if !more && offset == 1 && byte_offset.is_none() && hash.is_none() && args.get("limit").is_none() && args.get("offset").is_none() {
        return Ok(json!(content));
    }
    let next_offset = if more && byte_offset.is_none() && content.ends_with('\n') { Some(offset + lines) } else { None };
    Ok(json!({"content": content, "offset": if byte_offset.is_some() { None } else { Some(offset) },
        "byte_offset": start, "next_offset": next_offset, "next_byte_offset": if more { Some(position) } else { None },
        "truncated": more, "eof": !more, "sha256": hash}))
}

fn edit_diff(path: &Path, text: &str, at: usize, old: &str, new: &str) -> String {
    let line = text[..at].bytes().filter(|b| *b == b'\n').count() + 1;
    let mut diff = format!("edited {}\n@@ line {line} @@\n", path.display());
    for (prefix, value) in [("-", old), ("+", new)] {
        for line in value.lines() {
            if diff.len() > 8000 { diff.push_str("[diff truncated; read_file for full content]\n"); return diff; }
            diff.push_str(prefix);
            diff.extend(line.chars().take(1000));
            if line.chars().count() > 1000 { diff.push_str(" [line truncated]"); }
            diff.push('\n');
        }
    }
    diff
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn cap_read(file: std::fs::File) -> Result<String, String> {
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("file too large ({} bytes)", meta.len()));
    }
    let mut text = String::new();
    file.take(MAX_FILE_BYTES + 1).read_to_string(&mut text).map_err(|e| e.to_string())?;
    if text.len() as u64 > MAX_FILE_BYTES { return Err("file too large".into()); }
    Ok(text)
}

/// Resolve a member-visible artifact reference (`/artifacts/<name>`) inside the
/// session artifact directory; traversal out of it is refused like any root.
fn resolve_artifact(artifacts: Option<&PathBuf>, key: &str) -> Result<PathBuf, String> {
    let root = artifacts.ok_or_else(|| "no artifact directory for this member".to_string())?;
    let name = key.strip_prefix(ARTIFACTS_PREFIX).unwrap_or(key);
    resolve_in_root(root, name)
}

/// File/tool executor for one member's workspace. `artifacts` is the session
/// artifact directory: long shell output lands there
/// and members read it back through read_file/read_artifact.
pub fn workspace_executor(
    root: PathBuf,
    artifacts: Option<PathBuf>,
) -> impl Fn(&str, &Json) -> Result<Json, String> + Send + Sync + 'static {
    let executor = workspace_executor_with_control(root, artifacts);
    move |tool, args| executor(tool, args, &TurnControl::default())
}

fn workspace_executor_with_control(
    root: PathBuf, artifacts: Option<PathBuf>,
) -> impl Fn(&str, &Json, &TurnControl) -> Result<Json, String> + Send + Sync + 'static {
    // cross-process write locks live beside the session state, never in the project
    let lock_dir = artifacts.as_ref().and_then(|dir| dir.parent()).map(|state| state.join("locks"));
    move |tool: &str, args: &Json, control: &TurnControl| -> Result<Json, String> {
        control.check()?;
        let arg = |key: &str| args.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let arg_or = |args: &Json, key: &str, default: &str| -> String {
            args.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or(default).to_string()
        };
        // `/artifacts/...` is a virtual path for this member, not a host path;
        // that prefix routes to the session artifact directory
        let member_path = |key: &str| -> Result<PathBuf, String> {
            if key.starts_with(ARTIFACTS_PREFIX) {
                resolve_artifact(artifacts.as_ref(), key)
            } else {
                resolve_in_root(&root, key)
            }
        };
        let member_file = |key: &str| {
            let path = member_path(key)?;
            let file_root = if key.starts_with(ARTIFACTS_PREFIX) { artifacts.as_ref().unwrap() } else { &root };
            open_member_file(file_root, &path)
        };
        match tool {
            "ls" => {
                let key = if arg("path").is_empty() { ".".to_string() } else { arg("path") };
                let path = resolve_in_root(&root, &key)?;
                let mut names: Vec<String> = std::fs::read_dir(&path)
                    .map_err(|e| e.to_string())?
                    .flatten()
                    .map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if e.path().is_dir() {
                            format!("{name}/")
                        } else {
                            name
                        }
                    })
                    .collect();
                names.sort();
                Ok(json!(names.join("\n")))
            }
            "read_file" => read_page(member_file(&arg("path"))?, args, control),
            "read_artifact" => {
                let key = arg("path");
                let key = if key.is_empty() { arg("name") } else { key };
                let path = resolve_artifact(artifacts.as_ref(), &key)?;
                read_page(open_member_file(artifacts.as_ref().unwrap(), &path)?, args, control)
            }
            "write_file" => {
                let _guard = FILE_WRITES.lock().map_err(|_| "file mutation lock poisoned")?;
                control.check()?;
                let path = member_path(&arg("path"))?;
                let content = args.get("content").and_then(|v| v.as_str()).ok_or("content must be a string")?;
                let file_root = if arg("path").starts_with(ARTIFACTS_PREFIX) { artifacts.as_ref().unwrap() } else { &root };
                atomic_write(file_root, lock_dir.as_deref(), &path, content.as_bytes(), expected_hash(args)?, control)?;
                Ok(json!(format!("wrote {}", path.display())))
            }
            "edit_file" => {
                let _guard = FILE_WRITES.lock().map_err(|_| "file mutation lock poisoned")?;
                control.check()?;
                let path = member_path(&arg("path"))?;
                let text = cap_read(member_file(&arg("path"))?)?;
                let old = arg("old_string");
                let new = args.get("new_string").and_then(Json::as_str).ok_or("new_string must be a string")?;
                if old.is_empty() { return Err("old_string must not be empty".into()); }
                let at = text.find(&old).ok_or("old_string not found")?;
                // Count overlapping matches too (e.g. 'aa' in 'aaa').
                if text[at + old.chars().next().unwrap().len_utf8()..].contains(&old) {
                    return Err("old_string matches multiple locations; include more context".into());
                }
                let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
                if expected_hash(args)?.is_some_and(|expected| !expected.eq_ignore_ascii_case(&hash)) {
                    return Err("file conflict: expected_sha256 no longer matches; read the file again".into());
                }
                let file_root = if arg("path").starts_with(ARTIFACTS_PREFIX) { artifacts.as_ref().unwrap() } else { &root };
                atomic_write(file_root, lock_dir.as_deref(), &path, text.replacen(&old, new, 1).as_bytes(), Some(&hash), control)?;
                Ok(json!(edit_diff(&path, &text, at, &old, new)))
            }
            "delete" => {
                let _guard = FILE_WRITES.lock().map_err(|_| "file mutation lock poisoned")?;
                control.check()?;
                let path = member_path(&arg("path"))?;
                let recursive = args.get("recursive").and_then(|v| v.as_bool()).unwrap_or(false);
                let result = if recursive {
                    std::fs::remove_dir_all(&path)
                } else if path.is_dir() {
                    std::fs::remove_dir(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                result.map_err(|e| e.to_string())?;
                Ok(json!(format!("deleted {}", path.display())))
            }
            "glob" => {
                let pattern = if arg("pattern").is_empty() { "*".to_string() } else { arg("pattern") };
                if bwrap_available() && sandbox_rg_available() {
                    // A positive `rg --glob` overrides .gitignore. Filter the
                    // already-ignored file list instead.
                    let command = format!("set -o pipefail; rg --files -- . | rg --color never -- {}", shell_quote(&glob_regex(&pattern)));
                    return shell_run_with_control(&command, &root, 30, false, artifacts.as_deref(), control)
                        .map(Json::String);
                }
                let mut hits = vec![];
                glob_walk(&root, &root, &pattern, &mut hits, control)?;
                Ok(json!(hits.join("\n")))
            }
            "grep" => {
                let pattern = shell_quote(&arg("pattern"));
                let path = resolve_in_root(&root, &arg_or(args, "path", "."))?;
                let path = shell_quote(&path.to_string_lossy());
                let command = if sandbox_rg_available() {
                    format!("rg --line-number --no-heading --color never -- {pattern} {path}")
                } else {
                    format!("grep -rn -- {pattern} {path}")
                };
                shell_run_with_control(&command, &root, 30, false, artifacts.as_deref(), control)
                    .map(Json::String)
            }
            "shell" => {
                let command = arg("command");
                let timeout = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(120);
                let network = args.get("network").and_then(|v| v.as_bool()).unwrap_or(false);
                shell_run_with_control(&command, &root, timeout, network, artifacts.as_deref(), control).map(Json::String)
            }
            other => Err(format!("unknown tool {other}")),
        }
    }
}

/// One member's bound web providers. Binding a service *is* the authorization:
/// a member that did not bind a web service has
/// no web tool, and the executor says so instead of failing open.
#[derive(Default, Clone)]
pub(crate) struct WebTools {
    pub(crate) search: Option<teamagents_core::models::ToolBinding>,
    pub(crate) fetch: Option<teamagents_core::models::ToolBinding>,
}

fn is_web_kind(kind: &str) -> bool {
    matches!(kind, "web_search" | "web_fetch")
}

/// Resolve the member's web bindings: explicitly bound names win, in binding
/// order, then `web` expands to every configured web binding. The catalog is a
/// HashMap (unstable order), so the expansion is sorted by name.
pub(crate) fn web_tools(
    catalog: &teamagents_core::models::UserConfig,
    bindings: &[String],
) -> Result<WebTools, String> {
    let mut candidates: Vec<(String, &teamagents_core::models::ToolBinding)> = vec![];
    for name in bindings {
        if name == "web" {
            continue;
        }
        if let Some(binding) = catalog.tools.get(name) {
            if is_web_kind(&binding.kind) {
                candidates.push((name.clone(), binding));
            }
        }
    }
    if candidates.is_empty() && bindings.iter().any(|b| b == "web") {
        candidates = catalog
            .tools
            .iter()
            .filter(|(_, binding)| is_web_kind(&binding.kind))
            .map(|(name, binding)| (name.clone(), binding))
            .collect();
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
    }
    let mut tools = WebTools::default();
    for (name, binding) in candidates {
        // a required service that cannot load fails the member at load time
        // (ToolServiceUnavailable)
        if binding.required && binding.kind == "web_search" {
            let provider = binding.provider.clone().unwrap_or_else(|| "anysearch".into());
            if provider != "anysearch" {
                return Err(format!(
                    "required tool service {name:?} is unavailable: unsupported web_search provider {provider:?}"
                ));
            }
        }
        match binding.kind.as_str() {
            "web_search" if tools.search.is_none() => tools.search = Some(binding.clone()),
            "web_fetch" if tools.fetch.is_none() => tools.fetch = Some(binding.clone()),
            _ => {}
        }
    }
    Ok(tools)
}

/// Load-time check for the web half of a member's bindings (session.rs calls
/// this while building the member's runner).
pub fn validate_web_bindings(
    catalog: &teamagents_core::models::UserConfig,
    bindings: &[String],
) -> Result<(), String> {
    web_tools(catalog, bindings).map(|_| ())
}


/// Skills registry roots: user-configured `skills_paths` only (project and
/// member skill dirs live inside the workspace and are readable with `files`).
/// Read-only by construction (plan §12.2: selected skills are pre-authorized reads).
fn skill_roots(catalog: &teamagents_core::models::UserConfig) -> Vec<PathBuf> {
    catalog
        .skills_paths
        .iter()
        .map(|p| crate::config::expand_home(p))
        .filter(|p| p.is_dir())
        .collect()
}

/// (name, canonical SKILL.md path) pairs under one registry root; candidates
/// whose symlink chain resolves outside the canonical root are rejected (P2-6).
pub fn skill_candidates(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(root) = std::fs::canonicalize(root) else { return vec![] };
    let mut candidates = vec![root.join("SKILL.md")];
    if let Ok(entries) = std::fs::read_dir(&root) {
        candidates.extend(entries.flatten().map(|e| e.path().join("SKILL.md")));
    }
    let mut out: Vec<(String, PathBuf)> = vec![];
    for path in candidates {
        let Ok(real) = std::fs::canonicalize(&path) else { continue };
        if !real.starts_with(&root) || !real.is_file() {
            continue;
        }
        let name = real
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.is_empty() && !out.iter().any(|(n, _)| *n == name) {
            out.push((name, real));
        }
    }
    out
}

/// (name, SKILL.md path) pairs across all registry roots; first root wins on
/// a name clash (user order = priority).
fn skill_index(roots: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = vec![];
    for root in roots {
        for (name, path) in skill_candidates(root) {
            if !out.iter().any(|(n, _)| *n == name) {
                out.push((name, path));
            }
        }
    }
    out
}

/// Read the YAML description, including folded/literal block scalars.
fn skill_blurb(path: &Path) -> String {
    let mut bytes = vec![];
    if std::fs::File::open(path).and_then(|f| f.take(32_000).read_to_end(&mut bytes)).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.trim_start_matches('\u{feff}').lines();
    if lines.next().map(str::trim) != Some("---") { return String::new(); }
    let frontmatter = lines.take_while(|line| !matches!(line.trim(), "---" | "...")).collect::<Vec<_>>().join("\n");
    serde_yaml::from_str::<Json>(&frontmatter).ok()
        .and_then(|value| value["description"].as_str().map(|s| s.chars().take(200).collect()))
        .unwrap_or_default()
}

/// `skill` tool: action "search" (keyword match over name+blurb; empty query
/// lists, capped) or "read" (exact name -> full SKILL.md, capped).
fn skill_tool(catalog: &teamagents_core::models::UserConfig, args: &Json) -> Result<Json, String> {
    const READ_CAP: usize = 32_000;
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let index = skill_index(&skill_roots(catalog));
    if index.is_empty() {
        return Err("no skills configured (user config skills_paths)".into());
    }
    match action {
        "read" => {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let Some((_, path)) = index.iter().find(|(n, _)| n == name) else {
                let known: Vec<&str> = index.iter().map(|(n, _)| n.as_str()).collect();
                return Err(format!("unknown skill {name:?}; known: {}", known.join(", ")));
            };
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            Ok(Json::String(text.chars().take(READ_CAP).collect()))
        }
        "search" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let words: Vec<&str> = query.split_whitespace().collect();
            let mut hits: Vec<(usize, String)> = index
                .iter()
                .map(|(name, path)| {
                    let blurb = skill_blurb(path);
                    let hay = format!("{} {}", name.to_lowercase(), blurb.to_lowercase());
                    let score = words.iter().filter(|w| hay.contains(**w)).count();
                    (score, format!("{name} — {blurb}"))
                })
                .filter(|(score, _)| words.is_empty() || *score > 0)
                .collect();
            hits.sort_by(|a, b| b.0.cmp(&a.0));
            let total = hits.len();
            let mut lines: Vec<String> = hits.into_iter().take(10).map(|(_, line)| line).collect();
            if total > 10 {
                lines.push(format!("… {} more; refine the query", total - 10));
            }
            Ok(Json::String(if lines.is_empty() { "no matching skills".into() } else { lines.join("
") }))
        }
        other => Err(format!("unknown skill action {other:?} (use search|read)")),
    }
}

/// Executor for one member root: file/shell tools rooted there plus the web
/// tools that member actually bound.
pub fn member_executor(
    root: PathBuf,
    catalog: teamagents_core::models::UserConfig,
    bindings: Vec<String>,
    artifacts: Option<PathBuf>,
) -> impl Fn(&str, &Json) -> Result<Json, String> + Send + Sync + 'static {
    let executor = member_executor_with_control(root, catalog, bindings, artifacts);
    move |tool, args| executor(tool, args, &TurnControl::default())
}

pub(crate) fn member_executor_with_control(
    root: PathBuf, catalog: teamagents_core::models::UserConfig, bindings: Vec<String>, artifacts: Option<PathBuf>,
) -> impl Fn(&str, &Json, &TurnControl) -> Result<Json, String> + Send + Sync + 'static {
    let workspace = workspace_executor_with_control(root, artifacts);
    let web: OnceLock<Result<WebTools, String>> = OnceLock::new();
    move |tool: &str, args: &Json, control: &TurnControl| match tool {
        "web_search" => {
            let binding = web
                .get_or_init(|| web_tools(&catalog, &bindings))
                .as_ref()
                .map_err(|e| e.clone())?
                .search
                .clone()
                .ok_or_else(|| format!("tool {tool} is not bound to this member"))?;
            let provider = binding.provider.clone().unwrap_or_else(|| "anysearch".into());
            if provider != "anysearch" {
                return Err(format!("unsupported web_search provider {provider:?}"));
            }
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let max = args.get("max_results").and_then(|v| v.as_i64()).unwrap_or(5);
            let include = args.get("include_content").and_then(|v| v.as_bool()).unwrap_or(false);
            web_search(query, max, binding.url.as_deref(), binding.api_key_env.as_deref(), include)
        }
        "web_fetch" => {
            let binding = web
                .get_or_init(|| web_tools(&catalog, &bindings))
                .as_ref()
                .map_err(|e| e.clone())?
                .fetch
                .clone()
                .ok_or_else(|| format!("tool {tool} is not bound to this member"))?;
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let max = args.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(2_000_000) as usize;
            let allow_private = binding.env.get("allow_private").map(|v| v == "1").unwrap_or(false);
            web_fetch(url, max, allow_private)
        }
        "skill" => {
            if !bindings.iter().any(|b| b == "skills") {
                return Err("tool skill is not bound to this member".into());
            }
            skill_tool(&catalog, args)
        }
        other => {
            let capability = if other == "shell" { "shell" } else { "files" };
            if !bindings.iter().any(|b| b == capability) {
                return Err(format!("tool {other} is not bound to this member"));
            }
            workspace(other, args, control)
        }
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn sandbox_rg_available() -> bool {
    // Match shell_run's sanitized PATH; a host-only ~/.local binary is not
    // mounted into the shell sandbox.
    ["/usr/local/bin/rg", "/usr/bin/rg", "/bin/rg"].iter().any(|path| Path::new(path).is_file())
}

fn glob_regex(pattern: &str) -> String {
    let mut regex = String::from("^(?:\\./)?");
    let parts: Vec<&str> = pattern.trim_start_matches("./").split('/').filter(|s| !s.is_empty()).collect();
    for (index, part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        if *part == "**" {
            regex.push_str(if last { ".*" } else { "(?:[^/]+/)*" });
            continue;
        }
        for ch in part.chars() {
            match ch {
                '*' => regex.push_str("[^/]*"),
                '?' => regex.push_str("[^/]"),
                '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\' => { regex.push('\\'); regex.push(ch); }
                _ => regex.push(ch),
            }
        }
        if !last { regex.push('/'); }
    }
    regex.push('$');
    regex
}

/// Tiny glob: `*` (within a segment), `?`, `**` (any depth). Enough for the
/// patterns members send; a full glob crate is not worth the dependency yet.
fn glob_walk(root: &Path, dir: &Path, pattern: &str, out: &mut Vec<String>, control: &TurnControl) -> Result<(), String> {
    // ponytail: without rg the fallback supports simple globs, not gitignore;
    // install rg for the same ignore semantics as repository search.
    let segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((current, index)) = stack.pop() {
        control.check()?;
        if out.len() >= 500 || index > segments.len() {
            continue;
        }
        if index == segments.len() {
            if let Ok(rel) = current.strip_prefix(root) {
                if !rel.as_os_str().is_empty() {
                    out.push(rel.to_string_lossy().into_owned());
                }
            }
            continue;
        }
        let segment = segments[index];
        if segment == "**" {
            stack.push((current.clone(), index + 1));
            if let Ok(entries) = std::fs::read_dir(&current) {
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                        stack.push((entry.path(), index));
                    }
                }
            }
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&current) {
            for entry in entries.flatten() {
                control.check()?;
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_symlink() { continue; }
                let name = entry.file_name().to_string_lossy().into_owned();
                if !segment_match(segment, &name) {
                    continue;
                }
                let path = entry.path();
                if index + 1 == segments.len() {
                    if let Ok(rel) = path.strip_prefix(root) {
                        out.push(rel.to_string_lossy().into_owned());
                        if out.len() >= 500 { return Ok(()); }
                    }
                } else if kind.is_dir() {
                    stack.push((path, index + 1));
                }
            }
        }
    }
    Ok(())
}

fn segment_match(pattern: &str, name: &str) -> bool {
    // iterative wildcard match over bytes (patterns are ASCII globs)
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ni;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

pub fn bwrap_available() -> bool {
    which("bwrap").is_some()
}

pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Read-only system mounts, sanitized env, private
/// /tmp, no network unless the call was approved for it.
pub fn bwrap_argv(workdir: &Path, network: bool, command: &str) -> Vec<String> {
    let mut argv: Vec<String> = vec!["bwrap".into()];
    for path in ["/usr", "/etc", "/opt"] {
        if Path::new(path).exists() {
            argv.extend(["--ro-bind".into(), path.into(), path.into()]);
        }
    }
    for (target, link) in [("usr/lib", "/lib"), ("usr/lib64", "/lib64"), ("usr/bin", "/bin"), ("usr/bin", "/sbin")] {
        // the host's /bin is itself a symlink, so only the target is checked
        if Path::new(&format!("/{target}")).exists() {
            argv.extend(["--symlink".into(), target.into(), link.into()]);
        }
    }
    let workdir = workdir.to_string_lossy().into_owned();
    argv.extend(["--proc".into(), "/proc".into(), "--dev".into(), "/dev".into(), "--tmpfs".into(), "/tmp".into()]);
    // after --tmpfs /tmp: the sandbox is the only place writable enough to hold
    // these mount points, and $HOME stays invisible.
    argv.extend(toolchain_binds());
    argv.extend(["--bind".into(), workdir.clone(), workdir.clone()]);
    argv.extend(["--chdir".into(), workdir]);
    argv.extend([
        "--unshare-pid".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
        "--die-with-parent".into(),
        "--new-session".into(),
    ]);
    if !network {
        argv.push("--unshare-net".into());
    }
    argv.extend(["--".into(), "/bin/bash".into(), "-lc".into(), command.into()]);
    argv
}

/// Where the sandbox mirrors the build toolchains, under the private /tmp.
const TOOLCHAIN_ROOT: &str = "/tmp/.teamagents-toolchain";

/// Build toolchains live under $HOME, which the sandbox hides on purpose, so the
/// caches are mirrored read-only instead: every rustup shim on PATH needs
/// `RUSTUP_HOME` to pick a toolchain at all, and cargo needs its registry/git
/// caches to build offline. Without this a member can edit a project but never
/// build or test it, which defeats "verify your own work".
/// ponytail: Rust only; add go/node caches when a member actually needs them.
fn toolchain_mounts() -> Vec<(&'static str, PathBuf, PathBuf)> {
    let mut mounts = vec![];
    for (key, home, guest) in [("RUSTUP_HOME", ".rustup", "rustup"), ("CARGO_HOME", ".cargo", "cargo")] {
        let host = std::env::var(key).map(PathBuf::from).ok().unwrap_or_else(|| crate::config::expand_home(&format!("~/{home}")));
        if host.is_dir() {
            mounts.push((key, host, PathBuf::from(TOOLCHAIN_ROOT).join(guest)));
        }
    }
    mounts
}

/// `credentials.toml` and `config.toml` can carry registry tokens, so CARGO_HOME
/// exposes only its cache subdirectories (plus `bin` for rustup shims).
fn toolchain_binds() -> Vec<String> {
    let mut argv = vec![];
    for (key, host, guest) in toolchain_mounts() {
        for sub in if key == "CARGO_HOME" { &["bin", "registry", "git"][..] } else { &[""][..] } {
            let source = if sub.is_empty() { host.clone() } else { host.join(sub) };
            if !source.exists() {
                continue;
            }
            let target = if sub.is_empty() { guest.clone() } else { guest.join(sub) };
            argv.extend(["--ro-bind".into(), source.to_string_lossy().into_owned(), target.to_string_lossy().into_owned()]);
        }
    }
    argv
}

fn toolchain_env() -> Vec<(String, String)> {
    toolchain_mounts().into_iter().map(|(key, _, guest)| (key.to_string(), guest.to_string_lossy().into_owned())).collect()
}

fn sandbox_path() -> String {
    let mut path = String::from("/usr/local/bin:/usr/bin:/bin");
    if let Some((_, host, guest)) = toolchain_mounts().into_iter().find(|(key, _, _)| *key == "CARGO_HOME") {
        if host.join("bin").is_dir() {
            path.push(':');
            path.push_str(&guest.join("bin").to_string_lossy());
        }
    }
    path
}

struct OutputSink {
    head: Vec<u8>,
    total: u64,
    written: u64,
    cap: u64,
    artifact: Option<std::fs::File>,
    reference: Option<String>,
    error: Option<String>,
}

impl OutputSink {
    fn new(dir: Option<&Path>) -> Result<Self, String> {
        let (artifact, reference) = if let Some(dir) = dir {
            std::fs::create_dir_all(dir).map_err(|e| format!("cannot create output artifact directory: {e}"))?;
            let name = format!("exec-{}.log", uuid::Uuid::new_v4());
            let file = std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join(&name))
                .map_err(|e| format!("cannot create output artifact: {e}"))?;
            prune_artifacts(dir, ARTIFACT_DIR_BYTES);
            (Some(file), Some(format!("{ARTIFACTS_PREFIX}{name}")))
        } else { (None, None) };
        Ok(Self { head: Vec::with_capacity(MAX_OUTPUT), total: 0, written: 0, cap: MAX_ARTIFACT_BYTES, artifact, reference, error: None })
    }

    fn append(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len() as u64);
        let remaining = MAX_OUTPUT.saturating_sub(self.head.len());
        self.head.extend_from_slice(&bytes[..remaining.min(bytes.len())]);
        if let Some(file) = self.artifact.as_mut() {
            let room = self.cap.saturating_sub(self.written).min(bytes.len() as u64) as usize;
            if room > 0 {
                if let Err(e) = file.write_all(&bytes[..room]) { self.error.get_or_insert_with(|| format!("output artifact write failed: {e}")); }
                else { self.written += room as u64; }
            }
        }
    }

    fn finish(&mut self, interrupted: bool) -> Result<String, String> {
        if let Some(file) = self.artifact.as_mut() {
            if let Err(e) = file.sync_all() { self.error.get_or_insert_with(|| format!("output artifact sync failed: {e}")); }
        }
        let mut text = String::from_utf8_lossy(&self.head).into_owned();
        if self.total > self.head.len() as u64 || interrupted || self.error.is_some() {
            let capped = if self.total > self.written { format!(" (artifact truncated at {} MiB)", self.cap / (1024 * 1024)) } else { String::new() };
            match &self.reference {
                Some(reference) => text.push_str(&format!("\n[{} bytes captured; full output: {reference}]{capped}", self.total)),
                None if self.total > self.head.len() as u64 => text.push_str("\n[output truncated; no artifact directory configured]"),
                None => {}
            }
        }
        match &self.error {
            Some(error) => Err(format!("{error}\n{text}")),
            None => Ok(text),
        }
    }
}

/// Both readers spool immediately and retain only a bounded preview. The
/// artifact preserves arrival order; separate stdout/stderr ordering is not
/// recoverable after the OS has delivered their independent pipe chunks.
fn drain(mut pipe: impl Read + Send + 'static, sink: Arc<Mutex<OutputSink>>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => sink.lock().unwrap().append(&chunk[..read]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => { sink.lock().unwrap().error = Some(format!("output pipe read failed: {e}")); break; }
            }
        }
    })
}

/// Wait for reader threads, but never block the caller forever: if a pipe is
/// still open after the grace period the partial buffer is all we can use.
/// ponytail: a reader that never sees EOF is leaked (its data is already
/// collected); kill the process group instead if that ever shows up.
fn join_bounded(handles: Vec<std::thread::JoinHandle<()>>, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    let mut complete = true;
    for handle in handles {
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if handle.is_finished() {
            complete &= handle.join().is_ok();
        } else { complete = false; }
        // else: leak the reader thread rather than hang the engine; its data is
        // already in the buffer (kill is instantaneous with --die-with-parent)
    }
    complete
}

/// bwrap-only: missing isolation is an error,
/// never a silent fallback to unsandboxed execution (plan §12.2).
pub fn shell_run(
    command: &str,
    workdir: &Path,
    timeout_s: u64,
    network: bool,
    artifacts: Option<&Path>,
) -> Result<String, String> {
    shell_run_with_control(command, workdir, timeout_s, network, artifacts, &TurnControl::default())
}

fn shell_run_with_control(
    command: &str, workdir: &Path, timeout_s: u64, network: bool, artifacts: Option<&Path>, control: &TurnControl,
) -> Result<String, String> {
    control.check()?;
    if !bwrap_available() {
        return Err("IsolationUnavailable: bwrap is not available: refusing to run commands without isolation".into());
    }
    let workdir = std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
    let argv = bwrap_argv(&workdir, network, command);
    let sink = Arc::new(Mutex::new(OutputSink::new(artifacts)?));
    let mut sandbox = Command::new(&argv[0]);
    // whitelist environment: no model keys, no credentials (plan §12.2)
    sandbox
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .env("PATH", sandbox_path())
        .env("HOME", &workdir)
        .env("LANG", std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".into()))
        .env("TERM", "dumb")
        .env("TMPDIR", "/tmp")
        .env("PYTHONIOENCODING", "utf-8");
    for (key, value) in toolchain_env() {
        sandbox.env(key, value);
    }
    let mut child = sandbox
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let stdout_reader = drain(stdout, sink.clone());
    let stderr_reader = drain(stderr, sink.clone());
    let started = Instant::now();
    let mut failure = None;
    let mut status = None;
    loop {
        if control.check().is_err() {
            failure = Some("turn interrupted".to_string());
            break;
        }
        if sink.lock().unwrap().error.is_some() {
            failure = Some("command stopped because output capture failed".into());
            break;
        }
        match child.try_wait() {
            Ok(Some(exit)) => {
                status = Some(exit);
                break;
            }
            Ok(None) if started.elapsed() >= Duration::from_secs(timeout_s) => {
                failure = Some(format!("command timed out after {timeout_s}s"));
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                failure = Some(format!("command wait failed: {e}"));
                break;
            }
        }
    }
    if failure.is_some() {
        // Killing bwrap tears down its private PID namespace and command tree.
        if let Err(e) = child.kill() {
            if e.kind() != std::io::ErrorKind::InvalidInput { failure = Some(format!("{}; kill failed: {e}", failure.unwrap())); }
        }
        if let Err(e) = child.wait() { failure = Some(format!("{}; wait failed: {e}", failure.unwrap())); }
    }
    if !join_bounded(vec![stdout_reader, stderr_reader], Duration::from_secs(5)) {
        sink.lock().unwrap().error.get_or_insert_with(|| "output capture incomplete: reader did not finish".into());
    }
    let text = sink.lock().unwrap().finish(failure.is_some())?;
    if let Some(note) = failure {
        return Err(if text.is_empty() { note } else { format!("{note}\n{text}") });
    }
    let status = status.ok_or("no exit status")?;
    Ok(if status.success() { text } else { format!("{text}\n(exit {})", status.code().unwrap_or(-1)) })
}

/// Host part of `rest` ("host[:port]/path..."), brackets and userinfo included.
fn url_host(rest: &str) -> &str {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map(|(_, host)| host).unwrap_or(authority);
    match authority.strip_prefix('[') {
        Some(inner) => inner.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    }
}

/// Block private/loopback/reserved/multicast targets: the not-globally-
/// reachable blocks of the IANA special-purpose address registries.
pub fn guard_url(url: &str) -> Result<String, String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| format!("unsupported url {url}"))?;
    let host = url_host(rest).to_string();
    if host.is_empty() {
        return Err(format!("unsupported url {url}"));
    }
    if host == "localhost" || host.ends_with(".localhost") {
        return Err(format!("refusing private address for {host}"));
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if is_private_addr(ip) {
            return Err(format!("refusing private address for {host}"));
        }
        return Ok(url.to_string());
    }
    // resolve: every address must be public
    use std::net::ToSocketAddrs;
    let addrs: Vec<std::net::SocketAddr> = (host.as_str(), 443u16)
        .to_socket_addrs()
        .or_else(|_| (host.as_str(), 80u16).to_socket_addrs())
        .map_err(|e| format!("cannot resolve {host}: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("cannot resolve {host}"));
    }
    for addr in addrs {
        if is_private_addr(addr.ip()) {
            return Err(format!("refusing private address for {host}"));
        }
    }
    Ok(url.to_string())
}

// Not-globally-reachable blocks from the IANA special-purpose registries,
// rejected as private/loopback/link-local/reserved/multicast.
const IPV4_BLOCKED: &[(u128, u8)] = &[
    (0x0000_0000, 8),   // 0.0.0.0/8
    (0x0a00_0000, 8),   // 10.0.0.0/8
    (0x7f00_0000, 8),   // 127.0.0.0/8
    (0xa9fe_0000, 16),  // 169.254.0.0/16
    (0xac10_0000, 12),  // 172.16.0.0/12
    (0xc000_0000, 24),  // 192.0.0.0/24
    (0xc000_00aa, 31),  // 192.0.0.170/31
    (0xc000_0200, 24),  // 192.0.2.0/24
    (0xc0a8_0000, 16),  // 192.168.0.0/16
    (0xc612_0000, 15),  // 198.18.0.0/15
    (0xc633_6400, 24),  // 198.51.100.0/24
    (0xcb00_7100, 24),  // 203.0.113.0/24
    (0xe000_0000, 4),   // 224.0.0.0/4 multicast
    (0xf000_0000, 4),   // 240.0.0.0/4 reserved
    (0xffff_ffff, 32),  // 255.255.255.255
];
/// Addresses inside a blocked block that are globally reachable (IANA exceptions).
const IPV4_ALLOWED: &[(u128, u8)] = &[(0xc000_0009, 32), (0xc000_000a, 32)];

fn in_block(bits: u128, block: &[(u128, u8)], width: u32) -> bool {
    block.iter().any(|(net, prefix)| {
        let prefix = *prefix as u32;
        match prefix {
            0 => true,
            _ => (bits >> (width - prefix)) == (*net >> (width - prefix)),
        }
    })
}

fn ipv6_bits(addr: &std::net::Ipv6Addr) -> u128 {
    addr.segments().iter().fold(0u128, |acc, segment| (acc << 16) | u128::from(*segment))
}

fn is_private_v4(addr: std::net::Ipv4Addr) -> bool {
    let bits = u128::from(u32::from(addr));
    in_block(bits, IPV4_BLOCKED, 32) && !in_block(bits, IPV4_ALLOWED, 32)
}

/// Private, loopback, link-local, unique-local, reserved and multicast space
/// (ipaddress.IPv6Address: _private_networks + _reserved_networks + multicast).
const IPV6_BLOCKED: &[([u16; 8], u8)] = &[
    ([0, 0, 0, 0, 0, 0, 0, 1], 128),             // ::1
    ([0, 0, 0, 0, 0, 0, 0, 0], 128),             // ::
    ([0, 0, 0, 0, 0, 0xffff, 0, 0], 96),         // ::ffff:0:0/96 (v4-mapped)
    ([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48),      // 64:ff9b:1::/48
    ([0x100, 0, 0, 0, 0, 0, 0, 0], 64),          // 100::/64
    ([0x2001, 0, 0, 0, 0, 0, 0, 0], 23),         // 2001::/23
    ([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32),     // 2001:db8::/32
    ([0x2002, 0, 0, 0, 0, 0, 0, 0], 16),         // 2002::/16
    ([0x3fff, 0, 0, 0, 0, 0, 0, 0], 20),         // 3fff::/20
    ([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7),          // fc00::/7 unique local
    ([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10),         // fe80::/10 link local
    ([0, 0, 0, 0, 0, 0, 0, 0], 8),               // ::/8
    ([0x100, 0, 0, 0, 0, 0, 0, 0], 8),           // 100::/8
    ([0x200, 0, 0, 0, 0, 0, 0, 0], 7),           // 200::/7
    ([0x400, 0, 0, 0, 0, 0, 0, 0], 6),           // 400::/6
    ([0x800, 0, 0, 0, 0, 0, 0, 0], 5),           // 800::/5
    ([0x1000, 0, 0, 0, 0, 0, 0, 0], 4),          // 1000::/4
    ([0x4000, 0, 0, 0, 0, 0, 0, 0], 3),          // 4000::/3
    ([0x6000, 0, 0, 0, 0, 0, 0, 0], 3),          // 6000::/3
    ([0x8000, 0, 0, 0, 0, 0, 0, 0], 3),          // 8000::/3
    ([0xa000, 0, 0, 0, 0, 0, 0, 0], 3),          // a000::/3
    ([0xc000, 0, 0, 0, 0, 0, 0, 0], 3),          // c000::/3
    ([0xe000, 0, 0, 0, 0, 0, 0, 0], 4),          // e000::/4
    ([0xf000, 0, 0, 0, 0, 0, 0, 0], 5),          // f000::/5
    ([0xf800, 0, 0, 0, 0, 0, 0, 0], 6),          // f800::/6
    ([0xfe00, 0, 0, 0, 0, 0, 0, 0], 9),          // fe00::/9
    ([0xff00, 0, 0, 0, 0, 0, 0, 0], 8),          // ff00::/8 multicast
];
/// Globally reachable exceptions inside the blocked v6 blocks.
const IPV6_ALLOWED: &[([u16; 8], u8)] = &[
    ([0x2001, 1, 0, 0, 0, 0, 0, 1], 128),
    ([0x2001, 1, 0, 0, 0, 0, 0, 2], 128),
    ([0x2001, 3, 0, 0, 0, 0, 0, 0], 32),
    ([0x2001, 4, 0x112, 0, 0, 0, 0, 0], 48),
    ([0x2001, 0x20, 0, 0, 0, 0, 0, 0], 28),
    ([0x2001, 0x30, 0, 0, 0, 0, 0, 0], 28),
];

fn ipv6_blocks() -> &'static (Vec<(u128, u8)>, Vec<(u128, u8)>) {
    static BLOCKS: OnceLock<(Vec<(u128, u8)>, Vec<(u128, u8)>)> = OnceLock::new();
    BLOCKS.get_or_init(|| {
        let convert = |table: &[([u16; 8], u8)]| -> Vec<(u128, u8)> {
            table.iter().map(|(segments, prefix)| (ipv6_bits(&std::net::Ipv6Addr::from(*segments)), *prefix)).collect()
        };
        (convert(IPV6_BLOCKED), convert(IPV6_ALLOWED))
    })
}

pub fn is_private_addr(addr: std::net::IpAddr) -> bool {
    match addr {
        std::net::IpAddr::V4(v4) => is_private_v4(v4),
        std::net::IpAddr::V6(v6) => {
            // v4-mapped addresses keep the IPv4 semantics (ipaddress does this too)
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_v4(v4);
            }
            let bits = ipv6_bits(&v6);
            let (blocked, allowed) = ipv6_blocks();
            in_block(bits, blocked, 128) && !in_block(bits, allowed, 128)
        }
    }
}

/// AnySearch provider: POST {query, max_results}.
pub fn web_search(
    query: &str,
    max_results: i64,
    url: Option<&str>,
    api_key_env: Option<&str>,
    include_content: bool,
) -> Result<Json, String> {
    let url = url.unwrap_or("https://api.anysearch.com/v1/search");
    let key = api_key_env.and_then(|env| std::env::var(env).ok());
    let count = max_results.clamp(1, 20);
    let mut request = ureq::post(url)
        .timeout(std::time::Duration::from_secs(30))
        .set("content-type", "application/json");
    if let Some(key) = key {
        request = request.set("authorization", &format!("Bearer {key}"));
    }
    let response = request
        .send_string(&json!({"query": query, "max_results": count}).to_string())
        .map_err(|e| format!("web_search failed: {e}"))?;
    let payload: Json = response.into_json().map_err(|e| format!("web_search bad json: {e}"))?;
    Ok(parse_search_response(&payload, query, count, include_content))
}

fn parse_search_response(payload: &Json, query: &str, count: i64, include_content: bool) -> Json {
    let results: Vec<Json> = payload
        .get("data")
        .and_then(|d| d.get("results"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .take(count as usize)
        .map(|item| {
            let mut entry = json!({
                "title": item.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                "url": item.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                "snippet": item.get("snippet").and_then(|v| v.as_str()).unwrap_or(""),
                "fetched_at": iso_now(),
            });
            if include_content {
                if let Some(content) = item.get("content") {
                    entry["content"] = content.clone();
                }
            }
            entry
        })
        .collect();
    json!({"query": query, "provider": "anysearch", "results": results})
}

type UrlGuard = std::sync::Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

/// GET with redirects followed manually so every hop passes the SSRF guard
/// (ureq's built-in following would only guard the first URL, P1-2). The
/// custom resolver vets DNS answers through the same guard and hands only the
/// passing addresses to the connector: a rebind between guard and connect
/// cannot reroute the request, and each redirect hop re-resolves through it.
fn http_get_guarded(url: &str, guard: &UrlGuard) -> Result<ureq::Response, String> {
    let resolver = {
        let guard = guard.clone();
        move |netloc: &str| -> std::io::Result<Vec<std::net::SocketAddr>> {
            use std::net::ToSocketAddrs;
            let host = match netloc.strip_prefix('[') {
                Some(rest) => rest.split(']').next().unwrap_or(""),
                None => netloc.rsplit_once(':').map(|(h, _)| h).unwrap_or(netloc),
            };
            let addrs: Vec<std::net::SocketAddr> = netloc.to_socket_addrs()?.collect();
            // a literal was already vetted by the guard when it approved the URL
            if host.parse::<std::net::IpAddr>().is_ok() {
                return Ok(addrs);
            }
            let vetted: Vec<std::net::SocketAddr> = addrs
                .into_iter()
                .filter(|addr| {
                    let check = match addr.ip() {
                        std::net::IpAddr::V4(ip) => format!("http://{ip}"),
                        std::net::IpAddr::V6(ip) => format!("http://[{ip}]"),
                    };
                    guard(&check).is_ok()
                })
                .collect();
            if vetted.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("refusing private address for {host}"),
                ));
            }
            Ok(vetted)
        }
    };
    let agent = ureq::AgentBuilder::new().redirects(0).resolver(resolver).build();
    let mut current = guard(url)?;
    for _ in 0..10 {
        let response = agent
            .get(&current)
            .timeout(std::time::Duration::from_secs(30))
            .call()
            .map_err(|e| format!("web_fetch failed: {e}"))?;
        if !(300..400).contains(&response.status()) {
            return Ok(response);
        }
        let location = response.header("location").ok_or("web_fetch: redirect without location")?;
        let next = url::Url::parse(&current)
            .and_then(|base| base.join(location))
            .map_err(|e| format!("web_fetch: bad redirect target {location:?}: {e}"))?;
        current = guard(next.as_str())?;
    }
    Err("web_fetch: too many redirects".into())
}

/// Guarded GET, title + readable text body.
pub fn web_fetch(url: &str, max_bytes: usize, allow_private: bool) -> Result<Json, String> {
    let guard: UrlGuard = std::sync::Arc::new(move |u: &str| if allow_private { Ok(u.to_string()) } else { guard_url(u) });
    let response = http_get_guarded(url, &guard)?;
    let content_type = response.header("content-type").unwrap_or("").to_string();
    let final_url = response.get_url().to_string();
    let body = response.into_string().map_err(|e| format!("web_fetch failed: {e}"))?;
    if !content_type.contains("html") && !content_type.contains("text/") {
        return Err(format!("unsupported content type {content_type:?} for {url:?}"));
    }
    let truncated_by_bytes = body.len() > max_bytes;
    let body: String = body.chars().take(max_bytes).collect();
    let (title, text) = strip_html(&body);
    let capped: String = text.chars().take(200_000).collect();
    Ok(json!({
        "title": if title.is_empty() { url.to_string() } else { title },
        "url": final_url,
        "fetched_at": iso_now(),
        "content": capped,
        "truncated": truncated_by_bytes || text.chars().count() > 200_000,
    }))
}

/// Minimal HTML → (title, visible text), skipping script/style/noscript.
fn strip_html(html: &str) -> (String, String) {
    let mut out = String::new();
    let mut rest = html;
    loop {
        // earliest of the skipped-element openers, not just the first match found
        let Some((start, opener)) = ["<script", "<style", "<noscript"]
            .iter()
            .filter_map(|opener| rest.find(opener).map(|index| (index, *opener)))
            .min_by_key(|(index, _)| *index)
        else {
            break;
        };
        out.push_str(&rest[..start]);
        let close = format!("</{}>", &opener[1..]);
        match rest[start..].find(&close) {
            Some(end) => rest = &rest[start + end + close.len()..],
            None => break,
        }
    }
    out.push_str(rest);
    let mut text = String::new();
    let mut title = String::new();
    let mut in_tag = false;
    let mut in_title = false;
    let mut tag = String::new();
    for ch in out.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
                if !in_title {
                    in_title = false;
                }
            }
            '>' => {
                in_tag = false;
                if tag.trim().eq_ignore_ascii_case("title") {
                    in_title = true;
                } else if tag.trim().eq_ignore_ascii_case("/title") {
                    in_title = false;
                }
                text.push(' ');
            }
            c if in_tag => tag.push(c),
            c if in_title => title.push(c),
            c => text.push(c),
        }
    }
    (title.split_whitespace().collect::<Vec<_>>().join(" "), text.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn iso_now() -> String {
    iso8601(teamagents_core::models::now() as i64)
}

/// "YYYY-MM-DDTHH:MM:SSZ" (UTC), no date library needed.
pub fn iso8601(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let (hour, minute, second) = (
        (seconds.rem_euclid(86_400)) / 3600,
        (seconds.rem_euclid(3600)) / 60,
        seconds.rem_euclid(60),
    );
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_capture_spools_bounded_previews_and_reports_storage_errors() {
        let dir = std::env::temp_dir().join(format!("ta-output-{}", uuid::Uuid::new_v4()));
        let mut first = OutputSink::new(Some(&dir)).unwrap();
        let second = OutputSink::new(Some(&dir)).unwrap();
        assert_ne!(first.reference, second.reference);
        for _ in 0..100 { first.append(&[b'x'; 8192]); }
        assert_eq!(first.head.len(), MAX_OUTPUT);
        assert_eq!(first.total, 819200);
        assert_eq!(first.artifact.as_ref().unwrap().metadata().unwrap().len(), 819200);
        assert!(first.finish(false).unwrap().contains("full output: /artifacts/exec-"));
        first.artifact = Some(std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap());
        first.append(b"cannot save");
        assert!(first.finish(false).unwrap_err().contains("output artifact write failed"));
        let not_a_dir = dir.join("file");
        std::fs::write(&not_a_dir, "x").unwrap();
        assert!(OutputSink::new(Some(&not_a_dir)).err().unwrap().contains("artifact directory"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn path_lock_serializes_two_writers() {
        let dir = std::env::temp_dir().join(format!("ta-lock-{}", uuid::Uuid::new_v4()));
        // like production: the workspace and the lock directory are different trees
        let locks = dir.join("locks");
        let workspace = dir.join("work");
        std::fs::create_dir_all(&workspace).unwrap();
        let target = workspace.join("shared.txt");
        let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));

        let (first_order, first_dir, first_target) = (order.clone(), locks.clone(), target.clone());
        let first = std::thread::spawn(move || {
            with_path_lock(Some(&first_dir), &first_target, || {
                first_order.lock().unwrap().push("first-in".into());
                std::thread::sleep(std::time::Duration::from_millis(300));
                first_order.lock().unwrap().push("first-out".into());
                Ok(())
            })
            .unwrap()
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        let (second_order, second_dir, second_target) = (order.clone(), locks.clone(), target.clone());
        let second = std::thread::spawn(move || {
            with_path_lock(Some(&second_dir), &second_target, || {
                second_order.lock().unwrap().push("second-in".into());
                Ok(())
            })
            .unwrap()
        });
        first.join().unwrap();
        second.join().unwrap();
        assert_eq!(
            order.lock().unwrap().clone(),
            vec!["first-in", "first-out", "second-in"],
            "the second writer must wait for the first to finish"
        );

        // lock files stay out of the project directory
        let workspace_files: Vec<String> = std::fs::read_dir(&workspace).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(workspace_files.iter().all(|name| !name.ends_with(".lock")), "no lock artifacts in the project: {workspace_files:?}");
        let lock_files: Vec<String> = std::fs::read_dir(&locks).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(lock_files.len(), 1, "one stable lock file per target path: {lock_files:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn artifacts_are_pruned_to_the_directory_budget() {
        let dir = std::env::temp_dir().join(format!("ta-artifact-prune-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for index in 0..5 {
            let path = dir.join(format!("exec-{index}.log"));
            std::fs::write(&path, vec![b'x'; 100]).unwrap();
            // distinct mtimes: index 0 is the oldest
            let stamp = std::time::SystemTime::now() - std::time::Duration::from_secs(60 - index as u64);
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_modified(stamp).unwrap();
        }
        assert_eq!(prune_artifacts(&dir, 250), 3, "oldest files go first");
        let kept: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept.iter().all(|name| name == "exec-3.log" || name == "exec-4.log"), "{kept:?}");
        // under budget: nothing is touched, and unrelated files are never candidates
        std::fs::write(dir.join("keep.txt"), vec![b'y'; 4096]).unwrap();
        assert_eq!(prune_artifacts(&dir, 4096), 0);
        assert!(dir.join("keep.txt").exists(), "only exec-*.log files are pruned");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shell_artifact_stops_at_the_size_cap() {
        let dir = std::env::temp_dir().join(format!("ta-artifact-cap-{}", uuid::Uuid::new_v4()));
        let mut sink = OutputSink { cap: 3 * 1024 * 1024, ..OutputSink::new(Some(&dir)).unwrap() };
        for _ in 0..64 { sink.append(&[b'x'; 65536]); }
        assert_eq!(sink.total, 4 * 1024 * 1024);
        assert_eq!(sink.written, 3 * 1024 * 1024);
        assert_eq!(sink.artifact.as_ref().unwrap().metadata().unwrap().len(), 3 * 1024 * 1024);
        let text = sink.finish(false).unwrap();
        assert!(text.contains("artifact truncated at 3 MiB"), "{text}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancelled_shell_keeps_partial_output_and_its_artifact() {
        if !bwrap_available() { return; }
        let dir = std::env::temp_dir().join(format!("ta-cancel-output-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let artifacts = dir.join("artifacts");
        let control = Arc::new(TurnControl::default());
        let child_control = control.clone();
        let child_dir = dir.clone();
        let child_artifacts = artifacts.clone();
        let job = std::thread::spawn(move || shell_run_with_control(
            "printf before-cancel; touch ready; sleep 30; touch should-not-exist", &child_dir, 40, false, Some(&child_artifacts), &child_control));
        let started = Instant::now();
        while !dir.join("ready").exists() && !job.is_finished() && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(dir.join("ready").exists(), "sandbox did not start the command");
        control.cancel();
        let err = job.join().unwrap().unwrap_err();
        assert!(err.contains("turn interrupted") && err.contains("before-cancel") && err.contains("/artifacts/exec-"), "{err}");
        let files: Vec<_> = std::fs::read_dir(&artifacts).unwrap().map(|entry| entry.unwrap().path()).collect();
        assert_eq!(files.len(), 1);
        assert_eq!(std::fs::read_to_string(&files[0]).unwrap(), "before-cancel");
        assert!(!dir.join("should-not-exist").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fallback_glob_never_follows_directory_links_and_checks_cancellation() {
        let dir = std::env::temp_dir().join(format!("ta-glob-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested/file.txt"), "ok").unwrap();
        std::os::unix::fs::symlink("/etc", dir.join("escape")).unwrap();
        let control = TurnControl::default();
        let mut hits = vec![];
        glob_walk(&dir, &dir, "**/*", &mut hits, &control).unwrap();
        assert!(hits.iter().any(|hit| hit == "nested/file.txt"));
        assert!(!hits.iter().any(|hit| hit.starts_with("escape")));
        control.cancel();
        assert!(glob_walk(&dir, &dir, "**/*", &mut hits, &control).unwrap_err().contains("interrupted"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Serve `response` to the first connection, then report our URL.
    fn serve_once(response: String) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        url
    }

    #[test]
    fn web_fetch_guards_every_redirect_hop() {
        // P1-2: a redirect to a target the guard refuses must stop the fetch
        let second = serve_once("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 6\r\n\r\nsecret".into());
        let first = serve_once(format!("HTTP/1.1 302 Found\r\nlocation: {second}\r\ncontent-length: 0\r\n\r\n"));
        let allowed = first.clone();
        let guard: UrlGuard = std::sync::Arc::new(move |u: &str| {
            if u == allowed { Ok(u.to_string()) } else { Err(format!("private address refused: {u}")) }
        });
        let err = http_get_guarded(&first, &guard).unwrap_err();
        assert!(err.contains("private address"), "{err}");
    }

    #[test]
    fn resolver_refuses_dns_answers_the_guard_rejects() {
        // DNS-rebinding TOCTOU: the name-based guard accepts "localhost", but
        // its loopback answer must not reach the connector once re-resolved
        let guard: UrlGuard = std::sync::Arc::new(|u: &str| {
            if u.contains("localhost") { Ok(u.to_string()) } else { Err(format!("private address refused: {u}")) }
        });
        let err = http_get_guarded("http://localhost:1/", &guard).unwrap_err();
        assert!(err.contains("refusing private address"), "{err}");
    }

    #[test]
    fn resolver_connects_to_the_vetted_answer() {
        // a name (not a literal) is resolved, vetted, and the vetted address
        // is the one dialed
        let server = serve_once("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 2\r\n\r\nhi".into());
        let url = server.replace("127.0.0.1", "localhost");
        let guard: UrlGuard = std::sync::Arc::new(|u: &str| Ok(u.to_string()));
        let response = http_get_guarded(&url, &guard).unwrap();
        assert_eq!(response.status(), 200);
    }

    #[test]
    fn web_fetch_follows_guarded_redirect_chain() {
        // both hops pass the guard -> the final hop's body and URL are returned
        let second = serve_once("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 5\r\n\r\nhello".into());
        let first = serve_once(format!("HTTP/1.1 302 Found\r\nlocation: {second}\r\ncontent-length: 0\r\n\r\n"));
        let result = web_fetch(&first, 1_000_000, true).expect("allow_private serves the local chain");
        assert_eq!(result["content"], json!("hello"));
        assert_eq!(result["url"], json!(second));
    }

    #[test]
    fn web_fetch_stops_after_ten_redirects() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/loop", listener.local_addr().unwrap());
        let location = url.clone();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(
                    format!("HTTP/1.1 302 Found\r\nlocation: {location}\r\ncontent-length: 0\r\n\r\n").as_bytes(),
                );
            }
        });
        let err = web_fetch(&url, 1_000_000, true).unwrap_err();
        assert!(err.contains("too many redirects"), "{err}");
    }

    #[test]
    fn skill_tool_searches_and_reads_registry() {
        let root = std::env::temp_dir().join(format!("ta-skilltool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("ponytail")).unwrap();
        std::fs::create_dir_all(root.join("scanpy")).unwrap();
        std::fs::write(
            root.join("ponytail/SKILL.md"),
            "---\nname: ponytail\ndescription: laziest solution that works\n---\nponytail body",
        )
        .unwrap();
        std::fs::write(
            root.join("scanpy/SKILL.md"),
            "---\nname: scanpy\ndescription: single-cell analysis\n---\nscanpy body",
        )
        .unwrap();
        let mut catalog = teamagents_core::models::UserConfig::default();
        catalog.skills_paths = vec![root.to_string_lossy().into_owned()];

        // search hits name and description, ranks multi-word matches first
        let hits = skill_tool(&catalog, &json!({"action": "search", "query": "single-cell"})).unwrap();
        let hits = hits.as_str().unwrap();
        assert!(hits.contains("scanpy") && !hits.contains("ponytail"), "{hits}");
        for description in [">\n  single-cell\n  analysis", "|\n  single-cell\n  analysis", "'single-cell analysis'"] {
            std::fs::write(root.join("scanpy/SKILL.md"), format!("---\nname: scanpy\ndescription: {description}\n---\nscanpy body")).unwrap();
            let hits = skill_tool(&catalog, &json!({"action":"search", "query":"single-cell"})).unwrap();
            assert!(hits.as_str().unwrap().contains("scanpy"), "{description}: {hits}");
        }
        // empty query lists everything
        let all = skill_tool(&catalog, &json!({"action": "search", "query": ""})).unwrap();
        assert!(all.as_str().unwrap().contains("ponytail"));
        // read returns the full body; unknown names fail with the known list
        let body = skill_tool(&catalog, &json!({"action": "read", "name": "ponytail"})).unwrap();
        assert!(body.as_str().unwrap().contains("ponytail body"));
        let err = skill_tool(&catalog, &json!({"action": "read", "name": "nope"})).unwrap_err();
        assert!(err.contains("scanpy"), "{err}");
        // an empty registry and a bad action are explicit errors
        let empty = teamagents_core::models::UserConfig::default();
        assert!(skill_tool(&empty, &json!({"action": "search"})).is_err());
        assert!(skill_tool(&catalog, &json!({"action": "delete"})).is_err());
        // executor enforces the binding (bound = authorized, like web tools)
        let executor = member_executor(root.clone(), catalog, vec![], None);
        assert!(executor("skill", &json!({"action": "search"})).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn workspace_paths_stay_inside_root() {
        let root = std::env::temp_dir().join(format!("ta-tools-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/file.txt"), "hello").unwrap();
        assert!(resolve_in_root(&root, "sub/file.txt").is_ok());
        assert!(resolve_in_root(&root, "../etc/passwd").is_err());
        assert!(resolve_in_root(&root, "/etc/passwd").is_err());
        let executor = workspace_executor(root.clone(), None);
        assert_eq!(executor("read_file", &json!({"path": "sub/file.txt"})).unwrap(), json!("hello"));
        assert!(executor("read_file", &json!({"path": "../../etc/hosts"})).is_err());
        let listing = executor("ls", &json!({"path": "."})).unwrap();
        assert!(listing.as_str().unwrap().contains("sub/"));
        let globbed = executor("glob", &json!({"pattern": "**/*.txt"})).unwrap();
        assert!(globbed.as_str().unwrap().contains("sub/file.txt"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn guard_url_blocks_private_targets() {
        assert!(guard_url("http://127.0.0.1/x").is_err());
        assert!(guard_url("http://10.0.0.5/x").is_err());
        assert!(guard_url("http://localhost/x").is_err());
        assert!(guard_url("ftp://example.com/x").is_err());
        assert_eq!(guard_url("https://93.184.216.34/x").unwrap(), "https://93.184.216.34/x");
        // reserved/multicast/documentation/benchmarking blocks
        for blocked in [
            "http://224.0.0.1/",
            "http://198.18.0.1/",
            "http://192.0.2.1/",
            "http://198.51.100.7/",
            "http://203.0.113.9/",
            "http://240.0.0.1/",
            "http://0.0.0.0/",
            "http://169.254.169.254/latest/meta-data/",
        ] {
            assert!(guard_url(blocked).is_err(), "{blocked} must be refused");
        }
        // bracketed IPv6 literals are parsed, not rejected as unresolvable
        assert!(guard_url("http://[::1]:8000/x").unwrap_err().contains("private address"));
        assert!(guard_url("http://[fe80::1]/").is_err());
        assert!(guard_url("http://[fc00::1]/").is_err());
        assert!(guard_url("http://[::ffff:127.0.0.1]:8000/x").is_err());
        // …and public v6 targets stay usable
        assert_eq!(guard_url("https://[2606:4700::1]/x").unwrap(), "https://[2606:4700::1]/x");
        // userinfo does not confuse host extraction
        assert!(guard_url("http://user:pass@127.0.0.1/x").is_err());
    }

    #[test]
    fn bwrap_argv_is_stable_and_runs_isolated() {
        let dir = std::env::temp_dir();
        let argv = bwrap_argv(&dir, false, "echo hi");
        assert!(argv.iter().any(|a| a == "--unshare-net"), "network off by default");
        assert!(argv.windows(3).any(|w| w == ["--ro-bind", "/usr", "/usr"]));
        assert!(argv.windows(3).any(|w| w == ["--symlink", "usr/bin", "/bin"]));
        assert!(argv.windows(2).any(|w| w[0] == "--chdir" && w[1] == dir.to_string_lossy()));
        assert_eq!(&argv[argv.len() - 4..], ["--", "/bin/bash", "-lc", "echo hi"]);
        assert!(!bwrap_argv(&dir, true, "x").iter().any(|a| a == "--unshare-net"));
        if bwrap_available() {
            let out = shell_run("echo isolated-ok && id -u", &dir, 30, false, None).unwrap();
            assert!(out.contains("isolated-ok"), "{out}");
        }
    }

    #[test]
    fn sandbox_builds_with_the_host_toolchain() {
        if !bwrap_available() || toolchain_mounts().is_empty() || which("cargo").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("ta-toolchain-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "#[test]\nfn adds() { assert_eq!(1 + 1, 2); }\n").unwrap();
        let out = shell_run("cargo test --offline 2>&1 | tail -30; echo cargo-rc=${PIPESTATUS[0]}", &dir, 300, false, None).unwrap();
        assert!(out.contains("cargo-rc=0") && out.contains("1 passed"), "cargo unusable inside the sandbox: {out}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn web_shapes_are_stable_and_bindings_are_required() {
        let payload = json!({"code": 0, "data": {"results": [
            {"title": "T", "url": "https://x", "snippet": "S", "content": "BODY"},
        ]}});
        let parsed = parse_search_response(&payload, "q", 5, false);
        assert_eq!(parsed["provider"], "anysearch");
        assert_eq!(parsed["results"][0]["title"], "T");
        assert!(parsed["results"][0].get("content").is_none());
        assert_eq!(parse_search_response(&payload, "q", 5, true)["results"][0]["content"], "BODY");

        let (title, text) = strip_html(
            "<html><head><title>Hi</title><style>x{}</style></head><body><p>Hello  world</p><script>evil()</script></body></html>",
        );
        assert_eq!(title, "Hi");
        assert_eq!(text, "Hello world");
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_700_000_000), "2023-11-14T22:13:20Z");

        // web tools resolve their binding from the catalog ("no binding" = no capability)
        let catalog = teamagents_core::models::UserConfig::default();
        let executor = member_executor(std::env::temp_dir(), catalog.clone(), vec![], None);
        assert!(executor("web_search", &json!({"query": "q"})).is_err());
        // a member that only binds web still needs a configured provider
        let executor = member_executor(std::env::temp_dir(), catalog.clone(), vec!["web".into()], None);
        assert!(executor("web_search", &json!({"query": "q"})).unwrap_err().contains("not bound"));
        let mut catalog = catalog;
        catalog.tools.insert(
            "web".into(),
            serde_json::from_value(json!({"kind": "web_search", "provider": "unknown"})).unwrap(),
        );
        let executor = member_executor(std::env::temp_dir(), catalog, vec!["web".into()], None);
        assert!(executor("web_search", &json!({"query": "q"})).unwrap_err().contains("unsupported"));
    }


    // -- P1-2 / P2-6 regression tests (review 2026-09-14) --

    fn tiny_server(reply: String) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            while let Ok((mut conn, _)) = listener.accept() {
                use std::io::{Read, Write};
                let mut buf = [0u8; 2048];
                let _ = conn.read(&mut buf);
                if conn.write_all(reply.as_bytes()).is_err() {
                    return;
                }
            }
        });
        base
    }

    #[test]
    fn redirect_chain_is_guarded_at_every_hop() {
        let hop_b = tiny_server("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 2\r\n\r\nhi".into());
        let hop_a = tiny_server(format!(
            "HTTP/1.1 302 Found\r\nlocation: {hop_b}/secret\r\ncontent-length: 0\r\n\r\n"
        ));
        let hop_blocked = tiny_server(
            "HTTP/1.1 302 Found\r\nlocation: http://blocked.invalid/\r\ncontent-length: 0\r\n\r\n".into(),
        );

        // happy path: an allowed redirect is followed to its 200
        let (a, b) = (hop_a.clone(), hop_b.clone());
        let allow_local: UrlGuard = std::sync::Arc::new(move |u: &str| {
            if u.starts_with(&a) || u.starts_with(&b) {
                Ok(u.to_string())
            } else {
                Err(format!("blocked {u}"))
            }
        });
        let response = http_get_guarded(&format!("{hop_a}/x"), &allow_local).unwrap();
        assert_eq!(response.status(), 200);

        // the guard must also run on the redirect target, not just hop one
        let blocked_base = hop_blocked.clone();
        let allow_first_only: UrlGuard = std::sync::Arc::new(move |u: &str| {
            if u.starts_with(&blocked_base) {
                Ok(u.to_string())
            } else {
                Err(format!("blocked {u}"))
            }
        });
        let err = http_get_guarded(&format!("{hop_blocked}/x"), &allow_first_only).unwrap_err();
        assert!(err.contains("blocked"), "redirect target escaped the guard: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn skill_candidates_reject_symlink_escapes() {
        let base = std::env::temp_dir().join(format!("ta-skill-test-{}", std::process::id()));
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(root.join("real/SKILL.md"), "real").unwrap();
        std::fs::write(outside.join("SKILL.md"), "outside").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linkdir")).unwrap();
        // a symlinked SKILL.md inside a real directory
        std::fs::create_dir_all(root.join("linked")).unwrap();
        std::os::unix::fs::symlink(outside.join("SKILL.md"), root.join("linked/SKILL.md")).unwrap();

        let names: Vec<String> = skill_candidates(&root).into_iter().map(|(n, _)| n).collect();
        assert!(names.contains(&"real".to_string()), "real skill missing: {names:?}");
        assert!(!names.contains(&"linkdir".to_string()), "symlinked dir escaped: {names:?}");
        assert!(!names.contains(&"linked".to_string()), "symlinked SKILL.md escaped: {names:?}");
        std::fs::remove_dir_all(&base).ok();
    }

}
