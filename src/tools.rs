//! Workspace tools. The caller authorizes dispatch; every operation enforces its own boundary.
use crate::{
    net::Network,
    sandbox::{self, fd_path as fd_dir},
    Result,
};
use serde_json::Value;
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::OwnedFd,
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

const FILE_LIMIT: usize = 256 * 1024;
const OUTPUT_LIMIT: usize = 64 * 1024; // per stream
const SHELL_TIMEOUT: Duration = Duration::from_secs(60);

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
        return Err("file exceeds 256 KiB".into());
    }
    let mut bytes = Vec::new();
    f.take(FILE_LIMIT as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("read file: {e}"))?;
    if bytes.len() > FILE_LIMIT {
        return Err("file exceeds 256 KiB".into());
    }
    Ok((String::from_utf8(bytes).map_err(|_| "file is not UTF-8 text")?, meta))
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
        return Err("edit exceeds 256 KiB".into());
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
            return Err("edited file exceeds 256 KiB".into());
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

// No blocking reader threads: even a descendant that escapes the group and inherits a writer cannot hold us open.
fn drain(stream: &mut UnixStream, bytes: &mut Vec<u8>, label: &str) -> Result<bool> {
    let mut buf = [0; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                let keep = n.min(OUTPUT_LIMIT - bytes.len());
                bytes.extend_from_slice(&buf[..keep]);
                if keep != n {
                    return Err(format!("{label} exceeds 64 KiB; output truncated"));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
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

fn shell(workspace: &Path, command: &str, timeout: Duration) -> Result<String> {
    let (mut stdout, out_fd) = pipe().map_err(|e| format!("shell stdout: {e}"))?;
    let (mut stderr, err_fd) = pipe().map_err(|e| format!("shell stderr: {e}"))?;
    let stage = sandbox::Stage::new(workspace)?;
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
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut connected = false;
    let mut failure = (|| -> Result<()> {
        network.connect(&child)?;
        connected = true;
        loop {
            drain(&mut stdout, &mut out, "stdout")?;
            drain(&mut stderr, &mut err, "stderr")?;
            if exited(&child).map_err(|e| format!("poll shell: {e}"))? {
                return Ok(());
            }
            if start.elapsed() >= timeout {
                return Err(format!("shell timed out after {}ms", timeout.as_millis()));
            }
            thread::sleep(Duration::from_millis(10));
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
            loop {
                let out_done = drain(&mut stdout, &mut out, "stdout")?;
                let err_done = drain(&mut stderr, &mut err, "stderr")?;
                if out_done && err_done {
                    return Ok(());
                }
                if Instant::now() >= end {
                    return Err("shell descendants still hold output open; capture incomplete".into());
                }
                thread::sleep(Duration::from_millis(5));
            }
        })()
        .err();
    }
    let text = format!("stdout:\n{}\nstderr:\n{}", String::from_utf8_lossy(&out), String::from_utf8_lossy(&err));
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

pub fn execute(workspace: &Path, action: &Value) -> Result<String> {
    match field(action, "op")? {
        "read_file" => {
            let lock = sandbox::Lock::workspace(workspace)?;
            let (dir, target) = lock.parent(Path::new(field(action, "path")?))?;
            load(&dir, &target).map(|(text, _)| text)
        }
        "edit_file" => {
            let lock = sandbox::Lock::workspace(workspace)?;
            let (dir, target) = lock.parent(Path::new(field(action, "path")?))?;
            edit(&dir, &target, field(action, "old")?, field(action, "new")?)
        }
        "shell" => shell(workspace, field(action, "command")?, SHELL_TIMEOUT),
        _ => Err("unknown workspace tool".into()),
    }
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
        assert!(t.read("big").unwrap_err().contains("256 KiB"));
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
        let value = shell(&t.0, "env", Duration::from_secs(2)).unwrap();
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
        for (command, stream) in [("yes x", "stdout"), ("yes x >&2", "stderr")] {
            let err = shell(&t.0, command, Duration::from_secs(3)).unwrap_err();
            assert!(
                err.contains(&format!("{stream} exceeds")) && err.len() < OUTPUT_LIMIT + 200,
                "{}",
                err.chars().take(1000).collect::<String>()
            );
        }
        let err = shell(&t.0, "(sleep 0.5; touch leaked) & wait", Duration::from_millis(100)).unwrap_err();
        assert!(err.contains("timed out"));
        shell(&t.0, "(sleep 0.5; touch orphan) &", Duration::from_secs(2)).unwrap();
        thread::sleep(Duration::from_millis(600));
        assert!(!t.0.join("leaked").exists() && !t.0.join("orphan").exists());
        assert!(start.elapsed() < Duration::from_secs(4));
    }
}
