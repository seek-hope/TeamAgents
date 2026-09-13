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

fn lines_of(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_string).collect()
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
        app.panel = panel.parse::<usize>().unwrap_or(0).min(teamagents_tui::app::PANELS.len() - 1);
    }
    if std::env::var("TEAMAGENTS_DUMP_SETTINGS").is_ok() {
        app.settings_open = true;
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
    assert!(text.contains("TeamAgents  proj_abc123"), "status bar missing");
    assert!(text.contains("Pre-authorized"), "mode missing");
    // tab row: the six panes (Settings moved behind /settings)
    for label in ["Team", "Tasks", "Shared", "Approvals", "Sessions", "Log"] {
        assert!(text.contains(label), "tab {label} missing");
    }
    assert!(!text.contains("▍Settings"), "settings must not be a tab");
    // team table header + row
    assert!(text.contains("Member"), "team header missing");
    assert!(text.contains("leader"), "team row missing");
    assert!(text.contains("leader_main"), "model missing");
    // chat: user + leader entries
    assert!(text.contains("› You"), "user label missing");
    assert!(text.contains("▍"), "active tab marker missing");
    assert!(text.contains("build me a thing"), "user text missing");
    assert!(text.contains("• Leader"), "leader label missing");
    assert!(text.contains("on it"), "leader text missing");
    // composer: title (Leader state/profile), prompt, hint
    assert!(text.contains("Leader / leader_main"), "composer title missing");
    assert!(text.contains("Enter send"), "composer hint missing from the box border");
    // activity line: status chips + the latest-activity text
    assert!(text.contains("Ready"), "activity missing");
    assert!(text.contains("Waiting for input"), "latest activity missing");
    // footer keys (dim chips) + focus label
    assert!(text.contains("^q Quit"), "footer missing");
    assert!(text.contains("^p Pause/resume"), "footer pause missing");
    assert!(text.contains("esc Stop Leader"), "footer stop missing");
    assert!(text.contains("Compose") || text.contains("输入"), "footer focus missing");
}

#[test]
fn zh_frame_uses_message_ids() {
    let mut app = sample_app();
    app.lang = "zh-CN";
    let backend = TestBackend::new(100, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let text = frame_text(terminal.backend().buffer());
    for label in ["团队", "任务", "共享空间", "批准", "会话", "日志"] {
        assert!(text.contains(label), "zh tab {label} missing");
    }
    assert!(text.contains("预授权"), "zh mode missing");
    assert!(text.contains("› 你"), "zh user label missing");
    assert!(text.contains("退出"), "zh footer missing");
}

#[test]
fn frame_shows_the_rust_shell_regions() {
    // The Rust-native shell (D-20): status chips, a sidebar box with tabs and
    // counts, an accented selection, an activity chip line, a rounded composer
    // and a dim footer. Textual parity is no longer a goal.
    let mut app = parity_app();
    app.focus = teamagents_tui::app::Focus::Panel;
    let backend = TestBackend::new(120, 34);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let text = frame_text(terminal.backend().buffer());
    assert!(text.contains("TeamAgents  s1"), "{text}");
    assert!(text.contains("Pre-authorized") || text.contains("预授权"), "mode chip missing");
    assert!(text.contains("Approvals 1"), "approval chip missing");
    assert!(text.contains("Tasks 2"), "task chip missing");
    assert!(text.contains("▍Team"), "active tab marker missing");
    assert!(text.contains("╭"), "sidebar box missing");
    assert!(text.contains("▌"), "selection bar missing");
    assert!(text.contains("○ Ready") || text.contains("Ready"), "activity chips missing");
    // bottom-right is the permission mode in plain dim text (no chip, no pane label)
    let last = lines_of(&text)
        .into_iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default();
    assert!(last.trim_end().ends_with("Pre-authorized"), "mode label missing: {last:?}");
    assert!(!text.contains("pane:"), "the pane label is gone");
    assert!(!text.contains("面板："), "the pane label is gone");
    assert!(text.contains("^q") && text.contains("Quit"), "footer keys missing");
    // the sessions tab carries no count
    let tabs = lines_of(&text)
        .into_iter()
        .find(|l| l.contains("▍Team"))
        .unwrap_or_default();
    assert!(tabs.contains("Sessions") && !tabs.contains("Sessions 1"), "sessions badge: {tabs:?}");
}

#[test]
fn narrow_terminals_stack_the_sidebar_above_the_chat() {
    let mut app = parity_app();
    let backend = TestBackend::new(96, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let text = frame_text(terminal.backend().buffer());
    let lines: Vec<&str> = text.split('\n').collect();
    // sidebar box on top, chat (activity line) below it
    assert!(lines[1].contains("╭"), "sidebar box should start at the top: {:?}", lines[1]);
    let activity = lines.iter().position(|l| l.contains("Ready")).expect("activity line");
    let box_bottom = lines.iter().position(|l| l.contains("╰")).expect("box bottom");
    assert!(activity > box_bottom, "chat sits below the stacked sidebar: {lines:?}");
}

#[test]
fn chat_scroll_and_new_message_marker() {
    let mut app = parity_app();
    for i in 0..40 {
        app.chat.push(("Leader".into(), format!("line {i}")));
    }
    let backend = TestBackend::new(120, 34);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let before = frame_text(terminal.backend().buffer());
    app.scroll_up(4);
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let after = frame_text(terminal.backend().buffer());
    assert_ne!(before, after, "scrolling changes the visible window");
    assert!(after.contains("lines up") || after.contains("已上翻"), "scroll marker missing");
    app.scroll_to_bottom();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    assert_eq!(frame_text(terminal.backend().buffer()), before, "bottom is the pinned default");
}

#[test]
fn empty_panels_state_their_case() {
    let mut app = App::new(
        "s1",
        json!({"models": {}, "tools": {}, "skills_paths": [], "instruction_files": []}),
        "/tmp/config.toml".into(),
        "en",
        true,
        vec![],
    );
    app.apply_state(&json!({
        "session": {"session_id": "s1", "status": "ACTIVE", "cwd": "/tmp",
                    "permissions_mode": "approved_scope", "goal_id": null, "goal_state": "idle"},
        "spec": {"leader_id": "leader", "agents": [{"id": "leader", "name": "L", "role": "leader",
                  "runtime_kind": "deepagents", "model_profile": "m"}], "channels": [], "observers": [],
                  "shared_spaces": []},
        "leader_id": "leader", "revision": 1, "limits": {},
        "agents": [{"id": "leader", "status": "IDLE"}], "runs": [], "tasks": [],
        "pending_approvals": [], "events": [],
    }));
    for (panel, needle) in [(1, "No tasks yet"), (3, "Nothing waiting"), (4, "No sessions")] {
        app.panel = panel;
        let backend = TestBackend::new(120, 34);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::render(f, &mut app)).unwrap();
        let text = frame_text(terminal.backend().buffer());
        assert!(text.contains(needle), "panel {panel} empty state: {text}");
    }
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

#[test]
fn panes_use_the_fixed_top_bottom_split() {
    use teamagents_tui::app::Focus;
    let mut app = parity_app();
    app.focus = Focus::Panel;
    // wide and narrow terminals produce the same pane order: box on top, chat below
    for width in [80u16, 120, 200] {
        let backend = TestBackend::new(width, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::render(f, &mut app)).unwrap();
        let text = frame_text(terminal.backend().buffer());
        let lines: Vec<&str> = text.split('\n').collect();
        let box_top = lines.iter().position(|l| l.contains('╭')).expect("panel box");
        let box_bottom = lines.iter().position(|l| l.contains('╰')).expect("box bottom");
        let activity = lines.iter().position(|l| l.contains("Ready")).expect("activity");
        assert!(box_top < box_bottom && box_bottom < activity, "layout at {width}: {lines:?}");
        assert!(lines[box_top].starts_with('╭'), "the box starts at column 0 at {width}");
    }
    // the tab strip is a single row in every case
    let widths = [6usize, 8, 9, 12, 9, 4];
    assert_eq!(ui::tab_window(&widths, 5, 100), (0, 5), "everything fits");
    let (lo, hi) = ui::tab_window(&widths, 5, 20);
    assert_eq!(hi, 5, "the active tab is always shown");
    assert!(lo > 0, "older tabs are dropped first");
    let (lo, hi) = ui::tab_window(&widths, 0, 20);
    assert_eq!(lo, 0);
    assert!(hi < 5);
}

#[test]
fn settings_lives_behind_the_slash_command() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = parity_app();
    // no settings tab any more
    assert!(!teamagents_tui::app::PANELS.contains(&"settings"));

    app.composer.set_text("/settings");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.settings_open, "/settings opens the overlay");
    assert_eq!(app.composer.text(), "", "the command is not sent as a message");

    // ↑↓ + Enter edit a setting; Esc closes
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.settings_row, 1);
    let before = app.animations;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_ne!(app.animations, before, "Enter toggles the selected row");
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.settings_open, "Esc closes the overlay");

    // and the overlay renders as a centred box with the info lines
    app.settings_open = true;
    let backend = TestBackend::new(120, 36);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let text = frame_text(terminal.backend().buffer());
    assert!(text.contains("Settings"), "overlay title: {text}");
    assert!(text.contains("Interface language"), "language row missing");
    assert!(text.contains("Session: s1"), "info lines missing");
    assert!(text.contains("Esc") && text.contains("close"), "overlay hint missing");
}

#[test]
fn long_tables_scroll_with_the_selection() {
    // the window follows the cursor (pure function) and the frame shows it
    assert_eq!(ui::table_start(30, 25, 10), 20);
    assert_eq!(ui::table_start(5, 4, 10), 0);
    assert_eq!(ui::table_start(30, 0, 10), 0);

    let mut app = App::new(
        "s1",
        json!({"models": {}, "tools": {}, "skills_paths": [], "instruction_files": []}),
        "/tmp/config.toml".into(),
        "en",
        true,
        vec![],
    );
    let sessions: Vec<Json> = (0..30)
        .map(|i| json!({"sessionId": format!("proj_{i:04}"), "path": "/tmp", "cwd": "/tmp",
                        "status": "ACTIVE", "goalState": "idle", "permissionsMode": "approved_scope",
                        "updatedAt": 0.0, "events": i, "tasks": 0, "sizeMb": 0.1,
                        "archived": false, "locked": false}))
        .collect();
    app.apply_state(&json!({
        "session": {"session_id": "s1", "status": "ACTIVE", "cwd": "/tmp",
                    "permissions_mode": "approved_scope", "goal_id": null, "goal_state": "idle"},
        "spec": {"leader_id": "leader", "agents": [{"id": "leader", "name": "L", "role": "leader",
                  "runtime_kind": "deepagents", "model_profile": "m"}], "channels": [], "observers": [],
                  "shared_spaces": []},
        "leader_id": "leader", "revision": 1, "limits": {},
        "agents": [{"id": "leader", "status": "IDLE"}], "runs": [], "tasks": [],
        "pending_approvals": [], "events": [],
    }));
    app.sessions = sessions;
    app.panel = 4; // sessions
    app.table_cursors.insert("sessions", (None, 29));
    let backend = TestBackend::new(150, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let text = frame_text(terminal.backend().buffer());
    assert!(text.contains("proj_0029"), "the selected row stays visible: {text}");
    assert!(text.contains('┃'), "a scrollbar marks the hidden rows");
}

#[test]
fn ascii_frame_has_no_cjk_leaks() {
    // every hint/label must be translated in English mode; only user/model
    // content may contain non-ASCII text
    let mut app = App::new(
        "s1",
        json!({"models": {"m": {"provider": "openai", "model": "m"}}, "tools": {},
               "skills_paths": [], "instruction_files": []}),
        "/tmp/config.toml".into(),
        "en",
        true,
        vec![],
    );
    app.apply_state(&json!({
        "session": {"session_id": "s1", "status": "ACTIVE", "cwd": "/tmp",
                    "permissions_mode": "approved_scope", "goal_id": null, "goal_state": "idle"},
        "spec": {"leader_id": "leader", "agents": [{"id": "leader", "name": "L", "role": "leader",
                  "runtime_kind": "deepagents", "model_profile": "m"}], "channels": [], "observers": [],
                  "shared_spaces": []},
        "leader_id": "leader", "revision": 1, "limits": {},
        "agents": [{"id": "leader", "status": "IDLE"}], "runs": [],
        "tasks": [{"task_id": "task_1", "requester": "leader", "assignee": "leader",
                   "description": "demo", "acceptance": "", "dependencies": [], "status": "PENDING",
                   "result_refs": [], "created_at": 0.0, "updated_at": 0.0}],
        "pending_approvals": [], "events": [],
    }));
    let mut cases: Vec<usize> = (0..teamagents_tui::app::PANELS.len()).collect();
    cases.push(usize::MAX); // the /settings overlay
    for panel in cases {
        if panel != usize::MAX {
            app.panel = panel;
        } else {
            app.settings_open = true;
        }
        let backend = TestBackend::new(140, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::render(f, &mut app)).unwrap();
        let text = frame_text(terminal.backend().buffer());
        let cjk: Vec<char> = text.chars().filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c)).collect();
        let name = if panel == usize::MAX { "settings overlay".to_string() } else { teamagents_tui::app::PANELS[panel].to_string() };
        assert!(
            cjk.is_empty(),
            "{name} leaks untranslated text: {:?}",
            cjk.iter().collect::<String>()
        );
    }
}
