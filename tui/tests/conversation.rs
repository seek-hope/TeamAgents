use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    backend::TestBackend,
    style::{Modifier, Style},
    text::{Line, Span},
    Terminal,
};
use serde_json::json;
use teamagents_tui::{markdown, theme, transcript, v2app::V2App, v2ui, wrap::wrap_line};

fn text(lines: &[Line<'_>]) -> String {
    lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n")
}
fn app() -> V2App {
    let mut app = V2App::new("s-render");
    app.apply_checkpoint(json!({"instances": [{"id":"i-leader", "lifecycle":"ACTIVE", "phase":"MODEL_PENDING"}]}), 1);
    app
}
fn frame(app: &mut V2App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| v2ui::render(f, app)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .chunks(usize::from(width).max(1))
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn markdown_renders_structure_without_losing_destinations_or_code() {
    let source = "# Review\n\n**Ready** and *careful* with `a_b`. [Guide](https://example.test/docs).\n\n> Quote\n\n1. First item with enough words to wrap\n2. Second\n\n- [x] Checked\n\n```rust\nfn main() {\n    println!(\"a_b **literal**\");\n}\n```\n\n```diff\n-old\n+new\n```";
    let rows = markdown::render(source, 62);
    let shown = text(&rows);
    assert!(shown.contains("Review") && !shown.contains("# Review"), "{shown}");
    assert!(shown.contains("Ready and careful with a_b."), "{shown}");
    assert!(shown.contains("https://example.test/docs"), "{shown}");
    assert!(shown.contains("│ Quote") && shown.contains("2. Second") && shown.contains("[x] Checked"), "{shown}");
    assert!(shown.contains("    println!(\"a_b **literal**\");"), "{shown}");
    assert!(rows
        .iter()
        .flat_map(|l| &l.spans)
        .any(|s| s.content.contains("Ready") && s.style.add_modifier.contains(Modifier::BOLD)));
    assert!(rows.iter().flat_map(|l| &l.spans).any(|s| s.content == "+new" && s.style.fg == Some(theme::ADDED)));
}

#[test]
fn tables_keep_cells_and_links_at_wide_and_narrow_widths() {
    let source = "| Check | Evidence |\n|---|---|\n| Tests | **pass** |\n| Docs | [guide](https://example.test) |";
    for width in [12, 32, 80] {
        let rows = markdown::render(source, width);
        assert!(rows.iter().all(|l| l.width() <= width), "{rows:?}");
        let shown = text(&rows);
        assert!(shown.contains("Tests") && shown.contains("Docs") && shown.contains("pass"), "{shown}");
        // A URL may wrap, but no part of its destination is discarded.
        let compact: String = shown.chars().filter(|c| !c.is_whitespace() && *c != '│').collect();
        assert!(compact.contains("https://example.test"), "{shown}");
        if width == 12 {
            assert!(shown.contains("Check:"), "{shown}");
        }
    }
}

#[test]
fn rendering_bounds_graphemes_controls_and_unfinished_markdown() {
    let wide = "\u{754c}\u{9762}e\u{301}\u{1f469}\u{200d}\u{1f4bb}";
    let hostile = "safe\x1b[31m red\x1b[0m\x1b]52;c;secret\x07 after\r\nnext\tcell";
    assert_eq!(markdown::clean(hostile), "safe red after\nnext    cell");
    let styled = Line::from(vec![Span::styled(wide, Style::default().add_modifier(Modifier::BOLD))]);
    let wrapped = wrap_line(&styled, 2);
    assert_eq!(text(&wrapped).replace('\n', ""), wide);
    assert!(wrapped.iter().flat_map(|l| &l.spans).all(|s| s.style.add_modifier.contains(Modifier::BOLD)));
    for width in 0..81 {
        for source in [
            format!("- {wide} words words\n\n```rust\n    {wide}\n"),
            "**unfinished\n\n[a link](https://example.test/long/path)".into(),
            hostile.into(),
        ] {
            let rows = markdown::render(&source, width);
            assert!(rows.iter().all(|l| l.width() <= width), "width={width} {rows:?}");
            assert!(!text(&rows).contains('\x1b'));
        }
    }
    assert_eq!(wrap_line(&Line::raw("1234"), 4).len(), 1, "exact-width code must not gain a blank row");
}

#[test]
fn tool_results_pair_by_member_epoch_and_id_without_guessing_success() {
    let rows = json!([
        {"instance_id":"i-leader", "epoch":0, "kind":"assistant", "message":{"tool_calls":[{"id":"a", "function":{"name":"shell", "arguments":"{\"command\":\"make check\"}"}}]}},
        {"instance_id":"i-worker", "epoch":0, "kind":"tool_result", "message":{"tool_call_id":"a", "content":"worker-only"}},
        {"instance_id":"i-leader", "epoch":1, "kind":"tool_result", "message":{"tool_call_id":"a", "content":"different epoch"}},
        {"instance_id":"i-leader", "epoch":0, "kind":"tool_result", "message":{"tool_call_id":"a", "content":"FAILED\n(exit 1)"}}
    ]);
    let entries = transcript::project(rows.as_array().unwrap(), "i-leader", true);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].tool.as_ref().unwrap().output.as_deref(), Some("FAILED\n(exit 1)"));
    let mut cache = transcript::Transcript::default();
    let shown = text(cache.lines(&entries, 80, Some(4)));
    assert!(shown.contains("result received") && shown.contains("FAILED"), "{shown}");
    assert!(shown.contains("worker-only") && shown.contains("different epoch"), "{shown}");
    assert!(!shown.contains("succeeded"), "a receipt is not a successful outcome");
    let calls = json!([{"kind":"assistant", "message":{"tool_calls":[{"id":"x", "function":{"name":"edit_file",
        "arguments":"{\"path\":\"a.rs\",\"old_string\":\"before\",\"new_string\":\"after\"}"}}]}}]);
    let full = transcript::project(calls.as_array().unwrap(), "i-leader", false);
    let shown = text(cache.lines(&full, 80, None));
    assert!(shown.contains("before") && shown.contains("after"), "full history must retain every argument: {shown}");
    let mut app = app();
    let notes = json!({"entries":[{"kind":"note", "message":{"content":"worker delivered its report"}}]});
    app.apply_history(notes.clone());
    app.apply_history(notes);
    assert_eq!(app.entries.len(), 1, "stored notes survive without being duplicated as client notes");
    assert!(frame(&mut app, 80, 24).contains("worker delivered its report"));
}

#[test]
fn tool_expansion_preserves_draft_and_refresh_invalidates_cached_output() {
    let mut app = app();
    let mut history = json!({"entries":[
        {"kind":"assistant", "message":{"tool_calls":[{"id":"a", "function":{"name":"shell", "arguments":"{\"command\":\"cargo test\"}"}}]}},
        {"kind":"tool_result", "message":{"tool_call_id":"a", "content":(0..12).map(|i| format!("result-{i:02}")).collect::<Vec<_>>().join("\n")}}
    ]});
    app.apply_history(history.clone());
    app.composer.set_text("keep this draft");
    let compact = frame(&mut app, 80, 40);
    assert!(compact.contains("result-00") && compact.contains("result-11") && !compact.contains("result-05"));
    assert!(app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)).is_none());
    assert_eq!(app.composer.text(), "keep this draft");
    assert!(frame(&mut app, 80, 40).contains("result-05"));
    history["entries"][1]["message"]["content"] = json!("new-result");
    app.apply_history(history);
    let changed = frame(&mut app, 80, 40);
    assert!(changed.contains("new-result") && !changed.contains("result-05"));
}

#[test]
fn stream_keeps_its_beginning_and_settlement_replaces_it_once() {
    let mut app = app();
    let source = format!("# Stream start\n\n{}\n\n```rust\nfn finish() {{}}", "Visible words. ".repeat(50));
    app.apply_previews(&json!([{"instance_id":"i-leader", "text":source}]));
    let shown = frame(&mut app, 100, 40);
    assert!(shown.contains("Stream start") && shown.contains("Leader · streaming"), "{shown}");
    app.apply_previews(&json!([]));
    app.apply_history(json!({"entries":[{"kind":"assistant", "message":{"content":source}}]}));
    let settled = frame(&mut app, 100, 40);
    assert_eq!(settled.matches("Stream start").count(), 1);
    assert!(!settled.contains("streaming"));
    for width in [1, 2, 8, 30, 80, 140] {
        for height in [1, 2, 6, 24, 40] {
            frame(&mut app, width, height);
        }
    }
}
