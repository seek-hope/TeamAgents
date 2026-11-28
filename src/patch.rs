//! Codex V4A `apply_patch` parser and preflighted sandbox-pinned workspace patcher.
//! Preflight prevents malformed or escaping patches from touching the workspace; it is
//! not a filesystem transaction, so a failure during publication can leave earlier
//! operations applied.
use crate::{sandbox, Result};
use std::{
    collections::HashSet,
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

const FILE_LIMIT: usize = 8 * 1024 * 1024;
const BEGIN_PATCH: &str = "*** Begin Patch";
const END_PATCH: &str = "*** End Patch";
const ADD_FILE: &str = "*** Add File: ";
const DELETE_FILE: &str = "*** Delete File: ";
const UPDATE_FILE: &str = "*** Update File: ";
const MOVE_TO: &str = "*** Move to: ";
const EOF_MARKER: &str = "*** End of File";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpdateChunk {
    pub context: Option<String>,
    pub lines: Vec<HunkLine>,
    pub is_end_of_file: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PatchOp {
    Add { path: String, lines: Vec<String> },
    Delete { path: String },
    Update { path: String, move_to: Option<String>, chunks: Vec<UpdateChunk> },
}

pub(crate) fn parse_patch(raw: &str) -> Result<Vec<PatchOp>> {
    let lines: Vec<&str> = raw.lines().collect();
    let first = lines.iter().position(|l| !l.trim().is_empty()).ok_or("apply_patch input is empty")?;
    let last = lines.iter().rposition(|l| !l.trim().is_empty()).ok_or("apply_patch input is empty")?;
    if lines[first].trim() != BEGIN_PATCH || lines[last].trim() != END_PATCH || first >= last {
        return Err("apply_patch must start with `*** Begin Patch` and end with `*** End Patch`".into());
    }
    let body = &lines[first + 1..last];
    let mut ops = Vec::new();
    let mut i = 0usize;
    while i < body.len() {
        let line = body[i].trim_end_matches('\r');
        if line.is_empty() {
            i += 1;
            continue;
        }
        if let Some(path) = line.strip_prefix(ADD_FILE) {
            let path = clean_path_arg(path, "Add File")?;
            i += 1;
            let mut add_lines = Vec::new();
            while i < body.len() {
                let l = body[i].trim_end_matches('\r');
                if is_file_header(l) {
                    break;
                }
                if let Some(text) = l.strip_prefix('+') {
                    add_lines.push(text.to_owned());
                } else {
                    return Err(format!("invalid line in Add File block for {path}: expected leading '+'"));
                }
                i += 1;
            }
            ops.push(PatchOp::Add { path, lines: add_lines });
        } else if let Some(path) = line.strip_prefix(DELETE_FILE) {
            let path = clean_path_arg(path, "Delete File")?;
            i += 1;
            ops.push(PatchOp::Delete { path });
        } else if let Some(path) = line.strip_prefix(UPDATE_FILE) {
            let path = clean_path_arg(path, "Update File")?;
            i += 1;
            let mut move_to = None;
            if i < body.len() {
                if let Some(dst) = body[i].trim_end_matches('\r').strip_prefix(MOVE_TO) {
                    move_to = Some(clean_path_arg(dst, "Move to")?);
                    i += 1;
                }
            }
            let mut chunks = Vec::new();
            while i < body.len() {
                let l = body[i].trim_end_matches('\r');
                if is_file_header(l) {
                    break;
                }
                if l.is_empty() {
                    i += 1;
                    continue;
                }
                let context = if l == "@@" {
                    None
                } else if let Some(rest) = l.strip_prefix("@@ ") {
                    Some(rest.to_owned())
                } else {
                    return Err(format!("expected `@@` hunk header in Update File {path}, got: {l}"));
                };
                i += 1;
                let mut hunk_lines = Vec::new();
                let mut eof = false;
                while i < body.len() {
                    let hl = body[i].trim_end_matches('\r');
                    if hl == EOF_MARKER {
                        eof = true;
                        i += 1;
                        break;
                    }
                    if hl == "@@" || hl.starts_with("@@ ") || is_file_header(hl) {
                        break;
                    }
                    if let Some(rest) = hl.strip_prefix(' ') {
                        hunk_lines.push(HunkLine::Context(rest.to_owned()));
                    } else if let Some(rest) = hl.strip_prefix('-') {
                        hunk_lines.push(HunkLine::Remove(rest.to_owned()));
                    } else if let Some(rest) = hl.strip_prefix('+') {
                        hunk_lines.push(HunkLine::Add(rest.to_owned()));
                    } else if hl.is_empty() {
                        hunk_lines.push(HunkLine::Context(String::new()));
                    } else {
                        return Err(format!("invalid hunk line in Update File {path}: {hl}"));
                    }
                    i += 1;
                }
                if hunk_lines.is_empty() && context.is_none() {
                    return Err(format!("empty hunk in Update File {path}"));
                }
                chunks.push(UpdateChunk { context, lines: hunk_lines, is_end_of_file: eof });
            }
            if chunks.is_empty() && move_to.is_none() {
                return Err(format!("Update File block for {path} has no hunks or Move to"));
            }
            ops.push(PatchOp::Update { path, move_to, chunks });
        } else {
            return Err(format!("unexpected apply_patch line: {line}"));
        }
    }
    if ops.is_empty() {
        return Err("apply_patch contains no file operations".into());
    }
    Ok(ops)
}

fn is_file_header(line: &str) -> bool {
    line.starts_with(ADD_FILE) || line.starts_with(DELETE_FILE) || line.starts_with(UPDATE_FILE)
}

fn clean_path_arg(raw: &str, label: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("apply_patch {label} requires a relative path"));
    }
    Ok(trimmed.to_owned())
}

pub(crate) fn preview_patch(patch: &str) -> Vec<(char, String)> {
    let mut out = Vec::new();
    for line in patch.lines() {
        let l = line.trim_end_matches('\r');
        if l == BEGIN_PATCH || l == END_PATCH {
            continue;
        }
        if l.starts_with(ADD_FILE)
            || l.starts_with(DELETE_FILE)
            || l.starts_with(UPDATE_FILE)
            || l.starts_with(MOVE_TO)
            || l == "@@"
            || l.starts_with("@@ ")
        {
            out.push(('@', l.to_owned()));
        } else if l.starts_with('+') {
            out.push(('+', l.to_owned()));
        } else if l.starts_with('-') {
            out.push(('-', l.to_owned()));
        } else {
            out.push((' ', l.to_owned()));
        }
    }
    out
}

fn version(m: &Metadata) -> (u64, u64, u64, i64, i64) {
    (m.dev(), m.ino(), m.len(), m.ctime(), m.ctime_nsec())
}

fn load_regular_text(dir: &File, target: &Path) -> Result<(String, Metadata)> {
    let f = sandbox::open_regular(dir, target)?;
    let meta = f.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("expected a regular file".into());
    }
    if meta.len() > FILE_LIMIT as u64 {
        return Err("file exceeds 8 MiB limit".into());
    }
    let mut bytes = Vec::new();
    f.take(FILE_LIMIT as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("read file: {e}"))?;
    if bytes.len() > FILE_LIMIT {
        return Err("file exceeds 8 MiB limit".into());
    }
    Ok((String::from_utf8(bytes).map_err(|_| "file is not UTF-8 text")?, meta))
}

fn find_subsequence(haystack: &[String], needle: &[String], start: usize, eof: bool) -> Option<usize> {
    if needle.is_empty() {
        return Some(if eof { haystack.len() } else { start.min(haystack.len()) });
    }
    if needle.len() > haystack.len() {
        return None;
    }
    let matches_exact = |slice: &[String]| slice.iter().zip(needle.iter()).all(|(a, b)| a == b);
    let matches_rtrim = |slice: &[String]| slice.iter().zip(needle.iter()).all(|(a, b)| a.trim_end() == b.trim_end());
    let matches_trim = |slice: &[String]| slice.iter().zip(needle.iter()).all(|(a, b)| a.trim() == b.trim());

    if eof {
        let eof_idx = haystack.len() - needle.len();
        if eof_idx >= start && (matches_exact(&haystack[eof_idx..]) || matches_rtrim(&haystack[eof_idx..])) {
            return Some(eof_idx);
        }
    }
    for pass in [0u8, 1, 2] {
        let pred = |slice: &[String]| match pass {
            0 => matches_exact(slice),
            1 => matches_rtrim(slice),
            _ => matches_trim(slice),
        };
        if let Some(pos) = (start..=haystack.len() - needle.len()).find(|&idx| pred(&haystack[idx..idx + needle.len()]))
        {
            return Some(pos);
        }
        if let Some(pos) =
            (0..start.min(haystack.len() - needle.len() + 1)).find(|&idx| pred(&haystack[idx..idx + needle.len()]))
        {
            return Some(pos);
        }
    }
    None
}

fn apply_chunks(path: &str, original: &str, chunks: &[UpdateChunk]) -> Result<(String, usize, usize)> {
    let trailing_nl = original.is_empty() || original.ends_with('\n');
    let mut lines: Vec<String> = if original.is_empty() {
        Vec::new()
    } else {
        original.split('\n').map(|s| s.strip_suffix('\r').unwrap_or(s).to_owned()).collect()
    };
    if original.ends_with('\n') && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }

    let mut search_from = 0usize;
    let mut additions = 0usize;
    let mut deletions = 0usize;
    let mut replacements: Vec<(usize, usize, Vec<String>)> = Vec::new();

    for chunk in chunks {
        let mut cursor = search_from;
        if let Some(anchor) = chunk.context.as_deref() {
            if let Some(pos) = (search_from..lines.len())
                .find(|&idx| lines[idx].trim() == anchor.trim())
                .or_else(|| (0..lines.len()).find(|&idx| lines[idx].trim() == anchor.trim()))
            {
                cursor = pos + 1;
            }
        }
        let mut pattern = Vec::new();
        let mut new_section = Vec::new();
        for hl in &chunk.lines {
            match hl {
                HunkLine::Context(s) => {
                    pattern.push(s.clone());
                    new_section.push(s.clone());
                }
                HunkLine::Remove(s) => {
                    pattern.push(s.clone());
                    deletions += 1;
                }
                HunkLine::Add(s) => {
                    new_section.push(s.clone());
                    additions += 1;
                }
            }
        }
        if pattern.is_empty() && !chunk.is_end_of_file && chunk.context.is_none() {
            // Bare insertion at end unless file already empty.
            let idx = lines.len();
            replacements.push((idx, idx, new_section));
            search_from = idx;
            continue;
        }
        let found = find_subsequence(&lines, &pattern, cursor, chunk.is_end_of_file)
            .ok_or_else(|| format!("failed to find expected lines in {path}"))?;
        if let Some(prev) = replacements.last() {
            if found < prev.1 {
                return Err(format!("overlapping patch hunks in {path}"));
            }
        }
        search_from = found + pattern.len();
        replacements.push((found, found + pattern.len(), new_section));
    }

    for (start, end, new_section) in replacements.into_iter().rev() {
        lines.splice(start..end, new_section);
    }

    let mut result = lines.join("\n");
    if trailing_nl && !lines.is_empty() {
        result.push('\n');
    } else if lines.is_empty() && !original.is_empty() {
        result.clear();
    }
    if result.len() > FILE_LIMIT {
        return Err(format!("patched {path} exceeds 8 MiB limit"));
    }
    Ok((result, additions, deletions))
}

struct StagedTemp(PathBuf);
impl Drop for StagedTemp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

enum PlannedAction {
    WriteNew { rel: PathBuf, content: String, label: String },
    Overwrite { rel: PathBuf, content: String, meta: Metadata, label: String },
    Move { src: PathBuf, dst: PathBuf, content: String, meta: Metadata, label: String },
    Delete { rel: PathBuf, meta: Metadata, label: String },
}

pub(crate) fn apply_patch(workspace: &Path, patch_text: &str) -> Result<String> {
    let ops = parse_patch(patch_text)?;
    let lock = sandbox::Lock::workspace(workspace)?;
    let mut seen_paths = HashSet::new();
    let mut planned = Vec::with_capacity(ops.len());

    // Preflight every operation before mutating any file on disk.
    for op in ops {
        match op {
            PatchOp::Add { path, lines } => {
                let rel = PathBuf::from(&path);
                if !seen_paths.insert(rel.clone()) {
                    return Err(format!("duplicate path in apply_patch: {path}"));
                }
                if lock.check_new_parent(&rel)? {
                    return Err(format!("cannot add existing file: {path}"));
                }
                let adds = lines.len();
                let mut content = lines.join("\n");
                if !content.is_empty() {
                    content.push('\n');
                }
                if content.len() > FILE_LIMIT {
                    return Err(format!("added file {path} exceeds 8 MiB limit"));
                }
                planned.push(PlannedAction::WriteNew { rel, content, label: format!("A {path} (+{adds})") });
            }
            PatchOp::Delete { path } => {
                let rel = PathBuf::from(&path);
                if !seen_paths.insert(rel.clone()) {
                    return Err(format!("duplicate path in apply_patch: {path}"));
                }
                let (dir, target) = lock.parent(&rel)?;
                let (_text, meta) = load_regular_text(&dir, &target)?;
                planned.push(PlannedAction::Delete { rel, meta, label: format!("D {path}") });
            }
            PatchOp::Update { path, move_to, chunks } => {
                let src = PathBuf::from(&path);
                if !seen_paths.insert(src.clone()) {
                    return Err(format!("duplicate path in apply_patch: {path}"));
                }
                let (dir, target) = lock.parent(&src)?;
                let (original, meta) = load_regular_text(&dir, &target)?;
                let (content, adds, dels) =
                    if chunks.is_empty() { (original, 0, 0) } else { apply_chunks(&path, &original, &chunks)? };
                if let Some(dst_str) = move_to {
                    let dst = PathBuf::from(&dst_str);
                    if !seen_paths.insert(dst.clone()) {
                        return Err(format!("duplicate path in apply_patch: {dst_str}"));
                    }
                    if lock.check_new_parent(&dst)? {
                        return Err(format!("Move to destination already exists: {dst_str}"));
                    }
                    planned.push(PlannedAction::Move {
                        src,
                        dst,
                        content,
                        meta,
                        label: format!("R {path} -> {dst_str} (+{adds}/-{dels})"),
                    });
                } else {
                    planned.push(PlannedAction::Overwrite {
                        rel: src,
                        content,
                        meta,
                        label: format!("M {path} (+{adds}/-{dels})"),
                    });
                }
            }
        }
    }

    let mut summary = vec!["Success. Updated the following files:".to_owned()];
    for item in planned {
        match item {
            PlannedAction::WriteNew { rel, content, label } => {
                let (dir, target) = lock.ensure_parent(&rel)?;
                publish_temp(&dir, &target, &content, None)?;
                summary.push(label);
            }
            PlannedAction::Overwrite { rel, content, meta, label } => {
                let (dir, target) = lock.parent(&rel)?;
                let now = fs::symlink_metadata(&target).map_err(|e| format!("check before write: {e}"))?;
                if version(&now) != version(&meta) {
                    return Err(format!("file changed concurrently: {}", rel.display()));
                }
                publish_temp(&dir, &target, &content, Some(meta.mode() & 0o777))?;
                summary.push(label);
            }
            PlannedAction::Move { src, dst, content, meta, label } => {
                let (_src_dir, src_target) = lock.parent(&src)?;
                let now = fs::symlink_metadata(&src_target).map_err(|e| format!("check before move: {e}"))?;
                if version(&now) != version(&meta) {
                    return Err(format!("file changed concurrently: {}", src.display()));
                }
                let (dst_dir, dst_target) = lock.ensure_parent(&dst)?;
                publish_temp(&dst_dir, &dst_target, &content, Some(meta.mode() & 0o777))?;
                if let Err(e) = fs::remove_file(&src_target) {
                    // Do not leave two copies behind when the move cannot be completed.
                    let _ = fs::remove_file(&dst_target);
                    return Err(format!("remove moved source {}: {e}", src.display()));
                }
                summary.push(label);
            }
            PlannedAction::Delete { rel, meta, label } => {
                let (_dir, target) = lock.parent(&rel)?;
                let now = fs::symlink_metadata(&target).map_err(|e| format!("check before delete: {e}"))?;
                if version(&now) != version(&meta) {
                    return Err(format!("file changed concurrently: {}", rel.display()));
                }
                fs::remove_file(&target).map_err(|e| format!("delete {}: {e}", rel.display()))?;
                summary.push(label);
            }
        }
    }
    Ok(summary.join("\n"))
}

fn publish_temp(dir: &File, target: &Path, content: &str, mode: Option<u32>) -> Result<()> {
    let tmp = sandbox::fd_path(dir).join(format!(".teamagents-{}", uuid::Uuid::new_v4()));
    let mut f = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("stage patch write: {e}"))?;
    let guard = StagedTemp(tmp);
    f.write_all(content.as_bytes()).map_err(|e| format!("stage patch write: {e}"))?;
    if let Some(perm) = mode {
        f.set_permissions(fs::Permissions::from_mode(perm)).map_err(|e| format!("set mode: {e}"))?;
        fs::rename(&guard.0, target).map_err(|e| format!("replace patched file: {e}"))?;
    } else {
        fs::hard_link(&guard.0, target).map_err(|e| format!("create patched file: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("teamagents-patch-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn apply_patch_supports_add_update_move_delete_and_trailing_ws_fallback() {
        let ws = Temp::new();
        fs::write(ws.0.join("lib.rs"), "fn alpha() -> i32 {   \n    1\n}\n\nfn beta() -> i32 {\n    2\n}\n").unwrap();
        fs::write(ws.0.join("old.txt"), "hello world\n").unwrap();
        fs::write(ws.0.join("gone.txt"), "bye\n").unwrap();

        let patch = "\
*** Begin Patch
*** Add File: sub/new.txt
+line one
+line two
*** Update File: lib.rs
@@ fn alpha()
-fn alpha() -> i32 {
+fn alpha() -> i32 {
-    1
+    10
@@ fn beta()
-    2
+    20
*** Update File: old.txt
*** Move to: moved/new_name.txt
@@
-hello world
+hello codex
*** Delete File: gone.txt
*** End Patch";

        let report = apply_patch(&ws.0, patch).unwrap();
        assert!(report.contains("A sub/new.txt (+2)"));
        assert!(report.contains("M lib.rs (+3/-3)"));
        assert!(report.contains("R old.txt -> moved/new_name.txt (+1/-1)"));
        assert!(report.contains("D gone.txt"));

        assert_eq!(fs::read_to_string(ws.0.join("sub/new.txt")).unwrap(), "line one\nline two\n");
        assert_eq!(
            fs::read_to_string(ws.0.join("lib.rs")).unwrap(),
            "fn alpha() -> i32 {\n    10\n}\n\nfn beta() -> i32 {\n    20\n}\n"
        );
        assert!(!ws.0.join("old.txt").exists());
        assert_eq!(fs::read_to_string(ws.0.join("moved/new_name.txt")).unwrap(), "hello codex\n");
        assert!(!ws.0.join("gone.txt").exists());
    }

    #[test]
    fn apply_patch_preflights_before_mutation_and_blocks_escapes() {
        let ws = Temp::new();
        let outside = Temp::new();
        fs::write(ws.0.join("a.txt"), "keep\n").unwrap();
        fs::create_dir(ws.0.join(".git")).unwrap();
        fs::write(ws.0.join(".git/config"), "orig\n").unwrap();
        symlink(&outside.0, ws.0.join("linkdir")).unwrap();

        // Failing hunk in second file must leave first file untouched AND create no new parent dirs.
        let atomic_fail = "\
*** Begin Patch
*** Add File: nested/newdir/should_not_exist.txt
+hi
*** Update File: a.txt
@@
-missing
+changed
*** End Patch";
        assert!(apply_patch(&ws.0, atomic_fail).is_err());
        assert!(!ws.0.join("nested").exists(), "preflight failure must not create directories");
        assert_eq!(fs::read_to_string(ws.0.join("a.txt")).unwrap(), "keep\n");

        // Rejects .git, symlink escape, and parent traversal.
        for bad in [
            "*** Begin Patch\n*** Update File: .git/config\n@@\n-orig\n+pwn\n*** End Patch",
            "*** Begin Patch\n*** Add File: linkdir/pwn.txt\n+x\n*** End Patch",
            "*** Begin Patch\n*** Add File: ../escape.txt\n+x\n*** End Patch",
        ] {
            assert!(apply_patch(&ws.0, bad).is_err());
        }
        assert_eq!(fs::read_to_string(ws.0.join(".git/config")).unwrap(), "orig\n");
    }
}
