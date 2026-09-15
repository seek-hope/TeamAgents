//! teamagents-tui: ratatui front-end for TeamAgents. The UI is a pure
//! client: execution lives in the headless engine (`teamagents serve`),
//! authoritative state in the core.

use std::io::Write;
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
use teamagents_tui::worker::{PendingCall, Worker};
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
        eprintln!("TUI 需要真实终端；哑终端请用 teamagents --plain");
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
    match Arc::try_unwrap(worker) {
        Ok(w) => w.close(),
        // a slow tick still holds an Arc: kill the engine outright so it
        // cannot orphan holding the session flock
        Err(w) => w.kill(),
    }
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

enum RequestKind {
    Effect(Effect, u64),
    State,
    Log(Option<String>),
    Shared,
    Sessions,
}

struct UiRequest {
    session: String,
    generation: u64,
    kind: RequestKind,
    call: PendingCall,
}

#[derive(Default)]
struct AsyncUi {
    pending: Vec<UiRequest>,
    generation: u64,
    switching: bool,
    poll_failures: u32,
}

impl AsyncUi {
    fn enqueue(&mut self, kind: RequestKind, worker: &Worker, app: &App, method: &str, params: Json, timeout: Duration) {
        self.pending.push(UiRequest {
            session: app.session_id.clone(), generation: self.generation, kind,
            call: worker.start_call(method, params, timeout),
        });
    }

    fn has(&self, check: impl Fn(&RequestKind) -> bool) -> bool {
        self.pending.iter().any(|r| r.generation == self.generation && check(&r.kind))
    }

    fn drain(&mut self, worker: &Worker, app: &mut App) -> bool {
        let mut dirty = false;
        let mut index = 0;
        while index < self.pending.len() {
            let Some(result) = self.pending[index].call.try_result() else { index += 1; continue; };
            let request = self.pending.remove(index);
            if request.session != app.session_id || request.generation != self.generation { continue; }
            dirty = true;
            match request.kind {
                RequestKind::Effect(effect, model_generation) => {
                    let transition = session_operation(&effect);
                    // A timed-out switch may still finish in the engine. Keep
                    // controls locked until restart rather than target an unknown session.
                    let uncertain = transition && result.as_ref().err().is_some_and(|e| e.contains("timed out"));
                    let switched = transition && result.is_ok();
                    let effects = apply_effect_result(effect, model_generation, result, app);
                    if transition {
                        self.switching = uncertain;
                        if uncertain {
                            app.disconnected = true;
                            app.chat.push(("system".into(), "会话操作超时，当前会话未确认；请退出后重新打开。".into()));
                        }
                        if switched {
                            self.generation += 1;
                            // Pushes carry run IDs, not session IDs. Discard the
                            // old session's buffered deltas at the switch boundary.
                            while worker.try_push().is_some() {}
                        }
                    }
                    for effect in effects { run_effect(effect, worker, app, self); }
                }
                RequestKind::State => match result {
                    Ok(st) => {
                        self.poll_failures = 0;
                        app.disconnected = false;
                        let effects = app.apply_state(&st);
                        if app::PANELS[app.panel] == "log" {
                            app.append_log(st.get("events").and_then(Json::as_array).map(Vec::as_slice).unwrap_or(&[]));
                        }
                        for effect in effects { run_effect(effect, worker, app, self); }
                    }
                    Err(error) => {
                        self.poll_failures += 1;
                        if self.poll_failures == 3 {
                            app.disconnected = true;
                            let msg = app.t("[界面刷新失败] ", &[]) + &error;
                            app.chat.push(("system".into(), msg));
                        }
                    }
                },
                RequestKind::Log(member) if app::PANELS[app.panel] == "log" && member == app.log_member => match result {
                    Ok(st) => app.replay_log(st.get("events").and_then(Json::as_array).map(Vec::as_slice).unwrap_or(&[])),
                    Err(error) => {
                        let msg = app.t("[界面读取事件失败] {v0}", &[("v0", &error)]);
                        app.chat.push(("system".into(), msg));
                    }
                },
                RequestKind::Shared => {
                    if let Ok(value) = result {
                        if let Some(entries) = value.get("entries").and_then(Json::as_array) { app.shared = entries.clone(); }
                    }
                }
                RequestKind::Sessions => {
                    if let Ok(value) = result {
                        if let Some(sessions) = value.get("sessions").and_then(Json::as_array) { app.sessions = sessions.clone(); }
                    }
                }
                RequestKind::Log(_) => {}
            }
        }
        dirty
    }
}

fn run(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, worker: &Arc<Worker>, app: &mut App) -> i32 {
    let mut requests = AsyncUi::default();
    app.push_startup_warnings();
    let mut last_state_poll = Instant::now() - Duration::from_secs(1);
    let mut last_activity = Instant::now();
    let mut last_flush = Instant::now();
    let mut last_slow = Instant::now() - Duration::from_secs(1);
    let mut last_log_sig = None;
    let mut dirty = true;

    loop {
        // Apply replies before accepting input, so a completed switch changes
        // the session before the next action is enqueued.
        dirty |= requests.drain(worker, app);
        if event::poll(Duration::from_millis(40)).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                    for effect in app.handle_key(key) { run_effect(effect, worker, app, &mut requests); }
                    dirty = true;
                }
                Ok(Event::Mouse(m)) => { handle_mouse(m, terminal, app); dirty = true; }
                Ok(Event::Resize(_, _)) => dirty = true,
                Ok(Event::Paste(text)) => { app.handle_paste(&text); dirty = true; }
                _ => {}
            }
        }
        while let Some(push) = worker.try_push() {
            if push.kind == "plan" && !requests.switching {
                app.on_plan(&push.agent_id, &push.items);
            }
            if push.kind == "tool" && !requests.switching {
                app.on_tool_result(&push.agent_id, &push.tool, push.ok, &push.arguments, &push.result);
            }
            if push.kind == "delta" && !requests.switching {
                app.on_delta(&push.run_id, &push.agent_id, &push.text);
                dirty = true;
            }
        }
        if !requests.switching {
            if last_state_poll.elapsed() >= Duration::from_millis(250) && !requests.has(|k| matches!(k, RequestKind::State)) {
                last_state_poll = Instant::now();
                let after = if app::PANELS[app.panel] == "log" { app.cursor.min(app.log_cursor) } else { app.cursor };
                requests.enqueue(RequestKind::State, worker, app, "call", json!({"method":"state", "params":{"after_sequence":after}}), Duration::from_secs(5));
            }
            let log_sig = (requests.generation, app::PANELS[app.panel] == "log", app.log_member.clone());
            if log_sig.1 && last_log_sig.as_ref() != Some(&log_sig) {
                requests.enqueue(RequestKind::Log(app.log_member.clone()), worker, app, "call", json!({"method":"state", "params":{"after_sequence":0}}), Duration::from_secs(30));
            }
            last_log_sig = Some(log_sig);
            if last_slow.elapsed() >= Duration::from_secs(1) {
                last_slow = Instant::now();
                if !requests.has(|k| matches!(k, RequestKind::Shared)) {
                    let space_ids: Vec<&str> = app.spec().get("shared_spaces").and_then(Json::as_array)
                        .map(|spaces| spaces.iter().filter_map(|s| s.get("id").and_then(Json::as_str)).collect()).unwrap_or_default();
                    requests.enqueue(RequestKind::Shared, worker, app, "call", json!({"method":"shared_entries", "params":{"space_ids":space_ids,"limit":1000}}), Duration::from_secs(5));
                }
                if !requests.has(|k| matches!(k, RequestKind::Sessions)) {
                    requests.enqueue(RequestKind::Sessions, worker, app, "list_sessions", json!({}), Duration::from_secs(5));
                }
            }
        }
        if last_activity.elapsed() >= Duration::from_millis(120) { last_activity = Instant::now(); dirty = true; }
        if last_flush.elapsed() >= Duration::from_millis(80) { last_flush = Instant::now(); app.flush_deltas(); }
        if app.should_quit { return 0; }
        if dirty { let _ = terminal.draw(|f| ui::render(f, app)); dirty = false; }
    }
}

fn session_operation(effect: &Effect) -> bool {
    matches!(effect, Effect::SwitchSession(_) | Effect::NewSession | Effect::Fork | Effect::ArchiveSession(_) | Effect::DeleteSession(_))
}

fn run_effect(effect: Effect, worker: &Worker, app: &mut App, requests: &mut AsyncUi) {
    match effect {
        Effect::Quit => { app.should_quit = true; return; }
        Effect::Bell => { let _ = std::io::stdout().write_all(b"\x07"); let _ = std::io::stdout().flush(); return; }
        _ => {}
    }
    if requests.switching {
        if let Effect::UserMessage(text) = &effect {
            if app.composer.text().is_empty() { app.composer.set_text(text); }
        }
        app.notify("会话操作进行中，请稍后重试。".into(), app::Severity::Warning, 5);
        return;
    }
    let (method, params) = match &effect {
        Effect::Submit { action, .. } => ("submit", json!({"action": action})),
        Effect::CancelTask(task_id) => ("submit", json!({"action": {"action_id":format!("ui-cancel-task-{task_id}"),"actor_id":"user","kind":"cancel_task","payload":{"task_id":task_id}}})),
        Effect::AcknowledgeRun(run_id) => ("submit", json!({"action": {"action_id":format!("ui-ack-run-{run_id}"),"actor_id":"user","kind":"cancel_run","payload":{"run_id":run_id}}})),
        Effect::DecideApproval { approval_id, decision } => ("submit", json!({"action": {"action_id":format!("ui-approval-{approval_id}-{decision}"),"actor_id":"user","kind":"approval_decision","payload":{"approval_id":approval_id,"decision":decision}}})),
        Effect::UsageStatus => ("usage", json!({})),
        Effect::RewindPoints => ("rewind_points", json!({})),
        Effect::Rewind { node } => ("rewind", json!({"node_id":node})),
        Effect::Fork => ("fork_session", json!({})),
        Effect::ModelStatus => ("model", json!({})),
        Effect::DiscoverModels { provider } => ("discover_models", json!({"provider":provider})),
        Effect::SetModel { agent_id, profile, model, effort } => ("set_model", json!({"agent_id":agent_id,"profile":profile,"model":model,"effort":effort})),
        Effect::UserMessage(text) => ("user_message", json!({"text":text})),
        Effect::SwitchSession(target) => ("switch_session", json!({"session_id":target})),
        Effect::NewSession => ("new_session", json!({})),
        Effect::ArchiveSession(target) => ("archive_session", json!({"session_id":target})),
        Effect::DeleteSession(target) => ("delete_session", json!({"session_id":target})),
        Effect::Quit | Effect::Bell => unreachable!(),
    };
    requests.switching = session_operation(&effect);
    requests.enqueue(RequestKind::Effect(effect, app.model_generation), worker, app, method, params, Duration::from_secs(120));
}

fn receipt_error(result: &Result<Json, String>) -> String {
    match result {
        Err(error) => error.clone(),
        Ok(value) => value.get("error").and_then(Json::as_str).unwrap_or("请求被拒绝").into(),
    }
}

fn apply_effect_result(effect: Effect, generation: u64, result: Result<Json, String>, app: &mut App) -> Vec<Effect> {
    let ok = result.as_ref().ok().and_then(|r| r.get("ok")).and_then(Json::as_bool).unwrap_or(false);
    match effect {
        Effect::Submit { ok_msg, err_msg, .. } => {
            let msg = if ok { ok_msg } else { err_msg.map(|m| m.replace("{error}", &receipt_error(&result))) };
            if let Some(msg) = msg { app.chat.push(("system".into(), msg)); }
        }
        Effect::CancelTask(task_id) => {
            let msg = app::cancel_task_feedback(app.lang, &task_id, &result.unwrap_or(Json::Null));
            app.notify(msg.clone(), app::Severity::Info, 10);
            app.chat.push(("system".into(), msg));
        }
        Effect::AcknowledgeRun(run_id) => {
            let receipt = result.as_ref().ok();
            let acknowledged = receipt.and_then(|r| r.get("status")).and_then(|v| v.as_str()) == Some("acknowledged");
            let msg = if ok && acknowledged {
                app.t("已结清结果不明的回合（{v0}）", &[("v0", &run_id)])
            } else {
                let fallback = app.t("未知错误", &[]);
                let error = receipt
                    .and_then(|r| r.get("error"))
                    .and_then(|v| v.as_str())
                    .or_else(|| result.as_ref().err().map(|e| e.as_str()))
                    .unwrap_or(fallback.as_str())
                    .to_string();
                app.t("结清失败：{v0}", &[("v0", &error)])
            };
            app.notify(msg.clone(), if ok && acknowledged { app::Severity::Info } else { app::Severity::Error }, 10);
            app.chat.push(("system".into(), msg));
        }
        Effect::DecideApproval { decision, .. } => {
            let msg = app::decide_feedback(app.lang, &decision, ok);
            app.notify(msg, if ok { app::Severity::Info } else { app::Severity::Error }, 10);
        }
        Effect::UsageStatus => app.show_usage(result),
        Effect::RewindPoints => app.show_rewind_points(result),
        Effect::Rewind { .. } => app.show_rewind_done(result),
        Effect::Fork => app.show_fork_done(result),
        Effect::ModelStatus => app.show_models(result),
        Effect::DiscoverModels { provider } => app.show_discovered_models(&app.session_id.clone(), generation, &provider, result),
        Effect::SetModel { .. } => app.show_model_set(result),
        Effect::UserMessage(text) => {
            if ok {
                app.composer.record_submission(&text);
                let _ = i18n::write_history(&app.composer.history);
            } else {
                // A late rejection must not overwrite a draft typed while waiting.
                if app.composer.text().is_empty() { app.composer.set_text(&text); }
                let msg = app::rejected_feedback(app.lang, &receipt_error(&result));
                app.chat.push(("system".into(), format!("{msg}\n{text}")));
            }
        }
        Effect::SwitchSession(_) | Effect::NewSession => {
            let target = if let Effect::SwitchSession(target) = effect { target } else { String::new() };
            let op = match result {
                Ok(v) => OpResult::Switched {
                    session_id: v.get("session_id").and_then(Json::as_str).unwrap_or("").into(),
                    catalog: v.get("catalog").cloned().unwrap_or(Json::Null),
                    config_path: v.get("user_config_path").and_then(Json::as_str).unwrap_or("").into(),
                },
                Err(error) => OpResult::Failed { op:"switch", target, error },
            };
            return app.on_op_result(op);
        }
        Effect::ArchiveSession(target) => return app.on_op_result(match result {
            Ok(v) => OpResult::Archived { session_id:target, target:v.get("target").and_then(Json::as_str).unwrap_or("").into(), was_current:v.get("was_current").and_then(Json::as_bool).unwrap_or(false) },
            Err(error) => OpResult::Failed { op:"archive", target, error },
        }),
        Effect::DeleteSession(target) => return app.on_op_result(match result {
            Ok(v) => OpResult::Deleted { session_id:target, was_current:v.get("was_current").and_then(Json::as_bool).unwrap_or(false) },
            Err(error) => OpResult::Failed { op:"delete", target, error },
        }),
        Effect::Quit | Effect::Bell => {}
    }
    vec![]
}

fn handle_mouse(m: event::MouseEvent, terminal: &Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App) {
    let size = terminal.size().unwrap_or(ratatui::layout::Size { width: 80, height: 24 });
    let area = ratatui::layout::Rect { x: 0, y: 0, width: size.width, height: size.height };
    // every mouse event updates the hover position (grey surface + white text marks
    // what a click would hit)
    app.pointer = Some((m.row, m.column));
    if app.settings_open || app.model_picker.is_some() {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        App::new("s1", json!({}), "cfg".into(), "en", false, vec![])
    }

    /// Deterministic stand-in for `teamagents serve`: echoes every request id
    /// back with an empty result, so worker calls succeed with no process-death
    /// races (a dying child's pipe has a deferred-fput window where a write can
    /// still succeed into a pipe nobody ever reads again — that timing artifact
    /// made a `cat`-based version of this test flaky).
    fn mock_engine(tag: &str) -> String {
        // pid+tag: cargo tests share one process, so the pid alone is not unique
        let path = std::env::temp_dir().join(format!("teamagents-tui-mock-engine-{}-{tag}.sh", std::process::id()));
        std::fs::write(&path, "#!/bin/bash\nwhile read -r l; do \
            id=$(printf '%s' \"$l\" | grep -o '\"id\":[0-9]*' | head -1 | cut -d: -f2); \
            printf '{\"id\":%s,\"result\":{\"entries\":[],\"sessions\":[]}}\\n' \"$id\"; done\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn plan_overlay_shows_the_whole_list() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = test_app();
        app.state = Some(json!({"spec": {"leader_id": "leader", "agents": [
            {"id": "leader", "name": "leader", "role": "leader", "runtime_kind": "deepagents", "model_profile": "m"}
        ]}}));
        app.on_plan("leader", &json!([
            {"text": "复现失败", "status": "done"},
            {"text": "修 mul", "status": "in_progress"},
            {"text": "跑测试", "status": "pending"}
        ]));

        app.panel = 0; // team
        app.focus = app::Focus::Panel;
        app.table_cursors.insert("team".into(), (Some("leader".into()), 0));
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert!(app.review_open, "p opens the plan");
        assert!(app.review_title().contains("leader"), "{}", app.review_title());
        assert_eq!(app.review_lines.len(), 3);
        assert_eq!(app.review_lines[0], "[x] 复现失败");
        assert_eq!(app.review_lines[1], "[~] 修 mul");
        assert_eq!(app.review_lines[2], "[ ] 跑测试");

        // Esc closes, and a member without a plan gets told so
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.review_open);
        app.plans.clear();
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert!(!app.review_open);
        let hint = app.t("{v0} 还没有计划", &[("v0", "leader")]);
        assert!(app.toasts.iter().any(|t| t.text.contains(&hint)), "{:?}", app.toasts);
    }

    #[test]
    fn unknown_outcome_runs_are_visible_and_acknowledgeable() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = test_app();
        app.state = Some(json!({
            "spec": {"leader_id": "leader", "agents": [
                {"id": "leader", "name": "leader", "role": "leader", "runtime_kind": "deepagents", "model_profile": "m"}
            ]},
            "runs": [{"run_id": "run_x", "agent_id": "leader", "status": "OUTCOME_UNKNOWN"}]
        }));
        let state = app.state.clone().unwrap();
        app.apply_state(&state); // the poll path fills unknown_runs
        assert_eq!(app.unknown_runs.get("leader").map(String::as_str), Some("run_x"));

        let row = app.team_rows().into_iter().find(|(id, _)| id == "leader").expect("leader row");
        let marker = app.t("结果不明（c 结清）", &[]);
        assert!(row.1[4].0.contains(&marker), "the status cell says so: {:?}", row.1[4].0);

        app.panel = 0; // team
        app.focus = app::Focus::Panel;
        app.table_cursors.insert("team".into(), (Some("leader".into()), 0));
        let effects = app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        assert_eq!(effects.len(), 1, "{effects:?}");
        assert!(matches!(&effects[0], Effect::AcknowledgeRun(run) if run == "run_x"), "{effects:?}");

        // a member without one just gets told so
        app.unknown_runs.clear();
        let effects = app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(effects.is_empty());
        let empty_hint = app.t("该成员没有结果不明的回合", &[]);
        assert!(app.toasts.iter().any(|t| t.text.contains(&empty_hint)), "{:?}", app.toasts);
    }

    #[test]
    fn plan_status_strip_tracks_the_selected_member() {
        let mut app = test_app();
        assert!(app.plan_status().is_none(), "no plan, no strip");
        app.state = Some(json!({"spec": {"leader_id": "leader", "agents": [
            {"id": "leader", "name": "leader", "role": "leader", "runtime_kind": "deepagents", "model_profile": "m"}
        ]}}));

        // a push from the leader shows progress and the in-progress item
        app.on_plan("leader", &json!([
            {"text": "复现失败", "status": "done"},
            {"text": "修 mul", "status": "in_progress"},
            {"text": "跑测试", "status": "pending"}
        ]));
        let (summary, current) = app.plan_status().expect("leader's plan is shown");
        assert!(summary.contains("1/3") && summary.contains("leader"), "{summary}");
        assert_eq!(current, "修 mul");

        // the strip needs a row: geometry reserves it only when a plan exists
        let area = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 30 };
        let with_plan = ui::geometry(&app, area);
        assert_eq!(with_plan.plan.height, 1, "a plan gets its own status row");
        let mut bare = test_app();
        bare.state = app.state.clone();
        let without = ui::geometry(&bare, area);
        assert_eq!(without.plan.height, 0);
        assert_eq!(with_plan.chat.height, without.chat.height - 1, "the row comes out of the chat area");

        // everything done reads as such
        app.on_plan("leader", &json!([{"text": "复现失败", "status": "done"}]));
        let (summary, current) = app.plan_status().unwrap();
        assert!(summary.contains("1/1"), "{summary}");
        assert!(current.is_empty(), "no in-progress item: {current}");
    }

    #[test]
    fn review_overlay_shows_the_last_edit_diff() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = test_app();
        app.state = Some(json!({"spec": {"agents": [
            {"id": "alpha", "name": "alpha", "role": "worker", "runtime_kind": "deepagents", "model_profile": "m"}
        ]}}));
        assert!(!app.open_review("alpha"), "nothing to review yet");

        app.on_tool_result(
            "alpha",
            "edit_file",
            true,
            "{\"path\":\"a.txt\"}",
            "edited a.txt\n@@ line 1 @@\n-old\n+new",
        );
        assert!(app.open_review("alpha"), "the member's diff opens");
        assert!(app.review_open);
        assert!(app.review_title().contains("alpha"));
        assert_eq!(app.review_lines[0], "edited a.txt");
        assert_eq!(app.review_lines[2], "-old");

        // Esc closes; a member with no recorded edit still reports nothing
        assert!(app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).is_empty());
        assert!(!app.review_open);
        assert!(!app.open_review("ghost"));

        // the panel key path: `v` on the selected team row opens the same view
        app.panel = 0; // team
        app.focus = app::Focus::Panel;
        app.table_cursors.insert("team".into(), (Some("alpha".into()), 0));
        app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        assert!(app.review_open, "v on the member row opens the review");
    }

    #[test]
    fn team_panel_shows_each_members_last_tool() {
        let mut app = test_app();
        app.state = Some(json!({"spec": {"agents": [
            {"id": "alpha", "name": "alpha", "role": "worker", "runtime_kind": "deepagents", "model_profile": "m"},
            {"id": "beta", "name": "beta", "role": "worker", "runtime_kind": "deepagents", "model_profile": "m"}
        ]}}));
        let headers = i18n::table_headers("team");
        let rows = app.team_rows();
        assert_eq!(rows[0].1.len(), headers.len(), "every row must fill every column");
        let activity = |id: &str| {
            rows.iter().find(|(key, _)| key == id).map(|(_, cells)| cells.last().unwrap().0.clone()).unwrap()
        };
        assert_eq!(activity("alpha"), "-", "no tool run yet");

        app.on_tool("alpha", "edit_file", true, "{}");
        app.on_tool("beta", "shell", false, "{}");
        let rows = app.team_rows();
        let activity = |id: &str| {
            rows.iter().find(|(key, _)| key == id).map(|(_, cells)| cells.last().unwrap().0.clone()).unwrap()
        };
        assert!(activity("alpha").starts_with("edit_file"), "{}", activity("alpha"));
        assert!(activity("beta").starts_with("✗ shell"), "{}", activity("beta"));
    }

    #[test]
    fn tool_activity_lands_in_the_log_panel() {
        let mut app = test_app();
        app.on_tool("alpha-fixer", "edit_file", true, "{\"path\":\"alpha/alpha.py\"}");
        app.on_tool("beta-fixer", "shell", false, "{\"command\":\"cd beta && python3 check.py\"}");
        assert_eq!(app.log_lines.len(), 2);
        assert!(app.log_lines[0].contains("edit_file") && app.log_lines[0].contains("alpha-fixer"), "{:?}", app.log_lines[0]);
        assert!(app.log_lines[0].contains("alpha/alpha.py"), "arguments show up: {:?}", app.log_lines[0]);
        assert!(app.log_lines[1].starts_with("      ✗"), "a failed call is marked: {:?}", app.log_lines[1]);

        // a member filter hides other members' tool lines too
        app.log_member = Some("alpha-fixer".into());
        app.on_tool("beta-fixer", "ls", true, "{}");
        assert_eq!(app.log_lines.len(), 2, "filtered member stays out of the log");
    }

    #[test]
    fn log_panel_keeps_only_the_newest_lines() {
        let mut app = test_app();
        for index in 0..2500 {
            app.on_tool("leader", "ls", true, &format!("{{\"path\":\"{index}\"}}"));
        }
        assert_eq!(app.log_lines.len(), 2000, "the panel is a ring, not a leak");
        assert!(app.log_lines[0].contains("\"path\":\"500\""), "oldest lines are the ones dropped: {:?}", app.log_lines[0]);
    }

    #[test]
    fn stale_background_result_cannot_replace_session_state() {
        let path = mock_engine("stale");
        let worker = Worker::spawn(&path).unwrap();
        let mut app = test_app();
        let mut requests = AsyncUi::default();
        requests.enqueue(RequestKind::Shared, &worker, &app, "list_sessions", json!({}), Duration::from_secs(5));
        worker.call("barrier", json!({})).unwrap();
        requests.generation += 1;
        app.shared = vec![json!({"id":"new-session-data"})];
        assert!(!requests.drain(&worker, &mut app));
        assert_eq!(app.shared, vec![json!({"id":"new-session-data"})]);
        assert!(requests.pending.is_empty());
        worker.kill();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stalled_control_keeps_input_and_quit_responsive() {
        let path = std::env::temp_dir().join(format!("teamagents-tui-stalled-{}.sh", std::process::id()));
        std::fs::write(&path, "#!/bin/bash\nwhile read -r line; do :; done\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let worker = Worker::spawn(path.to_str().unwrap()).unwrap();
        let mut app = test_app();
        let mut requests = AsyncUi::default();
        let started = Instant::now();
        run_effect(Effect::UserMessage("first".into()), &worker, &mut app, &mut requests);
        run_effect(Effect::UsageStatus, &worker, &mut app, &mut requests);
        app.handle_paste("next draft");
        run_effect(Effect::Quit, &worker, &mut app, &mut requests);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(requests.pending.len(), 2);
        assert!(app.should_quit);
        assert_eq!(app.composer.text(), "next draft");
        assert!(!requests.drain(&worker, &mut app));
        worker.kill();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn transition_blocks_further_mutations_and_rejection_keeps_new_draft() {
        let path = mock_engine("transition");
        let worker = Worker::spawn(&path).unwrap();
        let mut app = test_app();
        let mut requests = AsyncUi::default();
        run_effect(Effect::SwitchSession("s2".into()), &worker, &mut app, &mut requests);
        run_effect(Effect::UserMessage("held".into()), &worker, &mut app, &mut requests);
        run_effect(Effect::NewSession, &worker, &mut app, &mut requests);
        assert_eq!(requests.pending.len(), 1);
        assert_eq!(app.composer.text(), "held");
        app.composer.set_text("new draft");
        apply_effect_result(Effect::UserMessage("old message".into()), 0, Err("denied".into()), &mut app);
        assert_eq!(app.composer.text(), "new draft");
        assert!(app.chat.last().unwrap().1.contains("old message"));
        worker.kill();
        let _ = std::fs::remove_file(path);
    }
}
