//! Review navigation and rendered evidence are driven by worker reports, not logs.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::{json, Value as Json};
use teamagents_tui::{
    app::{App, Effect, Focus, PANELS},
    review::Review,
    ui,
};

fn app() -> App {
    let mut app = App::new("s1", json!({}), String::new(), "en", false, vec![]);
    app.state = Some(json!({"session":{"session_id":"s1","status":"ACTIVE"},
        "spec":{"leader_id":"leader","agents":[
            {"id":"leader","role":"leader","name":"Leader","runtime_kind":"deepagents","model_profile":"m"},
            {"id":"dev","role":"worker","name":"Dev","runtime_kind":"codex","model_profile":"m"}]},
        "agents":[],"runs":[],"tasks":[],"events":[],"pending_approvals":[]}));
    app
}

fn report() -> Json {
    json!({"root":"/repo","baseline_at":1750000000.0,"baseline_kind":"first_observed",
        "scope":"git_tracked_and_unignored","complete":true,"shared":true,"revision":"revision-1","warnings":[],
        "changes":[{"path":"a.txt","status":"modified",
            "before":{"kind":"text","size":4,"mode":420,"hash":"a".repeat(64)},
            "after":{"kind":"text","size":8,"mode":493,"hash":"b".repeat(64)}},
            {"path":"目录/很长的文件名称.txt","status":"added"}],"detail":null})
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn generation(effects: &[Effect]) -> u64 {
    match effects {
        [Effect::Review { generation, .. }] => *generation,
        _ => panic!("expected review: {effects:?}"),
    }
}

fn loaded() -> Review {
    let mut view = Review::new("dev".into(), 1);
    let gen = generation(&[view.request()]);
    view.apply(gen, Ok(report()));
    view
}

fn frame(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::render(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..height {
        let mut x = 0;
        while x < width {
            let symbol = buffer[(x, y)].symbol();
            text.push_str(symbol);
            x += unicode_width::UnicodeWidthStr::width(symbol).max(1) as u16;
        }
        text.push('\n');
    }
    text
}

#[test]
fn slash_team_and_log_shortcuts_request_the_correct_root() {
    let mut app = app();
    app.composer.set_text("/review");
    let effect = app.handle_key(key(KeyCode::Enter));
    assert!(matches!(&effect[..], [Effect::Review {agent_id, path:None, ..}] if agent_id == "leader"));
    app.handle_key(key(KeyCode::Esc));
    app.focus = Focus::Panel;
    app.table_cursors.insert("team", (Some("dev".into()), 1));
    let effect = app.handle_key(key(KeyCode::Char('v')));
    assert!(matches!(&effect[..], [Effect::Review {agent_id, ..}] if agent_id == "dev"));
    app.handle_key(key(KeyCode::Esc));
    app.panel = PANELS.iter().position(|p| *p == "log").unwrap();
    app.log_member = Some("dev".into());
    let effect = app.handle_key(key(KeyCode::Char('v')));
    assert!(matches!(&effect[..], [Effect::Review {agent_id, ..}] if agent_id == "dev"));
    app.handle_key(key(KeyCode::Esc));
    app.log_member = None;
    let effect = app.handle_key(key(KeyCode::Char('v')));
    assert!(matches!(&effect[..], [Effect::Review {agent_id, ..}] if agent_id == "leader"));
}

#[test]
fn details_page_with_revision_refresh_resets_it_and_back_never_reuses_old_diff() {
    let mut view = loaded();
    let effects = view.handle_key(key(KeyCode::Enter));
    assert!(matches!(&effects[..], [Effect::Review {path:Some(path),offset:0,revision:Some(revision),..}]
        if path == "a.txt" && revision == "revision-1"));
    let mut first = report();
    first["detail"] = json!({"path":"a.txt","lines":["-old","+new"],"offset":0,"next_offset":120,"total_lines":250});
    view.apply(generation(&effects), Ok(first));
    assert_eq!(view.lines("en", 4)[1].0, "+new");
    let next = view.handle_key(key(KeyCode::Char('n')));
    assert!(matches!(&next[..], [Effect::Review { offset: 120, revision: Some(_), .. }]));
    assert!(view.lines("en", 4).is_empty(), "loading cannot mislabel old page");
    view.apply(generation(&next), Err("workspace changed".into()));
    let refreshed = view.handle_key(key(KeyCode::Char('r')));
    assert!(matches!(&refreshed[..], [Effect::Review { offset: 0, revision: None, .. }]));
    view.apply(generation(&refreshed), Ok(report()));
    view.handle_key(key(KeyCode::Esc));
    assert!(!view.closed && view.path.is_none());
    view.handle_key(key(KeyCode::Down));
    let second_file = view.handle_key(key(KeyCode::Enter));
    assert!(matches!(&second_file[..], [Effect::Review {path:Some(path),..}] if path.starts_with("目录/")));
    assert!(view.lines("en", 4).is_empty());
    view.handle_key(key(KeyCode::Esc));
    view.handle_key(key(KeyCode::Esc));
    assert!(view.closed);
}

#[test]
fn closed_superseded_and_switched_views_ignore_late_replies() {
    let mut app = app();
    let old = generation(&app.open_review("dev"));
    app.handle_key(key(KeyCode::Esc));
    app.show_review(old, Ok(report()));
    assert!(app.workspace_review.is_none());
    let new = generation(&app.open_review("leader"));
    app.show_review(old, Ok(report()));
    assert!(app.workspace_review.as_ref().unwrap().report.is_none());
    app.show_review(new, Ok(report()));
    let request = app.handle_key(key(KeyCode::Enter));
    let stale = generation(&request);
    app.handle_key(key(KeyCode::Esc));
    app.show_review(stale, Err("late failure".into()));
    assert!(app.workspace_review.as_ref().unwrap().error.is_none());
    app.show_fork_done(Ok(json!({"session_id":"s2","catalog":{},"user_config_path":""})));
    let next = generation(&app.open_review("leader"));
    assert!(next > stale);
    app.show_review(stale, Ok(report()));
    assert!(app.workspace_review.as_ref().unwrap().report.is_none());
}

#[test]
fn loading_review_keeps_approval_quit_and_dismissal_responsive_without_hidden_paste() {
    let mut app = app();
    app.composer.set_text("draft");
    app.open_review("dev");
    app.handle_paste("do not insert");
    assert_eq!(app.composer.text(), "draft");
    assert!(matches!(&app.handle_key(ctrl('q'))[..], [Effect::Quit]));
    assert!(app.handle_key(ctrl('g')).is_empty());
    assert!(app.workspace_review.is_none());
    assert_eq!(PANELS[app.panel], "approvals");
    app.open_review("dev");
    app.handle_key(key(KeyCode::Esc));
    assert!(app.workspace_review.is_none());
}

#[test]
fn info_retains_all_warnings_metadata_and_esc_returns_to_the_same_file() {
    let mut view = loaded();
    let effects = view.handle_key(key(KeyCode::Enter));
    let mut data = report();
    data["complete"] = json!(false);
    data["warnings"] = json!((0..20).map(|n| format!("warning-{n}")).collect::<Vec<_>>());
    data["changes"][0]["after"]["problem"] = json!("file budget exceeded");
    view.apply(generation(&effects), Ok(data));
    view.handle_key(key(KeyCode::Char('i')));
    let all = view.lines("en", 100).iter().map(|l| l.0.as_str()).collect::<Vec<_>>().join("\n");
    assert!(all.contains("warning-19") && all.contains("file budget exceeded") && all.contains("mode=755"));
    assert!(all.contains("Shared workspace") && all.contains("first snapshot"));
    for _ in 0..8 {
        view.handle_key(key(KeyCode::Down));
    }
    assert!(view.lines("en", 2).iter().any(|l| l.0.contains("file budget exceeded")));
    view.handle_key(key(KeyCode::Esc));
    assert_eq!(view.path.as_deref(), Some("a.txt"));
    assert!(!view.info && !view.closed);
}

#[test]
fn control_and_bidi_characters_are_escaped_and_long_paths_can_pan() {
    let mut view = loaded();
    let data = view.report.as_mut().unwrap();
    data["changes"][0]["path"] = json!(format!("{}TAIL\u{1b}[31m\n\u{202e}", "目录".repeat(100)));
    let before = view.lines("en", 10)[0].0.clone();
    assert!(!before.contains('\u{1b}') && !before.contains('\n') && !before.contains('\u{202e}'));
    for _ in 0..24 {
        view.handle_key(key(KeyCode::Right));
    }
    let after = view.lines("en", 10)[0].0.clone();
    assert!(after.contains("TAIL") && after.len() < before.len());
    assert!(after.contains("\\u{1b}"));
}

#[test]
fn review_frames_show_incomplete_state_in_both_languages_and_tiny_sizes_do_not_panic() {
    let mut app = app();
    let request = app.open_review("dev");
    let mut data = report();
    data["complete"] = json!(false);
    data["changes"][0]["after"]["problem"] = json!("budget exceeded");
    app.show_review(generation(&request), Ok(data));
    for lang in ["en", "zh-CN"] {
        app.lang = lang;
        let text = frame(&mut app, 70, 14);
        assert!(text.contains(if lang == "en" { "incomplete" } else { "审查不完整" }), "{text}");
        assert!(text.contains("a.txt") && text.contains("Esc"));
        for (width, height) in [(1, 1), (5, 4), (16, 6), (32, 8)] {
            frame(&mut app, width, height);
        }
    }
    app.lang = "en";
    let req = app.handle_key(key(KeyCode::Enter));
    let mut data = report();
    data["detail"] = json!({"path":"a.txt","lines":["-old","+new"],"offset":0,"next_offset":120,"total_lines":250});
    app.show_review(generation(&req), Ok(data));
    let text = frame(&mut app, 80, 14);
    assert!(text.contains("n/p 2/250") && text.contains("-old") && text.contains("+new"), "{text}");
    app.handle_key(key(KeyCode::Char('i')));
    let text = frame(&mut app, 100, 22);
    assert!(text.contains("first snapshot") && text.contains("sha256=") && text.contains("mode=755"), "{text}");
}

#[test]
fn empty_mode_only_and_unread_details_are_not_presented_as_successful_tests() {
    let mut view = loaded();
    view.report.as_mut().unwrap()["changes"] = json!([]);
    assert!(view.lines("en", 10)[0].0.contains("not proof that tests passed"));
    view.path = Some("a.txt".into());
    assert!(view.lines("en", 10)[0].0.contains("not fully read"));
    view.report.as_mut().unwrap()["detail"] = json!({"path":"a.txt","lines":[]});
    assert!(view.lines("en", 10)[0].0.contains("metadata"));
    view.error = Some("unavailable".into());
    assert!(view.lines("en", 10).is_empty());
    assert!(view.status("en").contains("unavailable"));
}
