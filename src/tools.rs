//! Workspace tools. The caller authorizes dispatch; every operation enforces its own boundary.
use crate::{
    context, mcp,
    net::Network,
    patch,
    sandbox::{self, fd_path as fd_dir, SandboxMode},
    Result,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, Metadata, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    os::{
        fd::{AsRawFd, OwnedFd, RawFd},
        unix::{
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
            net::UnixStream,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Stdio},
    thread,
    time::{Duration, Instant},
};

const FILE_LIMIT: usize = 8 * 1024 * 1024;
const GREP_FILE_LIMIT: u64 = 2 * 1024 * 1024;
const GREP_MAX_SCAN_FILES: usize = 10_000;
pub(crate) const OUTPUT_LIMIT: usize = 128 * 1024; // per stream (half head + half tail on middle truncation)
pub(crate) const HALF_OUTPUT: usize = OUTPUT_LIMIT / 2;
const DEFAULT_SHELL_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_SHELL_TIMEOUT: Duration = Duration::from_secs(600);

fn field<'a>(action: &'a Value, key: &str) -> Result<&'a str> {
    action[key].as_str().ok_or_else(|| format!("expected string field {key}"))
}

fn load(dir: &File, path: &Path) -> Result<(String, Metadata)> {
    let f = sandbox::open_regular(dir, path)?;
    let meta = f.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("expected a regular file".into());
    }
    if meta.len() > FILE_LIMIT as u64 {
        return Err("file exceeds 8 MiB; pass offset and limit to read a line range".into());
    }
    let mut bytes = Vec::new();
    f.take(FILE_LIMIT as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("read file: {e}"))?;
    if bytes.len() > FILE_LIMIT {
        return Err("file exceeds 8 MiB; pass offset and limit to read a line range".into());
    }
    Ok((String::from_utf8(bytes).map_err(|_| "file is not UTF-8 text")?, meta))
}

fn load_slice(
    dir: &File,
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
    line_numbers: bool,
) -> Result<String> {
    if offset.is_none() && limit.is_none() && !line_numbers {
        return load(dir, path).map(|(text, _)| text);
    }
    let start_line = match offset {
        Some(0) => return Err("read_file offset must be >= 1".into()),
        Some(n) => n,
        None => 1,
    };
    let max_lines = match limit {
        Some(0) => return Err("read_file limit must be >= 1".into()),
        Some(n) => n,
        None => usize::MAX,
    };
    let f = sandbox::open_regular(dir, path)?;
    let meta = f.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("expected a regular file".into());
    }
    let mut reader = BufReader::new(f);
    let mut line_buf = Vec::new();
    let mut out = String::new();
    let mut line_no = 0usize;
    let mut end_line = 0usize;
    let mut more = false;
    loop {
        line_buf.clear();
        // Bound each individual line read to avoid unbounded allocation on minified files.
        let read = (&mut reader)
            .take(FILE_LIMIT as u64 + 1)
            .read_until(b'\n', &mut line_buf)
            .map_err(|e| format!("read file: {e}"))?;
        if read == 0 {
            break;
        }
        if line_buf.len() > FILE_LIMIT {
            return Err("file line exceeds 8 MiB".into());
        }
        let line_str = std::str::from_utf8(&line_buf).map_err(|_| "file is not UTF-8 text")?;
        line_no += 1;
        if line_no < start_line {
            continue;
        }
        if end_line.saturating_sub(start_line.saturating_sub(1)) >= max_lines {
            more = true;
            break;
        }
        let formatted = if line_numbers {
            let prefix = format!("L{line_no}: ");
            let add_len = prefix.len().saturating_add(line_str.len());
            if out.len().saturating_add(add_len) > FILE_LIMIT {
                if out.is_empty() {
                    return Err("selected line exceeds 8 MiB".into());
                }
                more = true;
                break;
            }
            out.push_str(&prefix);
            out.push_str(line_str);
            end_line = line_no;
            continue;
        } else {
            line_str
        };
        if out.len().saturating_add(formatted.len()) > FILE_LIMIT {
            if out.is_empty() {
                return Err("selected line exceeds 8 MiB".into());
            }
            more = true;
            break;
        }
        out.push_str(formatted);
        end_line = line_no;
    }
    if more && end_line >= start_line {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("[showing lines {start_line}-{end_line}; more lines available]"));
    }
    Ok(out)
}

struct Staged(PathBuf);
impl Drop for Staged {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn version(m: &Metadata) -> (u64, u64, u64, i64, i64) {
    (m.dev(), m.ino(), m.len(), m.ctime(), m.ctime_nsec())
}

fn edit(dir: &File, target: &Path, old: &str, new: &str) -> Result<String> {
    if old.len() > FILE_LIMIT || new.len() > FILE_LIMIT {
        return Err("edit exceeds 8 MiB".into());
    }
    let (updated, original) = if old.is_empty() {
        (new.to_owned(), None)
    } else {
        let (mut text, meta) = load(dir, target)?;
        let start = text.find(old).ok_or("old text was not found")?;
        // Count overlapping matches too (e.g. 'aa' in 'aaa').
        if text[start + old.chars().next().unwrap().len_utf8()..].contains(old) {
            return Err("old text is not unique".into());
        }
        if text.len() - old.len() + new.len() > FILE_LIMIT {
            return Err("edited file exceeds 8 MiB".into());
        }
        text.replace_range(start..start + old.len(), new);
        (text, Some(meta))
    };
    let tmp = fd_dir(dir).join(format!(".teamagents-{}", uuid::Uuid::new_v4()));
    let mut f = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("stage edit: {e}"))?;
    let staged = Staged(tmp);
    f.write_all(updated.as_bytes()).map_err(|e| format!("stage edit: {e}"))?;
    if let Some(meta) = original {
        f.set_permissions(fs::Permissions::from_mode(meta.mode() & 0o777))
            .map_err(|e| format!("set file mode: {e}"))?;
        let now = fs::symlink_metadata(target).map_err(|e| format!("check file before replacing: {e}"))?;
        if version(&now) != version(&meta) {
            return Err("file changed while editing; not replaced".into());
        }
        // ponytail: external writers need not take our flock. Linux lacks rename-if-version;
        // use a versioned workspace for a strict compare-and-swap against other editors.
        fs::rename(&staged.0, target).map_err(|e| format!("publish edit: {e}"))?;
    } else {
        // Publish the complete inode only if the name is still absent, even against external creators.
        fs::hard_link(&staged.0, target).map_err(|e| format!("create file (must be absent): {e}"))?;
    }
    Ok(format!("wrote {} bytes", updated.len()))
}

fn pipe() -> io::Result<(UnixStream, Stdio)> {
    let (r, w) = UnixStream::pair()?;
    r.set_nonblocking(true)?;
    Ok((r, Stdio::from(OwnedFd::from(w))))
}

#[derive(Default)]
pub(crate) struct StreamBuffer {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    omitted_bytes: usize,
    omitted_lines: usize,
}

impl StreamBuffer {
    #[cfg(test)]
    pub(crate) fn stats(&self) -> (usize, usize, usize, usize) {
        (self.head.len(), self.tail.len(), self.omitted_bytes, self.omitted_lines)
    }

    pub(crate) fn push(&mut self, mut chunk: &[u8]) {
        if self.head.len() < HALF_OUTPUT {
            let take = chunk.len().min(HALF_OUTPUT - self.head.len());
            self.head.extend_from_slice(&chunk[..take]);
            chunk = &chunk[take..];
        }
        if chunk.is_empty() {
            return;
        }
        if chunk.len() >= HALF_OUTPUT {
            let skip = chunk.len() - HALF_OUTPUT;
            self.omitted_bytes = self.omitted_bytes.saturating_add(self.tail.len()).saturating_add(skip);
            self.omitted_lines = self
                .omitted_lines
                .saturating_add(self.tail.iter().filter(|&&b| b == b'\n').count())
                .saturating_add(chunk[..skip].iter().filter(|&&b| b == b'\n').count());
            self.tail.clear();
            self.tail.extend(&chunk[skip..]);
        } else {
            let overflow = self.tail.len().saturating_add(chunk.len()).saturating_sub(HALF_OUTPUT);
            for _ in 0..overflow {
                if let Some(b) = self.tail.pop_front() {
                    self.omitted_bytes = self.omitted_bytes.saturating_add(1);
                    if b == b'\n' {
                        self.omitted_lines = self.omitted_lines.saturating_add(1);
                    }
                }
            }
            self.tail.extend(chunk);
        }
    }

    pub(crate) fn finish(self, label: &str) -> String {
        self.finish_with_stats(label).0
    }

    pub(crate) fn finish_with_stats(mut self, label: &str) -> (String, usize, usize, usize, usize) {
        if self.omitted_bytes == 0 {
            self.head.extend(self.tail);
            let len = self.head.len();
            return (String::from_utf8_lossy(&self.head).into_owned(), len, 0, 0, 0);
        }
        // Avoid splitting multi-byte UTF-8 sequences at the middle splice point.
        let mut head_end = self.head.len();
        if head_end > 0 {
            let mut back = 0;
            while back < 4 && head_end - back > 0 && (self.head[head_end - back - 1] & 0xC0) == 0x80 {
                back += 1;
            }
            if back > 0 && head_end > back {
                let lead = self.head[head_end - back - 1];
                let expected = lead.leading_ones() as usize;
                if back + 1 < expected {
                    head_end = head_end - back - 1;
                    self.omitted_bytes += back + 1;
                }
            } else if head_end > 0 && self.head[head_end - 1].leading_ones() >= 2 {
                head_end -= 1;
                self.omitted_bytes += 1;
            }
        }
        while self.tail.front().is_some_and(|&b| (b & 0xC0) == 0x80) {
            self.tail.pop_front();
            self.omitted_bytes += 1;
        }
        let tail_len = self.tail.len();
        let head = String::from_utf8_lossy(&self.head[..head_end]);
        let (tail_a, tail_b) = self.tail.as_slices();
        let mut tail_vec = Vec::with_capacity(tail_a.len() + tail_b.len());
        tail_vec.extend_from_slice(tail_a);
        tail_vec.extend_from_slice(tail_b);
        let tail = String::from_utf8_lossy(&tail_vec);
        let rendered = format!(
            "{head}\n[... {} bytes ({} lines) omitted from {label} ...]\n{tail}",
            self.omitted_bytes, self.omitted_lines
        );
        (rendered, head_end, tail_len, self.omitted_bytes, self.omitted_lines)
    }
}

const DRAIN_BATCH_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DrainState {
    Eof,
    WouldBlock,
    More,
}

// No blocking reader threads: even a descendant that escapes the group and inherits a writer cannot hold us open.
// Oversize output uses middle-truncation (head + tail preserved) so commands always finish with real exit codes.
// Bounding each drain batch prevents a saturated pipe from starving the opposite stream or timeout checks.
pub(crate) type ChunkCallback<'a> = &'a dyn Fn(&[u8]);

fn drain(
    stream: &mut UnixStream,
    buffer: &mut StreamBuffer,
    label: &str,
    on_chunk: Option<ChunkCallback<'_>>,
) -> Result<DrainState> {
    let mut buf = [0; 8192];
    let mut read_bytes = 0usize;
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return Ok(DrainState::Eof),
            Ok(n) => {
                if let Some(cb) = on_chunk {
                    cb(&buf[..n]);
                }
                buffer.push(&buf[..n]);
                read_bytes += n;
                if read_bytes >= DRAIN_BATCH_BYTES {
                    return Ok(DrainState::More);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(DrainState::WouldBlock),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("read shell {label}: {e}")),
        }
    }
}

fn exited(child: &Child) -> io::Result<bool> {
    // SAFETY: valid pid of our unreaped child and writable siginfo. WNOWAIT pins its group id through cleanup.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        let rc = libc::waitid(libc::P_PID, child.id(), &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT);
        if rc == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(info.si_pid() != 0)
        }
    }
}

fn poll_streams(out: Option<RawFd>, err: Option<RawFd>, wait: Duration) -> Result<()> {
    let mut fds = [libc::pollfd { fd: -1, events: 0, revents: 0 }; 2];
    let mut len = 0;
    for fd in [out, err].into_iter().flatten() {
        fds[len] = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        len += 1;
    }
    if len == 0 {
        thread::sleep(wait);
        return Ok(());
    }
    let ms = wait.as_millis().clamp(1, i32::MAX as u128) as i32;
    // SAFETY: `fds[..len]` holds valid open pipe fds for the duration of `poll`.
    let rc = unsafe { libc::poll(fds.as_mut_ptr(), len as libc::nfds_t, ms) };
    if rc == -1 {
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(format!("poll shell output: {e}"));
        }
    }
    Ok(())
}

fn shell_with_options(
    workspace: &Path,
    command: &str,
    timeout: Duration,
    mode: SandboxMode,
    on_chunk: Option<ChunkCallback<'_>>,
) -> Result<String> {
    let (mut stdout, out_fd) = pipe().map_err(|e| format!("shell stdout: {e}"))?;
    let (mut stderr, err_fd) = pipe().map_err(|e| format!("shell stderr: {e}"))?;
    let stage = sandbox::Workspace::new_mode(workspace, mode)?;
    let (mut cmd, filter) = stage.command()?;
    let mut network = Network::prepare(&mut cmd)?;
    let mut child = cmd
        .args(["--", "/bin/sh", "-c", command])
        .stdin(Stdio::null())
        .stdout(out_fd)
        .stderr(err_fd)
        .spawn()
        .map_err(|e| format!("start sandbox /usr/bin/bwrap: {e}"))?;
    drop(cmd);
    drop(filter);
    let start = Instant::now();
    let (mut out, mut err) = (StreamBuffer::default(), StreamBuffer::default());
    let mut connected = false;
    let mut failure = (|| -> Result<()> {
        network.connect(&child)?;
        connected = true;
        let (mut out_state, mut err_state) = (DrainState::WouldBlock, DrainState::WouldBlock);
        loop {
            if out_state != DrainState::Eof {
                out_state = drain(&mut stdout, &mut out, "stdout", on_chunk)?;
            }
            if err_state != DrainState::Eof {
                err_state = drain(&mut stderr, &mut err, "stderr", on_chunk)?;
            }
            if exited(&child).map_err(|e| format!("poll shell: {e}"))? {
                return Ok(());
            }
            let Some(remaining) = timeout.checked_sub(start.elapsed()) else {
                return Err(format!("shell timed out after {}ms (exit 124)", timeout.as_millis()));
            };
            if remaining.is_zero() {
                return Err(format!("shell timed out after {}ms (exit 124)", timeout.as_millis()));
            }
            if out_state != DrainState::More && err_state != DrainState::More {
                poll_streams(
                    (out_state != DrainState::Eof).then_some(stdout.as_raw_fd()),
                    (err_state != DrainState::Eof).then_some(stderr.as_raw_fd()),
                    remaining.min(Duration::from_millis(10)),
                )?;
            }
        }
    })()
    .err();
    // Also stop background group members on success, before reaping the leader and permitting pid reuse.
    // The private PID namespace and --die-with-parent also contain descendants that change groups.
    // SAFETY: process_group(0) made the child's pid the pgid; waitid did not reap it.
    if unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) } == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            failure = Some(format!("shell group cleanup failed: {error}"));
            let _ = child.kill();
        }
    }
    let status = child.wait().map_err(|e| format!("wait for shell: {e}"))?;
    drop(network);
    if failure.is_none() {
        // Capture bytes already written, but never wait indefinitely for escaped writers.
        let end = Instant::now() + Duration::from_millis(200);
        failure = (|| -> Result<()> {
            let (mut out_state, mut err_state) = (DrainState::WouldBlock, DrainState::WouldBlock);
            loop {
                if out_state != DrainState::Eof {
                    out_state = drain(&mut stdout, &mut out, "stdout", on_chunk)?;
                }
                if err_state != DrainState::Eof {
                    err_state = drain(&mut stderr, &mut err, "stderr", on_chunk)?;
                }
                if out_state == DrainState::Eof && err_state == DrainState::Eof {
                    return Ok(());
                }
                if Instant::now() >= end {
                    return Err("shell descendants still hold output open; capture incomplete".into());
                }
                if out_state != DrainState::More && err_state != DrainState::More {
                    thread::sleep(Duration::from_millis(5));
                }
            }
        })()
        .err();
    }
    let text = format!("stdout:\n{}\nstderr:\n{}", out.finish("stdout"), err.finish("stderr"));
    // A completed command can make useful edits even on nonzero exit. Do not publish interrupted output.
    if connected && failure.is_none() {
        if let Err(e) = stage.publish() {
            return Err(format!("sandbox publication failed: {e}\n{text}"));
        }
    }
    if let Some(e) = failure {
        Err(format!("{e}\n{text}"))
    } else if !status.success() {
        Err(format!("shell failed ({status})\n{text}"))
    } else {
        Ok(text)
    }
}

#[cfg(test)]
fn shell(workspace: &Path, command: &str, timeout: Duration, staged: bool) -> Result<String> {
    shell_with_options(
        workspace,
        command,
        timeout,
        if staged { SandboxMode::Staged } else { SandboxMode::Direct },
        None,
    )
}

fn opt_str<'a>(action: &'a Value, key: &str) -> Result<Option<&'a str>> {
    let Some(v) = action.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    v.as_str().ok_or_else(|| format!("expected string field {key}")).map(Some)
}

fn opt_bool(action: &Value, key: &str) -> Result<Option<bool>> {
    let Some(v) = action.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    v.as_bool().ok_or_else(|| format!("expected boolean field {key}")).map(Some)
}

fn glob_match(pat: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut match_ti) = (0usize, 0usize, None, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            pi += 1;
            match_ti = ti;
        } else if let Some(s) = star {
            pi = s + 1;
            match_ti += 1;
            ti = match_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn list_dir(workspace: &Path, rel_path: Option<&str>, max_depth: usize, limit: usize) -> Result<String> {
    let lock = sandbox::Lock::workspace(workspace)?;
    let start_dir = lock.dir(rel_path.map(Path::new))?;
    let max_depth = max_depth.clamp(1, 8);
    let limit = limit.clamp(1, 1000);
    let prefix = rel_path.filter(|s| !s.trim().is_empty() && *s != ".").map(PathBuf::from).unwrap_or_default();

    let mut items: Vec<String> = Vec::new();
    let mut truncated = false;
    #[allow(clippy::too_many_arguments)]
    fn walk_dir(
        root: &File,
        dir: &File,
        rel: &Path,
        depth: usize,
        max_depth: usize,
        limit: usize,
        items: &mut Vec<String>,
        truncated: &mut bool,
    ) -> Result<()> {
        if depth > max_depth || items.len() >= limit {
            if items.len() >= limit {
                *truncated = true;
            }
            return Ok(());
        }
        let mut children: BTreeMap<String, (PathBuf, bool, u64)> = BTreeMap::new();
        for entry in fs::read_dir(fd_dir(dir)).map_err(|e| format!("read dir: {e}"))? {
            let Ok(entry) = entry else { continue };
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with(".teamagents-") {
                continue;
            }
            let child_rel = if rel.as_os_str().is_empty() { PathBuf::from(&name) } else { rel.join(&name) };
            if sandbox::reserved(&child_rel) {
                continue;
            }
            let p = entry.path();
            let Ok(meta) = fs::symlink_metadata(&p) else { continue };
            if meta.file_type().is_symlink() {
                continue;
            }
            children.insert(name_str.into_owned(), (p, meta.is_dir(), meta.len()));
        }
        for (_name_str, (p, is_dir, size)) in children {
            if items.len() >= limit {
                *truncated = true;
                return Ok(());
            }
            let name = p.file_name().unwrap();
            let child_rel = if rel.as_os_str().is_empty() { PathBuf::from(name) } else { rel.join(name) };
            if is_dir {
                let Ok(sub) = sandbox::checked_dir(root, dir, name) else { continue };
                items.push(format!("{}/", child_rel.display()));
                walk_dir(root, &sub, &child_rel, depth + 1, max_depth, limit, items, truncated)?;
            } else if sandbox::regular(&fs::symlink_metadata(&p).map_err(|e| e.to_string())?).is_ok() {
                items.push(format!("{} ({size} B)", child_rel.display()));
            }
        }
        Ok(())
    }

    walk_dir(lock.root_fd(), &start_dir, &prefix, 1, max_depth, limit, &mut items, &mut truncated)?;
    if items.is_empty() {
        return Ok("(empty directory)".into());
    }
    if truncated {
        items.push(format!("[truncated at {limit} entries]"));
    }
    Ok(items.join("\n"))
}

fn grep_files(
    workspace: &Path,
    pattern: &str,
    rel_path: Option<&str>,
    glob: Option<&str>,
    case_insensitive: bool,
    limit: usize,
) -> Result<String> {
    if pattern.is_empty() {
        return Err("grep_files requires nonempty pattern".into());
    }
    let lock = sandbox::Lock::workspace(workspace)?;
    let start_dir = lock.dir(rel_path.map(Path::new))?;
    let prefix = rel_path.filter(|s| !s.trim().is_empty() && *s != ".").map(PathBuf::from).unwrap_or_default();
    let limit = limit.clamp(1, 500);
    let needle = if case_insensitive { pattern.to_ascii_lowercase() } else { pattern.to_owned() };

    let mut scanned = 0usize;
    let mut matches = Vec::new();
    let mut truncated = false;
    let mut stack = vec![(start_dir, prefix)];
    while let Some((dir, rel)) = stack.pop() {
        if matches.len() >= limit {
            truncated = true;
            break;
        }
        let Ok(rd) = fs::read_dir(fd_dir(&dir)) else { continue };
        let mut entries = Vec::new();
        for e in rd.flatten() {
            entries.push(e);
        }
        entries.sort_by_key(|e| e.file_name());
        let mut subdirs = Vec::new();
        for entry in entries {
            if scanned >= GREP_MAX_SCAN_FILES || matches.len() >= limit {
                if matches.len() >= limit {
                    truncated = true;
                }
                break;
            }
            let name = entry.file_name();
            if name.to_string_lossy().starts_with(".teamagents-") {
                continue;
            }
            let child_rel = if rel.as_os_str().is_empty() { PathBuf::from(&name) } else { rel.join(&name) };
            if sandbox::reserved(&child_rel) {
                continue;
            }
            let p = entry.path();
            let Ok(meta) = fs::symlink_metadata(&p) else { continue };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if name == "node_modules" {
                    continue;
                }
                if let Ok(sub) = sandbox::checked_dir(lock.root_fd(), &dir, &name) {
                    subdirs.push((sub, child_rel));
                }
                continue;
            }
            if sandbox::regular(&meta).is_err() || meta.len() > GREP_FILE_LIMIT {
                continue;
            }
            if let Some(g) = glob {
                let rel_str = child_rel.to_string_lossy();
                let target = if g.contains('/') { rel_str.as_ref() } else { &*name.to_string_lossy() };
                if !glob_match(g, target) {
                    continue;
                }
            }
            scanned += 1;
            let Ok(file) = sandbox::open_regular(&dir, &p) else { continue };
            let mut raw = Vec::new();
            if file.take(GREP_FILE_LIMIT + 1).read_to_end(&mut raw).is_err()
                || raw.len() as u64 > GREP_FILE_LIMIT
                || raw.contains(&0)
            {
                continue;
            }
            let Ok(text) = String::from_utf8(raw) else { continue };
            for (idx, line) in text.lines().enumerate() {
                let hit =
                    if case_insensitive { line.to_ascii_lowercase().contains(&needle) } else { line.contains(&needle) };
                if hit {
                    let trimmed = line.trim_end();
                    let snippet = if trimmed.len() > 300 {
                        format!("{}...", &trimmed[..trimmed.floor_char_boundary(300)])
                    } else {
                        trimmed.to_owned()
                    };
                    matches.push(format!("{}:{}: {snippet}", child_rel.display(), idx + 1));
                    if matches.len() >= limit {
                        truncated = true;
                        break;
                    }
                }
            }
        }
        for sub in subdirs.into_iter().rev() {
            stack.push(sub);
        }
    }
    if matches.is_empty() {
        return Ok(format!("No matches for `{pattern}`."));
    }
    if truncated {
        matches.push(format!("[showing first {limit} matches]"));
    }
    Ok(matches.join("\n"))
}

fn opt_usize(action: &Value, key: &str) -> Result<Option<usize>> {
    let Some(v) = action.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let n = v.as_u64().ok_or_else(|| format!("expected positive integer for {key}"))?;
    usize::try_from(n).map(Some).map_err(|_| format!("{key} out of range"))
}

fn shell_timeout(action: &Value) -> Result<Duration> {
    let Some(v) = action.get("timeout_ms").filter(|v| !v.is_null()) else {
        return Ok(DEFAULT_SHELL_TIMEOUT);
    };
    let ms = v.as_u64().ok_or("expected integer timeout_ms")?;
    if ms == 0 {
        return Err("timeout_ms must be positive".into());
    }
    Ok(Duration::from_millis(ms.clamp(100, MAX_SHELL_TIMEOUT.as_millis() as u64)))
}

pub(crate) fn execute_with_options(
    workspace: &Path,
    action: &Value,
    mode: SandboxMode,
    on_chunk: Option<ChunkCallback<'_>>,
) -> Result<String> {
    let op = field(action, "op")?;
    if mode.is_read_only() && matches!(op, "edit_file" | "apply_patch") {
        return Err("workspace is in read-only mode; file edits are disabled".into());
    }
    match op {
        "read_file" => {
            let offset = opt_usize(action, "offset")?;
            let limit = opt_usize(action, "limit")?;
            let line_numbers = opt_bool(action, "line_numbers")?.unwrap_or(false);
            let lock = sandbox::Lock::workspace(workspace)?;
            let (dir, target) = lock.parent(Path::new(field(action, "path")?))?;
            load_slice(&dir, &target, offset, limit, line_numbers)
        }
        "edit_file" => {
            let lock = sandbox::Lock::workspace(workspace)?;
            let (dir, target) = lock.parent(Path::new(field(action, "path")?))?;
            edit(&dir, &target, field(action, "old")?, field(action, "new")?)
        }
        "apply_patch" => patch::apply_patch(workspace, field(action, "patch")?),
        "list_dir" => {
            let rel_path = opt_str(action, "path")?;
            let depth = opt_usize(action, "depth")?.unwrap_or(2);
            let limit = opt_usize(action, "limit")?.unwrap_or(200);
            list_dir(workspace, rel_path, depth, limit)
        }
        "grep_files" => {
            let pattern = field(action, "pattern")?;
            let rel_path = opt_str(action, "path")?;
            let glob = opt_str(action, "glob")?;
            let ci = opt_bool(action, "case_insensitive")?.unwrap_or(false);
            let limit = opt_usize(action, "limit")?.unwrap_or(100);
            grep_files(workspace, pattern, rel_path, glob, ci, limit)
        }
        "shell" => shell_with_options(workspace, field(action, "command")?, shell_timeout(action)?, mode, on_chunk),
        "read_skill" => context::read_skill_doc(workspace, field(action, "name")?),
        "mcp_list" => mcp::list_tools_text(opt_str(action, "server")?),
        "mcp_call" => mcp::call_tool_text(field(action, "server")?, field(action, "tool")?, action.get("arguments")),
        _ => Err("unknown workspace tool".into()),
    }
}

#[allow(dead_code)]
pub fn execute_with_mode(workspace: &Path, action: &Value, staged: bool) -> Result<String> {
    execute_with_options(workspace, action, if staged { SandboxMode::Staged } else { SandboxMode::Direct }, None)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn execute(workspace: &Path, action: &Value) -> Result<String> {
    execute_with_mode(workspace, action, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{os::unix::fs::symlink, process::Command};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("teamagents-tools-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn edit(&self, path: &str, old: &str, new: &str) -> Result<String> {
            execute(&self.0, &json!({"op":"edit_file", "path":path,"old":old,"new":new}))
        }
        fn read(&self, path: &str) -> Result<String> {
            execute(&self.0, &json!({"op":"read_file","path":path}))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unique_replacement_no_clobber_and_overlapping_matches() {
        let t = Temp::new();
        fs::create_dir(t.0.join("src")).unwrap();
        let p = "src/file";
        t.edit(p, "", "aaa beta").unwrap();
        assert!(t.edit(p, "", "overwrite").is_err());
        assert!(t.edit(p, "aa", "X").is_err());
        assert!(t.edit(p, "missing", "X").is_err());
        fs::set_permissions(t.0.join(p), fs::Permissions::from_mode(0o750)).unwrap();
        t.edit(p, "beta", "alpha").unwrap();
        assert_eq!(t.read(p).unwrap(), "aaa alpha");
        assert_eq!(fs::metadata(t.0.join(p)).unwrap().mode() & 0o777, 0o750);
        assert_eq!(fs::read_dir(t.0.join("src")).unwrap().count(), 1);
    }

    #[test]
    fn component_symlinks_and_special_or_large_files_are_refused() {
        let (t, outside) = (Temp::new(), Temp::new());
        outside.edit("private", "", "untouched").unwrap();
        symlink(&outside.0, t.0.join("dir")).unwrap();
        symlink(outside.0.join("private"), t.0.join("file")).unwrap();
        for p in ["../private", "/etc/passwd", "dir/private", "file", "."] {
            assert!(t.read(p).is_err());
            assert!(t.edit(p, "untouched", "bad").is_err());
            assert!(t.edit(p, "", "bad").is_err());
        }
        assert!(t.edit("dir/created", "", "bad").is_err());
        assert_eq!(outside.read("private").unwrap(), "untouched");
        assert!(!outside.0.join("created").exists());
        assert!(Command::new("mkfifo").arg(t.0.join("fifo")).status().unwrap().success());
        assert!(t.read("fifo").is_err());
        fs::write(t.0.join("big"), vec![b'x'; FILE_LIMIT + 1]).unwrap();
        assert!(t.read("big").unwrap_err().contains("8 MiB"));
        let mut sliced = String::new();
        for idx in 1..=130_000 {
            sliced.push_str(&format!("line-{idx:06}-payload-0123456789abcdef0123456789abcdef0123456789abcdef\n"));
        }
        assert!(sliced.len() > FILE_LIMIT);
        fs::write(t.0.join("sliced"), &sliced).unwrap();
        let page = execute(&t.0, &json!({"op":"read_file","path":"sliced","offset":129_990,"limit":5})).unwrap();
        assert!(
            page.contains("line-129990")
                && page.contains("line-129994")
                && !page.contains("line-129995")
                && page.contains("[showing lines 129990-129994; more lines available]")
        );
        assert!(execute(&t.0, &json!({"op":"read_file","path":"sliced","offset":0,"limit":5})).is_err());
        fs::write(t.0.join("binary"), [0xff]).unwrap();
        assert!(t.read("binary").is_err());
    }

    #[test]
    fn racing_creators_do_not_overwrite() {
        let t = Temp::new();
        let results = thread::scope(|s| {
            let a = s.spawn(|| t.edit("shared", "", "one"));
            let b = s.spawn(|| t.edit("shared", "", "two"));
            (a.join().unwrap(), b.join().unwrap())
        });
        assert_ne!(results.0.is_ok(), results.1.is_ok());
        assert_eq!(t.read("shared").unwrap(), if results.0.is_ok() { "one" } else { "two" });
    }

    #[test]
    fn shell_uses_workspace_and_reports_both_streams_and_exit() {
        let t = Temp::new();
        let value = execute(&t.0, &json!({"op":"shell","command":"pwd; printf problem >&2; exit 7"})).unwrap_err();
        assert!(
            value.contains("status: 7") && value.contains("/workspace") && value.contains("stderr:\nproblem"),
            "{value}"
        );
        let value = shell(&t.0, "env", Duration::from_secs(2), false).unwrap();
        for line in value.lines().filter(|s| s.contains('=')) {
            let name = line.split_once('=').unwrap().0;
            assert!(matches!(
                name,
                "PATH"
                    | "HOME"
                    | "LANG"
                    | "PWD"
                    | "CARGO_HOME"
                    | "CARGO_TARGET_DIR"
                    | "SSL_CERT_FILE"
                    | "GIT_SSL_CAINFO"
                    | "SHLVL"
                    | "_"
            ));
        }
    }

    #[test]
    fn output_and_hanging_process_trees_are_bounded() {
        let t = Temp::new();
        let start = Instant::now();
        // Large finite stdout/stderr succeeds via middle truncation, preserving head, tail, and real exit status.
        let flood = "printf 'HEAD-OUT\\n'; seq 1 30000; printf 'TAIL-OUT\\n'; \
             printf 'HEAD-ERR\\n' >&2; seq 1 30000 >&2; printf 'TAIL-ERR\\n' >&2"
            .to_string();
        let ok = shell(&t.0, &flood, Duration::from_secs(5), false).unwrap();
        assert!(
            ok.contains("HEAD-OUT")
                && ok.contains("TAIL-OUT")
                && ok.contains("omitted from stdout")
                && ok.contains("HEAD-ERR")
                && ok.contains("TAIL-ERR")
                && ok.contains("omitted from stderr")
                && ok.len() < OUTPUT_LIMIT * 2 + 512,
            "{}",
            ok.chars().take(1000).collect::<String>()
        );
        for command in ["yes x", "yes x >&2"] {
            let flood_start = Instant::now();
            let err = shell(&t.0, command, Duration::from_millis(150), false).unwrap_err();
            assert!(
                err.contains("timed out")
                    && err.contains("exit 124")
                    && err.len() < OUTPUT_LIMIT * 2 + 512
                    && flood_start.elapsed() < Duration::from_secs(2),
                "{}",
                err.chars().take(1000).collect::<String>()
            );
        }
        let err = execute(&t.0, &json!({"op":"shell","command":"(sleep 0.5; touch leaked) & wait","timeout_ms":100}))
            .unwrap_err();
        assert!(err.contains("timed out"));
        shell(&t.0, "(sleep 0.5; touch orphan) &", Duration::from_secs(2), false).unwrap();
        thread::sleep(Duration::from_millis(600));
        assert!(!t.0.join("leaked").exists() && !t.0.join("orphan").exists());
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn codex_native_read_list_grep_and_readonly_sandbox_work() {
        let t = Temp::new();
        fs::create_dir_all(t.0.join("src")).unwrap();
        fs::write(t.0.join("src/main.rs"), "fn main() {\n    let Needle = 42;\n}\n").unwrap();
        fs::write(t.0.join("src/bin.dat"), b"\x00\x01binary").unwrap();

        let numbered = execute(&t.0, &json!({"op":"read_file","path":"src/main.rs","line_numbers":true})).unwrap();
        assert_eq!(numbered, "L1: fn main() {\nL2:     let Needle = 42;\nL3: }\n");

        let listed = execute(&t.0, &json!({"op":"list_dir","depth":2})).unwrap();
        assert!(listed.contains("src/\n") && listed.contains("src/main.rs"), "{listed}");

        let hits = execute(&t.0, &json!({"op":"grep_files","pattern":"needle","glob":"*.rs","case_insensitive":true}))
            .unwrap();
        assert_eq!(hits, "src/main.rs:2:     let Needle = 42;");

        // Read-only mode refuses edit_file & apply_patch directly and mounts /workspace read-only in bwrap.
        let ro_edit = execute_with_options(
            &t.0,
            &json!({"op":"edit_file","path":"src/main.rs","old":"42","new":"7"}),
            SandboxMode::ReadOnly,
            None,
        )
        .unwrap_err();
        assert!(ro_edit.contains("read-only"));

        let ro_shell = execute_with_options(
            &t.0,
            &json!({"op":"shell","command":"touch blocked.txt"}),
            SandboxMode::ReadOnly,
            None,
        )
        .unwrap_err();
        assert!(ro_shell.contains("Read-only file system") || ro_shell.contains("failed"), "{ro_shell}");
        assert!(!t.0.join("blocked.txt").exists());
    }
}
