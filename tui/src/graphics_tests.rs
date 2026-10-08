use super::*;

#[test]
fn viewport_admission_and_eviction_keep_cache_and_queue_bounded() {
    let (send, requests) = mpsc::sync_channel(8);
    let (results, receive) = mpsc::sync_channel(8);
    let mut graphics = Graphics { deferred: true, worker: Some(Worker { send, receive }), ..Graphics::default() };
    let origin = Origin::default();
    for selected in 0..20 {
        graphics.begin_frame();
        let mut doc = Document::default();
        for i in 0..20 {
            doc.append(graphics.block(Source::Image(format!("image-{i}.png")), &origin, 80));
        }
        doc.resolve(&mut graphics, selected * 2, Rect::new(0, 0, 80, 2));
        let request = requests.try_recv().expect("the visible request must be admitted");
        assert_eq!(request.source, Source::Image(format!("image-{selected}.png")));
        assert!(requests.try_recv().is_err(), "off-screen content must not enter the worker queue");
        results.send((request, Ok(Document::text(vec![Line::raw("decoded preview")])))).unwrap();
        assert!(graphics.poll());
        assert!(graphics.cache.len() <= CACHE_SIZE);
        assert!(graphics.sizes.len() <= 20);
    }
    assert!(!graphics.cache.iter().any(|c| c.request.source == Source::Image("image-0.png".into())));
    let mut doc = graphics.block(Source::Image("image-0.png".into()), &origin, 80);
    doc.resolve(&mut graphics, 0, Rect::new(0, 0, 80, 2));
    assert_eq!(requests.try_recv().unwrap().source, Source::Image("image-0.png".into()));
    assert!(graphics.cache.len() <= CACHE_SIZE);
    assert_eq!(graphics.sizes.len(), 1, "metadata is pruned to the current document");
}

#[test]
fn old_generation_results_wake_retries_without_repopulating_the_cache() {
    let (send, requests) = mpsc::sync_channel(8);
    let (results, receive) = mpsc::sync_channel(8);
    let mut graphics = Graphics { worker: Some(Worker { send, receive }), ..Graphics::default() };
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
