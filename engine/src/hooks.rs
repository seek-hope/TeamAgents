//! User-configured hooks: an external command told about engine events.
//!
//! Hooks are the user's own programs (path from the user config, never from a
//! model), so they run on the host rather than inside a member sandbox: the
//! event name is argv[1] and the full event JSON arrives on stdin.
//! Fire-and-forget — a hook that hangs is killed after [`HOOK_TIMEOUT`] and
//! never blocks a turn; failures only reach stderr.

use serde_json::{json, Value as Json};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// A configured argv: not just non-empty, but with a non-empty program.
fn configured(argv: &[String]) -> Option<Vec<String>> {
    let first = argv.first()?;
    (!first.trim().is_empty()).then(|| argv.to_vec())
}

pub struct Hooks {
    command: Vec<String>,
    pre_tool: Vec<String>,
    session_id: String,
}

impl Hooks {
    /// None when the user configured neither `[hooks] notify` nor `pre_tool`.
    pub fn from_config(config: &teamagents_core::models::UserConfig, session_id: &str) -> Option<Arc<Self>> {
        let command = configured(&config.hooks.notify);
        let pre_tool = configured(&config.hooks.pre_tool);
        if command.is_none() && pre_tool.is_none() {
            return None;
        }
        Some(Arc::new(Self {
            command: command.unwrap_or_default(),
            pre_tool: pre_tool.unwrap_or_default(),
            session_id: session_id.to_string(),
        }))
    }

    pub fn has_pre_tool(&self) -> bool {
        !self.pre_tool.is_empty()
    }

    /// Blocking policy check for one native tool call: `Some(reason)` denies it.
    /// Exit 0 allows; exit 2 denies with the hook's stderr (the Claude Code
    /// convention); anything else (other exit codes, spawn failure, timeout)
    /// allows and logs — a broken hook must not stop the team from working.
    pub fn deny_reason(&self, payload: &Json) -> Option<String> {
        if self.pre_tool.is_empty() {
            return None;
        }
        let body = json!({"event": "pre_tool", "session_id": self.session_id, "payload": payload}).to_string();
        let mut child = match std::process::Command::new(&self.pre_tool[0])
            .args(&self.pre_tool[1..])
            .arg("pre_tool")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                eprintln!("pre_tool hook: cannot start {:?}: {error}", self.pre_tool);
                return None;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(body.as_bytes());
        }
        // read stderr while waiting so a chatty hook cannot fill the pipe; the text
        // lands in a shared buffer, so a grandchild holding the pipe open cannot
        // stall this call (killing the hook only kills the hook itself)
        let buffer = Arc::new(Mutex::new(String::new()));
        let reader = child.stderr.take().map(|mut pipe| {
            let buffer = buffer.clone();
            std::thread::spawn(move || {
                use std::io::Read;
                let mut chunk = [0u8; 1024];
                while let Ok(read) = pipe.read(&mut chunk) {
                    if read == 0 {
                        break;
                    }
                    if let Ok(mut text) = buffer.lock() {
                        text.push_str(&String::from_utf8_lossy(&chunk[..read]));
                    }
                }
            })
        });
        let deadline = std::time::Instant::now() + HOOK_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Ok(None) => {
                    eprintln!("pre_tool hook: {:?} timed out after {HOOK_TIMEOUT:?}, allowing the call", self.pre_tool);
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                Err(error) => {
                    eprintln!("pre_tool hook: wait failed: {error}, allowing the call");
                    break None;
                }
            }
        };
        if let Some(handle) = reader {
            let grace = std::time::Instant::now() + Duration::from_secs(1);
            while !handle.is_finished() && std::time::Instant::now() < grace {
                std::thread::sleep(Duration::from_millis(10));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
        let stderr = buffer.lock().map(|text| text.clone()).unwrap_or_default();
        match status {
            Some(status) if status.code() == Some(0) => None,
            Some(status) if status.code() == Some(2) => {
                let reason = stderr.lines().next().unwrap_or("denied by pre_tool hook").trim().to_string();
                Some(if reason.is_empty() { "denied by pre_tool hook".into() } else { reason })
            }
            Some(status) => {
                eprintln!("pre_tool hook: exited with {status}, allowing the call");
                None
            }
            None => None,
        }
    }

    /// Run the hook for one event. Returns immediately.
    /// ponytail: one detached thread per event; batch them if a hook per tool
    /// call ever shows up in a profile.
    pub fn fire(self: &Arc<Self>, event: &str, payload: Json) {
        let command = self.command.clone();
        let event = event.to_string();
        let body = json!({"event": event, "session_id": self.session_id, "payload": payload}).to_string();
        std::thread::spawn(move || {
            let mut child = match std::process::Command::new(&command[0])
                .args(&command[1..])
                .arg(&event)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(error) => {
                    eprintln!("hook {event}: cannot start {command:?}: {error}");
                    return;
                }
            };
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(body.as_bytes());
                // dropping stdin closes it, which most hooks wait for
            }
            let deadline = std::time::Instant::now() + HOOK_TIMEOUT;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success() {
                            eprintln!("hook {event}: {command:?} exited with {status}");
                        }
                        return;
                    }
                    Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                    Ok(None) => {
                        eprintln!("hook {event}: {command:?} timed out after {HOOK_TIMEOUT:?}");
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                    Err(error) => {
                        eprintln!("hook {event}: wait failed: {error}");
                        return;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_tool_policy_decides_by_exit_code() {
        let dir = std::env::temp_dir().join(format!("ta-pretool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.to_string_lossy().into_owned()
        };
        let allow = write("allow.sh", "cat > /dev/null; exit 0");
        let deny = write("deny.sh", "cat > /dev/null; echo 'write_file is not allowed here' >&2; exit 2");
        let broken = write("broken.sh", "cat > /dev/null; exit 7");

        let hooks = |command: &str| {
            let mut config = teamagents_core::models::UserConfig::default();
            config.hooks.pre_tool = vec![command.to_string()];
            Hooks::from_config(&config, "s1").expect("configured hook")
        };
        let payload = json!({"tool": "write_file", "arguments": {"path": "a.txt"}});
        assert!(hooks(&allow).deny_reason(&payload).is_none(), "exit 0 allows");
        let reason = hooks(&deny).deny_reason(&payload).expect("exit 2 denies");
        assert!(reason.contains("not allowed here"), "stderr is the reason: {reason}");
        assert!(hooks(&broken).deny_reason(&payload).is_none(), "a broken hook must not brick the agent");

        // a hook that hangs is bounded the same way (and allows)
        let slow = write("slow.sh", "cat > /dev/null; sleep 30");
        let mut config = teamagents_core::models::UserConfig::default();
        config.hooks.pre_tool = vec![slow];
        let started = std::time::Instant::now();
        assert!(Hooks::from_config(&config, "s1").unwrap().deny_reason(&payload).is_none());
        assert!(started.elapsed() < Duration::from_secs(20), "the timeout bounds a hanging hook");

        // notify-only config has no policy hook
        let mut notify_only = teamagents_core::models::UserConfig::default();
        notify_only.hooks.notify = vec!["/bin/true".into()];
        assert!(!Hooks::from_config(&notify_only, "s1").unwrap().has_pre_tool());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hooks_receive_the_event_name_and_json_on_stdin() {
        let dir = std::env::temp_dir().join(format!("ta-hook-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("hook.sh");
        let out = dir.join("seen.txt");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s\\n' \"$1\" > {}\ncat >> {}\n", out.display(), out.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut config = teamagents_core::models::UserConfig::default();
        config.hooks.notify = vec![script.to_string_lossy().into_owned()];
        let hooks = Hooks::from_config(&config, "s1").expect("configured hook");
        hooks.fire("tool_call", json!({"tool": "edit_file", "ok": true}));

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut text = String::new();
        while std::time::Instant::now() < deadline {
            text = std::fs::read_to_string(&out).unwrap_or_default();
            if text.contains("tool_call") && text.contains("edit_file") {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(text.starts_with("tool_call\n"), "argv[1] is the event: {text}");
        assert!(text.contains("session_id"), "{text}");
        assert!(text.contains("edit_file"), "the payload travels on stdin: {text}");

        // no hook configured -> nothing to run
        assert!(Hooks::from_config(&teamagents_core::models::UserConfig::default(), "s1").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
