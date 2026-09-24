//! R19-b② v2 conversation interface: pure-logic key/state paths plus a
//! full-frame render smoke on ratatui's TestBackend — no terminal, no daemon.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::{json, Value as Json};
use teamagents_tui::v2app::{ChatKind, Confirm, Focus, V2App, V2Effect, View};
use teamagents_tui::v2ui;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn checkpoint() -> Json {
    json!({"instances": [
               {"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY"},
               {"id": "i-worker", "lifecycle": "ACTIVE", "phase": "MODEL_PENDING"}
           ],
           "goal": {"status": "ACTIVE", "known_usage": {"prompt": 7, "completion": 3, "total": 10},
                    "unknown_usage": 0, "limits": {"max_total_tokens": 1000}}})
}

fn app() -> V2App {
    let mut app = V2App::new("s-test");
    app.apply_checkpoint(checkpoint(), 5);
    app
}

#[test]
fn checkpoint_defaults_to_the_leader_and_tracks_budget() {
    let app = app();
    assert_eq!(app.active_instance().unwrap().id, "i-leader");
    assert_eq!(app.watermark, 5);
    let goal = app.goal.as_ref().unwrap();
    assert_eq!(goal.known_total, 10);
    assert_eq!(goal.limit_total, Some(1000));
    assert!(!goal.unknown);
    let status = app.status_line();
    assert!(status.contains("i-leader [READY]"), "{status}");
    assert!(status.contains("用量 10/1000"), "{status}");
}

#[test]
fn history_rebuilds_the_conversation_and_keeps_system_notes() {
    let mut app = app();
    app.apply_events(&[
        json!({"sequence": 6, "kind": "goal_completed", "scope": "g1", "payload": {"status": "SUCCEEDED"}}),
    ]);
    assert_eq!(app.entries.len(), 1); // the note
    app.apply_history(json!({"entries": [
        {"idx": 1, "kind": "user", "message": {"role": "user", "content": "写个脚本"}},
        {"idx": 2, "kind": "assistant", "message": {"role": "assistant", "content": "好的",
             "tool_calls": [{"id": "c1", "type": "function",
                             "function": {"name": "shell", "arguments": "{\"command\": \"ls\"}"}}]}},
        {"idx": 3, "kind": "tool_result", "message": {"role": "tool", "tool_call_id": "c1", "content": "file.txt"}},
        {"idx": 4, "kind": "assistant", "message": {"role": "assistant", "content": "完成了"}}
    ]}));
    let kinds: Vec<_> = app.entries.iter().map(|e| e.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            ChatKind::User,
            ChatKind::Assistant,
            ChatKind::Tool,
            ChatKind::Tool,
            ChatKind::Assistant,
            ChatKind::System
        ]
    );
    assert_eq!(app.entries[0].text, "写个脚本");
    assert_eq!(app.entries[0].who, "你");
    assert_eq!(app.entries[2].text, "[shell] ls");
    assert_eq!(app.entries[3].text, "file.txt");
    // the system note survived the rebuild, layered after the conversation
    assert!(app.entries[5].text.contains("目标完成"));
}

/// R22/A20: a compaction summary is shown as its own entry, so the user can
/// tell where the model's view was compacted while still seeing every
/// original exchange.
#[test]
fn a_compaction_summary_is_shown_and_does_not_duplicate_notes() {
    let mut app = app();
    let history = json!({"entries": [
        {"idx": 1, "kind": "user", "message": {"role": "user", "content": "长任务"}},
        {"idx": 2, "kind": "assistant", "message": {"role": "assistant", "content": "做完了"}},
        {"idx": 3, "kind": "summary", "message": {"role": "user",
             "content": "[Compacted conversation summary]\n1. Original request: 长任务"}},
        {"idx": 4, "kind": "user", "message": {"role": "user", "content": "接着来"}}
    ]});
    app.apply_history(history.clone());
    let kinds: Vec<_> = app.entries.iter().map(|e| e.kind.clone()).collect();
    assert_eq!(kinds, vec![ChatKind::User, ChatKind::Assistant, ChatKind::Summary, ChatKind::User]);
    assert_eq!(app.entries[2].who, "压缩");
    assert!(app.entries[2].text.contains("Compacted conversation summary"));
    // a second rebuild from the same history is not duplicated by the
    // retained-note path
    app.apply_history(history);
    let summaries = app.entries.iter().filter(|e| e.kind == ChatKind::Summary).count();
    assert_eq!(summaries, 1);
}

#[test]
fn events_drive_refreshes_and_notes() {
    let mut app = app();
    let refresh = app.apply_events(&[
        json!({"sequence": 6, "kind": "input", "scope": "i-leader", "payload": {"envelope_id": "e1", "applied": true}}),
        json!({"sequence": 7, "kind": "check_round_registered", "scope": "g1",
               "payload": {"goal_id": "g1", "round": 1, "checks": [{"id": "c1"}]}}),
        json!({"sequence": 8, "kind": "completion_repair", "scope": "i-leader",
               "payload": {"goal_id": "g1", "round": 1, "failures": []}}),
        json!({"sequence": 9, "kind": "approval_requested", "scope": "op-1", "payload": {"approval_id": "ap-1"}}),
        json!({"sequence": 10, "kind": "operation_completed", "scope": "op-2", "payload": {"status": "OUTCOME_UNKNOWN"}}),
    ]);
    assert!(refresh.history);
    assert!(refresh.approvals);
    assert!(!refresh.checkpoint);
    assert_eq!(app.watermark, 10);
    let notes: Vec<_> = app.entries.iter().filter(|e| e.kind == ChatKind::System).collect();
    assert!(notes.iter().any(|n| n.text.contains("完成检查第 1 轮开始（1 项）")), "{notes:?}");
    assert!(notes.iter().any(|n| n.text.contains("进入修复回合")), "{notes:?}");
    assert!(notes.iter().any(|n| n.text.contains("结果不明")), "{notes:?}");
}

#[test]
fn goal_events_mark_checkpoint_refresh() {
    let mut app = app();
    let refresh = app.apply_events(&[
        json!({"sequence": 6, "kind": "goal_blocked", "scope": "g1", "payload": {"reason": "checks failed"}}),
        json!({"sequence": 7, "kind": "instance_spawned", "scope": "i-w2", "payload": {"spawner": "i-leader", "task": true}}),
    ]);
    assert!(refresh.checkpoint);
    assert!(app.entries.iter().any(|e| e.text.contains("BLOCKED")));
    assert!(app.entries.iter().any(|e| e.text.contains("i-w2 由 i-leader 派出")));
}

#[test]
fn composer_submit_targets_the_active_instance() {
    let mut app = app();
    app.handle_key(key(KeyCode::Char('你')));
    app.handle_key(key(KeyCode::Char('好')));
    let effect = app.handle_key(key(KeyCode::Enter)).expect("submit effect");
    let V2Effect::SubmitInput { instance, envelope, text } = effect else { panic!("wrong effect") };
    assert_eq!(instance, "i-leader");
    assert!(envelope.starts_with("env-"), "{envelope}");
    assert_eq!(text, "你好");
    // the composer cleared for the next message
    assert!(app.composer.text().is_empty());
    // tab switches the conversation target
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.active_instance().unwrap().id, "i-worker");
    app.handle_key(key(KeyCode::Char('x')));
    let V2Effect::SubmitInput { instance, .. } = app.handle_key(key(KeyCode::Enter)).unwrap() else { panic!() };
    assert_eq!(instance, "i-worker");
}

#[test]
fn approvals_focus_decides_and_leaves() {
    let mut app = app();
    app.apply_approvals(json!({"approvals": [
        {"id": "ap-1", "operation_id": "op-1", "tool": "shell", "preview": "rm -rf /tmp/x"},
        {"id": "ap-2", "operation_id": "op-2", "tool": "shell", "preview": "curl example.com"}
    ]}));
    // F2 only enters with pending approvals
    app.handle_key(key(KeyCode::F(2)));
    assert_eq!(app.focus, Focus::Approvals);
    app.handle_key(key(KeyCode::Down));
    let effect = app.handle_key(key(KeyCode::Char('a'))).expect("decide");
    assert_eq!(effect, V2Effect::Decide { approval_id: "ap-2".into(), decision: "approve" });
    // decide→ the daemon answer refreshes the list; empty list returns focus
    app.apply_approvals(json!({"approvals": []}));
    assert_eq!(app.focus, Focus::Composer);
    // deny path
    app.apply_approvals(
        json!({"approvals": [{"id": "ap-3", "operation_id": "op-3", "tool": "shell", "preview": "p"}]}),
    );
    app.handle_key(key(KeyCode::F(2)));
    let effect = app.handle_key(key(KeyCode::Char('d'))).expect("deny");
    assert_eq!(effect, V2Effect::Decide { approval_id: "ap-3".into(), decision: "deny" });
}

#[test]
fn disconnect_marks_once_and_reconnect_notes_once() {
    let mut app = app();
    app.mark_disconnected("timeout");
    app.mark_disconnected("timeout");
    assert!(app.disconnected);
    let errors: Vec<_> = app.entries.iter().filter(|e| e.kind == ChatKind::Error).collect();
    assert_eq!(errors.len(), 1);
    assert!(app.status_line().contains("已断开"), "{}", app.status_line());
    app.mark_connected();
    assert!(!app.disconnected);
    let notes: Vec<_> = app.entries.iter().filter(|e| e.kind == ChatKind::System).collect();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].text.contains("重新连接"));
}

#[test]
fn scroll_clamps_to_the_wrapped_conversation() {
    let mut app = app();
    app.last_chat_height = 5;
    app.last_chat_lines = 20;
    app.scroll_chat(100);
    assert_eq!(app.chat_scroll, 15);
    app.scroll_chat_back(5);
    assert_eq!(app.chat_scroll, 10);
    app.scroll_chat_back(100);
    assert_eq!(app.chat_scroll, 0);
}

fn frame_lines(terminal: &Terminal<TestBackend>) -> Vec<String> {
    // wide chars occupy two cells; the continuation cell holds a space and
    // must be skipped when reading frame text
    let buffer = terminal.backend().buffer();
    let area = buffer.area;
    let mut out = Vec::new();
    for y in 0..area.height {
        let mut line = String::new();
        let mut skip_next = false;
        for x in 0..area.width {
            if skip_next {
                skip_next = false;
                continue;
            }
            let sym = buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or_default();
            line.push_str(sym);
            skip_next = unicode_width::UnicodeWidthStr::width(sym) == 2;
        }
        out.push(line.trim_end().to_string());
    }
    out
}

#[test]
fn frame_shows_status_chat_approvals_composer_and_footer() {
    let mut app = app();
    app.apply_history(json!({"entries": [
        {"idx": 1, "kind": "user", "message": {"role": "user", "content": "把测试跑起来"}},
        {"idx": 2, "kind": "assistant", "message": {"role": "assistant", "content": "已经在跑了"}}
    ]}));
    app.apply_approvals(json!({"approvals": [
        {"id": "ap-1", "operation_id": "op-1", "tool": "shell", "preview": "make check"}
    ]}));
    app.composer.set_text("继续");
    let backend = TestBackend::new(72, 18);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let lines = frame_lines(&terminal);
    let all = lines.join("\n");
    assert!(all.contains("i-leader [READY]"), "{all}");
    assert!(all.contains("把测试跑起来"), "{all}");
    assert!(all.contains("已经在跑了"), "{all}");
    assert!(all.contains("ap-1 · shell · make check"), "{all}");
    assert!(all.contains("发给 i-leader"), "{all}");
    assert!(all.contains("继续"), "{all}");
    assert!(all.contains("F2 批准(1)"), "{all}");
    // the geometry shares one source with hit-testing: the approvals box sits
    // directly above the composer
    let geo = v2ui::geometry(&app, ratatui::layout::Rect::new(0, 0, 72, 18));
    assert!(geo.approvals.height > 0);
    assert_eq!(v2ui::approval_at(&geo, &app, geo.approvals.y + 1, 2), Some(0));
    assert_eq!(v2ui::approval_at(&geo, &app, geo.approvals.y, 2), None);
}

// ---- R19-b③ panels: instances / tasks / topology -------------------------

fn tasks_json() -> Json {
    json!({"tasks": [
        {"id": "t-1", "goal_id": "g1", "assignee": "i-worker", "status": "RUNNING"},
        {"id": "t-2", "goal_id": "g1", "assignee": "i-leader", "status": "SUCCEEDED"}
    ]})
}

fn grants_json() -> Json {
    json!({"grants": [
        {"subject": "i-leader", "action": "manage", "resource_scope": "session", "revoked": false},
        {"subject": "i-worker", "action": "message", "resource_scope": "instance:i-leader", "revoked": false},
        {"subject": "i-old", "action": "manage", "resource_scope": "session", "revoked": true}
    ]})
}

#[test]
fn view_switching_is_global_and_esc_returns() {
    let mut app = app();
    assert_eq!(app.view, View::Chat);
    app.handle_key(key(KeyCode::F(3)));
    assert_eq!(app.view, View::Instances);
    assert!(app.status_line().contains("视图 实例"), "{}", app.status_line());
    app.handle_key(key(KeyCode::F(4)));
    assert_eq!(app.view, View::Tasks);
    app.handle_key(key(KeyCode::F(5)));
    assert_eq!(app.view, View::Topology);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.view, View::Chat);
    // F1 also returns from a panel
    app.handle_key(key(KeyCode::F(4)));
    app.handle_key(key(KeyCode::F(1)));
    assert_eq!(app.view, View::Chat);
}

#[test]
fn instances_panel_pauses_resumes_and_switches_the_conversation() {
    let mut app = app();
    app.handle_key(key(KeyCode::F(3)));
    assert_eq!(app.instance_sel, 0); // follows the conversation target
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.instance_sel, 1);
    let effect = app.handle_key(key(KeyCode::Char('p'))).expect("pause effect");
    assert_eq!(effect, V2Effect::SetLifecycle { instance: "i-worker".into(), lifecycle: "PAUSED" });
    let effect = app.handle_key(key(KeyCode::Char('r'))).expect("resume effect");
    assert_eq!(effect, V2Effect::SetLifecycle { instance: "i-worker".into(), lifecycle: "ACTIVE" });
    // Enter promotes the selection to the conversation target and returns
    let effect = app.handle_key(key(KeyCode::Enter));
    assert!(effect.is_none());
    assert_eq!(app.view, View::Chat);
    assert_eq!(app.active_instance().unwrap().id, "i-worker");
}

#[test]
fn termination_requires_an_explicit_confirmation() {
    let mut app = app();
    app.handle_key(key(KeyCode::F(3)));
    let effect = app.handle_key(key(KeyCode::Char('t')));
    assert!(effect.is_none());
    assert_eq!(app.confirm, Some(Confirm::TerminateInstance { instance: "i-leader".into() }));
    assert!(app.footer_hint().contains("确认终止"), "{}", app.footer_hint());
    // other keys are swallowed while the confirmation pends
    assert!(app.handle_key(key(KeyCode::Char('p'))).is_none());
    assert_eq!(app.confirm, Some(Confirm::TerminateInstance { instance: "i-leader".into() }));
    // n cancels; a fresh t + y terminates
    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(app.confirm, None);
    app.handle_key(key(KeyCode::Char('t')));
    let effect = app.handle_key(key(KeyCode::Char('y'))).expect("terminate effect");
    assert_eq!(effect, V2Effect::SetLifecycle { instance: "i-leader".into(), lifecycle: "TERMINATED" });
    assert_eq!(app.confirm, None);
}

#[test]
fn tasks_panel_cancels_only_live_tasks() {
    let mut app = app();
    app.apply_tasks(tasks_json());
    app.handle_key(key(KeyCode::F(4)));
    let effect = app.handle_key(key(KeyCode::Char('c'))).expect("cancel effect");
    assert_eq!(effect, V2Effect::CancelTask { task_id: "t-1".into() });
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.task_sel, 1);
    // SUCCEEDED is terminal: nothing to cancel
    assert!(app.handle_key(key(KeyCode::Char('c'))).is_none());
}

#[test]
fn task_and_grant_events_drive_panel_refreshes() {
    let mut app = app();
    let refresh = app.apply_events(&[
        json!({"sequence": 6, "kind": "task_delegated", "scope": "i-worker", "payload": {"task_id": "t-1"}}),
        json!({"sequence": 7, "kind": "grant_issued", "scope": "g-1", "payload": {}}),
        json!({"sequence": 8, "kind": "task_cancelled", "scope": "i-worker",
               "payload": {"task_id": "t-1", "reason": "superseded"}}),
    ]);
    assert!(refresh.tasks);
    assert!(refresh.grants);
    assert!(app.entries.iter().any(|e| e.text.contains("任务 t-1 已取消：superseded")), "{:?}", app.entries);
}

#[test]
fn panel_selections_clamp_after_data_refreshes() {
    let mut app = app();
    app.apply_tasks(tasks_json());
    app.task_sel = 1;
    app.apply_tasks(json!({"tasks": []}));
    assert_eq!(app.task_sel, 0);
    app.instance_sel = 5;
    app.apply_checkpoint(checkpoint(), 6);
    assert_eq!(app.instance_sel, 1); // two instances in the checkpoint
}

#[test]
fn frame_shows_the_panels_and_panel_hit_testing() {
    let mut app = app();
    app.apply_tasks(tasks_json());
    app.apply_grants(grants_json());
    let backend = TestBackend::new(72, 18);

    // instances panel
    app.handle_key(key(KeyCode::F(3)));
    let mut terminal = Terminal::new(TestBackend::new(72, 18)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let all = frame_lines(&terminal).join("\n");
    assert!(all.contains("实例（● 对话目标）"), "{all}");
    assert!(all.contains("i-leader · ACTIVE · READY"), "{all}");
    assert!(all.contains("i-worker"), "{all}");
    assert!(all.contains("t 终止"), "{all}");
    let geo = v2ui::geometry(&app, ratatui::layout::Rect::new(0, 0, 72, 18));
    assert_eq!(geo.approvals.height, 0); // panels hide the chat-only boxes
    assert_eq!(geo.composer.height, 0);
    assert_eq!(v2ui::panel_row_at(&geo, geo.body.y + 1, 2), Some(0));
    assert_eq!(v2ui::panel_row_at(&geo, geo.body.y + 2, 2), Some(1));
    assert_eq!(v2ui::panel_row_at(&geo, geo.body.y, 2), None); // the border
    drop(terminal);

    // tasks panel
    app.handle_key(key(KeyCode::F(4)));
    let mut terminal = Terminal::new(TestBackend::new(72, 18)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let all = frame_lines(&terminal).join("\n");
    assert!(all.contains("任务"), "{all}");
    assert!(all.contains("t-1 · RUNNING · 承接 i-worker · 目标 g1"), "{all}");
    assert!(all.contains("t-2 · SUCCEEDED"), "{all}");
    assert!(all.contains("c 取消任务"), "{all}");
    drop(terminal);

    // topology panel: grant/channel edges + task-delegation edges, revoked hidden
    app.handle_key(key(KeyCode::F(5)));
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let all = frame_lines(&terminal).join("\n");
    assert!(all.contains("拓扑 · 活跃授权 2 · 任务 2"), "{all}");
    assert!(all.contains("授权与通道"), "{all}");
    assert!(all.contains("i-leader ─manage→ session"), "{all}");
    assert!(all.contains("i-worker ─message→ instance:i-leader"), "{all}");
    assert!(!all.contains("i-old"), "{all}"); // revoked grants are not topology
    assert!(all.contains("任务委派"), "{all}");
    assert!(all.contains("t-1 ─→ i-worker  [RUNNING]"), "{all}");
}
