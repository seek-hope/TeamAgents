//! Layered Codex/Herdr-inspired Ratatui renderer with exact single-source hitbox emission.
use crate::{
    protocol::{
        outcome_label, preview, sanitize_inline, sanitize_multiline, ActionPreview, Policy, StageKind, TaskStatus,
    },
    sandbox::SandboxMode,
    tui::{
        input::matching_commands,
        state::{
            AppState, CatalogState, Focus, HitMap, HitTarget, InspectorTab, ModelField, Overlay, SidebarTab,
            EFFORT_OPTIONS, WIDE_THRESHOLD,
        },
        theme::{truncate_display, Palette},
    },
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Frame,
};
use std::collections::HashMap;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) fn render(frame: &mut Frame, state: &mut AppState) -> HitMap {
    let area = frame.area();
    state.last_width = area.width;
    let pal = state.palette;
    let mut hits = HitMap { viewport: area, entries: Vec::with_capacity(64) };

    frame.render_widget(Block::default().style(pal.canvas()), area);
    if area.height < 4 || area.width < 24 {
        return hits;
    }

    let main_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);
    let body = main_rows[0];
    let footer = main_rows[1];

    let wide = area.width >= WIDE_THRESHOLD;
    if wide && state.sidebar_visible {
        let side_w = (area.width * 28 / 100).clamp(28, 38);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(side_w), Constraint::Length(1), Constraint::Min(40)])
            .split(body);
        render_sidebar(frame, cols[0], state, &pal, &mut hits);
        let comp_row = body.height.saturating_sub(composer_height(cols[2].width, state));
        render_vertical_divider(frame, cols[1], state, &pal, comp_row);
        render_workspace(frame, cols[2], state, &pal, &mut hits, wide);
    } else if !wide && state.focus == Focus::Sidebar {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(4), Constraint::Length(composer_height(area.width, state))])
            .split(body);
        render_sidebar(frame, rows[0], state, &pal, &mut hits);
        render_composer_dock(frame, rows[1], state, &pal, &mut hits);
    } else {
        render_workspace(frame, body, state, &pal, &mut hits, wide);
    }

    render_status_bar(frame, footer, state, &pal, &mut hits);

    if state.overlay != Overlay::None {
        render_modal(frame, area, state, &pal, &mut hits);
    }
    hits
}

fn composer_height(width: u16, state: &AppState) -> u16 {
    let text_w = width.saturating_sub(5) as usize;
    let (wrapped, _, _) = state.input.visual_layout(text_w);
    let content_h = (wrapped.len() as u16).clamp(1, 4);
    content_h + 1
}

fn render_vertical_divider(frame: &mut Frame, area: Rect, state: &AppState, pal: &Palette, comp_row: u16) {
    let focused = state.focus == Focus::Sidebar || state.focus == Focus::Inspector;
    let style = pal.border(focused).bg(pal.canvas_bg);
    let lines: Vec<_> = (0..area.height)
        .map(|row| {
            let glyph = if row == 1 {
                "┼"
            } else if row == comp_row && comp_row > 1 {
                "├"
            } else {
                "│"
            };
            Line::from(Span::styled(glyph, style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn paint_hline(frame: &mut Frame, area: Rect, style: Style) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let line = "─".repeat(area.width as usize);
    frame.render_widget(Paragraph::new(Line::from(Span::styled(line, style))), area);
}

fn paint_divider(frame: &mut Frame, area: Rect, focused: bool, bg: Color, pal: &Palette) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if !focused || pal.is_plain() {
        paint_hline(frame, area, pal.border(focused).bg(bg));
        return;
    }
    let w = area.width as usize;
    let n = pal.rainbow.len();
    let mut spans = Vec::with_capacity(n);
    for i in 0..n {
        let start = i * w / n;
        let end = (i + 1) * w / n;
        if end > start {
            spans.push(Span::styled("─".repeat(end - start), Style::default().fg(pal.rainbow[i]).bg(bg)));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn rainbow_text_spans(text: &str, bg: Color, bold: bool, pal: &Palette) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut color_idx = 0usize;
    for ch in text.chars() {
        let mut style = if ch.is_whitespace() {
            Style::default().fg(pal.text_primary)
        } else {
            let c = pal.rainbow_color(color_idx);
            color_idx += 1;
            Style::default().fg(c)
        };
        style = style.bg(bg);
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(ch.to_string(), style));
    }
    spans
}

struct HitSpanWriter<'a> {
    x: u16,
    y: u16,
    max_x: u16,
    spans: Vec<Span<'static>>,
    hits: &'a mut HitMap,
}

impl<'a> HitSpanWriter<'a> {
    fn new(area: Rect, hits: &'a mut HitMap) -> Self {
        Self { x: area.x, y: area.y, max_x: area.x + area.width, spans: Vec::new(), hits }
    }

    fn push(&mut self, text: impl Into<String>, style: Style, target: Option<HitTarget>) {
        let s: String = text.into();
        let w = s.width() as u16;
        let vis_w = w.min(self.max_x.saturating_sub(self.x));
        if vis_w == 0 {
            return;
        }
        let fitted = if vis_w < w { truncate_display(&s, vis_w as usize) } else { s };
        let actual_w = fitted.width() as u16;
        if actual_w == 0 {
            return;
        }
        if let Some(t) = target {
            self.hits.push(Rect::new(self.x, self.y, actual_w, 1), t);
        }
        self.spans.push(Span::styled(fitted, style));
        self.x += actual_w;
    }

    fn finish(self) -> Line<'static> {
        Line::from(self.spans)
    }
}

struct SidebarRowItem {
    line0: Line<'static>,
    line1: Option<Line<'static>>,
    target: HitTarget,
    chevron_x: Option<(u16, u16, HitTarget)>,
    selected: bool,
    danger: bool,
}

fn render_sidebar(frame: &mut Frame, area: Rect, state: &mut AppState, pal: &Palette, hits: &mut HitMap) {
    frame.render_widget(Block::default().style(pal.sidebar()), area);
    hits.push(area, HitTarget::SidebarPane);
    if area.height < 3 || area.width < 12 {
        return;
    }

    let focused = state.focus == Focus::Sidebar;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)])
        .split(area);

    // Header row with exact clickable tab pills.
    let hdr = rows[0];
    let mut writer = HitSpanWriter::new(hdr, hits);
    writer.push(" ", pal.sidebar(), None);
    let sess_lbl = format!(" Sessions ({}) ", state.tasks.len());
    let mem_lbl = format!(" Agents ({}) ", state.members.len());
    let sess_style =
        if state.sidebar_tab == SidebarTab::Trees { pal.pill(pal.rainbow[4]) } else { pal.pill_inactive() };
    let mem_style =
        if state.sidebar_tab == SidebarTab::Members { pal.pill(pal.rainbow[6]) } else { pal.pill_inactive() };
    writer.push(sess_lbl, sess_style, Some(HitTarget::SidebarTab(SidebarTab::Trees)));
    writer.push(" ", pal.sidebar(), None);
    writer.push(mem_lbl, mem_style, Some(HitTarget::SidebarTab(SidebarTab::Members)));
    if hdr.x + hdr.width >= writer.x + 8 {
        writer.push(" ", pal.sidebar(), None);
        let new_style = if state.selected_task_id.is_none() { pal.pill(pal.rainbow[2]) } else { pal.pill_inactive() };
        writer.push(" +New ", new_style, Some(HitTarget::NewSession));
    }
    if hdr.x + hdr.width > writer.x + 8 {
        let right = truncate_display(&state.workspace_name, (hdr.x + hdr.width - writer.x - 2) as usize);
        let pad = (hdr.x + hdr.width - writer.x - 1).saturating_sub(right.width() as u16);
        if pad > 0 {
            writer.push(" ".repeat(pad as usize), pal.sidebar(), None);
        }
        writer.push(right, pal.dim().bg(pal.sidebar_bg), None);
    }
    frame.render_widget(Paragraph::new(writer.finish()), hdr);
    paint_divider(frame, rows[1], focused, pal.sidebar_bg, pal);

    let list_area = rows[2];
    if list_area.height == 0 || list_area.width < 6 {
        return;
    }

    let two_line_cards = list_area.height >= 12;
    let mut items: Vec<SidebarRowItem> = Vec::new();
    let mut selected_item_idx = None;
    let inner_w = list_area.width.saturating_sub(1) as usize;

    match state.sidebar_tab {
        SidebarTab::Trees => {
            if state.tree_rows.is_empty() {
                state.sidebar_scroll = 0;
                state.sidebar_max_scroll = 0;
                let msg = Line::from(vec![Span::styled("  No sessions yet.", pal.muted())]);
                let hint = Line::from(vec![Span::styled("  Submit a prompt below.", pal.dim())]);
                frame.render_widget(Paragraph::new(vec![Line::from(""), msg, hint]), list_area);
                return;
            }
            for row in &state.tree_rows {
                let Some(task) = state.tasks.iter().find(|t| t.id == row.task_id) else {
                    continue;
                };
                let selected = state.selected_task_id.as_deref() == Some(row.task_id.as_str());
                let is_armed_delete = state.armed_delete.as_ref().is_some_and(|a| a.root_id == row.root_id);
                if selected {
                    selected_item_idx = Some(items.len());
                }
                let bg = if is_armed_delete && selected {
                    pal.danger_bg
                } else if selected {
                    pal.selected_bg
                } else {
                    pal.sidebar_bg
                };
                let bar_fg = if is_armed_delete {
                    pal.error
                } else if selected {
                    if focused {
                        pal.accent
                    } else {
                        pal.text_secondary
                    }
                } else {
                    pal.sidebar_bg
                };
                let (glyph, glyph_style) = pal.status_badge(&task.status, state.tick);
                let suffix = if row.is_root {
                    match (task.turns_used, task.turn_budget) {
                        (Some(u), Some(b)) => format!(" {u}/{b}"),
                        (Some(u), None) => format!(" {u}t"),
                        _ => String::new(),
                    }
                } else if task.kind == StageKind::Summarizer {
                    " summary".into()
                } else if task.kind == StageKind::Repair {
                    " repair".into()
                } else {
                    format!(" {}", task.status.as_str())
                };
                let fold_str = if row.is_root && row.has_children {
                    if row.collapsed {
                        "▸ "
                    } else {
                        "▾ "
                    }
                } else if row.is_root {
                    "  "
                } else {
                    ""
                };
                let chevron_x = if row.is_root && row.has_children {
                    let start = list_area.x + 1;
                    Some((
                        start,
                        2,
                        HitTarget::SidebarSessionRow {
                            task_id: row.task_id.clone(),
                            root_id: row.root_id.clone(),
                            toggle_fold: true,
                        },
                    ))
                } else {
                    None
                };
                let bar = if selected || is_armed_delete { "▎" } else { " " };
                let head_w =
                    bar.width() + fold_str.width() + row.prefix.width() + glyph.width() + 1 + suffix.width() + 1;
                let title_budget = inner_w.saturating_sub(head_w);
                let raw_label = if row.is_root {
                    task.prompt.as_deref().unwrap_or(&task.agent)
                } else if task.kind == StageKind::Summarizer {
                    "◆ summarizer"
                } else {
                    &task.agent
                };
                let label = truncate_display(raw_label, title_budget);
                let title_style = if is_armed_delete && selected {
                    pal.danger_row()
                } else if selected {
                    pal.selected_row()
                } else if row.is_root {
                    pal.primary().bg(bg)
                } else {
                    pal.muted().bg(bg)
                };
                let mut l0 = vec![
                    Span::styled(bar, Style::default().fg(bar_fg).bg(bg)),
                    Span::styled(format!("{fold_str}{}", row.prefix), pal.dim().bg(bg)),
                    Span::styled(glyph, glyph_style.bg(bg)),
                    Span::styled(format!(" {label}"), title_style),
                ];
                let used0: usize = l0.iter().map(|s| s.content.width()).sum();
                let rem_pad0 = inner_w.saturating_sub(used0 + suffix.width() + 1);
                if rem_pad0 > 0 {
                    l0.push(Span::styled(" ".repeat(rem_pad0), Style::default().bg(bg)));
                }
                if !suffix.is_empty() {
                    l0.push(Span::styled(suffix, pal.muted().bg(bg)));
                }
                let final_pad0 = inner_w.saturating_sub(l0.iter().map(|s| s.content.width()).sum());
                if final_pad0 > 0 {
                    l0.push(Span::styled(" ".repeat(final_pad0), Style::default().bg(bg)));
                }

                let line1 = if two_line_cards && row.is_root {
                    let meta_str = if is_armed_delete {
                        "Press Del again to delete".to_owned()
                    } else {
                        let short_id: String = row.root_id.chars().take(6).collect();
                        let rnd = task.round.filter(|&r| r > 1).map(|r| format!(" · R{r}")).unwrap_or_default();
                        let pol = task.policy.map_or("", |p| if p == Policy::Review { " · review" } else { "" });
                        let v = task.check.as_deref().map_or("", |_| " · check");
                        format!("#{short_id}{rnd} · @{}{pol}{v}", sanitize_inline(&task.agent))
                    };
                    let m_budget = inner_w.saturating_sub(5);
                    let m_fit = truncate_display(&meta_str, m_budget);
                    let m_style = if is_armed_delete { pal.danger_row() } else { pal.dim().bg(bg) };
                    let mut l1 = vec![
                        Span::styled(bar, Style::default().fg(bar_fg).bg(bg)),
                        Span::styled(format!("   {m_fit}"), m_style),
                    ];
                    let pad1 = inner_w.saturating_sub(l1.iter().map(|s| s.content.width()).sum());
                    if pad1 > 0 {
                        l1.push(Span::styled(" ".repeat(pad1), Style::default().bg(bg)));
                    }
                    Some(Line::from(l1))
                } else {
                    None
                };

                items.push(SidebarRowItem {
                    line0: Line::from(l0),
                    line1,
                    target: HitTarget::SidebarSessionRow {
                        task_id: row.task_id.clone(),
                        root_id: row.root_id.clone(),
                        toggle_fold: false,
                    },
                    chevron_x,
                    selected,
                    danger: is_armed_delete,
                });
            }
        }
        SidebarTab::Members => {
            if state.members.is_empty() {
                state.sidebar_scroll = 0;
                state.sidebar_max_scroll = 0;
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled("  No members registered.", pal.muted()))),
                    list_area,
                );
                return;
            }
            for (idx, member) in state.members.iter().enumerate() {
                let selected = idx == state.selected_member_idx;
                if selected {
                    selected_item_idx = Some(items.len());
                }
                let bg = if selected { pal.selected_bg } else { pal.sidebar_bg };
                let bar_fg = if selected { pal.accent } else { pal.sidebar_bg };
                let is_target = member.name == state.agent;
                let active_tasks =
                    state.tasks.iter().filter(|t| t.agent == member.name && t.status.is_active()).count();
                let marker = if is_target { "●" } else { "○" };
                let badge = if active_tasks > 0 {
                    format!(" {active_tasks} active")
                } else if is_target {
                    " target".into()
                } else {
                    String::new()
                };
                let name_budget = inner_w.saturating_sub(5 + badge.width());
                let name = truncate_display(&member.name, name_budget);
                let m_style = if is_target { pal.ok().bg(bg) } else { pal.dim().bg(bg) };
                let name_style =
                    if selected { pal.selected_row() } else { Style::default().fg(pal.text_primary).bg(bg) };
                let bar = if selected { "▎" } else { " " };
                let mut l0 = vec![
                    Span::styled(bar, Style::default().fg(bar_fg).bg(bg)),
                    Span::styled(format!(" {marker} "), m_style),
                    Span::styled(name, name_style),
                ];
                let pad_mid =
                    inner_w.saturating_sub(l0.iter().map(|s| s.content.width()).sum::<usize>() + badge.width() + 1);
                if pad_mid > 0 {
                    l0.push(Span::styled(" ".repeat(pad_mid), Style::default().bg(bg)));
                }
                if !badge.is_empty() {
                    let b_style = if active_tasks > 0 { pal.active().bg(bg) } else { pal.muted().bg(bg) };
                    l0.push(Span::styled(badge, b_style));
                }
                let pad_end = inner_w.saturating_sub(l0.iter().map(|s| s.content.width()).sum());
                if pad_end > 0 {
                    l0.push(Span::styled(" ".repeat(pad_end), Style::default().bg(bg)));
                }
                items.push(SidebarRowItem {
                    line0: Line::from(l0),
                    line1: None,
                    target: HitTarget::SidebarMemberRow(idx),
                    chevron_x: None,
                    selected,
                    danger: false,
                });
            }
        }
    }

    // Compute exact line offsets and clamp scroll while honoring follow-selection.
    let mut offsets = Vec::with_capacity(items.len());
    let mut total_rows = 0usize;
    for item in &items {
        let h = if item.line1.is_some() { 2 } else { 1 };
        offsets.push((total_rows, h));
        total_rows += h;
    }
    let view_h = list_area.height as usize;
    let max_scroll = total_rows.saturating_sub(view_h);
    state.sidebar_max_scroll = max_scroll;
    if state.sidebar_follow_selection {
        if let Some((sy, sh)) = selected_item_idx.and_then(|i| offsets.get(i).copied()) {
            if sy < state.sidebar_scroll {
                state.sidebar_scroll = sy;
            } else if sy + sh > state.sidebar_scroll + view_h {
                state.sidebar_scroll = (sy + sh).saturating_sub(view_h);
            }
        }
    }
    state.sidebar_scroll = state.sidebar_scroll.min(max_scroll);

    let scroll = state.sidebar_scroll;
    let content_rect = Rect { width: inner_w as u16, ..list_area };
    let mut visible_lines: Vec<Line<'static>> = Vec::with_capacity(view_h);
    for (item, (iy, ih)) in items.into_iter().zip(offsets) {
        let mut vis_start_y = None;
        let mut vis_rows = 0u16;
        let _ = (item.selected, item.danger);
        for (sub_idx, line_opt) in [Some(item.line0), item.line1].into_iter().enumerate() {
            let Some(line) = line_opt else { continue };
            let abs_y = iy + sub_idx;
            if abs_y >= scroll && abs_y < scroll + view_h {
                let sy = list_area.y + (abs_y - scroll) as u16;
                if vis_start_y.is_none() {
                    vis_start_y = Some(sy);
                }
                vis_rows += 1;
                visible_lines.push(line);
                if sub_idx == 0 {
                    if let Some((cx, cw, chev_target)) = &item.chevron_x {
                        hits.push(Rect::new(*cx, sy, *cw, 1), chev_target.clone());
                    }
                }
            }
            let _ = ih;
        }
        if let Some(sy) = vis_start_y {
            hits.push(Rect::new(content_rect.x, sy, content_rect.width, vis_rows), item.target);
        }
    }
    frame.render_widget(Paragraph::new(visible_lines), content_rect);
    if total_rows > view_h {
        let mut sb = ScrollbarState::new(max_scroll + 1).position(scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight).style(pal.dim()),
            list_area,
            &mut sb,
        );
    }
}

fn render_workspace(frame: &mut Frame, area: Rect, state: &mut AppState, pal: &Palette, hits: &mut HitMap, wide: bool) {
    let has_alert = !state.pending_approvals().is_empty() || !state.pending_questions().is_empty();
    let comp_h = composer_height(area.width, state);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(if has_alert { 1 } else { 0 }),
            Constraint::Length(comp_h),
        ])
        .split(area);

    render_workspace_header(frame, chunks[0], state, pal, hits, wide);
    paint_divider(frame, chunks[1], state.focus == Focus::Inspector, pal.canvas_bg, pal);
    render_inspector_body(frame, chunks[2], state, pal, hits);
    if has_alert {
        render_alert_strip(frame, chunks[3], state, pal, hits);
    }
    render_composer_dock(frame, chunks[4], state, pal, hits);

    if state.focus == Focus::Composer && state.overlay == Overlay::None {
        if !matching_commands(state.input.as_str()).is_empty() {
            render_slash_popup(frame, chunks[4], area, state, pal, hits);
        } else if state.active_mention().is_some() {
            render_mention_popup(frame, chunks[4], area, state, pal, hits);
        }
    }
}

fn render_workspace_header(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    pal: &Palette,
    hits: &mut HitMap,
    wide: bool,
) {
    let tabs: &[(InspectorTab, &str)] = if area.width < 78 {
        &[
            (InspectorTab::Summary, " 1·Sum "),
            (InspectorTab::Evidence, " 2·Evd "),
            (InspectorTab::Tree, " 3·Tre "),
            (InspectorTab::Events, " 4·Log "),
            (InspectorTab::Board, " 5·Brd "),
        ]
    } else {
        &[
            (InspectorTab::Summary, " 1 Summary "),
            (InspectorTab::Evidence, " 2 Evidence "),
            (InspectorTab::Tree, " 3 Tree "),
            (InspectorTab::Events, " 4 Log "),
            (InspectorTab::Board, " 5 Board "),
        ]
    };
    let tabs_w: usize = tabs.iter().map(|(_, l)| l.width()).sum::<usize>() + (tabs.len() - 1);
    let left_budget = (area.width as usize).saturating_sub(tabs_w + 3);

    let mut writer = HitSpanWriter::new(area, hits);
    writer.push(" ", pal.canvas(), None);
    if !wide || !state.sidebar_visible {
        let lbl = if wide { " ☰ Sidebar " } else { " ☰ Sessions " };
        writer.push(lbl, pal.pill_inactive(), Some(HitTarget::SidebarToggle));
        writer.push(" ", pal.canvas(), None);
    }

    let used_toggle = (writer.x - area.x) as usize;
    let remain_left = (area.width as usize).saturating_sub(tabs_w + used_toggle + 2).min(left_budget.max(10));
    match state.selected_task() {
        Some(t) => {
            let (badge, bstyle) = pal.status_badge(&t.status, state.tick);
            let title_text = t
                .prompt
                .as_deref()
                .map(|p| truncate_display(p, remain_left.saturating_sub(18).max(8)))
                .unwrap_or_else(|| format!("@{} ({})", sanitize_inline(&t.agent), preview(&t.id, 10)));
            writer.push(format!("{badge} "), bstyle.bg(pal.canvas_bg), None);
            writer.push(title_text, pal.primary().bg(pal.canvas_bg).add_modifier(Modifier::BOLD), None);
            if remain_left > 24 {
                writer.push(format!("  @{}", sanitize_inline(&t.agent)), pal.muted().bg(pal.canvas_bg), None);
            }
        }
        None => {
            let brand = truncate_display("◆ TeamAgents", remain_left);
            for span in rainbow_text_spans(&brand, pal.canvas_bg, true, pal) {
                writer.push(span.content, span.style, None);
            }
            let ws_budget = remain_left.saturating_sub(brand.width());
            if ws_budget > 3 {
                let rest = truncate_display(&format!(" · {}", state.workspace_name), ws_budget);
                writer.push(rest, pal.primary().bg(pal.canvas_bg).add_modifier(Modifier::BOLD), None);
            }
        }
    }

    let target_x = area.x + area.width.saturating_sub(tabs_w as u16 + 1);
    if target_x > writer.x {
        writer.push(" ".repeat((target_x - writer.x) as usize), pal.canvas(), None);
    }

    for (i, (tab, lbl)) in tabs.iter().enumerate() {
        if i > 0 {
            writer.push(" ", pal.canvas(), None);
        }
        let tab_color = match tab {
            InspectorTab::Summary => pal.rainbow[4],
            InspectorTab::Evidence => pal.rainbow[3],
            InspectorTab::Tree => pal.rainbow[2],
            InspectorTab::Events => pal.rainbow[6],
            InspectorTab::Board => pal.rainbow[5],
        };
        let style = if state.inspector_tab == *tab { pal.pill(tab_color) } else { pal.pill_inactive() };
        writer.push(*lbl, style, Some(HitTarget::InspectorTab(*tab)));
    }
    frame.render_widget(Paragraph::new(writer.finish()), area);
}

#[derive(Clone)]
struct DocSpan {
    text: String,
    style: Style,
    hit: Option<HitTarget>,
}

struct DocLine {
    spans: Vec<DocSpan>,
    row_bg: Option<Color>,
}

impl DocLine {
    fn blank() -> Self {
        Self { spans: Vec::new(), row_bg: None }
    }

    fn text(text: impl Into<String>, style: Style) -> Self {
        Self { spans: vec![DocSpan { text: text.into(), style, hit: None }], row_bg: None }
    }

    fn from_spans(spans: Vec<DocSpan>) -> Self {
        Self { spans, row_bg: None }
    }

    fn with_bg(mut self, bg: Color) -> Self {
        self.row_bg = Some(bg);
        self
    }
}

struct WrappedRow {
    spans: Vec<Span<'static>>,
    row_bg: Option<Color>,
    hits: Vec<(u16, u16, HitTarget)>,
}

fn wrap_doc_lines(docs: Vec<DocLine>, width: u16) -> Vec<WrappedRow> {
    let max_w = width.max(4) as usize;
    let mut rows = Vec::new();
    for doc in docs {
        if doc.spans.is_empty() {
            rows.push(WrappedRow { spans: Vec::new(), row_bg: doc.row_bg, hits: Vec::new() });
            continue;
        }
        let mut cur_spans: Vec<Span<'static>> = Vec::new();
        let mut cur_hits: Vec<(u16, u16, HitTarget)> = Vec::new();
        let mut cur_w = 0usize;

        for span in doc.spans {
            let mut seg = String::new();
            let mut seg_w = 0usize;
            let mut seg_start_x = cur_w;
            for ch in span.text.chars() {
                let cw = ch.width().unwrap_or(0);
                if cur_w > 0 && cur_w + cw > max_w {
                    if !seg.is_empty() {
                        if let Some(t) = &span.hit {
                            cur_hits.push((seg_start_x as u16, seg_w as u16, t.clone()));
                        }
                        cur_spans.push(Span::styled(std::mem::take(&mut seg), span.style));
                    }
                    rows.push(WrappedRow {
                        spans: std::mem::take(&mut cur_spans),
                        row_bg: doc.row_bg,
                        hits: std::mem::take(&mut cur_hits),
                    });
                    cur_w = 0;
                    seg_w = 0;
                    seg_start_x = 0;
                }
                seg.push(ch);
                seg_w += cw;
                cur_w += cw;
            }
            if !seg.is_empty() {
                if let Some(t) = span.hit {
                    cur_hits.push((seg_start_x as u16, seg_w as u16, t));
                }
                cur_spans.push(Span::styled(seg, span.style));
            }
        }
        rows.push(WrappedRow { spans: cur_spans, row_bg: doc.row_bg, hits: cur_hits });
    }
    rows
}

fn render_inspector_body(frame: &mut Frame, area: Rect, state: &mut AppState, pal: &Palette, hits: &mut HitMap) {
    hits.push(area, HitTarget::InspectorPane);
    if area.height == 0 || area.width < 8 {
        return;
    }

    if state.selected_task().is_none() && state.inspector_tab == InspectorTab::Summary {
        state.inspector_max_scroll = 0;
        state.inspector_scroll = 0;
        render_welcome(frame, area, state, pal);
        return;
    }

    let docs = match state.inspector_tab {
        InspectorTab::Summary => build_summary_docs(state, pal),
        InspectorTab::Evidence => build_evidence_docs(state, pal),
        InspectorTab::Tree => build_tree_docs(state, pal),
        InspectorTab::Events => build_events_docs(state, pal),
        InspectorTab::Board => build_board_docs(state, pal),
    };

    let content_x = area.x + 2;
    let content_w = area.width.saturating_sub(4).max(4);
    let wrapped = wrap_doc_lines(docs, content_w);
    let total = wrapped.len();
    let view_h = area.height as usize;
    let max_scroll = total.saturating_sub(view_h).min(u16::MAX as usize) as u16;
    state.inspector_max_scroll = max_scroll;
    state.inspector_scroll = state.inspector_scroll.min(max_scroll);

    let scroll = state.inspector_scroll as usize;
    for (idx, row) in wrapped.into_iter().skip(scroll).take(view_h).enumerate() {
        let y = area.y + idx as u16;
        let row_rect = Rect::new(content_x, y, content_w, 1);
        if let Some(bg) = row.row_bg {
            frame.render_widget(Block::default().style(Style::default().bg(bg)), row_rect);
        }
        frame.render_widget(Paragraph::new(Line::from(row.spans)), row_rect);
        for (hx, hw, target) in row.hits {
            hits.push(Rect::new(content_x + hx, y, hw, 1), target);
        }
    }

    if total > view_h {
        let mut sb = ScrollbarState::new(max_scroll as usize + 1).position(scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight).style(pal.dim()),
            area,
            &mut sb,
        );
    }
}

fn render_welcome(frame: &mut Frame, area: Rect, state: &AppState, pal: &Palette) {
    let x = area.x + 3;
    let w = area.width.saturating_sub(6);
    if w < 16 || area.height < 6 {
        return;
    }
    let model_lbl = state
        .model
        .as_ref()
        .map(|m| {
            let id = m.model.as_deref().unwrap_or("auto");
            let eff = m.effort.as_deref().unwrap_or("default");
            format!("{}/{} ({eff})", m.provider, id)
        })
        .unwrap_or_else(|| "connecting…".into());
    let pol = if state.review_mode { "review (per-operation)" } else { "unattended" };
    let mut header_spans = rainbow_text_spans("◆ TeamAgents", pal.canvas_bg, true, pal);
    header_spans.push(Span::styled(
        format!("  ·  {}", sanitize_inline(&state.workspace_name)),
        pal.primary().add_modifier(Modifier::BOLD),
    ));
    let mut lines = vec![
        Line::from(""),
        Line::from(header_spans),
        Line::from(Span::styled("  Coordinated execution trees for coding and scientific research.", pal.muted())),
        Line::from(""),
        Line::from(Span::styled("▎ Session", Style::default().fg(pal.rainbow[4]).add_modifier(Modifier::BOLD))),
        Line::from(vec![
            Span::styled("  Target      ", pal.muted()),
            Span::styled(format!("@{}", sanitize_inline(&state.agent)), pal.ok()),
            Span::styled("   Model   ", pal.muted()),
            Span::styled(model_lbl, pal.secondary()),
        ]),
        Line::from(vec![
            Span::styled("  Policy      ", pal.muted()),
            Span::styled(pol, pal.secondary()),
            Span::styled("   Sandbox   ", pal.muted()),
            Span::styled(state.sandbox_mode.as_str(), pal.secondary()),
        ]),
        Line::from(""),
        Line::from(Span::styled("▎ Workflow", Style::default().fg(pal.rainbow[6]).add_modifier(Modifier::BOLD))),
        Line::from(vec![
            Span::styled("  • Prompt below and press ", pal.secondary()),
            Span::styled(" Enter ", pal.keycap()),
            Span::styled(" to launch a fresh execution tree", pal.secondary()),
        ]),
        Line::from(vec![Span::styled(
            "  • 1-leaf rounds settle directly; ≥2 leaves converge at ◆ summarizer; follow-ups extend that node",
            pal.secondary(),
        )]),
        Line::from(vec![
            Span::styled("  • Press ", pal.secondary()),
            Span::styled(" Ctrl+N ", pal.keycap()),
            Span::styled(" or ", pal.muted()),
            Span::styled("/new", Style::default().fg(pal.rainbow[4])),
            Span::styled(" for a blank session; configure ", pal.secondary()),
            Span::styled("/model", Style::default().fg(pal.rainbow[4])),
            Span::styled(", ", pal.muted()),
            Span::styled("/to", Style::default().fg(pal.rainbow[3])),
            Span::styled(", ", pal.muted()),
            Span::styled("/check", Style::default().fg(pal.rainbow[2])),
            Span::styled(", ", pal.muted()),
            Span::styled("/review", Style::default().fg(pal.rainbow[1])),
            Span::styled(", ", pal.muted()),
            Span::styled("/staged", Style::default().fg(pal.rainbow[5])),
            Span::styled(", ", pal.muted()),
            Span::styled("/wb", Style::default().fg(pal.rainbow[6])),
        ]),
        Line::from(vec![
            Span::styled("  • Navigate with ", pal.secondary()),
            Span::styled(" Tab ", pal.keycap()),
            Span::styled(" focus  ", pal.muted()),
            Span::styled(" Ctrl+B ", pal.keycap()),
            Span::styled(" sidebar  ", pal.muted()),
            Span::styled(" Del×2 ", pal.keycap()),
            Span::styled(" delete  ", pal.muted()),
            Span::styled(" F1 ", pal.keycap()),
            Span::styled(" help", pal.muted()),
        ]),
    ];
    lines.truncate(area.height as usize);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), Rect::new(x, area.y, w, area.height));
}

fn ds(text: impl Into<String>, style: Style) -> DocSpan {
    DocSpan { text: text.into(), style, hit: None }
}

fn ds_hit(text: impl Into<String>, style: Style, target: HitTarget) -> DocSpan {
    DocSpan { text: text.into(), style, hit: Some(target) }
}

fn build_summary_docs(state: &AppState, pal: &Palette) -> Vec<DocLine> {
    let Some(task) = state.selected_task() else {
        return Vec::new();
    };
    let mut docs = Vec::new();
    let (badge, bstyle) = pal.status_badge(&task.status, state.tick);

    // Compact metadata pill strip.
    let mut meta = vec![
        ds(format!(" {badge} {} ", task.status.as_str()), bstyle.add_modifier(Modifier::BOLD)),
        ds("  kind: ", pal.muted()),
        ds(task.kind.as_str().to_owned(), pal.secondary()),
    ];
    if let Some(stage) = &task.stage_status {
        if task.is_root() {
            meta.push(ds("  ·  stage: ", pal.muted()));
            meta.push(ds(stage.as_str().to_owned(), pal.secondary()));
        }
    }
    if let Some(u) = task.turns_used {
        if let Some(r) = task.round.filter(|&r| r > 1) {
            meta.push(ds("  ·  round: ", pal.muted()));
            meta.push(ds(r.to_string(), pal.secondary()));
        }
        meta.push(ds("  ·  turns: ", pal.muted()));
        let turns_lbl = match task.turn_budget {
            Some(b) => format!("{u}/{b}"),
            None => u.to_string(),
        };
        meta.push(ds(turns_lbl, pal.secondary()));
    }
    if let Some(pol) = task.policy {
        meta.push(ds("  ·  policy: ", pal.muted()));
        meta.push(ds(pol.as_str().to_owned(), if pol == Policy::Review { pal.active() } else { pal.muted() }));
    }
    if let Some(sb) = &task.sandbox {
        meta.push(ds("  ·  sandbox: ", pal.muted()));
        meta.push(ds(sanitize_inline(sb), pal.secondary()));
    }
    docs.push(DocLine::blank());
    docs.push(DocLine::from_spans(meta));
    let mut telemetry = Vec::new();
    if let Some(u) = &task.usage {
        if u.input_tokens > 0 || u.output_tokens > 0 {
            telemetry.push(ds("usage         ", pal.muted()));
            telemetry.push(ds(format!("{} tok · {}% cached", u.input_tokens, u.cache_hit_rate_pct), pal.secondary()));
        }
    }
    if task.pending_steerings > 0 {
        if !telemetry.is_empty() {
            telemetry.push(ds("  ·  ", pal.muted()));
        } else {
            telemetry.push(ds("steering      ", pal.muted()));
        }
        telemetry.push(ds(format!("{} steering queued", task.pending_steerings), pal.accent()));
    }
    if !telemetry.is_empty() {
        docs.push(DocLine::from_spans(telemetry));
    }
    if task.live.is_some() || !task.live_children.is_empty() {
        docs.push(DocLine::blank());
        docs.push(DocLine::text("Live Activity", pal.section_title()));
        let mut live_entries: Vec<(String, &crate::protocol::LiveActivity)> = Vec::new();
        if let Some(l) = &task.live {
            live_entries.push((format!("@{}", sanitize_inline(&task.agent)), l));
        }
        let mut child_ids: Vec<&String> = task.live_children.keys().collect();
        child_ids.sort();
        for cid in child_ids {
            if let Some(l) = task.live_children.get(cid) {
                live_entries.push((sanitize_inline(cid), l));
            }
        }
        for (actor, l) in live_entries {
            let mut spans = vec![
                ds(format!("  ▸ {actor} "), pal.accent_bold()),
                ds(format!("[{}]", sanitize_inline(&l.phase)), pal.secondary()),
            ];
            if !l.active_tools.is_empty() {
                spans.push(ds("  tools: ", pal.muted()));
                spans.push(ds(sanitize_inline(&l.active_tools.join(", ")), pal.primary()));
            }
            docs.push(DocLine::from_spans(spans));
            if !l.commentary.is_empty() {
                docs.push(DocLine::text(
                    format!("    {}", sanitize_inline(&preview(&l.commentary, 140))),
                    pal.primary(),
                ));
            } else if !l.reasoning.is_empty() {
                docs.push(DocLine::text(
                    format!("    … {}", sanitize_inline(&preview(&l.reasoning, 140))),
                    pal.muted(),
                ));
            }
            if !l.shell_tail.is_empty() {
                for tail in
                    sanitize_multiline(&l.shell_tail).lines().rev().take(3).collect::<Vec<_>>().into_iter().rev()
                {
                    docs.push(DocLine::text(format!("    $ {tail}"), pal.dim()));
                }
            }
        }
    }
    if !task.plan.is_empty() {
        docs.push(DocLine::blank());
        docs.push(DocLine::text(format!("Plan ({})", task.plan.len()), pal.section_title()));
        for step in &task.plan {
            let (mark, st) = match step.status.as_str() {
                "completed" => ("✓", pal.ok()),
                "in_progress" => ("▸", pal.accent_bold()),
                _ => (" ", pal.muted()),
            };
            docs.push(DocLine::from_spans(vec![
                ds(format!("  [{mark}] "), st),
                ds(sanitize_inline(&step.step), pal.primary()),
            ]));
        }
    }
    if let Some(target) = state.continuation_target() {
        docs.push(DocLine::from_spans(vec![
            ds("continuation  ", pal.muted()),
            ds(
                format!(
                    "↳ @{} ({}) — type below to extend subtree, or Ctrl+N for new session",
                    sanitize_inline(&target.label),
                    target.kind
                ),
                pal.accent(),
            ),
        ]));
    }

    if let Some(v) = &task.verification {
        let (v_lbl, v_style) = pal.verification_badge(v);
        let mut v_row = vec![ds("verification  ", pal.muted()), ds(v_lbl, v_style.add_modifier(Modifier::BOLD))];
        if let Some(chk) = &task.check {
            v_row.push(ds("   cmd: ", pal.muted()));
            v_row.push(ds(sanitize_inline(chk), pal.secondary()));
        }
        docs.push(DocLine::from_spans(v_row));
    }
    if let Some(note) = &task.verification_note {
        docs.push(DocLine::text(format!("  {}", sanitize_inline(note)), pal.dim()));
    }

    if task.rounds.len() > 1 {
        docs.push(DocLine::blank());
        docs.push(DocLine::text(format!("Conversation Rounds ({})", task.rounds.len()), pal.section_title()));
        for rnd in &task.rounds {
            let status_txt = rnd.outcome.as_ref().map_or("active", |s| s.as_str());
            docs.push(DocLine::from_spans(vec![
                ds(format!("  Round {} ", rnd.round), pal.accent().add_modifier(Modifier::BOLD)),
                ds(format!("[{status_txt}]  "), pal.muted()),
                ds(sanitize_inline(&preview(&rnd.prompt, 120)), pal.primary()),
            ]));
            if let Some(res) = &rnd.result {
                docs.push(DocLine::text(format!("    ↳ {}", sanitize_inline(&preview(res, 160))), pal.secondary()));
            }
        }
    } else if let Some(prompt) = &task.prompt {
        docs.push(DocLine::blank());
        docs.push(DocLine::text("Prompt", pal.section_title()));
        for line in sanitize_multiline(prompt).lines() {
            docs.push(
                DocLine::from_spans(vec![ds("▎ ", pal.accent()), ds(line, pal.primary())]).with_bg(pal.surface_bg),
            );
        }
    }

    if task.is_waiting_input() {
        docs.push(DocLine::blank());
        docs.push(DocLine::text("? Clarification Required", pal.accent_soft()));
        if let Some(q) = &task.question {
            for line in sanitize_multiline(q).lines() {
                docs.push(
                    DocLine::from_spans(vec![ds("▎ ", pal.accent_soft()), ds(line, pal.primary())])
                        .with_bg(pal.surface_bg),
                );
            }
        }
        docs.push(DocLine::from_spans(vec![
            ds_hit(" Reply (r) ", pal.pill(pal.accent_soft), HitTarget::InlineReply(task.id.clone())),
            ds(format!("  or /answer {} TEXT", sanitize_inline(&task.id)), pal.muted()),
        ]));
    }

    if !task.clarifications.is_empty() {
        docs.push(DocLine::blank());
        docs.push(DocLine::text("Clarifications", pal.section_title()));
        for c in &task.clarifications {
            docs.push(DocLine::from_spans(vec![
                ds("  Q  ", pal.muted()),
                ds(sanitize_inline(&c.question), pal.secondary()),
            ]));
            docs.push(DocLine::from_spans(vec![ds("  A  ", pal.ok()), ds(sanitize_inline(&c.answer), pal.primary())]));
        }
    }

    if let (Some(approval_id), Some(action)) = (&task.approval_id, &task.action) {
        docs.push(DocLine::blank());
        docs.push(DocLine::text(format!("! Pending Approval ({})", sanitize_inline(approval_id)), pal.warn()));
        push_action_preview_docs(&mut docs, action, pal);
        docs.push(DocLine::from_spans(vec![
            ds("  ", pal.canvas()),
            ds_hit(" ✓ Approve (a) ", pal.pill(pal.ok), HitTarget::InlineApprove(approval_id.clone())),
            ds("  ", pal.canvas()),
            ds_hit(" ✗ Deny (d) ", pal.pill(pal.error), HitTarget::InlineDeny(approval_id.clone())),
            ds("  or Ctrl+P queue", pal.muted()),
        ]));
    } else if task.is_root() {
        let child_approvals: Vec<_> = state
            .tasks
            .iter()
            .filter(|t| t.root == task.id && t.id != task.id && t.status == TaskStatus::Approval)
            .collect();
        if !child_approvals.is_empty() {
            docs.push(DocLine::blank());
            docs.push(DocLine::text(
                format!("! Child Approvals Waiting ({}) — press Ctrl+P to review", child_approvals.len()),
                pal.warn(),
            ));
            for child in child_approvals {
                let op = child.action.as_ref().map_or("action", |a| a.op());
                let aid = child.approval_id.as_deref().unwrap_or("?");
                let mut row = vec![ds(
                    format!(
                        "  • {} (@{}) {}: {}  ",
                        sanitize_inline(&child.id),
                        sanitize_inline(&child.agent),
                        sanitize_inline(op),
                        sanitize_inline(aid),
                    ),
                    pal.secondary(),
                )];
                if let Some(approval_id) = &child.approval_id {
                    row.push(ds_hit(" ✓ Approve ", pal.pill(pal.ok), HitTarget::InlineApprove(approval_id.clone())));
                    row.push(ds(" ", pal.canvas()));
                    row.push(ds_hit(" ✗ Deny ", pal.pill(pal.error), HitTarget::InlineDeny(approval_id.clone())));
                }
                docs.push(DocLine::from_spans(row));
            }
        }
    }

    if let Some(result) = &task.result {
        docs.push(DocLine::blank());
        if task.status == TaskStatus::Done {
            docs.push(DocLine::from_spans(vec![
                ds("Model Report", pal.ok().add_modifier(Modifier::BOLD)),
                ds("  ·  unverified unless backed by observed receipts below", pal.dim()),
            ]));
            push_markdown_docs(&mut docs, result, pal);
        } else {
            docs.push(DocLine::text("Outcome", pal.error().add_modifier(Modifier::BOLD)));
            for line in sanitize_multiline(result).lines() {
                docs.push(DocLine::text(format!("  {line}"), pal.primary()));
            }
        }
    }

    if let Some(err) = &task.stage_error {
        docs.push(DocLine::blank());
        docs.push(DocLine::text("Stage Error", pal.error().add_modifier(Modifier::BOLD)));
        for line in sanitize_multiline(err).lines() {
            docs.push(DocLine::text(format!("  {line}"), pal.error()));
        }
    }

    if !task.leaf_results.is_empty() {
        docs.push(DocLine::blank());
        docs.push(DocLine::text("Leaf Results", pal.section_title()));
        for leaf in &task.leaf_results {
            let (g, gs) = pal.status_badge(&leaf.status, state.tick);
            let res = leaf.result.as_deref().map(|r| preview(r, 120)).unwrap_or_default();
            docs.push(DocLine::from_spans(vec![
                ds(format!("  {g} "), gs),
                ds(format!("@{} ", sanitize_inline(&leaf.agent)), pal.primary().add_modifier(Modifier::BOLD)),
                ds(format!("[{}] ", leaf.status.as_str()), pal.muted()),
                ds(sanitize_inline(&res), pal.secondary()),
            ]));
        }
    }

    if !task.evidence.is_empty() || !task.subtree_receipts.is_empty() {
        docs.push(DocLine::blank());
        docs.push(DocLine::from_spans(vec![
            ds("Observed Receipts", pal.section_title()),
            ds("  ·  press [2] for full details", pal.dim()),
        ]));
        for e in task.evidence.iter().rev().take(4).rev() {
            let ok_lbl = outcome_label(e.ok);
            let ok_style = if e.ok == Some(true) { pal.ok() } else { pal.error() };
            docs.push(DocLine::from_spans(vec![
                ds("  • ", pal.dim()),
                ds(format!("[{ok_lbl}] "), ok_style),
                ds(format!("{} {}", e.action.op(), sanitize_inline(&preview(&e.action.label(), 80))), pal.secondary()),
            ]));
        }
        for r in task.subtree_receipts.iter().take(6) {
            let ok_lbl = outcome_label(r.ok);
            let ok_style = if r.ok == Some(true) { pal.ok() } else { pal.error() };
            docs.push(DocLine::from_spans(vec![
                ds("  ↳ ", pal.dim()),
                ds(format!("[{ok_lbl}] "), ok_style),
                ds(
                    format!(
                        "@{}: {} {}",
                        sanitize_inline(&r.agent),
                        sanitize_inline(&r.op),
                        sanitize_inline(&preview(&r.label, 70))
                    ),
                    pal.secondary(),
                ),
            ]));
        }
    }
    docs
}

fn build_evidence_docs(state: &AppState, pal: &Palette) -> Vec<DocLine> {
    let Some(task) = state.selected_task() else {
        return vec![DocLine::text("  Select a session or task to inspect observed evidence.", pal.muted())];
    };
    let mut docs = vec![
        DocLine::blank(),
        DocLine::text("Runtime-observed execution receipts (model narrative is separate):", pal.muted()),
    ];
    if task.evidence.is_empty() {
        docs.push(DocLine::blank());
        docs.push(DocLine::text("  No completed operation receipts on this stage.", pal.muted()));
    } else {
        for (idx, entry) in task.evidence.iter().enumerate() {
            docs.push(DocLine::blank());
            let ok_lbl = outcome_label(entry.ok);
            let ok_style = if entry.ok == Some(true) { pal.ok() } else { pal.error() };
            docs.push(DocLine::from_spans(vec![
                ds(format!("#{} ", idx + 1), pal.dim()),
                ds(format!("[{ok_lbl}] "), ok_style.add_modifier(Modifier::BOLD)),
                ds(entry.action.op().to_owned(), pal.accent_bold()),
                ds(format!("  {}", sanitize_inline(&entry.action.label())), pal.primary()),
            ]));
            if !entry.result.is_empty() {
                for line in sanitize_multiline(&entry.result).lines() {
                    docs.push(
                        DocLine::from_spans(vec![ds("│ ", pal.dim()), ds(line, pal.secondary())])
                            .with_bg(pal.surface_bg),
                    );
                }
            }
        }
    }
    if !task.subtree_receipts.is_empty() {
        docs.push(DocLine::blank());
        docs.push(DocLine::text("Subtree Receipts Index", pal.section_title()));
        for r in &task.subtree_receipts {
            let ok_lbl = outcome_label(r.ok);
            let ok_style = if r.ok == Some(true) { pal.ok() } else { pal.error() };
            docs.push(DocLine::from_spans(vec![
                ds(format!("  {} ", preview(&r.task, 8)), pal.dim()),
                ds(format!("@{} ", sanitize_inline(&r.agent)), pal.primary().add_modifier(Modifier::BOLD)),
                ds(format!("{} ", sanitize_inline(&r.op)), pal.secondary()),
                ds(format!("[{ok_lbl}] "), ok_style),
                ds(sanitize_inline(&r.label), pal.secondary()),
            ]));
        }
    }
    docs
}

fn build_tree_docs(state: &AppState, pal: &Palette) -> Vec<DocLine> {
    let Some(task) = state.selected_task() else {
        return vec![DocLine::text("  Select a session to inspect its execution tree.", pal.muted())];
    };
    if task.tree_nodes.is_empty() {
        let root_id = &task.root;
        let rows: Vec<_> = state.tree_rows.iter().filter(|r| &r.root_id == root_id).collect();
        if rows.is_empty() {
            return vec![DocLine::text("  No branch tree data yet.", pal.muted())];
        }
        let mut docs = vec![
            DocLine::blank(),
            DocLine::text(format!("Execution Tree ({})", sanitize_inline(root_id)), pal.section_title()),
        ];
        for row in rows {
            if let Some(t) = state.tasks.iter().find(|t| t.id == row.task_id) {
                let (g, gs) = pal.status_badge(&t.status, state.tick);
                let kind_tag = match t.kind {
                    StageKind::Repair => " (repair)",
                    StageKind::Summarizer => " (summarizer)",
                    _ => "",
                };
                docs.push(DocLine::from_spans(vec![
                    ds(format!("  {}", row.prefix), pal.dim()),
                    ds(format!("{g} "), gs),
                    ds(format!("@{}{kind_tag}", sanitize_inline(&t.agent)), pal.primary().add_modifier(Modifier::BOLD)),
                    ds(format!("  [{}]  {}", t.status.as_str(), sanitize_inline(&t.id)), pal.muted()),
                ]));
            }
        }
        return docs;
    }

    let mut children: HashMap<&str, Vec<_>> = HashMap::new();
    let mut root = None;
    for node in &task.tree_nodes {
        if let Some(p) = node.parent.as_deref() {
            children.entry(p).or_default().push(node);
        } else if root.is_none() {
            root = Some(node);
        }
    }
    let mut docs = vec![
        DocLine::blank(),
        DocLine::text(format!("Work Tree ({} stages)", task.tree_nodes.len()), pal.section_title()),
    ];
    if let Some(root_node) = root {
        let mut stack = vec![(root_node, String::new(), true, true)];
        while let Some((node, prefix, last, first)) = stack.pop() {
            let branch = if first {
                ""
            } else if last {
                "└─ "
            } else {
                "├─ "
            };
            let (g, gs) = pal.status_badge(&node.status, state.tick);
            let kind_tag = match node.kind {
                StageKind::Repair => " (repair)",
                StageKind::Summarizer => " (summarizer)",
                _ => "",
            };
            docs.push(DocLine::from_spans(vec![
                ds(format!("  {prefix}{branch}"), pal.dim()),
                ds(format!("{g} "), gs),
                ds(format!("@{}{kind_tag}", sanitize_inline(&node.agent)), pal.primary().add_modifier(Modifier::BOLD)),
                ds(format!("  [{}]  {}", node.status.as_str(), sanitize_inline(&node.task)), pal.muted()),
            ]));
            if let Some(next) = children.get(node.task.as_str()) {
                let child_prefix = if first {
                    String::new()
                } else if last {
                    format!("{prefix}   ")
                } else {
                    format!("{prefix}│  ")
                };
                for (i, &child) in next.iter().enumerate().rev() {
                    stack.push((child, child_prefix.clone(), i + 1 == next.len(), false));
                }
            }
        }
    }
    docs
}

fn build_events_docs(state: &AppState, pal: &Palette) -> Vec<DocLine> {
    if state.events.is_empty() {
        return vec![DocLine::text("  No session events recorded.", pal.muted())];
    }
    let mut docs = vec![DocLine::blank(), DocLine::text("Session Events (newest first)", pal.section_title())];
    for entry in state.events.iter().rev() {
        let style = match entry.level {
            crate::tui::state::EventLevel::Info => pal.dim(),
            crate::tui::state::EventLevel::Success => pal.ok(),
            crate::tui::state::EventLevel::Attention => pal.accent_soft(),
            crate::tui::state::EventLevel::Error => pal.error(),
        };
        docs.push(DocLine::from_spans(vec![
            ds(format!("  {} ", entry.timestamp), pal.dim()),
            ds("• ", style),
            ds(entry.text.clone(), pal.primary()),
        ]));
    }
    docs
}

fn build_board_docs(state: &AppState, pal: &Palette) -> Vec<DocLine> {
    let Some(task) = state.selected_task() else {
        return vec![
            DocLine::from_spans(vec![ds("No active task selected.", pal.muted())]),
            DocLine::from_spans(vec![ds(
                "Start a task or select one from the sidebar to inspect its collaborative whiteboard.",
                pal.muted(),
            )]),
        ];
    };
    let mut docs = vec![
        DocLine::from_spans(vec![
            ds("Collaborative Whiteboard ", pal.accent_bold()),
            ds(format!("(seq {}, {} entries)", task.whiteboard_seq, task.whiteboard.len()), pal.secondary()),
        ]),
        DocLine::from_spans(vec![ds(
            "Actions: wb_read, wb_write (set/append/resolve/delete), wb_wait · User: /wb ...",
            pal.muted(),
        )]),
        DocLine::from_spans(Vec::new()),
    ];
    if task.whiteboard.is_empty() {
        docs.push(DocLine::from_spans(vec![ds("Whiteboard is empty.", pal.muted())]));
        docs.push(DocLine::from_spans(vec![ds(
            "Use `/wb set KEY VALUE` or let sibling leaves coordinate with `wb_write`.",
            pal.muted(),
        )]));
        return docs;
    }
    for entry in &task.whiteboard {
        let status_style = if entry.status == "resolved" {
            Style::default().fg(pal.rainbow[3])
        } else {
            Style::default().fg(pal.rainbow[5])
        };
        docs.push(DocLine::from_spans(vec![
            ds("■ ", status_style),
            ds(sanitize_inline(&entry.key), pal.primary()),
            ds(
                format!(
                    " [{} · {} · v{} · #{} · r{} · {}]",
                    sanitize_inline(&entry.kind),
                    sanitize_inline(&entry.status),
                    entry.version,
                    entry.seq,
                    entry.round,
                    sanitize_inline(&entry.author_agent),
                ),
                pal.muted(),
            ),
        ]));
        for line in entry.content.lines() {
            docs.push(DocLine::from_spans(vec![ds(format!("    {}", sanitize_inline(line)), pal.secondary())]));
        }
        if entry.content.is_empty() {
            docs.push(DocLine::from_spans(vec![ds("    <empty>", pal.muted())]));
        }
        docs.push(DocLine::from_spans(Vec::new()));
    }
    docs
}

fn push_action_preview_docs(docs: &mut Vec<DocLine>, action: &ActionPreview, pal: &Palette) {
    match action {
        ActionPreview::Shell { command, .. } | ActionPreview::Verify { command } => {
            docs.push(DocLine::from_spans(vec![
                ds("  op: ", pal.muted()),
                ds(action.op().to_owned(), pal.accent_bold()),
            ]));
            for line in sanitize_multiline(command).lines() {
                docs.push(
                    DocLine::from_spans(vec![ds("  $ ", pal.accent_bold()), ds(line, pal.primary())])
                        .with_bg(pal.surface_bg),
                );
            }
        }
        ActionPreview::ReadFile { path, offset, limit } => {
            let mut spans = vec![ds("  read_file: ", pal.accent_bold()), ds(sanitize_inline(path), pal.primary())];
            if offset.is_some() || limit.is_some() {
                spans.push(ds(
                    format!(
                        " [offset={}, limit={}]",
                        offset.map_or_else(|| "0".into(), |v| v.to_string()),
                        limit.map_or_else(|| "default".into(), |v| v.to_string()),
                    ),
                    pal.muted(),
                ));
            }
            docs.push(DocLine::from_spans(spans));
        }
        ActionPreview::EditFile { path, old, new } => {
            docs.push(DocLine::from_spans(vec![
                ds("  edit_file: ", pal.accent_bold()),
                ds(sanitize_inline(path), pal.primary()),
            ]));
            if old.is_empty() {
                docs.push(DocLine::text("  (create new file)", pal.muted()));
            } else {
                for line in sanitize_multiline(old).lines() {
                    docs.push(DocLine::text(format!("  - {line}"), pal.diff_del()).with_bg(pal.diff_del_bg));
                }
            }
            for line in sanitize_multiline(new).lines() {
                docs.push(DocLine::text(format!("  + {line}"), pal.diff_add()).with_bg(pal.diff_add_bg));
            }
        }
        ActionPreview::ApplyPatch { patch } => {
            docs.push(DocLine::from_spans(vec![ds("  apply_patch:", pal.accent_bold())]));
            for (kind, text) in crate::patch::preview_patch(patch).into_iter().take(60) {
                match kind {
                    'H' | '@' => docs.push(DocLine::text(format!("  {text}"), pal.accent_bold())),
                    '+' => docs.push(DocLine::text(format!("  +{text}"), pal.diff_add()).with_bg(pal.diff_add_bg)),
                    '-' => docs.push(DocLine::text(format!("  -{text}"), pal.diff_del()).with_bg(pal.diff_del_bg)),
                    _ => docs.push(DocLine::text(format!("   {text}"), pal.secondary())),
                }
            }
        }
        ActionPreview::ListDir { path } => {
            docs.push(DocLine::from_spans(vec![
                ds("  list_dir: ", pal.accent_bold()),
                ds(sanitize_inline(path), pal.primary()),
            ]));
        }
        ActionPreview::GrepFiles { pattern, path } => {
            let suffix = path.as_deref().map(|p| format!(" in {p}")).unwrap_or_default();
            docs.push(DocLine::from_spans(vec![
                ds("  grep_files: ", pal.accent_bold()),
                ds(sanitize_inline(&format!("{pattern}{suffix}")), pal.primary()),
            ]));
        }
        ActionPreview::ReadSkill { name } => {
            docs.push(DocLine::from_spans(vec![
                ds("  read_skill: ", pal.accent_bold()),
                ds(sanitize_inline(name), pal.primary()),
            ]));
        }
        ActionPreview::McpList { server } => {
            docs.push(DocLine::from_spans(vec![
                ds("  mcp_list: ", pal.accent_bold()),
                ds(sanitize_inline(server.as_deref().unwrap_or("all")), pal.primary()),
            ]));
        }
        ActionPreview::McpCall { server, tool } => {
            docs.push(DocLine::from_spans(vec![
                ds("  mcp_call: ", pal.accent_bold()),
                ds(sanitize_inline(&format!("{server}:{tool}")), pal.primary()),
            ]));
        }
        ActionPreview::Other { op, summary } => {
            docs.push(DocLine::from_spans(vec![
                ds(format!("  {}: ", sanitize_inline(op)), pal.accent_bold()),
                ds(sanitize_inline(&preview(summary, 160)), pal.secondary()),
            ]));
        }
    }
}

fn push_markdown_docs(docs: &mut Vec<DocLine>, raw: &str, pal: &Palette) {
    let clean = sanitize_multiline(raw);
    let mut in_code = false;
    for line in clean.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            in_code = !in_code;
            let lang = rest.trim();
            if in_code && !lang.is_empty() {
                docs.push(DocLine::text(format!("  ┌ {lang}"), pal.dim()));
            }
            continue;
        }
        if in_code {
            docs.push(
                DocLine::from_spans(vec![ds("  │ ", pal.dim()), ds(line, pal.secondary())]).with_bg(pal.surface_bg),
            );
            continue;
        }
        if let Some(h) =
            trimmed.strip_prefix("### ").or_else(|| trimmed.strip_prefix("## ")).or_else(|| trimmed.strip_prefix("# "))
        {
            docs.push(DocLine::text(format!("  {h}"), pal.accent_bold()));
            continue;
        }
        if let Some(q) = trimmed.strip_prefix("> ") {
            let mut spans = vec![ds("  ▎ ", pal.accent_soft())];
            push_inline_markdown(&mut spans, q, pal.secondary(), pal);
            docs.push(DocLine::from_spans(spans));
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            let indent = " ".repeat(line.len() - trimmed.len() + 2);
            let mut spans = vec![ds(indent, pal.canvas()), ds("• ", pal.accent())];
            push_inline_markdown(&mut spans, item, pal.primary(), pal);
            docs.push(DocLine::from_spans(spans));
            continue;
        }
        let mut spans = vec![ds("  ", pal.canvas())];
        push_inline_markdown(&mut spans, line, pal.primary(), pal);
        docs.push(DocLine::from_spans(spans));
    }
}

fn push_inline_markdown(spans: &mut Vec<DocSpan>, text: &str, base: Style, pal: &Palette) {
    let mut rest = text;
    while !rest.is_empty() {
        let code = rest.find('`').map(|pos| (pos, "`", 1usize, pal.accent().bg(pal.surface_bg), true));
        let bold = rest.find("**").map(|pos| (pos, "**", 2usize, base.add_modifier(Modifier::BOLD), false));
        let pick = match (code, bold) {
            (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let Some((pos, delim, dlen, style, pad)) = pick else {
            spans.push(ds(rest, base));
            break;
        };
        if pos > 0 {
            spans.push(ds(&rest[..pos], base));
        }
        let after = &rest[pos + dlen..];
        if let Some(end) = after.find(delim) {
            let body = if pad { format!(" {} ", &after[..end]) } else { after[..end].to_owned() };
            spans.push(ds(body, style));
            rest = &after[end + dlen..];
        } else {
            spans.push(ds(rest, base));
            break;
        }
    }
}

fn render_alert_strip(frame: &mut Frame, area: Rect, state: &AppState, pal: &Palette, hits: &mut HitMap) {
    let approvals = state.pending_approvals();
    let questions = state.pending_questions();
    frame.render_widget(Block::default().style(Style::default().bg(pal.elevated_bg)), area);
    let mut writer = HitSpanWriter::new(area, hits);
    writer.push(" ", Style::default().bg(pal.elevated_bg), None);
    if !approvals.is_empty() {
        writer.push(
            format!(" ! {} approval(s) waiting — Ctrl+P or click ", approvals.len()),
            pal.pill(pal.warn),
            Some(HitTarget::AlertApprovals),
        );
    }
    if !approvals.is_empty() && !questions.is_empty() {
        writer.push("  ", Style::default().bg(pal.elevated_bg), None);
    }
    if !questions.is_empty() {
        let q_root = questions[0];
        let q = q_root.question.as_deref().unwrap_or("");
        writer.push(
            format!(" ? {} clarification — {} ", questions.len(), truncate_display(q, 38)),
            pal.pill(pal.accent_soft),
            Some(HitTarget::AlertClarification(q_root.id.clone())),
        );
    }
    frame.render_widget(Paragraph::new(writer.finish()), area);
}

fn render_composer_dock(frame: &mut Frame, area: Rect, state: &AppState, pal: &Palette, hits: &mut HitMap) {
    if area.height < 2 || area.width < 10 {
        return;
    }
    let focused = state.focus == Focus::Composer;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);

    // Top hairline cleanly separates workspace from the surface-tinted composer dock.
    let line_style = pal.border(focused).bg(pal.canvas_bg);
    let cont_target = state.continuation_target();
    let can_steer = cont_target.is_none()
        && state.selected_root_task().is_some_and(|r| !r.status.is_terminal() && r.kind != StageKind::Legacy);
    if let Some(root_id) = &state.answering_root {
        let mut w = HitSpanWriter::new(rows[0], hits);
        let lbl = format!("─── Answering root {} (Esc cancel) ", preview(root_id, 12));
        w.push(lbl, pal.accent_soft().bg(pal.canvas_bg), None);
        let rem = (rows[0].x + rows[0].width).saturating_sub(w.x) as usize;
        if rem > 0 {
            w.push("─".repeat(rem), line_style, None);
        }
        frame.render_widget(Paragraph::new(w.finish()), rows[0]);
    } else if state.selected_task().is_some() {
        let mut w = HitSpanWriter::new(rows[0], hits);
        let new_lbl = " + New (Ctrl+N) ";
        let left_lbl = if let Some(target) = &cont_target {
            format!("─── ↳ Continue @{} ({}) ", sanitize_inline(&target.label), target.kind)
        } else if can_steer {
            "─── ⇢ Steer active session ".to_owned()
        } else if state.selected_root_task().is_some_and(|r| !r.status.is_terminal()) {
            "─── Session active ".to_owned()
        } else {
            "─── Session settled ".to_owned()
        };
        let avail_left = (rows[0].width as usize).saturating_sub(new_lbl.width() + 2);
        let left_fit = truncate_display(&left_lbl, avail_left);
        let left_style = if cont_target.is_some() || can_steer { pal.accent().bg(pal.canvas_bg) } else { line_style };
        w.push(left_fit, left_style, None);
        let end_x = rows[0].x + rows[0].width;
        let pill_x = end_x.saturating_sub(new_lbl.width() as u16 + 1);
        if pill_x > w.x {
            w.push("─".repeat((pill_x - w.x) as usize), line_style, None);
        }
        if end_x >= w.x + new_lbl.width() as u16 {
            w.push(new_lbl, pal.pill_inactive(), Some(HitTarget::NewSession));
            if end_x > w.x {
                w.push("─".repeat((end_x - w.x) as usize), line_style, None);
            }
        }
        frame.render_widget(Paragraph::new(w.finish()), rows[0]);
    } else {
        paint_divider(frame, rows[0], focused, pal.canvas_bg, pal);
    }

    let dock = rows[1];
    frame.render_widget(Block::default().style(pal.surface()), dock);
    let inner_x = dock.x + 3;
    let inner_w = dock.width.saturating_sub(5).max(2);
    let text_area = Rect::new(inner_x, dock.y, inner_w, dock.height);

    let (wrapped, cur_col, cur_row) = state.input.visual_layout(inner_w as usize);
    let scroll = cur_row.saturating_sub(dock.height.saturating_sub(1));
    hits.push(dock, HitTarget::ComposerPane { inner: text_area, scroll });

    let gutter_glyph = if state.answering_root.is_some() {
        (" ? ", pal.accent_soft().bg(pal.surface_bg))
    } else if cont_target.is_some() {
        (" ↳ ", if focused { pal.accent().bg(pal.surface_bg) } else { pal.muted().bg(pal.surface_bg) })
    } else if can_steer {
        (" ⇢ ", if focused { pal.accent().bg(pal.surface_bg) } else { pal.muted().bg(pal.surface_bg) })
    } else if focused {
        (" › ", pal.accent_bold().bg(pal.surface_bg))
    } else {
        (" › ", pal.muted().bg(pal.surface_bg))
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(gutter_glyph.0, gutter_glyph.1))),
        Rect::new(dock.x, dock.y, 3.min(dock.width), 1),
    );

    if state.input.is_empty() {
        let placeholder_buf;
        let placeholder = if state.answering_root.is_some() {
            "Type clarification reply and press Enter…"
        } else if let Some(target) = &cont_target {
            placeholder_buf = format!(
                "Follow up with @{} (extends subtree), or Ctrl+N for new session…",
                sanitize_inline(&target.label)
            );
            placeholder_buf.as_str()
        } else if can_steer {
            "Type steering guidance (applied on next turn), or Ctrl+N for new session…"
        } else if state.selected_task().is_some() {
            "This session view cannot continue directly — press Ctrl+N or click + New for a fresh session…"
        } else {
            "New session — describe a coding or research task, or type / for commands (@ for files, Shift+Enter newline)…"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate_display(placeholder, inner_w as usize),
                pal.muted().bg(pal.surface_bg),
            ))),
            text_area,
        );
    } else {
        let lines: Vec<_> = wrapped
            .iter()
            .skip(scroll as usize)
            .take(dock.height as usize)
            .map(|l| Line::from(Span::styled(l.as_str(), pal.primary().bg(pal.surface_bg))))
            .collect();
        frame.render_widget(Paragraph::new(lines), text_area);
    }

    if focused && state.overlay == Overlay::None {
        let vx = text_area.x + cur_col.min(inner_w.saturating_sub(1));
        let vy = text_area.y + cur_row.saturating_sub(scroll).min(text_area.height.saturating_sub(1));
        frame.set_cursor_position(Position::new(vx, vy));
    }
}

fn render_status_bar(frame: &mut Frame, area: Rect, state: &AppState, pal: &Palette, hits: &mut HitMap) {
    frame.render_widget(Block::default().style(pal.sidebar()), area);
    if let Some(toast) = &state.toast {
        let style =
            if toast.is_error { pal.danger_row() } else { pal.ok().bg(pal.sidebar_bg).add_modifier(Modifier::BOLD) };
        let prefix = if toast.is_error { " ✗ " } else { " ✓ " };
        let msg = truncate_display(&format!("{prefix}{}", toast.text), area.width as usize);
        frame.render_widget(Paragraph::new(Line::from(Span::styled(msg, style))), area);
        return;
    }

    let help_lbl = " F1 Help ";
    let hint_right = if area.width >= 110 {
        " Ctrl+N new  Ctrl+B rail  Ctrl+P appr  Del×2 del "
    } else if area.width >= 76 {
        " Ctrl+N new  Ctrl+P appr  Del×2 del "
    } else {
        ""
    };
    let right_w = hint_right.width() + help_lbl.width() + 2;

    let mut writer = HitSpanWriter::new(area, hits);
    writer.push(" ", pal.sidebar(), None);
    let conn = if state.connected { ("● online", pal.ok()) } else { ("○ offline", pal.error()) };
    writer.push(conn.0, conn.1.bg(pal.sidebar_bg), None);
    writer.push(format!(" · @{}", sanitize_inline(&state.agent)), pal.accent().bg(pal.sidebar_bg), None);
    if let Some(cfg) = &state.model {
        let m = cfg.model.as_deref().unwrap_or("auto");
        let eff = cfg.effort.as_deref().unwrap_or("default");
        writer.push(
            format!(" · {}/{} ({eff})", cfg.provider, truncate_display(m, 18)),
            Style::default().fg(pal.rainbow_color(5)).bg(pal.sidebar_bg),
            None,
        );
    }
    let pol_lbl = if state.review_mode { " · review" } else { " · auto" };
    let pol_style = if state.review_mode {
        Style::default().fg(pal.rainbow_color(1)).bg(pal.sidebar_bg).add_modifier(Modifier::BOLD)
    } else {
        pal.muted().bg(pal.sidebar_bg)
    };
    writer.push(pol_lbl, pol_style, None);
    match state.sandbox_mode {
        SandboxMode::Direct => {}
        SandboxMode::Staged => writer.push(" · staged", Style::default().fg(pal.rainbow[2]).bg(pal.sidebar_bg), None),
        SandboxMode::ReadOnly => {
            writer.push(" · read-only", Style::default().fg(pal.rainbow[3]).bg(pal.sidebar_bg), None);
        }
    }
    if let Some(chk) = &state.armed_check {
        writer.push(format!(" · check: {}", truncate_display(chk, 18)), pal.accent_soft().bg(pal.sidebar_bg), None);
    }

    let target_x = area.x + area.width.saturating_sub(right_w as u16);
    if target_x > writer.x {
        writer.push(" ".repeat((target_x - writer.x) as usize), pal.sidebar(), None);
        if !hint_right.is_empty() {
            writer.push(hint_right, pal.dim().bg(pal.sidebar_bg), None);
        }
        writer.push(help_lbl, pal.keycap(), Some(HitTarget::OpenHelp));
    }
    frame.render_widget(Paragraph::new(writer.finish()), area);
}

fn render_slash_popup(
    frame: &mut Frame,
    composer: Rect,
    bounds: Rect,
    state: &AppState,
    pal: &Palette,
    hits: &mut HitMap,
) {
    let matches = matching_commands(state.input.as_str());
    if matches.is_empty() {
        return;
    }
    let max_items = matches.len().min(9);
    let h = (max_items + 2) as u16;
    let w = bounds.width.clamp(28, 82).min(bounds.width);
    let y = composer.y.saturating_sub(h);
    let rect = Rect::new(composer.x, y, w, h.min(bounds.height));
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(pal.elevated())
        .border_style(pal.border(true))
        .title(Span::styled(" Slash Commands — Tab or Click ", pal.accent_bold()));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let sel = state.slash_idx % matches.len();
    let start = sel.saturating_sub(max_items.saturating_sub(1));
    for (row_idx, (orig_idx, spec)) in matches.iter().enumerate().skip(start).take(max_items).enumerate() {
        let ry = inner.y + row_idx as u16;
        if ry >= inner.y + inner.height {
            break;
        }
        let selected = orig_idx == sel;
        let bg = if selected { pal.selected_bg } else { pal.elevated_bg };
        let row_rect = Rect::new(inner.x, ry, inner.width, 1);
        let usage = format!(" {:<36} ", truncate_display(spec.usage, 36));
        let sum_w = inner.width as usize;
        let desc = truncate_display(spec.summary, sum_w.saturating_sub(usage.width()));
        let pad = sum_w.saturating_sub(usage.width() + desc.width());
        let line = Line::from(vec![
            Span::styled(usage, if selected { pal.selected_row() } else { pal.accent().bg(bg) }),
            Span::styled(desc, if selected { pal.selected_row() } else { pal.secondary().bg(bg) }),
            Span::styled(" ".repeat(pad), Style::default().bg(bg)),
        ]);
        frame.render_widget(Paragraph::new(line), row_rect);
        hits.push(row_rect, HitTarget::SlashItem(orig_idx));
    }
}

fn render_mention_popup(
    frame: &mut Frame,
    composer: Rect,
    bounds: Rect,
    state: &AppState,
    pal: &Palette,
    hits: &mut HitMap,
) {
    let Some((_, _, _, matches)) = state.active_mention() else {
        return;
    };
    let max_items = matches.len().min(9);
    let h = (max_items + 2) as u16;
    let w = bounds.width.clamp(28, 72).min(bounds.width);
    let y = composer.y.saturating_sub(h);
    let rect = Rect::new(composer.x, y, w, h.min(bounds.height));
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(pal.elevated())
        .border_style(pal.border(true))
        .title(Span::styled(" Workspace Files (@) — Tab/Enter or Click ", pal.accent_bold()));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let sel = state.mention_idx % matches.len();
    for (row_idx, path) in matches.iter().take(max_items).enumerate() {
        let ry = inner.y + row_idx as u16;
        if ry >= inner.y + inner.height {
            break;
        }
        let selected = row_idx == sel;
        let bg = if selected { pal.selected_bg } else { pal.elevated_bg };
        let row_rect = Rect::new(inner.x, ry, inner.width, 1);
        let text = format!(" @{}", truncate_display(path, inner.width.saturating_sub(3) as usize));
        let pad = (inner.width as usize).saturating_sub(text.width());
        let line = Line::from(vec![
            Span::styled(text, if selected { pal.selected_row() } else { pal.primary().bg(bg) }),
            Span::styled(" ".repeat(pad), Style::default().bg(bg)),
        ]);
        frame.render_widget(Paragraph::new(line), row_rect);
        hits.push(row_rect, HitTarget::MentionItem(row_idx));
    }
}

fn render_modal(frame: &mut Frame, area: Rect, state: &AppState, pal: &Palette, hits: &mut HitMap) {
    hits.push(area, HitTarget::ModalBackdrop);
    match &state.overlay {
        Overlay::None => {}
        Overlay::Approvals { selected } => {
            let pending = state.pending_approvals();
            let rect = centered_rect(area, 86, 22);
            hits.push(rect, HitTarget::ModalSurface);
            frame.render_widget(Clear, rect);
            let close_x = rect.x + rect.width.saturating_sub(11);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(pal.elevated())
                .border_style(Style::default().fg(pal.warn))
                .title(Span::styled(
                    format!(" Pending Approvals ({}) — [↑↓] Pick  [Esc] Close ", pending.len()),
                    pal.warn(),
                ));
            let inner = block.inner(rect);
            frame.render_widget(block, rect);
            hits.push(Rect::new(close_x, rect.y, 10.min(rect.width), 1), HitTarget::ModalClose);
            if inner.height < 3 || inner.width < 12 {
                return;
            }
            let body = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(inner);
            if pending.is_empty() {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(" No pending approvals.", pal.muted()))),
                    body[0],
                );
                return;
            }
            let idx = (*selected).min(pending.len() - 1);
            let mut y = body[0].y;
            for (i, t) in pending.iter().enumerate() {
                if y >= body[0].y + body[0].height {
                    break;
                }
                let mark = if i == idx { "▸ " } else { "  " };
                let style = if i == idx { pal.selected_row() } else { pal.primary().bg(pal.elevated_bg) };
                let row_rect = Rect::new(body[0].x, y, body[0].width, 1);
                let label = truncate_display(
                    &format!(
                        "{mark}@{} · {}  ({})",
                        sanitize_inline(&t.agent),
                        sanitize_inline(&t.id),
                        t.approval_id.as_deref().unwrap_or("?")
                    ),
                    body[0].width as usize,
                );
                frame.render_widget(Paragraph::new(Line::from(Span::styled(label, style))), row_rect);
                hits.push(row_rect, HitTarget::ModalApprovalRow(i));
                y += 1;
            }
            if y < body[0].y + body[0].height {
                y += 1;
            }
            if let Some(action) = &pending[idx].action {
                let mut prev_docs = Vec::new();
                push_action_preview_docs(&mut prev_docs, action, pal);
                let wrapped = wrap_doc_lines(prev_docs, body[0].width);
                for wr in wrapped {
                    if y >= body[0].y + body[0].height {
                        break;
                    }
                    let r = Rect::new(body[0].x, y, body[0].width, 1);
                    if let Some(bg) = wr.row_bg {
                        frame.render_widget(Block::default().style(Style::default().bg(bg)), r);
                    }
                    frame.render_widget(Paragraph::new(Line::from(wr.spans)), r);
                    y += 1;
                }
            }
            if let Some(aid) = &pending[idx].approval_id {
                let mut w = HitSpanWriter::new(body[1], hits);
                w.push(" ", pal.elevated(), None);
                w.push(" ✓ Approve (a) ", pal.pill(pal.ok), Some(HitTarget::ModalApprove(aid.clone())));
                w.push("  ", pal.elevated(), None);
                w.push(" ✗ Deny (d) ", pal.pill(pal.error), Some(HitTarget::ModalDeny(aid.clone())));
                frame.render_widget(Paragraph::new(w.finish()), body[1]);
            }
        }
        Overlay::Model {
            providers,
            provider_idx,
            initial_provider: _,
            model_input,
            effort_idx,
            catalog,
            selected_catalog,
            field,
        } => {
            let rect = centered_rect(area, 82, 24);
            hits.push(rect, HitTarget::ModalSurface);
            frame.render_widget(Clear, rect);
            let active_p = providers.get(*provider_idx);
            let provider = active_p
                .map(|p| p.name.as_str())
                .unwrap_or_else(|| state.model.as_ref().map_or("unknown", |m| m.provider.as_str()));
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(pal.elevated())
                .border_style(pal.border(true))
                .title(Span::styled(
                    format!(" Provider & Model ({provider}) — [Tab] Field  [Enter] Save  [Esc] Close "),
                    pal.accent_bold(),
                ));
            let inner = block.inner(rect);
            frame.render_widget(block, rect);
            if inner.height < 9 || inner.width < 20 {
                return;
            }
            let prov_rect = Rect::new(inner.x, inner.y, inner.width, 1);
            let mut pw = HitSpanWriter::new(prov_rect, hits);
            pw.push(
                " Provider: ",
                if *field == ModelField::ProviderBar { pal.accent_bold() } else { pal.muted() },
                Some(HitTarget::ModalModelField(ModelField::ProviderBar)),
            );
            for (i, p) in providers.iter().enumerate() {
                if i > 0 {
                    pw.push(" ", pal.elevated(), None);
                }
                let style = if i == *provider_idx { pal.pill(pal.accent) } else { pal.pill_inactive() };
                let lbl = if p.ready {
                    format!(" {} ", sanitize_inline(&p.name))
                } else {
                    format!(" {}! ", sanitize_inline(&p.name))
                };
                pw.push(lbl, style, Some(HitTarget::ModalProviderOption(i)));
            }
            frame.render_widget(Paragraph::new(pw.finish()), prov_rect);

            if let Some(p) = active_p {
                let info_line = if let Some(env_name) = p.api_key_env.as_deref() {
                    format!("           {} · key env: {} · {}", p.wire, env_name, p.status)
                } else {
                    format!("           {} · {}", p.wire, p.status)
                };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        truncate_display(&sanitize_inline(&info_line), inner.width as usize),
                        if p.ready { pal.dim() } else { pal.error() },
                    ))),
                    Rect::new(inner.x, inner.y + 1, inner.width, 1),
                );
            }

            let input_rect = Rect::new(inner.x, inner.y + 2, inner.width, 1);
            hits.push(input_rect, HitTarget::ModalModelField(ModelField::ModelInput));
            let input_line = Line::from(vec![
                Span::styled(
                    " Model ID: ",
                    if *field == ModelField::ModelInput { pal.accent_bold() } else { pal.muted() },
                ),
                Span::styled(
                    if model_input.is_empty() {
                        "(empty = provider default)".into()
                    } else {
                        sanitize_inline(model_input.as_str())
                    },
                    if model_input.is_empty() { pal.dim() } else { pal.primary() },
                ),
            ]);
            frame.render_widget(Paragraph::new(input_line), input_rect);

            let eff_rect = Rect::new(inner.x, inner.y + 4, inner.width, 1);
            let mut ew = HitSpanWriter::new(eff_rect, hits);
            ew.push(
                " Effort:   ",
                if *field == ModelField::EffortBar { pal.accent_bold() } else { pal.muted() },
                Some(HitTarget::ModalModelField(ModelField::EffortBar)),
            );
            for (i, opt) in EFFORT_OPTIONS.iter().enumerate() {
                if i > 0 {
                    ew.push(" ", pal.elevated(), None);
                }
                let style = if i == *effort_idx { pal.pill(pal.accent) } else { pal.pill_inactive() };
                ew.push(format!(" {opt} "), style, Some(HitTarget::ModalEffortOption(i)));
            }
            frame.render_widget(Paragraph::new(ew.finish()), eff_rect);

            let cat_y = inner.y + 6;
            let save_y = inner.y + inner.height - 1;
            match catalog {
                CatalogState::Idle => frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        " Enter a model ID above (blank resets to provider default).",
                        pal.muted(),
                    ))),
                    Rect::new(inner.x, cat_y, inner.width, 1),
                ),
                CatalogState::Loading => frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(" Loading model catalog…", pal.active()))),
                    Rect::new(inner.x, cat_y, inner.width, 1),
                ),
                CatalogState::Error(e) => frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        truncate_display(&format!(" Catalog unavailable: {e}"), inner.width as usize),
                        pal.error(),
                    ))),
                    Rect::new(inner.x, cat_y, inner.width, 1),
                ),
                CatalogState::Loaded(list) => {
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            format!(" Catalog ({}) — click or ↑↓:", list.len()),
                            if *field == ModelField::CatalogList { pal.accent_bold() } else { pal.muted() },
                        ))),
                        Rect::new(inner.x, cat_y, inner.width, 1),
                    );
                    let max_rows = save_y.saturating_sub(cat_y + 1) as usize;
                    for (i, id) in list.iter().take(max_rows).enumerate() {
                        let sel = *selected_catalog == Some(i);
                        let row_rect = Rect::new(inner.x, cat_y + 1 + i as u16, inner.width, 1);
                        frame.render_widget(
                            Paragraph::new(Line::from(Span::styled(
                                format!(" {} {}", if sel { "▸" } else { " " }, sanitize_inline(id)),
                                if sel { pal.selected_row() } else { pal.primary().bg(pal.elevated_bg) },
                            ))),
                            row_rect,
                        );
                        hits.push(row_rect, HitTarget::ModalCatalogRow(i));
                    }
                }
            }
            let save_rect = Rect::new(inner.x, save_y, inner.width, 1);
            let mut sw = HitSpanWriter::new(save_rect, hits);
            sw.push(" ", pal.elevated(), None);
            sw.push(" Save (Enter) ", pal.pill(pal.ok), Some(HitTarget::ModalModelSave));
            sw.push("  ", pal.elevated(), None);
            sw.push(" Cancel (Esc) ", pal.pill_inactive(), Some(HitTarget::ModalClose));
            frame.render_widget(Paragraph::new(sw.finish()), save_rect);

            if *field == ModelField::ModelInput && inner.width > 12 {
                let (_, cur_x, _) = model_input.visual_layout(usize::MAX);
                let cx = inner.x + 11 + cur_x.min(inner.width.saturating_sub(12));
                frame.set_cursor_position(Position::new(cx, inner.y + 2));
            }
        }
        Overlay::Check { input } => {
            let rect = centered_rect(area, 76, 8);
            hits.push(rect, HitTarget::ModalSurface);
            frame.render_widget(Clear, rect);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(pal.elevated())
                .border_style(pal.border(true))
                .title(Span::styled(" Authorized --check Command — [Enter] Arm  [Esc] Close ", pal.accent_bold()));
            let inner = block.inner(rect);
            frame.render_widget(block, rect);
            if inner.height < 4 || inner.width < 12 {
                return;
            }
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(" Runs on completion (up to 10 attempts):", pal.muted())),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled(" $ ", pal.accent_bold()),
                        Span::styled(sanitize_inline(input.as_str()), pal.primary()),
                    ]),
                ]),
                Rect::new(inner.x, inner.y, inner.width, 3),
            );
            let btn_rect = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
            let mut bw = HitSpanWriter::new(btn_rect, hits);
            bw.push(" ", pal.elevated(), None);
            bw.push(" Arm Command (Enter) ", pal.pill(pal.ok), Some(HitTarget::ModalCheckSave));
            bw.push("  ", pal.elevated(), None);
            bw.push(" Cancel (Esc) ", pal.pill_inactive(), Some(HitTarget::ModalClose));
            frame.render_widget(Paragraph::new(bw.finish()), btn_rect);
            if inner.width > 4 {
                let (_, cur_x, _) = input.visual_layout(usize::MAX);
                let cx = inner.x + 3 + cur_x.min(inner.width.saturating_sub(4));
                frame.set_cursor_position(Position::new(cx, inner.y + 2));
            }
        }
        Overlay::Agents { selected } => {
            let rect = centered_rect(area, 48, 16);
            hits.push(rect, HitTarget::ModalSurface);
            frame.render_widget(Clear, rect);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(pal.elevated())
                .border_style(pal.border(true))
                .title(Span::styled(" Select Target Agent — [Enter] Pick  [Esc] Close ", pal.accent_bold()));
            let inner = block.inner(rect);
            frame.render_widget(block, rect);
            if state.members.is_empty() {
                frame.render_widget(Paragraph::new(Line::from(Span::styled(" No members.", pal.muted()))), inner);
            } else {
                let idx = (*selected).min(state.members.len() - 1);
                for (i, m) in state.members.iter().take(inner.height as usize).enumerate() {
                    let sel = i == idx;
                    let mark = if m.name == state.agent { "●" } else { " " };
                    let row_rect = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            format!(" {mark} {}", sanitize_inline(&m.name)),
                            if sel { pal.selected_row() } else { pal.primary().bg(pal.elevated_bg) },
                        ))),
                        row_rect,
                    );
                    hits.push(row_rect, HitTarget::ModalAgentRow(i));
                }
            }
        }
        Overlay::TextView { title, lines, scroll, is_diff } => {
            let rect = centered_rect(area, 96, 28);
            hits.push(rect, HitTarget::ModalSurface);
            frame.render_widget(Clear, rect);
            let close_x = rect.x + rect.width.saturating_sub(11);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(pal.elevated())
                .border_style(pal.border(true))
                .title(Span::styled(
                    format!(" {} — [↑↓/PgUp/PgDn] Scroll  [Esc] Close ", sanitize_inline(title)),
                    pal.accent_bold(),
                ));
            let inner = block.inner(rect);
            frame.render_widget(block, rect);
            hits.push(Rect::new(close_x, rect.y, 10.min(rect.width), 1), HitTarget::ModalClose);
            let rendered: Vec<Line> = lines
                .iter()
                .map(|raw| {
                    let clean = sanitize_inline(raw);
                    let style = if *is_diff {
                        if clean.starts_with("@@") || clean.starts_with("diff --git") {
                            pal.accent_bold().bg(pal.elevated_bg)
                        } else if clean.starts_with('+') && !clean.starts_with("+++") {
                            pal.diff_add().bg(pal.diff_add_bg)
                        } else if clean.starts_with('-') && !clean.starts_with("---") {
                            pal.diff_del().bg(pal.diff_del_bg)
                        } else {
                            pal.primary().bg(pal.elevated_bg)
                        }
                    } else {
                        pal.primary().bg(pal.elevated_bg)
                    };
                    Line::from(Span::styled(clean, style))
                })
                .collect();
            let total = rendered.len();
            let view_h = inner.height as usize;
            let pos = (*scroll as usize).min(total.saturating_sub(view_h.max(1)));
            frame.render_widget(Paragraph::new(rendered).scroll((pos as u16, 0)), inner);
            if total > view_h {
                let mut sb = ScrollbarState::new(total.saturating_sub(view_h) + 1).position(pos);
                frame.render_stateful_widget(
                    Scrollbar::new(ScrollbarOrientation::VerticalRight).style(pal.dim()),
                    inner,
                    &mut sb,
                );
            }
        }
        Overlay::Help => {
            let rect = centered_rect(area, 86, 25);
            hits.push(rect, HitTarget::ModalSurface);
            frame.render_widget(Clear, rect);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(pal.elevated())
                .border_style(pal.border(true))
                .title(Span::styled(" TeamAgents TUI Reference — [Esc / F1 / ?] Close ", pal.accent_bold()));
            let inner = block.inner(rect);
            frame.render_widget(block, rect);
            let help_lines = vec![
                Line::from(Span::styled(" Keyboard Shortcuts (Navigation, Layout & Actions)", pal.section_title())),
                Line::from("  Tab/BackTab  Complete /cmd or @file / cycle panes   Ctrl+N   New blank session"),
                Line::from("  Ctrl+B       Toggle sidebar                         Ctrl+P   Pending approvals queue"),
                Line::from("  Alt+1..5     Inspector tabs                         F1 / ?   Toggle help sheet"),
                Line::from("  Ctrl+Q       Leave TUI (daemon keeps running)       Del ×2   Delete root session"),
                Line::from("  ↑↓ / j k     Move selection / scroll                PgUp/Dn  Scroll fast"),
                Line::from("  [ / ]        Sessions / Agents sidebar              Space    Fold or pick"),
                Line::from("  a / d / r    Approve / deny / reply                 x        Cancel active"),
                Line::from("  i / /        Jump to composer (plain input on running root sends mid-turn steer)"),
                Line::from(""),
                Line::from(Span::styled(" Slash Commands (Configuration & Workflows)", pal.section_title())),
                Line::from("  /model [provider P|MODEL [EFFORT]|...]     Switch provider, model & effort"),
                Line::from("  /provider [list|use P|add ...|remove NAME] Manage custom providers"),
                Line::from("  /to [NAME]             /check [CMD|off]    Pick agent or arm --check"),
                Line::from("  /sandbox [direct|staged|read-only]         Select workspace sandbox mode"),
                Line::from("  /plan TEXT             /review code [BASE] Run read-only plan or code review"),
                Line::from("  /diff [BASE]           /steer TEXT         View git diff or steer active run"),
                Line::from("  /compact               /status             Compact active root or inspect status"),
                Line::from("  /skills                /mcp                Inspect discovered SKILL.md & MCP"),
                Line::from("  /wb [get|set|append|resolve|del] ...       Inspect or edit whiteboard"),
                Line::from("  /new [TEXT]            /answer ROOT_ID TEXT  New session or reply"),
                Line::from("  /clear                 /stop               Clear feed / stop daemon"),
            ];
            frame.render_widget(Paragraph::new(help_lines).wrap(Wrap { trim: false }), inner);
        }
    }
}

fn centered_rect(area: Rect, max_w: u16, max_h: u16) -> Rect {
    let w = area.width.min(max_w).max(area.width.min(24));
    let h = area.height.min(max_h).max(area.height.min(6));
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Member, ModelConfig, TaskRecord};
    use crate::tui::state::BackendEvent;
    use crate::tui::theme::RAINBOW_PALETTE;
    use ratatui::{backend::TestBackend, Terminal};
    use serde_json::json;

    fn render_and_hits(state: &mut AppState, width: u16, height: u16) -> (String, Vec<String>, HitMap) {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| {
                hits = render(f, state);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let mut rows = Vec::with_capacity(height as usize);
        let mut flat = String::new();
        for y in 0..buf.area.height {
            let mut row = String::new();
            let mut x = 0;
            while x < buf.area.width {
                let sym = buf[(x, y)].symbol();
                assert!(!sym.chars().any(char::is_control), "raw control char leaked in cell ({x},{y}): {sym:?}");
                row.push_str(sym);
                x += sym.width().max(1) as u16;
            }
            flat.push_str(&row);
            flat.push('\n');
            rows.push(row);
        }
        (flat, rows, hits)
    }

    fn read_rect_text(terminal_buf: &ratatui::buffer::Buffer, r: Rect) -> String {
        let mut out = String::new();
        for y in r.y..r.y + r.height {
            let mut x = r.x;
            while x < r.x + r.width {
                let sym = terminal_buf[(x, y)].symbol();
                out.push_str(sym);
                x += sym.width().max(1) as u16;
            }
        }
        out
    }

    fn sample_state() -> AppState {
        let mut state = AppState::new("team-agents".into(), "default".into(), true, SandboxMode::Direct);
        state.palette = RAINBOW_PALETTE;
        let root = TaskRecord::from_value(&json!({
            "id": "r1", "root": "r1", "agent": "default", "status": "done",
            "stage_status": "done", "kind": "root", "prompt": "修复解析器\x1b[2J",
            "result": "## Summary\n- Fixed `parser` bug\n- **Verified**",
            "turns_used": 2, "turn_budget": 32, "policy": "review",
            "verification": "passed", "check": "cargo test"
        }))
        .unwrap();
        let child = TaskRecord::from_value(&json!({
            "id": "c1", "root": "r1", "parent": "r1", "agent": "worker",
            "status": "approval", "approval_id": "a-9",
            "action": {"op": "edit_file", "path": "src/lib.rs", "old": "old_fn()", "new": "new_fn()"}
        }))
        .unwrap();
        state.apply_backend_event(BackendEvent::Snapshot {
            tasks: Ok(vec![root, child]),
            members: Ok(vec![Member { name: "default".into() }, Member { name: "worker".into() }]),
            model: Ok(ModelConfig { provider: "api".into(), model: Some("gpt-5".into()), effort: Some("high".into()) }),
            providers: None,
        });
        state
    }

    #[test]
    fn renders_responsive_layouts_markdown_and_sanitizes_control_sequences() {
        let mut state = sample_state();
        assert_eq!(state.selected_task_id, None);
        let (welcome, _, _) = render_and_hits(&mut state, 128, 36);
        assert!(welcome.contains("TeamAgents") && welcome.contains("New session"));

        state.selected_task_id = Some("r1".into());
        let (wide, _, _) = render_and_hits(&mut state, 128, 36);
        assert!(wide.contains("修复解析器\\u001b[2J"));
        assert!(wide.contains("Summary") && wide.contains("• Fixed"));
        assert!(wide.contains("approval(s) waiting"));
        assert!(!wide.contains('\x1b'));

        state.overlay = Overlay::Approvals { selected: 0 };
        let (modal, _, _) = render_and_hits(&mut state, 128, 36);
        assert!(modal.contains("Pending Approvals (1)") && modal.contains("edit_file") && modal.contains("+ new_fn()"));

        state.overlay = Overlay::None;
        state.focus = Focus::Sidebar;
        let (narrow, _, _) = render_and_hits(&mut state, 78, 22);
        assert!(narrow.contains("Sessions (2)") && narrow.contains("Agents (2)"));
    }

    #[test]
    fn hitmap_rects_match_actual_rendered_button_cells_across_resizes() {
        let mut state = sample_state();
        state.selected_task_id = Some("r1".into());
        for (w, h) in [(76, 22), (100, 30), (140, 40), (180, 48)] {
            state.focus = Focus::Inspector;
            state.inspector_tab = InspectorTab::Summary;
            let backend = TestBackend::new(w, h);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = HitMap::default();
            terminal.draw(|f| hits = render(f, &mut state)).unwrap();
            let buf = terminal.backend().buffer();

            for (rect, target) in &hits.entries {
                let text = read_rect_text(buf, *rect);
                match target {
                    HitTarget::InspectorTab(InspectorTab::Summary) => {
                        assert!(text.contains("Sum"), "{w}x{h}: {text:?}")
                    }
                    HitTarget::InspectorTab(InspectorTab::Evidence) => {
                        assert!(text.contains("Evd") || text.contains("Evidence"), "{w}x{h}: {text:?}");
                    }
                    HitTarget::InspectorTab(InspectorTab::Tree) => {
                        assert!(text.contains("Tre") || text.contains("Tree"), "{w}x{h}: {text:?}");
                    }
                    HitTarget::InspectorTab(InspectorTab::Events) => assert!(text.contains("Log"), "{w}x{h}: {text:?}"),
                    HitTarget::InspectorTab(InspectorTab::Board) => {
                        assert!(text.contains("Board") || text.contains("Brd"), "{w}x{h}: {text:?}")
                    }
                    HitTarget::InlineApprove(_) => assert!(text.contains("Approve"), "{w}x{h}: {text:?}"),
                    HitTarget::InlineDeny(_) => assert!(text.contains("Deny"), "{w}x{h}: {text:?}"),
                    HitTarget::SidebarTab(SidebarTab::Trees) if w >= WIDE_THRESHOLD => {
                        assert!(text.contains("Sessions"), "{w}x{h}: {text:?}");
                    }
                    HitTarget::SidebarTab(SidebarTab::Members) if w >= WIDE_THRESHOLD => {
                        assert!(text.contains("Agents"), "{w}x{h}: {text:?}");
                    }
                    HitTarget::OpenHelp => assert!(text.contains("Help"), "{w}x{h}: {text:?}"),
                    HitTarget::NewSession => assert!(text.contains("New"), "{w}x{h}: {text:?}"),
                    _ => {}
                }
            }

            // Clicking exact center of rendered Evidence tab must switch to Evidence.
            let ev_rect = hits
                .entries
                .iter()
                .find_map(|(r, t)| matches!(t, HitTarget::InspectorTab(InspectorTab::Evidence)).then_some(*r))
                .expect("Evidence tab hitbox present");
            state.handle_mouse(
                crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                    column: ev_rect.x + ev_rect.width / 2,
                    row: ev_rect.y,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                },
                &hits,
            );
            assert_eq!(state.inspector_tab, InspectorTab::Evidence);
        }
    }

    #[test]
    fn renders_plan_live_activity_mentions_and_diff_overlay() {
        let mut state = sample_state();
        let active = TaskRecord::from_value(&json!({
            "id": "r2", "root": "r2", "agent": "default", "status": "running", "kind": "root",
            "sandbox": "read_only", "pending_steerings": 1,
            "usage": {"input_tokens": 4200, "cached_tokens": 3150, "output_tokens": 210, "cache_hit_rate": 0.75, "last_input_tokens": 1800},
            "plan": [{"step": "Inspect store.rs", "status": "completed"}, {"step": "Patch UI", "status": "in_progress"}],
            "live": {"phase": "running", "reasoning": "", "commentary": "Checking tree rendering", "active_tools": ["read_file"], "shell_tail": "line1"},
            "evidence": [{"action": {"op": "apply_patch", "patch": "*** Begin Patch\n*** Update File: a.txt\n@@\n-old\n+new\n*** End Patch"}, "ok": true, "result": "M a.txt (+1/-1)"}]
        }))
        .unwrap();
        state.apply_backend_event(BackendEvent::Snapshot {
            tasks: Ok(vec![active]),
            members: Ok(vec![Member { name: "default".into() }]),
            model: Ok(ModelConfig { provider: "deepseek".into(), model: Some("deepseek-flash".into()), effort: None }),
            providers: None,
        });
        state.apply_backend_event(BackendEvent::FilesLoaded(vec!["src/tui/ui.rs".into(), "Cargo.toml".into()]));
        state.selected_task_id = Some("r2".into());
        state.focus = Focus::Composer;

        let (summary_view, _, _) = render_and_hits(&mut state, 128, 38);
        assert!(summary_view.contains("Live Activity") && summary_view.contains("read_file"));
        assert!(summary_view.contains("Plan (2)") && summary_view.contains("Inspect store.rs"));
        assert!(summary_view.contains("4200 tok") && summary_view.contains("75% cached"));
        assert!(summary_view.contains("Steer active session"));

        state.input.set_text("check @ui.rs");
        let (_, _, hits) = render_and_hits(&mut state, 128, 38);
        assert!(hits.entries.iter().any(|(_, t)| matches!(t, HitTarget::MentionItem(_))));

        state.input.clear();
        state.inspector_tab = InspectorTab::Evidence;
        let (ev_view, _, _) = render_and_hits(&mut state, 128, 38);
        assert!(ev_view.contains("apply_patch") && ev_view.contains("M a.txt (+1/-1)"));

        let appr_child = TaskRecord::from_value(&json!({
            "id": "c-patch", "root": "r2", "parent": "r2", "agent": "worker",
            "status": "approval", "approval_id": "ap-patch",
            "action": {"op": "apply_patch", "patch": "*** Begin Patch\n*** Update File: a.txt\n@@\n-old\n+new\n*** End Patch"}
        }))
        .unwrap();
        state.apply_backend_event(BackendEvent::Snapshot {
            tasks: Ok(vec![appr_child]),
            members: Ok(vec![Member { name: "default".into() }]),
            model: Ok(ModelConfig { provider: "deepseek".into(), model: Some("deepseek-flash".into()), effort: None }),
            providers: None,
        });
        state.overlay = Overlay::Approvals { selected: 0 };
        let (appr_view, _, _) = render_and_hits(&mut state, 128, 38);
        assert!(appr_view.contains("apply_patch:") && appr_view.contains("+new"));

        state.overlay = Overlay::TextView {
            title: "Workspace Diff".into(),
            lines: vec!["@@ -1,1 +1,1 @@".into(), "-old line".into(), "+new line".into()],
            scroll: 0,
            is_diff: true,
        };
        let (diff_view, _, _) = render_and_hits(&mut state, 128, 38);
        assert!(diff_view.contains("Workspace Diff") && diff_view.contains("+new line"));
    }
}
