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
        "tasks": [],
        "pending_approvals": [],
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

#[test]
fn full_frame_shows_all_regions() {
    let mut app = sample_app();
    let backend = TestBackend::new(100, 40);
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
    assert!(text.contains("build me a thing"), "user text missing");
    assert!(text.contains("• Leader"), "leader label missing");
    assert!(text.contains("on it"), "leader text missing");
    // composer: status, prefix, hint
    assert!(text.contains("Leader / leader_main"), "composer status missing");
    assert!(text.contains("Enter send · Shift+Enter / Ctrl+J newline"), "hint missing");
    // activity line
    assert!(text.contains("Ready"), "activity missing");
    assert!(text.contains("Latest:"), "latest activity missing");
    // footer keys
    assert!(text.contains("ctrl+q"), "footer missing");
    assert!(text.contains("Quit"), "footer labels missing");
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
