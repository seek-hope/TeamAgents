//! Operator shell, local to this terminal. This module has no daemon or model client.

use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

const LIMIT: usize = 16 * 1024;

pub struct Output {
    pub status: ExitStatus,
    pub text: String,
    pub limited: bool,
}

// The TUI and foreground command share the terminal foreground group. Interrupt the
// command, not the waiting TUI. Restore the parent's handlers and cooked tty settings
// even if spawn fails or the command leaves echo disabled.
struct Foreground {
    handlers: Vec<(i32, libc::sigaction)>,
    tty: libc::termios,
}

impl Foreground {
    fn enter() -> io::Result<Self> {
        unsafe {
            let mut tty = std::mem::zeroed();
            if libc::tcgetattr(0, &mut tty) < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut guard = Self { tty, handlers: Vec::new() };
            let mut ignore: libc::sigaction = std::mem::zeroed();
            ignore.sa_sigaction = libc::SIG_IGN;
            for sig in [libc::SIGINT, libc::SIGQUIT] {
                let mut old = std::mem::zeroed();
                if libc::sigaction(sig, &ignore, &mut old) < 0 {
                    return Err(io::Error::last_os_error());
                }
                guard.handlers.push((sig, old));
            }
            Ok(guard)
        }
    }
}
impl Drop for Foreground {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.tty);
            for (sig, old) in &self.handlers {
                libc::sigaction(*sig, old, std::ptr::null_mut());
            }
        }
    }
}

pub fn run(workspace: &Path, command: Option<&str>) -> io::Result<Output> {
    let mut shell = if command.is_some() {
        Command::new("/bin/sh")
    } else {
        Command::new(std::env::var_os("SHELL").filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into()))
    };
    if let Some(text) = command {
        shell.arg("-c").arg(text);
    } else {
        shell.arg("-i");
    }
    run_foreground(workspace, shell, command.is_some())
}

pub fn codegraph(workspace: &Path, action: &str) -> io::Result<Output> {
    let mut cli = Command::new(crate::session_client::executable().map_err(io::Error::other)?);
    cli.args(["codegraph", action]);
    run_foreground(workspace, cli, action != "init")
}

fn run_foreground(workspace: &Path, mut shell: Command, capture: bool) -> io::Result<Output> {
    if !workspace.is_absolute() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no absolute session workspace"));
    }
    let _foreground = Foreground::enter()?;
    shell.current_dir(workspace);
    // Only async-signal-safe operations between fork and exec.
    unsafe {
        shell.pre_exec(|| {
            for sig in [libc::SIGINT, libc::SIGQUIT] {
                if libc::signal(sig, libc::SIG_DFL) == libc::SIG_ERR {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    if !capture {
        return Ok(Output { status: shell.status()?, text: String::new(), limited: false });
    }
    let (mut reader, writer) = UnixStream::pair()?;
    reader.set_read_timeout(Some(Duration::from_millis(60)))?;
    // Both pipes go through the same socket so the terminal only ever receives text this module has
    // filtered. Leaving stderr inherited meant a repository-influenced command (the CodeGraph run this
    // path exists for) could write raw ANSI/OSC sequences to the user's terminal — including OSC 52
    // clipboard writes and title changes — while stdout was scrubbed. One filtered path for both.
    let stderr_writer = writer.try_clone()?;
    let mut child = shell.stdout(OwnedFd::from(writer)).stderr(OwnedFd::from(stderr_writer)).spawn()?;
    drop(shell);
    let mut bytes = Vec::new();
    let mut limited = false;
    let mut finished = None;
    let mut buf = [0; 8192];
    let status = loop {
        if finished.is_none() {
            finished = child.try_wait()?.map(|s| (s, Instant::now()));
        }
        match reader.read(&mut buf) {
            Ok(0) => break child.wait()?,
            Ok(n) => {
                let keep = n.min(LIMIT - bytes.len());
                bytes.extend_from_slice(&buf[..keep]);
                limited |= keep < n;
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => {
                if let Some((status, _)) = finished {
                    break status;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
        // Background descendants can inherit stdout. They do not own the editor; do not
        // wait for them or imply their remote or filesystem effects stopped.
        if let Some((status, at)) = finished {
            if at.elapsed() >= Duration::from_millis(180) {
                limited = true;
                break status;
            }
        }
    };
    let text =
        String::from_utf8_lossy(&bytes).chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect();
    Ok(Output { status, text, limited })
}
