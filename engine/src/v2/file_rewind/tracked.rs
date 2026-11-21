//! A delivery baseline contains the worktree bytes of regular files listed in the Git index.
//! Git only enumerates names. Confined file handles read the working copies, including dirty ones.
use super::{current_bytes, READ_BUDGET};
use std::{
    collections::BTreeMap,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Component, Path},
    process::{Command, Stdio},
};
use teamagents_core::v2::control::rewind::FileSnapshot;

const INDEX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PATHS: usize = 8192;

fn paths(root: &Path) -> Result<Vec<String>, String> {
    let mut child = Command::new("/usr/bin/git")
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["--no-optional-locks", "-c", "core.fsmonitor=false", "ls-files", "--stage", "-z", "--", "."])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("enumerate Git index: {e}"))?;
    let mut raw = Vec::new();
    let read = child.stdout.take().unwrap().take(INDEX_BYTES + 1).read_to_end(&mut raw);
    if read.is_err() || raw.len() as u64 > INDEX_BYTES {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    read.map_err(|e| e.to_string())?;
    if raw.len() as u64 > INDEX_BYTES {
        return Err("tracked path list exceeds the snapshot limit".into());
    }
    if !status.success() {
        return Err("Git could not enumerate the workspace index".into());
    }
    let mut paths = Vec::new();
    for entry in raw.split(|c| *c == 0).filter(|entry| !entry.is_empty()) {
        let (header, path) = std::str::from_utf8(entry)
            .map_err(|_| "unsupported non-UTF-8 tracked path")?
            .split_once('\t')
            .ok_or("invalid Git index entry")?;
        if !header.ends_with(" 0") {
            return Err("resolve Git index conflicts before using workspace rewind".into());
        }
        // Gitlinks and symbolic links are outside the regular-file restore set.
        if !header.starts_with("100644 ") && !header.starts_with("100755 ") {
            continue;
        }
        if path.is_empty()
            || path.chars().any(char::is_control)
            || !Path::new(path).components().all(|c| matches!(c,Component::Normal(name) if name!=".git"))
        {
            return Err("unsafe tracked path".into());
        }
        if paths.len() == MAX_PATHS {
            return Err("too many tracked files for one snapshot".into());
        }
        paths.push(path.to_owned());
    }
    Ok(paths)
}

pub(super) fn capture(root: &Path, state: &Path) -> Result<FileSnapshot, String> {
    let dir = super::super::attachments::open_confined(root, Path::new("."), true)?;
    let meta = dir.metadata().map_err(|e| e.to_string())?;
    let identity = format!("{}:{}", meta.dev(), meta.ino());
    let tracked = paths(root)?;
    let mut files = BTreeMap::new();
    let mut stamps = Vec::with_capacity(tracked.len());
    let mut budget = READ_BUDGET;
    for path in &tracked {
        if root.join(path).starts_with(state) {
            return Err("session storage is inside the tracked file set".into());
        }
        let (bytes, stamp) = current_bytes(root, path, &mut budget)?;
        stamps.push(stamp);
        files.insert(path.clone(), bytes);
    }
    if tracked != paths(root)? {
        return Err("Git index changed during workspace capture".into());
    }
    // Reopen by name so replacement of an already-read file invalidates the capture too.
    for (path, expected) in tracked.iter().zip(stamps) {
        let current = super::super::attachments::open_confined(root, Path::new(path), false)?;
        if super::stamp(&current)? != expected {
            return Err("workspace changed during snapshot capture".into());
        }
    }
    Ok(FileSnapshot { root: identity, files })
}
