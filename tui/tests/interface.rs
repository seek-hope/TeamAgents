use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde_json::json;
use teamagents_tui::v2app::{Focus, V2App, V2Effect};

fn app() -> V2App {
    let mut app = V2App::new("interface");
    app.apply_checkpoint(
        json!({"instances":[{"id":"i-leader","lifecycle":"ACTIVE","phase":"READY","model":"fixture"}], "models":[{"key":"fixture","model":"fixture"}]}),
        0,
    );
    app
}
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn approval_refresh_preserves_identity_and_decisions_require_plain_press() {
    let mut app = app();
    app.apply_approvals(json!({"approvals":[{"id":"first"},{"id":"second"}]}));
    app.approval_sel = 1;
    app.focus = Focus::Approvals;
    app.apply_approvals(json!({"approvals":[{"id":"second"},{"id":"first"}]}));
    assert_eq!(app.approvals[app.approval_sel].id, "second");
    for modifiers in [KeyModifiers::ALT, KeyModifiers::CONTROL, KeyModifiers::SHIFT] {
        assert!(app.handle_key(KeyEvent::new(KeyCode::Char('d'), modifiers)).is_none());
    }
    assert!(app
        .handle_key(KeyEvent::new_with_kind(KeyCode::Char('a'), KeyModifiers::NONE, KeyEventKind::Repeat))
        .is_none());
    assert!(
        matches!(app.handle_key(key(KeyCode::Char('a'))), Some(V2Effect::Decide { approval_id, decision: "approve" }) if approval_id == "second")
    );
    app.apply_approvals(json!({"approvals":[{"id":"first"}]}));
    assert_eq!(app.focus, Focus::Composer, "a removed selection requires explicit approval focus again");
    assert!(!matches!(app.handle_key(key(KeyCode::Char('a'))), Some(V2Effect::Decide { .. })));
}

#[test]
fn menu_shortcuts_and_confirmation_do_not_repeat_or_accept_extra_modifiers() {
    let mut app = app();
    let open = KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL);
    app.handle_key(open);
    assert!(app.profile.is_some());
    assert!(app.handle_key(KeyEvent::new_with_kind(open.code, open.modifiers, KeyEventKind::Repeat)).is_none());
    assert!(app.profile.is_some());
    assert!(app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)).is_none());
    assert!(app.profile.is_some());
    app.handle_key(open);
    assert!(app.profile.is_none(), "Ctrl+Enter toggles the same menu as Ctrl+M");
    app.handle_key(open);
    assert!(matches!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::SetRoleProfile { .. })));
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.help_open);
    app.handle_key(KeyEvent::new_with_kind(KeyCode::Char('?'), KeyModifiers::NONE, KeyEventKind::Repeat));
    assert!(app.help_open);
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(KeyEvent::new_with_kind(KeyCode::Char('?'), KeyModifiers::NONE, KeyEventKind::Repeat));
    assert!(!app.help_open);
    app.handle_key(KeyEvent::new(KeyCode::F(6), KeyModifiers::CONTROL));
    assert_eq!(app.focus, Focus::Composer);
    app.handle_key(key(KeyCode::F(6)));
    assert_eq!(app.focus, Focus::Team);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    app.handle_key(KeyEvent::new_with_kind(KeyCode::Char('h'), KeyModifiers::NONE, KeyEventKind::Repeat));
    assert_eq!(app.focus, Focus::Team);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Panel);
}

#[test]
fn editor_grammar_does_not_intercept_paths_or_ordinary_messages() {
    let mut app = app();
    app.composer.set_text("/compact preserve units");
    assert!(
        matches!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::Compact { focus, .. }) if focus == "preserve units")
    );
    app.composer.set_text("/later preserve units");
    assert!(
        matches!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::SubmitInput { text, after_work:true, .. }) if text == "preserve units")
    );
    app.composer.set_text("/attach src/edit");
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(app.attachments.paths, ["src/edit"]);
    app.composer.set_text("/edit  exact text\nlast line ");
    let Some(V2Effect::EditDraft { initial }) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("explicit edit command");
    };
    assert_eq!(initial, " exact text\nlast line ");
    app.finish_editor(&initial, Err("cancelled".into()));
    assert_eq!(app.composer.text(), initial);
    assert_eq!(app.attachments.paths, ["src/edit"]);
    app.composer.set_text("/edit");
    assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::EditDraft { initial: String::new() }));
    app.composer.set_text("mention /edit");
    assert!(
        matches!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::SubmitInput { text, .. }) if text == "mention /edit")
    );
    app.composer.set_text("/detach");
    app.handle_key(key(KeyCode::Enter));
    let notice = app.notice.as_ref().map(|notice| notice.text.clone()).unwrap_or_default();
    assert!(notice.contains("Use /detach INDEX"), "{notice}");
}

#[test]
fn every_completion_command_has_a_local_dispatch_path() {
    let mut state = app();
    state.open_rewind_draft(&serde_json::json!({"envelope":"selected","draft":{"text":"/quit"}}));
    assert!(
        matches!(state.handle_key(key(KeyCode::Enter)),Some(V2Effect::RewindSend{source,text,..}) if source=="selected" && text=="/quit")
    );
    assert!(!state.quit);
    state.open_rewind_draft(&serde_json::json!({"envelope":"selected","draft":{"text":"old"}}));
    state.handle_key(key(KeyCode::Esc));
    assert!(state.attachments.rewind_source.is_none());
    state.rewind_preview = Some(serde_json::json!({"envelope":"older"}));
    state.composer.set_text("/rewind files selected");
    assert!(matches!(state.handle_key(key(KeyCode::Enter)),Some(V2Effect::RewindFiles{key}) if key=="selected"));
    assert!(state.rewind_preview.is_none());
    state.composer.set_text("/mcp hide my-key");
    assert!(
        matches!(state.handle_key(key(KeyCode::Enter)),Some(V2Effect::McpChange{action,service}) if action=="hide" && service=="my-key")
    );
    state.apply_checkpoint(json!({"environment":{"workspace":"/tmp/fixture"},"instances":[{"id":"i-leader","lifecycle":"ACTIVE","phase":"READY"}]}), 1);
    for action in ["init", "sync", "status"] {
        state.composer.set_text(&format!("/codegraph {action}"));
        assert!(
            matches!(state.handle_key(key(KeyCode::Enter)), Some(V2Effect::CodeGraph {workspace,action:verb}) if workspace == "/tmp/fixture" && action == verb)
        );
    }
    for (name, _) in teamagents_tui::completion::COMMANDS {
        let mut app = app();
        app.composer.set_text(name);
        let effect = app.handle_key(key(KeyCode::Enter));
        assert!(!matches!(effect, Some(V2Effect::SubmitInput { .. })), "{name}");
        assert!(!app.entries.iter().any(|e| e.text.contains("Unknown command")), "{name}");
    }
}
