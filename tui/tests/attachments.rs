//! D-451: attachment selection is local draft editing until explicit Leader submission.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::json;
use teamagents_tui::{
    v2app::{Focus, V2App, V2Effect},
    v2ui,
};
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn app() -> V2App {
    let mut app = V2App::new("test");
    app.apply_checkpoint(json!({"instances":[{"id":"i-leader","lifecycle":"ACTIVE","phase":"READY"}]}), 0);
    app
}
#[test]
fn path_selection_cancellation_staging_retry_and_explicit_submission() {
    let mut app = app();
    let label = "space \u{03bb}.txt ";
    app.composer.set_text("/attach spa");
    assert!(matches!(app.handle_key(key(KeyCode::Tab)),Some(V2Effect::InputPaths{prefix}) if prefix=="spa"));
    let reply = json!({"prefix":"spa","paths":[label,"spare/"],"limited":false});
    app.attachments.apply(&reply, &app.composer);
    assert!(app.handle_key(key(KeyCode::Esc)).is_none());
    assert_eq!(app.composer.text(), "/attach spa");
    assert!(app.attachments.paths.is_empty());
    app.attachments.apply(&reply, &app.composer);
    let mut repeat = key(KeyCode::Enter);
    repeat.kind = KeyEventKind::Repeat;
    assert!(app.handle_key(repeat).is_none());
    assert_eq!(app.composer.text(), "/attach spa");
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(app.composer.text(), format!("/attach {label}"));
    assert!(app.attachments.paths.is_empty());
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(app.attachments.paths, [label]);
    assert!(app.composer.text().is_empty());
    app.composer.set_text("read attached\nwith details");
    let Some(V2Effect::SubmitInput { instance, envelope, text, attachments }) = app.handle_key(key(KeyCode::Enter))
    else {
        panic!("no submit")
    };
    assert_eq!(instance, "i-leader");
    assert_eq!(attachments, [label]);
    // Main restores this exact draft on an ambiguous/failed transport receipt.
    app.composer.set_text(&text);
    app.submit_failed("connection lost");
    let Some(V2Effect::SubmitInput { envelope: retry, .. }) = app.handle_key(key(KeyCode::Enter)) else { panic!() };
    assert_eq!(retry, envelope);
    app.composer.set_text("changed intent");
    let Some(V2Effect::SubmitInput { envelope: changed, .. }) = app.handle_key(key(KeyCode::Enter)) else { panic!() };
    assert_ne!(changed, envelope);
    app.attachments.accepted();
    assert!(app.attachments.paths.is_empty());
}
#[test]
fn attachments_preserve_approval_ownership_disconnected_drafts_and_narrow_frames() {
    let mut app = app();
    app.attachments.paths.push("already staged.txt".into());
    app.composer.set_text("/attach a");
    app.attachments.apply(&json!({"prefix":"a","paths":["alpha.txt"]}), &app.composer);
    app.focus = Focus::Approvals;
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(app.composer.text(), "/attach a");
    app.focus = Focus::Composer;
    app.composer.set_text("draft");
    app.mark_disconnected("test disconnect");
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(app.composer.text(), "draft");
    assert_eq!(app.attachments.paths, ["already staged.txt"]);
    app.composer.set_text("/attach a");
    for (w, h) in [(1, 1), (8, 4), (30, 8), (80, 24)] {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| v2ui::render(frame, &mut app)).unwrap();
    }
    app.composer.set_text("/detach 1");
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert!(app.attachments.paths.is_empty());
}

#[test]
fn mention_completion_preserves_multiline_drafts_and_stages_the_selected_file() {
    let mut app = app();
    app.composer.set_text("Inspect @src/m\nand keep this line");
    app.composer.row = 0;
    app.composer.col = app.composer.lines[0].len();
    assert!(matches!(app.handle_key(key(KeyCode::Tab)), Some(V2Effect::InputPaths { prefix }) if prefix == "src/m"));
    app.attachments
        .apply(&json!({"prefix":"src/m","paths":["src/main.rs","src/mod.rs"],"limited":false}), &app.composer);
    app.handle_key(key(KeyCode::Down));
    assert!(app.handle_key(key(KeyCode::Enter)).is_none(), "accepting a reference never submits it");
    assert_eq!(app.composer.text(), "Inspect @src/mod.rs \nand keep this line");
    assert_eq!(app.attachments.paths, ["src/mod.rs"]);
    assert!(app.entries.last().unwrap().text.contains("staged from @reference"));
    let Some(V2Effect::SubmitInput { text, attachments, .. }) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("the completed prompt did not submit")
    };
    assert_eq!(text, "Inspect @src/mod.rs \nand keep this line");
    assert_eq!(attachments, ["src/mod.rs"]);

    // A late reply for an older token cannot overwrite a newer draft.
    app.composer.set_text("@new");
    assert!(matches!(app.handle_key(key(KeyCode::Tab)), Some(V2Effect::InputPaths { prefix }) if prefix == "new"));
    app.composer.set_text("@other");
    app.attachments.apply(&json!({"prefix":"new","paths":["wrong.rs"]}), &app.composer);
    assert!(app.attachments.candidates.is_empty());
}

#[test]
fn bare_mention_and_caret_on_the_sigil_are_safe() {
    let mut app = app();
    app.composer.set_text("@");
    app.composer.col = 0;
    assert!(app.handle_key(key(KeyCode::Tab)).is_none(), "a caret before @ is not a mention prefix");
    app.composer.col = 1;
    assert!(matches!(app.handle_key(key(KeyCode::Tab)), Some(V2Effect::InputPaths { prefix }) if prefix.is_empty()));
    app.attachments.apply(&json!({"prefix":"","paths":["src/main.rs"]}), &app.composer);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.composer.text(), "@src/main.rs ");
    assert_eq!(app.attachments.paths, ["src/main.rs"]);
}

#[test]
fn mention_completion_enforces_the_attachment_cap_without_changing_the_draft() {
    let mut app = app();
    app.attachments.paths = (0..8).map(|index| format!("file-{index}.rs")).collect();
    app.composer.set_text("@src");
    assert!(matches!(app.handle_key(key(KeyCode::Tab)), Some(V2Effect::InputPaths { .. })));
    app.attachments.apply(&json!({"prefix":"src","paths":["ninth.rs"]}), &app.composer);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.composer.text(), "@src");
    assert_eq!(app.attachments.paths.len(), 8);
    assert!(app.entries.last().unwrap().text.contains("At most eight"));
}

#[test]
fn slash_commands_never_enter_input_history_but_messages_do() {
    let mut app = app();
    app.attachments.paths.push("staged.txt".into());
    app.composer.set_text("/detach 1");
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert!(app.attachments.paths.is_empty());
    app.composer.set_text("/attach new.txt");
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(app.attachments.paths, ["new.txt"]);
    app.composer.set_text("/help");
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert!(app.composer.history.is_empty(), "client commands are not sent messages");
    app.composer.set_text("ordinary");
    assert!(matches!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::SubmitInput { .. })));
    assert_eq!(app.composer.history.len(), 1);
    assert_eq!(app.composer.history[0], "ordinary");
}
