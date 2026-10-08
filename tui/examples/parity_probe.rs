//! Fixed synthetic fixture shared by the before/after D-450 comparison. No daemon/model calls.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::json;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};
use teamagents_tui::{
    markdown,
    v2app::{ChatEntry, ChatKind, V2App},
    v2ui,
};

fn main() {
    let mut app = V2App::new("parity-probe");
    app.graphics.native = true;
    for i in 0..20 {
        app.entries.push(ChatEntry {
            kind: ChatKind::Assistant, who: "i-leader".into(), tool: None,
            workspace: String::new(), media: vec![],
            text: format!("Figure-{i:02}\n\n```svg\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"160\" height=\"80\"><rect width=\"160\" height=\"80\" fill=\"#{:06x}\"/></svg>\n```", 0x202040 + i * 101),
        });
    }
    let mut terminal = Terminal::new(TestBackend::new(80, 28)).unwrap();
    let mut seen = BTreeSet::new();
    let mut durations = vec![];
    let started = Instant::now();
    // Visit top to bottom twice. Sample actual frames for four seconds per sweep;
    // scroll offsets are recomputed each time because preview layout may settle.
    for _ in 0..2 {
        for step in 0..40 {
            for _ in 0..10 {
                app.graphics.poll();
                let max = app.last_chat_lines.saturating_sub(app.last_chat_height);
                app.chat_scroll = max.saturating_sub(max * step / 39);
                let t = Instant::now();
                app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
                app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
                terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
                durations.push(t.elapsed().as_secs_f64() * 1000.0);
                let start = app.last_chat_lines.saturating_sub(app.last_chat_height).saturating_sub(app.chat_scroll);
                let image_rows: Vec<_> =
                    app.pictures
                        .iter()
                        .map(|p| {
                            start
                                + usize::from(p.area.y.saturating_sub(
                                    v2ui::geometry(&app, ratatui::layout::Rect::new(0, 0, 80, 28)).chat.y,
                                ))
                                - usize::from(p.skip)
                        })
                        .collect();
                let doc = app.transcript.document(&app.entries, 78, Some(4), Some(&mut app.graphics), "");
                for row in image_rows {
                    for line in doc.lines[..row.min(doc.lines.len())].iter().rev() {
                        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                        if let Some(n) = text.strip_prefix("Figure-").and_then(|s| s.parse::<usize>().ok()) {
                            seen.insert(n);
                            break;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    let mut visited = BTreeSet::new();
    let mut visits = vec![];
    for target in (0..20).chain([0, 19, 1, 18]) {
        let label = format!("Figure-{target:02}");
        let began = Instant::now();
        let mut rendered = false;
        while began.elapsed() < Duration::from_secs(2) {
            app.graphics.poll();
            terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
            let doc = app.transcript.document(&app.entries, 78, Some(4), Some(&mut app.graphics), "");
            let row = doc
                .lines
                .iter()
                .position(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>() == label)
                .unwrap();
            app.chat_scroll = app.last_chat_lines.saturating_sub(app.last_chat_height).saturating_sub(row);
            terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
            let start = app.last_chat_lines.saturating_sub(app.last_chat_height).saturating_sub(app.chat_scroll);
            let geo = v2ui::geometry(&app, ratatui::layout::Rect::new(0, 0, 80, 28));
            rendered = app
                .pictures
                .iter()
                .any(|p| start + usize::from(p.area.y - geo.chat.y) - usize::from(p.skip) == row + 3);
            if rendered {
                visited.insert(target);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        visits.push(json!({"target": target, "preview": rendered, "wait_ms": began.elapsed().as_secs_f64() * 1000.0}));
    }
    durations.sort_by(f64::total_cmp);
    let mut completion = V2App::new("completion-probe");
    completion.composer.set_text("/hel");
    let effect = completion.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let lines = markdown::render("```rust\nfn main() { let message = \"hello\"; }\n```", 120);
    let colors: BTreeSet<String> = lines
        .iter()
        .filter(|l| l.spans.iter().any(|s| s.content.contains("fn")))
        .flat_map(|l| l.spans.iter().filter(|s| !s.content.trim().is_empty()).map(|s| format!("{:?}", s.style.fg)))
        .collect();
    println!("{}", serde_json::to_string_pretty(&json!({
        "fixture": "20 distinct SVGs, 80x28 cells, native Kitty documents, two 40-step sweeps then 24 individual visits (2 seconds each)",
        "model_calls": 0, "elapsed_seconds": started.elapsed().as_secs_f64(),
        "logical_cpus": std::thread::available_parallelism().map_or(1, usize::from),
        "load_average": std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim(),
        "sweep_media_seen": seen, "media_seen": visited, "visits": visits,
        "media_missing": (0..20).filter(|i| !visited.contains(i)).collect::<Vec<_>>(),
        "key_and_frame_ms_p50": durations[durations.len()/2], "key_and_frame_ms_p95": durations[durations.len()*95/100],
        "key_and_frame_ms_max": durations.last(), "tab_draft": completion.composer.text(),
        "tab_effect": format!("{effect:?}"), "rust_body_color_count": colors.len(),
        "draft_preserved": app.composer.text().is_empty()
    })).unwrap());
}
