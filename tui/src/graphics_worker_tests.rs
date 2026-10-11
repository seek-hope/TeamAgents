use super::*;

fn request(language: &str) -> Request {
    Request {
        source: Source::Diagram { language: language.into(), code: "flowchart LR\n A --> B".into() },
        origin: Origin::default(),
        width: 80,
        native: false,
        cell: (10, 20),
        generation: 0,
    }
}

#[test]
fn worker_faults_are_bounded_isolated_and_recover() {
    let token = AtomicU64::new(0);
    for (mode, expected) in [
        ("hang", "timed out"),
        ("crash", "exited"),
        ("output", "limit"),
        ("stderr", "limit"),
        ("invalid", "response"),
        ("isolation", "isolation verified"),
    ] {
        let start = Instant::now();
        let error = run_helper(&request(&format!("__test_{mode}")), &token).err().expect("fault must fail");
        assert!(error.contains(expected), "{mode}: {error}");
        assert!(start.elapsed() < Duration::from_secs(2), "{mode} was not bounded");
        let next = run_helper(&request("mermaid"), &token).unwrap_or_else(|e| panic!("after {mode}: {e}"));
        assert!(!next.lines.is_empty());
    }
}

#[test]
fn active_and_queued_generations_cancel_and_owner_drop_reaps() {
    let token = Arc::new(AtomicU64::new(0));
    let worker_token = Arc::clone(&token);
    let start = Instant::now();
    let join = thread::spawn(move || run_helper(&request("__test_hang"), &worker_token).err().unwrap());
    thread::sleep(Duration::from_millis(100));
    token.store(1, Ordering::Relaxed);
    assert!(join.join().unwrap().contains("cancelled"));
    assert!(start.elapsed() < Duration::from_millis(600));
    // A stale request never starts a helper or consumes its wall-clock allowance.
    for _ in 0..8 {
        assert!(run_helper(&request("__test_hang"), &token).err().unwrap().contains("superseded"));
    }
    let mut graphics = Graphics::default();
    for i in 0..8 {
        graphics.block(
            Source::Diagram { language: "__test_hang".into(), code: format!("queued {i}") },
            &Origin::default(),
            80,
        );
    }
    thread::sleep(Duration::from_millis(80));
    let start = Instant::now();
    graphics.resize();
    assert!(graphics.cache.is_empty());
    graphics.toggle();
    graphics.toggle();
    drop(graphics);
    assert!(start.elapsed() < Duration::from_millis(600), "drop waited through stale work");
    // Linux children belong to the invoking thread; a successfully joined capture has reaped its child.
    let children = std::fs::read_to_string("/proc/thread-self/children").unwrap();
    assert!(children.trim().is_empty(), "unreaped display child: {children}");
}

#[test]
fn composer_and_frames_remain_live_while_helper_hangs() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{backend::TestBackend, Terminal};
    let mut app = crate::v2app::V2App::new("display-input");
    let source = Source::Diagram { language: "__test_hang".into(), code: "retained source".into() };
    app.graphics.block(source.clone(), &Origin::default(), 80);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let start = Instant::now();
    for character in "draft while parsing".chars() {
        assert!(app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)).is_none());
        terminal.draw(|frame| crate::v2ui::render(frame, &mut app)).unwrap();
    }
    assert_eq!(app.composer.text(), "draft while parsing");
    assert!(start.elapsed() < Duration::from_millis(500));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if app.graphics.poll() {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    let mut doc = app.graphics.block(source, &Origin::default(), 80);
    doc.resolve(&mut app.graphics, 0, Rect::new(0, 0, 80, 24));
    let shown = doc.lines.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
    assert!(shown.contains("timed out") && shown.contains("retained source"), "{shown}");
    assert_eq!(app.composer.text(), "draft while parsing");
}

#[test]
fn wire_refuses_bad_dimensions_controls_and_preserves_styles() {
    let req = request("mermaid");
    let mut wire = request_json(&req);
    for bad in [json!(0), json!(65537), json!(-1), json!("80")] {
        wire["width"] = bad;
        assert!(request_from_json(&wire).is_err());
    }
    wire = request_json(&req);
    wire["source"] = json!({"kind":"image","value":"/etc/passwd"});
    assert!(request_from_json(&wire).is_err());
    let style = Style::default()
        .fg(ratatui::style::Color::Indexed(3))
        .bg(ratatui::style::Color::Blue)
        .add_modifier(ratatui::style::Modifier::ITALIC | ratatui::style::Modifier::UNDERLINED);
    let document = Document::text(vec![Line::styled("preserved", style)]);
    let mut wire = document_json(&document);
    let decoded = document_from_json(&wire, &req).unwrap();
    assert_eq!(decoded.lines[0].spans[0].style, style);
    wire["lines"][0][0]["text"] = json!("\u{1b}]52;c;unsafe\u{7}");
    assert!(document_from_json(&wire, &req).is_err());
    wire = document_json(&document);
    wire["lines"][0][0]["fg"] = json!("invalid");
    assert!(document_from_json(&wire, &req).is_err());
    wire = document_json(&document);
    wire["pictures"] = json!([{"row":18446744073709551615u64}]);
    assert!(document_from_json(&wire, &req).is_err());
}

#[test]
fn hostile_content_is_bounded_and_next_preview_survives() {
    let token = AtomicU64::new(0);
    let sources = [
        Source::Diagram { language: "dot".into(), code: format!("digraph {{ {} }}", "subgraph {".repeat(1000)) },
        Source::Diagram { language: "svg".into(), code: "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"999999999\" height=\"999999999\"><image href=\"file:///etc/passwd\"/></svg>".into() },
        Source::Code { language: "rust".into(), code: format!("let value = \"{}\";", "\\\\".repeat(1900)) },
    ];
    for source in sources {
        let start = Instant::now();
        let _ = run_helper(&Request { source, ..request("mermaid") }, &token);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(run_helper(&request("mermaid"), &token).is_ok());
    }
}
