//! Synthetic redraw/allocation probe; no daemon, model or wall-clock assertions.
use ratatui::{backend::TestBackend, style::Style, Terminal};
use serde_json::json;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};
use teamagents_tui::{
    markdown,
    v2app::{ChatEntry, ChatKind, V2App},
    v2ui,
};

struct Counted;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
// SAFETY: every operation forwards the original layout and pointer to System.
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(size, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counted = Counted;

fn main() {
    let mut cases = vec![];
    for count in [40, 200, 400] {
        let mut app = V2App::new("redraw-probe");
        for _ in 0..count {
            app.entries.push(ChatEntry {
                kind: ChatKind::User,
                who: "you".into(),
                text: "x\n".repeat(500),
                tool: None,
                workspace: String::new(),
                media: vec![],
            });
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
        ALLOCATIONS.store(0, Ordering::Relaxed);
        BYTES.store(0, Ordering::Relaxed);
        let mut timings = Vec::with_capacity(100);
        for i in 0..100 {
            app.composer.set_text(if i % 2 == 0 { "draft" } else { "draftx" });
            let start = Instant::now();
            terminal.draw(|f| v2ui::render(f, &mut app)).unwrap();
            timings.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let allocations = ALLOCATIONS.load(Ordering::Relaxed);
        let bytes = BYTES.load(Ordering::Relaxed);
        timings.sort_by(f64::total_cmp);
        cases.push(json!({"entries":count,"rows":app.last_chat_lines,"p50_ms":timings[50],"p95_ms":timings[95],
            "allocations_per_frame":allocations/100,"allocated_bytes_per_frame":bytes/100}));
    }
    let input = "word with emoji \u{1f642} and combining e\u{301} ".repeat(10_000);
    ALLOCATIONS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    let start = Instant::now();
    let rows = markdown::plain(&input, 78, Style::default());
    let wrap = json!({"input_bytes":input.len(),"rows":rows.len(),"ms":start.elapsed().as_secs_f64()*1000.0,
        "allocations":ALLOCATIONS.load(Ordering::Relaxed),"allocated_bytes":BYTES.load(Ordering::Relaxed)});
    println!(
        "{}",
        json!({"profile":if cfg!(debug_assertions) {"debug"} else {"release"},
        "frames_per_case":100,"cells":[80,24],"redraw":cases,"wrap":wrap})
    );
}
