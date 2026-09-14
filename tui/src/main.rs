//! teamagents-tui: ratatui front-end for TeamAgents (visual parity with the
//! Textual TUI on main). The UI is a pure client: execution lives in the
//! headless engine (`teamagents serve`), authoritative state in the core.

use std::io::Write;
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind, MouseEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use serde_json::{json, Value as Json};

use teamagents_tui::app::{self, App, Effect, OpResult};
use teamagents_tui::worker::Worker;
use teamagents_tui::{i18n, ui};

struct Args {
    cwd: Option<String>,
    resume: Option<String>,
    full_auto: bool,
    team: Option<String>,
    engine_bin: String,
}

fn usage() -> ! {
    eprintln!("teamagents-tui [--cwd DIR] [--resume ID] [--full-auto] [--team SPEC.json]");
    eprintln!("  env: TEAMAGENTS_ENGINE (teamagents binary), --engine PATH");
    std::process::exit(2);
}

/// Locate the engine binary: --engine, TEAMAGENTS_ENGINE, a sibling of this
/// executable, the repo's engine/target/{release,debug}/teamagents, then PATH.
fn find_engine_binary(explicit: Option<String>) -> String {
    if let Some(path) = explicit {
        return path;
    }
    if let Some(path) = std::env::var_os("TEAMAGENTS_ENGINE").filter(|v| !v.is_empty()) {
        return path.to_string_lossy().into_owned();
    }
    let exe = std::env::current_exe().ok();
    if let Some(dir) = exe.as_ref().and_then(|p| p.parent()) {
        let sibling = dir.join("teamagents");
        if sibling.exists() {
            return sibling.to_string_lossy().into_owned();
        }
    }
    let mut roots: Vec<std::path::PathBuf> = vec![];
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    if let Some(exe) = &exe {
        roots.extend(exe.ancestors().map(std::path::Path::to_path_buf));
    }
    for root in roots {
        for candidate in [
            root.join("engine/target/release/teamagents"),
            root.join("engine/target/debug/teamagents"),
            root.join("target/release/teamagents"),
            root.join("target/debug/teamagents"),
        ] {
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    "teamagents".to_string()
}

fn parse_args() -> Args {
    let mut a = Args {
        cwd: None,
        resume: None,
        full_auto: false,
        team: None,
        engine_bin: String::new(),
    };
    let takes_value = |a: &mut Args, i: usize, argv: &[String]| -> usize {
        // value flags consume the next argument
        let _ = a;
        if i + 1 < argv.len() { 2 } else { 1 }
    };
    let mut engine_flag: Option<String> = None;
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let step = match argv[i].as_str() {
            "--cwd" => { a.cwd = argv.get(i + 1).cloned(); takes_value(&mut a, i, &argv) }
            "--resume" => { a.resume = argv.get(i + 1).cloned(); takes_value(&mut a, i, &argv) }
            "--full-auto" => { a.full_auto = true; 1 }
            "--team" => { a.team = argv.get(i + 1).cloned(); takes_value(&mut a, i, &argv) }
            "--engine" => { engine_flag = argv.get(i + 1).cloned(); takes_value(&mut a, i, &argv) }
            _ => usage(),
        };
        i += step;
    }
    a.engine_bin = find_engine_binary(engine_flag);
    a
}

fn main() {
    let args = parse_args();
    if !atty_stdout() {
        eprintln!("TUI 需要真实终端；哑终端请用 --plain (TS CLI)");
        std::process::exit(1);
    }
    let worker = match Worker::spawn(&args.engine_bin) {
        Ok(w) => Arc::new(w),
        Err(e) => {
            eprintln!("无法启动引擎 ({} serve): {e}", args.engine_bin);
            std::process::exit(1);
        }
    };
    let opened = worker.call(
        "open",
        json!({
            "cwd": args.cwd,
            "resume": args.resume,
            "fullAuto": args.full_auto,
            "team": args.team,
        }),
    );
    let opened = match opened {
        Ok(v) => v,
        Err(e) => {
            eprintln!("无法打开会话: {e}");
            std::process::exit(1);
        }
    };

    let prefs = i18n::read_preferences();
    let history = i18n::read_history();
    let mut app = App::new(
        opened.get("session_id").and_then(|v| v.as_str()).unwrap_or(""),
        opened.get("catalog").cloned().unwrap_or(Json::Null),
        opened.get("user_config_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        prefs.language,
        true, // the activity spinner is always animated (no settings switch any more)
        history,
    );

    // a panic while the terminal is raw must still hand it back (raw mode +
    // alternate screen + mouse/paste reporting)
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    // terminal setup
    let mut stdout = std::io::stdout();
    enable_raw_mode().expect("raw mode");
    stdout.execute(EnterAlternateScreen).expect("alt screen");
    if supports_keyboard_enhancement().unwrap_or(false) {
        let _ = stdout.execute(crossterm::event::PushKeyboardEnhancementFlags(
            crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | crossterm::event::KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
        ));
    }
    execute!(
        stdout,
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste
    )
    .ok();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).expect("terminal");

    let code = run(&mut terminal, &worker, &mut app);

    // teardown
    restore_terminal();
    drop(terminal);
    Arc::try_unwrap(worker).map(|w| w.close()).ok();
    std::process::exit(code);
}

/// Undo everything the TUI turns on; safe to call twice (teardown + panic hook).
fn restore_terminal() {
    let mut out = std::io::stdout();
    let _ = execute!(
        out,
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste,
        LeaveAlternateScreen
    );
    if supports_keyboard_enhancement().unwrap_or(false) {
        let _ = out.execute(crossterm::event::PopKeyboardEnhancementFlags);
    }
    let _ = disable_raw_mode();
}

fn atty_stdout() -> bool {
    unsafe { libc_isatty(1) == 1 }
}

#[link(name = "c")]
extern "C" {
    #[link_name = "isatty"]
    fn libc_isatty(fd: i32) -> i32;
}

enum BgMsg {
    Op(OpResult),
    SlowTick { shared: Vec<Json>, sessions: Vec<Json> },
}

fn run(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, worker: &Arc<Worker>, app: &mut App) -> i32 {
    let (bg_tx, bg_rx) = channel::<BgMsg>();
    app.push_startup_warnings();

    // initial full state
    if let Ok(st) = worker.core("state", json!({"after_sequence": 0})) {
        for e in app.apply_state(&st) {
            run_effect(e, worker, app, &bg_tx);
        }
    }

    let mut last_state_poll = Instant::now() - Duration::from_secs(1);
    let mut last_activity = Instant::now();
    let mut last_flush = Instant::now();
    let mut last_slow = Instant::now() - Duration::from_secs(1);
    let mut last_log_sig: Option<(bool, Option<String>)> = None;
    let mut poll_failures = 0u32;
    let mut dirty = true;

    loop {
        // input events
        if event::poll(Duration::from_millis(40)).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                    let effects = app.handle_key(key);
                    for e in effects {
                        run_effect(e, worker, app, &bg_tx);
                    }
                    dirty = true;
                }
                Ok(Event::Mouse(m)) => {
                    handle_mouse(m, terminal, app);
                    dirty = true;
                }
                Ok(Event::Resize(_, _)) => dirty = true,
                Ok(Event::Paste(text)) => {
                    app.handle_paste(&text);
                    dirty = true;
                }
                _ => {}
            }
        }
        // stream deltas
        while let Some(push) = worker.try_push() {
            if push.kind == "delta" {
                app.on_delta(&push.run_id, &push.agent_id, &push.text);
                dirty = true;
            }
        }
        // background op results
        while let Ok(msg) = bg_rx.try_recv() {
            match msg {
                BgMsg::Op(op) => {
                    for e in app.on_op_result(op) {
                        run_effect(e, worker, app, &bg_tx);
                    }
                }
                BgMsg::SlowTick { shared, sessions } => {
                    app.shared = shared;
                    app.sessions = sessions;
                }
            }
            dirty = true;
        }
        // state poll 250ms
        if last_state_poll.elapsed() >= Duration::from_millis(250) {
            last_state_poll = Instant::now();
            // log_cursor only leads while the log panel is live (activation does a
            // full replay); elsewhere it stays 0 and would force a full fetch
            let after = if matches!(app::PANELS[app.panel], "log") {
                app.cursor.min(app.log_cursor)
            } else {
                app.cursor
            };
            match worker.core("state", json!({"after_sequence": after})) {
                Ok(st) => {
                    poll_failures = 0;
                    app.disconnected = false;
                    let events = st.get("events").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                    let effects = app.apply_state(&st);
                    for e in effects {
                        run_effect(e, worker, app, &bg_tx);
                    }
                    if matches!(app::PANELS[app.panel], "log") {
                        app.append_log(&events);
                    }
                    dirty = true;
                }
                Err(err) => {
                    poll_failures += 1;
                    if poll_failures == 3 {
                        // engine unreachable: chip in the status bar + one chat line
                        app.disconnected = true;
                        let msg = app.t("[界面刷新失败] ", &[]) + &err;
                        app.chat.push(("system".into(), msg));
                    }
                    dirty = true;
                }
            }
        }
        // log tab (re)play on activation / filter change
        let log_sig = (app::PANELS[app.panel] == "log", app.log_member.clone());
        if log_sig.0 && last_log_sig.as_ref() != Some(&log_sig) {
            match worker.core("state", json!({"after_sequence": 0})) {
                Ok(st) => {
                    let events = st.get("events").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                    app.replay_log(&events);
                }
                Err(err) => {
                    let msg = app.t("[界面读取事件失败] {v0}", &[("v0", &err)]);
                    app.chat.push(("system".into(), msg));
                }
            }
            dirty = true;
        }
        last_log_sig = Some(log_sig);
        // slow tick: shared entries + session list
        if last_slow.elapsed() >= Duration::from_secs(1) {
            last_slow = Instant::now();
            spawn_slow_tick(worker.clone(), bg_tx.clone(), app);
        }
        // activity spinner 120ms
        if last_activity.elapsed() >= Duration::from_millis(120) {
            last_activity = Instant::now();
            dirty = true; // activity_lines decides visually; cheap enough
        }
        if last_flush.elapsed() >= Duration::from_millis(80) {
            last_flush = Instant::now();
            app.flush_deltas();
        }
        if app.should_quit {
            return 0;
        }
        if dirty {
            let _ = terminal.draw(|f| ui::render(f, app));
            dirty = false;
        }
    }
}

fn spawn_slow_tick(worker: Arc<Worker>, tx: std::sync::mpsc::Sender<BgMsg>, app: &App) {
    let space_ids: Vec<String> = app
        .spec()
        .get("shared_spaces")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|s| s.get("id").and_then(|v| v.as_str()).map(str::to_string)).collect())
        .unwrap_or_default();
    std::thread::spawn(move || {
        let shared = worker
            .core("shared_entries", json!({"space_ids": space_ids, "limit": 1000}))
            .ok()
            .and_then(|r| r.get("entries").and_then(|v| v.as_array()).cloned())
            .unwrap_or_default();
        let sessions = worker
            .call("list_sessions", json!({}))
            .ok()
            .and_then(|r| r.get("sessions").and_then(|v| v.as_array()).cloned())
            .unwrap_or_default();
        let _ = tx.send(BgMsg::SlowTick { shared, sessions });
    });
}

fn run_effect(e: Effect, worker: &Arc<Worker>, app: &mut App, bg: &std::sync::mpsc::Sender<BgMsg>) {
    match e {
        Effect::Quit => app.should_quit = true,
        Effect::Bell => {
            let _ = std::io::stdout().write_all(b"\x07");
            let _ = std::io::stdout().flush();
        }
        // ponytail: synchronous submit on the UI thread, worst case frozen for the
        // 120s worker call timeout; upgrade path: run submit on a background thread
        Effect::Submit { action, ok_msg, err_msg } => match worker.call("submit", json!({"action": action})) {
            Ok(receipt) if receipt.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) => {
                if let Some(msg) = ok_msg {
                    app.chat.push(("system".into(), msg));
                }
            }
            r => {
                let err = r.ok().and_then(|x| x.get("error").and_then(|v| v.as_str()).map(str::to_string))
                    .unwrap_or_else(|| "submit failed".into());
                if let Some(msg) = err_msg {
                    app.chat.push(("system".into(), msg.replace("{error}", &err)));
                }
            }
        },
        Effect::CancelTask(task_id) => {
            let receipt = worker.call(
                "submit",
                json!({"action": {
                    "action_id": format!("ui-cancel-task-{task_id}"),
                    "actor_id": "user",
                    "kind": "cancel_task",
                    "payload": {"task_id": task_id},
                }}),
            );
            let msg = app::cancel_task_feedback(app.lang, &task_id, &receipt.unwrap_or(Json::Null));
            app.notify(msg.clone(), app::Severity::Info, 10);
            app.chat.push(("system".into(), msg));
        }
        Effect::DecideApproval { approval_id, decision } => {
            let receipt = worker.call(
                "submit",
                json!({"action": {
                    "action_id": format!("ui-approval-{approval_id}-{decision}"),
                    "actor_id": "user",
                    "kind": "approval_decision",
                    "payload": {"approval_id": approval_id, "decision": decision},
                }}),
            );
            let ok = receipt.map(|r| r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)).unwrap_or(false);
            let msg = app::decide_feedback(app.lang, &decision, ok);
            app.notify(msg, if ok { app::Severity::Info } else { app::Severity::Error }, 10);
        }
        Effect::UserMessage(text) => match worker.call("user_message", json!({"text": text})) {
            Ok(receipt) if receipt.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) => {
                app.composer.record_submission(&text);
                let _ = i18n::write_history(&app.composer.history);
            }
            r => {
                let err = r.ok().and_then(|x| x.get("error").and_then(|v| v.as_str()).map(str::to_string))
                    .unwrap_or_else(|| "rejected".into());
                app.composer.set_text(&text);
                let msg = app::rejected_feedback(app.lang, &err);
                app.chat.push(("system".into(), msg));
            }
        },
        Effect::SwitchSession(target) => {
            let (worker, tx) = (worker.clone(), bg.clone());
            let orig = target.clone();
            std::thread::spawn(move || {
                let op = match worker.call("switch_session", json!({"session_id": target})) {
                    Ok(v) => OpResult::Switched {
                        session_id: v.get("session_id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        catalog: v.get("catalog").cloned().unwrap_or(Json::Null),
                        config_path: v.get("user_config_path").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    },
                    Err(e) => OpResult::Failed { op: "switch", target: orig, error: e },
                };
                let _ = tx.send(BgMsg::Op(op));
            });
        }
        Effect::NewSession => {
            let (worker, tx) = (worker.clone(), bg.clone());
            std::thread::spawn(move || {
                let op = match worker.call("new_session", json!({})) {
                    Ok(v) => OpResult::Switched {
                        session_id: v.get("session_id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        catalog: v.get("catalog").cloned().unwrap_or(Json::Null),
                        config_path: v.get("user_config_path").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    },
                    Err(e) => OpResult::Failed { op: "switch", target: String::new(), error: e },
                };
                let _ = tx.send(BgMsg::Op(op));
            });
        }
        Effect::ArchiveSession(target) => {
            let (worker, tx) = (worker.clone(), bg.clone());
            let orig = target.clone();
            std::thread::spawn(move || {
                let op = match worker.call("archive_session", json!({"session_id": target.clone()})) {
                    Ok(v) => OpResult::Archived {
                        session_id: target,
                        target: v.get("target").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        was_current: v.get("was_current").and_then(|x| x.as_bool()).unwrap_or(false),
                    },
                    Err(e) => OpResult::Failed { op: "archive", target: orig, error: e },
                };
                let _ = tx.send(BgMsg::Op(op));
            });
        }
        Effect::DeleteSession(target) => {
            let (worker, tx) = (worker.clone(), bg.clone());
            let orig = target.clone();
            std::thread::spawn(move || {
                let op = match worker.call("delete_session", json!({"session_id": target.clone()})) {
                    Ok(v) => OpResult::Deleted {
                        session_id: target,
                        was_current: v.get("was_current").and_then(|x| x.as_bool()).unwrap_or(false),
                    },
                    Err(e) => OpResult::Failed { op: "delete", target: orig, error: e },
                };
                let _ = tx.send(BgMsg::Op(op));
            });
        }
    }
}

fn handle_mouse(m: event::MouseEvent, terminal: &Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App) {
    let size = terminal.size().unwrap_or(ratatui::layout::Size { width: 80, height: 24 });
    let area = ratatui::layout::Rect { x: 0, y: 0, width: size.width, height: size.height };
    // every mouse event updates the hover position (grey surface + white text marks
    // what a click would hit)
    app.pointer = Some((m.row, m.column));
    if app.settings_open {
        return; // the settings overlay is modal
    }
    let geo = ui::geometry(app, area);
    let in_rect = |r: ratatui::layout::Rect, row: u16, col: u16| -> bool {
        col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
    };
    let on_side = in_rect(geo.side, m.row, m.column);
    match m.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = m.kind == MouseEventKind::ScrollUp;
            if on_side && app::PANELS[app.panel] != "log" {
                // the wheel drives the table selection, exactly like ↑/↓ — no
                // matter which pane has the focus
                app.move_table_selection(if up { -1 } else { 1 });
            } else {
                // the wheel scrolls the pane under the pointer, not the focused
                // one (D-20 #7): over the box it is the log, elsewhere the chat
                let target = ui::wheel_target(&geo, m.row, m.column);
                if up {
                    app.scroll_up_target(target, 3);
                } else {
                    app.scroll_down_target(target, 3);
                }
            }
            return;
        }
        MouseEventKind::Down(event::MouseButton::Left) => {}
        _ => return,
    }

    if on_side {
        if m.row == geo.tabs_y {
            // same layout the renderer used: clicking a tab hits that tab
            let inner = geo.side_inner;
            if let Some(index) = ui::tab_at(app, inner.x, inner.width, m.column) {
                app.panel = index;
                app.focus = app::Focus::Panel;
            }
            return;
        }
        if m.row >= geo.rows.y {
            app.focus = app::Focus::Panel;
            // the table body rect is the single source: renderer and hit-test
            // read it from `ui::geometry` (hint rows and borders included)
            if in_rect(geo.rows, m.row, m.column) {
                app.select_row_visible((m.row - geo.rows.y) as usize, geo.rows.height as usize);
            }
        }
        return;
    }
    if in_rect(geo.chat, m.row, m.column) {
        app.focus = app::Focus::Composer;
    }
}
