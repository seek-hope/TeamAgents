//! Full-frame render smoke on ratatui's TestBackend: the layout must show the
//! status bar, tab row, active table, chat, composer and footer keys.

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::{json, Value as Json};
use teamagents_tui::app::App;
use teamagents_tui::ui;

fn sample_app() -> App {
    let mut app = App::new(
        "proj_abc123",
        json!({"models": {"leader_main": {"provider": "openai", "model": "gpt-5", "api_key_env": "OPENAI_API_KEY"}},
               "tools": {}, "skills_paths": [], "instruction_files": []}),
        "/home/u/.config/teamagents/config.toml".into(),
        "en",
        true,
        vec![],
    );
    let st = json!({
        "session": {"session_id": "proj_abc123", "status": "ACTIVE", "cwd": "/repo",
                    "permissions_mode": "approved_scope", "goal_id": "g1", "goal_state": "in_progress"},
        "spec": {"leader_id": "leader", "agents": [
            {"id": "leader", "name": "Leader", "role": "leader", "runtime_kind": "deepagents",
             "instructions": "", "model_profile": "leader_main", "tool_bindings": [],
             "workspace_policy": "shared"}],
            "channels": [], "observers": [],
            "shared_spaces": [{"id": "main", "readers": ["leader"], "writers": ["leader"]}]},
        "leader_id": "leader",
        "limits": {"max_parallel_workers": 8, "max_members": 20, "max_turns_per_goal": 1000,
                   "max_model_steps_per_turn": 200, "turn_active_timeout_s": 1200,
                   "cancel_confirm_timeout_s": 60},
        "revision": 1,
        "agents": [{"id": "leader", "status": "IDLE"}],
        "runs": [],
        "tasks": [
            {"task_id": "task_11112222", "parent_task_id": null, "goal_id": null,
             "requester": "leader", "assignee": "researcher", "description": "调研协作文档并给出三条改进建议",
             "acceptance": "三条建议", "dependencies": [], "status": "RUNNING", "result_refs": [],
             "created_at": 1757740000.0, "updated_at": 1757740000.0},
            {"task_id": "task_33334444", "parent_task_id": null, "goal_id": null,
             "requester": "leader", "assignee": "coder", "description": "实现输入历史持久化",
             "acceptance": "重启后仍可调取", "dependencies": ["task_11112222"], "status": "PENDING",
             "result_refs": [], "created_at": 1757740100.0, "updated_at": 1757740100.0}
        ],
        "pending_approvals": [
            {"approval_id": "appr_abcdef1234567890", "session_id": "s1", "agent_id": "coder",
             "run_id": "run_coder_1", "tool_call_id": "call_1", "operation_hash": "deadbeefdeadbeefdeadbeefdeadbeef",
             "requested_scope": {"tool": "shell", "args": {"command": "curl https://example.com", "network": true},
                                 "reason": "shell network access is off by default"},
             "policy_revision": 1, "status": "PENDING", "created_at": 1757740200.0, "decided_at": null}
        ],
        "events": [
            {"sequence": 1, "kind": "user_message", "actor_id": "user", "payload": {"text": "build me a thing"}},
            {"sequence": 2, "kind": "leader_reply", "actor_id": "leader", "payload": {"text": "on it", "run_id": "r"}},
        ],
    });
    app.apply_state(&st);
    app
}

fn frame_text(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        let mut skip_next = false;
        for x in 0..area.width {
            if skip_next { // continuation cell of a wide char (holds a space)
                skip_next = false;
                continue;
            }
            let sym = buf[(x, y)].symbol();
            out.push_str(sym);
            skip_next = unicode_width::UnicodeWidthStr::width(sym) == 2;
        }
        out.push('\n');
    }
    out
}

/// Parity tool: with TEAMAGENTS_DUMP_FRAME=<path> and TEAMAGENTS_DUMP_SIZE=WxH
/// this writes the rendered frame as plain text so it can be diffed against the
/// Python (Textual) TUI. See review/tmp/dump_py_frame.py.
fn dump_frame(app: &mut App, name: &str) {
    let Ok(path) = std::env::var("TEAMAGENTS_DUMP_FRAME") else { return };
    if let Ok(panel) = std::env::var("TEAMAGENTS_DUMP_PANEL") {
        app.panel = panel.parse().unwrap_or(0);
    }
    if let Ok(lang) = std::env::var("TEAMAGENTS_DUMP_LANG") {
        app.lang = if lang == "zh-CN" { "zh-CN" } else { "en" };
    }
    let size = std::env::var("TEAMAGENTS_DUMP_SIZE").unwrap_or_else(|_| "110x32".into());
    let (w, h) = size.split_once('x').map(|(w, h)| (w.parse().unwrap_or(110), h.parse().unwrap_or(32))).unwrap_or((110, 32));
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, app)).unwrap();
    let target = if name.is_empty() { path } else { format!("{path}/{name}.txt") };
    std::fs::write(target, frame_text(terminal.backend().buffer())).unwrap();
}

/// The same scenario the Python dump script renders: one JSON file feeds both
/// sides (review/tmp/parity_scenario.json) so the frames stay comparable.
fn scenario() -> Json {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../review/tmp/parity_scenario.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("parity scenario")).expect("json")
}

fn parity_app() -> App {
    let scenario = scenario();
    let mut app = App::new(
        "s1",
        json!({"models": {"test": {"provider": "openai", "model": "test", "api_key_env": ""}},
               "tools": {}, "skills_paths": [], "instruction_files": []}),
        "/home/u/.config/teamagents/config.toml".into(),
        "en",
        true,
        vec![],
    );
    app.apply_state(&json!({
        "session": scenario["session"],
        "spec": scenario["spec"],
        "leader_id": scenario["spec"]["leader_id"],
        "limits": scenario["limits"],
        "revision": 1,
        "agents": scenario["agents_status"],
        "runs": scenario["runs"],
        "tasks": scenario["tasks"],
        "pending_approvals": scenario["pending_approvals"],
        "events": scenario["events"],
    }));
    app.composer.set_text(scenario["composer_text"].as_str().unwrap_or(""));
    // main.rs replays the log when the log tab is active; the dump does the same
    app.replay_log(scenario["events"].as_array().cloned().unwrap_or_default().as_slice());
    app.shared = scenario["shared_entries"].as_array().cloned().unwrap_or_default();
    app.sessions = scenario["sessions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    app
}

#[test]
fn frame_dump_for_parity() {
    let mut app = parity_app();
    dump_frame(&mut app, "");
}

#[test]
fn full_frame_shows_all_regions() {
    let mut app = sample_app();
    let backend = TestBackend::new(120, 40); // wide enough for the whole footer
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let text = frame_text(terminal.backend().buffer());

    // status bar
    assert!(text.contains("TeamAgents · proj_abc123"), "status bar missing");
    assert!(text.contains("Pre-authorized"), "mode missing");
    // tab row (all seven panels, active translated)
    for label in ["Team", "Tasks", "Shared", "Approvals", "Sessions", "Log", "Settings"] {
        assert!(text.contains(label), "tab {label} missing");
    }
    // team table header + row
    assert!(text.contains("Member"), "team header missing");
    assert!(text.contains("leader"), "team row missing");
    assert!(text.contains("leader_main"), "model missing");
    // chat: user + leader entries
    assert!(text.contains("› You"), "user label missing");
    assert!(text.contains("╸"), "active tab underline missing");
    assert!(text.contains("\n  Member"), "table padding missing");
    assert!(text.contains("build me a thing"), "user text missing");
    assert!(text.contains("• Leader"), "leader label missing");
    assert!(text.contains("on it"), "leader text missing");
    // composer: status, prefix, hint
    assert!(text.contains("Leader / leader_main"), "composer status missing");
    assert!(text.contains("Enter send · Shift+Enter / Ctrl+J newline"), "hint missing");
    // activity line
    assert!(text.contains("Ready"), "activity missing");
    assert!(text.contains("Latest:"), "latest activity missing");
    // footer keys (Textual chips: ^q / ^p / ^f ...)
    assert!(text.contains("^q Quit"), "footer missing");
    assert!(text.contains("^p Pause/resume"), "footer pause missing");
    assert!(text.contains("esc Stop Leader"), "footer stop missing");
    assert!(text.contains("^j Newline"), "footer newline missing");
}

#[test]
fn zh_frame_uses_message_ids() {
    let mut app = sample_app();
    app.lang = "zh-CN";
    let backend = TestBackend::new(100, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let text = frame_text(terminal.backend().buffer());
    for label in ["团队", "任务", "共享空间", "批准", "会话", "日志", "设置"] {
        assert!(text.contains(label), "zh tab {label} missing");
    }
    assert!(text.contains("预授权"), "zh mode missing");
    assert!(text.contains("› 你"), "zh user label missing");
    assert!(text.contains("退出"), "zh footer missing");
}

#[test]
fn frame_matches_textual_layout_details() {
    // Details verified against the Textual frame (review/tmp/diff_frames.py):
    // #side/#chat padding, the ContentTabs rule with the active-tab underline,
    // DataTable's extra cell pad, and the wrapping panel hint.
    let mut app = parity_app();
    let backend = TestBackend::new(110, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let text = frame_text(terminal.backend().buffer());
    let lines: Vec<&str> = text.split('\n').collect();
    assert!(lines[0].contains("TeamAgents · s1 | Pre-authorized | Approvals 1 | Tasks 2"), "{:?}", lines[0]);
    assert!(lines[2].starts_with("  Team  Tasks"), "{:?}", lines[2]);
    assert!(lines[3].starts_with(" ╸━━━━╺━━━"), "{:?}", lines[3]);
    assert!(lines[4].starts_with("  Member"), "{:?}", lines[4]);
    assert!(lines[5].starts_with("  leader"), "{:?}", lines[5]);
    assert!(lines[13].starts_with(" ○ Ready · No turns executing"), "{:?}", lines[13]);
    assert!(lines[28].starts_with(" ›  请继续验证"), "{:?}", lines[28]);
    assert!(lines[31].starts_with(" ^j Newline  ^q Quit"), "{:?}", lines[31]);
}

#[test]
fn python_repr_and_json_dumps_match_the_panels() {
    // approvals.py uses str(args) (Python repr); panels.py logs json.dumps() text
    let args = json!({"command": "curl https://example.com", "network": true});
    assert_eq!(
        teamagents_tui::app::py_repr(&args),
        "{'command': 'curl https://example.com', 'network': True}"
    );
    assert_eq!(
        teamagents_tui::app::py_json_dumps(&json!({"text": "审查", "ok": true})),
        "{\"text\": \"\\u5ba1\\u67e5\", \"ok\": true}"
    );
}

#[test]
fn streaming_preview_renders_markdown_bounded() {
    let mut app = sample_app();
    app.on_delta("r1", "leader", "# Heading\nsome **bold** text");
    for _ in 0..3 {
        app.flush_deltas();
        std::thread::sleep(std::time::Duration::from_millis(210));
    }
    let backend = TestBackend::new(100, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let text = frame_text(terminal.backend().buffer());
    assert!(text.contains("Heading"), "live markdown missing");
}

#[test]
fn debug_row_indent() {
    let mut app = parity_app();
    let backend = TestBackend::new(110, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let text = frame_text(terminal.backend().buffer());
    for (i, line) in text.split('\n').enumerate().skip(3).take(6) {
        println!("{i:02}|{line}");
    }
}
