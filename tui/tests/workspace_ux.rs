//! Integration tests covering D-532 interactive workspace UX features.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use serde_json::json;
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::thread;
use teamagents_tui::{
    live_sessions::{LiveMonitor, LiveSession},
    overlays::{osc52_sequence, ConfirmAction, RewindAction},
    sessions::SessionAction,
    team::View,
    transcript::{assistant, Transcript},
    v2app::{ChatEntry, ChatKind, Focus, ModelInfo, ToolInfo, V2App, V2Effect},
    v2ui,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn alt(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
}

fn repeat(code: KeyCode) -> KeyEvent {
    let mut k = KeyEvent::new(code, KeyModifiers::NONE);
    k.kind = KeyEventKind::Repeat;
    k
}

fn base_app() -> V2App {
    let mut app = V2App::new("sess-ux");
    app.session_name = "main-work".into();
    app.models = vec![ModelInfo {
        key: "leader_main".into(),
        model: "claude-sonnet".into(),
        provider: "anthropic".into(),
        efforts: vec!["medium".into(), "high".into()],
        context_window: Some(200_000),
    }];
    app.apply_checkpoint(
        json!({
            "environment": {"workspace": "/tmp/ws"},
            "instances": [
                {"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY", "model": "leader_main", "effort": "high"},
                {"id": "w-worker", "lifecycle": "ACTIVE", "phase": "MODEL_PENDING", "model": "leader_main", "effort": "medium"}
            ],
            "tasks": [
                {"id": "task-1", "status": "RUNNING", "assignee": "w-worker", "title": "Implement feature"}
            ],
            "work": {
                "status": "ACTIVE",
                "known_usage": {"total": 50_000},
                "unknown_usage": 0,
                "limits": {"max_total_tokens": 100_000},
                "cost": {"currency": "USD", "known_estimated_usd": 0.12, "attempts": 3, "complete": true}
            },
            "mcp": [{"name": "graph", "transport": "stdio", "required": false, "oauth": false}],
            "mcp_policy": {"services": {"graph": {"running": true, "exposed": true}}},
            "mcp_drivers": {"graph": {"status": "connected"}}
        }),
        10,
    );
    app
}

fn buffer_text(terminal: &Terminal<TestBackend>, width: u16, height: u16) -> String {
    let buf = terminal.backend().buffer();
    (0..height).map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

#[test]
fn team_interventions_require_confirmation_and_block_repeat() {
    let mut app = base_app();
    app.focus = Focus::Team;
    app.team.select("w-worker");

    // Pressing p opens explicit confirmation without executing anything yet.
    assert!(app.handle_key(key(KeyCode::Char('p'))).is_none());
    assert_eq!(app.confirm, Some(ConfirmAction::PauseMember("w-worker".into())));

    // Repeat Enter/y cannot confirm destructive or lifecycle actions.
    assert!(app.handle_key(repeat(KeyCode::Char('y'))).is_none());
    assert!(app.handle_key(repeat(KeyCode::Enter)).is_none());
    assert!(app.confirm.is_some());

    // Pressing 'y' confirms SetLifecycle.
    let eff = app.handle_key(key(KeyCode::Char('y')));
    assert_eq!(
        eff,
        Some(V2Effect::SetLifecycle {
            instance: "w-worker".into(),
            lifecycle: "PAUSED",
            reason: "PAUSED by the user (TUI)".into(),
        })
    );
    assert!(app.confirm.is_none());

    // Terminate and Task cancel flows require confirmation too.
    assert!(app.handle_key(key(KeyCode::Char('X'))).is_none());
    assert_eq!(app.confirm, Some(ConfirmAction::TerminateMember("w-worker".into())));
    assert!(app.handle_key(key(KeyCode::Esc)).is_none());
    assert!(app.confirm.is_none());

    app.handle_key(alt('2'));
    assert_eq!(app.team.view, View::Tasks);
    app.team.selected_task = 0;
    assert!(app.handle_key(key(KeyCode::Char('x'))).is_none());
    assert_eq!(app.confirm, Some(ConfirmAction::CancelTask("task-1".into())));
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(V2Effect::CancelTask { task_id: "task-1".into(), reason: "cancelled by the user (TUI)".into() })
    );
}

#[test]
fn direct_message_targets_selected_worker_once_and_resets_to_leader() {
    let mut app = base_app();
    app.focus = Focus::Team;
    app.team.select("w-worker");
    app.handle_key(key(KeyCode::Char('m')));
    assert_eq!(app.direct_target.as_deref(), Some("w-worker"));
    assert_eq!(app.focus, Focus::Composer);

    let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    assert!(buffer_text(&terminal, 140, 30).contains("Direct to w-worker · Esc back to Leader"));

    app.composer.set_text("check test.rs");
    let eff = app.handle_key(key(KeyCode::Enter));
    assert!(matches!(
        eff,
        Some(V2Effect::SubmitInput { instance, text, .. }) if instance == "w-worker" && text == "check test.rs"
    ));
    assert!(app.direct_target.is_none(), "single-shot direct target resets after submit");

    app.composer.set_text("for leader");
    let eff2 = app.handle_key(key(KeyCode::Enter));
    assert!(matches!(eff2, Some(V2Effect::SubmitInput { instance, .. }) if instance == "i-leader"));

    app.focus = Focus::Team;
    app.handle_key(key(KeyCode::Char('m')));
    assert!(app.direct_target.is_some());
    app.handle_key(key(KeyCode::Esc));
    assert!(app.direct_target.is_none());
}

#[test]
fn ctrl_k_palette_filters_and_dispatches_views_and_modals() {
    let mut app = base_app();
    app.handle_key(ctrl('k'));
    assert!(app.palette.is_some());

    for c in "wall".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert!(app.palette.is_none());
    assert_eq!(app.team.view, View::Wall);
    assert_eq!(app.focus, Focus::Panel);

    app.handle_key(ctrl('k'));
    for c in "rewind".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    let eff = app.handle_key(key(KeyCode::Enter));
    assert_eq!(eff, Some(V2Effect::RewindList { before: None }));
    assert!(app.rewind_modal.is_some());
    app.handle_key(key(KeyCode::Esc));
    assert!(app.rewind_modal.is_none());
}

#[test]
fn rewind_and_mcp_modals_drive_interactive_flows() {
    let mut app = base_app();
    app.composer.set_text("/rewind");
    assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::RewindList { before: None }));
    let follow = app
        .rewind_modal
        .as_mut()
        .unwrap()
        .apply_points(&json!({"points": [{"envelope": "env-1", "preview": "Initial task"}]}), false);
    assert_eq!(follow, Some(RewindAction::Preview { key: "env-1".into(), files: false }));
    app.rewind_modal.as_mut().unwrap().apply_preview(json!({
        "mode": "conversation",
        "envelope": "env-1",
        "epoch": 1,
        "revision": 2,
        "head": "h1",
        "draft": {"text": "Initial task"}
    }));
    // First Enter enters confirm step; second non-repeat Enter emits RewindApply.
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert!(app.rewind_modal.as_ref().unwrap().confirming);
    assert!(app.handle_key(repeat(KeyCode::Enter)).is_none());
    assert!(matches!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::RewindApply { .. })));

    app.rewind_modal = None;
    app.open_mcp_modal();
    assert!(app.mcp_modal.is_some());
    assert_eq!(
        app.handle_key(key(KeyCode::Char('s'))),
        Some(V2Effect::McpChange { action: "stop".into(), service: "graph".into() })
    );
    assert_eq!(
        app.handle_key(key(KeyCode::Char('h'))),
        Some(V2Effect::McpChange { action: "hide".into(), service: "graph".into() })
    );
    assert_eq!(
        app.handle_key(key(KeyCode::Char('r'))),
        Some(V2Effect::McpChange { action: "reconnect".into(), service: "graph".into() })
    );
    assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::McpStatus));
}

#[test]
fn approval_detail_drawer_and_foldable_tree_view() {
    let mut app = base_app();
    app.apply_approvals(json!({
        "approvals": [{
            "id": "ap-42",
            "operation_id": "op-42",
            "instance_id": "w-worker",
            "tool": "shell",
            "preview": "cargo check",
            "args": "{\n  \"cmd\": \"cargo check\"\n}"
        }]
    }));
    app.handle_key(ctrl('a'));
    assert_eq!(app.focus, Focus::Approvals);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.approval_detail.as_ref().map(|d| d.approval_id.as_str()), Some("ap-42"));

    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let rendered = buffer_text(&terminal, 120, 30);
    assert!(rendered.contains("op-42") && rendered.contains("cargo check"));
    assert_eq!(
        app.handle_key(key(KeyCode::Char('a'))),
        Some(V2Effect::Decide { approval_id: "ap-42".into(), decision: "approve" })
    );
    assert!(app.approval_detail.is_none());

    app.handle_key(alt('5'));
    assert_eq!(app.team.view, View::Tree);
    app.team.apply_tree(&json!({
        "revision": 3,
        "focus": "n2",
        "nodes": [
            {"id": "n1", "parent_id": null, "title": "Root analysis", "status": "active", "kind": "question"},
            {"id": "n2", "parent_id": "n1", "title": "Verify socket handling", "status": "open", "kind": "attempt"}
        ]
    }));
    assert_eq!(app.team.visible_tree_nodes().len(), 2);
    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(app.team.visible_tree_nodes().len(), 1);
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.team.visible_tree_nodes().len(), 2);
    app.team.tree_selected = 1;
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Composer);
    assert!(app.composer.text().starts_with("Focus on tree node n2 (Verify socket handling):"));
}

#[test]
fn wall_view_grid_and_pinned_right_inspector() {
    let mut app = base_app();
    app.handle_key(alt('6'));
    assert_eq!(app.team.view, View::Wall);
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let screen = buffer_text(&terminal, 120, 30);
    assert!(screen.contains("w-worker"));

    // Select worker 2 via number shortcut, pin right inspector, then unpin.
    app.handle_key(key(KeyCode::Char('2')));
    assert_eq!(app.team.selected, "w-worker");
    app.handle_key(key(KeyCode::Char('P')));
    assert_eq!(app.team.pinned.as_deref(), Some("w-worker"));
    app.team.select("i-leader");
    assert_eq!(app.team.inspected_id(true), "w-worker");
    app.team.select("w-worker");
    app.handle_key(key(KeyCode::Char('P')));
    assert!(app.team.pinned.is_none());
}

#[test]
fn transcript_diff_stats_search_osc52_and_status_bar() {
    let mut app = base_app();
    app.entries = vec![
        ChatEntry {
            kind: ChatKind::Tool,
            who: "i-leader".into(),
            text: "".into(),
            tool: Some(ToolInfo {
                name: "edit_file".into(),
                arguments: json!({"path":"src/lib.rs","old_text":"a\nb","new_text":"a\nc\nd"}).to_string(),
                output: Some("updated src/lib.rs".into()),
            }),
            workspace: "/tmp/ws".into(),
            media: vec![],
        },
        ChatEntry {
            kind: ChatKind::Assistant,
            who: "i-leader".into(),
            text: "Try running:\n```rust\nlet token_ok = true;\n```".into(),
            tool: None,
            workspace: "/tmp/ws".into(),
            media: vec![],
        },
    ];
    let mut tr = Transcript::default();
    let expanded = HashSet::new();
    let doc = tr.document_with_expanded(&app.entries, 80, Some(4), &expanded, None, "");
    let rendered = doc.lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
    assert!(rendered.contains("+2 -1"), "missing diff stats: {rendered}");
    assert!(!tr.code_blocks.is_empty());

    app.flash("Copied code block to clipboard (OSC 52).");
    assert!(!app.runtime_status_line().contains("Copied"), "notice leaked into top bar");
    assert!(app.activity_line().contains("Copied"));

    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let row0: String = (0..120).map(|x| terminal.backend().buffer()[(x, 0)].symbol()).collect();
    assert!(row0.contains("$0.12"), "expected cost USD on status row: {row0}");
    assert!(row0.contains("50%"), "expected budget bar on status row: {row0}");

    // Search Ctrl+F finds code block entry and navigates matches.
    app.handle_key(ctrl('f'));
    for c in "token_ok".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.chat_search.as_ref().map(|s| s.matches.len()), Some(1));
    app.handle_key(key(KeyCode::Enter));
    assert!(!app.chat_search.as_ref().unwrap().editing);
    app.handle_key(key(KeyCode::Esc));
    assert!(app.chat_search.is_none());

    // OSC 52 sequence is standard base64 wrapped.
    assert_eq!(osc52_sequence("hi"), "\x1b]52;c;aGk=\x07");
    let _ = assistant("```rust\nlet x = 1;\n```", "i-leader", 60, false);
}

#[test]
fn live_sessions_sidebar_click_opens_session_and_monitor_probes_socket() {
    let dir = std::env::temp_dir().join(format!("teamagents-live-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("daemon.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    let root_for_server = dir.to_string_lossy().into_owned();
    let server = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let greeting = json!({
                "server": "teamagents-daemon",
                "protocol_version": teamagents_tui::wire::VERSION,
                "session_id": "sib-1",
                "state_root": root_for_server,
            });
            let _ = writeln!(stream, "{greeting}");
            let _ = stream.flush();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            for _ in 0..2 {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let req: serde_json::Value = serde_json::from_str(&line).unwrap();
                let resp = match req["method"].as_str() {
                    Some("checkpoint") => json!({
                        "protocol_version": teamagents_tui::wire::VERSION,
                        "request_id": req["request_id"],
                        "ok": true,
                        "result": {
                            "snapshot": {
                                "instances": [
                                    {"id": "i-leader", "lifecycle": "ACTIVE", "phase": "MODEL_PENDING", "diagnosis": {"attention": true}}
                                ]
                            },
                            "watermark": 1
                        }
                    }),
                    _ => json!({
                        "protocol_version": teamagents_tui::wire::VERSION,
                        "request_id": req["request_id"],
                        "ok": true,
                        "result": {
                            "approvals": [{"id": "ap-1"}]
                        }
                    }),
                };
                let _ = writeln!(stream, "{resp}");
                let _ = stream.flush();
            }
        }
    });

    let monitor = LiveMonitor::new();
    monitor.set_targets(&[("sib-1".into(), "worker-box".into(), dir.to_string_lossy().into_owned())]);
    let mut app = base_app();
    for _ in 0..40 {
        if monitor.poll(&mut app) && app.live_sessions.first().is_some_and(|s| s.live) {
            break;
        }
        thread::sleep(std::time::Duration::from_millis(25));
    }
    let _ = server.join();
    drop(monitor);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        app.live_sessions.first(),
        Some(&LiveSession {
            id: "sib-1".into(),
            name: "worker-box".into(),
            root: dir.to_string_lossy().into_owned(),
            live: true,
            attention: 1,
            approvals: 1,
            busy: 1,
        })
    );

    let area = Rect::new(0, 0, 120, 30);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let geo = v2ui::geometry(&app, area);
    let sess_rect = teamagents_tui::team::live_sessions_window(&app, geo.sidebar).expect("LIVE SESSIONS strip");
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: sess_rect.x + 2,
        row: sess_rect.y + 1,
        modifiers: KeyModifiers::NONE,
    };
    let eff = v2ui::handle_mouse(&mut app, area, click);
    assert!(matches!(eff, Some(V2Effect::Sessions(SessionAction::Open(row))) if row.id == "sib-1"));
}
