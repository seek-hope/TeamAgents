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
            "--help" | "-h" => {
                println!("teamagents-tui [--daemon SOCKET] [--state-root PATH]\nConnect to a running session; --daemon takes precedence over PATH/daemon.sock.\nNormally launch with teamagents. Use teamagents exec for scripts.");
                std::process::exit(0);
            }
            // the session's workspace and mode belong to the daemon: the engine
            // boots it (`teamagents --cwd DIR`) and prints what the
            // live session uses, so a flag here would be accepted and ignored
            "--cwd" => refuse(
                "--cwd is not a TUI flag: the session's workspace belongs to the daemon. \
                 Start it with `teamagents --cwd DIR`; a running session keeps its own.",
            ),
            "--full-auto" => refuse(
                "--full-auto is not a TUI flag. The daemon already uses full_auto, subject to capabilities and approval policy.",
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
static ENHANCED_KEYS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn restore_terminal() {
    let mut out = std::io::stdout();
    let _ = execute!(
        out,
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste,
        LeaveAlternateScreen
    );
    if ENHANCED_KEYS.get() == Some(&true) {
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
    if *ENHANCED_KEYS.get_or_init(|| supports_keyboard_enhancement().unwrap_or(false)) {
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
    enter_terminal();
    let backend = CrosstermBackend::new(std::io::stdout());
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
    app.history_dirty = true;
    let Some(instance) = app.active_instance().map(|i| i.id.clone()) else { return };
    match client.call("history", json!({"instance_id": instance, "limit": 400,"current_epoch":true})) {
        Ok(result) => app.apply_history(result),
        Err(e) => app.mark_disconnected(&e),
    }
}

fn v2_refresh_observations(client: &mut DaemonClient, app: &mut V2App, retry: bool) -> bool {
    if app.browser.is_some()
        || app.queue.is_some()
        || app.profile.is_some()
        || app.help_open
        || app.palette.is_some()
        || app.rewind_modal.is_some()
        || app.mcp_modal.is_some()
        || app.approval_detail.is_some()
        || app.confirm.is_some()
    {
        return false;
    }
    let mut changed = false;
    if app.team.tasks_visible && app.team.tasks_dirty && (retry || app.team.tasks_error.is_none()) {
        match client.call("tasks", json!({})) {
            Ok(reply) => app.team.apply_tasks(&reply),
            Err(error) => app.team.tasks_error = Some(error),
        }
        changed = true;
    }
    if app.team.tree_visible && app.team.tree_dirty && (retry || app.team.tree_error.is_none()) {
        match client.call("search_tree", json!({})) {
            Ok(reply) => app.team.apply_tree(&reply),
            Err(error) => app.team.fail_tree(error),
        }
        changed = true;
    }
    if app.team.member_visible {
        if let Some(member) = &mut app.team.member {
            if member.dirty && (retry || member.error.is_none()) {
                match client.call("history", json!({"instance_id":member.id, "limit":400})) {
                    Ok(reply) => member.apply(&reply),
                    Err(error) => member.fail(error),
                }
                changed = true;
            }
        }
    }
    changed
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
            let Some(leader) = app.active_instance().map(|i| i.id.clone()) else { return };
            let result = match &action {
                Action::List { after } => {
                    client.call("inputs", json!({"instance_id":leader, "after":after, "limit":20}))
                }
                Action::Revise { envelope, revision, text, command_id } => {
                    let method = if text.is_some() { "edit_input" } else { "cancel_input" };
                    client.command(
                        command_id,
                        method,
                        json!({"instance_id":leader, "envelope_id":envelope, "revision":revision, "text":text}),
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
                            match client.call("inputs", json!({"instance_id":leader, "limit":20})) {
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
        V2Effect::SubmitInput { instance, envelope, text, attachments, after_work } => {
            // the envelope id doubles as the command id: a retry after a lost
            // reply dedups at the control plane (§9)
            let mut params =
                json!({"instance_id": instance, "envelope_id": envelope, "text": text, "attachments":attachments});
            if after_work {
                params["after"] = json!("work");
            }
            let result = client.command(&format!("input-{envelope}"), "submit_input", params);
            match result {
                Ok(reply) => {
                    app.attachments.accepted();
                    app.input_receipt(&envelope, &reply);
                }
                Err(e) => {
                    app.composer.set_text(&if after_work { format!("/later {text}") } else { text });
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
        V2Effect::SetLifecycle { instance, lifecycle, reason } => {
            let command_id = match lifecycle {
                "TERMINATED" => format!("lifecycle-{instance}-{lifecycle}"),
                _ => format!("lifecycle-{instance}-{lifecycle}-{}", uuid::Uuid::new_v4()),
            };
            match client.command(
                &command_id,
                "set_lifecycle",
                json!({"instance_id": instance, "lifecycle": lifecycle, "reason": reason}),
            ) {
                Ok(_) => {
                    app.flash(format!("{instance} -> {lifecycle}"));
                    v2_sync_checkpoint(client, app);
                }
                Err(e) => app.command_failed(&e),
            }
        }
        V2Effect::CancelTask { task_id, reason } => {
            match client.command(
                &format!("cancel-{task_id}"),
                "cancel_task",
                json!({"task_id": task_id, "reason": reason}),
            ) {
                Ok(_) => {
                    app.flash(format!("Cancelled task {task_id}."));
                    v2_sync_checkpoint(client, app);
                }
                Err(e) => app.command_failed(&e),
            }
        }
        V2Effect::CopyClipboard(text) => {
            use std::io::Write;
            let seq = teamagents_tui::overlays::osc52_sequence(&text);
            let mut out = std::io::stdout();
            let _ = out.write_all(seq.as_bytes());
            let _ = out.flush();
            app.flash("Copied code block to clipboard (OSC 52).");
        }
        V2Effect::RewindList { before } => match client.call("rewind_points", json!({"before": before})) {
            Ok(r) => {
                if let Some(modal) = app.rewind_modal.as_mut() {
                    if let Some(follow) = modal.apply_points(&r, before.is_some()) {
                        if let Some(effect) = app.apply_rewind_action(follow) {
                            run_v2_effect(effect, client, app, sessions);
                        }
                    }
                } else {
                    app.command_note(format!(
                        "Applicable Leader prompts (newest first): {}. Use /rewind preview KEY; older points: /rewind before {}",
                        r["points"], r["next_before"]
                    ));
                }
            }
            Err(e) => {
                if let Some(modal) = app.rewind_modal.as_mut() {
                    modal.fail(e);
                } else {
                    app.command_failed(&e);
                }
            }
        },
        V2Effect::RewindFiles { key } => match client.call("rewind_points", json!({"envelope": key, "files": true})) {
            Ok(report) => {
                if let Some(modal) = app.rewind_modal.as_mut() {
                    modal.apply_preview(report);
                } else if report["scope"] == "git_tracked_workspace" {
                    let mut lines = vec![format!(
                        "Restore Git-tracked workspace files to their contents when {key} was delivered, and reopen that prompt."
                    )];
                    for file in report["files"].as_array().into_iter().flatten() {
                        lines.push(format!("{}  {}", file["status"].as_str().unwrap_or("unavailable"), file["path"]));
                    }
                    lines.push(
                        "snapshot_difference includes edits by shells or external editors. Review every path.".into(),
                    );
                    if report["apply_available"] == true {
                        lines.push("Confirm with /rewind apply.".into());
                        app.rewind_preview = Some(report);
                    } else {
                        lines.push("Conflicts prevent restoration.".into());
                    }
                    app.command_note(lines.join("\n"));
                } else if report["scope"] == "recorded_leader_edits" {
                    let mut lines = vec![format!("Recorded Leader edits since {key} (read-only preview)")];
                    if let Some(files) = report["files"].as_array() {
                        for file in files {
                            lines.push(format!(
                                "{}  {}",
                                file["status"].as_str().unwrap_or("unavailable"),
                                file["path"]
                            ));
                        }
                    }
                    lines.push(format!("Workspace snapshot unavailable: {}", report["snapshot_error"]));
                    lines.push(format!(
                        "Other successful file operations without edit records: {}",
                        report["untracked_file_operations"]
                    ));
                    app.command_note(lines.join("\n"));
                } else {
                    app.command_failed("This daemon does not support file previews.");
                }
            }
            Err(e) => {
                if let Some(modal) = app.rewind_modal.as_mut() {
                    modal.fail(e);
                } else {
                    app.command_failed(&e);
                }
            }
        },
        V2Effect::RewindPreview { key } => match client.call("rewind_points", json!({"envelope": key})) {
            Ok(p) => {
                if let Some(modal) = app.rewind_modal.as_mut() {
                    modal.apply_preview(p);
                } else {
                    app.command_note(format!(
                        "Conversation only. Keep the prefix before {} and reopen its prompt. Files, work and external effects are unchanged. Old history stays readable. Confirm with /rewind apply. Prompt: {}",
                        key, p["draft"]["text"]
                    ));
                    app.rewind_preview = Some(p);
                }
            }
            Err(e) => {
                if let Some(modal) = app.rewind_modal.as_mut() {
                    modal.fail(e);
                } else {
                    app.command_failed(&e);
                }
            }
        },
        V2Effect::RewindApply { preview } => {
            let mut params = json!({"mode":preview["mode"],"envelope":preview["envelope"],"epoch":preview["epoch"],"revision":preview["revision"],"head":preview["head"]});
            if let Some(digest) = preview.get("preview_digest") {
                params["preview_digest"] = digest.clone();
            }
            match client.command(&format!("rewind-{}", uuid::Uuid::new_v4()), "rewind_conversation", params) {
                Ok(r) => {
                    app.rewind_modal = None;
                    app.open_rewind_draft(&r);
                    v2_sync_checkpoint(client, app);
                    v2_refresh_history(client, app);
                }
                Err(e) => {
                    if let Some(modal) = app.rewind_modal.as_mut() {
                        modal.fail(e);
                    } else {
                        app.command_failed(&e);
                    }
                }
            }
        }
        V2Effect::RewindDraft => match client.call("rewind_points", json!({"draft": true})) {
            Ok(r) => app.open_rewind_draft(&r),
            Err(e) => app.command_failed(&e),
        },
        V2Effect::RewindSend { source, envelope, text } => match client.command(
            &format!("input-{envelope}"),
            "send_rewind_draft",
            json!({"source": source, "envelope_id": envelope, "text": text}),
        ) {
            Ok(r) => {
                app.attachments.accepted();
                app.input_receipt(&envelope, &r);
            }
            Err(e) => {
                app.composer.set_text(&text);
                app.submit_failed(&e);
            }
        },
        V2Effect::McpStatus => match client.call("checkpoint", json!({})) {
            Ok(reply) => {
                let s = &reply["snapshot"];
                app.mcp_snapshot = s["mcp"].clone();
                app.mcp_policy = s["mcp_policy"].clone();
                app.mcp_drivers = s["mcp_drivers"].clone();
                if let Some(modal) = app.mcp_modal.as_mut() {
                    modal.refresh_from(&app.mcp_snapshot, &app.mcp_policy, &app.mcp_drivers);
                } else if std::mem::take(&mut app.pending_mcp_modal) {
                    app.open_mcp_modal();
                } else {
                    app.command_note(format!(
                        "Configured MCP bindings: {}\nDesired session policy: {}\nLast observed by driver: {}",
                        s["mcp"], s["mcp_policy"], s["mcp_drivers"]
                    ));
                }
            }
            Err(e) => {
                app.pending_mcp_modal = false;
                app.command_failed(&e);
            }
        },
        V2Effect::McpChange { action, service } => match client.command(
            &format!("mcp-{}", uuid::Uuid::new_v4()),
            "set_mcp_service",
            json!({"action": action, "service": service}),
        ) {
            Ok(reply) => {
                if app.mcp_modal.is_some() {
                    if let Some(modal) = app.mcp_modal.as_mut() {
                        modal.note = Some(format!("Requested {action} on {service}: {reply}"));
                    }
                    v2_sync_checkpoint(client, app);
                } else {
                    app.command_note(format!(
                        "MCP desired state: {reply}. Use /mcp status to check driver observations."
                    ))
                }
            }
            Err(e) => {
                if let Some(modal) = app.mcp_modal.as_mut() {
                    modal.note = Some(format!("MCP action failed: {e}"));
                } else {
                    app.command_failed(&e);
                }
            }
        },
        V2Effect::Compact { instance, focus } => {
            match client.command(
                &format!("compact-{}", uuid::Uuid::new_v4()),
                "request_compaction",
                json!({"instance_id":instance,"focus":focus}),
            ) {
                Ok(_) => app.command_note(
                    "Compaction requested, not yet completed. The most recent input and its results stay verbatim.",
                ),
                Err(e) => app.command_failed(&e),
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
        V2Effect::SearchTree => match client.call("search_tree", json!({})) {
            Ok(reply) => {
                app.team.apply_tree(&reply);
                if app.team.view != teamagents_tui::team::View::Tree {
                    app.command_note(reply["text"].as_str().unwrap_or("Session search tree is unavailable"));
                }
            }
            Err(error) => {
                app.team.fail_tree(error.clone());
                if app.team.view != teamagents_tui::team::View::Tree {
                    app.command_failed(&error);
                }
            }
        },
        V2Effect::SetRoleProfile { role, model, effort, follow } => {
            let mut params = json!({"role": match role {
                teamagents_tui::v2app::ProfileRole::Leader => "leader",
                teamagents_tui::v2app::ProfileRole::Workers => "workers",
            }});
            if follow {
                params["follow"] = json!(true);
            }
            if let Some(model) = model {
                params["model"] = json!(model);
            }
            if let Some(effort) = effort {
                params["effort"] = json!(effort);
            }
            match client.command(&format!("profile-{}", uuid::Uuid::new_v4()), "set_role_model", params) {
                Ok(reply) => {
                    // The composer border already shows the Leader's model and effort, and the checkpoint
                    // refresh below updates it. A Leader switch therefore needs no transcript note: it
                    // repeated the same label after the input box on every switch. Worker policy is not
                    // rendered in the border, so its confirmation stays.
                    let message = match role {
                        teamagents_tui::v2app::ProfileRole::Leader => None,
                        teamagents_tui::v2app::ProfileRole::Workers if reply["workers"]["mode"] == "follow" => {
                            Some("Workers will use the same model as your chat.".to_string())
                        }
                        teamagents_tui::v2app::ProfileRole::Workers => Some(
                            reply["workers"]["key"]
                                .as_str()
                                .map(|key| format!("Worker model set to {}.", app.model_label(key)))
                                .unwrap_or_else(|| "Model settings saved.".into()),
                        ),
                    };
                    if let Some(message) = message {
                        app.command_note(message);
                    }
                    v2_sync_checkpoint(client, app);
                }
                Err(error) => app.command_note(format!("Could not change the model: {error}")),
            }
        }
        V2Effect::EditDraft { .. } | V2Effect::OperatorShell { .. } | V2Effect::CodeGraph { .. } | V2Effect::Quit => {}
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
    let live_monitor = teamagents_tui::live_sessions::LiveMonitor::new();
    let mut images = teamagents_tui::graphics::KittyOutput::default();
    v2_sync_checkpoint(&mut client, &mut app);
    let mut last_siblings = app.sibling_sessions.clone();
    live_monitor.set_targets(&last_siblings);
    v2_refresh_history(&mut client, &mut app);
    v2_refresh_approvals(&mut client, &mut app);
    let mut last_event_poll = Instant::now();
    let mut last_slow = Instant::now();
    let mut last_animation = Instant::now();
    let mut dirty = true;
    let mut active_id = app.active_instance().map(|i| i.id.clone());
    // The connection indicator is part of the status line, so a change in it must rebuild the frame even
    // when nothing else happened: a reconnect that delivers no events used to leave a stale
    // "disconnected, reconnecting..." on screen while the client was in fact connected again (D-99).
    let mut last_disconnected = app.disconnected;
    loop {
        dirty |= sessions.poll(&mut client, &mut app);
        if app.sibling_sessions != last_siblings {
            last_siblings = app.sibling_sessions.clone();
            live_monitor.set_targets(&last_siblings);
        }
        dirty |= live_monitor.poll(&mut app);
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
                            local @ (V2Effect::OperatorShell { .. } | V2Effect::CodeGraph { .. }) => {
                                let _ = images.clear(&mut std::io::stdout());
                                restore_terminal();
                                use teamagents_tui::operator_shell;
                                let (label, draft, result) = match local {
                                    V2Effect::OperatorShell { workspace, command, draft } => (
                                        "Local shell",
                                        draft,
                                        operator_shell::run(workspace.as_ref(), command.as_deref()),
                                    ),
                                    V2Effect::CodeGraph { workspace, action } => {
                                        ("CodeGraph", false, operator_shell::codegraph(workspace.as_ref(), action))
                                    }
                                    _ => unreachable!(),
                                };
                                enter_terminal();
                                let _ = terminal.clear();
                                match result {
                                    Ok(out) => {
                                        let limit = if out.limited { " (output limited)" } else { "" };
                                        if draft && !out.text.is_empty() {
                                            // Prefix as plain text: stdout beginning with a slash must never execute as a local command.
                                            app.composer.set_text(&format!("Shell output:\n{}", out.text));
                                            app.command_note(format!(
                                                "Shell {}{limit}. Review the draft before sending; nothing sent.",
                                                out.status
                                            ));
                                        } else {
                                            app.command_note(format!(
                                                "{label} {}{limit}; not sent.\n{}",
                                                out.status, out.text
                                            ));
                                        }
                                    }
                                    Err(error) => app.command_failed(&format!("{label}: {error}; nothing sent")),
                                }
                            }
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
                    } else if let Some(effect) = v2ui::handle_mouse(&mut app, size, m) {
                        run_v2_effect(effect, &mut client, &mut app, &mut sessions);
                    }
                    dirty = true;
                }
                Ok(Event::Resize(_, _)) => {
                    app.graphics.resize();
                    dirty = true;
                }
                Ok(Event::Paste(_))
                    if app.help_open
                        || app.confirm.is_some()
                        || app.approval_detail.is_some()
                        || app.rewind_modal.is_some()
                        || app.mcp_modal.is_some() => {}
                Ok(Event::Paste(text)) if app.palette.is_some() => {
                    if let Some(palette) = &mut app.palette {
                        palette.paste(&text);
                    }
                    dirty = true;
                }
                Ok(Event::Paste(text))
                    if app.team.view == teamagents_tui::team::View::Conversation
                        && app.chat_search.as_ref().is_some_and(|s| s.editing) =>
                {
                    if let Some(search) = &mut app.chat_search {
                        for c in text.chars() {
                            if !c.is_control() {
                                search.query.push(c);
                            }
                        }
                    }
                    app.update_chat_search();
                    dirty = true;
                }
                Ok(Event::Paste(text)) if app.profile.is_some() => {
                    app.paste_model_search(&text);
                    dirty = true;
                }
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
        dirty |= v2_refresh_observations(&mut client, &mut app, false);
        if app.disconnected != last_disconnected {
            last_disconnected = app.disconnected;
            dirty = true;
        }
        if last_event_poll.elapsed() >= Duration::from_millis(150) {
            last_event_poll = Instant::now();
            // §9: the model's streaming preview is a transient read, not an event. Repaint only when it changes.
            // The comparison happens inside `apply_previews`: cloning the whole preview set here copied every
            // streamed character four times a second just to decide whether to repaint.
            if let Ok(result) = client.call("previews", serde_json::json!({})) {
                if app.apply_previews(&result["previews"]) {
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
        if last_animation.elapsed() >= Duration::from_millis(125) {
            last_animation = Instant::now();
            // Activity animation is repaint-only and never manufactures a runtime fact.
            // Idle Ready sessions keep the previous low-frequency event/redraw schedule.
            if app.disconnected || !app.runtime_status_line().starts_with("Ready") || app.team.has_busy_member() {
                dirty = true;
            }
        }
        if last_slow.elapsed() >= Duration::from_secs(1) {
            last_slow = Instant::now();
            dirty |= v2_refresh_observations(&mut client, &mut app, true);
            if app.disconnected || app.history_dirty {
                // Refresh debt survives successful checkpoint/event polls after a history read failed.
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
