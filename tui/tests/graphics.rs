use base64::{engine::general_purpose::STANDARD, Engine};
use image::{DynamicImage, ImageFormat, RgbImage};
use ratatui::{layout::Rect, text::Line};
use serde_json::json;
use std::{
    io::Cursor,
    time::{Duration, Instant},
};
use teamagents_tui::{
    graphics::{Document, Graphics, KittyOutput, Origin, Source},
    markdown, transcript,
    v2app::V2App,
};

fn text(lines: &[Line<'_>]) -> String {
    lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n")
}
fn finish(g: &mut Graphics, source: Source, origin: &Origin, width: usize) -> Document {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        g.poll();
        g.begin_frame();
        let doc = g.block(source.clone(), origin, width);
        let shown = text(&doc.lines);
        let compact: String = shown.chars().filter(|c| !c.is_whitespace()).collect();
        if !compact.contains("Rendering…") && !compact.contains("Rendererbusy") {
            return doc;
        }
        assert!(Instant::now() < until, "renderer did not settle: {shown}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn raster(format: ImageFormat) -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(RgbImage::from_fn(40, 40, |x, y| {
        image::Rgb([if x < 20 { 255 } else { 0 }, if y < 20 { 255 } else { 0 }, 100])
    }));
    let mut out = Cursor::new(vec![]);
    let image = if format == ImageFormat::Ico { DynamicImage::ImageRgba8(image.to_rgba8()) } else { image };
    image.write_to(&mut out, format).unwrap();
    out.into_inner()
}
fn data(format: ImageFormat) -> Source {
    Source::Image(format!("data:{};base64,{}", format.to_mime_type(), STANDARD.encode(raster(format))))
}

#[test]
fn raster_formats_and_svg_have_bounded_real_previews() {
    let mut g = Graphics::default();
    for format in [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Gif,
        ImageFormat::WebP,
        ImageFormat::Bmp,
        ImageFormat::Tiff,
        ImageFormat::Ico,
    ] {
        let doc = finish(&mut g, data(format), &Origin::default(), 12);
        assert!(text(&doc.lines).contains('▀'), "{format:?}: {}", text(&doc.lines));
        assert!(doc.lines.iter().all(|l| l.width() <= 12));
    }
    let svg = Source::Diagram { language: "svg".into(), code: "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"80\" height=\"40\"><rect width=\"80\" height=\"40\" fill=\"red\"/></svg>".into() };
    let doc = finish(&mut g, svg, &Origin::default(), 12);
    assert!(doc
        .lines
        .iter()
        .flat_map(|l| &l.spans)
        .any(|s| s.content == "▀" && s.style.fg == Some(ratatui::style::Color::Rgb(255, 0, 0))));
    let doc = finish(&mut g, Source::Image("data:image/png;base64,bm90IGEgcG5n".into()), &Origin::default(), 80);
    assert!(text(&doc.lines).contains("Preview unavailable"));
    assert!(!text(&doc.lines).contains("bm90"), "base64 must not become transcript text");
}

#[test]
fn mermaid_families_and_dot_render_without_a_browser() {
    let mut g = Graphics::default();
    g.native = true;
    for code in [
        "flowchart LR\n A[Start] --> B{Ready}\n B -->|yes| C[Done]",
        "sequenceDiagram\n Alice->>Bob: Hello\n Bob-->>Alice: Ready",
        "classDiagram\n Animal <|-- Dog\n Animal : +String name",
        "stateDiagram-v2\n [*] --> Idle\n Idle --> Working\n Working --> [*]",
        "erDiagram\n USER ||--o{ ORDER : places",
        "pie title Pets\n \"Dogs\" : 3\n \"Cats\" : 2",
        "gantt\n title Plan\n dateFormat YYYY-MM-DD\n section Build\n Work :2026-10-01, 2d",
        "journey\n title Delivery\n section Ship\n Tests: 5: Team",
        "mindmap\n root((Team))\n  Plan\n  Ship",
        "timeline\n title Roadmap\n 2026 : Ship\n 2027 : Improve",
        "gitGraph\n commit\n branch work\n checkout work\n commit\n checkout main\n merge work",
        "xychart-beta\n x-axis [A, B, C]\n y-axis 0 --> 10\n bar [2, 4, 6]",
        "quadrantChart\n title Scope\n x-axis Low --> High\n y-axis Low --> High\n A: [0.3, 0.6]",
    ] {
        let doc =
            finish(&mut g, Source::Diagram { language: "mermaid".into(), code: code.into() }, &Origin::default(), 100);
        assert_eq!(doc.pictures.len(), 1, "{code}: {}", text(&doc.lines));
        assert!(doc.pictures[0].image.columns <= 100 && doc.pictures[0].image.rows <= 36);
    }
    let doc = finish(
        &mut g,
        Source::Diagram {
            language: "dot".into(),
            code: "digraph { rankdir=LR; Start -> Done [label=\"ready\"]; }".into(),
        },
        &Origin::default(),
        80,
    );
    assert_eq!(doc.pictures.len(), 1, "{}", text(&doc.lines));
}

#[test]
fn text_diagrams_keep_relationships_and_fallback_keeps_source() {
    let mut g = Graphics::default();
    for code in ["flowchart LR\n A[Build] --> B[Test]", "sequenceDiagram\n Alice->>Bob: Hello"] {
        let source = Source::Diagram { language: "mermaid".into(), code: code.into() };
        let doc = finish(&mut g, source.clone(), &Origin::default(), 100);
        let shown = text(&doc.lines);
        assert!(!shown.contains("Preview unavailable"), "{shown}");
        assert!(shown.contains('─') && (shown.contains("Build") || shown.contains("Alice")), "{shown}");
        let narrow = finish(&mut g, source, &Origin::default(), 5);
        assert!(narrow.lines.iter().all(|l| l.width() <= 5));
        assert!(
            text(&narrow.lines)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .contains("Previewunavailable"),
            "{}",
            text(&narrow.lines)
        );
    }
    let code = "not-a-diagram\n A-->B";
    let doc = finish(&mut g, Source::Diagram { language: "mermaid".into(), code: code.into() }, &Origin::default(), 80);
    assert!(text(&doc.lines).contains(code), "{}", text(&doc.lines));
    assert!(text(&doc.lines).contains("Preview unavailable"));
    let table = "| Preview |\n|---|\n| ![Plot](data:image/png;base64,bm90) |";
    let doc = markdown::render_media(table, 40, Some(&mut g), &Origin::default());
    assert!(text(&doc.lines).contains("Plot") && !text(&doc.lines).contains("bm90"));
    let source = "before\n\n```mermaid\nflowchart LR\n A-->B";
    let doc = markdown::render_media(source, 80, Some(&mut g), &Origin::default());
    assert!(text(&doc.lines).contains("A-->B") && !text(&doc.lines).contains("Rendering"));
    for fenced in [format!("{source}\n```\nafter"), "> ~~~mermaid\n> flowchart LR\n> A-->B\n> ~~~".into()] {
        let doc = markdown::render_media(&fenced, 80, Some(&mut g), &Origin::default());
        assert!(!text(&doc.lines).contains("╭─"), "{}", text(&doc.lines));
    }
}

#[test]
fn local_references_use_their_member_and_refuse_escape_and_special_files() {
    let root = std::env::temp_dir().join(format!("graphics-{}", uuid::Uuid::new_v4()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let artifacts = root.join("instances/i-worker/artifacts");
    std::fs::create_dir_all(&artifacts).unwrap();
    std::fs::write(workspace.join("plot with space.png"), raster(ImageFormat::Png)).unwrap();
    std::fs::write(artifacts.join("plot.png"), raster(ImageFormat::Png)).unwrap();
    std::fs::write(root.join("outside.png"), raster(ImageFormat::Png)).unwrap();
    std::os::unix::fs::symlink(root.join("outside.png"), workspace.join("alias.png")).unwrap();
    let origin = Origin { session: root.clone(), workspace: workspace.clone(), member: "i-worker".into(), position: 0 };
    let mut g = Graphics::default();
    for path in [
        "plot%20with%20space.png".to_string(),
        "/artifacts/plot.png".into(),
        format!("file://{}", workspace.join("plot with space.png").display()),
    ] {
        assert!(text(&finish(&mut g, Source::Image(path), &origin, 80).lines).contains('▀'));
    }
    for path in [
        "../outside.png",
        "alias.png",
        ".",
        "https://example.invalid/plot.png",
        "file://remote/plot.png",
        "/etc/passwd",
    ] {
        assert!(
            text(&finish(&mut g, Source::Image(path.into()), &origin, 80).lines).contains("Preview unavailable"),
            "{path}"
        );
    }
    let invalid = Origin { member: "..".into(), ..origin.clone() };
    assert!(text(&finish(&mut g, Source::Image("/artifacts/plot.png".into()), &invalid, 80).lines)
        .contains("Preview unavailable"));
    let missing = Source::Image("later.png".into());
    assert!(text(&finish(&mut g, missing.clone(), &origin, 80).lines).contains("Preview unavailable"));
    std::fs::write(workspace.join("later.png"), raster(ImageFormat::Png)).unwrap();
    g.toggle();
    g.toggle();
    assert!(text(&finish(&mut g, missing, &origin, 80).lines).contains('▀'));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_content_and_tool_images_survive_projection_without_base64_text() {
    let Source::Image(url) = data(ImageFormat::Png) else { unreachable!() };
    for literal in ["null", "123", "\"quoted\"", "{\"result\":true}"] {
        let (shown, images) = teamagents_tui::graphics::content(&json!(literal));
        assert_eq!(shown, literal);
        assert!(images.is_empty());
    }
    let rows = json!([
        {"instance_id":"i-worker", "workspace":"/workspace/worker", "epoch":0,"kind":"assistant","message":{"tool_calls":[{"id":"one","function":{"name":"view_image","arguments":"{}"}}]}},
        {"instance_id":"i-worker", "workspace":"/workspace/worker", "epoch":0,"kind":"tool_result","message":{"tool_call_id":"one","content":json!({"output":{"image":"plot.png","media_type":"image/png"}}).to_string()}},
        {"kind":"user","message":{"content":[{"type":"text","text":"Look at this"},{"type":"image_url","image_url":{"url":url}}]}},
        {"kind":"tool_result","message":{"content":{"content":[{"type":"image","mimeType":"image/png","data":STANDARD.encode(raster(ImageFormat::Png))}]}}}
    ]);
    let projected = transcript::project(rows.as_array().unwrap(), "i-leader", true);
    assert_eq!(projected.len(), 3);
    assert_eq!(projected[0].workspace, "/workspace/worker");
    assert!(projected.iter().all(|e| e.media.len() == 1));
    assert!(projected[1].text.contains("Look at this"));
    assert!(projected.iter().all(|e| !e.text.contains("base64") && !e.text.contains("iVBOR")));
    let excess = json!((0..20).map(|_| json!({"type":"image_url", "image_url":{"url":url}})).collect::<Vec<_>>());
    let (shown, images) = teamagents_tui::graphics::content(&excess);
    assert_eq!(images.len(), 16);
    assert!(!shown.contains("iVBOR") && shown.contains("capacity"));
    let (shown, images) = teamagents_tui::graphics::content(&json!({"image":"debian:stable"}));
    assert!(images.is_empty() && shown.contains("debian:stable"));
    let mut app = V2App::new("graphics");
    app.composer.set_text("keep draft");
    assert!(app
        .handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('g'),
            crossterm::event::KeyModifiers::CONTROL
        ))
        .is_none());
    assert!(!app.graphics.enabled);
    assert_eq!(app.composer.text(), "keep draft");
    app.graphics.toggle();
    app.graphics.native = true;
    app.apply_history(json!({"entries": [rows[2].clone()]}));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 40)).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        app.graphics.poll();
        terminal.draw(|f| teamagents_tui::v2ui::render(f, &mut app)).unwrap();
        if !app.pictures.is_empty() {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut browser = teamagents_tui::sessions::SessionBrowser::default();
    browser.rows.push(teamagents_tui::sessions::SessionRow {
        id: "archive".into(),
        name: "Archived pictures".into(),
        path: "/state/archive/archive".into(),
        archived: true,
        live: false,
    });
    browser.apply_history(&json!({"entries": [rows[2].clone()]}), None);
    app.browser = Some(browser);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        app.graphics.poll();
        terminal.draw(|f| teamagents_tui::v2ui::render(f, &mut app)).unwrap();
        if !app.pictures.is_empty() {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('g'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    terminal.draw(|f| teamagents_tui::v2ui::render(f, &mut app)).unwrap();
    assert!(app.pictures.is_empty());
    assert_eq!(app.composer.text(), "keep draft");
}

#[test]
fn kitty_crops_to_the_viewport_reuses_data_and_deletes_only_owned_images() {
    let mut g = Graphics::default();
    g.native = true;
    let doc = finish(&mut g, data(ImageFormat::Png), &Origin::default(), 10);
    assert_eq!(doc.pictures.len(), 1);
    let Source::Image(url) = data(ImageFormat::Png) else { unreachable!() };
    let quoted = markdown::render_media(&format!("> ![Plot]({url})"), 10, Some(&mut g), &Origin::default());
    assert!(quoted.lines.iter().all(|line| line.width() <= 10));
    assert!(quoted.pictures.iter().all(|picture| picture.column == 2));
    let visible = doc.visible(doc.pictures[0].row + 1, Rect::new(3, 4, 10, 2));
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].skip, 1);
    assert_eq!(visible[0].area, Rect::new(3, 4, 10, 2));
    let mut output = KittyOutput::default();
    let mut bytes = vec![];
    output.paint(&mut bytes, &visible).unwrap();
    let first = String::from_utf8(bytes.clone()).unwrap();
    assert!(first.contains("a=t,f=100") && first.contains("c=10,r=2,x=0,y=20,w=100,h=40"), "{first}");
    bytes.clear();
    output.clear(&mut bytes).unwrap();
    output.paint(&mut bytes, &visible).unwrap();
    let second = String::from_utf8(bytes.clone()).unwrap();
    assert!(second.contains("a=d,d=i,i=") && !second.contains("a=t,f=100"));
    bytes.clear();
    output.paint(&mut bytes, &[]).unwrap();
    output.finish(&mut bytes).unwrap();
    let last = String::from_utf8(bytes).unwrap();
    assert!(last.contains("a=d,d=I,i=") && !last.contains("d=A"));
    assert!(doc.visible(usize::MAX, Rect::new(0, 0, 1, 1)).is_empty());
    for width in 0..11 {
        for height in 0..7 {
            for scroll in 0..20 {
                for p in doc.visible(scroll, Rect::new(2, 3, width, height)) {
                    assert!(p.area.right() <= 2 + width && p.area.bottom() <= 3 + height);
                }
            }
        }
    }
}
