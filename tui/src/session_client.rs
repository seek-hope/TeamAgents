//! D-446: asynchronous client of the engine's session CLI. No database or registry access in the TUI.
use crate::{daemon_client::DaemonClient, sessions::SessionAction, text::Composer, v2app::V2App};
use serde_json::{json, Value as Json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
};

enum Reply {
    Data(Json),
    Opened { client: DaemonClient, app: Box<V2App> },
}

pub struct SessionClient {
    root: String,
    pending: Option<mpsc::Receiver<(SessionAction, Result<Reply, String>)>>,
    drafts: BTreeMap<String, Composer>,
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
        self.pending = Some(rx);
        if let Some(browser) = app.browser.as_mut() {
            browser.busy = true;
        }
        let root = self.root.clone();
        std::thread::spawn(move || {
            let reply = execute(&root, &action);
            let _ = tx.send((action, reply));
        });
    }

    /// Switch atomically after handshake, checkpoint, history and approvals all succeeded.
    /// Drafts use state-root identity: every daemon may legitimately report the same session_id.
    pub fn poll(&mut self, client: &mut DaemonClient, app: &mut V2App) -> bool {
        let Some(rx) = &self.pending else {
            return false;
        };
        let (action, reply) = match rx.try_recv() {
            Ok(reply) => reply,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                if let Some(browser) = app.browser.as_mut() {
                    browser.fail("Session operation ended without a receipt.".into());
                }
                return true;
            }
        };
        self.pending = None;
        match reply {
            Err(error) => {
                if let Some(browser) = app.browser.as_mut() {
                    browser.fail(error);
                }
            }
            Ok(Reply::Opened { client: next_client, app: mut next_app }) => {
                let draft = std::mem::replace(&mut app.composer, Composer::new(vec![]));
                self.drafts.insert(app.session_root.clone(), draft);
                if let Some(draft) = self.drafts.remove(&next_app.session_root) {
                    next_app.composer = draft;
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

fn execute(root: &str, action: &SessionAction) -> Result<Reply, String> {
    let mut command = Command::new(executable()?);
    command.args(["--state-root", root, "sessions"]);
    match action {
        SessionAction::List => {
            command.arg("list");
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
    }
    let output = command.arg("--json").stdin(Stdio::null()).output().map_err(|e| format!("Session command: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let data: Json = serde_json::from_slice(&output.stdout).map_err(|e| format!("Invalid session receipt: {e}"))?;
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
        _ => Ok(Reply::Data(data)),
    }
}
