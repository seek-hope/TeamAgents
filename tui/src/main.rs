//! teamagents-tui: ratatui front-end for TeamAgents. The UI is a pure
//! client: execution lives in the headless engine (`teamagents serve`),
//! authoritative state in the core.

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use serde_json::json;

use teamagents_tui::daemon_client::DaemonClient;
use teamagents_tui::session_client::SessionClient;
use teamagents_tui::v2app::{Focus as V2Focus, V2App, V2Effect};
use teamagents_tui::v2ui;

struct Args {
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
    eprintln!("teamagents-tui --daemon SOCK | --state-root DIR   # session daemon (required)");
    std::process::exit(2);
}

/// A flag this front-end cannot honour: the session (its workspace, its mode)
/// belongs to the daemon the engine boots, so the client must say so instead of
/// accepting the flag and ignoring it (D-73).
fn refuse(message: &str) -> ! {
    eprintln!("{message}");
    eprintln!("usage: teamagents-tui --daemon SOCK | --state-root DIR");
    std::process::exit(2);
}

fn parse_args() -> Args {
    let mut a = Args { daemon: None, state_root: None };
    let takes_value = |i: usize, argv: &[String]| -> usize {
        // value flags consume the next argument
        if i + 1 < argv.len() {
            2
        } else {
            1
        }
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let step = match argv[i].as_str() {
            // the session's workspace and mode belong to the daemon: the engine
            // boots it (`teamagents --cwd DIR --full-auto`) and prints what the
            // live session uses, so a flag here would be accepted and ignored
            "--cwd" => refuse(
                "--cwd is not a TUI flag: the session's workspace belongs to the daemon. \
                 Start it with `teamagents --cwd DIR`; a running session keeps its own.",
            ),
            "--full-auto" => refuse(
                "--full-auto is not a TUI flag: the session's mode belongs to the daemon. \
                 Start it with `teamagents --full-auto`; a running session keeps the mode it booted with.",
            ),
            "--resume" | "--team" => refuse(&format!(
                "{} is no longer supported: the daemon owns the session. Run `teamagents` (TUI) or \
                 `teamagents exec \"…\"`.",
                argv[i]
            )),
            // D-180 retired the unused flag. D-446 gives session operations an explicit CLI locator.
            "--engine" => refuse(
                "--engine is not a TUI flag. Launch with teamagents, or set TEAMAGENTS_SESSION_CLI \
                 to locate the engine used for session browsing.",
            ),
            "--daemon" => {
                a.daemon = argv.get(i + 1).cloned();
                takes_value(i, &argv)
            }
            "--state-root" => {
                a.state_root = argv.get(i + 1).cloned();
                takes_value(i, &argv)
            }
            _ => usage(),
        };
        i += step;
    }
    a
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--display-worker") {
        std::process::exit(teamagents_tui::graphics::run_display_worker());
    }
    let args = parse_args();
    if !atty_stdout() {
        eprintln!("the TUI needs a real terminal; for scripts use teamagents exec \"…\"");
        std::process::exit(1);
    }
    // the daemon is the only backend: a missing socket is a usage error rather
    // than a silent in-process fallback
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
/// interface (plan §9 — models and tools execute in the engine).
fn enter_terminal() {
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
}

fn v2_main(socket: &std::path::Path) -> ! {
    let socket = socket.to_path_buf();
    let client = match DaemonClient::connect(&socket) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cannot connect to the daemon ({}): {e}", socket.display());
            eprintln!("start the session first: teamagents daemon [--state-root DIR]");
            std::process::exit(1);
        }
    };
    let mut app = V2App::new(&client.session_id);
    app.session_root = std::path::Path::new(&client.state_root)
        .canonicalize()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| client.state_root.clone());

    app.graphics = teamagents_tui::graphics::Graphics::terminal().unwrap_or_else(|e| refuse(&e));
    let main_thread = std::thread::current().id();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // A caught renderer panic must not restore the main thread's terminal.
        if std::thread::current().name() == Some("teamagents-graphics") {
            return;
        }
        if std::thread::current().id() == main_thread {
            restore_terminal();
        }
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

    let code = run_v2(&mut terminal, client, app);

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

fn run_v2_effect(effect: V2Effect, client: &mut DaemonClient, app: &mut V2App, sessions: &mut SessionClient) {
    match effect {
        V2Effect::Sessions(action) => {
            if matches!(action, teamagents_tui::sessions::SessionAction::CancelSearch) {
                sessions.cancel_search(app);
            } else {
                sessions.start(action, app);
            }
        }
        V2Effect::Queue(action) => {
            use teamagents_tui::queue::Action;
            let result = match &action {
                Action::List { after } => {
                    client.call("inputs", json!({"instance_id":"i-leader", "after":after, "limit":20}))
                }
                Action::Revise { envelope, revision, text, command_id } => {
                    let method = if text.is_some() { "edit_input" } else { "cancel_input" };
                    client.command(
                        command_id,
                        method,
                        json!({"instance_id":"i-leader", "envelope_id":envelope, "revision":revision, "text":text}),
                    )
                }
            };
            if let Some(queue) = &mut app.queue {
                match result {
                    Err(error) => queue.fail(error),
                    Ok(reply) => {
                        if matches!(action, Action::List { .. }) {
                            queue.apply(&reply);
                        } else {
                            queue.saved();
                            match client.call("inputs", json!({"instance_id":"i-leader", "limit":20})) {
                                Ok(reply) => queue.apply(&reply),
                                Err(error) => queue.fail(error),
                            }
                        }
                    }
                }
            }
        }
        V2Effect::InputPaths { prefix } => match client.call("input_paths", json!({"prefix":prefix})) {
            Ok(reply) => {
                app.attachments.apply(&reply, &app.composer);
                if app.attachments.candidates.is_empty() {
                    app.command_note("No matching workspace paths.");
                }
            }
            Err(error) => app.command_failed(&error),
        },
        V2Effect::SubmitInput { instance, envelope, text, attachments } => {
            // the envelope id doubles as the command id: a retry after a lost
            // reply dedups at the control plane (§9)
            let result = client.command(
                &format!("input-{envelope}"),
                "submit_input",
                json!({"instance_id": instance, "envelope_id": envelope, "text": text, "attachments":attachments}),
            );
            match result {
                Ok(reply) => {
                    app.attachments.accepted();
                    app.input_receipt(&envelope, &reply);
                }
                Err(e) => {
                    app.composer.set_text(&text);
                    app.submit_failed(&e);
                    app.command_note("No send acknowledgement: delivery may have succeeded. The draft is restored; retrying it unchanged reuses its input identity. Inspect Ctrl+P/history before changing it.");
                }
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
        V2Effect::Interrupt { instance } => {
            let command_id = format!("ir-{}", uuid::Uuid::new_v4());
            match client.command(
                &command_id,
                "interrupt_instance",
                json!({"instance_id": instance, "reason": "interrupted from the TUI"}),
            ) {
                Ok(reply) => {
                    if reply["interrupted"] == json!(true) {
                        app.command_note(
                            "Leader model request interrupted. Running tools and delegated work continue.",
                        );
                    } else {
                        app.command_note(format!(
                            "Nothing interrupted: {}. Running tools and delegated work continue.",
                            reply["reason"].as_str().unwrap_or("no model request in flight")
                        ));
                    }
                    v2_sync_checkpoint(client, app);
                }
                Err(e) => app.command_failed(&e),
            }
        }
        V2Effect::SetProfile { instance, model, effort } => {
            let mut params = json!({"instance_id": instance});
            if let Some(model) = model {
                params["model"] = json!(model);
            }
            if let Some(effort) = effort {
                params["effort"] = json!(effort);
            }
            match client.command(&format!("profile-{}", uuid::Uuid::new_v4()), "set_instance_profile", params) {
                Ok(reply) => {
                    app.command_note(format!(
                        "Profile: model {} · effort {}. It applies at the next turn; an in-flight turn keeps \
                         its profile.",
                        reply["model"].as_str().unwrap_or("?"),
                        reply["effort"].as_str().unwrap_or("?")
                    ));
                    v2_sync_checkpoint(client, app);
                }
                Err(error) => app.command_note(format!("The profile change was refused: {error}")),
            }
        }
        V2Effect::EditDraft { .. } => {}
        V2Effect::Quit => {}
    }
}

fn edit_in_editor(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut V2App, initial: &str) {
    restore_terminal();
    let result = teamagents_tui::editor::edit_text(initial, None);
    enter_terminal();
    let _ = terminal.clear();
    app.finish_editor(initial, result);
}

fn run_v2(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, mut client: DaemonClient, mut app: V2App) -> i32 {
    let mut sessions = SessionClient::new(&app.session_root);
    let mut images = teamagents_tui::graphics::KittyOutput::default();
    v2_sync_checkpoint(&mut client, &mut app);
    v2_refresh_history(&mut client, &mut app);
    v2_refresh_approvals(&mut client, &mut app);
    let mut last_event_poll = Instant::now();
    let mut last_slow = Instant::now();
    let mut dirty = true;
    let mut active_id = app.active_instance().map(|i| i.id.clone());
    // The connection indicator is part of the status line, so a change in it must rebuild the frame even
    // when nothing else happened: a reconnect that delivers no events used to leave a stale
    // "disconnected, reconnecting..." on screen while the client was in fact connected again (D-99).
    let mut last_disconnected = app.disconnected;
    loop {
        dirty |= sessions.poll(&mut client, &mut app);
        dirty |= app.graphics.poll();
        if dirty {
            let _ = images.clear(&mut std::io::stdout());
            let _ = terminal.draw(|f| v2ui::render(f, &mut app));
            let _ = images.paint(&mut std::io::stdout(), &app.pictures);
            dirty = false;
        }
        if app.quit {
            let _ = images.finish(&mut std::io::stdout());
            return 0;
        }
        if event::poll(Duration::from_millis(40)).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                    if let Some(effect) = app.handle_key(key) {
                        match effect {
                            V2Effect::EditDraft { initial } => edit_in_editor(terminal, &mut app, &initial),
                            other => run_v2_effect(other, &mut client, &mut app, &mut sessions),
                        }
                    }
                    dirty = true;
                }
                Ok(Event::Mouse(m)) => {
                    let size = terminal
                        .size()
                        .map(|s| ratatui::layout::Rect::new(0, 0, s.width, s.height))
                        .unwrap_or_default();
                    if let Some(browser) = app.browser.as_mut() {
                        if let Some(action) = browser.mouse(size, m) {
                            sessions.start(action, &mut app);
                        }
                    } else {
                        v2ui::handle_mouse(&mut app, size, m);
                    }
                    dirty = true;
                }
                Ok(Event::Resize(_, _)) => {
                    app.graphics.resize();
                    dirty = true;
                }
                Ok(Event::Paste(_)) if app.help_open || app.profile.is_some() => {}
                Ok(Event::Paste(text)) if app.queue.is_some() => {
                    if let Some(queue) = &mut app.queue {
                        queue.paste(&text);
                    }
                    dirty = true;
                }
                Ok(Event::Paste(text)) if app.browser.as_ref().is_some_and(|browser| browser.search_editing) => {
                    if let Some(browser) = &mut app.browser {
                        browser.paste_search(&text);
                    }
                    dirty = true;
                }
                Ok(Event::Paste(text)) if app.focus == V2Focus::Composer && app.browser.is_none() => {
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
            v2_refresh_history(&mut client, &mut app);
            dirty = true;
        }
        if app.disconnected != last_disconnected {
            last_disconnected = app.disconnected;
            dirty = true;
        }
        if last_event_poll.elapsed() >= Duration::from_millis(150) {
            last_event_poll = Instant::now();
            // §9: the model's streaming preview is a transient read, not an event. Repaint only when it changes.
            let before = app.active_preview().map(str::to_string);
            if let Ok(result) = client.call("previews", serde_json::json!({})) {
                app.apply_previews(&result["previews"]);
                if app.active_preview() != before.as_deref() {
                    dirty = true;
                }
            }
            match client.poll_events() {
                Ok(events) => {
                    app.mark_connected();
                    let refresh = app.apply_events(&events);
                    if refresh.history {
                        v2_refresh_history(&mut client, &mut app);
                    }
                    if refresh.approvals {
                        v2_refresh_approvals(&mut client, &mut app);
                    }
                    if refresh.checkpoint {
                        v2_sync_checkpoint(&mut client, &mut app);
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
                v2_sync_checkpoint(&mut client, &mut app);
                if !app.disconnected {
                    v2_refresh_history(&mut client, &mut app);
                    v2_refresh_approvals(&mut client, &mut app);
                    dirty = true;
                }
            } else {
                let before = app.approvals.clone();
                v2_refresh_approvals(&mut client, &mut app);
                dirty |= app.approvals != before;
            }
        }
    }
}

fn atty_stdout() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}
