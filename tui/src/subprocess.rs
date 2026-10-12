//! Bounded subprocess I/O without reader threads that can outlive cancellation.
use std::{
    io::{Read, Write},
    os::fd::AsFd,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

struct OwnedChild {
    child: Child,
    /// Kill the whole process group on drop. Set on every failure/cancel/timeout path;
    /// cleared on success so a child that legitimately started a long-lived descendant
    /// (e.g. `sessions open` starting a daemon) is not torn down with it.
    kill_group: bool,
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.kill_group {
            // The child is its process-group leader (capture sets process_group(0)); a negative
            // pid signals the group, reaping grandchildren a plain kill would leave behind.
            // If the group is already gone this fails with ESRCH, which is fine.
            unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
        }
        // wait also reaps an already-exited child. No pipe reader must join first.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn nonblocking(fd: &impl AsFd) -> Result<(), String> {
    use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
    let flags = fcntl_getfl(fd).map_err(|e| e.to_string())?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(|e| e.to_string())
}

fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>, limit: usize) -> Result<bool, String> {
    let mut buffer = [0; 16 * 1024];
    // Bound each sweep so a flooding stream cannot starve cancellation or the other pipe.
    for _ in 0..16 {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                if n > limit.saturating_sub(bytes.len()) {
                    return Err("subprocess output exceeds byte limit".into());
                }
                bytes.extend_from_slice(&buffer[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("subprocess read: {e}")),
        }
    }
    Ok(false)
}

pub(crate) fn capture(
    command: &mut Command,
    input: &[u8],
    output_limit: usize,
    deadline: Instant,
    cancelled: impl Fn() -> bool,
) -> Result<Output, String> {
    if cancelled() {
        return Err("subprocess cancelled".into());
    }
    if Instant::now() >= deadline {
        return Err("subprocess timed out".into());
    }
    use std::os::unix::process::CommandExt;
    // The child leads its own process group, so failure paths can reap the whole tree,
    // including grandchildren that inherited the pipes.
    command.process_group(0);
    let mut child = OwnedChild {
        child: command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("subprocess spawn: {e}"))?,
        kill_group: true,
    };
    let mut stdin = child.child.stdin.take();
    let mut stdout = child.child.stdout.take().ok_or("subprocess stdout unavailable")?;
    let mut stderr = child.child.stderr.take().ok_or("subprocess stderr unavailable")?;
    nonblocking(stdin.as_ref().ok_or("subprocess stdin unavailable")?)?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let mut written = 0;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut out_done, mut err_done) = (false, false);
    let mut status = None;
    loop {
        if cancelled() {
            return Err("subprocess cancelled and reaped".into());
        }
        if Instant::now() >= deadline {
            return Err("subprocess timed out and reaped".into());
        }
        if written == input.len() {
            stdin.take();
        }
        if let Some(pipe) = &mut stdin {
            let end = (written + 16 * 1024).min(input.len());
            match pipe.write(&input[written..end]) {
                Ok(0) => return Err("subprocess input closed".into()),
                Ok(n) => written += n,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {}
                Err(e) => return Err(format!("subprocess input: {e}")),
            }
        }
        if !out_done {
            out_done = drain(&mut stdout, &mut out, output_limit)?;
        }
        if !err_done {
            err_done = drain(&mut stderr, &mut err, output_limit)?;
        }
        if status.is_none() {
            status = child.child.try_wait().map_err(|e| format!("subprocess wait: {e}"))?;
        }
        if out_done && err_done {
            if let Some(status) = status {
                // Success: the direct child exited and its pipes are done. Descendants that
                // outlive it by design (a daemon started by `sessions open`) keep running.
                child.kill_group = false;
                return Ok(Output { status, stdout: out, stderr: err });
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    fn python(code: &str) -> Command {
        let mut command = Command::new("python3");
        command.args(["-c", code]);
        command
    }

    #[test]
    fn drains_both_large_pipes_while_writing_input() {
        let output = capture(&mut python("import sys\nsys.stdout.buffer.write(b'o'*300000)\nsys.stderr.buffer.write(b'e'*300000)\nassert len(sys.stdin.buffer.read())==300000"),
            &vec![b'i'; 300000], 400000, Instant::now()+Duration::from_secs(5), || false).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, vec![b'o'; 300000]);
        assert_eq!(output.stderr, vec![b'e'; 300000]);
    }

    #[test]
    fn overflow_timeout_and_cancel_reap_then_next_operation_works() {
        for (code, reason) in
            [("import sys; sys.stderr.write('x'*100000)", "limit"), ("import time; time.sleep(30)", "timed out")]
        {
            let started = Instant::now();
            let error =
                capture(&mut python(code), &[], 1000, started + Duration::from_millis(200), || false).unwrap_err();
            assert!(error.contains(reason), "{error}");
            assert!(started.elapsed() < Duration::from_secs(2));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let setter = Arc::clone(&cancel);
        let thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            setter.store(true, Ordering::Relaxed);
        });
        let error = capture(
            &mut python("import time; time.sleep(30)"),
            &[],
            1000,
            Instant::now() + Duration::from_secs(5),
            || cancel.load(Ordering::Relaxed),
        )
        .unwrap_err();
        thread.join().unwrap();
        assert!(error.contains("cancelled"));
        let output =
            capture(&mut python("print('next')"), &[], 1000, Instant::now() + Duration::from_secs(2), || false)
                .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"next\n");
    }

    #[test]
    fn inherited_pipe_does_not_defeat_deadline_after_direct_child_exits() {
        // The fixture descendant exits itself; capture owns only the direct child, not arbitrary descendants.
        let start = Instant::now();
        let error = capture(
            &mut python("import subprocess; subprocess.Popen(['python3','-c','import time; time.sleep(0.5)'])"),
            &[],
            1000,
            start + Duration::from_millis(100),
            || false,
        )
        .unwrap_err();
        assert!(error.contains("timed out"));
        assert!(start.elapsed() < Duration::from_millis(450));
    }

    #[test]
    fn timeout_kills_the_whole_process_group_including_grandchildren() {
        let marker = std::env::temp_dir().join(format!("ta-grandchild-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let code = format!(
            "import subprocess; subprocess.Popen(['python3','-c',\
             'import time,pathlib; time.sleep(0.4); pathlib.Path({:?}).touch()'])",
            marker.to_string_lossy()
        );
        let start = Instant::now();
        let error = capture(&mut python(&code), &[], 1000, start + Duration::from_millis(100), || false).unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        // The grandchild would write the marker at 0.4 s if it survived the group kill at the deadline.
        thread::sleep(Duration::from_millis(700));
        assert!(!marker.exists(), "grandchild survived the process-group kill");
    }
}
