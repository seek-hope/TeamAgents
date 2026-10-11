//! D-446: asynchronous client of the engine's session CLI. No database or registry access in the TUI.
use crate::{daemon_client::DaemonClient, sessions::SessionAction, text::Composer, v2app::V2App};
use serde_json::{json, Value as Json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

enum Reply {
    Data(Json),
    Opened { client: DaemonClient, app: Box<V2App> },
}

struct Pending {
    receiver: mpsc::Receiver<(SessionAction, Result<Reply, String>)>,
    cancel: Arc<AtomicBool>,
    search: bool,
    join: Option<thread::JoinHandle<()>>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        // Only search is cancellable. Explicit open/delete operations keep their engine ownership
        // when the UI detaches; they already have a bounded subprocess wait.
        if self.search {
            self.cancel.store(true, Ordering::Relaxed);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }
}

pub struct SessionClient {
    root: String,
    pending: Option<Pending>,
    drafts: BTreeMap<String, (Composer, crate::attachments::Attachments)>,
}

impl SessionClient {
    pub fn new(root: &str) -> Self {
        Self { root: root.into(), pending: None, drafts: BTreeMap::new() }
    }

    pub fn start(&mut self, action: SessionAction, app: &mut V2App) {
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let search = matches!(action, SessionAction::Search { .. });
        if let Some(browser) = app.browser.as_mut() {
            browser.busy = true;
        }
        let root = self.root.clone();
        let join = thread::spawn(move || {
            let reply = execute(&root, &action, &worker_cancel);
            if !worker_cancel.load(Ordering::Relaxed) {
                let _ = tx.send((action, reply));
            }
        });
        self.pending = Some(Pending { receiver: rx, cancel, search, join: Some(join) });
    }

    /// Cancel the outstanding session CLI operation and drop its receipt channel. The child is
    /// killed by the worker, while the dropped channel prevents a late result from touching a
    /// newly opened browser or a superseding search.
    pub fn cancel_search(&mut self, app: &mut V2App) {
        if self.pending.as_ref().is_some_and(|pending| !pending.search) {
            return;
        }
        self.pending.take();
        if let Some(browser) = app.browser.as_mut() {
            browser.busy = false;
        }
    }

    /// Switch atomically after handshake, checkpoint, history and approvals all succeeded.
    /// Drafts use state-root identity: every daemon may legitimately report the same session_id.
    pub fn poll(&mut self, client: &mut DaemonClient, app: &mut V2App) -> bool {
        let received = match self.pending.as_ref() {
            None => return false,
            Some(pending) => match pending.receiver.try_recv() {
                Ok(reply) => Some(reply),
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => None,
            },
        };
        let Some((action, reply)) = received else {
            self.pending = None;
            if let Some(browser) = app.browser.as_mut() {
                browser.fail("Session operation ended without a receipt.".into());
            }
            return true;
        };
        self.pending = None;
        match reply {
            Err(error) => {
                if let SessionAction::Search { ref query, generation } = action {
                    if let Some(browser) = app.browser.as_mut() {
                        browser.fail_search(error, query, generation);
                    }
                } else if let Some(browser) = app.browser.as_mut() {
                    browser.fail(error);
                }
            }
            Ok(Reply::Opened { client: next_client, app: mut next_app }) => {
                let draft = std::mem::replace(&mut app.composer, Composer::new(vec![]));
                let attachments = std::mem::take(&mut app.attachments);
                self.drafts.insert(app.session_root.clone(), (draft, attachments));
                if let Some((draft, attachments)) = self.drafts.remove(&next_app.session_root) {
                    next_app.composer = draft;
                    next_app.attachments = attachments;
                }
                *client = next_client;
                *app = *next_app;
            }
            Ok(Reply::Data(data)) => match action {
                SessionAction::List => {
                    if let Some(browser) = app.browser.as_mut() {
                        if let Some(action) = browser.apply_list(&data, &app.session_root) {
                            self.start(action, app);
                        } else {
                            browser.busy = false;
                        }
                    }
                }
                SessionAction::Search { ref query, generation } => {
                    if let Some(browser) = app.browser.as_mut() {
                        browser.apply_search(&data, query, generation);
                    }
                }
                SessionAction::History { before, .. } => {
                    if let Some(browser) = app.browser.as_mut() {
                        browser.apply_history(&data, before);
                    }
                }
                SessionAction::Delete(row) => {
                    self.drafts.remove(&row.attached_path());
                    if row.attached_path() == app.session_root {
                        app.quit = true;
                    } else {
                        self.start(SessionAction::List, app);
                    }
                }
                SessionAction::Open(_) => {}
                SessionAction::CancelSearch => {}
            },
        }
        true
    }
}

fn executable() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("TEAMAGENTS_SESSION_CLI") {
        return Ok(path.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("teamagents");
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
        for dir in exe.ancestors() {
            for profile in ["debug", "release"] {
                let path = dir.join("engine/target").join(profile).join("teamagents");
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths).filter(|dir| dir.is_absolute()) {
            let path = dir.join("teamagents");
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    Err("Cannot find teamagents. Launch this interface through the teamagents CLI.".into())
}

const MAX_SESSION_OUTPUT: usize = 64 * 1024 * 1024;

fn execute(root: &str, action: &SessionAction, cancel: &AtomicBool) -> Result<Reply, String> {
    if matches!(action, SessionAction::CancelSearch) {
        return Ok(Reply::Data(json!({})));
    }
    let mut command = Command::new(executable()?);
    command.args(["--state-root", root, "sessions"]);
    match action {
        SessionAction::List => {
            command.arg("list");
        }
        SessionAction::Search { query, .. } => {
            command.args(["search", "--query", query, "--limit", "100"]);
        }
        SessionAction::History { id, before } => {
            command.args(["history", "--id", id, "--limit", "200"]);
            if let Some(cursor) = before {
                command.arg("--before").arg(cursor.to_string());
            }
        }
        SessionAction::Open(row) => {
            command.args(["open", "--id", &row.id]);
        }
        SessionAction::Delete(row) => {
            command.args(["delete", "--id", &row.id, "--stop", "--yes"]);
        }
        SessionAction::CancelSearch => unreachable!("cancel actions never spawn a session process"),
    }
    command.arg("--json");
    let output = crate::subprocess::capture(
        &mut command,
        &[],
        MAX_SESSION_OUTPUT,
        Instant::now() + Duration::from_secs(120),
        || cancel.load(Ordering::Relaxed),
    )?;
    let std::process::Output { status, stdout, stderr } = output;
    if !status.success() {
        return Err(String::from_utf8_lossy(&stderr).trim().to_string());
    }
    let data: Json = serde_json::from_slice(&stdout).map_err(|e| format!("Invalid session receipt: {e}"))?;
    match action {
        SessionAction::Open(row) => {
            let socket = data["socket"].as_str().ok_or("Session receipt has no socket")?;
            let mut client = DaemonClient::connect(Path::new(socket))?;
            let root =
                Path::new(&client.state_root).canonicalize().map_err(|e| e.to_string())?.to_string_lossy().into_owned();
            let expected = data["state_root"].as_str().ok_or("Session receipt has no state root")?;
            let expected = Path::new(expected).canonicalize().map_err(|e| e.to_string())?;
            if Path::new(&root) != expected || data["id"] != row.id {
                return Err("Session handshake returned a different state root.".into());
            }
            let (snapshot, watermark) = client.checkpoint()?;
            let mut app = V2App::new(&client.session_id);
            app.session_root = root;
            app.apply_checkpoint(snapshot, watermark);
            app.session_name = if row.id == "default" { "default".into() } else { row.name.clone() };
            app.apply_history(client.call("history", json!({"instance_id": "i-leader", "limit": 400}))?);
            app.apply_approvals(client.call("approvals", json!({}))?);
            Ok(Reply::Opened { client, app: Box::new(app) })
        }
        SessionAction::Delete(row) if data["deleted"] != true || data["id"] != row.id => {
            Err("Deletion was not confirmed by the engine; staying in the TUI.".into())
        }
        SessionAction::CancelSearch => Ok(Reply::Data(data)),
        _ => Ok(Reply::Data(data)),
    }
}
