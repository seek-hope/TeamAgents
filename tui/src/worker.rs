//! Client for the engine's `serve` stdio JSON-lines service (teamagents
//! engine, src/worker.rs): one request per line, id-matched responses, plus
//! unsolicited {"push": ...} messages forwarded to the UI.

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Push {
    pub kind: String,
    pub run_id: String,
    pub agent_id: String,
    pub text: String,
}

pub struct Worker {
    // Mutex so `kill(&self)` can reap the child even while other Arc
    // holders keep the Worker alive (exit path when try_unwrap fails)
    child: Mutex<Child>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, Sender<Result<Json, String>>>>>,
    pushes: Mutex<Receiver<Push>>,
    next_id: Mutex<u64>,
}

/// stderr target for the engine child: append to `engine-stderr.log` in
/// the shared state dir, falling back to null so a log failure never
/// blocks engine startup.
fn engine_stderr() -> Stdio {
    let dir = crate::i18n::state_dir();
    let path = dir.join("engine-stderr.log");
    std::fs::create_dir_all(&dir)
        .and_then(|_| {
            // diagnostic log, old bytes are expendable: truncate past 1MB at
            // spawn (within one session it can still grow past 1MB). Two opens,
            // because OpenOptions rejects append+truncate in one call; every
            // live handle stays O_APPEND so a second TUI sharing the state dir
            // never overwrites lines the other instance just wrote
            if std::fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
                std::fs::File::create(&path)?; // truncate, closed on drop
            }
            std::fs::OpenOptions::new().create(true).append(true).open(&path)
        })
        .map(Stdio::from)
        .unwrap_or(Stdio::null())
}

impl Worker {
    /// Spawn the engine worker (`teamagents serve`); stderr goes to
    /// `engine-stderr.log` in the shared state dir (append) so engine
    /// startup failures stay diagnosable; inherited stderr would corrupt
    /// the TUI frame, so an unwritable log falls back to null.
    pub fn spawn(engine_bin: &str) -> std::io::Result<Worker> {
        let mut child = Command::new(engine_bin)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(engine_stderr())
            .spawn()?;
        let stdout = child.stdout.take().expect("piped");
        let stdin = Arc::new(Mutex::new(child.stdin.take().expect("piped")));
        let pending: Arc<Mutex<HashMap<u64, Sender<Result<Json, String>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (push_tx, push_rx) = channel::<Push>();
        let pending2 = pending.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<Json>(&line) else { continue };
                if let Some(kind) = msg.get("push").and_then(|v| v.as_str()) {
                    let _ = push_tx.send(Push {
                        kind: kind.to_string(),
                        run_id: msg.get("run_id").and_then(|v| v.as_str()).unwrap_or("").into(),
                        agent_id: msg.get("agent_id").and_then(|v| v.as_str()).unwrap_or("").into(),
                        text: msg.get("text").and_then(|v| v.as_str()).unwrap_or("").into(),
                    });
                    continue;
                }
                let Some(id) = msg.get("id").and_then(|v| v.as_u64()) else { continue };
                let slot = pending2.lock().unwrap().remove(&id);
                if let Some(tx) = slot {
                    let r = if let Some(e) = msg.get("error").and_then(|v| v.as_str()) {
                        Err(e.to_string())
                    } else {
                        Ok(msg.get("result").cloned().unwrap_or(Json::Null))
                    };
                    let _ = tx.send(r);
                }
            }
            // worker stdout closed: fail everything still pending
            for (_, tx) in pending2.lock().unwrap().drain() {
                let _ = tx.send(Err("worker exited".into()));
            }
        });
        Ok(Worker { child: Mutex::new(child), stdin, pending, pushes: Mutex::new(push_rx), next_id: Mutex::new(1) })
    }

    pub fn call(&self, method: &str, params: Json) -> Result<Json, String> {
        self.call_timeout(method, params, Duration::from_secs(120))
    }

    /// `call` with a caller-chosen timeout; short polls must not freeze the
    /// UI behind a wedged engine.
    pub fn call_timeout(&self, method: &str, params: Json, timeout: Duration) -> Result<Json, String> {
        let id = {
            let mut g = self.next_id.lock().unwrap();
            let id = *g;
            *g += 1;
            id
        };
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        let line = json!({"id": id, "method": method, "params": params}).to_string() + "\n";
        let result: Result<Json, String> = (|| {
            {
                // ponytail: with a wedged engine the pipe buffer fills after
                // ~1h of writes and this write_all blocks the UI thread for
                // good; upgrade path: dedicated writer thread or O_NONBLOCK
                // with a write deadline
                let mut stdin = self.stdin.lock().unwrap();
                stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()).map_err(|e| e.to_string())?;
            }
            rx.recv_timeout(timeout).map_err(|_| format!("worker call {method} timed out"))?
        })();
        if result.is_err() {
            // a failed call never gets a response; drop the slot so a wedged
            // engine does not leak one pending entry per timed-out poll
            self.pending.lock().unwrap().remove(&id);
        }
        result
    }

    /// Core passthrough with session_id injected by the worker.
    pub fn core(&self, sessionless_method: &str, params: Json) -> Result<Json, String> {
        self.call("call", json!({"method": sessionless_method, "params": params}))
    }

    /// `core` with a caller-chosen timeout (see `call_timeout`).
    pub fn core_timeout(&self, sessionless_method: &str, params: Json, timeout: Duration) -> Result<Json, String> {
        self.call_timeout("call", json!({"method": sessionless_method, "params": params}), timeout)
    }

    pub fn try_push(&self) -> Option<Push> {
        self.pushes.lock().unwrap().try_recv().ok()
    }

    /// Best-effort kill of the engine child; used at exit when another
    /// thread still holds an Arc so `close` cannot run — a wedged engine
    /// must not orphan holding the session flock.
    pub fn kill(&self) {
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }

    pub fn close(self) {
        // ponytail: this call can block exit for the full 120s call timeout while
        // the engine awaits inflight turns; upgrade path: close on a background
        // thread and exit immediately
        let _ = self.call("close", json!({}));
        // the worker awaits inflight turns on close; never let that hang quit
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            {
                let mut child = self.child.lock().unwrap();
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    _ if std::time::Instant::now() > deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Engine that accepts requests but never replies (wedged engine).
    fn silent_engine(tag: &str) -> String {
        // pid+tag: cargo tests share one process, so the pid alone is not unique
        let path = std::env::temp_dir().join(format!("teamagents-tui-silent-engine-{}-{tag}.sh", std::process::id()));
        std::fs::write(&path, "#!/bin/bash\nwhile read -r l; do :; done\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn spawn_captures_engine_stderr_to_state_dir_log() {
        let dir = std::env::temp_dir().join(format!("teamagents-tui-stderr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let old_xdg = std::env::var("XDG_STATE_HOME").ok();
        std::env::set_var("XDG_STATE_HOME", &dir);
        let engine = std::env::temp_dir().join(format!("teamagents-tui-stderr-engine-{}.sh", std::process::id()));
        std::fs::write(&engine, "#!/bin/bash\necho boom >&2\n").unwrap();
        // >1MB of stale log: the spawn must truncate it, not append
        let log_dir = dir.join("teamagents");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::write(log_dir.join("engine-stderr.log"), vec![b'x'; 1_100_000]).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
        let worker = Worker::spawn(engine.to_str().unwrap()).expect("spawn stderr mock");
        // engine exits right after writing; give it a moment, then check the log
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let log = dir.join("teamagents").join("engine-stderr.log");
        while std::time::Instant::now() < deadline {
            if std::fs::read_to_string(&log).map(|s| s.contains("boom")).unwrap_or(false) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let content = std::fs::read_to_string(&log).expect("engine-stderr.log must exist");
        worker.kill();
        // restore the env we clobbered; the temp dir is pid-unique, drop it whole
        match old_xdg {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&engine);
        assert!(content.contains("boom"), "engine stderr lost: {content:?}");
        assert!(content.len() < 1_000_000, "stale >1MB log was not truncated");
    }

    #[test]
    fn kill_reaps_child_when_try_unwrap_fails() {
        let worker = Arc::new(Worker::spawn(&silent_engine("kill")).expect("spawn silent mock"));
        let _extra = Arc::clone(&worker);
        // same shape as the exit path: extra ref held, try_unwrap cannot win
        assert!(Arc::try_unwrap(Arc::clone(&worker)).is_err());
        worker.kill();
        let status = worker.child.lock().unwrap().try_wait().unwrap();
        assert!(status.is_some(), "kill() must reap the engine child");
    }

    #[test]
    fn call_timeout_returns_promptly_and_drops_pending() {
        let worker = Worker::spawn(&silent_engine("kill")).expect("spawn silent mock");
        let start = std::time::Instant::now();
        let r = worker.call_timeout("state", json!({}), Duration::from_millis(300));
        let elapsed = start.elapsed();
        assert!(r.is_err(), "a silent engine must fail the call");
        assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}, expected ~300ms");
        assert!(
            worker.pending.lock().unwrap().is_empty(),
            "timed-out call leaked a pending entry"
        );
    }
}
