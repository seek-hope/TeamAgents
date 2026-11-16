use super::*;

#[test]
fn split_transcripts_keep_both_media_layouts_and_visible_cache_entries() {
    use ratatui::{backend::TestBackend, Terminal};
    use serde_json::json;
    let mut app = crate::v2app::V2App::new("split-media");
    let (send, requests) = mpsc::sync_channel(8);
    let (results, receive) = mpsc::sync_channel(8);
    app.graphics.worker = Some(Worker { send, receive, join: None, token: Arc::clone(&app.graphics.generation_token) });
    app.apply_checkpoint(
        json!({"instances":[{"id":"i-leader","lifecycle":"ACTIVE","phase":"READY"},
        {"id":"worker","lifecycle":"ACTIVE","phase":"READY"}]}),
        0,
    );
    let entry =
        json!({"kind":"assistant","message":{"content":[{"type":"image_url","image_url":{"url":"picture.png"}}]}});
    app.apply_history(json!({"entries":[entry.clone()]}));
    app.team.select("worker");
    app.team.history_open = true;
    app.team.member.as_mut().unwrap().apply(&json!({"instance_id":"worker","entries":[entry]}));
    let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
    terminal.draw(|frame| crate::v2ui::render(frame, &mut app)).unwrap();
    for _ in 0..2 {
        let request = requests.try_recv().expect("both visible media must be admitted");
        let lines = (0..6).map(|i| Line::raw(format!("{} media row {i}", request.origin.member))).collect();
        results.send((request, Ok(Document::text(lines)))).unwrap();
    }
    assert!(app.graphics.poll());
    terminal.draw(|frame| crate::v2ui::render(frame, &mut app)).unwrap();
    let shown = terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect::<String>();
    assert!(shown.contains("i-leader media row 5"), "Leader media lost its layout");
    assert!(shown.contains("worker media row 5"), "member media lost its layout");
    let visible: HashSet<_> = app.graphics.touched.iter().map(|request| request.origin.member.as_str()).collect();
    assert!(visible.contains("i-leader") && visible.contains("worker"));
    app.team.show(crate::team::View::Tasks);
    terminal.draw(|frame| crate::v2ui::render(frame, &mut app)).unwrap();
    app.team.show(crate::team::View::Conversation);
    app.entries.push(crate::v2app::ChatEntry {
        kind: crate::v2app::ChatKind::Assistant,
        who: "i-leader".into(),
        text: "new text after task navigation".into(),
        tool: None,
        workspace: String::new(),
        media: vec![],
    });
    terminal.draw(|frame| crate::v2ui::render(frame, &mut app)).unwrap();
    let shown = terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect::<String>();
    assert!(shown.contains("i-leader media row 5") && shown.contains("worker media row 5"));
}

#[test]
fn viewport_admission_and_eviction_keep_cache_and_queue_bounded() {
    let (send, requests) = mpsc::sync_channel(8);
    let (results, receive) = mpsc::sync_channel(8);
    let mut graphics = Graphics { deferred: true, ..Graphics::default() };
    graphics.worker = Some(Worker { send, receive, join: None, token: Arc::clone(&graphics.generation_token) });
    let origin = Origin::default();
    for selected in 0..20 {
        graphics.begin_frame();
        let mut doc = Document::default();
        for i in 0..20 {
            doc.append(graphics.block(Source::Image(format!("image-{i}.png")), &origin, 80));
        }
        doc.viewport(&Document::default(), &mut graphics, selected * 2, Rect::new(0, 0, 80, 2));
        let request = requests.try_recv().expect("the visible request must be admitted");
        assert_eq!(request.source, Source::Image(format!("image-{selected}.png")));
        assert!(requests.try_recv().is_err(), "off-screen content must not enter the worker queue");
        results.send((request, Ok(Document::text(vec![Line::raw("decoded preview")])))).unwrap();
        assert!(graphics.poll());
        graphics.end_frame();
        assert!(graphics.cache.len() <= CACHE_SIZE);
        assert!(graphics.sizes.len() <= 20);
    }
    assert!(!graphics.cache.iter().any(|c| c.request.source == Source::Image("image-0.png".into())));
    graphics.begin_frame();
    let doc = graphics.block(Source::Image("image-0.png".into()), &origin, 80);
    doc.viewport(&Document::default(), &mut graphics, 0, Rect::new(0, 0, 80, 2));
    graphics.end_frame();
    assert_eq!(requests.try_recv().unwrap().source, Source::Image("image-0.png".into()));
    assert!(graphics.cache.len() <= CACHE_SIZE);
    assert_eq!(graphics.sizes.len(), 1, "metadata is pruned to the current document");
}

#[test]
fn old_generation_results_wake_retries_without_repopulating_the_cache() {
    let (send, requests) = mpsc::sync_channel(8);
    let (results, receive) = mpsc::sync_channel(8);
    let mut graphics = Graphics::default();
    graphics.worker = Some(Worker { send, receive, join: None, token: Arc::clone(&graphics.generation_token) });
    graphics.block(Source::Image("old.png".into()), &Origin::default(), 80);
    let stale = requests.try_recv().unwrap();
    graphics.toggle();
    graphics.toggle();
    results.send((stale, Ok(Document::text(vec![Line::raw("old pixels")])))).unwrap();
    assert!(graphics.poll(), "free queue space must wake pending visible retries");
    assert!(graphics.cache.is_empty() && graphics.sizes.is_empty());
    graphics.block(Source::Image("old.png".into()), &Origin::default(), 80);
    let current = requests.try_recv().unwrap();
    assert_eq!(current.generation, graphics.generation);
    assert_eq!(graphics.cache.len(), 1);
    assert!(graphics.cache[0].result.is_none());
}
