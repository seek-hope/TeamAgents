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
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, Sender<Result<Json, String>>>>>,
    pushes: Mutex<Receiver<Push>>,
    next_id: Mutex<u64>,
}

impl Worker {
    /// Spawn the engine worker (`teamagents serve`); stderr is inherited so
    /// runtime errors surface in the terminal.
    pub fn spawn(engine_bin: &str) -> std::io::Result<Worker> {
        let mut child = Command::new(engine_bin)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
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
        Ok(Worker { child, stdin, pending, pushes: Mutex::new(push_rx), next_id: Mutex::new(1) })
    }

    pub fn call(&self, method: &str, params: Json) -> Result<Json, String> {
        let id = {
            let mut g = self.next_id.lock().unwrap();
            let id = *g;
            *g += 1;
            id
        };
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        let line = json!({"id": id, "method": method, "params": params}).to_string() + "\n";
        {
            let mut stdin = self.stdin.lock().unwrap();
            stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()).map_err(|e| e.to_string())?;
        }
        rx.recv_timeout(Duration::from_secs(120))
            .map_err(|_| format!("worker call {method} timed out"))?
    }

    /// Core passthrough with session_id injected by the worker.
    pub fn core(&self, sessionless_method: &str, params: Json) -> Result<Json, String> {
        self.call("call", json!({"method": sessionless_method, "params": params}))
    }

    pub fn try_push(&self) -> Option<Push> {
        self.pushes.lock().unwrap().try_recv().ok()
    }

    pub fn close(mut self) {
        // ponytail: this call can block exit for the full 120s call timeout while
        // the engine awaits inflight turns; upgrade path: close on a background
        // thread and exit immediately
        let _ = self.call("close", json!({}));
        // the worker awaits inflight turns on close; never let that hang quit
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                _ if std::time::Instant::now() > deadline => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
}
