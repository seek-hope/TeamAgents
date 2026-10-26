use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use serde_json::json;
use teamagents_tui::{
    v2app::{Focus, InstanceInfo, ModelInfo, V2App, V2Effect},
    v2ui,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}
fn app() -> V2App {
    let mut app = V2App::new("minimal");
    app.apply_checkpoint(
        json!({"instances": [
            {"id": "a-worker", "lifecycle": "ACTIVE", "phase": "READY"},
            {"id": "i-leader", "lifecycle": "ACTIVE", "phase": "MODEL_PENDING"}
        ]}),
        1,
    );
    app
}

#[test]
fn input_receipts_survive_refresh_and_follow_authoritative_delivery() {
    let mut app = app();
    app.input_receipt("env-visible", &json!({"queued":true}));
    app.apply_checkpoint(json!({"input_delivery":[{"id":"env-visible","state":"ACCEPTED"}]}), 2);
    assert!(app.entries.iter().any(|e| e.text.contains("env-visible") && e.text.contains("queued")));
    app.apply_history(json!({"entries":[{"kind":"user","envelope_id":"env-visible",
        "message":{"role":"user","content":"work"}}]}));
    assert!(app.entries.iter().any(|e| e.text.contains("env-visible") && e.text.contains("received into")));
    app.input_receipt("env-cancelled", &json!({"deferred":true,"queued":true}));
    assert!(app.entries.iter().any(|e| e.text.contains("waiting for the current turn")));
    app.apply_checkpoint(json!({"input_delivery":[{"id":"env-cancelled","state":"CANCELLED"}]}), 3);
    assert!(app.entries.iter().any(|e| e.text.contains("env-cancelled") && e.text.contains("cancelled")));
    app.input_receipt("env-old", &json!({"queued":true}));
    app.apply_checkpoint(json!({"input_delivery":[{"id":"env-old","state":"SUPERSEDED"}]}), 4);
    assert!(app.entries.iter().any(|e| e.text.contains("env-old") && e.text.contains("undelivered")));
    let refresh = app.apply_events(&[json!({"kind":"input_cancelled","sequence":5,"payload":{}})]);
    assert!(refresh.checkpoint);
    app.input_receipt("env-pending", &json!({"queued":true}));
    app.apply_history(json!({"entries":[
        {"kind":"user","envelope_id":"env-visible","message":{"content":"work"}},
        {"kind":"assistant","message":{"content":"latest answer"}}
    ]}));
    let position = |needle: &str| app.entries.iter().position(|entry| entry.text.contains(needle)).unwrap();
    assert!(position("work") < position("env-visible"));
    assert!(position("env-visible") < position("latest answer"));
    assert!(
        position("env-cancelled") < position("latest answer"),
        "off-page receipts must not hide the latest response"
    );
    assert!(position("env-pending") > position("latest answer"), "pending input still needs current feedback");
}

#[test]
fn help_reports_live_environment_without_changing_drafts_or_permissions() {
    let mut app = app();
    app.apply_checkpoint(
        json!({"environment":{
            "workspace":"/project","sandbox":"sbx","permissions":"approved_scope",
            "configuration":{"restart_required":true},
            "leader_instruction_load":{"epoch":0,"observed_at":123,"files":[
                {"path":"/project/AGENTS.md","status":"loaded","bytes":100},
                {"path":"/missing.md","status":"unreadable","reason":"No such file"}]},
            "sources":{"instruction_files":["/project/AGENTS.md"],"skills_paths":["/skills"]}
        }}),
        2,
    );
    app.composer.set_text("/help");
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    let text = app.entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>().join("\n");
    for needle in [
        "Workspace: /project",
        "sbx (approved_scope)",
        "Configuration changed",
        "Last Leader prompt assembly",
        "/missing.md: unreadable (No such file)",
        "/project/AGENTS.md",
        "/skills",
        "Cancelling a task does not stop",
        "doctor environment",
    ] {
        assert!(text.contains(needle), "{needle}: {text}");
    }
    // Plain rendering on a narrow terminal remains bounded by existing geometry.
    let mut terminal = Terminal::new(TestBackend::new(48, 18)).unwrap();
    terminal.draw(|frame| v2ui::render(frame, &mut app)).unwrap();
}

#[test]
fn interruption_and_exit_are_distinct_and_keep_the_draft() {
    let mut app = app();
    app.composer.set_text("unfinished instruction");
    assert_eq!(app.handle_key(ctrl('c')), Some(V2Effect::Interrupt { instance: "i-leader".into() }));
    assert!(!app.quit);
    assert_eq!(app.composer.text(), "unfinished instruction");
    app.apply_approvals(json!({"approvals": [{"id": "ap-1"}]}));
    app.handle_key(ctrl('a'));
    assert_eq!(app.handle_key(ctrl('c')), Some(V2Effect::Interrupt { instance: "i-leader".into() }));
    assert_eq!(app.handle_key(ctrl('q')), Some(V2Effect::Quit));
    assert!(app.quit);
}

#[test]
fn ctrl_m_opens_the_model_menu_and_enter_changes_the_profile() {
    let mut app = app();
    app.models = vec![ModelInfo {
        key: "leader_main".into(),
        model: "test".into(),
        provider: "compatible".into(),
        efforts: vec![],
    }];
    app.efforts = vec!["low".into(), "high".into()];
    app.apply_checkpoint(
        json!({"instances": [{"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY",
                              "model": "leader_main", "effort": "low"}]}),
        2,
    );
    assert!(app.handle_key(ctrl('m')).is_none());
    assert!(app.profile.is_some(), "Ctrl+M opens the menu");
    assert!(app.handle_key(key(KeyCode::Right)).is_none());
    let effect = app.handle_key(key(KeyCode::Enter));
    match effect {
        Some(V2Effect::SetProfile { instance, model, effort }) => {
            assert_eq!(instance, "i-leader");
            assert_eq!(model.as_deref(), Some("leader_main"));
            assert_eq!(effort.as_deref(), Some("high"));
        }
        other => panic!("{other:?}"),
    }
    assert!(app.profile.is_none(), "Enter closes the menu");
}

#[test]
fn default_effort_menu_includes_xhigh_and_max() {
    let app = V2App::new("minimal");
    assert_eq!(app.efforts, vec!["minimal", "low", "medium", "high", "xhigh", "max"]);
}

#[test]
fn a_missing_effort_falls_back_to_the_nearest_lower_level() {
    let mut app = app();
    app.models = vec![ModelInfo {
        key: "leader_main".into(),
        model: "test".into(),
        provider: "compatible".into(),
        efforts: vec!["high".into(), "max".into()],
    }];
    app.apply_checkpoint(
        json!({"instances": [{"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY",
                              "model": "leader_main", "effort": "xhigh"}]}),
        2,
    );
    assert!(app.handle_key(ctrl('m')).is_none());
    let menu = app.profile.as_ref().expect("the model menu opens");
    assert_eq!(menu.efforts, vec!["high".to_string(), "max".to_string()]);
    assert_eq!(menu.effort_ix, 0, "xhigh is absent, so high is the nearest lower level");
    match app.handle_key(key(KeyCode::Enter)) {
        Some(V2Effect::SetProfile { effort, .. }) => assert_eq!(effort.as_deref(), Some("high")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn switching_models_rebuilds_efforts_and_applies_the_same_fallback() {
    let mut app = app();
    app.models = vec![
        ModelInfo {
            key: "wide".into(),
            model: "wide".into(),
            provider: "compatible".into(),
            efforts: vec!["low".into(), "medium".into(), "high".into(), "xhigh".into(), "max".into()],
        },
        ModelInfo {
            key: "narrow".into(),
            model: "narrow".into(),
            provider: "compatible".into(),
            efforts: vec!["high".into(), "max".into()],
        },
    ];
    app.apply_checkpoint(
        json!({"instances": [{"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY",
                              "model": "wide", "effort": "xhigh"}]}),
        2,
    );
    assert!(app.handle_key(ctrl('m')).is_none());
    assert_eq!(app.profile.as_ref().unwrap().effort_ix, 3);
    assert!(app.handle_key(key(KeyCode::Down)).is_none());
    let menu = app.profile.as_ref().unwrap();
    assert_eq!(menu.model_ix, 1);
    assert_eq!(menu.efforts, vec!["high".to_string(), "max".to_string()]);
    assert_eq!(menu.effort_ix, 0);
    match app.handle_key(key(KeyCode::Enter)) {
        Some(V2Effect::SetProfile { model, effort, .. }) => {
            assert_eq!(model.as_deref(), Some("narrow"));
            assert_eq!(effort.as_deref(), Some("high"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn missing_leader_or_connection_never_redirects_or_discards_input() {
    let mut app = app();
    app.composer.set_text("keep this draft");
    app.mark_disconnected("test disconnect");
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert!(app.handle_key(ctrl('c')).is_none());
    app.mark_connected();
    app.apply_checkpoint(json!({"instances": [{"id": "a-worker", "lifecycle": "ACTIVE"}]}), 2);
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert!(app.handle_key(ctrl('c')).is_none());
    assert_eq!(app.composer.text(), "keep this draft");
    app.composer.set_text("/quit");
    assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::Quit));
}

#[test]
fn retired_commands_are_local_and_approval_arrival_does_not_steal_typing() {
    let mut app = app();
    for command in [
        "/tasks",
        "/instances",
        "/topology",
        "/goals cancel x",
        "/switch other",
        "/mcp",
        "/status",
        "/sessions extra",
        "/help extra",
    ] {
        app.composer.set_text(command);
        assert!(app.handle_key(key(KeyCode::Enter)).is_none(), "{command}");
        assert!(app.entries.last().unwrap().text.contains("teamagents --help"));
    }
    app.composer.set_text("dr");
    app.apply_approvals(json!({"approvals": [{"id": "ap-1"}]}));
    assert_eq!(app.focus, Focus::Composer);
    assert!(app.handle_key(key(KeyCode::Char('a'))).is_none());
    assert_eq!(app.composer.text(), "dra");
    app.handle_key(ctrl('a'));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.focus, Focus::Composer);
    assert_eq!(app.composer.text(), "dra");
}

#[test]
fn every_size_keeps_one_nonoverlapping_conversation_and_conditional_approvals() {
    for pending in [false, true] {
        for width in [0, 1, 2, 8, 40, 80, 119, 140] {
            for height in [0, 1, 2, 4, 8, 12, 24] {
                let mut app = app();
                if pending {
                    app.apply_approvals(json!({"approvals": [{"id": "ap-1"}]}));
                }
                let area = Rect::new(0, 0, width, height);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
                let geo = v2ui::geometry(&app, area);
                let regions = [geo.status, geo.chat, geo.approvals, geo.composer, geo.footer];
                for (i, region) in regions.iter().enumerate() {
                    assert_eq!(region.intersection(area), *region);
                    for other in &regions[i + 1..] {
                        assert!(region.intersection(*other).is_empty());
                    }
                }
                assert_eq!(geo.chat.width, width);
                if !pending {
                    assert_eq!(geo.approvals.height, 0);
                }
                if height >= 12 {
                    assert_eq!(geo.status.height + geo.composer.height + geo.footer.height, 5);
                }
            }
        }
    }
}

#[test]
fn a_waiting_capability_member_is_visible_in_the_status_line() {
    let mut app = V2App::new("minimal");
    app.instances.push(InstanceInfo {
        id: "i-worker".into(),
        lifecycle: "ACTIVE".into(),
        phase: "WAITING_CAPABILITY".into(),
        model: String::new(),
        reason: String::new(),
        effort: String::new(),
    });
    assert!(app.status_line().contains("waiting for capability approval"), "{}", app.status_line());
}

#[test]
fn diagnostics_are_local_visible_and_replaced_on_reconnect() {
    let mut app = app();
    app.composer.set_text("keep typing");
    app.apply_checkpoint(json!({"instances": [
        {"id": "i-leader", "lifecycle": "ACTIVE", "phase": "WAITING"},
        {"id": "worker", "lifecycle": "ACTIVE", "phase": "READY", "diagnosis": {
            "code": "idle_unsettled", "summary": "Assignee idle with RUNNING task(s) not settled.",
            "attention": true, "next_steps": "Use instances send --id INSTANCE --prompt TEXT or tasks cancel --id TASK."}}
    ]}), 20);
    assert_eq!(app.composer.text(), "keep typing");
    assert_eq!(app.focus, Focus::Composer);
    assert!(app.status_line().contains("idle unsettled"));
    let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let frame: String = terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();
    assert!(frame.contains("idle unsettled") && frame.contains("/help"), "{frame}");
    app.composer.set_text("/help");
    assert!(app.handle_key(key(KeyCode::Enter)).is_none(), "help never dispatches recovery");
    let help = &app.entries.last().unwrap().text;
    assert!(help.contains("instances send") && help.contains("tasks cancel") && help.contains("same --state-root"));
    app.mark_disconnected("test");
    assert!(app.status_line().contains("Disconnected"));
    app.mark_connected();
    app.apply_checkpoint(json!({"instances": [{"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY"}]}), 21);
    assert!(!app.status_line().contains("idle unsettled"), "fresh/older-daemon snapshots clear stale diagnostics");
}

#[test]
fn blocked_task_and_wait_diagnostics_refresh_without_stealing_approvals() {
    let mut app = app();
    app.apply_checkpoint(json!({"instances": [{"id": "i-leader", "lifecycle": "ACTIVE", "phase": "WAITING", "diagnosis": {
        "code": "waiting", "summary": "Waiting; no timer is recorded.", "attention": false,
        "next_steps": "Inspect tasks; do not infer deadlock.", "waits": [{"id": "w", "mode": "ALL", "conditions": [], "timer_at": null}]}}],
        "tasks": [{"id": "blocked-task", "status": "BLOCKED", "diagnosis": {
            "code": "blocked", "summary": "Task BLOCKED; does not satisfy a task wait.", "attention": true, "reason": "outcome_unknown",
            "next_steps": "Verify unknown effects before retrying; tasks cancel does not stop commands."}}]}), 2);
    assert!(app.status_line().contains("blocked-task"));
    app.composer.set_text("/help");
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert!(app.entries.last().unwrap().text.contains("outcome_unknown"));
    assert!(app.entries.last().unwrap().text.contains("timer_at=null"));
    app.apply_approvals(json!({"approvals": [{"id": "approval"}]}));
    assert!(app.status_line().contains("Approval needed"));
    assert_eq!(app.focus, Focus::Composer);
    for kind in ["wait_satisfied", "inbox_drained", "approval_denied", "capability_approved"] {
        assert!(app.apply_events(&[json!({"kind": kind, "payload": {}})]).checkpoint, "{kind}");
    }
    assert!(
        app.apply_events(&[json!({"kind":"operation_completed", "payload":{"status":"OUTCOME_UNKNOWN"}})]).checkpoint
    );
}

#[test]
fn ctrl_e_edits_the_current_draft_and_slash_edit_is_local() {
    let mut app = app();
    app.composer.set_text("draft to edit");
    assert_eq!(
        app.handle_key(ctrl('e')),
        Some(V2Effect::EditDraft { initial: "draft to edit".into() }),
        "Ctrl+E hands the current draft to the editor"
    );
    assert_eq!(app.composer.text(), "draft to edit", "the effect does not clear the draft");

    let mut app = V2App::new("minimal");
    app.composer.set_text("draft text /edit");
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(V2Effect::EditDraft { initial: "draft text".into() }),
        "/edit after a draft hands that draft to the editor"
    );
    assert_eq!(app.composer.text(), "", "the edited draft is not left in the composer");
}

#[test]
fn editor_cancel_restores_the_draft_for_the_slash_path() {
    let mut app = V2App::new("minimal");
    app.composer.set_text("draft text /edit");
    let effect = app.handle_key(key(KeyCode::Enter));
    let Some(V2Effect::EditDraft { initial }) = effect else {
        panic!("the /edit suffix must open the editor");
    };
    assert_eq!(app.composer.text(), "", "the effect takes the draft out of the composer");
    app.finish_editor(&initial, Err("editor failed".into()));
    assert_eq!(app.composer.text(), "draft text", "cancel restores the draft");
}
