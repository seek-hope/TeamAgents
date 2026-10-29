//! D-447: one transcript projection and presentation for the live conversation and session history.
use crate::{
    graphics::{Document, Graphics, Origin},
    markdown,
    theme::*,
    v2app::{ChatEntry, ChatKind, ToolInfo},
};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value as Json;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

/// Pair results only with a call in the same member and epoch. An orphan remains visible.
/// Display limits never alter the daemon's stored messages.
pub fn project(rows: &[Json], member: &str, bounded: bool) -> Vec<ChatEntry> {
    project_with_receipts(rows, member, bounded, &mut HashMap::new())
}

/// D-479: place local acknowledgements beside their authoritative user message.
/// Remove placed receipts so callers can retain off-page and still-pending ones separately.
pub(crate) fn project_with_receipts(
    rows: &[Json],
    member: &str,
    bounded: bool,
    receipts: &mut HashMap<String, ChatEntry>,
) -> Vec<ChatEntry> {
    let mut entries: Vec<ChatEntry> = Vec::new();
    let mut pending = HashMap::new();
    for row in rows {
        let message = &row["message"];
        let who = row["instance_id"].as_str().unwrap_or(member);
        let (mut content, media) = crate::graphics::content(&message["content"]);
        for attachment in message["attachments"].as_array().into_iter().flatten() {
            content.push_str(&format!(
                "\nAttachment {:?} · {} · {} bytes · SHA-256 {} · snapshot {}",
                attachment["label"].as_str().unwrap_or("unknown"),
                attachment["media_type"].as_str().unwrap_or("unknown"),
                attachment["bytes"],
                attachment["digest"].as_str().unwrap_or("missing"),
                attachment["artifact_id"].as_str().unwrap_or("missing")
            ));
        }
        let workspace = row["workspace"].as_str().unwrap_or("").to_string();
        let kind = match row["kind"].as_str().unwrap_or("") {
            "user" => ChatKind::User,
            "assistant" => ChatKind::Assistant,
            "tool_result" => ChatKind::Tool,
            "runtime" => ChatKind::Runtime,
            "summary" => ChatKind::Summary,
            _ => ChatKind::Context,
        };
        let content = if bounded && kind == ChatKind::Tool {
            preview_output(&content, 20_000)
        } else if bounded && kind == ChatKind::Summary {
            preview(&content, 20_000)
        } else {
            content
        };
        if kind == ChatKind::Tool {
            let id = message["tool_call_id"].as_str().unwrap_or("");
            let key = (who.to_string(), row["epoch"].as_i64(), id.to_string());
            if let Some(index) = pending.remove(&key) {
                let entry: &mut ChatEntry = &mut entries[index];
                entry.tool.as_mut().unwrap().output = Some(content);
                entry.media.extend(media);
            } else {
                entries.push(ChatEntry { kind, who: who.into(), text: content, tool: None, workspace, media });
            }
            continue;
        }
        let label = match kind {
            ChatKind::User if who == "i-leader" || who.is_empty() => "you",
            ChatKind::User => who,
            ChatKind::Runtime => "runtime",
            ChatKind::Summary => "compaction",
            _ => who,
        };
        if !content.trim().is_empty() {
            entries.push(ChatEntry {
                kind: kind.clone(),
                who: label.into(),
                text: content,
                tool: None,
                workspace: workspace.clone(),
                media,
            });
        }
        if kind == ChatKind::User {
            if let Some(receipt) = row["envelope_id"].as_str().and_then(|id| receipts.remove(id)) {
                entries.push(receipt);
            }
        }
        if kind == ChatKind::Assistant {
            for call in message["tool_calls"].as_array().into_iter().flatten() {
                let name = call["function"]["name"].as_str().unwrap_or("tool");
                let args = call["function"]["arguments"].as_str().unwrap_or("");
                let id = call["id"].as_str().unwrap_or("");
                let args = if bounded { preview(args, 20_000) } else { args.into() };
                if !id.is_empty() {
                    pending.insert((who.to_string(), row["epoch"].as_i64(), id.to_string()), entries.len());
                }
                entries.push(ChatEntry {
                    kind: ChatKind::Tool,
                    who: who.into(),
                    text: String::new(),
                    tool: Some(ToolInfo { name: name.into(), arguments: args, output: None }),
                    workspace: workspace.clone(),
                    media: vec![],
                });
            }
        }
    }
    entries
}

/// Keep a long output envelope parseable when limiting its displayed body.
fn preview_output(text: &str, cap: usize) -> String {
    if let Ok(Json::Object(mut object)) = serde_json::from_str::<Json>(text) {
        if object.len() == 1 {
            for key in ["output", "error"] {
                if let Some(body) = object.get(key).and_then(Json::as_str) {
                    let shown = preview(body, cap);
                    object.insert(key.into(), Json::String(shown));
                    return Json::Object(object).to_string();
                }
            }
        }
    }
    preview(text, cap)
}

fn preview(text: &str, cap: usize) -> String {
    let mut chars = text.chars();
    let mut shown: String = chars.by_ref().take(cap).collect();
    if chars.next().is_some() {
        shown.push_str("\n… Display limit reached; full message in /sessions.");
    }
    shown
}

/// Parsing and wrapping are cached independently of the composer and streamed tail.
#[derive(Default)]
pub struct Transcript {
    key: Option<(u64, usize, Option<usize>)>,
    document: Document,
}
impl Transcript {
    pub fn lines(&mut self, entries: &[ChatEntry], width: usize, tool_limit: Option<usize>) -> &[Line<'static>] {
        &self.document(entries, width, tool_limit, None, "").lines
    }
    pub fn document(
        &mut self,
        entries: &[ChatEntry],
        width: usize,
        tool_limit: Option<usize>,
        mut graphics: Option<&mut Graphics>,
        session: &str,
    ) -> &Document {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        entries.hash(&mut hasher);
        session.hash(&mut hasher);
        graphics.as_ref().map(|g| (g.revision, g.enabled, g.native, g.cell)).hash(&mut hasher);
        let key = (hasher.finish(), width, tool_limit);
        if self.key != Some(key) {
            self.document = Document::default();
            for (position, entry) in entries.iter().enumerate() {
                let tool_continuation = entry.tool.is_some()
                    && position > 0
                    && entries[position - 1].tool.is_some()
                    && entries[position - 1].who == entry.who;
                if !self.document.lines.is_empty() && !tool_continuation {
                    self.document.lines.push(Line::default());
                }
                if entry.tool.is_some() && !tool_continuation {
                    let count =
                        entries[position..].iter().take_while(|e| e.tool.is_some() && e.who == entry.who).count();
                    if count > 1 {
                        self.document.lines.extend(markdown::plain(
                            &activity_heading(&entries[position..position + count]),
                            width,
                            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                        ));
                    }
                }
                let origin = Origin {
                    session: session.into(),
                    workspace: (&entry.workspace).into(),
                    position,
                    member: if entry.who == "you" { "i-leader".into() } else { entry.who.clone() },
                };
                if let Some(g) = graphics.as_deref_mut() {
                    if entry.kind == ChatKind::Assistant {
                        self.document.append(assistant_document(
                            &entry.text,
                            &entry.who,
                            width,
                            Some(g),
                            &origin,
                            false,
                        ));
                    } else if entry.kind == ChatKind::Summary {
                        self.document.lines.extend(markdown::plain(
                            "Conversation compacted",
                            width,
                            Style::default().fg(GREY),
                        ));
                        self.document.append(markdown::render_media(&entry.text, width, Some(g), &origin));
                    } else {
                        self.document.lines.extend(render_entry(entry, width, tool_limit));
                        if entry.kind == ChatKind::User {
                            for event in pulldown_cmark::Parser::new(&entry.text) {
                                if let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image { dest_url, .. }) = event
                                {
                                    self.document.append(g.block(
                                        crate::graphics::Source::Image(dest_url.into_string()),
                                        &origin,
                                        width,
                                    ));
                                }
                            }
                        }
                    }
                    for source in &entry.media {
                        self.document.append(g.block(source.clone(), &origin, width));
                    }
                } else {
                    self.document.lines.extend(render_entry(entry, width, tool_limit));
                }
            }
            self.key = Some(key);
        }
        &self.document
    }
}

pub fn assistant(text: &str, who: &str, width: usize, streaming: bool) -> Vec<Line<'static>> {
    let mut out = markdown::render(text, width.saturating_sub(2).max(1));
    mark_response(&mut out, width, who, streaming);
    out
}

pub(crate) fn assistant_document(
    text: &str,
    who: &str,
    width: usize,
    graphics: Option<&mut Graphics>,
    origin: &Origin,
    streaming: bool,
) -> Document {
    let mut body = markdown::render_media(text, width.saturating_sub(2).max(1), graphics, origin);
    let before = body.lines.len();
    mark_response(&mut body.lines, width, who, streaming);
    body.offset(body.lines.len().saturating_sub(before), usize::from(width > 2) * 2);
    body
}

/// A two-cell speaker gutter preserves Markdown styles and aligns continuation rows.
fn mark_response(lines: &mut Vec<Line<'static>>, width: usize, who: &str, streaming: bool) {
    if width == 0 {
        lines.clear();
        return;
    }
    if width > 2 {
        for (i, line) in lines.iter_mut().enumerate() {
            line.spans.insert(0, Span::styled(if i == 0 { "● " } else { "  " }, Style::default().fg(CODEX)));
        }
    }
    if streaming || (!who.is_empty() && who != "i-leader") {
        let who = if who.is_empty() || who == "i-leader" { "Leader" } else { who };
        let label = if streaming { format!("{who} · streaming") } else { who.to_string() };
        let mut heading = markdown::plain(&label, width, Style::default().fg(CODEX));
        heading.append(lines);
        *lines = heading;
    }
}

/// Name a consecutive tool group by its actual operations, following Grok/Codex's
/// task-oriented activity summaries without inferring semantic success from receipts.
fn activity_heading(entries: &[ChatEntry]) -> String {
    let names = entries.iter().filter_map(|entry| entry.tool.as_ref().map(|tool| tool.name.as_str()));
    let names = names.collect::<Vec<_>>();
    let count = names.len();
    let all = |allowed: &[&str]| names.iter().all(|name| allowed.contains(name));
    if all(&["shell"]) {
        format!("Ran {count} commands")
    } else if all(&["edit_file", "edit_files", "write_file"]) {
        format!("Edited · {count} operations")
    } else if all(&["read_file", "read_artifact", "read_history", "grep", "glob", "skill"]) {
        format!("Explored · {count} tools")
    } else if all(&["web_search", "web_fetch"]) {
        format!("Searched · {count} tools")
    } else if all(&["spawn", "delegate", "send_message", "cancel_task", "verify_task", "wait"]) {
        format!("Coordinated · {count} actions")
    } else {
        format!("Worked · {count} tools")
    }
}

fn render_entry(entry: &ChatEntry, width: usize, tool_limit: Option<usize>) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![];
    }
    if let Some(tool) = &entry.tool {
        return render_tool(tool, width, tool_limit);
    }
    if entry.kind == ChatKind::Assistant {
        return assistant(&entry.text, &entry.who, width, false);
    }
    if entry.kind == ChatKind::User {
        let body = if entry.who != "you" { format!("To {}: {}", entry.who, entry.text) } else { entry.text.clone() };
        let mut lines = markdown::plain(&body, width.saturating_sub(2).max(1), Style::default().fg(FG));
        for (i, line) in lines.iter_mut().enumerate() {
            if width > 2 {
                line.spans.insert(0, Span::styled(if i == 0 { "❯ " } else { "  " }, Style::default().fg(USER)));
            }
            line.spans.push(Span::raw(" ".repeat(width.saturating_sub(line.width()))));
            line.style = Style::default().bg(USER_BG);
        }
        // Grok Build's elevated user-prompt band: a full-width surface with vertical breathing room.
        // The fill is presentation only; prompt bytes and history remain unchanged.
        if width >= 8 {
            let padding = || {
                let mut line = Line::from(Span::raw(" ".repeat(width)));
                line.style = Style::default().bg(USER_BG);
                line
            };
            lines.insert(0, padding());
            lines.push(padding());
        }
        return lines;
    }
    let (label, color) = match entry.kind {
        ChatKind::User => ("You", USER),
        ChatKind::Tool => ("Tool result · call outside this history page", GREY),
        ChatKind::Runtime => ("Runtime", MUTED),
        ChatKind::Context => ("Team note", GREY),
        ChatKind::Summary => ("Conversation compacted", GREY),
        ChatKind::Error => ("Error", ERROR),
        ChatKind::Command => ("TeamAgents", ACCENT),
        _ => ("Notice", GREY),
    };
    let mut out = markdown::plain(label, width, Style::default().fg(color).add_modifier(Modifier::BOLD));
    out.extend(if entry.kind == ChatKind::Summary {
        markdown::render(&entry.text, width)
    } else {
        // Labels carry the speaker or severity color; the message itself stays readable prose.
        markdown::plain(&entry.text, width, Style::default().fg(NOTICE))
    });
    out
}

fn render_tool(tool: &ToolInfo, width: usize, limit: Option<usize>) -> Vec<Line<'static>> {
    let status = if tool.output.is_some() { "result received" } else { "awaiting result" };
    let args: Json = serde_json::from_str(&tool.arguments).unwrap_or(Json::Null);
    let compact = limit == Some(4);
    if tool.name == "shell" {
        return render_shell(tool, &args, width, limit);
    }
    let summary = if tool.name == "finish" && compact {
        format!("claimed {}", args["status"].as_str().unwrap_or("unspecified outcome"))
    } else {
        args["command"]
            .as_str()
            .map(|c| format!("$ {c}"))
            .or_else(|| {
                ["path", "query", "url", "description"]
                    .into_iter()
                    .find_map(|key| args[key].as_str().map(str::to_string))
            })
            .unwrap_or_else(|| tool.arguments.clone())
    };
    let header = Line::from(vec![
        Span::styled("› ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(markdown::clean(&tool.name), Style::default().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" · {status}"), Style::default().fg(MUTED)),
        Span::styled(
            if compact { format!("  {}", markdown::clean(&summary).replace('\n', " ")) } else { String::new() },
            Style::default().fg(CODE),
        ),
    ]);
    let mut out = crate::wrap::wrap_line(&header, width);
    if compact {
        shorten(&mut out, 2, width);
    }
    let detail = if compact {
        String::new()
    } else if args.is_null() {
        tool.arguments.clone()
    } else {
        serde_json::to_string_pretty(&args).unwrap_or_default()
    };
    let mut args = if compact {
        vec![]
    } else {
        markdown::plain(&detail, width.saturating_sub(2).max(1), Style::default().fg(CODE))
    };
    if let Some(limit) = limit {
        shorten(&mut args, limit.max(2), width.saturating_sub(2).max(1));
    }
    let mut body = args;
    if let Some(output) = &tool.output {
        let display = if tool.name != "shell" {
            serde_json::from_str::<Json>(output)
                .ok()
                .filter(|v| v.is_object() || v.is_array())
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
                .unwrap_or_else(|| output.clone())
        } else {
            output.clone()
        };
        let mut rows = markdown::plain(&display, width.saturating_sub(2).max(1), Style::default().fg(NOTICE));
        if matches!(tool.name.as_str(), "edit_file" | "edit_files") {
            for line in &mut rows {
                let first = line.spans.first().and_then(|s| s.content.chars().next());
                let color = match first {
                    Some('+') => ADDED,
                    Some('-') => ERROR,
                    Some('@') => ACCENT,
                    _ => NOTICE,
                };
                for span in &mut line.spans {
                    span.style = span.style.fg(color);
                }
            }
        }
        if let Some(limit) = limit {
            shorten(&mut rows, limit, width.saturating_sub(2).max(1));
        }
        body.extend(rows);
    }
    for mut line in body {
        if width > 2 {
            line.spans.insert(0, Span::styled("  ", Style::default().fg(MUTED)));
        }
        out.push(line);
    }
    out
}

/// Shell receipts carry one JSON output/error envelope; decode that envelope once, retaining literal logs.
/// The transcript has no authoritative exit metadata, so receiving a body never implies success.
fn render_shell(tool: &ToolInfo, args: &Json, width: usize, limit: Option<usize>) -> Vec<Line<'static>> {
    let status = if tool.output.is_some() { "result received" } else { "awaiting result" };
    let mut out = crate::wrap::wrap_line(
        &Line::from(vec![
            Span::styled("› shell", Style::default().fg(FG).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" · {status}"), Style::default().fg(MUTED)),
        ]),
        width,
    );
    let indent = if width > 4 { 4 } else { 0 };
    let inner = width.saturating_sub(indent).max(1);
    let command = args["command"].as_str().unwrap_or(&tool.arguments);
    let mut command = markdown::plain(command, inner, Style::default().fg(CODE));
    if let Some(limit) = limit {
        shorten(&mut command, limit.max(8), inner);
    }
    for (index, mut line) in command.into_iter().enumerate() {
        if indent > 0 {
            line.spans.insert(0, Span::styled(if index == 0 { "  $ " } else { "    " }, Style::default().fg(CODE)));
        }
        out.push(line);
    }
    if limit != Some(4) {
        out.extend(markdown::plain("  Arguments", width, Style::default().fg(MUTED)));
        let detail = serde_json::to_string_pretty(args).unwrap_or_else(|_| tool.arguments.clone());
        let detail = if args.is_null() { tool.arguments.as_str() } else { &detail };
        let mut rows = markdown::plain(detail, inner, Style::default().fg(GREY));
        if let Some(limit) = limit {
            shorten(&mut rows, limit, inner);
        }
        out.extend(rows.into_iter().map(|mut line| {
            if indent > 0 {
                line.spans.insert(0, Span::raw("    "));
            }
            line
        }));
    }
    if let Some(raw) = &tool.output {
        let value = serde_json::from_str::<Json>(raw).ok();
        let envelope = value.as_ref().and_then(Json::as_object).filter(|obj| obj.len() == 1);
        let error = envelope.and_then(|obj| obj.get("error")).and_then(Json::as_str);
        let output = envelope.and_then(|obj| obj.get("output")).and_then(Json::as_str).or(error).unwrap_or(raw);
        let color = if error.is_some() { ERROR } else { NOTICE };
        out.extend(markdown::plain(
            if error.is_some() { "  Error" } else { "  Output" },
            width,
            Style::default().fg(color),
        ));
        let mut rows =
            markdown::plain(if output.is_empty() { "(no output)" } else { output }, inner, Style::default().fg(color));
        if let Some(limit) = limit {
            shorten(&mut rows, limit, inner);
        }
        for mut line in rows {
            if indent > 0 {
                line.spans.insert(0, Span::styled("  │ ", Style::default().fg(MUTED)));
            }
            out.push(line);
        }
    }
    out
}

fn shorten(lines: &mut Vec<Line<'static>>, limit: usize, width: usize) {
    if lines.len() <= limit {
        return;
    }
    let omitted = lines.len() - limit;
    let tail = lines.split_off(lines.len() - limit / 2);
    lines.truncate(limit - limit / 2);
    let hint = if limit > 4 { "/sessions for full output" } else { "Ctrl+O details" };
    lines.extend(markdown::plain(&format!("… {omitted} more lines · {hint}"), width, Style::default().fg(GREY)));
    lines.extend(tail);
}
