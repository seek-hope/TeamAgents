//! teamagents-tui: ratatui front-end for TeamAgents. The UI is a pure
//! client: execution lives in the headless engine (`teamagents serve`),
//! authoritative state in the core.

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind, MouseEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use serde_json::json;

use teamagents_tui::daemon_client::DaemonClient;
use teamagents_tui::v2app::{Focus as V2Focus, V2App, V2Effect, View as V2View};
use teamagents_tui::v2ui;

struct Args {
    cwd: Option<String>,
    resume: Option<String>,
    full_auto: bool,
    team: Option<String>,
    engine_bin: String,
    /// v2 session daemon socket (R19): --daemon SOCK or --state-root DIR.
    daemon: Option<String>,
    state_root: Option<String>,
}

impl Args {
    /// The v2 daemon socket when either v2 flag is present: --daemon wins,
    /// --state-root derives the conventional <root>/daemon.sock (§9).
    fn daemon_socket(&self) -> Option<std::path::PathBuf> {
        if let Some(socket) = &self.daemon {
            return Some(socket.into());
        }
        self.state_root.as_ref().map(|root| std::path::Path::new(root).join("daemon.sock"))
    }
}

fn usage() -> ! {
    eprintln!("teamagents-tui --daemon SOCK | --state-root DIR   # v2 会话 daemon（R19/R29 必填）");
    eprintln!("              [--cwd DIR]");
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
        daemon: None,
        state_root: None,
    };
    let takes_value = |a: &mut Args, i: usize, argv: &[String]| -> usize {
        // value flags consume the next argument
        let _ = a;
        if i + 1 < argv.len() {
            2
        } else {
            1
        }
    };
    let mut engine_flag: Option<String> = None;
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let step = match argv[i].as_str() {
            "--cwd" => {
                a.cwd = argv.get(i + 1).cloned();
                takes_value(&mut a, i, &argv)
            }
            "--resume" => {
                a.resume = argv.get(i + 1).cloned();
                takes_value(&mut a, i, &argv)
            }
            "--full-auto" => {
                a.full_auto = true;
                1
            }
            "--team" => {
                a.team = argv.get(i + 1).cloned();
                takes_value(&mut a, i, &argv)
            }
            "--engine" => {
                engine_flag = argv.get(i + 1).cloned();
                takes_value(&mut a, i, &argv)
            }
            "--daemon" => {
                a.daemon = argv.get(i + 1).cloned();
                takes_value(&mut a, i, &argv)
            }
            "--state-root" => {
                a.state_root = argv.get(i + 1).cloned();
                takes_value(&mut a, i, &argv)
            }
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
    // R29: the v2 daemon is the only backend; the legacy in-process path is
    // retired, so a missing socket is a usage error rather than a silent
    // fallback to code that no longer receives fixes
    let Some(socket) = args.daemon_socket() else {
        usage();
    };
    v2_main(&socket);
}
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

/// R19 v2 entry: connect the session daemon and run the conversation
/// interface (plan §9 — the TUI never executes anything itself).
fn v2_main(socket: &std::path::Path) -> ! {
    let mut client = match DaemonClient::connect(socket) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("无法连接 daemon ({}): {e}", socket.display());
            eprintln!("先启动会话：teamagents daemon [--state-root DIR]");
            std::process::exit(1);
        }
    };
    let mut app = V2App::new(&client.session_id.clone());

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
    let mut stdout = std::io::stdout();
    enable_raw_mode().expect("raw mode");
    stdout.execute(EnterAlternateScreen).expect("alt screen");
    if supports_keyboard_enhancement().unwrap_or(false) {
        let _ = stdout.execute(crossterm::event::PushKeyboardEnhancementFlags(
            crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | crossterm::event::KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
        ));
    }
    execute!(stdout, crossterm::event::EnableMouseCapture, crossterm::event::EnableBracketedPaste).ok();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).expect("terminal");

    let code = run_v2(&mut terminal, &mut client, &mut app);

    restore_terminal();
    drop(terminal);
    std::process::exit(code);
}

fn v2_sync_checkpoint(client: &mut DaemonClient, app: &mut V2App) {
    match client.checkpoint() {
        Ok((snapshot, watermark)) => {
            app.apply_checkpoint(snapshot, watermark);
            app.mark_connected();
        }
        Err(e) => app.mark_disconnected(&e),
    }
}

fn v2_refresh_history(client: &mut DaemonClient, app: &mut V2App) {
    let Some(instance) = app.active_instance().map(|i| i.id.clone()) else { return };
    match client.call("history", json!({"instance_id": instance, "limit": 400})) {
        Ok(result) => app.apply_history(result),
        Err(e) => app.mark_disconnected(&e),
    }
}

fn v2_refresh_approvals(client: &mut DaemonClient, app: &mut V2App) {
    match client.call("approvals", json!({})) {
        Ok(result) => app.apply_approvals(result),
        Err(e) => app.mark_disconnected(&e),
    }
}

fn v2_refresh_tasks(client: &mut DaemonClient, app: &mut V2App) {
    match client.call("tasks", json!({})) {
        Ok(result) => app.apply_tasks(result),
        Err(e) => app.mark_disconnected(&e),
    }
}

fn v2_refresh_grants(client: &mut DaemonClient, app: &mut V2App) {
    match client.call("grants", json!({})) {
        Ok(result) => app.apply_grants(result),
        Err(e) => app.mark_disconnected(&e),
    }
}

fn run_v2_effect(effect: V2Effect, client: &mut DaemonClient, app: &mut V2App) {
    match effect {
        V2Effect::SubmitInput { instance, envelope, text } => {
            // the envelope id doubles as the command id: a retry after a lost
            // reply dedups at the control plane (§9)
            let result = client.command(
                &format!("input-{envelope}"),
                "submit_input",
                json!({"instance_id": instance, "envelope_id": envelope, "text": text}),
            );
            if let Err(e) = result {
                app.submit_failed(&e);
            }
        }
        V2Effect::Decide { approval_id, decision } => {
            let result = client.command(
                &format!("decide-{approval_id}-{decision}"),
                decision,
                json!({"approval_id": approval_id}),
            );
            match result {
                Ok(_) => v2_refresh_approvals(client, app),
                Err(e) => app.decide_failed(&e),
            }
        }
        // panel interventions are business commands with fresh command ids;
        // the user can retry after a disconnect without duplicating an effect
        V2Effect::SetLifecycle { instance, lifecycle } => {
            let command_id = format!("lc-{}", uuid::Uuid::new_v4());
            let result = client.command(
                &command_id,
                "set_lifecycle",
                json!({"instance_id": instance, "lifecycle": lifecycle, "reason": "tui 面板干预"}),
            );
            match result {
                Ok(_) => v2_sync_checkpoint(client, app),
                Err(e) => app.command_failed(&e),
            }
        }
        V2Effect::CancelTask { task_id } => {
            let command_id = format!("ct-{}", uuid::Uuid::new_v4());
            let result =
                client.command(&command_id, "cancel_task", json!({"task_id": task_id, "reason": "tui 面板取消"}));
            match result {
                Ok(_) => v2_refresh_tasks(client, app),
                Err(e) => app.command_failed(&e),
            }
        }
        V2Effect::Quit => {}
    }
}

fn run_v2(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    client: &mut DaemonClient,
    app: &mut V2App,
) -> i32 {
    v2_sync_checkpoint(client, app);
    v2_refresh_history(client, app);
    v2_refresh_approvals(client, app);
    v2_refresh_tasks(client, app);
    v2_refresh_grants(client, app);
    let mut last_event_poll = Instant::now();
    let mut last_slow = Instant::now();
    let mut dirty = true;
    let mut active_id = app.active_instance().map(|i| i.id.clone());
    let mut last_view = app.view;
    loop {
        if dirty {
            let _ = terminal.draw(|f| v2ui::render(f, app));
            dirty = false;
        }
        if app.quit {
            return 0;
        }
        if event::poll(Duration::from_millis(40)).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                    if let Some(effect) = app.handle_key(key) {
                        run_v2_effect(effect, client, app);
                    }
                    dirty = true;
                }
                Ok(Event::Mouse(m)) => {
                    match m.kind {
                        MouseEventKind::ScrollUp => app.wheel(true),
                        MouseEventKind::ScrollDown => app.wheel(false),
                        MouseEventKind::Down(_) => {
                            let size = terminal
                                .size()
                                .map(|s| ratatui::layout::Rect::new(0, 0, s.width, s.height))
                                .unwrap_or_default();
                            let geo = v2ui::geometry(app, size);
                            match app.view {
                                V2View::Chat => {
                                    if let Some(index) = v2ui::approval_at(&geo, app, m.row, m.column) {
                                        app.focus = V2Focus::Approvals;
                                        app.approval_sel = index;
                                    }
                                }
                                V2View::Instances => {
                                    if let Some(index) = v2ui::panel_row_at(&geo, m.row, m.column) {
                                        if index < app.instances.len() {
                                            app.instance_sel = index;
                                        }
                                    }
                                }
                                V2View::Tasks => {
                                    if let Some(index) = v2ui::panel_row_at(&geo, m.row, m.column) {
                                        if index < app.tasks.len() {
                                            app.task_sel = index;
                                        }
                                    }
                                }
                                V2View::Topology => {}
                            }
                        }
                        _ => {}
                    }
                    dirty = true;
                }
                Ok(Event::Resize(_, _)) => dirty = true,
                Ok(Event::Paste(text)) => {
                    for c in text.chars() {
                        if c == '\n' {
                            app.composer.insert_newline();
                        } else if !c.is_control() {
                            app.composer.insert_char(c);
                        }
                    }
                    dirty = true;
                }
                _ => {}
            }
        }
        // instance switching needs the new conversation's history
        let now_active = app.active_instance().map(|i| i.id.clone());
        if now_active != active_id {
            active_id = now_active;
            v2_refresh_history(client, app);
            dirty = true;
        }
        // entering a panel refreshes its data once; events keep it current
        if app.view != last_view {
            last_view = app.view;
            match app.view {
                V2View::Instances => v2_sync_checkpoint(client, app),
                V2View::Tasks => v2_refresh_tasks(client, app),
                V2View::Topology => {
                    v2_refresh_grants(client, app);
                    v2_refresh_tasks(client, app);
                }
                V2View::Chat => {}
            }
            dirty = true;
        }
        if last_event_poll.elapsed() >= Duration::from_millis(150) {
            last_event_poll = Instant::now();
            match client.poll_events() {
                Ok(events) => {
                    app.mark_connected();
                    let refresh = app.apply_events(&events);
                    if refresh.history {
                        v2_refresh_history(client, app);
                    }
                    if refresh.approvals {
                        v2_refresh_approvals(client, app);
                    }
                    if refresh.checkpoint {
                        v2_sync_checkpoint(client, app);
                    }
                    if refresh.tasks {
                        v2_refresh_tasks(client, app);
                    }
                    if refresh.grants {
                        v2_refresh_grants(client, app);
                    }
                    if !events.is_empty() {
                        dirty = true;
                    }
                }
                Err(e) => {
                    app.mark_disconnected(&e);
                    dirty = true;
                }
            }
        }
        if last_slow.elapsed() >= Duration::from_secs(1) {
            last_slow = Instant::now();
            if app.disconnected {
                // reconnect: a fresh checkpoint re-syncs snapshot + watermark
                v2_sync_checkpoint(client, app);
                if !app.disconnected {
                    v2_refresh_history(client, app);
                    v2_refresh_approvals(client, app);
                    v2_refresh_tasks(client, app);
                    v2_refresh_grants(client, app);
                    dirty = true;
                }
            } else {
                v2_refresh_approvals(client, app);
            }
        }
    }
}

#[link(name = "c")]
extern "C" {
    #[link_name = "isatty"]
    fn libc_isatty(fd: i32) -> i32;
}

fn atty_stdout() -> bool {
    unsafe { libc_isatty(1) == 1 }
}
