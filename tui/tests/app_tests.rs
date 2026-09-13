//! App logic + render smoke tests. The state snapshots mirror core `state`.

use serde_json::{json, Value as Json};
use teamagents_tui::*; // see lib target note below

fn spec() -> Json {
    json!({
        "leader_id": "leader",
        "agents": [
            {"id": "leader", "name": "Leader", "role": "leader", "runtime_kind": "deepagents",
             "instructions": "", "model_profile": "leader_main", "tool_bindings": ["files", "shell"],
             "workspace_policy": "shared"},
            {"id": "worker", "name": "W", "role": "dev", "runtime_kind": "codex",
             "instructions": "", "model_profile": "worker_main", "tool_bindings": ["shell"],
             "workspace_policy": "isolated"}
        ],
        "channels": [
            {"source": "worker", "targets": ["leader"], "mode": "message"},
            {"source": "leader", "targets": ["worker"], "mode": "task"},
            {"source": "leader", "targets": ["worker"], "mode": "broadcast"}
        ],
        "observers": [{"agent_id": "boss", "subjects": ["worker"], "event_types": [],
                       "payload_scope": "status", "wake_policy": "none", "capabilities": []}],
        "shared_spaces": [{"id": "main", "readers": ["leader"], "writers": ["leader", "worker"]}]
    })
}

fn state(events: Vec<Json>) -> Json {
    json!({
        "session": {"session_id": "s1", "status": "ACTIVE", "cwd": "/repo",
                    "permissions_mode": "approved_scope", "goal_id": "g1", "goal_state": "in_progress"},
        "spec": spec(),
        "leader_id": "leader",
        "limits": {"max_parallel_workers": 8, "max_members": 20, "max_turns_per_goal": 1000,
                   "max_model_steps_per_turn": 200, "turn_active_timeout_s": 1200,
                   "cancel_confirm_timeout_s": 60},
        "revision": 3,
        "agents": [{"id": "leader", "status": "BUSY"}, {"id": "worker", "status": "IDLE"}],
        "runs": [{"run_id": "r1", "session_id": "s1", "task_id": null, "goal_id": "g1",
                  "agent_id": "leader", "config_revision": 1, "topology_revision": 3,
                  "status": "RUNNING", "input_delivery_ids": [], "context_ref": null,
                  "external_turn_id": null, "cancel_requested": false, "waiting_on": [],
                  "created_at": 1000.0}],
        "tasks": [
            {"task_id": "task-aaaaaaaabbbb", "parent_task_id": null, "goal_id": "g1",
             "requester": "leader", "assignee": "worker", "description": "write tests",
             "acceptance": "", "dependencies": [], "status": "PENDING", "result_refs": [],
             "created_at": 1700000000.0},
            {"task_id": "task-ccccccccdddd", "parent_task_id": "task-aaaaaaaabbbb", "goal_id": "g1",
             "requester": "worker", "assignee": "leader", "description": "review", "acceptance": "",
             "dependencies": ["task-aaaaaaaabbbb"], "status": "RUNNING", "result_refs": [],
             "created_at": 1700000001.0}
        ],
        "pending_approvals": [{"approval_id": "ap1", "session_id": "s1", "agent_id": "worker",
                               "run_id": "r2", "tool_call_id": "c1", "operation_hash": "h",
                               "requested_scope": {"tool": "shell", "args": {"cmd": "rm -rf x"}},
                               "policy_revision": 1, "status": "PENDING", "created_at": 1.0,
                               "decided_at": null}],
        "events": events,
    })
}

fn app_with(state: Json) -> App {
    let mut app = App::new("s1", json!({"models": {}, "tools": {}, "skills_paths": [], "instruction_files": []}),
                           "/home/u/.config/teamagents/config.toml".into(), "en", true, vec![]);
    app.apply_state(&state);
    app
}

#[test]
fn events_render_to_chat_like_python() {
    let evs = vec![
        json!({"sequence": 1, "kind": "user_message", "actor_id": "user", "payload": {"text": "hi"}}),
        json!({"sequence": 2, "kind": "leader_reply", "actor_id": "leader", "payload": {"text": "hello", "run_id": "r1"}}),
        json!({"sequence": 3, "kind": "task_created", "actor_id": "leader",
               "payload": {"task_id": "task-aaaaaaaabbbb", "assignee": "worker", "description": "write tests"}}),
        json!({"sequence": 4, "kind": "task_completed", "actor_id": "worker",
               "payload": {"task_id": "task-aaaaaaaabbbb", "summary": "done"}}),
        json!({"sequence": 5, "kind": "message", "actor_id": "worker",
               "payload": {"target": "leader", "text": "ping"}}),
        json!({"sequence": 6, "kind": "goal_done", "actor_id": "leader", "payload": {"summary": "all good"}}),
    ];
    let app = app_with(state(evs.clone()));
    let chat: Vec<&(String, String)> = app.chat.iter().collect();
    assert_eq!(chat[0], &("user".to_string(), "hi".to_string()));
    assert_eq!(chat[1], &("Leader".to_string(), "hello".to_string()));
    assert_eq!(chat[2], &("system".to_string(), "Task aaaabbbb → worker: write tests".to_string()));
    assert_eq!(chat[3], &("system".to_string(), "[task_completed] aaaabbbb done".to_string()));
    assert_eq!(chat[4], &("worker→leader".to_string(), "ping".to_string()));
    assert_eq!(chat[5], &("system".to_string(), "Goal complete: all good".to_string()));
    // duplicate replay of old events is guarded by the cursor
    let mut app2 = app_with(state(evs.clone()));
    let _ = app2.apply_state(&state(evs));
    assert_eq!(app2.chat.len(), 6);
}

#[test]
fn approval_event_toasts_and_bells() {
    let evs = vec![json!({"sequence": 1, "kind": "approval_requested", "actor_id": "worker",
        "payload": {"approval_id": "ap1", "agent_id": "worker",
                    "scope": {"tool": "shell", "args": {"cmd": "rm -rf x"}}}})];
    let mut app = app_with(state(vec![]));
    let effects = app.apply_state(&state(evs));
    assert!(effects.iter().any(|e| matches!(e, Effect::Bell)));
    assert_eq!(app.toasts.len(), 1);
    assert!(app.chat[0].1.contains("Approval required:"));
    assert!(app.chat[0].1.contains("Ctrl+G"));
}

#[test]
fn team_rows_reach_column_matches_spec_rules() {
    let app = app_with(state(vec![]));
    let rows = app.team_rows();
    let leader = &rows.iter().find(|(k, _)| k == "leader").unwrap().1;
    let worker = &rows.iter().find(|(k, _)| k == "worker").unwrap().1;
    // leader: message→worker (broadcast), task→leader(self)+worker, no observers
    assert_eq!(leader[6].0, "Messages → leader,worker Tasks → leader,worker"); // broadcast includes self
    // worker: message→leader, no task channel; observed by boss
    assert_eq!(worker[6].0, "Messages → leader Observers: boss");
    assert_eq!(worker[5].0, "isolated");
}

#[test]
fn tasks_rows_newest_first_with_indent() {
    let app = app_with(state(vec![]));
    let rows = app.tasks_rows();
    assert_eq!(rows[0].0, "task-ccccccccdddd"); // newer first
    assert!(rows[0].1[0].0.starts_with("  ")); // indented under parent
    assert_eq!(rows[0].1[5].0, "aaaabbbb"); // dependency tail
}

#[test]
fn status_bar_shows_counts_and_mode() {
    let app = app_with(state(vec![]));
    let bar = app.status_bar();
    assert!(bar.contains("TeamAgents · s1"), "{bar}");
    assert!(bar.contains("Pre-authorized"), "{bar}");
    assert!(bar.contains("Approvals 1"), "{bar}");
    assert!(bar.contains("Tasks 2"), "{bar}");
}

#[test]
fn key_paths_produce_expected_effects() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let key = |c: KeyCode, m: KeyModifiers| KeyEvent::new(c, m);

    // ctrl+f toggles full auto
    let mut app = app_with(state(vec![]));
    let fx = app.handle_key(key(KeyCode::Char('f'), KeyModifiers::CONTROL));
    match &fx[0] {
        Effect::Submit { action, .. } => {
            assert_eq!(action["kind"], "set_permission_mode");
            assert_eq!(action["payload"]["mode"], "full_auto");
        }
        other => panic!("{other:?}"),
    }

    // esc interrupts the leader's active run
    let fx = app.handle_key(key(KeyCode::Esc, KeyModifiers::NONE));
    match &fx[0] {
        Effect::Submit { action, .. } => {
            assert_eq!(action["kind"], "cancel_run");
            assert_eq!(action["payload"]["run_id"], "r1");
        }
        other => panic!("{other:?}"),
    }

    // typing + enter submits a user message
    for c in "hello".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let fx = app.handle_key(key(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(&fx[0], Effect::UserMessage(t) if t == "hello"));

    // approvals: ctrl+g jumps to the panel and focuses it; 'a' decides once
    let _ = app.handle_key(key(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert_eq!(app.panel, 3);
    let fx = app.handle_key(key(KeyCode::Char('a'), KeyModifiers::NONE));
    match &fx[0] {
        Effect::DecideApproval { approval_id, decision } => {
            assert_eq!(approval_id, "ap1");
            assert_eq!(decision, "once");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn cancel_task_feedback_covers_all_receipts() {
    assert_eq!(cancel_task_feedback("en", "task-xxxxxxxxyyyy", &json!({"ok": false, "error": "nope"})),
               "Cannot cancel task: nope");
    assert_eq!(cancel_task_feedback("en", "task-xxxxxxxxyyyy", &json!({"ok": true, "result": {"status": "CANCELLED"}})),
               "Task xxxxyyyy cancelled");
    assert_eq!(cancel_task_feedback("en", "task-xxxxxxxxyyyy", &json!({"ok": true, "result": {"status": "CANCEL_REQUESTED"}})),
               "Task xxxxyyyy: cancellation requested (waiting for the active turn to stop)");
    assert_eq!(cancel_task_feedback("en", "task-xxxxxxxxyyyy", &json!({"ok": true, "result": {"status": "SUCCEEDED"}})),
               "Task xxxxyyyy is already terminal (SUCCEEDED); no cancellation needed");
}

#[test]
fn sessions_double_delete_and_zh_rendering() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app_with(state(vec![]));
    app.sessions = vec![json!({"sessionId": "s2", "status": "CLOSED", "goalState": "done",
        "events": 5, "sizeMb": 1.2, "updatedAt": 1700000000.0, "archived": false, "locked": false})];
    app.panel = 4; // sessions
    app.focus = Focus::Panel;
    let fx = app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(fx.is_empty()); // first press only asks for confirmation
    assert!(app.chat.last().unwrap().1.contains("Press d again"));
    let fx = app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(matches!(&fx[0], Effect::DeleteSession(s) if s == "s2"));

    // zh-CN renders the message ids
    let mut zh = app_with(state(vec![]));
    zh.lang = "zh-CN";
    assert!(zh.status_bar().contains("预授权"));
    let rows = zh.team_rows();
    assert!(rows.iter().find(|(k, _)| k == "leader").unwrap().1[6].0.contains("任务→"));
}

#[test]
fn composer_control_chords_navigate_and_never_insert_letters() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app_with(state(vec![]));
    for c in "hello".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    // Ctrl+A / Ctrl+E move the cursor instead of typing "a" / "e" (D-14 contract)
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
    app.handle_key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));
    assert_eq!(app.composer.text(), "xhello!");
    // an unrelated control chord is swallowed, not typed
    app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    assert_eq!(app.composer.text(), "xhello!");
}
