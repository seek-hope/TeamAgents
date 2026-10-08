//! D-448: bounded, asynchronous display of local images and text diagrams.
//! Renderers never execute a model-supplied command, fetch a URL, or rewrite history.
use crate::{markdown, theme::*, viewport::clip_rows};
use base64::{engine::general_purpose::STANDARD, Engine};
use image::{DynamicImage, ImageReader, RgbaImage};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
};
use serde_json::Value as Json;
use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{mpsc, Arc},
};

#[cfg(test)]
#[path = "graphics_tests.rs"]
mod tests;

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 64 * 1024;
const MAX_SOURCE_LINES: usize = 500;
const MAX_SOURCE_PREVIEW_CHARS: usize = 32 * 1024;
const CACHE_SIZE: usize = 16;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum Source {
    Image(String),
    Diagram { language: String, code: String },
}
impl Source {
    fn label(&self) -> String {
        match self {
            Self::Image(url) if url.starts_with("data:") => "Embedded image".into(),
            Self::Image(url) => format!(
                "Image: {}{}",
                markdown::clean(url).chars().take(240).collect::<String>(),
                if url.chars().count() > 240 { "…" } else { "" }
            ),
            Self::Diagram { language, .. } => format!("{language} diagram"),
        }
    }
}

#[derive(Clone, Debug, Default, Hash, PartialEq, Eq)]
pub struct Origin {
    pub session: PathBuf,
    pub workspace: PathBuf,
    pub member: String,
    pub position: usize,
}

#[derive(Clone, Default)]
pub struct Document {
    pub lines: Vec<Line<'static>>,
    pub pictures: Vec<Picture>,
    pub(crate) media: Vec<MediaBlock>,
}
#[derive(Clone)]
pub(crate) struct MediaBlock {
    row: usize,
    column: usize,
    rows: usize,
    width: usize,
    request: Request,
}
impl Document {
    pub fn text(lines: Vec<Line<'static>>) -> Self {
        Self { lines, pictures: vec![], media: vec![] }
    }
    pub fn append(&mut self, mut other: Self) {
        other.offset(self.lines.len(), 0);
        self.lines.extend(other.lines);
        self.pictures.extend(other.pictures);
        self.media.extend(other.media);
    }
    pub(crate) fn offset(&mut self, rows: usize, columns: usize) {
        for picture in &mut self.pictures {
            picture.row += rows;
            picture.column += columns;
        }
        for media in &mut self.media {
            media.row += rows;
            media.column += columns;
        }
    }
    /// Resolve only media intersecting the viewport. Geometry survives eviction of decoded images.
    pub fn resolve(&mut self, graphics: &mut Graphics, start: usize, area: Rect) {
        let layout: HashSet<&Request> = self.media.iter().map(|m| &m.request).collect();
        graphics.sizes.retain(|request, _| layout.contains(request));
        let visible: Vec<_> =
            self.media.iter().filter(|m| clip_rows(m.row, m.rows, start, area.height as usize).is_some()).collect();
        graphics.touched = visible.iter().map(|m| m.request.clone()).collect();
        for media in visible {
            let mut rendered = graphics.render_request(media.request.clone(), media.width);
            // A result may have just changed the layout. The next revision supplies its full height;
            // never let a larger image cover the following message in the current frame.
            if rendered.lines.len() <= media.rows {
                rendered.offset(media.row, media.column);
                self.pictures.extend(rendered.pictures);
            }
            for (index, mut line) in rendered.lines.into_iter().take(media.rows).enumerate() {
                let prefix = self.lines[media.row + index].spans.first().cloned();
                if media.column > 0 {
                    if let Some(prefix) = prefix {
                        line.spans.insert(0, prefix);
                    }
                }
                self.lines[media.row + index] = line;
            }
        }
    }
    /// Images intersect the same row viewport as their surrounding text.
    pub fn visible(&self, start: usize, area: Rect) -> Vec<Placement> {
        self.pictures
            .iter()
            .filter_map(|p| {
                let (skip, row, count) = clip_rows(p.row, p.image.rows as usize, start, area.height as usize)?;
                if p.column + p.image.columns as usize > area.width as usize {
                    return None;
                }
                Some(Placement {
                    image: p.image.clone(),
                    skip: skip as u16,
                    area: Rect::new(area.x + p.column as u16, area.y + row as u16, p.image.columns, count as u16),
                })
            })
            .collect()
    }
}
#[derive(Clone)]
pub struct Picture {
    pub row: usize,
    pub column: usize,
    pub image: Arc<PreviewImage>,
}
#[derive(Clone)]
pub struct Placement {
    pub image: Arc<PreviewImage>,
    pub skip: u16,
    pub area: Rect,
}
pub struct PreviewImage {
    png: String,
    pixels: (u32, u32),
    pub columns: u16,
    pub rows: u16,
    id: u32,
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct Request {
    source: Source,
    origin: Origin,
    width: usize,
    native: bool,
    cell: (u16, u16),
    generation: u64,
}
struct Worker {
    send: mpsc::SyncSender<Request>,
    receive: mpsc::Receiver<(Request, Result<Document, String>)>,
}
struct Cached {
    request: Request,
    result: Option<Result<Document, String>>,
}

pub struct Graphics {
    pub enabled: bool,
    pub native: bool,
    pub cell: (u16, u16),
    pub revision: u64,
    worker: Option<Worker>,
    cache: Vec<Cached>,
    // Layout metadata belongs to the current document, not the decoded-image cache.
    sizes: HashMap<Request, usize>,
    pub(crate) deferred: bool,
    touched: HashSet<Request>,
    generation: u64,
}
impl Default for Graphics {
    fn default() -> Self {
        Self {
            enabled: true,
            native: false,
            cell: (10, 20),
            revision: 0,
            worker: None,
            cache: vec![],
            sizes: HashMap::new(),
            deferred: false,
            touched: HashSet::new(),
            generation: 0,
        }
    }
}
impl Graphics {
    /// Auto-detect only an explicit Kitty terminal, not an inherited window-id variable.
    pub fn terminal() -> Result<Self, String> {
        let mode = std::env::var("TEAMAGENTS_GRAPHICS").unwrap_or_else(|_| "auto".into());
        let native = match mode.as_str() {
            "auto" => {
                std::env::var("TERM").is_ok_and(|v| v == "xterm-kitty")
                    && std::env::var_os("TMUX").is_none()
                    && std::env::var_os("ZELLIJ").is_none()
            }
            "kitty" => true,
            "blocks" | "off" => false,
            _ => return Err("TEAMAGENTS_GRAPHICS must be auto, kitty, blocks or off".into()),
        };
        let mut graphics = Self { native, enabled: mode != "off", ..Self::default() };
        graphics.resize();
        Ok(graphics)
    }
    pub fn resize(&mut self) {
        if let Ok(size) = crossterm::terminal::window_size() {
            if size.columns > 0 && size.rows > 0 && size.width > 0 && size.height > 0 {
                let cell = ((size.width / size.columns).clamp(1, 32), (size.height / size.rows).clamp(1, 64));
                if cell != self.cell {
                    self.cell = cell;
                    self.revision = self.revision.wrapping_add(1);
                }
            }
        }
    }
    pub fn toggle(&mut self) {
        self.enabled = !self.enabled;
        self.revision = self.revision.wrapping_add(1);
        if self.enabled {
            self.cache.clear();
            self.sizes.clear();
            self.generation = self.generation.wrapping_add(1);
        }
    }
    pub fn begin_frame(&mut self) {
        self.touched.clear();
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(worker) = &self.worker {
            while let Ok((request, result)) = worker.receive.try_recv() {
                // A discarded generation also frees queue space; retry a visible request after reload.
                changed = true;
                if let Some(cached) = self.cache.iter_mut().find(|c| c.request == request) {
                    let rows = match &result {
                        Ok(doc) => doc.lines.len(),
                        Err(reason) => {
                            source_fallback(&request.source, request.width, &format!("Preview unavailable: {reason}"))
                                .len()
                        }
                    };
                    self.sizes.insert(request, rows);
                    cached.result = Some(result);
                }
            }
        }
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }
    pub fn block(&mut self, source: Source, origin: &Origin, width: usize) -> Document {
        if width == 0 {
            return Document::default();
        }
        let request = Request {
            source,
            origin: origin.clone(),
            width: width.min(120),
            native: self.native,
            cell: self.cell,
            generation: self.generation,
        };
        if self.deferred && self.enabled {
            let mut doc = Document::text(markdown::plain(&request.source.label(), width, Style::default().fg(ACCENT)));
            let rows = self.sizes.get(&request).copied().unwrap_or(1).max(1);
            doc.lines.push(Line::styled("Preview pending", Style::default().fg(GREY)));
            doc.lines.extend((1..rows).map(|_| Line::raw(" ")));
            doc.media.push(MediaBlock { row: 0, column: 0, rows: doc.lines.len(), width, request });
            return doc;
        }
        self.render_request(request, width)
    }
    fn render_request(&mut self, request: Request, width: usize) -> Document {
        let label = request.source.label();
        let mut doc = Document::text(markdown::plain(&label, width, Style::default().fg(ACCENT)));
        if !self.enabled {
            doc.lines.extend(source_fallback(&request.source, width, "Preview off · Ctrl+G to show"));
            return doc;
        }
        self.touched.insert(request.clone());
        if let Some(index) = self.cache.iter().position(|c| c.request == request) {
            let cached = self.cache.remove(index);
            match &cached.result {
                Some(Ok(result)) => doc.append(result.clone()),
                Some(Err(reason)) => {
                    doc.lines.extend(source_fallback(&request.source, width, &format!("Preview unavailable: {reason}")))
                }
                None => doc.lines.extend(markdown::plain("Rendering…", width, Style::default().fg(GREY))),
            }
            self.cache.push(cached);
            return doc;
        }
        if self.cache.len() >= CACHE_SIZE {
            if let Some(index) =
                self.cache.iter().position(|c| c.result.is_some() && !self.touched.contains(&c.request))
            {
                self.cache.remove(index);
            } else {
                doc.lines.extend(source_fallback(
                    &request.source,
                    width,
                    "Preview capacity reached; Ctrl+G shows source",
                ));
                return doc;
            }
        }
        let worker = self.worker.get_or_insert_with(|| {
            let (send, receive) = mpsc::sync_channel::<Request>(8);
            let (results, completed) = mpsc::sync_channel(8);
            std::thread::Builder::new()
                .name("teamagents-graphics".into())
                .spawn(move || {
                    while let Ok(request) = receive.recv() {
                        let result = std::panic::catch_unwind(|| prepare(&request))
                            .unwrap_or_else(|_| Err("renderer rejected this input".into()));
                        if results.send((request, result)).is_err() {
                            break;
                        }
                    }
                })
                .expect("graphics worker");
            Worker { send, receive: completed }
        });
        if worker.send.try_send(request.clone()).is_ok() {
            self.cache.push(Cached { request, result: None });
            doc.lines.extend(markdown::plain("Rendering…", width, Style::default().fg(GREY)));
        } else {
            doc.lines.extend(markdown::plain("Renderer busy; preview will retry", width, Style::default().fg(GREY)));
        }
        doc
    }
}

fn source_fallback(source: &Source, width: usize, reason: &str) -> Vec<Line<'static>> {
    let mut lines = markdown::plain(reason, width, Style::default().fg(GREY));
    if let Source::Diagram { code, .. } = source {
        lines.extend(markdown::plain(
            &code.chars().take(MAX_SOURCE_PREVIEW_CHARS).collect::<String>(),
            width,
            Style::default().fg(CODE),
        ));
        if code.chars().count() > MAX_SOURCE_PREVIEW_CHARS {
            lines.extend(markdown::plain(
                "Source preview limit reached; the stored message is unchanged.",
                width,
                Style::default().fg(GREY),
            ));
        }
    }
    lines
}

fn prepare(request: &Request) -> Result<Document, String> {
    let image = match &request.source {
        Source::Image(url) => decode(&load_image(url, &request.origin)?)?,
        Source::Diagram { language, code } => {
            if code.len() > MAX_SOURCE_BYTES || code.lines().count() > MAX_SOURCE_LINES {
                return Err("diagram size limit exceeded".into());
            }
            if code.matches('{').count() > 64 || code.matches('[').count() > 512 {
                return Err("diagram nesting/shape limit exceeded".into());
            }
            if language == "mermaid" {
                mermaid_header(code)?;
                let parsed = mermaid_rs_renderer::parse_mermaid_strict(code).map_err(|e| e.to_string())?;
                if parsed.graph.nodes.len() > 128 || parsed.graph.edges.len() > 256 {
                    return Err("diagram exceeds 128 nodes or 256 edges".into());
                }
            }
            if language == "mermaid" && !request.native {
                // Text previews are intentionally limited to the families whose relationships remain graphical.
                let kind = code.split_whitespace().next().unwrap_or("");
                if matches!(kind, "graph" | "flowchart" | "sequenceDiagram" | "classDiagram" | "stateDiagram-v2") {
                    let options = mermaid_text::RenderOptions {
                        max_width: Some(request.width),
                        max_width_strict: true,
                        ..Default::default()
                    };
                    let text = mermaid_text::render_with_options(code, &options).map_err(|e| e.to_string())?;
                    if text.len() > 128 * 1024 {
                        return Err("diagram output limit exceeded".into());
                    }
                    return Ok(Document::text(markdown::plain(&text, request.width, Style::default().fg(CODE))));
                }
            }
            let svg = match language.as_str() {
                "mermaid" => mermaid_rs_renderer::render_strict(
                    code,
                    mermaid_rs_renderer::RenderOptions {
                        theme: mermaid_rs_renderer::Theme::dark(),
                        ..Default::default()
                    },
                )
                .map_err(|e| e.to_string())?,
                "dot" | "graphviz" => {
                    let graph = layout::gv::DotParser::new(code).process().map_err(|e| format!("DOT: {e:?}"))?;
                    let mut builder = layout::gv::GraphBuilder::new();
                    builder.visit_graph(&graph);
                    let mut graph = builder.get();
                    if graph.num_nodes() > 128 {
                        return Err("DOT exceeds 128 nodes".into());
                    }
                    let mut svg = layout::backends::svg::SVGWriter::new();
                    graph.do_it(false, false, false, &mut svg);
                    svg.finalize()
                }
                "svg" => code.clone(),
                _ => return Err("unsupported diagram language".into()),
            };
            rasterize(&svg)?
        }
    };
    picture(image, request)
}

fn mermaid_header(code: &str) -> Result<(), String> {
    let mut frontmatter = false;
    for line in code.lines().map(str::trim) {
        if line == "---" {
            frontmatter = !frontmatter;
            continue;
        }
        if frontmatter || line.is_empty() || line.starts_with("%%") {
            continue;
        }
        let word =
            line.split(|c: char| c.is_whitespace() || c == ';' || c == '(').next().unwrap_or("").to_ascii_lowercase();
        let base = word.trim_end_matches("-beta").trim_end_matches("-v2");
        return if matches!(
            base,
            "graph"
                | "flowchart"
                | "sequencediagram"
                | "classdiagram"
                | "statediagram"
                | "erdiagram"
                | "pie"
                | "xychart"
                | "quadrantchart"
                | "gantt"
                | "timeline"
                | "journey"
                | "mindmap"
                | "gitgraph"
                | "requirementdiagram"
                | "sankey"
                | "block"
                | "packet"
                | "kanban"
                | "architecture"
                | "radar"
                | "treemap"
                | "zenuml"
                | "c4context"
                | "c4container"
                | "c4component"
                | "c4dynamic"
                | "c4deployment"
        ) {
            Ok(())
        } else {
            Err("unknown Mermaid diagram header".into())
        };
    }
    Err("missing Mermaid diagram header".into())
}

fn load_image(url: &str, origin: &Origin) -> Result<Vec<u8>, String> {
    if let Some(data) = url.strip_prefix("data:") {
        let (header, body) = data.split_once(',').ok_or("invalid image data URL")?;
        if !header.starts_with("image/") || body.len() > MAX_BYTES * 4 / 3 + 4 {
            return Err("image data URL is unsupported or too large".into());
        }
        let bytes = if header.ends_with(";base64") {
            STANDARD.decode(body).map_err(|_| "invalid image base64")?
        } else {
            percent_encoding::percent_decode_str(body).collect()
        };
        if bytes.len() > MAX_BYTES {
            return Err("image exceeds 8 MiB".into());
        }
        return Ok(bytes);
    }
    if url.starts_with("file://") && !url.starts_with("file:///") {
        return Err("only local file URLs are supported".into());
    }
    let url = url.strip_prefix("file://").unwrap_or(url);
    if url.contains("://") {
        return Err("remote URL retained as a link; use a local image to preview".into());
    }
    let path = percent_encoding::percent_decode_str(url).decode_utf8().map_err(|_| "invalid image path encoding")?;
    let (root, relative) = if let Some(key) = path.strip_prefix("/artifacts/") {
        if origin.member.is_empty()
            || !matches!(Path::new(&origin.member).components().collect::<Vec<_>>().as_slice(), [Component::Normal(_)])
            || !origin.session.is_absolute()
        {
            return Err("image has no authoritative member location".into());
        }
        (origin.session.clone(), PathBuf::from("instances").join(&origin.member).join("artifacts").join(key))
    } else {
        if !origin.workspace.is_absolute() {
            return Err("image has no authoritative workspace".into());
        }
        let root = origin.workspace.clone();
        let path = Path::new(path.as_ref());
        let relative = if path.is_absolute() {
            path.strip_prefix(&root).map_err(|_| "image is outside its workspace")?.to_path_buf()
        } else {
            path.to_path_buf()
        };
        (root, relative)
    };
    // Open every component relative to a directory handle; no symlink race can escape the root.
    use rustix::fs::{open, openat, Mode, OFlags};
    let mut dir = open(&root, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())
        .map_err(|e| format!("image root: {e}"))?;
    let mut components = relative.components().peekable();
    let mut file = None;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err("image path contains traversal".into());
        };
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        let fd = openat(
            &dir,
            name,
            if components.peek().is_some() { flags | OFlags::DIRECTORY } else { flags },
            Mode::empty(),
        )
        .map_err(|e| format!("image file: {e}"))?;
        if components.peek().is_none() {
            file = Some(std::fs::File::from(fd));
        } else {
            dir = fd;
        }
    }
    let file = file.ok_or("empty image path")?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_BYTES as u64 {
        return Err("image must be a regular file up to 8 MiB".into());
    }
    let mut bytes = vec![];
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("image exceeds 8 MiB".into());
    }
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    if let Ok(svg) = std::str::from_utf8(bytes) {
        if svg.trim_start().starts_with('<') {
            return rasterize(svg);
        }
    }
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map_err(|e| format!("image decode: {e}"))
}
fn rasterize(svg: &str) -> Result<DynamicImage, String> {
    if svg.len() > MAX_BYTES {
        return Err("SVG size limit exceeded".into());
    }
    // SVG is a drawing, never a file or network loader. Disable both external and embedded images.
    let mut options = resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    static FONTS: std::sync::OnceLock<Arc<resvg::usvg::fontdb::Database>> = std::sync::OnceLock::new();
    options.fontdb = FONTS
        .get_or_init(|| {
            let mut db = resvg::usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone();
    let tree = resvg::usvg::Tree::from_str(svg, &options).map_err(|e| format!("SVG: {e}"))?;
    let size = tree.size();
    let scale = (1920.0 / size.width()).min(1080.0 / size.height()).min(1.0);
    let width = (size.width() * scale).ceil() as u32;
    let height = (size.height() * scale).ceil() as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).ok_or("invalid SVG dimensions")?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for pixel in pixmap.pixels() {
        let p = pixel.demultiply();
        rgba.extend_from_slice(&[p.red(), p.green(), p.blue(), p.alpha()]);
    }
    Ok(DynamicImage::ImageRgba8(RgbaImage::from_raw(width, height, rgba).ok_or("invalid SVG raster")?))
}
fn picture(image: DynamicImage, request: &Request) -> Result<Document, String> {
    let (cw, ch) = request.cell;
    let max_w = (request.width as u32 * cw as u32).min(1920);
    let max_h = (36 * ch as u32).min(1080);
    let image = image.resize(max_w.max(1), max_h.max(1), image::imageops::FilterType::Triangle);
    let columns = image.width().div_ceil(cw as u32) as u16;
    let rows = image.height().div_ceil(ch as u32) as u16;
    let mut canvas =
        RgbaImage::from_pixel(columns as u32 * cw as u32, rows as u32 * ch as u32, image::Rgba([18, 20, 27, 255]));
    image::imageops::overlay(&mut canvas, &image.to_rgba8(), 0, 0);
    if !request.native {
        let small =
            image::imageops::resize(&canvas, columns as u32, rows as u32 * 2, image::imageops::FilterType::Triangle);
        let lines = (0..rows as u32)
            .map(|y| {
                Line::from(
                    (0..columns as u32)
                        .map(|x| {
                            let a = small.get_pixel(x, 2 * y);
                            let b = small.get_pixel(x, 2 * y + 1);
                            Span::styled(
                                "▀",
                                Style::default()
                                    .fg(ratatui::style::Color::Rgb(a[0], a[1], a[2]))
                                    .bg(ratatui::style::Color::Rgb(b[0], b[1], b[2])),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        return Ok(Document::text(lines));
    }
    let pixels = canvas.dimensions();
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(canvas).write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    let id = (uuid::Uuid::new_v4().as_u128() as u32).max(1);
    let image = Arc::new(PreviewImage { png: STANDARD.encode(png.into_inner()), pixels, columns, rows, id });
    Ok(Document {
        lines: vec![Line::raw(" "); rows as usize],
        pictures: vec![Picture { row: 0, column: 0, image }],
        media: vec![],
    })
}

/// Only these application's image IDs are erased. Image placements never enter Ratatui's text buffer.
#[derive(Default)]
pub struct KittyOutput {
    uploaded: HashMap<u32, Arc<PreviewImage>>,
}
impl KittyOutput {
    pub fn clear(&mut self, out: &mut impl Write) -> std::io::Result<()> {
        for id in self.uploaded.keys() {
            write!(out, "\x1b_Ga=d,d=i,i={id},q=2;\x1b\\")?;
        }
        Ok(())
    }
    pub fn paint(&mut self, out: &mut impl Write, placements: &[Placement]) -> std::io::Result<()> {
        let visible: HashSet<u32> = placements.iter().map(|p| p.image.id).collect();
        for id in self.uploaded.keys().filter(|id| !visible.contains(id)) {
            write!(out, "\x1b_Ga=d,d=I,i={id},q=2;\x1b\\")?;
        }
        self.uploaded.retain(|id, _| visible.contains(id));
        if placements.is_empty() {
            return out.flush();
        }
        write!(out, "\x1b7")?;
        for p in placements {
            let image = &p.image;
            if let std::collections::hash_map::Entry::Vacant(entry) = self.uploaded.entry(image.id) {
                let chunks = image.png.as_bytes().chunks(4096);
                let count = chunks.len();
                for (i, chunk) in chunks.enumerate() {
                    if i == 0 {
                        write!(out, "\x1b_Ga=t,f=100,i={},q=2,m={};", image.id, u8::from(i + 1 < count))?;
                    } else {
                        write!(out, "\x1b_Gm={};", u8::from(i + 1 < count))?;
                    }
                    out.write_all(chunk)?;
                    out.write_all(b"\x1b\\")?;
                }
                entry.insert(image.clone());
            }
            let cell_height = image.pixels.1 / image.rows as u32;
            write!(
                out,
                "\x1b[{};{}H\x1b_Ga=p,i={},q=2,C=1,c={},r={},x=0,y={},w={},h={};\x1b\\",
                p.area.y + 1,
                p.area.x + 1,
                image.id,
                p.area.width,
                p.area.height,
                p.skip as u32 * cell_height,
                image.pixels.0,
                p.area.height as u32 * cell_height
            )?;
        }
        write!(out, "\x1b8")?;
        out.flush()
    }
    pub fn finish(&mut self, out: &mut impl Write) -> std::io::Result<()> {
        for id in self.uploaded.keys() {
            write!(out, "\x1b_Ga=d,d=I,i={id},q=2;\x1b\\")?;
        }
        self.uploaded.clear();
        out.flush()
    }
}

/// Normalize native content parts without showing base64 in a text transcript.
pub fn content(value: &Json) -> (String, Vec<Source>) {
    fn parts(value: &Json, images: &mut Vec<Source>, depth: usize) -> Json {
        if depth > 8 {
            return Json::String("[nested content exceeds display depth]".into());
        }
        {
            let url = value["image_url"]["url"].as_str().or_else(|| value["image_url"].as_str()).or_else(|| {
                value["media_type"]
                    .as_str()
                    .filter(|mime| mime.starts_with("image/"))
                    .and_then(|_| value["image"].as_str())
            });
            let embedded = if value["type"] == "image" {
                value["data"]
                    .as_str()
                    .zip(value["mimeType"].as_str())
                    .map(|(data, mime)| format!("data:{mime};base64,{data}"))
                    .or_else(|| {
                        value["source"]["data"]
                            .as_str()
                            .zip(value["source"]["media_type"].as_str())
                            .map(|(data, mime)| format!("data:{mime};base64,{data}"))
                    })
            } else {
                None
            };
            if let Some(url) = url.map(str::to_string).or(embedded) {
                if images.len() >= 16 {
                    return Json::String("[image preview capacity reached]".into());
                }
                if url.len() <= MAX_BYTES * 4 / 3 + 100 {
                    images.push(Source::Image(url));
                    return Json::String("[image preview]".into());
                }
                return Json::String("[image exceeds display limit]".into());
            }
        }
        match value {
            Json::Array(values) => Json::Array(values.iter().map(|v| parts(v, images, depth + 1)).collect()),
            Json::Object(values) => {
                Json::Object(values.iter().map(|(k, v)| (k.clone(), parts(v, images, depth + 1))).collect())
            }
            _ => value.clone(),
        }
    }
    let mut images = vec![];
    let parsed = value.as_str().and_then(|s| serde_json::from_str::<Json>(s).ok());
    let cleaned = parts(parsed.as_ref().unwrap_or(value), &mut images, 0);
    if images.is_empty() && parsed.as_ref().unwrap_or(value) == &cleaned {
        if let Some(text) = value.as_str() {
            return (text.to_string(), images);
        }
    }
    let text = match &cleaned {
        Json::String(s) => s.clone(),
        Json::Null => String::new(),
        Json::Array(values)
            if values.iter().all(|v| {
                v.is_string() || v["type"] == "text" || v["type"] == "input_text" || v["type"] == "output_text"
            }) =>
        {
            values.iter().filter_map(|v| v.as_str().or_else(|| v["text"].as_str())).collect::<Vec<_>>().join("\n")
        }
        _ => serde_json::to_string_pretty(&cleaned).unwrap_or_default(),
    };
    (text, images)
}
