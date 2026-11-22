use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use serde_json::{json, Value as Json};
use teamagents_tui::{
    team::View,
    v2app::{Focus, V2App, V2Effect},
    v2ui,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn snapshot() -> Json {
    json!({"instances":[
        {"id":"i-leader","lifecycle":"ACTIVE","phase":"WAITING","model":"leader_main","effort":"high",
            "diagnosis":{"code":"waiting","summary":"Waiting for assigned work.","waits":[{"id":"wait-1","mode":"all","timer_at":null,"conditions":[{"task":"task-1"}]}]}},
        {"id":"i-worker","lifecycle":"PARKED","phase":"READY","model":"worker_model","effort":"max",
            "diagnosis":{"code":"inactive","attention":true,"summary":"Instance parked.","reason":"HTTP 400: invalid model name","work":{"state":"STOPPED"},
                "next_steps":"Fix the cause, then explicitly resume.","operations":[{"id":"op-unknown","status":"OUTCOME_UNKNOWN","cancel_requested":true}]}}
    ],"tasks":[{"id":"task-1","assignee":"i-worker","status":"BLOCKED","diagnosis":{"reason":"Required check failed"}}],
      "work":{"status":"ACTIVE","known_usage":{"total":1200},"unknown_usage":1,"limits":{"max_total_tokens":5000}}})
}
fn app() -> V2App {
    let mut app = V2App::new("team-fixture");
    app.apply_checkpoint(snapshot(), 1);
    app.apply_history(json!({"entries":[{"kind":"assistant","message":{"content":"Leader conversation marker"}}]}));
    app
}
fn render(app: &mut V2App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| v2ui::render(frame, app)).unwrap();
    (0..height)
        .map(|y| (0..width).map(|x| terminal.backend().buffer()[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn member_navigation_keeps_leader_routing_drafts_attachments_and_approval_ownership() {
    let mut app = app();
    app.composer.set_text("draft\nsecond line");
    app.attachments.paths.push("notes.txt".into());
    for code in [KeyCode::F(6), KeyCode::Down, KeyCode::Enter, KeyCode::Char('h')] {
        assert!(app.handle_key(key(code)).is_none());
    }
    assert_eq!(app.team.selected, "i-worker");
    assert_eq!(app.focus, Focus::Panel);
    assert!(app.team.history_open);
    app.apply_approvals(json!({"approvals":[{"id":"approval-1"}]}));
    assert_eq!(app.focus, Focus::Panel);
    app.handle_key(key(KeyCode::Char('a')));
    assert_eq!(app.composer.text(), "draft\nsecond line");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.attachments.paths, vec!["notes.txt"]);
    assert!(
        matches!(app.handle_key(key(KeyCode::Enter)),Some(V2Effect::SubmitInput{instance,text,..}) if instance=="i-leader" && text=="draft\nsecond line")
    );
    assert_eq!(
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(V2Effect::Interrupt { instance: "i-leader".into() })
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL));
    assert_eq!(app.profile.as_ref().unwrap().instance, "i-leader");
}

#[test]
fn member_history_stays_separate_rejects_wrong_identity_and_keeps_refresh_debt() {
    let mut app = app();
    app.team.select("i-worker");
    app.team.history_open = true;
    app.team.view = View::Inspect;
    let reply = json!({"instance_id":"i-worker","entries":[{"epoch":0,"idx":1,"kind":"assistant","message":{"content":"worker-private-marker"}}],"next_before":{"epoch":0,"idx":1}});
    app.team.member.as_mut().unwrap().apply(&reply);
    assert!(render(&mut app, 100, 30).contains("worker-private-marker"));
    assert!(app.entries.iter().all(|entry| !entry.text.contains("worker-private-marker")));
    assert!(app.composer.text().is_empty() && app.composer.history.is_empty());
    assert!(app.team.member.as_ref().unwrap().older);
    for bad in [
        json!({"instance_id":"other","entries":[]}),
        json!({"instance_id":"i-worker"}),
        json!({"instance_id":"i-worker","entries":[{"instance_id":"other","kind":"assistant","message":{"content":"wrong-marker"}}]}),
    ] {
        app.team.member.as_mut().unwrap().apply(&bad);
        app.mark_connected();
        app.apply_checkpoint(snapshot(), 2);
        let member = app.team.member.as_ref().unwrap();
        assert!(member.dirty && member.error.is_some());
        assert!(member.entries.iter().all(|entry| !entry.text.contains("wrong-marker")));
    }
    app.team.member.as_mut().unwrap().apply(&reply);
    assert!(!app.team.member.as_ref().unwrap().dirty);
    app.team.select("i-leader");
    assert!(app.team.member.is_none());
    app.team.select("i-worker");
    assert!(app.team.member.as_ref().unwrap().entries.is_empty());
    app.apply_checkpoint(json!({"instances":[{"id":"i-leader"}]}), 3);
    assert_eq!(app.team.selected, "i-leader");
    assert!(app.team.member.is_none());
    assert!(V2App::new("another-session").team.member.is_none());
}

#[test]
fn team_diagnostics_expose_actual_blockers_waits_unknown_effects_and_missing_fields() {
    let mut app = app();
    app.team.select("i-worker");
    let wide = render(&mut app, 160, 40);
    for label in [
        "Leader conversation marker",
        "i-worker",
        "PARKED",
        "HTTP 400:",
        "invalid model name",
        "OUTCOME_UNKNOWN",
        "1200 tokens + unknown",
        "task-1",
    ] {
        assert!(wide.contains(label), "missing {label}:\n{wide}");
    }
    app.team.view = View::Inspect;
    let narrow = render(&mut app, 60, 40);
    assert!(narrow.contains("HTTP 400: invalid model name"));
    assert!(narrow.contains("Message Leader"));
    app.team.select("i-leader");
    let waits = render(&mut app, 100, 40);
    assert!(waits.contains("wait-1") && waits.contains("timer null") && waits.contains("task-1"));
    app.apply_checkpoint(json!({"instances":[{"id":"i-leader"}]}), 3);
    let missing = render(&mut app, 100, 40);
    assert!(missing.contains("Open tasks unreported") && missing.contains("No diagnosis reported"));
    assert!(!missing.contains("HTTP 400") && !missing.contains("1200 tokens"));
}

#[test]
fn task_outcomes_and_activity_are_read_only_bounded_and_survive_checkpoint_refresh() {
    let mut app = app();
    app.team.show(View::Tasks);
    app.team.apply_tasks(&json!({"tasks":[{"id":"done-1","assignee":"i-worker","status":"SUCCEEDED"},
        {"id":"blocked-1","assignee":"i-worker","status":"BLOCKED","diagnosis":{"reason":"missing artifact"}}]}));
    let shown = render(&mut app, 100, 30);
    assert!(shown.contains("done-1") && shown.contains("SUCCEEDED") && shown.contains("BLOCKED"));
    assert!(shown.contains("not process-stop evidence"));
    app.team.apply_tasks(&json!({"bad":true}));
    app.apply_checkpoint(snapshot(), 3);
    assert!(app.team.tasks_dirty && app.team.tasks_error.is_some());
    app.team.show(View::Activity);
    for sequence in 1..=230 {
        app.apply_events(&[json!({"sequence":sequence,"kind":"operation_completed","scope":"i-worker","payload":{"status":"OUTCOME_UNKNOWN","reason":"x".repeat(2000),"private_body":"must-not-copy"}})]);
    }
    assert_eq!(app.team.activity.len(), 200);
    assert!(app.team.activity.iter().all(|row| row.len() < 2000 && !row.contains("must-not-copy")));
    let newest = app.team.activity.front().unwrap().clone();
    app.apply_events(&[json!({"sequence":230,"kind":"duplicate"})]);
    app.apply_checkpoint(snapshot(), 230);
    assert_eq!(app.team.activity.front(), Some(&newest));
    assert_eq!(app.team.activity.len(), 200);
    assert!(render(&mut app, 100, 30).contains("observed since attachment"));
}

#[test]
fn all_team_views_keep_geometry_contained_and_disjoint_at_tiny_narrow_and_wide_sizes() {
    for width in [0, 1, 2, 16, 40, 80, 96, 132, 160] {
        for height in [0, 1, 2, 5, 8, 12, 24, 40] {
            for view in [View::Conversation, View::Tasks, View::Activity, View::Inspect] {
                for focus in [Focus::Composer, Focus::Team, Focus::Panel] {
                    let mut app = app();
                    app.team.select("i-worker");
                    app.team.view = view;
                    app.focus = focus;
                    let area = Rect::new(0, 0, width, height);
                    render(&mut app, width, height);
                    let g = v2ui::geometry(&app, area);
                    let regions = [
                        g.status,
                        g.nav,
                        g.sidebar,
                        g.chat,
                        g.inspector,
                        g.activity,
                        g.approvals,
                        g.completion,
                        g.composer,
                        g.footer,
                    ];
                    for (index, region) in regions.iter().enumerate() {
                        assert_eq!(region.intersection(area), *region, "{width}x{height} {view:?} {focus:?}");
                        for other in &regions[index + 1..] {
                            assert!(
                                region.intersection(*other).is_empty(),
                                "{width}x{height} {view:?} {focus:?}: {region:?} vs {other:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn shrinking_a_focused_inspector_and_command_receipts_keep_focus_on_visible_content() {
    let mut app = app();
    app.team.select("i-worker");
    app.focus = Focus::Panel;
    render(&mut app, 160, 30);
    let narrow = render(&mut app, 100, 30);
    assert!(narrow.contains("HTTP 400: invalid model name"));
    assert_eq!(app.team.view, View::Inspect);
    assert_eq!(app.focus, Focus::Panel);
    app.command_note("Nothing interrupted");
    let receipt = render(&mut app, 100, 30);
    assert!(receipt.contains("Nothing interrupted"));
    assert_eq!(app.focus, Focus::Composer);
    app.handle_key(key(KeyCode::Char('x')));
    assert_eq!(app.composer.text(), "x");
    app.team.show(View::Tasks);
    render(&mut app, 80, 30);
    assert!(app.team.tasks_visible);
    app.focus = Focus::Team;
    render(&mut app, 80, 30);
    assert!(!app.team.tasks_visible);
    app.focus = Focus::Panel;
    render(&mut app, 80, 30);
    assert!(app.team.tasks_visible);
}

#[test]
fn member_previews_media_and_scroll_do_not_replace_the_leader_or_task_view() {
    let mut app = app();
    app.graphics.enabled = false;
    app.team.select("i-worker");
    app.team.history_open = true;
    let body = (0..40).map(|index| format!("member row {index}")).collect::<Vec<_>>().join("\n\n");
    app.team.member.as_mut().unwrap().apply(&json!({"instance_id":"i-worker","entries":[
        {"workspace":"/member/workspace","kind":"assistant","message":{"content":[
            {"type":"text","text":body},{"type":"image_url","image_url":{"url":"member.png"}}]}}
    ]}));
    let entry = &app.team.member.as_ref().unwrap().entries[0];
    assert_eq!(entry.workspace, "/member/workspace");
    assert_eq!(entry.media, vec![teamagents_tui::graphics::Source::Image("member.png".into())]);
    app.previews =
        vec![("i-leader".into(), "leader-stream-marker".into()), ("i-worker".into(), "member-stream-marker".into())];
    let wide = render(&mut app, 160, 30);
    assert!(wide.contains("leader-stream-marker") && wide.contains("member-stream-marker"));
    assert!(app.entries.iter().all(|entry| entry.media.is_empty() && !entry.text.contains("member row")));
    app.team.show(View::Inspect);
    app.focus = Focus::Panel;
    let narrow = render(&mut app, 80, 30);
    assert!(!narrow.contains("leader-stream-marker") && narrow.contains("member-stream-marker"));
    let page = app.team.member.as_ref().unwrap().height;
    assert!(page > 1);
    app.handle_key(key(KeyCode::PageUp));
    assert_eq!(app.team.member.as_ref().unwrap().scroll, page);
    assert_eq!(app.chat_scroll, 0);
    app.team.show(View::Tasks);
    app.team.apply_tasks(
        &json!({"tasks":(0..40).map(|i| json!({"id":format!("task-{i}"),"status":"RUNNING"})).collect::<Vec<_>>()}),
    );
    render(&mut app, 80, 30);
    assert!(!app.team.member_visible);
    app.handle_key(key(KeyCode::PageDown));
    assert!(app.team.scroll > 0);
    assert_eq!(app.team.member.as_ref().unwrap().scroll, page);
}

#[test]
fn sidebar_and_tabs_mouse_targets_match_rendered_rows_and_narrow_focus_can_reach_inspection() {
    let mut app = app();
    for i in 0..30 {
        app.team.rows.push(json!({"id":format!("worker-{i:02}"),"lifecycle":"ACTIVE","phase":"READY"}));
    }
    app.team.select("worker-25");
    let area = Rect::new(0, 0, 140, 24);
    let geometry = v2ui::geometry(&app, area);
    let (list, start, count) = teamagents_tui::team::member_window(&app, geometry.sidebar);
    assert!(start > 0 && count > 0);
    let wanted = app.team.rows[start]["id"].as_str().unwrap().to_string();
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: list.x,
            row: list.y,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(app.team.selected, wanted);
    let column = geometry.nav.x + 10;
    let wanted = teamagents_tui::team::nav_at(geometry.nav, column).unwrap();
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: geometry.nav.y,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(app.team.view, wanted);
    app.focus = Focus::Composer;
    app.team.view = View::Conversation;
    app.composer.set_text("kept draft");
    app.handle_key(key(KeyCode::F(6)));
    assert_eq!(app.focus, Focus::Team);
    assert!(render(&mut app, 40, 24).contains("Team"));
    app.handle_key(key(KeyCode::F(6)));
    assert_eq!(app.focus, Focus::Panel);
    assert_eq!(app.team.view, View::Inspect);
    app.handle_key(key(KeyCode::F(6)));
    assert_eq!(app.focus, Focus::Composer);
    assert_eq!(app.composer.text(), "kept draft");
}

#[test]
fn member_badges_attention_jump_and_focus_distinction() {
    use crossterm::event::KeyEventKind;
    use ratatui::style::Modifier;
    use teamagents_tui::{team, theme};

    let tasks = vec![json!({"id":"task-1","assignee":"i-worker","status":"BLOCKED"})];
    let (g_parked, s_parked) = team::state_badge(
        &json!({"id":"i-worker","lifecycle":"PARKED","phase":"READY","diagnosis":{"attention":true}}),
        &tasks,
    );
    assert_eq!(g_parked, "!");
    assert_eq!(s_parked.fg, Some(theme::ERROR));
    assert!(s_parked.add_modifier.contains(Modifier::BOLD));

    let (g_approval, s_approval) =
        team::state_badge(&json!({"id":"a","lifecycle":"ACTIVE","phase":"WAITING_CAPABILITY"}), &[]);
    assert_eq!(g_approval, "◆");
    assert!(s_approval.add_modifier.contains(Modifier::BOLD));

    let (g_idle, s_idle) = team::state_badge(&json!({"id":"b","lifecycle":"ACTIVE","phase":"READY"}), &[]);
    assert_eq!(g_idle, "○");
    assert!(s_idle.add_modifier.contains(Modifier::DIM));

    let (g_term, s_term) = team::state_badge(&json!({"id":"c","lifecycle":"TERMINATED","phase":"READY"}), &[]);
    assert_eq!(g_term, "×");
    assert!(s_term.add_modifier.contains(Modifier::DIM));

    let mut app = app();
    app.composer.set_text("keep-draft");
    let area = Rect::new(0, 0, 140, 36);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let geo = v2ui::geometry(&app, area);
    let (list, _, _) = team::member_window(&app, geo.sidebar);
    // Unfocused sidebar keeps selection marker without reversed video so composer stays clear.
    let unfocused_cell = terminal.backend().buffer()[(list.x + 1, list.y)].clone();
    assert!(!unfocused_cell.modifier.contains(Modifier::REVERSED));

    app.focus = Focus::Team;
    terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
    let focused_cell = terminal.backend().buffer()[(list.x + 1, list.y)].clone();
    assert!(focused_cell.modifier.contains(Modifier::REVERSED));

    app.focus = Focus::Composer;
    assert!(app
        .handle_key(KeyEvent::new_with_kind(KeyCode::Char('n'), KeyModifiers::ALT, KeyEventKind::Repeat))
        .is_none());
    assert_eq!(app.team.selected, "i-leader");
    assert!(app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT)).is_none());
    assert_eq!(app.team.selected, "i-worker");
    assert_eq!(app.focus, Focus::Panel);

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.focus, Focus::Composer);
    assert_eq!(
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(V2Effect::Interrupt { instance: "i-leader".into() })
    );
    assert!(
        matches!(app.handle_key(key(KeyCode::Enter)), Some(V2Effect::SubmitInput { instance, text, .. }) if instance == "i-leader" && text == "keep-draft")
    );

    app.team.select("i-leader");
    app.focus = Focus::Team;
    assert!(app
        .handle_key(KeyEvent::new_with_kind(KeyCode::Char('!'), KeyModifiers::SHIFT, KeyEventKind::Repeat))
        .is_none());
    assert_eq!(app.team.selected, "i-leader");
    assert!(app.handle_key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::SHIFT)).is_none());
    assert_eq!(app.team.selected, "i-worker");
}

#[test]
fn sidebar_toggle_zoom_and_border_drag_keep_disjoint_geometry() {
    use crossterm::event::KeyEventKind;

    let mut app = app();
    let area = Rect::new(0, 0, 140, 36);
    app.focus = Focus::Team;
    assert!(app
        .handle_key(KeyEvent::new_with_kind(KeyCode::Char('b'), KeyModifiers::ALT, KeyEventKind::Repeat))
        .is_none());
    assert!(!app.sidebar_hidden);
    assert!(app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT)).is_none());
    assert!(app.sidebar_hidden);
    assert_eq!(app.focus, Focus::Composer);
    assert!(v2ui::geometry(&app, area).sidebar.is_empty());

    assert!(app
        .handle_key(KeyEvent::new_with_kind(KeyCode::Char('z'), KeyModifiers::ALT, KeyEventKind::Repeat))
        .is_none());
    assert!(!app.zoomed);
    assert!(app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT)).is_none());
    assert!(app.zoomed);
    let zoomed_screen = render(&mut app, area.width, area.height);
    assert!(zoomed_screen.contains("[ZOOM]"));
    assert_eq!(v2ui::geometry(&app, area).chat.width, area.width);
    assert!(v2ui::geometry(&app, area).inspector.is_empty());

    // Clicking the [Zoom] pill in the nav bar toggles zoom back off, and [Team] restores the sidebar.
    let nav_rect = v2ui::geometry(&app, area).nav;
    let zoom_pill = teamagents_tui::team::nav_actions(&app, nav_rect, area.width)
        .into_iter()
        .find(|(_, action, _, _)| *action == teamagents_tui::team::NavAction::Zoom)
        .unwrap()
        .0;
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: zoom_pill.x,
            row: zoom_pill.y,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(!app.zoomed);
    let nav_rect = v2ui::geometry(&app, area).nav;
    let team_pill = teamagents_tui::team::nav_actions(&app, nav_rect, area.width)
        .into_iter()
        .find(|(_, action, _, _)| *action == teamagents_tui::team::NavAction::Sidebar)
        .unwrap()
        .0;
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: team_pill.x,
            row: team_pill.y,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(!app.sidebar_hidden);

    // Dragging split borders resizes within bounds and clears drag state on release.
    let geo = v2ui::geometry(&app, area);
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: geo.sidebar.right() - 1,
            row: geo.sidebar.y + 1,
            modifiers: KeyModifiers::NONE,
        },
    );
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 34,
            row: geo.sidebar.y + 1,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(app.sidebar_width, 35);
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 2,
            row: geo.sidebar.y + 1,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(app.sidebar_width, 18);
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 2,
            row: geo.sidebar.y + 1,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(app.drag.is_none());

    let geo = v2ui::geometry(&app, area);
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: geo.inspector.x,
            row: geo.inspector.y + 1,
            modifiers: KeyModifiers::NONE,
        },
    );
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: geo.inspector.right() - 10,
            row: geo.inspector.y + 1,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(app.inspector_pct, 25);
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: geo.inspector.right() - 10,
            row: geo.inspector.y + 1,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(app.drag.is_none());

    for sidebar_hidden in [false, true] {
        for zoomed in [false, true] {
            for width in [0, 1, 24, 80, 96, 132, 160] {
                for height in [0, 1, 8, 24, 40] {
                    app.sidebar_hidden = sidebar_hidden;
                    app.zoomed = zoomed;
                    let rect = Rect::new(0, 0, width, height);
                    render(&mut app, width, height);
                    let g = v2ui::geometry(&app, rect);
                    let regions = [
                        g.status,
                        g.nav,
                        g.sidebar,
                        g.chat,
                        g.inspector,
                        g.activity,
                        g.approvals,
                        g.completion,
                        g.composer,
                        g.footer,
                    ];
                    for (i, region) in regions.iter().enumerate() {
                        assert_eq!(region.intersection(rect), *region);
                        for other in &regions[i + 1..] {
                            assert!(region.intersection(*other).is_empty());
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn clickable_inspector_tabs_activity_filter_chat_latest_and_completion() {
    use crossterm::event::KeyEventKind;

    let mut app = app();
    let area = Rect::new(0, 0, 140, 30);
    app.team.select("i-worker");
    let geo = v2ui::geometry(&app, area);
    let (details_tab, history_tab) = teamagents_tui::team::inspector_tabs(geo.inspector).unwrap();
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: history_tab.x,
            row: history_tab.y,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(app.team.history_open);
    assert_eq!(app.focus, Focus::Panel);
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: details_tab.x,
            row: details_tab.y,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(!app.team.history_open);

    app.apply_events(&[
        json!({"sequence": 2, "scope": "i-leader", "kind": "leader_event_marker", "payload": {}}),
        json!({"sequence": 3, "scope": "i-worker", "kind": "worker_event_marker", "payload": {}}),
    ]);
    app.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::ALT));
    assert_eq!(app.team.view, View::Activity);
    assert!(app
        .handle_key(KeyEvent::new_with_kind(KeyCode::Char('f'), KeyModifiers::NONE, KeyEventKind::Repeat))
        .is_none());
    assert!(!app.team.activity_filter_selected);
    app.handle_key(key(KeyCode::Char('f')));
    assert!(app.team.activity_filter_selected);
    let filtered = render(&mut app, area.width, area.height);
    assert!(filtered.contains("worker_event_marker"));
    assert!(!filtered.contains("leader_event_marker"));
    let filter_rect = teamagents_tui::team::activity_filter_rect(v2ui::geometry(&app, area).chat).unwrap();
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: filter_rect.x,
            row: filter_rect.y,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(!app.team.activity_filter_selected);

    app.handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT));
    for i in 0..60 {
        app.entries.push(teamagents_tui::v2app::ChatEntry {
            kind: teamagents_tui::v2app::ChatKind::Assistant,
            who: "i-leader".into(),
            text: format!("chat-row-{i:02}"),
            tool: None,
            workspace: String::new(),
            media: vec![],
        });
    }
    render(&mut app, area.width, area.height);
    app.scroll_chat(15);
    assert!(app.chat_scroll > 0);
    let scrolled_screen = render(&mut app, area.width, area.height);
    assert!(scrolled_screen.contains("below · latest"));
    let chat_rect = v2ui::geometry(&app, area).chat;
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: chat_rect.right() - 4,
            row: chat_rect.y,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(app.chat_scroll, 0);

    app.composer.set_text("/");
    let comp_rect = v2ui::geometry(&app, area).completion;
    assert!(comp_rect.height >= 2);
    let expected = app.completion.candidates(&app.composer)[1].0;
    let effect = v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: comp_rect.x + 1,
            row: comp_rect.y + 1,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(effect.is_none());
    assert_eq!(app.composer.text(), expected);

    app.composer.clear();
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.help_open);
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 0, row: 0, modifiers: KeyModifiers::NONE },
    );
    assert!(!app.help_open);

    app.open_report("Output", "Report body");
    assert!(app.report.is_some());
    v2ui::handle_mouse(
        &mut app,
        area,
        MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 0, row: 0, modifiers: KeyModifiers::NONE },
    );
    assert!(app.report.is_none());
}
