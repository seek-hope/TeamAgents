//! User-configured hooks: an external command told about engine events.
//!
//! Hooks are the user's own programs (path from the user config, never from a
//! model), so they run on the host rather than inside a member sandbox: the
//! event name is argv[1] and the full event JSON arrives on stdin.
//! Fire-and-forget — a hook that hangs is killed after [`HOOK_TIMEOUT`] and
//! never blocks a turn; failures only reach stderr.

use serde_json::{json, Value as Json};
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Hooks {
    command: Vec<String>,
    session_id: String,
}

impl Hooks {
    /// None when the user configured no hook (`[hooks] notify = [...]`).
    pub fn from_config(config: &teamagents_core::models::UserConfig, session_id: &str) -> Option<Arc<Self>> {
        let command = config.hooks.notify.clone();
        if command.is_empty() || command[0].trim().is_empty() {
            return None;
        }
        Some(Arc::new(Self { command, session_id: session_id.to_string() }))
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
    fn hooks_receive_the_event_name_and_json_on_stdin() {
        let dir = std::env::temp_dir().join(format!("ta-hook-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("hook.sh");
        let out = dir.join("seen.txt");
        std::fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' \"$1\" > {}\ncat >> {}\n", out.display(), out.display())).unwrap();
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
