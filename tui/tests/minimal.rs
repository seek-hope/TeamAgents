use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use serde_json::json;
use teamagents_tui::{
    v2app::{Focus, V2App, V2Effect},
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
fn interruption_and_exit_are_distinct_and_keep_the_draft() {
    let mut app = app();
    app.composer.set_text("unfinished instruction");
    assert_eq!(app.handle_key(ctrl('c')), Some(V2Effect::Interrupt { instance: "i-leader".into() }));
    assert!(!app.quit);
    assert_eq!(app.composer.text(), "unfinished instruction");
    app.apply_approvals(json!({"approvals": [{"id": "ap-1"}]}));
    app.handle_key(ctrl('a'));
    assert_eq!(app.handle_key(ctrl('c')), Some(V2Effect::Interrupt { instance: "i-leader".into() }));
    assert_eq!(app.handle_key(ctrl('d')), Some(V2Effect::Quit));
    assert!(app.quit);
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
