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

#[test]
fn esc_leaves_the_panel_before_it_stops_the_leader() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use teamagents_tui::app::Focus;
    let mut app = app_with(state(vec![]));
    app.focus = Focus::Panel;
    let effects = app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(effects.is_empty(), "Esc in a panel only leaves the panel");
    assert_eq!(app.focus, Focus::Composer);

    let effects = app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(effects.iter().any(|e| matches!(e, Effect::Submit { .. })), "Esc in the composer stops the Leader");
}

#[test]
fn page_keys_scroll_the_chat_and_clamp_at_both_ends() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app_with(state(vec![]));
    for i in 0..50 {
        app.chat.push(("Leader".into(), format!("line {i}")));
    }
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert_eq!(app.chat_scroll, 10);
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert_eq!(app.chat_scroll, 0);
    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
    assert_eq!(app.chat_scroll, 0);
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert!(app.chat_scroll > 0);
    // typing snaps back to the newest entry
    app.composer.set_text("hello");
    app.handle_key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));
    assert_eq!(app.chat_scroll, 0);
}

#[test]
fn composer_word_motion_and_deletion() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app_with(state(vec![]));
    app.composer.set_text("fix the failing test");
    app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
    assert_eq!(app.composer.text(), "fix the failing ");
    app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
    assert_eq!(app.composer.text(), "fix the ");
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
    assert_eq!(app.composer.col, 4, "Ctrl+← lands on the previous word");
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
    assert_eq!(app.composer.col, 7);
}

/// The panel actions are plain keys: Ctrl+A/E are line motions, Ctrl+D/U scroll
/// (per the docs), so a chord in the panels must never archive, deny, delete or
/// cancel the selected row.
#[test]
fn panel_chords_never_fire_destructive_actions() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use teamagents_tui::app::Focus;
    let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    let cases: [(usize, &str, Vec<char>); 3] = [
        (4, "s2", vec!['a', 'd', 'd']),      // sessions: archive / delete
        (3, "ap1", vec!['a', 's', 'd']),     // approvals: approve / deny
        (1, "task-aaaaaaaabbbb", vec!['c']), // tasks: cancel
    ];
    for (panel, cursor_key, chords) in cases {
        let mut app = app_with(state(vec![]));
        app.sessions = vec![json!({"sessionId": "s2", "status": "CLOSED", "goalState": "done",
            "events": 1, "sizeMb": 0.1, "updatedAt": 0.0, "archived": false, "locked": false})];
        app.panel = panel;
        app.focus = Focus::Panel;
        app.table_cursors.insert(teamagents_tui::app::PANELS[panel], (Some(cursor_key.to_string()), 0));
        for c in chords {
            let effects = app.handle_key(ctrl(c));
            assert!(
                effects.is_empty(),
                "{} + Ctrl+{} produced {effects:?}",
                teamagents_tui::app::PANELS[panel],
                c.to_ascii_uppercase()
            );
        }
        assert!(app.pending_delete.is_none(), "a chord armed the delete confirmation");
        assert_eq!(app.focus, Focus::Panel, "a chord moved the focus");
    }
    // the plain keys still work: sessions + 'a' archives, approvals + 's' allows
    let mut app = app_with(state(vec![]));
    app.sessions = vec![json!({"sessionId": "s2", "status": "CLOSED", "goalState": "done",
        "events": 1, "sizeMb": 0.1, "updatedAt": 0.0, "archived": false, "locked": false})];
    app.panel = 4;
    app.focus = Focus::Panel;
    app.table_cursors.insert("sessions", (Some("s2".into()), 0));
    let effects = app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(matches!(&effects[0], Effect::ArchiveSession(s) if s == "s2"));
    let mut app = app_with(state(vec![]));
    app.panel = 3;
    app.focus = Focus::Panel;
    app.table_cursors.insert("approvals", (Some("ap1".into()), 0));
    let effects = app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert!(matches!(&effects[0], Effect::DecideApproval { decision, .. } if decision == "session"));
}

/// Ctrl+U/D are documented scrolls: with the panel focused they must scroll the
/// pane (the log inside the log panel, the chat elsewhere), never table actions.
#[test]
fn ctrl_d_and_ctrl_u_scroll_while_the_panel_is_focused() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use teamagents_tui::app::Focus;
    let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    let mut app = app_with(state(vec![]));
    app.focus = Focus::Panel; // team panel: the chat scrolls
    app.handle_key(ctrl('u'));
    assert_eq!(app.chat_scroll, 10);
    assert_eq!(app.log_scroll, 0);
    app.handle_key(ctrl('d'));
    assert_eq!(app.chat_scroll, 0);

    let mut app = app_with(state(vec![]));
    app.panel = 5; // log panel: the log stream scrolls, the chat does not
    app.focus = Focus::Panel;
    app.handle_key(ctrl('u'));
    assert_eq!(app.log_scroll, 10);
    assert_eq!(app.chat_scroll, 0);
    app.handle_key(ctrl('d'));
    assert_eq!(app.log_scroll, 0);
}

/// `/foo` and `/settings now` must never reach the Leader: they leave a system
/// note in the chat and keep the draft in the composer for editing.
#[test]
fn unknown_slash_commands_stay_out_of_the_chat() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    for (typed, keeps_draft) in [("/foo", true), ("/settings now", true), ("/set", false)] {
        let mut app = app_with(state(vec![]));
        app.composer.set_text(typed);
        let effects = app.handle_key(enter);
        let sent = effects.iter().find(|e| matches!(e, Effect::UserMessage(_)));
        assert!(sent.is_none(), "{typed} was sent to the Leader: {effects:?}");
        if keeps_draft {
            assert_eq!(app.composer.text(), typed, "{typed} must stay editable in the composer");
            let note = app.chat.last().expect("a system note").clone();
            assert_eq!(note.0, "system");
            assert!(note.1.contains(typed) || note.1.contains("Unknown command"), "note: {note:?}");
        } else {
            // a known command still runs from the composer
            assert!(app.settings_open, "/set must complete to /settings and open the overlay");
            assert_eq!(app.composer.text(), "");
        }
    }
}

/// Bracketed paste drops the whole clipboard into the composer: newlines become
/// soft breaks and nothing is submitted to the Leader.
#[test]
fn paste_fills_the_composer_without_submitting() {
    let mut app = app_with(state(vec![]));
    app.handle_paste("first line\nsecond line\r\nthird");
    assert_eq!(app.composer.text(), "first line\nsecond line\nthird");
    assert!(app.chat.is_empty(), "a paste must not answer for the user");
    // the draft is what Enter would send later
    assert_eq!(app.composer.submit().as_deref(), Some("first line\nsecond line\nthird"));
}

/// The wheel moves the table selection wherever the focus is — in particular it
/// must not fall through to the composer's history recall.
#[test]
fn table_selection_moves_without_touching_the_composer() {
    use teamagents_tui::app::Focus;
    let mut app = app_with(state(vec![])); // team panel: leader, worker
    app.focus = Focus::Composer;
    app.composer.record_submission("an older message");
    app.move_table_selection(1);
    assert_eq!(
        app.table_cursors.get("team").and_then(|(k, _)| k.clone()).as_deref(),
        Some("worker"),
        "the wheel moves the row selection"
    );
    assert_eq!(app.log_member.as_deref(), Some("worker"), "highlighting a member filters the log");
    assert_eq!(app.composer.text(), "", "the wheel must not recall the composer history");
    app.move_table_selection(-5);
    assert_eq!(
        app.table_cursors.get("team").and_then(|(k, _)| k.clone()).as_deref(),
        Some("leader"),
        "the selection clamps at the first row"
    );
    app.move_table_selection(5);
    assert_eq!(
        app.table_cursors.get("team").and_then(|(k, _)| k.clone()).as_deref(),
        Some("worker"),
        "the selection clamps at the last row"
    );
}

#[test]
fn tab_enters_the_panel_from_any_tab() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use teamagents_tui::app::{Focus, PANELS};
    let mut app = app_with(state(vec![]));
    for (idx, name) in PANELS.iter().enumerate() {
        app.panel = idx;
        app.focus = Focus::Composer;
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Panel, "Tab must enter the {name} panel");
    }
}

#[test]
fn log_panel_up_down_cycle_the_member_filter() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use teamagents_tui::app::{Focus, PANELS};
    let mut app = app_with(state(vec![])); // team: leader, worker
    app.panel = PANELS.iter().position(|p| *p == "log").unwrap();
    app.focus = Focus::Panel;
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.log_member.as_deref(), Some("leader"), "Down starts at the first member");
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.log_member.as_deref(), Some("worker"));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.log_member, None, "Down wraps back to the unfiltered stream");
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.log_member.as_deref(), Some("worker"), "Up wraps to the last member");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.log_member, None, "Enter clears the filter");
}

#[test]
fn delta_truncation_respects_char_boundaries() {
    // P1-4: 12000 汉字 = 36000 bytes; the 32000-byte cap used to split a UTF-8
    // sequence and panic in drain()
    let mut app = App::new("s1", json!({}), "/tmp/cfg".into(), "zh-CN", true, vec![]);
    app.on_delta("r1", "leader", &"汉".repeat(12000));
    assert!(app.delta_buffers["r1"].len() <= 32000);
    for _ in 0..5 {
        app.on_delta("r1", "leader", &"汉".repeat(4000));
    }
    assert!(app.delta_buffers["r1"].len() <= 32000);
}

#[test]
fn approval_arrival_toasts_against_new_snapshot() {
    // P2-10: the event and its PENDING row arrive in the same snapshot; the old
    // snapshot does not know the approval yet
    let mut app = App::new("s1", json!({}), "/tmp/cfg".into(), "en", true, vec![]);
    app.apply_state(&json!({"spec": {}, "session": {}, "events": [], "runs": [], "pending_approvals": []}));
    let ev = json!({"sequence": 1, "kind": "approval_requested", "actor_id": "worker",
        "payload": {"approval_id": "ap1", "agent_id": "worker",
                    "scope": {"tool": "shell", "args": {"cmd": "rm -rf x"}}}});
    let st = json!({"spec": {}, "session": {}, "events": [ev], "runs": [],
        "pending_approvals": [{"approval_id": "ap1", "status": "PENDING"}]});
    let effects = app.apply_state(&st);
    assert!(effects.iter().any(|e| matches!(e, Effect::Bell)));
    assert_eq!(app.toasts.len(), 1);
}

#[test]
fn approval_already_decided_stays_quiet() {
    // same event, but the new snapshot no longer lists it PENDING: no toast/bell
    let mut app = App::new("s1", json!({}), "/tmp/cfg".into(), "en", true, vec![]);
    app.apply_state(&json!({"spec": {}, "session": {}, "events": [], "runs": [], "pending_approvals": []}));
    let ev = json!({"sequence": 1, "kind": "approval_requested", "actor_id": "worker",
        "payload": {"approval_id": "ap1", "agent_id": "worker",
                    "scope": {"tool": "shell", "args": {"cmd": "rm -rf x"}}}});
    let st = json!({"spec": {}, "session": {}, "events": [ev], "runs": [],
        "pending_approvals": [{"approval_id": "ap1", "status": "APPROVED"}]});
    let effects = app.apply_state(&st);
    assert!(!effects.iter().any(|e| matches!(e, Effect::Bell)));
    assert!(app.toasts.is_empty());
}

#[test]
fn click_hits_right_row_after_task_reorder() {
    // P2-11: rows reordered by a state refresh; the click must resolve the saved
    // key to its current index, exactly like the renderer does
    let task = |id: &str, created: f64| json!({
        "task_id": id, "parent_task_id": null, "goal_id": "g1",
        "requester": "leader", "assignee": "worker", "description": id,
        "acceptance": "", "dependencies": [], "status": "PENDING",
        "result_refs": [], "created_at": created
    });
    let mut st = state(vec![]);
    st["tasks"] = json!([task("task-a", 1000.0), task("task-b", 1001.0)]);
    let mut app = app_with(st);
    app.panel = 1; // tasks
    // newest first: [task-b, task-a]; the user selects row 1 (task-a)
    assert_eq!(app.panel_row_keys("tasks"), vec!["task-b".to_string(), "task-a".to_string()]);
    app.select_row(1);
    let mut st2 = state(vec![]);
    st2["tasks"] = json!([task("task-a", 1002.0), task("task-b", 1001.0)]);
    app.apply_state(&st2);
    assert_eq!(app.panel_row_keys("tasks"), vec!["task-a".to_string(), "task-b".to_string()]);
    // one visible row; the renderer shows task-a on it
    app.select_row_visible(0, 1);
    let selected = app.table_cursors.get("tasks").and_then(|(k, _)| k.clone());
    assert_eq!(selected.as_deref(), Some("task-a"));
}

#[test]
fn shared_panel_scrolls_with_stable_keys() {
    // P2-13: shared rows carry `space_id:sequence` keys so wheel / ↑↓ work
    let mut app = app_with(state(vec![]));
    app.shared = (1..=30)
        .map(|i| json!({"space_id": "main", "author": "leader", "kind": "note",
                        "content": format!("entry {i}"), "sequence": i}))
        .collect();
    let keys = app.panel_row_keys("shared");
    assert_eq!(keys.len(), 30);
    assert_eq!(keys[0], "main:1");
    assert_eq!(keys[29], "main:30");
    app.panel = teamagents_tui::app::PANELS.iter().position(|p| *p == "shared").unwrap();
    app.move_table_selection(1);
    assert_eq!(app.table_cursors.get("shared").cloned(), Some((Some("main:2".into()), 1)));
    app.move_table_selection(100);
    assert_eq!(app.table_cursors.get("shared").cloned(), Some((Some("main:30".into()), 29)));
    app.move_table_selection(-100);
    assert_eq!(app.table_cursors.get("shared").cloned(), Some((Some("main:1".into()), 0)));
}

#[test]
fn status_slash_command_renders_usage_report() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app_with(state(vec![]));
    for c in "/status".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let fx = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(fx.as_slice(), [Effect::UsageStatus]), "{fx:?}");

    let report = json!({"session_id": "s1", "agents": [
        {"agent_id": "leader", "name": "Leader", "model_profile": "leader_main", "model": "gpt-5",
         "context_window": 128000,
         "usage": {"calls": 4, "prompt_tokens": 30000, "completion_tokens": 2000,
                   "total_tokens": 32000, "last_prompt_tokens": 12000}},
        {"agent_id": "worker", "name": "W", "model_profile": "worker_main", "model": null,
         "context_window": null, "usage": null},
    ]});
    app.show_usage(Ok(report));
    let text = app.chat.last().unwrap().1.clone();
    assert!(text.contains("Token usage"), "{text}");
    assert!(text.contains("Leader | gpt-5 | window 128000 | 32000 (30000/2000) | remaining 116000"), "{text}");
    assert!(text.contains("W | Not configured | window Not configured | 0 (0/0) | remaining Not configured"), "{text}");

    // zh-CN rendering
    let mut zh = App::new("s1", json!({"models": {}}), "/tmp/cfg".into(), "zh-CN", true, vec![]);
    zh.show_usage(Err("boom".into()));
    assert!(zh.chat.last().unwrap().1.contains("获取用量失败"), "{}", zh.chat.last().unwrap().1);
}

#[test]
fn status_report_visible_in_rendered_frame() {
    let mut app = app_with(state(vec![]));
    app.show_usage(Ok(json!({"session_id": "s1", "agents": [
        {"agent_id": "leader", "name": "Leader", "model_profile": "leader_main", "model": "gpt-5",
         "context_window": 128000,
         "usage": {"calls": 1, "prompt_tokens": 10, "completion_tokens": 5,
                   "total_tokens": 15, "last_prompt_tokens": 10}},
    ]})));
    let backend = ratatui::backend::TestBackend::new(120, 30);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let buf = terminal.backend().buffer();
    let text: String = (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Leader | gpt-5 | window 128000 | 15 (10/5) | remaining 127990"), "frame missing usage line:\n{text}");
}

#[test]
fn model_slash_command_lists_effective_models() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app_with(state(vec![]));
    for c in "/model".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let fx = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(fx.as_slice(), [Effect::ModelStatus]), "{fx:?}");

    app.show_models(Ok(json!({"session_id": "s1", "agents": [
        {"agent_id": "leader", "name": "Leader", "model_profile": "leader_main",
         "model": "gpt-5", "effort": "high", "overridden": true},
        {"agent_id": "worker", "name": "W", "model_profile": "worker_main",
         "model": "gpt-default", "effort": null, "overridden": false},
    ]})));
    let text = app.chat.last().unwrap().1.clone();
    assert!(text.contains("Member models"), "{text}");
    assert!(text.contains("Leader | model gpt-5 | effort high*"), "{text}");
    assert!(text.contains("W | model gpt-default | effort Not configured"), "{text}");
    assert!(app.composer_title().contains("gpt-5 · high *"), "the selected model appears in the composer title");

    // zh-CN rendering
    let mut zh = App::new("s1", json!({"models": {}}), "/tmp/cfg".into(), "zh-CN", true, vec![]);
    zh.show_models(Ok(json!({"agents": [
        {"agent_id": "leader", "name": "Leader", "model": "gpt-5", "effort": "low", "overridden": true},
    ]})));
    let text = zh.chat.last().unwrap().1.clone();
    assert!(text.contains("成员模型"), "{text}");
    assert!(text.contains("Leader | 模型 gpt-5 | 档位 low*"), "{text}");
}

#[test]
fn model_slash_command_with_args_sets_or_clears() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let type_and_enter = |app: &mut App, command: &str| {
        for c in command.chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    };

    let mut app = app_with(state(vec![]));
    let fx = type_and_enter(&mut app, "/model worker gpt-5 high");
    assert!(
        matches!(fx.as_slice(), [Effect::SetModel { agent_id, model, effort, .. }]
            if agent_id == "worker" && model.as_deref() == Some("gpt-5") && effort.as_deref() == Some("high")),
        "{fx:?}"
    );

    let mut app = app_with(state(vec![]));
    let fx = type_and_enter(&mut app, "/model worker clear");
    assert!(
        matches!(fx.as_slice(), [Effect::SetModel { agent_id, model: None, effort: None, .. }] if agent_id == "worker"),
        "{fx:?}"
    );

    // a bare member name is a usage hint, never a message to the Leader
    let mut app = app_with(state(vec![]));
    let fx = type_and_enter(&mut app, "/model worker");
    assert!(fx.is_empty(), "{fx:?}");
    assert!(app.chat.last().unwrap().1.contains("Usage: /model"), "{}", app.chat.last().unwrap().1);

    // result rendering: switched / restored / failed
    app.show_model_set(Ok(json!({"agent_id": "worker", "model": "gpt-5", "effort": "high", "overridden": true})));
    assert!(app.chat.last().unwrap().1.contains("Switched worker: model gpt-5 · effort high"), "{}", app.chat.last().unwrap().1);
    app.show_model_set(Ok(json!({"agent_id": "worker", "model": "gpt-default", "effort": "medium", "overridden": false})));
    assert!(app.chat.last().unwrap().1.contains("Restored worker to the profile default: model gpt-default · effort medium"), "{}", app.chat.last().unwrap().1);
    app.show_model_set(Err("boom".into()));
    assert!(app.chat.last().unwrap().1.contains("Model switch failed: boom"), "{}", app.chat.last().unwrap().1);
}

#[test]
fn model_picker_selects_members_profiles_effort_and_restores_defaults() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let key = |app: &mut App, code| app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    let report = json!({"leader_id":"leader", "agents":[
        {"agent_id":"worker", "name":"W", "runtime_kind":"deepagents", "provider":"old", "model_profile":"old"},
        {"agent_id":"leader", "name":"Leader", "runtime_kind":"deepagents", "provider":"old", "model_profile":"old"}
    ], "profiles":[
        {"id":"old", "provider":"old", "model":"old-model", "protocol":"openai", "efforts":["low","high"]},
        {"id":"new", "provider":"vendor", "model":"chosen-model", "protocol":"openai", "efforts":["low","high","max"]}
    ]});
    for (member_index, member) in [(0,"leader"), (1,"worker")] {
        let mut app = app_with(state(vec![]));
        app.show_models(Ok(report.clone()));
        for _ in 0..member_index { key(&mut app, KeyCode::Down); }
        assert!(key(&mut app, KeyCode::Enter).is_empty());
        for c in "vendor".chars() { key(&mut app, KeyCode::Char(c)); }
        key(&mut app, KeyCode::Enter); // provider
        key(&mut app, KeyCode::Enter); // model
        app.handle_paste("high");
        assert!(app.composer.text().is_empty(), "picker paste must not enter the composer");
        for _ in 0..10 { key(&mut app, KeyCode::Down); }
        assert_eq!(app.model_picker.as_ref().unwrap().index, 0, "filtered selection stays in bounds");
        let effects = key(&mut app, KeyCode::Enter);
        assert!(matches!(effects.as_slice(), [Effect::SetModel { agent_id, profile, model: None, effort }]
            if agent_id == member && profile.as_deref() == Some("new") && effort.as_deref() == Some("high")));
        assert!(app.model_picker.is_none());
    }
    let mut app = app_with(state(vec![]));
    app.show_models(Ok(report.clone()));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Esc); // back to members, without interrupting leader
    assert!(app.model_picker.is_some());
    assert!(key(&mut app, KeyCode::Esc).is_empty());
    assert!(app.model_picker.is_none());
    app.show_models(Ok(report));
    key(&mut app, KeyCode::Enter);
    for _ in 0..10 { key(&mut app, KeyCode::Down); }
    let effects = key(&mut app, KeyCode::Enter);
    assert!(matches!(effects.as_slice(), [Effect::SetModel { agent_id, profile: None, model: None, effort: None }] if agent_id == "leader"));
}

#[test]
fn dynamic_models_merge_without_moving_selection_or_reviving_closed_pickers() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let key = |app: &mut App, code| app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    let report = json!({"agents":[{"agent_id":"leader", "name":"Leader"}], "profiles":[
        {"id":"p", "provider":"local", "model":"configured", "protocol":"openai", "efforts":["low","high"]}
    ]});
    let remote = json!({"models":[
        {"id":"p", "provider":"local", "model":"online", "protocol":"openai", "discovered":true,"efforts":["low","high"]}
    ],"errors":[]});
    let mut app = app_with(state(vec![]));
    app.show_models(Ok(report.clone()));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(key(&mut app, KeyCode::Enter).as_slice(), [Effect::DiscoverModels {provider}] if provider == "local"));
    let generation = app.model_generation;
    app.show_discovered_models("old-session", generation, "local", Ok(remote.clone()));
    assert_eq!(app.model_picker.as_ref().unwrap().options("en").len(), 1);
    app.show_discovered_models("s1", generation, "local", Ok(remote.clone()));
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(picker.options("en").len(), 2);
    assert_eq!(picker.index, 0);
    assert!(!picker.loading);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    app.handle_paste("high");
    let selected = key(&mut app, KeyCode::Enter);
    assert!(matches!(selected.as_slice(), [Effect::SetModel {profile, model, effort, ..}]
        if profile.as_deref() == Some("p") && model.as_deref() == Some("online") && effort.as_deref() == Some("high")));
    app.show_discovered_models("s1", generation, "local", Ok(remote.clone()));
    assert!(app.model_picker.is_none());
    app.show_models(Ok(report));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    app.show_discovered_models("s1", generation, "local", Ok(remote));
    assert_eq!(app.model_picker.as_ref().unwrap().options("en").len(), 1, "old responses must not alter a new picker");
    app.show_discovered_models("s1", app.model_generation, "local", Err("HTTP 403".into()));
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(picker.options("en").len(), 1);
    assert!(picker.notice.contains("configured models remain available") && picker.notice.contains("HTTP 403"));
}


#[test]
fn rewind_slash_command_lists_and_picks_points() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app_with(state(vec![]));
    let type_enter = |app: &mut App, text: &str| -> Vec<Effect> {
        for c in text.chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    };
    // bare /rewind asks for the points list
    assert!(matches!(type_enter(&mut app, "/rewind").as_slice(), [Effect::RewindPoints]));
    app.show_rewind_points(Ok(json!({"agent_id": "leader", "thread": "ctx:leader:1", "points": [
        {"id": "n3", "depth": 1, "preview": "第二问"},
        {"id": "n1", "depth": 3, "preview": "第一问"},
    ]})));
    // numeric pick resolves through the shown list; 0 empties the conversation
    assert!(matches!(type_enter(&mut app, "/rewind 2").as_slice(), [Effect::Rewind { node }] if node.as_deref() == Some("n1")));
    assert!(matches!(type_enter(&mut app, "/rewind 0").as_slice(), [Effect::Rewind { node: None }]));
    // out-of-range index stays put with a hint, no effect
    assert!(type_enter(&mut app, "/rewind 9").is_empty());
    // a bare node id also works
    assert!(matches!(type_enter(&mut app, "/rewind n3").as_slice(), [Effect::Rewind { node }] if node.as_deref() == Some("n3")));
    app.show_rewind_done(Ok(json!({"depth": 3})));
}

#[test]
fn fork_slash_command_resets_local_state() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app_with(state(vec![]));
    for c in "/fork".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let fx = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(fx.as_slice(), [Effect::Fork]), "{fx:?}");
    app.show_fork_done(Ok(json!({"session_id": "s2", "forked_from": "s1", "catalog": {}, "user_config_path": ""})));
    assert_eq!(app.session_id, "s2");
    app.show_fork_done(Err("有回合进行中".into()));
}
