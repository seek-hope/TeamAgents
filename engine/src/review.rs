//! User-only workspace review. The baseline records actual dirty input bytes,
//! not HEAD and not tool receipts. Shared roots deliberately share a baseline.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ponytail: bounded first-observed snapshots, not a VCS or filesystem journal.
// A future incremental watcher can reduce large-repository rescans; omitted
// paths/content must remain explicit rather than being presented as unchanged.
const MAX_FILES: usize = 5000;
const MAX_FILE: u64 = 2 * 1024 * 1024;
const MAX_TOTAL: u64 = 64 * 1024 * 1024;
const MAX_COMMAND: usize = 2 * 1024 * 1024;
const MAX_PATH: usize = 1024;
const PAGE_LINES: usize = 120;
const MAX_DIFF_LINES: usize = 20_000;
const MAX_MANIFEST: u64 = 16 * 1024 * 1024;
static CAPTURE: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Entry {
    kind: String,
    size: u64,
    mode: u32,
    hash: Option<String>,
    problem: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Baseline {
    version: u32,
    root: PathBuf,
    captured_at: f64,
    scope: String,
    files: BTreeMap<String, Entry>,
    warnings: Vec<String>,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

fn directory(session: &Path, root: &Path) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    session.join("reviews").join(digest(root.as_os_str().as_bytes()))
}

fn private_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut file = OpenOptions::new().create_new(true).write(true).mode(0o600).open(path).map_err(|e| e.to_string())?;
    file.write_all(bytes).and_then(|()| file.sync_all()).map_err(|e| e.to_string())
}

fn save_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = write_private(&tmp, &serde_json::to_vec(value).map_err(|e| e.to_string())?)
        .and_then(|()| std::fs::rename(&tmp, path).map_err(|e| e.to_string()));
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = OpenOptions::new().read(true).custom_flags(0x800 | 0x20000).open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("审查记录不是普通文件".into());
    }
    let mut bytes = vec![];
    file.take(limit + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("审查记录超限".into());
    }
    Ok(bytes)
}

fn check(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::SeqCst) {
        Err("工作区审查已取消".into())
    } else {
        Ok(())
    }
}

/// A subprocess cannot grow our memory without bound or hold the worker's
/// control thread. No shell, pager, credentials, textconv, or external diff.
fn git(root: &Path, args: &[&str], cancelled: &AtomicBool) -> Result<(i32, Vec<u8>, bool), String> {
    check(cancelled)?;
    let mut command = Command::new("git");
    command
        .arg("--no-pager")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null"])
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LANG", "C.UTF-8")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| format!("无法运行 Git 审查：{e}"))?;
    fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Result<(Vec<u8>, bool), String>> {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut chunk = [0u8; 8192];
            let mut truncated = false;
            loop {
                let n = pipe.read(&mut chunk).map_err(|e| e.to_string())?;
                if n == 0 {
                    return Ok((kept, truncated));
                }
                let room = MAX_COMMAND.saturating_sub(kept.len()).min(n);
                kept.extend_from_slice(&chunk[..room]);
                truncated |= room < n;
            }
        })
    }
    let stdout = drain(child.stdout.take().ok_or("审查输出管道不可用")?);
    let stderr = drain(child.stderr.take().ok_or("审查错误管道不可用")?);
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < Duration::from_secs(10) && check(cancelled).is_ok() => {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => {
                let _ = Command::new("/bin/sh")
                    .args(["-c", &format!("kill -KILL -{}", child.id())])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = child.kill();
                let _ = child.wait();
                break Err(match result {
                    Err(e) => e.to_string(),
                    _ if cancelled.load(Ordering::SeqCst) => "工作区审查已取消".into(),
                    _ => "Git 审查超过 10 秒，未完成".into(),
                });
            }
        }
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    while (!stdout.is_finished() || !stderr.is_finished()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !stdout.is_finished() || !stderr.is_finished() {
        // No configured Git helper should run, but even a wrapper in PATH must
        // not leave this caller waiting forever on inherited pipe handles.
        let _ = Command::new("/bin/sh")
            .args(["-c", &format!("kill -KILL -{}", child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        return Err("Git 审查输出管道未关闭，结果不可用".into());
    }
    let (out, truncated) = stdout.join().map_err(|_| "审查输出线程失败")??;
    let (err, _) = stderr.join().map_err(|_| "审查错误线程失败")??;
    let status = status?;
    let code = status.code().ok_or("Git 审查进程异常终止")?;
    if code > 1 {
        return Err(format!("Git 审查失败：{}", String::from_utf8_lossy(&err)));
    }
    Ok((code, out, truncated))
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH
        && Path::new(path).components().all(|c| matches!(c, Component::Normal(p) if p != ".git"))
}

fn excluded(path: &Path, session: &Path, root: &Path) -> bool {
    // Isolated/worktree roots intentionally live beneath session state. Their
    // own files are reviewable; siblings containing private history are not.
    let state = crate::config::state_dir();
    (path.starts_with(session) && !root.starts_with(session))
        || (path.starts_with(&state) && !root.starts_with(&state))
        || path.strip_prefix(root).is_ok_and(|relative| relative.components().any(|c| c.as_os_str() == ".git"))
}

fn paths(
    root: &Path,
    session: &Path,
    cancelled: &AtomicBool,
) -> Result<(BTreeSet<String>, String, Vec<String>), String> {
    check(cancelled)?;
    let mut names = BTreeSet::new();
    let mut warnings = vec![];
    // Distinguish non-Git projects from a Git failure: never silently change
    // review scope after a corrupt index or a timed-out Git operation.
    // Some sandbox mounts expose an empty .git placeholder on an ancestor;
    // that is not a repository. A marker at the requested root is still treated
    // as Git, so corruption is an error rather than a silent scope switch.
    let in_git = root.join(".git").exists()
        || root.ancestors().any(|p| p.join(".git").is_file() || p.join(".git/HEAD").is_file());
    if in_git {
        let (code, output, truncated) =
            git(root, &["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "."], cancelled)?;
        if code != 0 {
            return Err("无法枚举 Git 文件，审查未完成".into());
        }
        // A capped pipe may end in the middle of a path. Never invent a file
        // by treating that partial record as a complete name.
        for record in output.split_inclusive(|b| *b == 0) {
            check(cancelled)?;
            let Some(bytes) = record.strip_suffix(&[0]) else { continue };
            if bytes.is_empty() {
                continue;
            }
            match std::str::from_utf8(bytes) {
                Ok(path) if valid_path(path) && !excluded(&root.join(path), session, root) => {
                    names.insert(path.to_string());
                }
                _ => warnings.push("部分路径非 UTF-8、过长、含 Git 元数据或会话状态，未纳入审查".into()),
            }
            if names.len() == MAX_FILES {
                warnings.push(format!("文件枚举达到 {MAX_FILES} 项上限，结果可能不完整"));
                break;
            }
        }
        if truncated {
            warnings.push("Git 文件枚举输出超限，结果不完整".into());
        }
        warnings.sort();
        warnings.dedup();
        return Ok((names, "git_tracked_and_unignored".into(), warnings));
    }
    let started = Instant::now();
    let mut pending = vec![root.to_path_buf()];
    let mut visited = 0;
    'walk: while let Some(dir) = pending.pop() {
        check(cancelled)?;
        use std::os::fd::AsRawFd;
        let handle = OpenOptions::new()
            .read(true)
            .custom_flags(0x10000 | 0x20000)
            .open(&dir)
            .map_err(|e| format!("无法枚举 {}：{e}", dir.display()))?;
        let pinned = PathBuf::from(format!("/proc/self/fd/{}", handle.as_raw_fd()));
        let actual = std::fs::canonicalize(&pinned).map_err(|e| e.to_string())?;
        if !actual.starts_with(root) || excluded(&actual, session, root) {
            return Err("枚举目录离开审查范围，审查中止".into());
        }
        for item in std::fs::read_dir(&pinned).map_err(|e| format!("无法枚举 {}：{e}", dir.display()))? {
            check(cancelled)?;
            visited += 1;
            if visited > MAX_FILES || started.elapsed() > Duration::from_secs(10) {
                warnings.push("目录枚举超过文件数或时限，结果不完整".into());
                break 'walk;
            }
            let item = item.map_err(|e| e.to_string())?;
            let path = dir.join(item.file_name());
            if excluded(&path, session, root) {
                continue;
            }
            let Some(name) = path.strip_prefix(root).ok().and_then(Path::to_str).filter(|s| valid_path(s)) else {
                warnings.push("部分路径无法安全表示，未纳入审查".into());
                continue;
            };
            let ty = item.file_type().map_err(|e| e.to_string())?;
            if ty.is_dir() {
                if matches!(
                    item.file_name().to_str(),
                    Some(".git" | "node_modules" | "target" | ".venv" | "venv" | "__pycache__")
                ) {
                    continue;
                }
                pending.push(path);
            } else {
                names.insert(name.to_string());
            }
        }
    }
    warnings.sort();
    warnings.dedup();
    Ok((names, "files_except_build_and_dependency_directories".into(), warnings))
}

fn read_entry(root: &Path, session: &Path, name: &str, budget: &mut u64) -> Result<Option<(Entry, Vec<u8>)>, String> {
    if !valid_path(name) {
        return Err("审查路径必须是工作目录内的相对文件路径".into());
    }
    let path = root.join(name);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("无法读取 {name}：{e}")),
    };
    let mut entry = Entry {
        kind: "text".into(),
        size: meta.len(),
        mode: meta.permissions().mode() & 0o7777,
        hash: None,
        problem: None,
    };
    if meta.file_type().is_symlink() {
        use std::os::unix::ffi::OsStrExt;
        let parent = std::fs::canonicalize(path.parent().ok_or("审查文件没有父目录")?).map_err(|e| e.to_string())?;
        if !parent.starts_with(root) || excluded(&parent, session, root) {
            return Err("审查链接的父目录离开工作区".into());
        }
        entry.kind = "symlink".into();
        // Read the link text, never its target (which may be outside the root).
        let bytes = std::fs::read_link(&path).map_err(|e| e.to_string())?.as_os_str().as_bytes().to_vec();
        if bytes.len() as u64 > *budget {
            entry.kind = "unreviewed".into();
            entry.problem = Some("快照内容达到上限".into());
            return Ok(Some((entry, vec![])));
        }
        *budget -= bytes.len() as u64;
        entry.hash = Some(digest(&bytes));
        if std::str::from_utf8(&bytes).is_err() {
            entry.problem = Some("符号链接目标非 UTF-8；仅记录哈希，不生成有损文本差异".into());
        }
        return Ok(Some((entry, bytes)));
    }
    if !meta.is_file() || meta.len() > MAX_FILE || meta.len() > *budget {
        entry.kind = "unreviewed".into();
        entry.problem = Some("特殊文件/子模块、单文件超过 2 MiB，或快照达到 64 MiB 上限".into());
        return Ok(Some((entry, vec![])));
    }
    // Reuse fd-pinned, bounded workspace reads; FIFOs and symlink escapes fail.
    let mut file = crate::tools::open_member_file(root, &path)?;
    use std::os::fd::AsRawFd;
    let actual = std::fs::canonicalize(format!("/proc/self/fd/{}", file.as_raw_fd())).map_err(|e| e.to_string())?;
    if excluded(&actual, session, root) {
        return Err("审查文件指向会话私有状态或 Git 元数据，已排除".into());
    }
    let before = file.metadata().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    (&mut file).take(MAX_FILE + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let after = file.metadata().map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE
        || bytes.len() as u64 > *budget
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err(format!("{name} 在审查读取期间变化或超限，请刷新"));
    }
    *budget -= bytes.len() as u64;
    entry.size = bytes.len() as u64;
    entry.mode = after.permissions().mode() & 0o7777;
    entry.hash = Some(digest(&bytes));
    if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
        entry.kind = "binary".into();
    }
    Ok(Some((entry, bytes)))
}

/// Called before a production runner/tool can write. Existing captures never
/// get reset on session reopen; new shared members reuse the root's capture.
pub fn ensure(session: &Path, root: &Path) -> Result<(), String> {
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let base = directory(session, &root);
    let _guard = CAPTURE.lock().map_err(|_| "审查快照锁不可用")?;
    if base.join("manifest.json").exists() {
        return Ok(());
    }
    let captured_at = teamagents_core::models::now();
    private_dir(&base)?;
    if base.join("capture-started").exists() {
        return Err("先前审查基线未成功建立；不会用已变更的目录覆盖起点".into());
    }
    write_private(&base.join("capture-started"), b"first observed capture\n")?;
    private_dir(&base.join("blobs"))?;
    let (names, scope, mut warnings) = paths(&root, session, &AtomicBool::new(false))?;
    let mut budget = MAX_TOTAL;
    let mut files = BTreeMap::new();
    let started = Instant::now();
    for name in names {
        if started.elapsed() > Duration::from_secs(10) {
            warnings.push("基线读取超过 10 秒，后续文件未纳入基线".into());
            break;
        }
        match read_entry(&root, session, &name, &mut budget) {
            Ok(Some((entry, bytes))) => {
                if matches!(entry.kind.as_str(), "text" | "symlink") {
                    let blob = base.join("blobs").join(entry.hash.as_deref().ok_or("快照缺少内容哈希")?);
                    if !blob.exists() {
                        write_private(&blob, &bytes)?;
                    }
                }
                files.insert(name, entry);
            }
            Ok(None) => {}
            Err(error) => {
                warnings.push(error.clone());
                files.insert(
                    name,
                    Entry { kind: "unreviewed".into(), size: 0, mode: 0, hash: None, problem: Some(error) },
                );
            }
        }
    }
    save_json(&base.join("manifest.json"), &Baseline { version: 1, root, captured_at, scope, files, warnings })
}

/// Register only the effective root, outside the member's accessible worktree.
pub fn register(session: &Path, member: &Path, root: &Path) -> Result<(), String> {
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    save_json(&member.join("review-root.json"), &root)?;
    ensure(session, &root)
}

pub fn registered_root(member: &Path) -> Result<PathBuf, String> {
    let bytes = read_bounded(&member.join("review-root.json"), 16_384)
        .map_err(|_| "此成员尚无工作区审查基线；需先开始使用工作目录".to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| format!("审查工作目录记录损坏：{e}"))
}

fn patch(old: &[u8], new: &[u8], cancelled: &AtomicBool) -> Result<(Vec<String>, bool), String> {
    // A session may close/archive while this read runs. Never recreate its
    // state directory to render a late response.
    let scratch = std::env::temp_dir().join(format!("teamagents-diff-{}", uuid::Uuid::new_v4().simple()));
    private_dir(&scratch)?;
    let result = (|| {
        write_private(&scratch.join("before"), old)?;
        write_private(&scratch.join("after"), new)?;
        let (_, out, mut truncated) = git(
            &scratch,
            &[
                "diff",
                "--no-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--text",
                "--unified=3",
                "--",
                "before",
                "after",
            ],
            cancelled,
        )?;
        let text = String::from_utf8_lossy(&out);
        truncated |= text.lines().count() > MAX_DIFF_LINES;
        let lines = text
            .lines()
            .take(MAX_DIFF_LINES)
            .map(|line| {
                let mut text: String = line.chars().take(4000).collect();
                if line.chars().count() > 4000 {
                    truncated = true;
                    text.push_str(" [line truncated]");
                }
                text
            })
            .collect();
        Ok((lines, truncated))
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// Read-only, user-facing API. This is never exposed as a member model tool.
pub fn report(session: &Path, root: &Path, selected: Option<&str>, offset: usize) -> Result<Json, String> {
    report_checked(session, root, selected, offset, None)
}

pub fn report_checked(
    session: &Path,
    root: &Path,
    selected: Option<&str>,
    offset: usize,
    expected: Option<&str>,
) -> Result<Json, String> {
    report_cancellable(session, root, selected, offset, expected, &AtomicBool::new(false))
}

pub(crate) fn report_cancellable(
    session: &Path,
    root: &Path,
    selected: Option<&str>,
    offset: usize,
    expected: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<Json, String> {
    check(cancelled)?;
    if offset > MAX_DIFF_LINES || (selected.is_none() && offset != 0) {
        return Err("审查分页位置无效".into());
    }
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let base = directory(session, &root);
    let bytes =
        read_bounded(&base.join("manifest.json"), MAX_MANIFEST).map_err(|e| format!("无法读取审查基线：{e}"))?;
    let baseline: Baseline = serde_json::from_slice(&bytes).map_err(|e| format!("审查基线损坏：{e}"))?;
    if baseline.version != 1 || baseline.root != root {
        return Err("审查基线版本或工作目录不匹配".into());
    }
    if baseline.files.len() > MAX_FILES
        || baseline.files.iter().any(|(name, entry)| {
            !valid_path(name)
                || match entry.kind.as_str() {
                    "text" | "binary" | "symlink" => !entry.hash.as_deref().is_some_and(valid_hash),
                    "unreviewed" => entry.problem.is_none(),
                    _ => true,
                }
        })
    {
        return Err("审查基线文件记录损坏或超限".into());
    }
    let (mut names, scope, mut warnings) = paths(&root, session, cancelled)?;
    names.extend(baseline.files.keys().cloned());
    warnings.extend(baseline.warnings.iter().cloned());
    if scope != baseline.scope {
        warnings.push("工作目录的 Git 状态已改变，枚举范围与基线不同".into());
    }
    if selected.is_some_and(|name| !valid_path(name) || !names.contains(name)) {
        return Err("所选文件不在审查范围内".into());
    }
    let mut budget = MAX_TOTAL;
    let mut changes = vec![];
    let mut detail = Json::Null;
    let started = Instant::now();
    for name in names {
        check(cancelled)?;
        if excluded(&root.join(&name), session, &root) {
            warnings.push("基线含会话私有路径，已排除".into());
            continue;
        }
        if started.elapsed() > Duration::from_secs(10) {
            warnings.push("差异读取超过 10 秒，后续文件未完成检查".into());
            break;
        }
        let old = baseline.files.get(&name);
        let current = read_entry(&root, session, &name, &mut budget);
        let (new, data) = match current {
            Ok(value) => value.map(|(entry, bytes)| (Some(entry), bytes)).unwrap_or((None, vec![])),
            Err(error) => {
                (Some(Entry { kind: "unreviewed".into(), size: 0, mode: 0, hash: None, problem: Some(error) }), vec![])
            }
        };
        let unknown = (old.is_none() && !baseline.warnings.is_empty())
            || old.into_iter().chain(new.as_ref()).any(|entry| entry.problem.is_some());
        if !unknown && old == new.as_ref() {
            if selected == Some(name.as_str()) {
                if offset != 0 {
                    return Err("审查分页位置已失效，请按 r 刷新".into());
                }
                detail = json!({"path":name,"offset":offset,"total_lines":0,"lines":[],"truncated":false,
                    "next_offset":Json::Null});
            }
            continue;
        }
        let status = if unknown {
            "unreviewed"
        } else if old.is_none() {
            "added"
        } else if new.is_none() {
            "deleted"
        } else {
            "modified"
        };
        changes.push(json!({"path":name, "status":status, "before":old, "after":new}));
        if selected == Some(name.as_str()) {
            let binary = old.into_iter().chain(new.as_ref()).any(|entry| entry.kind == "binary");
            let (lines, truncated) = if unknown || binary {
                (
                    vec![if binary {
                        "二进制文件：仅比较字节哈希与大小，不生成文本差异".into()
                    } else {
                        "内容未完整读取：请查看文件清单中的限制或错误".into()
                    }],
                    false,
                )
            } else {
                let old_bytes = match old.and_then(|entry| entry.hash.as_ref()) {
                    Some(hash) if valid_hash(hash) => {
                        let bytes = read_bounded(&base.join("blobs").join(hash), MAX_FILE)
                            .map_err(|e| format!("无法读取基线内容：{e}"))?;
                        if digest(&bytes) != *hash {
                            return Err("基线内容哈希不匹配，拒绝生成不可信差异".into());
                        }
                        bytes
                    }
                    Some(_) => return Err("审查基线内容引用无效".into()),
                    None => vec![],
                };
                patch(&old_bytes, &data, cancelled)?
            };
            if offset > lines.len() {
                return Err("审查分页位置已失效，请按 r 刷新".into());
            }
            let end = offset.saturating_add(PAGE_LINES).min(lines.len());
            detail = json!({"path":name, "offset":offset, "total_lines":lines.len(),
                "lines":lines.get(offset..end).unwrap_or(&[]), "truncated":truncated,
                "next_offset":(end < lines.len()).then_some(end)});
        }
    }
    check(cancelled)?;
    let complete =
        warnings.is_empty() && changes.iter().all(|c| c["status"] != "unreviewed") && detail["truncated"] != true;
    let revision = digest(
        &serde_json::to_vec(&json!({"baseline":baseline.captured_at,"changes":changes,"warnings":warnings}))
            .map_err(|e| e.to_string())?,
    );
    if expected.is_some_and(|expected| expected != revision) {
        return Err("工作区已在分页期间变化，请按 r 从最新内容重新审查".into());
    }
    Ok(json!({"root":root, "baseline_at":baseline.captured_at, "baseline_kind":"first_observed",
        "scope":baseline.scope, "baseline_files":baseline.files.len(), "complete":complete,
        "warnings":warnings, "changes":changes, "detail":detail, "revision":revision}))
}
