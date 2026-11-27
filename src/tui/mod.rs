//! Interactive Ratatui terminal interface backed by a non-blocking daemon worker.
mod input;
mod state;
mod theme;
mod ui;

use crate::{
    protocol::{preview, Client},
    Result,
};
use crossterm::{
    cursor::MoveTo,
    event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event},
    execute,
    terminal::{self, disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use state::{AppState, BackendEvent, HitMap, UiCommand};
use std::{
    io::{self, Write},
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const ACTIVE_POLL: Duration = Duration::from_millis(250);
const IDLE_POLL: Duration = Duration::from_millis(750);
const ANIM_TICK: Duration = Duration::from_millis(80);
const IDLE_TICK: Duration = Duration::from_millis(200);

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().map_err(|e| e.to_string())?;
        let mut out = io::stdout();
        // Clear directly rather than calling ratatui's Terminal::clear(), which queries DSR (\x1b[6n).
        if let Err(e) = execute!(
            out,
            EnterAlternateScreen,
            Clear(ClearType::All),
            MoveTo(0, 0),
            EnableBracketedPaste,
            EnableMouseCapture
        ) {
            let _ = disable_raw_mode();
            return Err(e.to_string());
        }
        install_panic_hook();
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let mut out = io::stdout();
    let _ = execute!(out, DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen);
    let _ = out.flush();
    let _ = disable_raw_mode();
}

fn install_panic_hook() {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            prev(info);
        }));
    });
}

fn fetch_snapshot(client: &Client) -> (BackendEvent, bool) {
    let tasks = client.tasks();
    let active = tasks.as_ref().is_ok_and(|rows| rows.iter().any(|t| !t.status.is_terminal()));
    let members = client.members();
    let model = client.model_get();
    (BackendEvent::Snapshot { tasks, members, model }, active)
}

fn handle_command(client: &Client, cmd: UiCommand, tx: &Sender<BackendEvent>) -> bool {
    match cmd {
        UiCommand::Quit => return false,
        UiCommand::RefreshAll => {
            let (snap, _) = fetch_snapshot(client);
            let _ = tx.send(snap);
        }
        UiCommand::FetchDetail(id) => {
            let detail = client.status(&id).map(Box::new);
            let _ = tx.send(BackendEvent::TaskDetail(detail));
        }
        UiCommand::LoadPlanModels => {
            let _ = tx.send(BackendEvent::PlanModels(crate::model::plan_models()));
        }
        UiCommand::Submit { agent, text, review, staged, check } => {
            let outcome = match client.submit_with_options(&agent, &text, review, check.as_deref(), staged) {
                Ok(receipt) => {
                    let id = receipt["id"].as_str().map(str::to_owned);
                    let msg = format!("Submitted {} to @{agent}", id.as_deref().unwrap_or("task"));
                    BackendEvent::CommandOutcome { message: Some(msg), error: None, select_task: id, refresh: true }
                }
                Err(e) => {
                    BackendEvent::CommandOutcome { message: None, error: Some(e), select_task: None, refresh: false }
                }
            };
            let _ = tx.send(outcome);
        }
        UiCommand::Continue { id, text, review, staged, check } => {
            let outcome = match client.continue_with_options(&id, &text, review, check.as_deref(), staged) {
                Ok(receipt) => {
                    let root = receipt["id"].as_str().map(str::to_owned);
                    let node = receipt["node"].as_str().unwrap_or(&id);
                    let msg = format!("Continued {} via node {}", root.as_deref().unwrap_or(&id), preview(node, 14));
                    BackendEvent::CommandOutcome { message: Some(msg), error: None, select_task: root, refresh: true }
                }
                Err(e) => {
                    BackendEvent::CommandOutcome { message: None, error: Some(e), select_task: None, refresh: true }
                }
            };
            let _ = tx.send(outcome);
        }
        UiCommand::WbWrite { root_id, key, mode, kind, content } => {
            let outcome = match client.wb_write(&root_id, &key, &mode, kind.as_deref(), content.as_deref()) {
                Ok(_) => BackendEvent::CommandOutcome {
                    message: Some(format!("Whiteboard {mode}: {key}")),
                    error: None,
                    select_task: Some(root_id),
                    refresh: true,
                },
                Err(e) => {
                    BackendEvent::CommandOutcome { message: None, error: Some(e), select_task: None, refresh: true }
                }
            };
            let _ = tx.send(outcome);
        }
        UiCommand::Answer { root_id, text } => {
            let outcome = match client.answer(&root_id, &text) {
                Ok(_) => BackendEvent::CommandOutcome {
                    message: Some(format!("Answered clarification on {}", preview(&root_id, 18))),
                    error: None,
                    select_task: Some(root_id),
                    refresh: true,
                },
                Err(e) => {
                    BackendEvent::CommandOutcome { message: None, error: Some(e), select_task: None, refresh: true }
                }
            };
            let _ = tx.send(outcome);
        }
        UiCommand::Approve { approval_id, allow } => {
            let verb = if allow { "Approved" } else { "Denied" };
            let outcome = match client.approve(&approval_id, allow) {
                Ok(_) => BackendEvent::CommandOutcome {
                    message: Some(format!("{verb} {approval_id}")),
                    error: None,
                    select_task: None,
                    refresh: true,
                },
                Err(e) => {
                    BackendEvent::CommandOutcome { message: None, error: Some(e), select_task: None, refresh: true }
                }
            };
            let _ = tx.send(outcome);
        }
        UiCommand::Cancel(id) => {
            let outcome = match client.cancel(&id) {
                Ok(_) => BackendEvent::CommandOutcome {
                    message: Some(format!("Cancelled {id}")),
                    error: None,
                    select_task: None,
                    refresh: true,
                },
                Err(e) => {
                    BackendEvent::CommandOutcome { message: None, error: Some(e), select_task: None, refresh: true }
                }
            };
            let _ = tx.send(outcome);
        }
        UiCommand::Delete(id) => {
            let outcome = match client.delete(&id) {
                Ok(val) => {
                    let root = val["root"].as_str().unwrap_or(&id);
                    BackendEvent::CommandOutcome {
                        message: Some(format!("Deleted session {}", preview(root, 18))),
                        error: None,
                        select_task: None,
                        refresh: true,
                    }
                }
                Err(e) => {
                    BackendEvent::CommandOutcome { message: None, error: Some(e), select_task: None, refresh: true }
                }
            };
            let _ = tx.send(outcome);
        }
        UiCommand::SetModel { model, effort } => {
            let m = model.as_ref().map(|opt| opt.as_deref());
            let e = effort.as_ref().map(|opt| opt.as_deref());
            let outcome = match client.model_set(m, e) {
                Ok(cfg) => {
                    let model_name = cfg.model.as_deref().unwrap_or("default");
                    let eff_name = cfg.effort.as_deref().unwrap_or("default");
                    BackendEvent::CommandOutcome {
                        message: Some(format!("Model: {}/{model_name} (effort: {eff_name})", cfg.provider)),
                        error: None,
                        select_task: None,
                        refresh: true,
                    }
                }
                Err(err) => {
                    BackendEvent::CommandOutcome { message: None, error: Some(err), select_task: None, refresh: false }
                }
            };
            let _ = tx.send(outcome);
        }
        UiCommand::StopDaemon => {
            let outcome = match client.stop() {
                Ok(_) => BackendEvent::CommandOutcome {
                    message: Some("Daemon stopped (session preserved).".into()),
                    error: None,
                    select_task: None,
                    refresh: true,
                },
                Err(e) => {
                    BackendEvent::CommandOutcome { message: None, error: Some(e), select_task: None, refresh: false }
                }
            };
            let _ = tx.send(outcome);
        }
    }
    true
}

fn spawn_backend(client: Client, cmd_rx: Receiver<UiCommand>, evt_tx: Sender<BackendEvent>) -> JoinHandle<()> {
    thread::spawn(move || {
        let (snap, mut active) = fetch_snapshot(&client);
        if evt_tx.send(snap).is_err() {
            return;
        }
        loop {
            let wait = if active { ACTIVE_POLL } else { IDLE_POLL };
            match cmd_rx.recv_timeout(wait) {
                Ok(first) => {
                    if !handle_command(&client, first, &evt_tx) {
                        break;
                    }
                    while let Ok(next) = cmd_rx.try_recv() {
                        if !handle_command(&client, next, &evt_tx) {
                            return;
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let (snap, next_active) = fetch_snapshot(&client);
                    active = next_active;
                    if evt_tx.send(snap).is_err() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    })
}

pub(crate) fn run(client: Client, agent: String, review: bool, staged: bool) -> Result<()> {
    let workspace_name = client
        .workspace
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("workspace")
        .to_owned();

    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout())).map_err(|e| e.to_string())?;

    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (evt_tx, evt_rx) = mpsc::channel();
    let worker = spawn_backend(client, cmd_rx, evt_tx);

    let mut state = AppState::new(workspace_name, agent, review, staged);
    let mut hits = HitMap::default();
    let mut dirty = true;
    let mut last_anim = Instant::now();

    let result = (|| -> Result<()> {
        loop {
            while let Ok(evt) = evt_rx.try_recv() {
                for cmd in state.apply_backend_event(evt) {
                    let _ = cmd_tx.send(cmd);
                }
                dirty = true;
            }
            if state.should_quit {
                break;
            }
            if last_anim.elapsed() >= ANIM_TICK {
                if state.advance_tick() {
                    dirty = true;
                }
                last_anim = Instant::now();
            }
            if dirty {
                terminal
                    .draw(|frame| {
                        hits = ui::render(frame, &mut state);
                    })
                    .map_err(|e| e.to_string())?;
                dirty = false;
            }

            let timeout = if state.has_animations() { ANIM_TICK } else { IDLE_TICK };
            if event::poll(timeout).map_err(|e| e.to_string())? {
                loop {
                    match event::read().map_err(|e| e.to_string())? {
                        Event::Key(key) => {
                            for cmd in state.handle_key(key) {
                                let _ = cmd_tx.send(cmd);
                            }
                            dirty = true;
                        }
                        Event::Paste(text) => {
                            state.handle_paste(&text);
                            dirty = true;
                        }
                        Event::Mouse(mouse) => {
                            if let Ok((w, h)) = terminal::size() {
                                if (w, h) != (hits.viewport.width, hits.viewport.height) {
                                    terminal
                                        .draw(|frame| {
                                            hits = ui::render(frame, &mut state);
                                        })
                                        .map_err(|e| e.to_string())?;
                                }
                            }
                            for cmd in state.handle_mouse(mouse, &hits) {
                                let _ = cmd_tx.send(cmd);
                            }
                            dirty = true;
                        }
                        Event::Resize(_, _) => {
                            dirty = true;
                            break;
                        }
                        Event::FocusGained | Event::FocusLost => {}
                    }
                    if state.should_quit || !event::poll(Duration::ZERO).map_err(|e| e.to_string())? {
                        break;
                    }
                }
            }
        }
        Ok(())
    })();

    let _ = cmd_tx.send(UiCommand::Quit);
    let _ = worker.join();
    let _ = terminal.show_cursor();
    result
}
