//! Member-history navigation is a human-only, stale-safe TUI view.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::{json, Value as Json};
use teamagents_tui::{
    app::{App, Effect},
    history::History,
    ui,
};

fn app() -> App {
    let mut app = App::new("s1", json!({}), String::new(), "en", false, vec![]);
    app.state = Some(json!({
        "session": {"session_id":"s1", "status":"ACTIVE"},
        "spec": {"leader_id":"leader", "agents":[
            {"id":"leader", "name":"Leader", "role":"leader", "runtime_kind":"deepagents", "model_profile":"m"},
            {"id":"dev", "name":"Dev", "role":"worker", "runtime_kind":"codex", "model_profile":"m"}
        ]},
        "leader_id":"leader", "agents":[], "runs":[], "tasks":[], "events":[], "pending_approvals":[]
    }));
    app
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn generation(effects: &[Effect]) -> u64 {
    match effects {
        [Effect::History { generation, .. }] => *generation,
        _ => panic!("expected history request: {effects:?}"),
    }
}

fn page(agent: Option<&str>, source: Option<&str>, item: Option<&str>, entries: Json) -> Json {
    json!({
        "session_id":"s1", "agent_id":agent, "source":source, "item":item,
        "entries":entries, "offset":0, "next_offset":null, "revision":null, "through":null
    })
}

fn source_page(agent: &str) -> Json {
    page(
        Some(agent),
        None,
        None,
        json!([
            {"id":"tree", "label":"chat_tree.json"},
            {"id":"snapshot", "label":"chat_history.json"},
            {"id":"turns", "label":"turns"},
            {"id":"events", "label":"events"}
        ]),
    )
}

fn member_page() -> Json {
    page(
        None,
        None,
        None,
        json!([
            {"id":"dev", "label":"dev · IDLE"},
            {"id":"leader", "label":"leader · IDLE"}
        ]),
    )
}

fn history_request_for(effects: &[Effect]) -> &Json {
    match effects {
        [Effect::History { params, .. }] => params,
        _ => panic!("expected history request: {effects:?}"),
    }
}

#[test]
fn native_history_preserves_page_identity_for_details_back_navigation_and_refresh() {
    let mut app = app();
    let start = app.open_history(Some("dev".into()));
    app.show_history(generation(&start), Ok(page(Some("dev"), None, None, json!([{"id":"codex","label":"native"}]))));
    let first = app.handle_key(key(KeyCode::Enter));
    assert_eq!(history_request_for(&first)["source"], "codex");
    app.show_history(
        generation(&first),
        Ok(json!({
            "agent_id":"dev","source":"codex","item":null,"cursor":null,
            "entries":[{"id":"first","label":"First native item"}],
            "revision":"page-1","next_offset":40,"next_cursor":"opaque-native-position"
        })),
    );
    let second = app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(history_request_for(&second)["cursor"], "opaque-native-position");
    assert_eq!(history_request_for(&second)["offset"], 40);
    assert!(history_request_for(&second)["revision"].is_null());
    app.show_history(
        generation(&second),
        Ok(json!({
            "agent_id":"dev","source":"codex","item":null,"cursor":"opaque-native-position",
            "entries":[{"id":"second","label":"Second native item"}],
            "revision":"page-2","next_offset":null,"next_cursor":null
        })),
    );
    let detail = app.handle_key(key(KeyCode::Enter));
    assert_eq!(history_request_for(&detail)["cursor"], "opaque-native-position");
    assert_eq!(history_request_for(&detail)["revision"], "page-2");
    assert_eq!(history_request_for(&detail)["item"], "second");
    assert_eq!(history_request_for(&detail)["offset"], 0);
    app.show_history(
        generation(&detail),
        Ok(json!({
            "agent_id":"dev","source":"codex","item":"second","cursor":"opaque-native-position",
            "text":"原生工具结果","revision":"page-2","next_offset":12000
        })),
    );
    assert!(app
        .member_history
        .as_ref()
        .unwrap()
        .lines("zh", 80, 24)
        .iter()
        .any(|(line, _)| line.contains("原生工具结果")));
    let tail = app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(history_request_for(&tail)["cursor"], "opaque-native-position");
    assert_eq!(history_request_for(&tail)["revision"], "page-2");
    assert_eq!(history_request_for(&tail)["offset"], 12000);
    app.handle_key(key(KeyCode::Esc));
    let previous = app.handle_key(key(KeyCode::Char('p')));
    assert!(history_request_for(&previous)["cursor"].is_null());
    assert_eq!(history_request_for(&previous)["revision"], "page-1");
    // Finish the read before asking for an explicit refresh.
    app.show_history(
        generation(&previous),
        Ok(json!({
            "agent_id":"dev","source":"codex","item":null,"cursor":null,"entries":[],
            "revision":"page-1","next_offset":null
        })),
    );
    let refresh = app.handle_key(key(KeyCode::Char('r')));
    assert!(history_request_for(&refresh)["cursor"].is_null());
    assert!(history_request_for(&refresh)["revision"].is_null());
    assert_eq!(history_request_for(&refresh)["offset"], 0);
}

fn render(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::render(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..height {
        for x in 0..width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

#[test]
fn slash_team_and_log_shortcuts_open_the_expected_member_history() {
    let mut app = app();

    app.composer.set_text("/history");
    let effects = app.handle_key(key(KeyCode::Enter));
    assert_eq!(history_request_for(&effects)["agent_id"], Json::Null);
    app.handle_key(key(KeyCode::Esc));
    assert!(app.member_history.is_none());

    app.panel = 0;
    app.focus = teamagents_tui::app::Focus::Panel;
    app.table_cursors.insert("team", (Some("dev".into()), 1));
    let effects = app.handle_key(key(KeyCode::Char('h')));
    assert_eq!(history_request_for(&effects)["agent_id"], "dev");
    app.handle_key(key(KeyCode::Esc));

    app.panel = 5;
    app.log_member = Some("dev".into());
    let effects = app.handle_key(key(KeyCode::Char('h')));
    assert_eq!(history_request_for(&effects)["agent_id"], "dev");
}

#[test]
fn member_source_and_detail_navigation_preserves_parent_pages() {
    let mut app = app();
    let initial = app.open_history(None);
    app.show_history(generation(&initial), Ok(member_page()));

    app.handle_key(key(KeyCode::Down));
    let member = app.handle_key(key(KeyCode::Enter));
    assert_eq!(history_request_for(&member)["agent_id"], "leader");
    // The sorted production query returns dev before leader in this fixture;
    // use the response id rather than relying on a display index.
    app.show_history(
        generation(&member),
        Ok(page(
            Some("leader"),
            None,
            None,
            json!([
                {"id":"tree", "label":"chat_tree.json"},
                {"id":"snapshot", "label":"chat_history.json"},
                {"id":"turns", "label":"turns"},
                {"id":"events", "label":"events"}
            ]),
        )),
    );
    let source = app.handle_key(key(KeyCode::Enter));
    assert_eq!(history_request_for(&source)["source"], "tree");
    app.show_history(
        generation(&source),
        Ok(json!({
            "session_id":"s1", "agent_id":"leader", "source":"tree", "item":null,
            "entries":[{"id":"0","label":"ctx:leader:1 n1 · user private"}],
            "offset":0, "next_offset":null, "revision":"tree-rev", "through":null
        })),
    );
    let detail = app.handle_key(key(KeyCode::Enter));
    assert_eq!(history_request_for(&detail)["item"], "0");
    assert_eq!(history_request_for(&detail)["revision"], "tree-rev");
    app.show_history(
        generation(&detail),
        Ok(json!({
            "session_id":"s1", "agent_id":"leader", "source":"tree", "item":"0",
            "entries":[], "text":"{\"message\":\"private tool result\"}", "offset":0,
            "next_offset":null, "revision":"tree-rev", "through":null
        })),
    );
    assert!(app
        .member_history
        .as_ref()
        .unwrap()
        .lines("en", 80, 5)
        .iter()
        .any(|(line, _)| line.contains("private tool result")));

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.member_history.as_ref().unwrap().location().item, None);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.member_history.as_ref().unwrap().location().source, None);
    app.handle_key(key(KeyCode::Esc));
    assert!(app.member_history.is_some());
    app.handle_key(key(KeyCode::Esc));
    assert!(app.member_history.is_none());
}

#[test]
fn stale_generation_and_closed_views_ignore_late_history_replies() {
    let mut app = app();
    let old = app.open_history(None);
    let old_generation = generation(&old);
    app.handle_key(key(KeyCode::Esc));
    app.show_history(old_generation, Ok(member_page()));
    assert!(app.member_history.is_none());

    let current = app.open_history(None);
    let current_generation = generation(&current);
    app.show_history(old_generation, Ok(member_page()));
    assert!(app.member_history.as_ref().unwrap().entries().is_empty());
    app.show_history(current_generation, Ok(member_page()));
    assert_eq!(app.member_history.as_ref().unwrap().entries().len(), 2);
}

#[test]
fn missing_codex_history_is_shown_as_a_warning_not_fabricated_content() {
    let mut view = History::new(Some("codex".into()), 1);
    let request = view.request();
    view.apply(generation(&[request]), Ok(source_page("codex")));
    let request = view.handle_key(key(KeyCode::Enter));
    view.apply(
        generation(&request),
        Ok(json!({
            "session_id":"s1", "agent_id":"codex", "source":"tree", "item":null,
            "entries":[], "warning":"Codex 没有本地对话树；后端未提供可读的完整记录",
            "offset":0, "next_offset":null, "revision":null, "through":null
        })),
    );
    assert!(view.status("en").contains("Codex"));
    assert!(view.lines("en", 80, 5).iter().any(|(line, _)| line.contains("Codex")));
    assert!(!view.lines("en", 80, 5).iter().any(|(line, _)| line.contains("assistant")));
}

#[test]
fn pagination_refresh_info_and_narrow_frames_are_safe() {
    let mut view = History::new(Some("dev".into()), 1);
    let first = view.request();
    view.apply(generation(&[first]), Ok(source_page("dev")));
    let tree = view.handle_key(key(KeyCode::Enter));
    let list_generation = generation(&tree);
    view.apply(
        list_generation,
        Ok(json!({
            "session_id":"s1", "agent_id":"dev", "source":"tree", "item":null,
            "entries":(0..40).map(|n| json!({"id":n.to_string(),"label":format!("node-{n}")})).collect::<Vec<_>>(),
            "offset":0, "next_offset":40, "revision":"r1", "through":null
        })),
    );
    let next = view.handle_key(key(KeyCode::Char('n')));
    assert_eq!(history_request_for(&next)["offset"], 40);
    assert_eq!(history_request_for(&next)["revision"], "r1");
    view.apply(
        view.generation,
        Ok(json!({
            "session_id":"s1", "agent_id":"dev", "source":"tree", "item":null,
            "entries":[{"id":"40","label":"node-40"}], "offset":40, "next_offset":null,
            "revision":"r1", "through":null
        })),
    );
    let previous = view.handle_key(key(KeyCode::Char('p')));
    assert_eq!(history_request_for(&previous)["offset"], 0);

    view.handle_key(key(KeyCode::Char('i')));
    assert!(view.lines("en", 80, 20).iter().any(|(line, _)| line.contains("local user")));
    view.handle_key(key(KeyCode::Esc));
    assert!(!view.info && !view.closed);
    let mut app = app();
    let effects = app.open_history(Some("codex".into()));
    app.show_history(
        generation(&effects),
        Ok(json!({
            "session_id":"s1", "agent_id":"codex", "source":null, "item":null,
            "entries":[], "warning":"Codex 没有本地完整记录", "offset":0,
            "next_offset":null, "revision":null, "through":null
        })),
    );
    for (width, height) in [(1, 1), (5, 4), (16, 6), (32, 8), (80, 14)] {
        let _ = render(&mut app, width, height);
    }
    assert!(render(&mut app, 80, 14).contains("Codex"));
}
