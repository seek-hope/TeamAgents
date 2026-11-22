//! Background read-only monitor of sibling sessions under the same state registry (D-532).
//! Probes reachable daemon sockets periodically without modifying SQLite or dispatching commands.

use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use serde_json::json;

use crate::{daemon_client::DaemonClient, team, v2app::V2App};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveSession {
    pub id: String,
    pub name: String,
    pub root: String,
    pub live: bool,
    pub attention: usize,
    pub approvals: usize,
    pub busy: usize,
}

pub struct LiveMonitor {
    targets: Arc<Mutex<Vec<(String, String, String)>>>,
    receiver: mpsc::Receiver<Vec<LiveSession>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Default for LiveMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveMonitor {
    pub fn new() -> Self {
        let targets = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let thread_targets = Arc::clone(&targets);
        let thread_stop = Arc::clone(&stop);
        let join = thread::Builder::new()
            .name("teamagents-live-monitor".into())
            .spawn(move || {
                let mut last_targets;
                while !thread_stop.load(Ordering::Relaxed) {
                    let current = thread_targets.lock().map(|g| g.clone()).unwrap_or_default();
                    let snapshot = current
                        .iter()
                        .take(8)
                        .map(|(id, name, root)| {
                            let (live, attention, approvals, busy) = probe_session(root);
                            LiveSession {
                                id: id.clone(),
                                name: name.clone(),
                                root: root.clone(),
                                live,
                                attention,
                                approvals,
                                busy,
                            }
                        })
                        .collect();
                    let _ = tx.send(snapshot);
                    last_targets = current;
                    for _ in 0..30 {
                        if thread_stop.load(Ordering::Relaxed) {
                            return;
                        }
                        if thread_targets.lock().is_ok_and(|g| *g != last_targets) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            })
            .ok();
        Self { targets, receiver: rx, stop, join }
    }

    pub fn set_targets(&self, targets: &[(String, String, String)]) {
        if let Ok(mut guard) = self.targets.lock() {
            if *guard != targets {
                *guard = targets.to_vec();
            }
        }
    }

    pub fn poll(&self, app: &mut V2App) -> bool {
        let mut latest = None;
        while let Ok(rows) = self.receiver.try_recv() {
            latest = Some(rows);
        }
        if let Some(rows) = latest {
            if app.live_sessions != rows {
                app.live_sessions = rows;
                return true;
            }
        }
        false
    }
}

impl Drop for LiveMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn probe_session(root: &str) -> (bool, usize, usize, usize) {
    let sock = Path::new(root).join("daemon.sock");
    if !sock.exists() {
        return (false, 0, 0, 0);
    }
    let Ok(mut client) = DaemonClient::connect(&sock) else {
        return (false, 0, 0, 0);
    };
    let Ok((snapshot, _)) = client.checkpoint() else {
        return (false, 0, 0, 0);
    };
    let tasks = snapshot["tasks"].as_array().cloned().unwrap_or_default();
    let instances = snapshot["instances"].as_array().cloned().unwrap_or_default();
    let attention = instances.iter().filter(|row| team::Team::member_attention(row, &tasks)).count();
    let busy = instances
        .iter()
        .filter(|row| {
            row["lifecycle"].as_str() == Some("ACTIVE")
                && matches!(row["phase"].as_str(), Some("MODEL_PENDING" | "TOOLS_PENDING" | "COMPLETION_PENDING"))
        })
        .count();
    let approvals =
        client.call("approvals", json!({})).ok().and_then(|r| r["approvals"].as_array().map(Vec::len)).unwrap_or(0);
    (true, attention, approvals, busy)
}
