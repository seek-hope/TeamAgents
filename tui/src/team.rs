//! User-only team observations. Navigation never dispatches work or changes the Leader composer.
use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use serde_json::Value as Json;

use crate::{
    theme::*,
    transcript::Transcript,
    v2app::{ChatEntry, Focus, V2App},
    v2ui::{fit, panel, selection},
    viewport::window,
};

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    #[default]
    Conversation,
    Tasks,
    Activity,
    Inspect,
}

pub const VIEWS: [(View, &str); 4] = [
    (View::Conversation, "1 Chat"),
    (View::Tasks, "2 Tasks"),
    (View::Activity, "3 Activity"),
    (View::Inspect, "4 Inspect"),
];

pub struct MemberHistory {
    pub id: String,
    pub entries: Vec<ChatEntry>,
    pub transcript: Transcript,
    pub dirty: bool,
    pub error: Option<String>,
    pub older: bool,
    pub scroll: usize,
    pub lines: usize,
    pub width: usize,
    pub height: usize,
}

impl MemberHistory {
    fn new(id: String) -> Self {
        Self {
            id,
            entries: vec![],
            transcript: Transcript::default(),
            dirty: true,
            error: None,
            older: false,
            scroll: 0,
            lines: 0,
            width: 0,
            height: 1,
        }
    }

    pub fn apply(&mut self, reply: &Json) {
        if reply["instance_id"].as_str() != Some(&self.id)
            || !reply["entries"].is_array()
            || reply["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|row| row["instance_id"].as_str().is_some_and(|id| id != self.id))
        {
            self.fail("Member history identity or entries missing".into());
            return;
        }
        self.entries = crate::transcript::project(reply["entries"].as_array().unwrap(), &self.id, true);
        self.older = !reply["next_before"].is_null();
        self.error = None;
        self.dirty = false;
    }

    pub fn fail(&mut self, error: String) {
        self.dirty = true;
        self.error = Some(error);
    }
}

pub struct Team {
    pub view: View,
    pub(crate) leader_id: String,
    pub selected: String,
    pub rows: Vec<Json>,
    pub tasks: Vec<Json>,
    pub tasks_known: bool,
    pub all_tasks: Option<Vec<Json>>,
    pub tasks_dirty: bool,
    pub tasks_error: Option<String>,
    pub activity: VecDeque<String>,
    last_event: i64,
    pub scroll: usize,
    pub lines: usize,
    pub height: usize,
    pub history_open: bool,
    pub member: Option<MemberHistory>,
    /// Set by geometry/rendering; hidden histories do not cause polling.
    pub member_visible: bool,
    pub tasks_visible: bool,
}

impl Default for Team {
    fn default() -> Self {
        Self {
            view: View::Conversation,
            leader_id: "i-leader".into(),
            selected: "i-leader".into(),
            rows: vec![],
            tasks: vec![],
            tasks_known: false,
            all_tasks: None,
            tasks_dirty: true,
            tasks_error: None,
            activity: VecDeque::new(),
            last_event: 0,
            scroll: 0,
            lines: 0,
            height: 1,
            history_open: false,
            member: None,
            member_visible: false,
            tasks_visible: false,
        }
    }
}

impl Team {
    pub fn checkpoint(&mut self, snapshot: &Json) {
        self.rows = snapshot["instances"].as_array().cloned().unwrap_or_default();
        // Old protocol-2 daemons omitted role_models. A present record with no Leader is not a fallback.
        let leader = snapshot.get("role_models").map_or("i-leader", |r| r["leader_id"].as_str().unwrap_or(""));
        if self.leader_id != leader {
            self.leader_id = leader.into();
            self.selected = leader.into();
            self.member = None;
            self.scroll = 0;
        }
        self.rows.sort_by_key(|row| (row["id"] != leader, row["id"].as_str().unwrap_or("").to_string()));
        self.tasks = snapshot["tasks"].as_array().cloned().unwrap_or_default();
        self.tasks_known = snapshot["tasks"].is_array();
        self.tasks_dirty = true;
        if let Some(member) = &mut self.member {
            member.dirty = true;
        }
        if !self.rows.iter().any(|row| row["id"] == self.selected) {
            self.select(leader);
        }
    }

    pub fn select(&mut self, id: &str) {
        if id != self.selected {
            self.selected = id.to_string();
            self.scroll = 0;
            self.member = (id != self.leader_id).then(|| MemberHistory::new(id.to_string()));
        }
    }

    pub fn selected_index(&self) -> usize {
        self.rows.iter().position(|row| row["id"] == self.selected).unwrap_or(0)
    }

    pub fn move_selection(&mut self, down: bool) {
        let index = if down {
            (self.selected_index() + 1).min(self.rows.len().saturating_sub(1))
        } else {
            self.selected_index().saturating_sub(1)
        };
        if let Some(id) = self.rows.get(index).and_then(|row| row["id"].as_str()).map(str::to_string) {
            self.select(&id);
        }
    }

    pub fn show(&mut self, view: View) {
        self.view = view;
        self.scroll = 0;
        if view == View::Tasks {
            self.tasks_dirty = true;
        }
    }

    pub fn events(&mut self, events: &[Json]) {
        for event in events {
            let Some(sequence) = event["sequence"].as_i64().filter(|seq| *seq > self.last_event) else { continue };
            self.last_event = sequence;
            // Only short event metadata is retained. Bodies and private messages stay in history.
            let detail = ["reason", "status", "tool", "lifecycle", "task_id", "operation_id"]
                .into_iter()
                .filter_map(|key| event["payload"][key].as_str())
                .map(|value| value.chars().take(240).collect::<String>())
                .collect::<Vec<_>>()
                .join(" · ");
            self.activity.push_front(format!(
                "#{sequence}  {}  {}{}{}",
                fit(event["scope"].as_str().unwrap_or("session"), 80),
                fit(event["kind"].as_str().unwrap_or("unknown event"), 80),
                if detail.is_empty() { "" } else { " · " },
                fit(&detail, 320)
            ));
            self.activity.truncate(200);
        }
        if !events.is_empty() {
            self.tasks_dirty = true;
            if let Some(member) = &mut self.member {
                member.dirty = true;
            }
        }
    }

    pub fn apply_tasks(&mut self, reply: &Json) {
        if let Some(rows) = reply["tasks"].as_array() {
            self.all_tasks = Some(rows.clone());
            self.tasks_dirty = false;
            self.tasks_error = None;
        } else {
            self.tasks_error = Some("tasks reply missing rows".into());
            self.tasks_dirty = true;
        }
    }

    pub fn scroll_by(&mut self, down: bool, amount: usize) {
        if self.history_open && matches!(self.view, View::Inspect | View::Conversation) {
            if let Some(member) = &mut self.member {
                member.scroll = if down {
                    member.scroll.saturating_sub(amount)
                } else {
                    member.scroll.saturating_add(amount).min(member.lines.saturating_sub(member.height))
                };
                return;
            }
        }
        self.scroll = if down {
            self.scroll.saturating_add(amount).min(self.lines.saturating_sub(self.height))
        } else {
            self.scroll.saturating_sub(amount)
        };
    }
}

/// Panel keys never enter the draft or execute a runtime command.
pub fn key(app: &mut V2App, key: KeyEvent) {
    use crossterm::event::KeyEventKind;
    if !key.modifiers.is_empty()
        || key.kind == KeyEventKind::Release
        || (key.kind == KeyEventKind::Repeat
            && matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('h' | 'd')))
    {
        return;
    }
    match key.code {
        KeyCode::Esc => app.focus = Focus::Composer,
        KeyCode::Up if app.focus == Focus::Team => app.team.move_selection(false),
        KeyCode::Down if app.focus == Focus::Team => app.team.move_selection(true),
        KeyCode::Enter if app.focus == Focus::Team => {
            app.team.show(View::Inspect);
            app.focus = Focus::Panel;
        }
        KeyCode::Char('h') => {
            if app.team.selected == app.team.leader_id {
                app.team.show(View::Conversation);
                app.focus = Focus::Composer;
            } else {
                app.team.history_open = true;
                app.team.show(View::Inspect);
                app.focus = Focus::Panel;
            }
        }
        KeyCode::Char('d') => {
            app.team.history_open = false;
            app.team.show(View::Inspect);
        }
        KeyCode::Up => app.team.scroll_by(false, 1),
        KeyCode::Down => app.team.scroll_by(true, 1),
        KeyCode::PageUp => app.team.scroll_by(false, app.team.height.max(1)),
        KeyCode::PageDown => app.team.scroll_by(true, app.team.height.max(1)),
        _ => {}
    }
}

pub fn state(row: &Json) -> &str {
    match row["lifecycle"].as_str() {
        Some("ACTIVE") => match row["phase"].as_str() {
            Some("MODEL_PENDING") => "THINKING",
            Some("TOOLS_PENDING") => "TOOLS",
            Some("COMPLETION_PENDING") => "CHECKING",
            Some("WAITING_CAPABILITY") => "APPROVAL",
            Some(phase) => phase,
            None => "UNKNOWN",
        },
        Some(value) => value,
        None => "UNKNOWN",
    }
}

fn state_style(state: &str) -> Style {
    Style::default().fg(match state {
        "PARKED" | "BLOCKED" | "FAILED" | "OUTCOME_UNKNOWN" => ERROR,
        "WAITING" | "PAUSED" | "APPROVAL" => WARNING,
        "SUCCEEDED" => SUCCESS,
        _ => ACCENT,
    })
}

/// Same member window used for painting and hit testing.
pub fn member_window(app: &V2App, area: Rect) -> (Rect, usize, usize) {
    let inner = panel("").inner(area);
    let reserve = if inner.height >= 15 { 7 } else { 0 };
    let list = Rect::new(
        inner.x,
        inner.y.saturating_add(1).min(inner.bottom()),
        inner.width,
        inner.height.saturating_sub(reserve + 1),
    );
    let (start, count) = window(app.team.rows.len(), app.team.selected_index(), list.height as usize / 2);
    (list, start, count)
}

pub fn render_sidebar(frame: &mut Frame, app: &V2App, area: Rect) {
    if area.is_empty() {
        return;
    }
    let block = panel(format!("Team · {}", app.team.rows.len())).border_style(border(app.focus == Focus::Team));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(" F6 focus · Enter inspect").style(secondary()),
        Rect::new(inner.x, inner.y, inner.width, inner.height.min(1)),
    );
    let (list, start, count) = member_window(app, area);
    let mut lines = vec![];
    for row in app.team.rows.iter().skip(start).take(count) {
        let id = row["id"].as_str().unwrap_or("?");
        let selected = id == app.team.selected;
        let label = if id == app.team.leader_id { "Leader" } else { id };
        lines.push(Line::styled(
            fit(&format!("{} {label}", if selected { "›" } else { " " }), list.width as usize),
            if selected { selection() } else { Style::default().fg(FG) },
        ));
        lines.push(Line::from(vec![
            Span::styled(format!("  {}", state(row)), state_style(state(row))),
            Span::styled(
                format!(
                    " · {}",
                    fit(
                        &app.member_model_label(id, row["model"].as_str().unwrap_or("")),
                        list.width.saturating_sub(14) as usize
                    )
                ),
                secondary(),
            ),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::styled(" Waiting for team snapshot", secondary()));
    }
    frame.render_widget(Paragraph::new(lines), list);
    if inner.height >= 15 {
        let mut summary = vec![
            Line::styled("WORK", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Line::raw(if app.team.tasks_known {
                format!("{} open tasks", app.team.tasks.len())
            } else {
                "Open tasks unreported".into()
            }),
            Line::raw(format!("{} approvals", app.approvals.len())),
        ];
        if let Some(work) = &app.work {
            summary.push(Line::styled(work.status.clone(), state_style(&work.status)));
            summary.push(Line::raw(format!(
                "{} tokens{}",
                work.known_total,
                if work.unknown { " + unknown" } else { "" }
            )));
            if let Some(limit) = work.limit_total {
                summary.push(Line::raw(format!("Token limit {limit}")));
            }
            if let Some(deadline) = work.deadline {
                summary.push(Line::raw(format!(
                    "Deadline in {}m",
                    ((deadline - crate::now_epoch()) / 60.0).ceil() as i64
                )));
            }
        }
        frame.render_widget(
            Paragraph::new(summary).style(secondary()),
            Rect::new(inner.x, inner.bottom() - 7, inner.width, 7),
        );
    }
}

pub fn render_nav(frame: &mut Frame, app: &V2App, area: Rect) {
    if area.is_empty() {
        return;
    }
    let lines = std::iter::once(Span::styled("Alt ", secondary()))
        .chain(VIEWS.iter().map(|(view, label)| {
            Span::styled(format!(" {label} "), if app.team.view == *view { selection() } else { secondary() })
        }))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Line::from(lines)).style(Style::default().bg(PANEL_BG)), area);
}

pub fn nav_at(area: Rect, column: u16) -> Option<View> {
    let mut x = area.x.saturating_add(4);
    for (view, label) in VIEWS {
        let end = x.saturating_add(label.len() as u16 + 2).min(area.right());
        if (x..end).contains(&column) {
            return Some(view);
        }
        x = end;
    }
    None
}

fn detail_lines(app: &V2App) -> Vec<String> {
    let Some(row) = app.team.rows.iter().find(|row| row["id"] == app.team.selected) else {
        return vec!["No member snapshot available.".into()];
    };
    let diagnostic = &row["diagnosis"];
    let mut lines = vec![
        format!("{} · {}", app.team.selected, state(row)),
        format!(
            "Model: {} · {}",
            app.member_model_label(&app.team.selected, row["model"].as_str().unwrap_or("")),
            row["effort"].as_str().unwrap_or("default effort")
        ),
        String::new(),
        diagnostic["summary"].as_str().unwrap_or("No diagnosis reported by this daemon.").into(),
    ];
    if let Some(lifetime) = row["lifetime"].as_str() {
        lines.push(format!("Worker lifetime: {lifetime}"));
    }
    if let Some(reason) = diagnostic["reason"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| row["reason"].as_str().filter(|s| !s.is_empty()))
    {
        lines.push(format!("Reason: {reason}"));
    }
    if let Some(state) = diagnostic["work"]["state"].as_str() {
        lines.push(format!("Work state: {state}"));
    }
    if let Some(next) = diagnostic["next_steps"].as_str() {
        lines.push(format!("Next: {next}"));
    }
    for wait in diagnostic["waits"].as_array().into_iter().flatten() {
        lines.push(format!(
            "Wait {} · {} · timer {}",
            wait["id"].as_str().unwrap_or("?"),
            wait["mode"].as_str().unwrap_or("?"),
            wait["timer_at"]
        ));
        lines.push(format!("Conditions: {}", wait["conditions"]));
    }
    for operation in diagnostic["operations"].as_array().into_iter().flatten() {
        lines.push(format!(
            "Operation {}: {}{}",
            operation["id"].as_str().unwrap_or("?"),
            operation["status"].as_str().unwrap_or("?"),
            if operation["cancel_requested"] == true { " · cancellation requested" } else { "" }
        ));
    }
    lines.push(String::new());
    lines.push("ASSIGNED OPEN TASKS".into());
    let mut assigned = 0;
    for task in app.team.tasks.iter().filter(|task| task["assignee"] == app.team.selected) {
        assigned += 1;
        lines.push(format!("{} · {}", task["id"].as_str().unwrap_or("?"), task["status"].as_str().unwrap_or("?")));
        if let Some(reason) = task["diagnosis"]["reason"].as_str() {
            lines.push(reason.into());
        }
    }
    if assigned == 0 {
        lines.push(if app.team.tasks_known {
            "None at this checkpoint.".into()
        } else {
            "No open-task snapshot reported.".into()
        });
    }
    lines
}

fn render_lines(frame: &mut Frame, app: &mut V2App, area: Rect, lines: Vec<String>) {
    let lines: Vec<_> = lines
        .iter()
        .flat_map(|line| crate::markdown::plain(line, area.width as usize, Style::default().fg(FG)))
        .collect();
    app.team.lines = lines.len();
    app.team.height = area.height.max(1) as usize;
    app.team.scroll = app.team.scroll.min(lines.len().saturating_sub(area.height as usize));
    frame.render_widget(
        Paragraph::new(lines.into_iter().skip(app.team.scroll).take(area.height as usize).collect::<Vec<_>>()),
        area,
    );
}

pub fn render_panel(frame: &mut Frame, app: &mut V2App, area: Rect, inspect: bool) {
    if area.is_empty() {
        return;
    }
    let title = if inspect {
        format!("{} · d details / h history", app.team.selected)
    } else if app.team.view == View::Tasks {
        "Tasks · recorded outcomes".into()
    } else {
        "Activity · observed since attachment".into()
    };
    let block = panel(title).border_style(border(app.focus == Focus::Panel));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inspect && app.team.history_open && app.team.selected != app.team.leader_id {
        render_history(frame, app, inner);
        return;
    }
    let lines = if inspect {
        detail_lines(app)
    } else if app.team.view == View::Tasks {
        app.team.tasks_visible = !inner.is_empty();
        let mut lines = vec!["Task outcome is not process-stop evidence.".into(), String::new()];
        if let Some(error) = &app.team.tasks_error {
            lines.push(format!("Refresh failed; rows may be stale: {error}"));
        }
        let tasks = app.team.all_tasks.as_ref().unwrap_or(&app.team.tasks);
        if app.team.all_tasks.is_none() {
            lines.push("Loading all tasks; showing open checkpoint rows.".into());
        }
        if tasks.is_empty() {
            lines.push("No tasks reported.".into());
        }
        for task in tasks {
            lines.push(format!(
                "{}  {}  → {}",
                task["status"].as_str().unwrap_or("?"),
                task["id"].as_str().unwrap_or("?"),
                task["assignee"].as_str().unwrap_or("?")
            ));
            if let Some(summary) = task["diagnosis"]["summary"].as_str() {
                lines.push(format!("  {summary}"));
            }
            if let Some(reason) = task["diagnosis"]["reason"].as_str() {
                lines.push(format!("  Reason: {reason}"));
            }
            lines.push(String::new());
        }
        lines
    } else {
        let mut lines = vec![
            "Newest 200 observed events. Reconnect gaps are possible; export for a full audit.".into(),
            String::new(),
        ];
        lines.extend(app.team.activity.iter().cloned());
        if app.team.activity.is_empty() {
            lines.push("No events observed since attachment.".into());
        }
        lines
    };
    render_lines(frame, app, inner, lines);
}

fn render_history(frame: &mut Frame, app: &mut V2App, area: Rect) {
    let Some(member) = &mut app.team.member else { return };
    app.team.member_visible = true;
    let note = if let Some(error) = &member.error {
        format!("Stale history · retrying: {error}")
    } else if member.dirty && member.entries.is_empty() {
        "Loading member history…".into()
    } else if member.older {
        "Newest 400 entries · older history in /sessions".into()
    } else {
        "Read-only member history · composer targets Leader".into()
    };
    frame.render_widget(
        Paragraph::new(fit(&note, area.width as usize)).style(secondary()),
        Rect::new(area.x, area.y, area.width, area.height.min(1)),
    );
    let area = Rect::new(
        area.x,
        area.y.saturating_add(1).min(area.bottom()),
        area.width.saturating_sub(1),
        area.height.saturating_sub(1),
    );
    let base = member.transcript.document(
        &member.entries,
        area.width as usize,
        Some(if app.expanded_tools { 40 } else { 4 }),
        Some(&mut app.graphics),
        &app.session_root,
    );
    let mut tail = crate::graphics::Document::default();
    if let Some((_, preview)) = app.previews.iter().find(|(id, _)| id == &member.id) {
        let origin = crate::graphics::Origin {
            session: app.session_root.clone().into(),
            workspace: member
                .entries
                .iter()
                .rev()
                .find(|e| !e.workspace.is_empty())
                .map(|e| e.workspace.clone())
                .unwrap_or_default()
                .into(),
            member: member.id.clone(),
            position: usize::MAX,
        };
        tail.append(crate::transcript::assistant_document(
            preview,
            &member.id,
            area.width as usize,
            Some(&mut app.graphics),
            &origin,
            true,
        ));
    }
    let total = base.lines.len() + tail.lines.len();
    let previous = if member.width == area.width as usize { member.lines } else { total };
    let (start, offset) = crate::viewport::transcript_window(total, previous, area.height as usize, member.scroll);
    member.scroll = offset;
    member.lines = total;
    member.width = area.width as usize;
    member.height = area.height.max(1) as usize;
    app.team.height = member.height;
    let (visible, pictures) = base.viewport(&tail, &mut app.graphics, start, area);
    app.pictures.extend(pictures);
    frame.render_widget(Paragraph::new(visible), area);
}
