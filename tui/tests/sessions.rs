use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use serde_json::json;
use teamagents_tui::{
    sessions::{SessionAction, SessionBrowser},
    v2app::{V2App, V2Effect},
    v2ui,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn browser() -> SessionBrowser {
    let mut browser = SessionBrowser::default();
    browser.apply_list(&json!({"sessions": (0..40).map(|n| json!({"id": format!("s-{n}"), "name": format!("conversation-{n}"), "path": format!("/root/{n}")})).collect::<Vec<_>>()}), "/root/0");
    browser.apply_history(&json!({"entries": [], "next_before": 200}), None);
    browser
}

#[test]
fn session_actions_are_modal_and_confirmation_binds_one_identity() {
    let mut app = V2App::new("s-main");
    app.composer.set_text("unsent draft");
    assert_eq!(
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
        Some(V2Effect::Sessions(SessionAction::List))
    );
    app.browser = Some(browser());
    assert!(app.handle_key(key(KeyCode::Char('d'))).is_none());
    assert!(app.handle_key(key(KeyCode::Down)).is_none());
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(app.browser.as_ref().unwrap().selected, 0);
    assert_eq!(app.composer.text(), "unsent draft");
    assert!(!app.quit);
    let effect = app.handle_key(key(KeyCode::Char('y')));
    assert!(matches!(effect, Some(V2Effect::Sessions(SessionAction::Delete(row))) if row.id == "s-0"));
    assert!(!app.quit, "a delete request is not its successful receipt");
    app.browser.as_mut().unwrap().fail("deletion refused".into());
    assert!(!app.quit);
    app.handle_key(key(KeyCode::Char('d')));
    app.handle_key(key(KeyCode::Esc));
    assert!(app.browser.is_some(), "Esc first cancels confirmation");
    app.handle_key(key(KeyCode::Esc));
    assert!(app.browser.is_none());
    assert_eq!(app.composer.text(), "unsent draft");
}

#[test]
fn failed_paging_keeps_its_cursor_and_a_successful_page_can_return() {
    let mut browser = browser();
    let older = browser.key(key(KeyCode::Char('[')));
    assert_eq!(older, Some(SessionAction::History { id: "s-0".into(), before: Some(200) }));
    browser.fail("read failed".into());
    assert_eq!(browser.before, None);
    assert!(browser.newer.is_empty());
    assert_eq!(browser.key(key(KeyCode::Char('['))), older);
    browser.apply_history(&json!({"entries": [{"message": {"content": "old"}}], "next_before": null}), Some(200));
    assert_eq!(browser.before, Some(200));
    assert_eq!(browser.key(key(KeyCode::Char(']'))), Some(SessionAction::History { id: "s-0".into(), before: None }));
    browser.apply_history(&json!({"entries": [], "next_before": 200}), None);
    assert!(browser.newer.is_empty());
}

#[test]
fn browser_search_is_local_editable_and_opens_only_on_explicit_enter() {
    let mut browser = browser();
    browser.begin_search();
    for character in "needle λ".chars() {
        assert!(browser.key(key(KeyCode::Char(character))).is_none());
    }
    let action = browser.key(key(KeyCode::Enter));
    assert_eq!(action, Some(SessionAction::Search { query: "needle λ".into(), generation: 1 }));
    assert!(browser.busy);
    browser.apply_search(
        &json!({"hits": [
            {"session_id":"s-2", "instance_id":"i-leader", "idx":7, "snippet":"needle λ"},
            {"session_id":"s-3", "instance_id":"i-leader", "idx":8, "snippet":"another"}
        ], "skipped": []}),
        "needle λ",
        1,
    );
    assert!(!browser.busy);
    assert_eq!(browser.key(key(KeyCode::Down)), None);
    assert_eq!(browser.search_selected, 1);
    assert_eq!(browser.key(key(KeyCode::Enter)), Some(SessionAction::Open(browser.rows[3].clone())));
}

#[test]
fn browser_search_no_match_cancel_and_stale_result_keep_the_browser_and_draft() {
    let mut browser = browser();
    browser.begin_search();
    browser.paste_search("\u{4e0d}\u{5b58}\u{5728}");
    let action = browser.key(key(KeyCode::Enter));
    assert_eq!(action, Some(SessionAction::Search { query: "\u{4e0d}\u{5b58}\u{5728}".into(), generation: 1 }));
    browser.cancel_search();
    assert!(!browser.search_active());
    assert_eq!(browser.search_query, "");
    browser.apply_search(
        &json!({"hits": [{"session_id":"s-1", "snippet":"stale"}], "skipped": []}),
        "\u{4e0d}\u{5b58}\u{5728}",
        1,
    );
    assert!(browser.search_hits.is_empty(), "cancelled results must not repopulate the browser");
    assert_eq!(browser.selected, 0, "search must not change the session selection");
    browser.begin_search();
    browser.paste_search("none");
    let action = browser.key(key(KeyCode::Enter));
    assert!(matches!(action, Some(SessionAction::Search { .. })));
    browser.apply_search(&json!({"hits": [], "skipped": []}), "none", 3);
    assert!(browser.search_active());
    assert!(browser.key(key(KeyCode::Enter)).is_none(), "no hit cannot switch sessions");
    assert!(browser.error.as_deref().unwrap().contains("No"), "rendered no-match state remains explicit");
    browser.key(key(KeyCode::Esc));
    assert!(!browser.search_active());
}

#[test]
fn session_page_uses_shared_rounded_chrome_and_selection() {
    let mut app = V2App::new("s-main");
    app.browser = Some(browser());
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal.draw(|frame| v2ui::render(frame, &mut app)).unwrap();
    let buffer = terminal.backend().buffer();
    let corner = buffer[(0, 1)].clone();
    assert_eq!(corner.symbol(), "╭");
    assert_eq!(corner.fg, teamagents_tui::theme::BORDER);
    let selected = buffer[(1, 2)].clone();
    assert_eq!(selected.symbol(), ">");
    assert_eq!(selected.fg, ratatui::style::Color::Reset);
    assert!(selected.modifier.contains(ratatui::style::Modifier::BOLD | ratatui::style::Modifier::REVERSED));
    assert!(!selected.modifier.contains(ratatui::style::Modifier::DIM));
    assert!(!selected.modifier.contains(ratatui::style::Modifier::UNDERLINED));
    let footer = buffer
        .content
        .chunks(80)
        .find(|row| row.iter().take(5).map(|cell| cell.symbol()).collect::<String>() == "Enter")
        .expect("session footer shortcut");
    assert_eq!(footer[0].fg, teamagents_tui::theme::ACCENT);
    assert!(footer[0].modifier.contains(ratatui::style::Modifier::BOLD));
    assert!(!footer[0].modifier.contains(ratatui::style::Modifier::DIM));
    assert!(footer[6].modifier.contains(ratatui::style::Modifier::DIM));
}

#[test]
fn browser_search_renders_at_narrow_sizes() {
    let mut app = V2App::new("s-main");
    let mut state = browser();
    state.begin_search();
    state.paste_search("record");
    state.apply_search(
        &json!({"hits": [{"session_id":"s-0", "instance_id":"i-leader", "idx":1, "snippet":"record"}], "skipped": []}),
        "record",
        0,
    );
    app.browser = Some(state);
    for width in [0, 1, 2, 8, 40] {
        for height in [0, 1, 2, 4, 12] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| v2ui::render(frame, &mut app)).unwrap();
        }
    }
}

#[test]
fn long_session_lists_render_and_click_the_same_visible_rows_at_every_size() {
    for width in [0, 1, 2, 8, 40, 80, 140] {
        for height in [0, 1, 2, 4, 12, 24, 40] {
            let mut app = V2App::new("s-main");
            let mut browser = browser();
            browser.selected = 39;
            browser.entries = vec![
                json!({"instance_id": "i-leader", "kind": "user", "message": {"content": "long text\n\nsecond line"}}),
            ];
            app.browser = Some(browser);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| v2ui::render(frame, &mut app)).unwrap();
            if width >= 40 && height >= 24 {
                let text = terminal.backend().buffer().content.iter().map(|c| c.symbol()).collect::<String>();
                assert!(text.contains("conversation-39"));
                let browser = app.browser.as_mut().unwrap();
                let action = browser.mouse(
                    Rect::new(0, 0, width, height),
                    MouseEvent {
                        kind: MouseEventKind::Down(MouseButton::Left),
                        column: 2,
                        row: 2,
                        modifiers: KeyModifiers::NONE,
                    },
                );
                assert!(matches!(action, Some(SessionAction::History { .. })));
                assert!(browser.selected > 0 && browser.selected < 39, "clicked a scrolled row, never index zero");
            }
        }
    }
}
