use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, layout::Rect, text::Line, Terminal};
use serde_json::json;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use teamagents_tui::{
    markdown, theme,
    v2app::{Focus, V2App, V2Effect},
    v2ui,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn text(lines: &[Line<'_>]) -> String {
    lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n")
}
fn body<'a>(lines: &'a [Line<'a>]) -> Vec<&'a ratatui::text::Span<'a>> {
    lines
        .iter()
        .filter(|l| l.spans.first().is_some_and(|s| s.content.starts_with("│ ")))
        .flat_map(|l| l.spans.iter().skip(1))
        .collect()
}

#[test]
fn completion_edits_only_until_an_explicit_submit_and_keeps_input_ownership() {
    for (prefix, command) in [("/h", "/help"), ("/s", "/sessions"), ("/i", "/interrupt"), ("/q", "/quit")] {
        let mut app = V2App::new("completion");
        app.composer.set_text(prefix);
        assert!(app.handle_key(key(KeyCode::Tab)).is_none());
        assert_eq!(app.composer.text(), command);
        assert!(app.entries.is_empty() && app.browser.is_none() && !app.quit);
        assert!(app.composer.history.is_empty());
    }
    let mut app = V2App::new("completion");
    app.composer.set_text("/qui");
    assert!(app.handle_key(key(KeyCode::Up)).is_none());
    assert!(app.handle_key(key(KeyCode::Enter)).is_none());
    assert_eq!(app.composer.text(), "/quit");
    assert!(!app.quit);
    let mut repeated = key(KeyCode::Enter);
    repeated.kind = crossterm::event::KeyEventKind::Repeat;
    assert!(app.handle_key(repeated).is_none());
    assert_eq!(app.composer.text(), "/quit");
    assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::Quit));

    let mut app = V2App::new("approval");
    app.composer.set_text("/s");
    app.apply_approvals(json!({"approvals": [{"id":"approval-one"}]}));
    assert_eq!(app.focus, Focus::Composer);
    assert!(app.handle_key(key(KeyCode::Esc)).is_none());
    assert_eq!(app.composer.text(), "/s");
    assert!(app.completion.candidates(&app.composer).is_empty());
    app.handle_key(key(KeyCode::Char('e')));
    assert!(!app.completion.candidates(&app.composer).is_empty());
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert_eq!(app.focus, Focus::Approvals);
    assert!(app.handle_key(key(KeyCode::Tab)).is_none());
    assert_eq!(app.composer.text(), "/se");
    app.handle_key(key(KeyCode::Esc));
    assert!(app.handle_key(key(KeyCode::Tab)).is_none());
    assert_eq!(app.composer.text(), "/sessions");
    assert!(matches!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::Sessions(_))));
    for draft in ["ordinary \u{1f642}", "/\u{4e2d}", "/help extra", "/h\nkeep"] {
        let mut app = V2App::new("draft");
        app.composer.set_text(draft);
        assert!(app.completion.candidates(&app.composer).is_empty());
        assert!(app.handle_key(key(KeyCode::Tab)).is_none());
        assert_eq!(app.composer.text(), draft);
    }
}

#[test]
fn completion_layout_preserves_regions_and_selected_candidate_at_small_sizes() {
    for width in [0, 1, 2, 8, 24, 80] {
        for height in [0, 1, 2, 4, 8, 12, 24] {
            let mut app = V2App::new("layout");
            app.composer.set_text("/");
            app.handle_key(key(KeyCode::Up));
            app.apply_approvals(json!({"approvals": [{"id":"one"}]}));
            let area = Rect::new(0, 0, width, height);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
            let geo = v2ui::geometry(&app, area);
            let regions = [geo.status, geo.chat, geo.approvals, geo.completion, geo.composer, geo.footer];
            for (i, region) in regions.iter().enumerate() {
                assert_eq!(region.intersection(area), *region);
                assert!(regions[i + 1..].iter().all(|r| region.intersection(*r).is_empty()));
            }
            if width >= 8 && geo.completion.height > 0 {
                let last = teamagents_tui::completion::COMMANDS.len() - 1;
                let (start, count) = teamagents_tui::viewport::window(last + 1, last, geo.completion.height as usize);
                assert!(start + count > last);
                let row = geo.completion.y + (last - start) as u16;
                let shown: String = (0..width).map(|x| terminal.backend().buffer()[(x, row)].symbol()).collect();
                assert!(shown.starts_with(teamagents_tui::completion::COMMANDS[last].0), "{width}x{height}: {shown}");
            }
        }
    }
}

#[test]
fn grammars_preserve_source_and_color_tokens_in_the_shared_renderer() {
    for (lang, source) in [
        ("rust", "fn main() { let s = \"hi\"; }"),
        ("python", "def greet():\n    return \"hi\""),
        ("javascript", "const value = \"hi\";"),
        ("typescript", "const value: string = \"hi\";"),
        ("json", "{\"answer\": 42, \"ok\": true}"),
        ("toml", "[section]\nvalue = \"hi\""),
        ("shell", "#!/bin/sh\nprintf \"hello\""),
    ] {
        let fenced = format!("```{lang}\n{source}\n```");
        let mut graphics = teamagents_tui::graphics::Graphics::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        let lines = loop {
            let changed = graphics.poll();
            let lines = markdown::render_media(&fenced, 120, Some(&mut graphics), &Default::default()).lines;
            if changed {
                break lines;
            }
            assert!(Instant::now() < deadline, "highlight timeout: {lang}");
            std::thread::sleep(Duration::from_millis(5));
        };
        let colors: HashSet<_> =
            body(&lines).into_iter().filter(|s| !s.content.trim().is_empty()).map(|s| s.style.fg).collect();
        assert!(colors.len() >= 2, "no token colors for {lang}: {colors:?}");
        let shown = lines
            .iter()
            .filter_map(|l| {
                let s: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                s.strip_prefix("│ ").map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(shown, source, "{lang}");
        let entries = teamagents_tui::transcript::project(
            &[json!({"instance_id":"i-leader","kind":"assistant","message":{"content":fenced}})],
            "i-leader",
            false,
        );
        let mut history = teamagents_tui::transcript::Transcript::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let changed = graphics.poll();
            let saved = history.document(&entries, 120, None, Some(&mut graphics), "");
            if changed {
                assert_eq!(body(&saved.lines), body(&lines));
                break;
            }
            assert!(Instant::now() < deadline, "saved highlight timeout: {lang}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let diff = markdown::render("```diff\n-old\n+new\n```", 40);
    assert!(body(&diff).iter().any(|s| s.content == "+new" && s.style.fg == Some(theme::ADDED)));
    assert!(body(&diff).iter().any(|s| s.content == "-old" && s.style.fg == Some(theme::ERROR)));
}

#[test]
fn highlighting_fallback_and_control_filtering_preserve_safe_text() {
    for text in [
        "```not-a-language\nfn main() {}\n```".to_string(),
        "```rust\nfn main() {}".to_string(),
        format!("```rust\n{}\n```", "x".repeat(4097)),
        format!("```rust\n{}\n```", "let x = 1;\n".repeat(501)),
        format!("```rust\n{}\n```", "// padding here\n".repeat(4500)),
    ] {
        let lines = markdown::render(&text, 90);
        assert!(body(&lines).iter().all(|s| s.style.fg == Some(theme::CODE)));
    }
    let lines = markdown::render("```rust\nlet s = \"\u{4e2d}\u{1f642}e\u{301}\";\u{1b}]52;c;secret\u{7}\n```", 12);
    assert!(lines.iter().all(|l| l.width() <= 12));
    let shown = text(&lines);
    assert!(!shown.contains('\u{1b}') && !shown.contains("secret"));
    assert!(shown.contains('\u{4e2d}') && shown.contains('\u{1f642}'));
}

#[test]
fn every_media_item_in_long_history_can_be_revisited_after_eviction() {
    let mut app = V2App::new("media");
    app.graphics.native = true;
    let rows: Vec<_> = (0..20).map(|i| json!({"instance_id":"i-leader","kind":"assistant","message":{"content":format!("Figure-{i:02}\n\n```svg\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\"><rect width=\"20\" height=\"20\" fill=\"#{:06x}\"/></svg>\n```",0x303030+i)}})).collect();
    app.entries = teamagents_tui::transcript::project(&rows, "i-leader", false);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    for target in (0..20).chain([0, 19, 1, 18]) {
        let label = format!("Figure-{target:02}");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.graphics.poll();
            terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
            let doc = app.transcript.document(&app.entries, 78, Some(4), Some(&mut app.graphics), "");
            let row = doc.lines.iter().position(|l| text(std::slice::from_ref(l)) == label).unwrap();
            app.chat_scroll = app.last_chat_lines.saturating_sub(app.last_chat_height).saturating_sub(row);
            terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
            let start = app.last_chat_lines.saturating_sub(app.last_chat_height).saturating_sub(app.chat_scroll);
            let geo = v2ui::geometry(&app, Rect::new(0, 0, 80, 24));
            let rendered = app.pictures.iter().any(|p| {
                let absolute = start + usize::from(p.area.y - geo.chat.y) - usize::from(p.skip);
                absolute == row + 3
            });
            if rendered {
                break;
            }
            assert!(Instant::now() < deadline, "{label} never received its preview");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    // The optional browser has its own history cache and a top-based viewport.
    let mut browser = teamagents_tui::sessions::SessionBrowser::default();
    browser.apply_history(&json!({"entries": rows}), None);
    app.browser = Some(browser);
    for target in (0..20).chain([0, 19]) {
        let label = format!("Figure-{target:02}");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.graphics.poll();
            terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
            let doc = app.transcript.document(&app.entries, 78, None, Some(&mut app.graphics), "");
            let row = doc.lines.iter().position(|l| text(std::slice::from_ref(l)) == label).unwrap();
            let browser = app.browser.as_mut().unwrap();
            browser.scroll = row.min(browser.last_lines.saturating_sub(browser.last_height));
            terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
            let browser = app.browser.as_ref().unwrap();
            let history_y = (0..24)
                .find(|y| {
                    let line: String = (0..80).map(|x| terminal.backend().buffer()[(x, *y)].symbol()).collect();
                    line.contains("History")
                })
                .expect("the history border is drawn")
                + 1;
            if app
                .pictures
                .iter()
                .any(|p| browser.scroll + usize::from(p.area.y - history_y) - usize::from(p.skip) == row + 3)
            {
                break;
            }
            assert!(Instant::now() < deadline, "saved {label} never received its preview");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
