//! Integration tests covering D-532 interactive workspace UX features.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{backend::TestBackend, layout::Rect, style::Modifier, Terminal};
use serde_json::json;
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::thread;
use teamagents_tui::{
    live_sessions::{LiveMonitor, LiveSession},
    overlays::{osc52_sequence, ConfirmAction, RewindAction, RewindModal},
    queue::{Action as QueueAction, Queue},
    sessions::{SessionAction, SessionBrowser},
    team::View,
    transcript::{assistant, Transcript},
    v2app::{ApprovalKind, ChatEntry, ChatKind, Focus, InstanceInfo, ModelInfo, ToolInfo, V2App, V2Effect},
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
                "known_usage": {"total": 50_000, "cache_hit": 30_000, "cache_miss": 20_000},
                "unknown_usage": 0,
                "limits": {"max_total_tokens": 100_000}
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
    // Usage is shown as recorded tokens; cost and the usage-percentage gauge were removed by request.
    // The cache hit rate replaces them: 30k hit of 50k reported tokens is 60%.
    assert!(row0.contains("50k/100k tokens"), "expected recorded token usage on status row: {row0}");
    assert!(row0.contains("cache 60%"), "expected the cache hit rate on the status row: {row0}");
    assert!(!row0.contains('$'), "no cost estimate on the status row: {row0}");

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

#[test]
fn deduplicated_chrome_keeps_single_source_per_signal() {
    let mut app = base_app();
    app.apply_checkpoint(
        json!({
            "environment": {"workspace": "/tmp/ws"},
            "instances": [
                {"id": "i-leader", "lifecycle": "ACTIVE", "phase": "MODEL_PENDING", "model": "leader_main", "effort": "high"},
                {"id": "w-worker", "lifecycle": "ACTIVE", "phase": "READY", "model": "leader_main", "effort": "medium"}
            ],
            "work": {
                "status": "ACTIVE",
                "known_usage": {"total": 50_000, "cache_read": 30_000, "uncached_input": 20_000},
                "limits": {"max_total_tokens": 100_000}
            }
        }),
        11,
    );
    app.team.phase_since.insert("i-leader".into(), ("ACTIVE:MODEL_PENDING".into(), teamagents_tui::now_epoch() - 5.0));
    app.team.select("w-worker");
    app.zoomed = true;

    let mut terminal = Terminal::new(TestBackend::new(140, 32)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let row0: String = (0..140).map(|x| terminal.backend().buffer()[(x, 0)].symbol()).collect();

    assert!(row0.contains("main-work") && row0.contains("50k/100k tokens"), "{row0}");
    assert!(
        !row0.contains("Thinking") && !row0.contains("[ZOOM]") && !row0.contains("Ready"),
        "top row should hold only session identity and usage: {row0}"
    );

    app.zoomed = false;
    app.team.selected = "w-worker".into();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let geo = v2ui::geometry(&app, Rect::new(0, 0, 140, 32));
    let full = buffer_text(&terminal, 140, 32);
    let act_row: String = (0..140).map(|x| terminal.backend().buffer()[(x, geo.activity.y)].symbol()).collect();
    assert!(act_row.contains("Thinking") && act_row.contains("5s"), "{act_row}");
    assert!(!act_row.contains("Ctrl+C interrupt"), "shortcut belongs only in footer: {act_row}");
    assert!(!full.contains("Leader · conversation"));
    assert!(!full.contains("open tasks") && !full.contains("Token limit"));
    assert!(!full.contains("Message Leader · viewing"));
    assert!(full.contains("Message Leader"));
    assert!(!full.contains("d details / h history"));
    assert!(!full.contains("Read-only member history"));
}

fn mouse_click(column: u16, row: u16) -> MouseEvent {
    MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column, row, modifiers: KeyModifiers::NONE }
}

fn mouse_scroll(up: bool, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: if up { MouseEventKind::ScrollUp } else { MouseEventKind::ScrollDown },
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn capability_requests_unify_with_ctrl_a_detail_and_exact_button_rects() {
    let mut app = base_app();
    // Capability events trigger both approvals and checkpoint refreshes.
    for kind in ["capability_requested", "capability_approved", "capability_denied"] {
        let refresh = app.apply_events(&[json!({"kind": kind, "payload": {}})]);
        assert!(refresh.approvals && refresh.checkpoint, "{kind}");
    }

    // Fallback status_line when WAITING_CAPABILITY is observed without loaded requests.
    app.instances.push(InstanceInfo {
        id: "w-cap".into(),
        lifecycle: "ACTIVE".into(),
        phase: "WAITING_CAPABILITY".into(),
        model: String::new(),
        reason: String::new(),
        effort: String::new(),
    });
    assert!(app.status_line().contains("waiting for capability approval"));
    assert!(app.status_line().contains("Ctrl+A"));

    // Load both an operation approval and a capability request (even with colliding id).
    app.apply_approvals(json!({
        "approvals": [{
            "id": "shared-1",
            "operation_id": "op-1",
            "instance_id": "i-leader",
            "tool": "shell",
            "preview": "ls -la",
            "args": "{\"command\":\"ls -la\"}"
        }],
        "capability_requests": [{
            "id": "shared-1",
            "requester": "i-leader",
            "subject": "w-worker",
            "action": "shell",
            "resource_scope": "workspace",
            "task_id": "task-1",
            "parent_grant_id": "g-parent",
            "status": "PENDING"
        }]
    }));
    assert_eq!(app.approvals.len(), 2);
    assert_eq!(app.approvals[0].kind, ApprovalKind::Operation);
    assert_eq!(app.approvals[1].kind, ApprovalKind::Capability);

    app.handle_key(ctrl('a'));
    assert_eq!(app.focus, Focus::Approvals);
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.approval_sel, 1);

    // Refresh preserves selection by (kind, id) even when operation approval is removed.
    app.apply_approvals(json!({
        "approvals": [],
        "capability_requests": [{
            "id": "shared-1",
            "requester": "i-leader",
            "subject": "w-worker",
            "action": "shell",
            "resource_scope": "workspace",
            "task_id": "task-1",
            "parent_grant_id": "g-parent",
            "status": "PENDING"
        }]
    }));
    assert_eq!(app.approval_sel, 0);
    assert_eq!(app.focus, Focus::Approvals);

    let area = Rect::new(0, 0, 120, 30);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let box_text = buffer_text(&terminal, area.width, area.height);
    assert!(box_text.contains("shared-1 · capability · shell@workspace -> w-worker (task-1)"), "{box_text}");

    // Open ApprovalDetail drawer and verify capability fields + button hit-testing.
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.approval_detail.as_ref().map(|d| (d.kind, d.approval_id.as_str())),
        Some((ApprovalKind::Capability, "shared-1"))
    );
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let drawer = buffer_text(&terminal, area.width, area.height);
    assert!(drawer.contains("Capability request details") && drawer.contains("shell@workspace"));
    assert!(drawer.contains("g-parent") && drawer.contains("[a Approve grant]"));

    // Footer is at inner bottom (x=18, y=24): "[a Approve grant] · [d Deny] · Esc back · Up/Down scroll".
    // Clicking separators (col 35..38, 46..49) or scroll hint (col 60) must NOT decide or close.
    for col in [35, 36, 37, 46, 47, 48, 60] {
        assert!(v2ui::handle_mouse(&mut app, area, mouse_click(col, 24)).is_none(), "col {col}");
        assert!(app.approval_detail.is_some(), "col {col} closed drawer");
    }
    // Clicking "[a Approve grant]" at col 20 emits DecideCapability approve.
    assert_eq!(
        v2ui::handle_mouse(&mut app, area, mouse_click(20, 24)),
        Some(V2Effect::DecideCapability { request_id: "shared-1".into(), decision: "approve" })
    );
    assert!(app.approval_detail.is_none());

    // Direct 'd' in approvals list and inside drawer emits DecideCapability deny.
    assert_eq!(
        app.handle_key(key(KeyCode::Char('d'))),
        Some(V2Effect::DecideCapability { request_id: "shared-1".into(), decision: "deny" })
    );
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.handle_key(key(KeyCode::Char('d'))),
        Some(V2Effect::DecideCapability { request_id: "shared-1".into(), decision: "deny" })
    );

    // ConfirmAction mouse clicks: separator (col 38) and trailing space (col 55) do not trigger.
    app.confirm = Some(ConfirmAction::OpenNewWork);
    assert!(v2ui::handle_mouse(&mut app, area, mouse_click(38, 17)).is_none());
    assert!(app.confirm.is_some());
    assert!(v2ui::handle_mouse(&mut app, area, mouse_click(55, 17)).is_none());
    assert!(app.confirm.is_some());
    assert!(v2ui::handle_mouse(&mut app, area, mouse_click(42, 17)).is_none());
    assert!(app.confirm.is_none(), "clicking [Esc Cancel] closes confirmation");
}

#[test]
fn ctrl_f_search_scrolls_and_highlights_user_and_assistant_entries_with_ctrl_u_clear() {
    let mut app = base_app();
    for i in 0..16 {
        app.entries.push(ChatEntry {
            kind: ChatKind::User,
            who: "you".into(),
            text: if i == 0 { "user_target_alpha first question".into() } else { format!("prompt {i}") },
            tool: None,
            workspace: "/tmp/ws".into(),
            media: vec![],
        });
        app.entries.push(ChatEntry {
            kind: ChatKind::Assistant,
            who: "i-leader".into(),
            text: if i == 2 {
                "assistant_target_beta line\nmore detail".into()
            } else {
                format!("answer line {i}\nsecond line {i}")
            },
            tool: None,
            workspace: "/tmp/ws".into(),
            media: vec![],
        });
    }
    let area = Rect::new(0, 0, 120, 22);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    assert_eq!(app.chat_scroll, 0);
    assert!(app.transcript.entry_ranges.len() >= 32, "entry_ranges must include all chat entries");

    // Search for the off-screen top User entry.
    app.handle_key(ctrl('f'));
    for c in "user_target_alpha".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.chat_search.as_ref().map(|s| s.matches.clone()), Some(vec![0]));
    assert!(app.chat_scroll > 0, "search must scroll up to the User match");

    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let shown = buffer_text(&terminal, area.width, area.height);
    assert!(shown.contains("user_target_alpha"), "scrolled viewport must show matched user entry:\n{shown}");
    let buf = terminal.backend().buffer();
    let mut found_selected = false;
    for y in 0..area.height {
        let row: String = (0..area.width).map(|x| buf[(x, y)].symbol()).collect();
        if row.contains("user_target_alpha") && !row.contains("Find:") {
            let x = row.find("user_target_alpha").unwrap() as u16;
            assert!(buf[(x, y)].modifier.contains(Modifier::REVERSED | Modifier::BOLD));
            found_selected = true;
            break;
        }
    }
    assert!(found_selected, "current User match must use selection highlight");

    // Ctrl+U clears the search query and matches; then search Assistant entry.
    app.handle_key(ctrl('u'));
    assert_eq!(app.chat_search.as_ref().map(|s| s.query.as_str()), Some(""));
    assert!(app.chat_search.as_ref().unwrap().matches.is_empty());

    for c in "assistant_target_beta".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.chat_search.as_ref().map(|s| s.matches.clone()), Some(vec![5]));
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    assert!(buffer_text(&terminal, area.width, area.height).contains("assistant_target_beta"));
}

#[test]
fn rewind_modal_advertises_c_key_and_scrolls_points_list_with_wheel() {
    let mut app = base_app();
    let mut modal = RewindModal::loading();
    modal.apply_points(
        &json!({
            "points": [
                {"envelope": "env-1", "position": 1, "preview": "First"},
                {"envelope": "env-2", "position": 2, "preview": "Second"},
                {"envelope": "env-3", "position": 3, "preview": "Third"}
            ]
        }),
        false,
    );
    app.rewind_modal = Some(modal);
    let hint = app.footer_hint();
    assert!(hint.contains("c conversation") && !hint.contains("r chat preview"), "{hint}");

    let area = Rect::new(0, 0, 120, 30);
    let eff = v2ui::handle_mouse(&mut app, area, mouse_scroll(false, 20, 4));
    assert_eq!(eff, Some(V2Effect::RewindPreview { key: "env-2".into() }));
    assert_eq!(app.rewind_modal.as_ref().unwrap().selected, 1);

    let eff_up = v2ui::handle_mouse(&mut app, area, mouse_scroll(true, 20, 4));
    assert_eq!(eff_up, Some(V2Effect::RewindPreview { key: "env-1".into() }));
    assert_eq!(app.rewind_modal.as_ref().unwrap().selected, 0);
}

#[test]
fn palette_offers_work_lifecycle_and_approval_recovery_with_confirmations() {
    let mut app = base_app();
    app.apply_checkpoint(
        json!({
            "environment": {"workspace": "/tmp/ws"},
            "instances": [
                {"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY", "diagnosis": {"code": "work_blocked", "reason": "checks failed"}},
                {"id": "w-worker", "lifecycle": "PARKED", "phase": "READY", "reason": "quota exhausted"}
            ],
            "work": {"id": "work-dead", "status": "BLOCKED", "known_usage": {"total": 100}}
        }),
        12,
    );
    app.apply_approvals(json!({
        "capability_requests": [{"id": "cap-9", "requester": "i-leader", "subject": "w-worker", "action": "shell", "resource_scope": "workspace", "task_id": "t-1", "status": "PENDING"}]
    }));

    // 1. Start new work scope from Ctrl+K palette requires confirmation and blocks Repeat.
    app.handle_key(ctrl('k'));
    for c in "Start new work scope".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(app.confirm, Some(ConfirmAction::OpenNewWork));
    assert!(app.handle_key(repeat(KeyCode::Char('y'))).is_none());
    assert!(app.confirm.is_some());
    assert!(matches!(
        app.handle_key(key(KeyCode::Char('y'))),
        Some(V2Effect::CreateWork { instance, .. }) if instance == "i-leader"
    ));

    // 2. Resume PARKED member from Ctrl+K palette requires confirmation.
    app.handle_key(ctrl('k'));
    for c in "Resume w-worker".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(app.confirm, Some(ConfirmAction::ResumeMember("w-worker".into())));
    assert_eq!(
        app.handle_key(key(KeyCode::Char('y'))),
        Some(V2Effect::SetLifecycle {
            instance: "w-worker".into(),
            lifecycle: "ACTIVE",
            reason: "ACTIVE by the user (TUI)".into(),
        })
    );

    // 3. Pending capability request in Ctrl+K palette focuses Approvals and opens detail.
    app.handle_key(ctrl('k'));
    for c in "cap-9".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(app.focus, Focus::Approvals);
    assert_eq!(app.approval_detail.as_ref().map(|d| d.approval_id.as_str()), Some("cap-9"));
    app.handle_key(key(KeyCode::Esc));

    // 4. Cancel active work scope when work is ACTIVE with an id (and test UTF-8 label render).
    app.apply_checkpoint(
        json!({
            "environment": {"workspace": "/tmp/ws"},
            "instances": [{"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY"}],
            "work": {"id": "work-active-1", "status": "ACTIVE", "known_usage": {"total": 100}},
            "sessions": [{"id": "s-utf8", "name": "\u{5de5}\u{4f5c}\u{7a7a}\u{95f4}", "root": "/tmp/ws", "active": false}]
        }),
        13,
    );
    app.handle_key(ctrl('k'));
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    for c in "Cancel active work scope".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(app.confirm, Some(ConfirmAction::CancelWork("work-active-1".into())));
    assert_eq!(app.handle_key(key(KeyCode::Char('y'))), Some(V2Effect::CancelWork { work_id: "work-active-1".into() }));
}

#[test]
fn context_aware_composer_placeholders_work_blocked_reason_and_queue_count() {
    let mut app = base_app();
    let mut terminal = Terminal::new(TestBackend::new(140, 28)).unwrap();

    // 1. needs_input placeholder
    app.apply_checkpoint(
        json!({
            "environment": {"workspace": "/tmp/ws"},
            "instances": [{"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY", "model": "leader_main", "diagnosis": {"code": "needs_input", "reason": "Pick target"}}],
            "input_delivery": [{"id": "env-cli", "state": "ACCEPTED"}, {"id": "env-done", "state": "APPLIED"}]
        }),
        20,
    );
    app.input_receipt("env-tui", &json!({"queued": true, "applied": false}));
    app.input_receipt("env-cli", &json!({"queued": true, "applied": false}));
    assert_eq!(app.pending_input_count(), 2, "union must deduplicate env-cli");

    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let frame = buffer_text(&terminal, 140, 28);
    assert!(frame.contains("Reply to the Leader's question…"), "{frame}");
    assert!(frame.contains("2 queued · Ctrl+P"), "{frame}");

    // 2. work_blocked placeholder + reason in status_line
    app.apply_checkpoint(
        json!({
            "environment": {"workspace": "/tmp/ws"},
            "instances": [{"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY", "model": "leader_main", "diagnosis": {"code": "work_blocked", "reason": "Missing API token"}}],
            "attachments": []
        }),
        21,
    );
    assert!(app.status_line().starts_with("Work blocked"));
    assert!(app.status_line().contains("Missing API token"));
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    assert!(buffer_text(&terminal, 140, 28).contains("Work blocked · Ctrl+K to start a new work…"));

    // 3. paused/parked leader placeholder
    app.apply_checkpoint(
        json!({
            "environment": {"workspace": "/tmp/ws"},
            "instances": [{"id": "i-leader", "lifecycle": "PAUSED", "phase": "READY", "model": "leader_main", "reason": "paused by user"}]
        }),
        22,
    );
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    assert!(buffer_text(&terminal, 140, 28).contains("Leader paused/parked · Ctrl+K or Team (r) to resume…"));

    // 4. active & busy placeholder + combined attachment & queue count
    app.apply_checkpoint(
        json!({
            "environment": {"workspace": "/tmp/ws"},
            "instances": [{"id": "i-leader", "lifecycle": "ACTIVE", "phase": "MODEL_PENDING", "model": "leader_main"}]
        }),
        23,
    );
    app.attachments.paths.push("src/main.rs".into());
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let busy_frame = buffer_text(&terminal, 140, 28);
    assert!(busy_frame.contains("Type to queue follow-up (or /later for after work)…"), "{busy_frame}");
    assert!(busy_frame.contains("1 attached · /attach · 2 queued · Ctrl+P"), "{busy_frame}");
}

#[test]
fn tasks_view_auto_scrolls_and_clicks_rows_and_inspect_shows_shell_and_waits() {
    let mut app = base_app();
    let tasks: Vec<_> =
        (0..12).map(|i| json!({"id": format!("task-{i}"), "status": "RUNNING", "assignee": "w-worker"})).collect();
    app.apply_checkpoint(
        json!({
            "environment": {"workspace": "/tmp/ws"},
            "instances": [
                {
                    "id": "i-leader",
                    "lifecycle": "ACTIVE",
                    "phase": "WAITING",
                    "can_run_shell": true,
                    "diagnosis": {
                        "code": "waiting",
                        "summary": "Waiting on worker",
                        "waits": [{
                            "id": "wait-1",
                            "mode": "ALL",
                            "timer_at": null,
                            "conditions": [
                                {"kind": "task", "task_id": "t-9"},
                                {"kind": "message", "from": "w-worker"}
                            ]
                        }]
                    }
                },
                {
                    "id": "w-worker",
                    "lifecycle": "ACTIVE",
                    "phase": "READY",
                    "can_run_shell": false
                }
            ],
            "tasks": tasks
        }),
        30,
    );

    // Inspect view shows Shell grant status and human-readable wait conditions.
    app.team.select("i-leader");
    app.team.show(View::Inspect);
    let area = Rect::new(0, 0, 100, 20);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let leader_inspect = buffer_text(&terminal, area.width, area.height);
    assert!(leader_inspect.contains("Shell: allowed"), "{leader_inspect}");
    assert!(leader_inspect.contains("task t-9, message from w-worker"), "{leader_inspect}");

    app.team.select("w-worker");
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    assert!(buffer_text(&terminal, area.width, area.height).contains("Shell: not granted"));

    // Tasks view auto-scrolls when selecting off-screen tasks and supports clicking rows.
    app.handle_key(alt('2'));
    assert_eq!(app.team.view, View::Tasks);
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    assert_eq!(app.team.scroll, 0);

    for _ in 0..11 {
        app.handle_key(key(KeyCode::Char('j')));
    }
    assert_eq!(app.team.selected_task, 11);
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    assert!(app.team.scroll > 0, "tasks view must auto-scroll down to task-11");
    assert!(buffer_text(&terminal, area.width, area.height).contains("task-11"));

    for _ in 0..11 {
        app.handle_key(key(KeyCode::Char('k')));
    }
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    assert_eq!(app.team.scroll, 0);

    // Click row 1 (currently unselected) to select it, then click row 1 again to inspect assignee.
    let (_, row1_range) = app.team.task_row_ranges[1].clone();
    let click_y = app.team.tasks_list_rect.y + (row1_range.start - app.team.scroll) as u16;
    let click_x = app.team.tasks_list_rect.x + 2;
    v2ui::handle_mouse(&mut app, area, mouse_click(click_x, click_y));
    assert_eq!(app.team.selected_task, 1);
    assert_eq!(app.team.view, View::Tasks);

    v2ui::handle_mouse(&mut app, area, mouse_click(click_x, click_y));
    assert_eq!(app.team.view, View::Inspect);
    assert_eq!(app.team.selected, "w-worker");
}

#[test]
fn modal_ergonomics_queue_sessions_and_broadened_ctrl_m_ctrl_p() {
    // 1. Ctrl+M and Ctrl+P work from Focus::Team and Focus::Panel; Ctrl+Enter remains Composer-only.
    for start_focus in [Focus::Team, Focus::Panel] {
        let mut app = base_app();
        app.focus = start_focus;
        assert!(app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)).is_none());
        assert!(app.profile.is_none());

        assert!(app.handle_key(ctrl('m')).is_none());
        assert!(app.profile.is_some(), "Ctrl+M should open model menu from {start_focus:?}");
        app.handle_key(key(KeyCode::Esc));

        assert_eq!(app.handle_key(ctrl('p')), Some(V2Effect::Queue(QueueAction::List { after: 0 })));
        assert!(app.queue.is_some(), "Ctrl+P should open queue from {start_focus:?}");
    }

    // 2. Queue j/k + mouse click/scroll + guard during confirming/editing.
    let mut app = base_app();
    let mut q = Queue::default();
    q.apply(&json!({
        "inputs": [
            {"envelope_id": "e1", "revision": 1, "state": "ACCEPTED", "payload": {"text": "first", "attachments": []}},
            {"envelope_id": "e2", "revision": 1, "state": "ACCEPTED", "payload": {"text": "second", "attachments": []}},
            {"envelope_id": "e3", "revision": 1, "state": "ACCEPTED", "payload": {"text": "third", "attachments": []}}
        ]
    }));
    app.queue = Some(q);
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.queue.as_ref().unwrap().selected, 1);
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.queue.as_ref().unwrap().selected, 0);

    // Mouse wheel down selects row 1; click row 2 selects row 2; second click opens editor.
    let area = Rect::new(0, 0, 80, 16);
    v2ui::handle_mouse(&mut app, area, mouse_scroll(false, 10, 4));
    assert_eq!(app.queue.as_ref().unwrap().selected, 1);
    v2ui::handle_mouse(&mut app, area, mouse_click(10, 4)); // row 2 inside panel (y=1 border -> inner y=2 -> idx 2 at y=4)
    assert_eq!(app.queue.as_ref().unwrap().selected, 2);
    assert!(app.queue.as_ref().unwrap().editor.is_none());
    v2ui::handle_mouse(&mut app, area, mouse_click(10, 4));
    assert!(app.queue.as_ref().unwrap().editor.is_some());
    app.handle_key(key(KeyCode::Char('j')));
    assert!(app.queue.as_ref().unwrap().editor.as_ref().unwrap().text().contains('j'));
    app.handle_key(key(KeyCode::Esc));

    // Confirming mode ignores n, j, k.
    app.handle_key(key(KeyCode::Delete));
    assert!(app.queue.as_ref().unwrap().confirming);
    app.handle_key(key(KeyCode::Char('n')));
    app.handle_key(key(KeyCode::Char('j')));
    assert!(app.queue.as_ref().unwrap().confirming);

    // 3. SessionBrowser Ctrl+U clears search query; j/k navigates list and search hits.
    let mut browser = SessionBrowser::default();
    browser.apply_list(
        &json!({"sessions": [
            {"id": "s-0", "name": "zero", "path": "/tmp/0"},
            {"id": "s-1", "name": "one", "path": "/tmp/1"}
        ]}),
        "/tmp/0",
    );
    browser.apply_history(&json!({"entries": []}), None);
    browser.begin_search();
    browser.key(key(KeyCode::Char('a')));
    browser.key(key(KeyCode::Char('b')));
    assert_eq!(browser.search_query, "ab");
    browser.key(ctrl('u'));
    assert_eq!(browser.search_query, "");
    browser.key(key(KeyCode::Esc));

    assert_eq!(browser.selected, 0);
    assert_eq!(browser.key(key(KeyCode::Char('j'))), Some(SessionAction::History { id: "s-1".into(), before: None }));
    assert_eq!(browser.selected, 1);
    browser.apply_history(&json!({"entries": []}), None);
    assert_eq!(browser.key(key(KeyCode::Char('k'))), Some(SessionAction::History { id: "s-0".into(), before: None }));
    assert_eq!(browser.selected, 0);
    browser.apply_history(&json!({"entries": []}), None);

    // Confirming delete blocks j/k.
    browser.key(key(KeyCode::Char('d')));
    assert!(browser.confirmation.is_some());
    browser.key(key(KeyCode::Char('j')));
    assert_eq!(browser.selected, 0);
    browser.key(key(KeyCode::Esc));

    // Search hits j/k navigation
    browser.begin_search();
    browser.key(key(KeyCode::Char('x')));
    let Some(SessionAction::Search { generation, .. }) = browser.key(key(KeyCode::Enter)) else {
        panic!("expected Search action");
    };
    browser.apply_search(
        &json!({"hits": [{"session_id": "s-0", "snippet": "a"}, {"session_id": "s-1", "snippet": "b"}]}),
        "x",
        generation,
    );
    browser.key(key(KeyCode::Char('j')));
    assert_eq!(browser.search_selected, 1);
    browser.key(key(KeyCode::Char('k')));
    assert_eq!(browser.search_selected, 0);
}

#[test]
fn live_monitor_counts_operation_approvals_and_capability_requests() {
    let dir = std::env::temp_dir().join(format!("teamagents-cap-live-test-{}", std::process::id()));
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
                "session_id": "sib-cap",
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
                                    {"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY"}
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
                            "approvals": [{"id": "ap-1"}],
                            "capability_requests": [{"id": "cap-1"}, {"id": "cap-2"}]
                        }
                    }),
                };
                let _ = writeln!(stream, "{resp}");
                let _ = stream.flush();
            }
        }
    });

    let monitor = LiveMonitor::new();
    monitor.set_targets(&[("sib-cap".into(), "cap-box".into(), dir.to_string_lossy().into_owned())]);
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

    assert_eq!(app.live_sessions.first().map(|s| s.approvals), Some(3));
}

fn find_cell(terminal: &Terminal<TestBackend>, width: u16, height: u16, needle: &str) -> Option<(u16, u16)> {
    (0..height).find_map(|y| find_cell_on_row(terminal, width, y, needle).map(|col| (col, y)))
}

fn find_cell_on_row(terminal: &Terminal<TestBackend>, width: u16, y: u16, needle: &str) -> Option<u16> {
    let buf = terminal.backend().buffer();
    let mut row = String::new();
    let mut cell_cols = Vec::new();
    for x in 0..width {
        let sym = buf[(x, y)].symbol();
        let start = row.len();
        row.push_str(sym);
        cell_cols.push((start, x));
    }
    let byte_idx = row.find(needle)?;
    cell_cols.iter().find(|&&(start, _)| start >= byte_idx).map(|&(_, col)| col)
}

#[test]
fn welcome_starter_prompts_fill_composer_without_submitting_and_fit_narrow_width() {
    let mut app = base_app();
    let narrow = Rect::new(0, 0, 40, 24);
    let mut term = Terminal::new(TestBackend::new(narrow.width, narrow.height)).unwrap();
    term.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let screen = buffer_text(&term, narrow.width, narrow.height);
    assert!(screen.contains("Welcome to TeamAgents"), "{screen}");
    assert!(screen.contains("What would you like to do?"), "{screen}");
    assert!(screen.contains("What does this project do?"), "{screen}");

    let (sx, sy) = find_cell(&term, narrow.width, narrow.height, "What does this project do?").unwrap();
    assert!(v2ui::handle_mouse(&mut app, narrow, mouse_click(sx, sy)).is_none());
    assert_eq!(app.composer.text(), "What does this project do?");
    assert_eq!(app.focus, Focus::Composer);
    assert_eq!(app.activity_line(), "Loaded starter prompt · press Enter to send");

    let wide = Rect::new(0, 0, 140, 28);
    let mut wide_term = Terminal::new(TestBackend::new(wide.width, wide.height)).unwrap();
    wide_term.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let wide_screen = buffer_text(&wide_term, wide.width, wide.height);
    assert!(wide_screen.contains("Shift+Enter new line"), "{wide_screen}");
    assert!(wide_screen.contains("Click [Model] to change AI"), "{wide_screen}");

    for prompt in
        ["Explain the main files and how they fit together", "Find potential bugs or improvements in this project"]
    {
        app.focus = Focus::Panel;
        let (px, py) = find_cell(&wide_term, wide.width, wide.height, prompt).expect(prompt);
        assert!(v2ui::handle_mouse(&mut app, wide, mouse_click(px, py)).is_none());
        assert_eq!(app.composer.text(), prompt);
        assert_eq!(app.focus, Focus::Composer);
    }

    // Once conversation has entries, clicking in chat must not treat old coords as starter prompts.
    app.composer.set_text("keep draft");
    app.entries.push(ChatEntry {
        kind: ChatKind::User,
        who: "you".into(),
        text: "hello".into(),
        tool: None,
        workspace: "/tmp/ws".into(),
        media: vec![],
    });
    wide_term.draw(|f| v2ui::render(f, &mut app)).unwrap();
    assert!(v2ui::handle_mouse(&mut app, wide, mouse_click(4, 4)).is_none());
    assert_eq!(app.composer.text(), "keep draft");
}

#[test]
fn composer_border_menu_help_and_model_buttons_open_overlays_without_overlap() {
    let mut app = base_app();
    let area = Rect::new(0, 0, 100, 28);
    let mut term = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    term.draw(|f| v2ui::render(f, &mut app)).unwrap();

    let (title_x, title_y) = find_cell(&term, area.width, area.height, "Message Leader").unwrap();
    let menu_x = find_cell_on_row(&term, area.width, title_y, "[Menu]").unwrap();
    let help_x = find_cell_on_row(&term, area.width, title_y, "[?]").unwrap();
    let model_x = find_cell_on_row(&term, area.width, title_y, "[Model]").unwrap();
    assert!(title_x + "Message Leader".len() as u16 + 1 < menu_x);
    assert!(menu_x + 6 < help_x);
    assert!(help_x + 3 < model_x);

    assert!(v2ui::handle_mouse(&mut app, area, mouse_click(menu_x, title_y)).is_none());
    assert!(app.palette.is_some());
    app.handle_key(key(KeyCode::Esc));
    assert!(app.palette.is_none());

    assert!(v2ui::handle_mouse(&mut app, area, mouse_click(help_x, title_y)).is_none());
    assert!(app.help_open);
    app.handle_key(key(KeyCode::Esc));
    assert!(!app.help_open);

    assert!(v2ui::handle_mouse(&mut app, area, mouse_click(model_x, title_y)).is_none());
    assert!(app.profile.is_some());
    app.handle_key(key(KeyCode::Esc));
    assert!(app.profile.is_none());

    // Clicking [Model] inside the welcome tip line also opens the model chooser.
    let (tip_model_x, tip_model_y) = find_cell(&term, area.width, area.height, "[Model]").unwrap();
    assert!(tip_model_y < title_y);
    assert!(v2ui::handle_mouse(&mut app, area, mouse_click(tip_model_x, tip_model_y)).is_none());
    assert!(app.profile.is_some());
    app.handle_key(key(KeyCode::Esc));
    assert!(app.profile.is_none());

    // Narrow composer (< 54 cols) hides [Menu] and [?] while keeping [Model] and title.
    let narrow = Rect::new(0, 0, 50, 24);
    let mut narrow_term = Terminal::new(TestBackend::new(narrow.width, narrow.height)).unwrap();
    narrow_term.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let (_, narrow_title_y) = find_cell(&narrow_term, narrow.width, narrow.height, "Message Leader").unwrap();
    assert!(find_cell_on_row(&narrow_term, narrow.width, narrow_title_y, "[Model]").is_some());
    assert!(find_cell_on_row(&narrow_term, narrow.width, narrow_title_y, "[Menu]").is_none());
    assert!(find_cell_on_row(&narrow_term, narrow.width, narrow_title_y, "[?]").is_none());

    // Long direct-message title hides [Menu] instead of overlapping the border title.
    app.direct_target = Some("worker-very-long-name-123".into());
    let mid = Rect::new(0, 0, 60, 24);
    let mut mid_term = Terminal::new(TestBackend::new(mid.width, mid.height)).unwrap();
    mid_term.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let (_, mid_title_y) = find_cell(&mid_term, mid.width, mid.height, "Esc back to Leader").unwrap();
    assert!(find_cell_on_row(&mid_term, mid.width, mid_title_y, "[Menu]").is_none());
}

#[test]
fn ctrl_k_palette_everyday_keywords_match_core_actions() {
    let mut app = base_app();

    app.handle_key(ctrl('k'));
    for c in "undo".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::RewindList { before: None }));
    assert!(app.rewind_modal.is_some());
    app.handle_key(key(KeyCode::Esc));

    app.handle_key(ctrl('k'));
    for c in "ai".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert!(app.profile.is_some());
    app.handle_key(key(KeyCode::Esc));

    app.handle_key(ctrl('k'));
    for c in "stop".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::Interrupt { instance: "i-leader".into() }));
}

#[test]
fn shortcuts_overlay_leads_with_quick_start_and_preserves_existing_sections() {
    assert!(teamagents_tui::v2app::HELP_SECTIONS[0].0.starts_with("Quick start"));
    let mut app = base_app();
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.help_open);

    let area = Rect::new(0, 0, 100, 28);
    let mut term = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    term.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let top_screen = buffer_text(&term, area.width, area.height);
    assert!(top_screen.contains("Quick start (4 basics)"), "{top_screen}");
    assert!(top_screen.contains("Input editing"), "{top_screen}");

    for _ in 0..20 {
        app.handle_key(key(KeyCode::PageDown));
    }
    term.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let bottom_screen = buffer_text(&term, area.width, area.height);
    assert!(bottom_screen.contains("Approvals"), "{bottom_screen}");
}
