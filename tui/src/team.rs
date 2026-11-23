//! User-only team observations and explicitly confirmed member/task controls.
use std::collections::{HashMap, HashSet, VecDeque};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};
use serde_json::Value as Json;

use crate::{
    overlays::ConfirmAction,
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
    Tree,
    Wall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavAction {
    Alert,
    Zoom,
    Sidebar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorVerb {
    Message,
    Pause,
    Resume,
    Terminate,
    Pin,
}

pub const VIEWS: [(View, &str); 6] = [
    (View::Conversation, "1 Chat"),
    (View::Tasks, "2 Tasks"),
    (View::Activity, "3 Activity"),
    (View::Inspect, "4 Inspect"),
    (View::Tree, "5 Tree"),
    (View::Wall, "6 Wall"),
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
    pub pinned: Option<String>,
    pub rows: Vec<Json>,
    pub tasks: Vec<Json>,
    pub tasks_known: bool,
    pub all_tasks: Option<Vec<Json>>,
    pub tasks_dirty: bool,
    pub tasks_error: Option<String>,
    pub selected_task: usize,
    pub task_row_ranges: Vec<(usize, std::ops::Range<usize>)>,
    pub tasks_list_rect: Rect,
    pub tasks_follow_selection: bool,
    pub activity: VecDeque<String>,
    pub activity_filter_selected: bool,
    last_event: i64,
    pub scroll: usize,
    pub lines: usize,
    pub height: usize,
    pub history_open: bool,
    pub member: Option<MemberHistory>,
    /// Set by geometry/rendering; hidden histories do not cause polling.
    pub member_visible: bool,
    pub tasks_visible: bool,
    pub tree: Option<Json>,
    pub tree_dirty: bool,
    pub tree_visible: bool,
    pub tree_error: Option<String>,
    pub tree_selected: usize,
    pub tree_collapsed: HashSet<String>,
    pub phase_since: HashMap<String, (String, f64)>,
}

impl Default for Team {
    fn default() -> Self {
        Self {
            view: View::Conversation,
            leader_id: "i-leader".into(),
            selected: "i-leader".into(),
            pinned: None,
            rows: vec![],
            tasks: vec![],
            tasks_known: false,
            all_tasks: None,
            tasks_dirty: true,
            tasks_error: None,
            selected_task: 0,
            task_row_ranges: vec![],
            tasks_list_rect: Rect::default(),
            tasks_follow_selection: false,
            activity: VecDeque::new(),
            activity_filter_selected: false,
            last_event: 0,
            scroll: 0,
            lines: 0,
            height: 1,
            history_open: false,
            member: None,
            member_visible: false,
            tasks_visible: false,
            tree: None,
            tree_dirty: true,
            tree_visible: false,
            tree_error: None,
            tree_selected: 0,
            tree_collapsed: HashSet::new(),
            phase_since: HashMap::new(),
        }
    }
}

impl Team {
    pub fn inspected_id(&self, right_inspector: bool) -> &str {
        if right_inspector {
            self.pinned.as_deref().unwrap_or(&self.selected)
        } else {
            &self.selected
        }
    }

    pub fn ensure_inspected_history(&mut self, id: &str) {
        if id != self.leader_id && self.member.as_ref().is_none_or(|m| m.id != id) {
            self.member = Some(MemberHistory::new(id.to_string()));
        }
    }

    pub fn phase_elapsed(&self, id: &str) -> Option<u64> {
        let row = self.rows.iter().find(|r| r["id"].as_str() == Some(id))?;
        let busy = row["lifecycle"].as_str() == Some("ACTIVE")
            && (matches!(row["phase"].as_str(), Some("MODEL_PENDING" | "TOOLS_PENDING" | "COMPLETION_PENDING"))
                || (row["phase"].as_str() == Some("READY") && row["diagnosis"]["work"]["state"] == "RUNNING"));
        if !busy {
            return None;
        }
        let (_, started) = self.phase_since.get(id)?;
        let secs = (crate::now_epoch() - *started).floor().max(0.0) as u64;
        (secs >= 1).then_some(secs)
    }

    pub fn apply_tree(&mut self, reply: &Json) {
        self.tree = Some(reply.clone());
        self.tree_dirty = false;
        self.tree_error = None;
    }

    pub fn fail_tree(&mut self, error: String) {
        self.tree_dirty = true;
        self.tree_error = Some(error);
    }

    pub fn visible_tree_nodes(&self) -> Vec<(usize, &Json, bool)> {
        let Some(tree) = &self.tree else { return vec![] };
        let Some(nodes) = tree["nodes"].as_array() else { return vec![] };
        let parents: HashMap<&str, Option<&str>> =
            nodes.iter().filter_map(|n| Some((n["id"].as_str()?, n["parent_id"].as_str()))).collect();
        let mut children_set: HashSet<&str> = HashSet::new();
        for n in nodes {
            if let Some(p) = n["parent_id"].as_str() {
                children_set.insert(p);
            }
        }
        let mut out = Vec::new();
        for node in nodes {
            let id = node["id"].as_str().unwrap_or("");
            let mut cur = parents.get(id).copied().flatten();
            let mut depth = 0usize;
            let mut hidden = false;
            let mut seen = HashSet::new();
            while let Some(pid) = cur {
                if !seen.insert(pid) {
                    break;
                }
                if self.tree_collapsed.contains(pid) {
                    hidden = true;
                    break;
                }
                depth += 1;
                cur = parents.get(pid).copied().flatten();
            }
            if !hidden {
                out.push((depth, node, children_set.contains(id)));
            }
        }
        out
    }

    pub fn member_attention(row: &Json, tasks: &[Json]) -> bool {
        let id = row["id"].as_str().unwrap_or("");
        let st = state(row);
        row["diagnosis"]["attention"] == true
            || matches!(st, "PARKED" | "BLOCKED" | "FAILED" | "OUTCOME_UNKNOWN" | "APPROVAL")
            || (!id.is_empty() && tasks.iter().any(|t| t["assignee"].as_str() == Some(id) && t["status"] == "BLOCKED"))
    }

    pub fn has_busy_member(&self) -> bool {
        self.rows.iter().any(|row| {
            row["lifecycle"].as_str() == Some("ACTIVE")
                && (matches!(row["phase"].as_str(), Some("MODEL_PENDING" | "TOOLS_PENDING" | "COMPLETION_PENDING"))
                    || (row["phase"].as_str() == Some("READY") && row["diagnosis"]["work"]["state"] == "RUNNING"))
        })
    }

    pub fn attention_count(&self) -> usize {
        self.rows.iter().filter(|row| Self::member_attention(row, &self.tasks)).count()
    }

    pub fn blocked_task_count(&self) -> usize {
        self.all_tasks
            .as_ref()
            .unwrap_or(&self.tasks)
            .iter()
            .filter(|task| task["status"].as_str() == Some("BLOCKED"))
            .count()
    }

    pub fn next_attention(&mut self) -> Option<String> {
        if self.rows.is_empty() {
            return None;
        }
        let start = self.selected_index();
        let len = self.rows.len();
        for step in 1..=len {
            let row = &self.rows[(start + step) % len];
            if Self::member_attention(row, &self.tasks) {
                if let Some(id) = row["id"].as_str().map(str::to_string) {
                    self.select(&id);
                    return Some(id);
                }
            }
        }
        None
    }

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
        let now = crate::now_epoch();
        let mut live_ids = HashSet::new();
        for row in &self.rows {
            let id = row["id"].as_str().unwrap_or("");
            if id.is_empty() {
                continue;
            }
            live_ids.insert(id.to_string());
            let key = format!("{}:{}", row["lifecycle"].as_str().unwrap_or(""), row["phase"].as_str().unwrap_or(""));
            match self.phase_since.get(id) {
                Some((prev, _)) if prev == &key => {}
                _ => {
                    self.phase_since.insert(id.to_string(), (key, now));
                }
            }
        }
        self.phase_since.retain(|id, _| live_ids.contains(id));
        if let Some(pin) = &self.pinned {
            if !live_ids.contains(pin) {
                self.pinned = None;
            }
        }
        self.tasks = snapshot["tasks"].as_array().cloned().unwrap_or_default();
        self.tasks_known = snapshot["tasks"].is_array();
        self.tasks_dirty = true;
        self.tree_dirty = true;
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
            if self.pinned.is_none() {
                self.member = (id != self.leader_id).then(|| MemberHistory::new(id.to_string()));
            }
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
            if self.selected_task > 0 {
                self.tasks_follow_selection = true;
            }
        } else if view == View::Tree {
            self.tree_dirty = true;
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
            self.tree_dirty = true;
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

    pub fn scroll_edge(&mut self, bottom: bool) {
        if self.history_open && matches!(self.view, View::Inspect | View::Conversation) {
            if let Some(member) = &mut self.member {
                member.scroll = if bottom { 0 } else { member.lines.saturating_sub(member.height) };
                return;
            }
        }
        self.scroll = if bottom { self.lines.saturating_sub(self.height) } else { 0 };
    }
}

/// Panel keys never enter the draft or execute a runtime command without explicit confirmation.
pub fn key(app: &mut V2App, key: KeyEvent) {
    use crossterm::event::KeyEventKind;
    let allowed_shift = key.modifiers == KeyModifiers::SHIFT && matches!(key.code, KeyCode::Char('!' | 'X' | 'P'));
    if (!key.modifiers.is_empty() && !allowed_shift)
        || key.kind == KeyEventKind::Release
        || (key.kind == KeyEventKind::Repeat
            && matches!(
                key.code,
                KeyCode::Enter
                    | KeyCode::Esc
                    | KeyCode::Char('h' | 'd' | '!' | 'z' | 'b' | 'f' | 'm' | 'p' | 'r' | 'x' | 'X' | 'P' | ' ')
            ))
    {
        return;
    }
    let inspect_target = app.team.inspected_id(app.team.view == View::Conversation).to_string();
    match key.code {
        KeyCode::Esc => app.focus = Focus::Composer,
        KeyCode::Char('!') => app.jump_next_attention(),
        KeyCode::Char('z') => app.toggle_zoom(),
        KeyCode::Char('b') => app.toggle_sidebar(),
        KeyCode::Char('P') => {
            if app.team.pinned.as_deref() == Some(app.team.selected.as_str()) {
                app.team.pinned = None;
                let id = app.team.selected.clone();
                app.team.ensure_inspected_history(&id);
            } else {
                let id = app.team.selected.clone();
                app.team.pinned = Some(id.clone());
                app.team.ensure_inspected_history(&id);
            }
        }
        KeyCode::Char('m')
            if app.focus == Focus::Team || matches!(app.team.view, View::Inspect | View::Wall | View::Conversation) =>
        {
            if inspect_target != app.team.leader_id {
                app.direct_target = Some(inspect_target);
                app.focus = Focus::Composer;
            } else {
                app.direct_target = None;
                app.focus = Focus::Composer;
            }
        }
        KeyCode::Char('p')
            if app.focus == Focus::Team || matches!(app.team.view, View::Inspect | View::Wall | View::Conversation) =>
        {
            if let Some(row) = app.team.rows.iter().find(|r| r["id"] == inspect_target) {
                if row["lifecycle"].as_str() == Some("ACTIVE") {
                    app.confirm = Some(ConfirmAction::PauseMember(inspect_target));
                }
            }
        }
        KeyCode::Char('r')
            if app.focus == Focus::Team || matches!(app.team.view, View::Inspect | View::Wall | View::Conversation) =>
        {
            if let Some(row) = app.team.rows.iter().find(|r| r["id"] == inspect_target) {
                if matches!(row["lifecycle"].as_str(), Some("PAUSED" | "PARKED")) {
                    app.confirm = Some(ConfirmAction::ResumeMember(inspect_target));
                }
            }
        }
        KeyCode::Char('X')
            if app.focus == Focus::Team || matches!(app.team.view, View::Inspect | View::Wall | View::Conversation) =>
        {
            if let Some(row) = app.team.rows.iter().find(|r| r["id"] == inspect_target) {
                if row["lifecycle"].as_str() != Some("TERMINATED") {
                    app.confirm = Some(ConfirmAction::TerminateMember(inspect_target));
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') if app.focus == Focus::Team => app.team.move_selection(false),
        KeyCode::Down | KeyCode::Char('j') if app.focus == Focus::Team => app.team.move_selection(true),
        KeyCode::Home if app.focus == Focus::Team => {
            if let Some(id) = app.team.rows.first().and_then(|row| row["id"].as_str()).map(str::to_string) {
                app.team.select(&id);
            }
        }
        KeyCode::End if app.focus == Focus::Team => {
            if let Some(id) = app.team.rows.last().and_then(|row| row["id"].as_str()).map(str::to_string) {
                app.team.select(&id);
            }
        }
        KeyCode::Enter if app.focus == Focus::Team => {
            app.team.show(View::Inspect);
            app.focus = Focus::Panel;
        }
        KeyCode::Up | KeyCode::Char('k') if app.focus == Focus::Panel && app.team.view == View::Tasks => {
            app.team.selected_task = app.team.selected_task.saturating_sub(1);
            app.team.tasks_follow_selection = true;
        }
        KeyCode::Down | KeyCode::Char('j') if app.focus == Focus::Panel && app.team.view == View::Tasks => {
            let count = app.team.all_tasks.as_ref().unwrap_or(&app.team.tasks).len();
            app.team.selected_task = (app.team.selected_task + 1).min(count.saturating_sub(1));
            app.team.tasks_follow_selection = true;
        }
        KeyCode::Enter if app.focus == Focus::Panel && app.team.view == View::Tasks => {
            let assignee = app
                .team
                .all_tasks
                .as_ref()
                .unwrap_or(&app.team.tasks)
                .get(app.team.selected_task)
                .and_then(|t| t["assignee"].as_str())
                .map(str::to_string);
            if let Some(id) = assignee {
                app.team.select(&id);
                app.team.show(View::Inspect);
            }
        }
        KeyCode::Char('x') if app.focus == Focus::Panel && app.team.view == View::Tasks => {
            let task = app.team.all_tasks.as_ref().unwrap_or(&app.team.tasks).get(app.team.selected_task).cloned();
            if let Some(task) = task {
                let st = task["status"].as_str().unwrap_or("");
                if !matches!(st, "SUCCEEDED" | "FAILED" | "CANCELLED") {
                    if let Some(id) = task["id"].as_str() {
                        app.confirm = Some(ConfirmAction::CancelTask(id.to_string()));
                    }
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') if app.focus == Focus::Panel && app.team.view == View::Tree => {
            app.team.tree_selected = app.team.tree_selected.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') if app.focus == Focus::Panel && app.team.view == View::Tree => {
            let count = app.team.visible_tree_nodes().len();
            app.team.tree_selected = (app.team.tree_selected + 1).min(count.saturating_sub(1));
        }
        KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
            if app.focus == Focus::Panel && app.team.view == View::Tree =>
        {
            if let Some((_, node, has_children)) = app.team.visible_tree_nodes().get(app.team.tree_selected) {
                if *has_children {
                    if let Some(id) = node["id"].as_str().map(str::to_string) {
                        if key.code == KeyCode::Left {
                            app.team.tree_collapsed.insert(id);
                        } else if key.code == KeyCode::Right {
                            app.team.tree_collapsed.remove(&id);
                        } else if !app.team.tree_collapsed.remove(&id) {
                            app.team.tree_collapsed.insert(id);
                        }
                    }
                }
            }
        }
        KeyCode::Enter if app.focus == Focus::Panel && app.team.view == View::Tree => {
            let prompt = app.team.visible_tree_nodes().get(app.team.tree_selected).map(|(_, node, _)| {
                let id = node["id"].as_str().unwrap_or("?");
                let title = node["title"].as_str().unwrap_or("Untitled");
                format!("Focus on tree node {id} ({title}): ")
            });
            if let Some(prefix) = prompt {
                let current = app.composer.text();
                let next = if current.trim().is_empty() { prefix } else { format!("{prefix}{current}") };
                app.composer.set_text(&next);
                app.focus = Focus::Composer;
            }
        }
        KeyCode::Up | KeyCode::Left | KeyCode::Char('k')
            if app.focus == Focus::Panel && app.team.view == View::Wall =>
        {
            app.team.move_selection(false);
        }
        KeyCode::Down | KeyCode::Right | KeyCode::Char('j')
            if app.focus == Focus::Panel && app.team.view == View::Wall =>
        {
            app.team.move_selection(true);
        }
        KeyCode::Char(d @ '1'..='9') if app.focus == Focus::Panel && app.team.view == View::Wall => {
            let idx = (d as usize) - ('1' as usize);
            if let Some(id) = app.team.rows.get(idx).and_then(|r| r["id"].as_str()).map(str::to_string) {
                app.team.select(&id);
            }
        }
        KeyCode::Enter if app.focus == Focus::Panel && app.team.view == View::Wall => {
            if app.team.selected != app.team.leader_id {
                app.team.history_open = true;
            }
            app.team.show(View::Inspect);
        }
        KeyCode::Char('f') if app.team.view == View::Activity => {
            app.team.activity_filter_selected = !app.team.activity_filter_selected;
            app.team.scroll = 0;
        }
        KeyCode::Char('h') => {
            if inspect_target == app.team.leader_id {
                app.team.show(View::Conversation);
                app.focus = Focus::Composer;
            } else {
                app.team.ensure_inspected_history(&inspect_target);
                app.team.history_open = true;
                if app.team.view != View::Conversation {
                    app.team.show(View::Inspect);
                }
                app.focus = Focus::Panel;
            }
        }
        KeyCode::Char('d') => {
            app.team.history_open = false;
            if app.team.view != View::Conversation {
                app.team.show(View::Inspect);
            }
        }
        KeyCode::Up | KeyCode::Char('k') => app.team.scroll_by(false, 1),
        KeyCode::Down | KeyCode::Char('j') => app.team.scroll_by(true, 1),
        KeyCode::PageUp => app.team.scroll_by(false, app.team.height.max(1)),
        KeyCode::PageDown => app.team.scroll_by(true, app.team.height.max(1)),
        KeyCode::Home => app.team.scroll_edge(false),
        KeyCode::End => app.team.scroll_edge(true),
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
    match state {
        "PARKED" | "BLOCKED" | "FAILED" | "OUTCOME_UNKNOWN" => Style::default().fg(ERROR).add_modifier(Modifier::BOLD),
        "WAITING" | "PAUSED" | "APPROVAL" => Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
        "SUCCEEDED" => Style::default().fg(SUCCESS),
        "READY" | "TERMINATED" | "CANCELLED" => secondary(),
        _ => Style::default().fg(ACCENT),
    }
}

/// Shape, weight and semantic color badge for one team member row.
pub fn state_badge(row: &Json, tasks: &[Json]) -> (&'static str, Style) {
    let st = state(row);
    let running = row["lifecycle"].as_str() == Some("ACTIVE")
        && row["phase"].as_str() == Some("READY")
        && row["diagnosis"]["work"]["state"].as_str() == Some("RUNNING");
    if matches!(st, "PARKED" | "BLOCKED" | "FAILED" | "OUTCOME_UNKNOWN")
        || (row["diagnosis"]["attention"] == true && st != "APPROVAL")
    {
        return ("!", Style::default().fg(ERROR).add_modifier(Modifier::BOLD));
    }
    if st == "APPROVAL" {
        return ("◆", Style::default().fg(WARNING).add_modifier(Modifier::BOLD));
    }
    if running || matches!(st, "THINKING" | "TOOLS" | "CHECKING") {
        const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let glyph = SPINNER[(crate::now_epoch() * 8.0) as usize % SPINNER.len()];
        return (glyph, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));
    }
    match st {
        "WAITING" => ("◌", Style::default().fg(WARNING).add_modifier(Modifier::BOLD)),
        "PAUSED" => ("‖", Style::default().fg(WARNING).add_modifier(Modifier::BOLD)),
        "SUCCEEDED" => ("✓", Style::default().fg(SUCCESS)),
        "READY" => {
            if Team::member_attention(row, tasks) {
                ("!", Style::default().fg(ERROR).add_modifier(Modifier::BOLD))
            } else {
                ("○", secondary())
            }
        }
        "TERMINATED" => ("×", secondary()),
        _ => ("·", secondary()),
    }
}

fn task_glyph(status: &str) -> &'static str {
    match status {
        "BLOCKED" | "FAILED" | "OUTCOME_UNKNOWN" => "!",
        "SUCCEEDED" => "✓",
        "CANCELLED" => "×",
        "RUNNING" => "●",
        _ => "○",
    }
}

fn live_sessions_height(app: &V2App, inner_h: u16) -> u16 {
    if app.live_sessions.is_empty() || inner_h < 10 {
        0
    } else {
        1 + app.live_sessions.len().min(3) as u16
    }
}

pub fn live_sessions_window(app: &V2App, area: Rect) -> Option<Rect> {
    let inner = panel("").inner(area);
    let sessions_h = live_sessions_height(app, inner.height);
    if sessions_h == 0 {
        return None;
    }
    let y = inner.bottom().saturating_sub(sessions_h);
    Some(Rect::new(inner.x, y, inner.width, sessions_h))
}

/// Same member window used for painting and hit testing.
pub fn member_window(app: &V2App, area: Rect) -> (Rect, usize, usize) {
    let inner = panel("").inner(area);
    let sessions_h = live_sessions_height(app, inner.height);
    let list = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(sessions_h));
    let (start, count) = window(app.team.rows.len(), app.team.selected_index(), list.height as usize / 2);
    (list, start, count)
}

pub fn render_sidebar(frame: &mut Frame, app: &V2App, area: Rect) {
    if area.is_empty() {
        return;
    }
    let title = format!("Team · {}", app.team.rows.len());
    let block = panel(title).border_style(border(app.focus == Focus::Team));
    frame.render_widget(block, area);
    let (list, start, count) = member_window(app, area);
    let w = list.width as usize;
    let mut lines = vec![];
    for row in app.team.rows.iter().skip(start).take(count) {
        let id = row["id"].as_str().unwrap_or("?");
        let selected = id == app.team.selected;
        let label = if id == app.team.leader_id { "Leader" } else { id };
        let marker = if selected { "›" } else { " " };
        let (glyph, glyph_style) = state_badge(row, &app.team.tasks);
        let open_count = app.team.tasks.iter().filter(|task| task["assignee"].as_str() == Some(id)).count();
        let blocked_count = app
            .team
            .tasks
            .iter()
            .filter(|task| task["assignee"].as_str() == Some(id) && task["status"].as_str() == Some("BLOCKED"))
            .count();
        let badge = if blocked_count > 0 {
            format!(" !{blocked_count}")
        } else if open_count > 0 {
            format!(" {open_count}t")
        } else {
            String::new()
        };
        let show_badge = !badge.is_empty() && w >= 14 && w > 6 + badge.len();
        let name_max = if show_badge { w.saturating_sub(4 + badge.len()) } else { w.saturating_sub(4) };
        let fitted_label = fit(label, name_max);
        let st = state(row);
        if selected && app.focus == Focus::Team {
            let line_text = if show_badge {
                let used = 4 + unicode_width::UnicodeWidthStr::width(fitted_label.as_str());
                let pad = w.saturating_sub(used + badge.len());
                format!("{marker} {glyph} {fitted_label}{}{badge}", " ".repeat(pad))
            } else {
                fit(&format!("{marker} {glyph} {label}"), w)
            };
            lines.push(Line::styled(line_text, selection()));
        } else {
            let marker_style =
                if selected { Style::default().fg(ACCENT).add_modifier(Modifier::BOLD) } else { secondary() };
            let name_style = if selected {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else if st == "TERMINATED" {
                secondary()
            } else {
                Style::default().fg(FG)
            };
            let mut spans = vec![
                Span::styled(format!("{marker} "), marker_style),
                Span::styled(format!("{glyph} "), glyph_style),
                Span::styled(fitted_label.clone(), name_style),
            ];
            if show_badge {
                let used = 4 + unicode_width::UnicodeWidthStr::width(fitted_label.as_str());
                let pad = w.saturating_sub(used + badge.len());
                let badge_style = if blocked_count > 0 {
                    Style::default().fg(ERROR).add_modifier(Modifier::BOLD)
                } else {
                    secondary()
                };
                if pad > 0 {
                    spans.push(Span::raw(" ".repeat(pad)));
                }
                spans.push(Span::styled(badge, badge_style));
            }
            lines.push(Line::from(spans));
        }
        let elapsed = app.team.phase_elapsed(id).map(|s| format!(" {s}s")).unwrap_or_default();
        let state_label = format!("  {st}{elapsed}");
        let model_w = w.saturating_sub(state_label.len() + 3);
        lines.push(Line::from(vec![
            Span::styled(state_label, state_style(st)),
            Span::styled(
                format!(" · {}", fit(&app.member_model_label(id, row["model"].as_str().unwrap_or("")), model_w)),
                secondary(),
            ),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::styled(" Waiting for team snapshot", secondary()));
    }
    frame.render_widget(Paragraph::new(lines), list);
    if let Some(sess_rect) = live_sessions_window(app, area) {
        let mut s_lines = vec![Line::styled("SESSIONS", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))];
        for s in app.live_sessions.iter().take(sess_rect.height.saturating_sub(1) as usize) {
            let alerts = s.attention + s.approvals;
            let (badge, bstyle) = if alerts > 0 {
                ("!", Style::default().fg(ERROR).add_modifier(Modifier::BOLD))
            } else if s.busy > 0 {
                ("●", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
            } else if s.live {
                ("●", Style::default().fg(SUCCESS))
            } else {
                ("○", secondary())
            };
            let suffix = if alerts > 0 { format!(" !{alerts}") } else { String::new() };
            let name = fit(&s.name, (sess_rect.width as usize).saturating_sub(3 + suffix.len()));
            let mut spans = vec![
                Span::styled(format!("{badge} "), bstyle),
                Span::styled(name, if s.live { Style::default().fg(FG) } else { secondary() }),
            ];
            if !suffix.is_empty() {
                spans.push(Span::styled(suffix, Style::default().fg(ERROR).add_modifier(Modifier::BOLD)));
            }
            s_lines.push(Line::from(spans));
        }
        frame.render_widget(Paragraph::new(s_lines), sess_rect);
    }
}

pub fn nav_actions(app: &V2App, area: Rect, screen_width: u16) -> Vec<(Rect, NavAction, String, Style)> {
    if area.is_empty() {
        return vec![];
    }
    let tabs_width: u16 = 4 + VIEWS.iter().map(|(_, label)| label.len() as u16 + 2).sum::<u16>();
    let min_x = area.x.saturating_add(tabs_width + 2);
    let mut candidates = Vec::new();
    let alerts = app.team.attention_count();
    if alerts > 0 {
        candidates.push((
            NavAction::Alert,
            format!("[! {alerts}]"),
            Style::default().fg(ERROR).add_modifier(Modifier::BOLD),
        ));
    }
    candidates.push((
        NavAction::Zoom,
        if app.zoomed { "[Unzoom]".into() } else { "[Zoom]".into() },
        if app.zoomed { Style::default().fg(ACCENT).add_modifier(Modifier::BOLD) } else { secondary() },
    ));
    if screen_width >= 96 && !app.zoomed {
        candidates.push((
            NavAction::Sidebar,
            "[Team]".into(),
            if app.sidebar_hidden { Style::default().fg(ACCENT).add_modifier(Modifier::BOLD) } else { secondary() },
        ));
    }
    let mut out = Vec::new();
    let mut cursor = area.right().saturating_sub(1);
    for (action, label, style) in candidates.into_iter().rev() {
        let w = label.len() as u16;
        if cursor >= min_x.saturating_add(w) {
            let x = cursor - w;
            out.push((Rect::new(x, area.y, w, 1), action, label, style));
            cursor = x.saturating_sub(1);
        }
    }
    out.reverse();
    out
}

pub fn render_nav(frame: &mut Frame, app: &V2App, area: Rect, screen_width: u16) {
    if area.is_empty() {
        return;
    }
    let mut spans = vec![Span::styled("Alt ", secondary())];
    for (view, label) in VIEWS {
        if app.team.view == view {
            spans.push(Span::styled(format!(" {label} "), selection()));
        } else {
            let (badge, badge_style) = match view {
                View::Tasks if app.team.blocked_task_count() > 0 => {
                    ("!", Style::default().fg(ERROR).add_modifier(Modifier::BOLD))
                }
                View::Tasks if !app.team.tasks.is_empty() => ("•", Style::default().fg(ACCENT)),
                View::Activity if !app.team.activity.is_empty() => ("•", Style::default().fg(ACCENT)),
                View::Inspect if app.team.attention_count() > 0 => {
                    ("!", Style::default().fg(ERROR).add_modifier(Modifier::BOLD))
                }
                View::Tree if app.team.tree.as_ref().is_some_and(|t| t["stale"] == true) => {
                    ("!", Style::default().fg(WARNING).add_modifier(Modifier::BOLD))
                }
                View::Tree if app.team.tree.is_some() => ("•", Style::default().fg(ACCENT)),
                _ => (" ", secondary()),
            };
            spans.push(Span::styled(badge.to_string(), badge_style));
            spans.push(Span::styled(format!("{label} "), secondary()));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(PANEL_BG)), area);
    for (rect, _, label, style) in nav_actions(app, area, screen_width) {
        frame.render_widget(Paragraph::new(label).style(style.bg(PANEL_BG)), rect);
    }
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

pub fn inspector_tabs(area: Rect) -> Option<(Rect, Rect)> {
    let inner = panel("").inner(area);
    if inner.height < 4 || inner.width < 23 {
        return None;
    }
    Some((Rect::new(inner.x, inner.y, 11, 1), Rect::new(inner.x + 12, inner.y, 11, 1)))
}

pub fn inspector_actions(app: &V2App, area: Rect, right_inspector: bool) -> Vec<(Rect, InspectorVerb, &'static str)> {
    let inner = panel("").inner(area);
    if inner.height < 4 || inner.width < 46 {
        return vec![];
    }
    let target = app.team.inspected_id(right_inspector);
    let Some(row) = app.team.rows.iter().find(|r| r["id"] == target) else {
        return vec![];
    };
    let min_x = if target != app.team.leader_id { inner.x.saturating_add(24) } else { inner.x };
    let mut actions = Vec::new();
    if target != app.team.leader_id {
        actions.push((InspectorVerb::Message, "[m Msg]"));
    }
    match row["lifecycle"].as_str() {
        Some("ACTIVE") => actions.push((InspectorVerb::Pause, "[p Pause]")),
        Some("PAUSED" | "PARKED") => actions.push((InspectorVerb::Resume, "[r Resume]")),
        _ => {}
    }
    if row["lifecycle"].as_str() != Some("TERMINATED") && target != app.team.leader_id {
        actions.push((InspectorVerb::Terminate, "[X Term]"));
    }
    actions
        .push((InspectorVerb::Pin, if app.team.pinned.as_deref() == Some(target) { "[P Unpin]" } else { "[P Pin]" }));
    let mut out = Vec::new();
    let mut cursor = inner.right();
    for (verb, label) in actions.into_iter().rev() {
        let w = label.len() as u16;
        if cursor >= min_x.saturating_add(w) {
            let x = cursor - w;
            out.push((Rect::new(x, inner.y, w, 1), verb, label));
            cursor = x.saturating_sub(1);
        }
    }
    out.reverse();
    out
}

pub fn activity_filter_rect(area: Rect) -> Option<Rect> {
    let inner = panel("").inner(area);
    if inner.height < 3 || inner.width < 20 {
        return None;
    }
    Some(Rect::new(inner.x, inner.y, inner.width, 1))
}

fn format_wait_condition(condition: &Json) -> String {
    if let Some(s) = condition.as_str() {
        return s.to_string();
    }
    if let Some(obj) = condition.as_object() {
        match obj.get("kind").and_then(Json::as_str) {
            Some("task") => {
                return format!("task {}", obj.get("task_id").and_then(Json::as_str).unwrap_or("?"));
            }
            Some("message") => {
                return match obj.get("from").and_then(Json::as_str) {
                    Some(from) => format!("message from {from}"),
                    None => "message".into(),
                };
            }
            Some("operation") => {
                return format!("operation {}", obj.get("operation_id").and_then(Json::as_str).unwrap_or("?"));
            }
            Some("envelope") => {
                return format!("envelope {}", obj.get("envelope_id").and_then(Json::as_str).unwrap_or("?"));
            }
            Some("capability") => {
                return format!("capability {}", obj.get("request_id").and_then(Json::as_str).unwrap_or("?"));
            }
            _ => {
                let parts: Vec<String> = obj
                    .iter()
                    .map(|(k, v)| match v.as_str() {
                        Some(s) => format!("{k} {s}"),
                        None => format!("{k} {v}"),
                    })
                    .collect();
                if !parts.is_empty() {
                    return parts.join(" ");
                }
            }
        }
    }
    condition.to_string()
}

fn format_wait_conditions(conditions: &Json) -> String {
    if let Some(arr) = conditions.as_array() {
        if arr.is_empty() {
            return "none".into();
        }
        arr.iter().map(format_wait_condition).collect::<Vec<_>>().join(", ")
    } else if conditions.is_null() {
        "unavailable".into()
    } else {
        format_wait_condition(conditions)
    }
}

fn detail_styled_lines(app: &V2App, target_id: &str) -> Vec<Line<'static>> {
    let Some(row) = app.team.rows.iter().find(|row| row["id"] == target_id) else {
        return vec![Line::styled("No member snapshot available.", secondary())];
    };
    let diagnostic = &row["diagnosis"];
    let st = state(row);
    let (glyph, glyph_style) = state_badge(row, &app.team.tasks);
    let is_alert = Team::member_attention(row, &app.team.tasks);
    let elapsed = app.team.phase_elapsed(target_id).map(|s| format!(" · {s}s")).unwrap_or_default();
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{glyph} "), glyph_style),
        Span::styled(format!("{st}{elapsed}"), state_style(st)),
    ])];
    if target_id != app.team.leader_id {
        lines.push(Line::from(vec![
            Span::styled("Model: ", secondary()),
            Span::styled(
                format!(
                    "{} · {}",
                    app.member_model_label(target_id, row["model"].as_str().unwrap_or("")),
                    row["effort"].as_str().unwrap_or("default effort")
                ),
                Style::default().fg(FG),
            ),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        diagnostic["summary"].as_str().unwrap_or("No diagnosis reported by this daemon.").to_string(),
        Style::default().fg(FG),
    ));
    if let Some(lifetime) = row["lifetime"].as_str() {
        lines.push(Line::from(vec![
            Span::styled("Worker lifetime: ", secondary()),
            Span::styled(lifetime.to_string(), Style::default().fg(FG)),
        ]));
    }
    if let Some(can_shell) = row["can_run_shell"].as_bool() {
        lines.push(Line::from(vec![
            Span::styled("Shell: ", secondary()),
            Span::styled(
                if can_shell { "allowed" } else { "not granted" },
                if can_shell { Style::default().fg(SUCCESS) } else { secondary() },
            ),
        ]));
    }
    if let Some(reason) = diagnostic["reason"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| row["reason"].as_str().filter(|s| !s.is_empty()))
    {
        lines.push(Line::from(vec![
            Span::styled("Reason: ", secondary()),
            Span::styled(
                reason.to_string(),
                if is_alert { Style::default().fg(ERROR) } else { Style::default().fg(FG) },
            ),
        ]));
    }
    if let Some(ws) = diagnostic["work"]["state"].as_str() {
        lines.push(Line::from(vec![
            Span::styled("Work state: ", secondary()),
            Span::styled(ws.to_string(), state_style(ws)),
        ]));
    }
    if let Some(next) = diagnostic["next_steps"].as_str() {
        lines.push(Line::from(vec![
            Span::styled("Next: ", secondary()),
            Span::styled(next.to_string(), Style::default().fg(ACCENT)),
        ]));
    }
    for wait in diagnostic["waits"].as_array().into_iter().flatten() {
        lines.push(Line::from(vec![
            Span::styled("◌ ", Style::default().fg(WARNING).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(
                    "Wait {} · {} · timer {}",
                    wait["id"].as_str().unwrap_or("?"),
                    wait["mode"].as_str().unwrap_or("?"),
                    wait["timer_at"]
                ),
                Style::default().fg(FG),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Conditions: ", secondary()),
            Span::styled(format_wait_conditions(&wait["conditions"]), Style::default().fg(FG)),
        ]));
    }
    for operation in diagnostic["operations"].as_array().into_iter().flatten() {
        let op_status = operation["status"].as_str().unwrap_or("?");
        let mut spans = vec![
            Span::styled("Operation ", secondary()),
            Span::styled(format!("{}: ", operation["id"].as_str().unwrap_or("?")), Style::default().fg(FG)),
            Span::styled(op_status.to_string(), state_style(op_status)),
        ];
        if operation["cancel_requested"] == true {
            spans.push(Span::styled(
                " · cancellation requested",
                Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::default());
    lines.push(Line::styled("ASSIGNED OPEN TASKS", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)));
    let mut assigned = 0;
    for task in app.team.tasks.iter().filter(|task| task["assignee"] == target_id) {
        assigned += 1;
        let tst = task["status"].as_str().unwrap_or("?");
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", task_glyph(tst)), state_style(tst)),
            Span::styled(
                task["id"].as_str().unwrap_or("?").to_string(),
                Style::default().fg(FG).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", secondary()),
            Span::styled(tst.to_string(), state_style(tst)),
        ]));
        if let Some(reason) = task["diagnosis"]["reason"].as_str() {
            lines.push(Line::styled(
                format!("  {reason}"),
                if tst == "BLOCKED" { Style::default().fg(ERROR) } else { secondary() },
            ));
        }
    }
    if assigned == 0 {
        lines.push(Line::styled(
            if app.team.tasks_known { "None at this checkpoint." } else { "No open-task snapshot reported." },
            secondary(),
        ));
    }
    lines
}

fn render_tasks_panel(frame: &mut Frame, app: &mut V2App, area: Rect) {
    app.team.task_row_ranges.clear();
    if area.is_empty() {
        app.team.tasks_visible = false;
        app.team.tasks_list_rect = Rect::default();
        app.team.lines = 0;
        app.team.height = 1;
        return;
    }
    app.team.tasks_visible = true;
    let has_bar = area.width > 2;
    let content_w = if has_bar { area.width - 1 } else { area.width };
    app.team.tasks_list_rect = Rect::new(area.x, area.y, content_w, area.height);
    let mut header = vec![Line::styled("Task outcome is not process-stop evidence.", secondary())];
    if let Some(error) = &app.team.tasks_error {
        header.push(Line::styled(format!("Refresh failed; rows may be stale: {error}"), Style::default().fg(ERROR)));
    }
    let tasks = app.team.all_tasks.as_ref().unwrap_or(&app.team.tasks);
    if app.team.all_tasks.is_none() {
        header.push(Line::styled("Loading all tasks; showing open checkpoint rows.", secondary()));
    }
    if tasks.is_empty() {
        header.push(Line::default());
        header.push(Line::styled("No tasks reported.", secondary()));
        let wrapped: Vec<Line<'static>> =
            header.iter().flat_map(|line| crate::wrap::wrap_line(line, content_w as usize)).collect();
        render_wrapped_lines(frame, app, area, wrapped);
        return;
    }
    let selected_task = app.team.selected_task.min(tasks.len().saturating_sub(1));
    let blocked = tasks.iter().filter(|t| t["status"].as_str() == Some("BLOCKED")).count();
    let running = tasks.iter().filter(|t| t["status"].as_str() == Some("RUNNING")).count();
    let succeeded = tasks.iter().filter(|t| t["status"].as_str() == Some("SUCCEEDED")).count();
    header.push(Line::from(vec![
        Span::styled(format!("{} total", tasks.len()), Style::default().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(" · ", secondary()),
        Span::styled(
            format!("{blocked} blocked"),
            if blocked > 0 { Style::default().fg(ERROR).add_modifier(Modifier::BOLD) } else { secondary() },
        ),
        Span::styled(" · ", secondary()),
        Span::styled(format!("{running} running"), if running > 0 { Style::default().fg(ACCENT) } else { secondary() }),
        Span::styled(" · ", secondary()),
        Span::styled(
            format!("{succeeded} succeeded"),
            if succeeded > 0 { Style::default().fg(SUCCESS) } else { secondary() },
        ),
    ]));
    header.push(Line::default());
    let mut wrapped: Vec<Line<'static>> =
        header.iter().flat_map(|line| crate::wrap::wrap_line(line, content_w as usize)).collect();
    for (idx, task) in tasks.iter().enumerate() {
        let tst = task["status"].as_str().unwrap_or("?");
        let id = task["id"].as_str().unwrap_or("?");
        let assignee = task["assignee"].as_str().unwrap_or("?");
        let selected = idx == selected_task && app.focus == Focus::Panel;
        let mut group = Vec::new();
        if selected {
            group.push(Line::styled(format!("> {} {tst}  {id}  → {assignee}", task_glyph(tst)), selection()));
        } else {
            group.push(Line::from(vec![
                Span::styled(format!("  {} ", task_glyph(tst)), state_style(tst)),
                Span::styled(format!("{tst}  "), state_style(tst)),
                Span::styled(id.to_string(), Style::default().fg(FG).add_modifier(Modifier::BOLD)),
                Span::styled("  → ", secondary()),
                Span::styled(assignee.to_string(), Style::default().fg(ACCENT)),
            ]));
        }
        if let Some(summary) = task["diagnosis"]["summary"].as_str() {
            group.push(Line::styled(format!("    {summary}"), Style::default().fg(FG)));
        }
        if let Some(reason) = task["diagnosis"]["reason"].as_str() {
            group.push(Line::styled(
                format!("    Reason: {reason}"),
                if tst == "BLOCKED" { Style::default().fg(ERROR) } else { secondary() },
            ));
        }
        let start = wrapped.len();
        for line in &group {
            wrapped.extend(crate::wrap::wrap_line(line, content_w as usize));
        }
        let end = wrapped.len();
        if end > start {
            app.team.task_row_ranges.push((idx, start..end));
        }
        wrapped.push(Line::default());
    }
    if app.team.tasks_follow_selection {
        let height = area.height as usize;
        if let Some((_, range)) = app.team.task_row_ranges.iter().find(|(ix, _)| *ix == selected_task) {
            if range.start < app.team.scroll {
                app.team.scroll = if selected_task == 0 && range.end <= height { 0 } else { range.start };
            } else if range.end > app.team.scroll + height {
                app.team.scroll = range.end.saturating_sub(height).min(range.start);
            }
        }
        app.team.tasks_follow_selection = false;
    }
    render_wrapped_lines(frame, app, area, wrapped);
}

fn activity_styled_lines(app: &V2App) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled("Newest 200 observed events. Reconnect gaps are possible; export for a full audit.", secondary()),
        Line::default(),
    ];
    let mut shown = 0usize;
    for raw in &app.team.activity {
        let parts: Vec<&str> = raw.splitn(3, "  ").collect();
        if parts.len() == 3 {
            let seq = parts[0];
            let scope = parts[1];
            let rest = parts[2];
            if app.team.activity_filter_selected && scope != app.team.selected {
                continue;
            }
            shown += 1;
            let scope_style = Style::default().fg(if scope == app.team.leader_id { CODEX } else { ACCENT });
            let (kind, detail) = rest.split_once(" · ").map_or((rest, ""), |(k, d)| (k, d));
            let kind_style = if kind.contains("blocked")
                || kind.contains("failed")
                || kind.contains("cancelled")
                || kind.contains("denied")
                || detail.contains("OUTCOME_UNKNOWN")
            {
                Style::default().fg(ERROR).add_modifier(Modifier::BOLD)
            } else if kind.contains("completed")
                || kind.contains("succeeded")
                || kind.contains("approved")
                || kind.contains("satisfied")
            {
                Style::default().fg(SUCCESS)
            } else {
                Style::default().fg(FG)
            };
            let mut spans = vec![
                Span::styled(format!("{seq}  "), secondary()),
                Span::styled(format!("{scope}  "), scope_style),
                Span::styled(kind.to_string(), kind_style),
            ];
            if !detail.is_empty() {
                let detail_style =
                    if detail.contains("OUTCOME_UNKNOWN") { Style::default().fg(ERROR) } else { secondary() };
                spans.push(Span::styled(" · ", secondary()));
                spans.push(Span::styled(detail.to_string(), detail_style));
            }
            lines.push(Line::from(spans));
        } else {
            if app.team.activity_filter_selected && !raw.contains(&app.team.selected) {
                continue;
            }
            shown += 1;
            lines.push(Line::styled(raw.clone(), Style::default().fg(FG)));
        }
    }
    if shown == 0 {
        lines.push(Line::styled(
            if app.team.activity_filter_selected && !app.team.activity.is_empty() {
                format!("No observed events for {}. Press f to show all.", app.team.selected)
            } else {
                "No events observed since attachment.".into()
            },
            secondary(),
        ));
    }
    lines
}

fn render_styled_lines(frame: &mut Frame, app: &mut V2App, area: Rect, lines: Vec<Line<'static>>) {
    if area.is_empty() {
        app.team.lines = 0;
        app.team.height = 1;
        return;
    }
    let has_bar = area.width > 2;
    let content_w = if has_bar { area.width - 1 } else { area.width };
    let wrapped: Vec<Line<'static>> =
        lines.iter().flat_map(|line| crate::wrap::wrap_line(line, content_w as usize)).collect();
    render_wrapped_lines(frame, app, area, wrapped);
}

fn render_wrapped_lines(frame: &mut Frame, app: &mut V2App, area: Rect, wrapped: Vec<Line<'static>>) {
    if area.is_empty() {
        app.team.lines = 0;
        app.team.height = 1;
        return;
    }
    let has_bar = area.width > 2;
    let content_w = if has_bar { area.width - 1 } else { area.width };
    app.team.lines = wrapped.len();
    app.team.height = area.height.max(1) as usize;
    let max_scroll = wrapped.len().saturating_sub(area.height as usize);
    app.team.scroll = app.team.scroll.min(max_scroll);
    let visible: Vec<_> = wrapped.into_iter().skip(app.team.scroll).take(area.height as usize).collect();
    frame.render_widget(Paragraph::new(visible), Rect::new(area.x, area.y, content_w, area.height));
    if has_bar && app.team.lines > area.height as usize {
        let mut state = ScrollbarState::new(max_scroll + 1).position(app.team.scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(Style::default().fg(ACCENT))
                .thumb_symbol("▏")
                .track_symbol(None),
            Rect::new(area.x + content_w, area.y, 1, area.height),
            &mut state,
        );
    }
}

pub fn wall_cards(app: &V2App, area: Rect) -> Vec<(Rect, String)> {
    let inner = panel("").inner(area);
    if inner.height < 6 || inner.width < 12 || app.team.rows.is_empty() {
        return vec![];
    }
    let body = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
    let cols: u16 = if body.width >= 76 && app.team.rows.len() > 1 { 2 } else { 1 };
    let visible_rows = (body.height / 5).max(1) as usize;
    let max_cards = visible_rows * cols as usize;
    let total = app.team.rows.len();
    let selected_idx = app.team.selected_index();
    let start = if total <= max_cards {
        0
    } else {
        (selected_idx / cols as usize)
            .saturating_sub(visible_rows.saturating_sub(1))
            .min(total.div_ceil(cols as usize).saturating_sub(visible_rows))
            * cols as usize
    };
    let shown = total.saturating_sub(start).min(max_cards);
    let rows_used = shown.div_ceil(cols as usize).max(1) as u16;
    let card_h = (body.height / rows_used).max(5);
    let col_w = body.width / cols;
    let mut out = Vec::new();
    for offset in 0..shown {
        let r = (offset / cols as usize) as u16;
        let c = (offset % cols as usize) as u16;
        let x = body.x + c * col_w;
        let w = if c + 1 == cols { body.right().saturating_sub(x) } else { col_w };
        let y = body.y + r * card_h;
        let h = if r + 1 == rows_used { body.bottom().saturating_sub(y) } else { card_h };
        if h >= 3 && w >= 6 {
            if let Some(id) = app.team.rows[start + offset]["id"].as_str() {
                out.push((Rect::new(x, y, w, h), id.to_string()));
            }
        }
    }
    out
}

fn render_wall(frame: &mut Frame, app: &V2App, area: Rect) {
    let inner = panel("").inner(area);
    if inner.is_empty() {
        return;
    }
    frame.render_widget(
        Paragraph::new(fit(
            "Live team monitor · 1-9/Arrows select · Enter inspect · m direct message",
            inner.width as usize,
        ))
        .style(secondary()),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    for (idx, (rect, id)) in wall_cards(app, area).into_iter().enumerate() {
        let Some(row) = app.team.rows.iter().find(|r| r["id"].as_str() == Some(&id)) else { continue };
        let selected = id == app.team.selected;
        let (glyph, glyph_style) = state_badge(row, &app.team.tasks);
        let st = state(row);
        let elapsed = app.team.phase_elapsed(&id).map(|s| format!(" {s}s")).unwrap_or_default();
        let label = if id == app.team.leader_id { "Leader" } else { id.as_str() };
        let title = format!(" {} {glyph} {label} · {st}{elapsed} ", idx + 1);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(if selected {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else if Team::member_attention(row, &app.team.tasks) {
                Style::default().fg(ERROR)
            } else {
                border(false)
            })
            .title(Span::styled(
                fit(&title, rect.width.saturating_sub(2) as usize),
                if selected { Style::default().fg(ACCENT).add_modifier(Modifier::BOLD) } else { glyph_style },
            ));
        let card_inner = block.inner(rect);
        frame.render_widget(block, rect);
        if card_inner.is_empty() {
            continue;
        }
        let model = app.member_model_label(&id, row["model"].as_str().unwrap_or(""));
        let mut lines = vec![Line::styled(fit(&model, card_inner.width as usize), secondary())];
        let body_h = card_inner.height.saturating_sub(1) as usize;
        if body_h > 0 {
            if let Some((_, preview)) = app.previews.iter().find(|(pid, _)| pid == &id) {
                let wrapped = crate::markdown::plain(preview, card_inner.width as usize, Style::default().fg(FG));
                let skip = wrapped.len().saturating_sub(body_h);
                lines.extend(wrapped.into_iter().skip(skip));
            } else if let Some(reason) = row["diagnosis"]["reason"].as_str().filter(|s| !s.is_empty()) {
                let style =
                    if Team::member_attention(row, &app.team.tasks) { Style::default().fg(ERROR) } else { secondary() };
                lines.extend(crate::markdown::plain(reason, card_inner.width as usize, style).into_iter().take(body_h));
            } else if let Some(summary) = row["diagnosis"]["summary"].as_str().filter(|s| !s.is_empty()) {
                lines.extend(
                    crate::markdown::plain(summary, card_inner.width as usize, secondary()).into_iter().take(body_h),
                );
            }
        }
        frame.render_widget(Paragraph::new(lines), card_inner);
    }
}

pub fn tree_list_rect(area: Rect) -> Option<Rect> {
    let inner = panel("").inner(area);
    if inner.height < 4 || inner.width < 12 {
        return None;
    }
    Some(Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2))
}

fn render_tree(frame: &mut Frame, app: &mut V2App, area: Rect) {
    let inner = panel("").inner(area);
    app.team.tree_visible = !inner.is_empty();
    if inner.is_empty() {
        return;
    }
    if let Some(error) = &app.team.tree_error {
        frame.render_widget(
            Paragraph::new(fit(&format!("Tree read failed (retrying): {error}"), inner.width as usize))
                .style(Style::default().fg(ERROR)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }
    let Some(tree) = &app.team.tree else {
        frame.render_widget(
            Paragraph::new("Loading session exploration tree…").style(secondary()),
            Rect::new(
                inner.x,
                inner.y.saturating_add(1).min(inner.bottom()),
                inner.width,
                inner.height.saturating_sub(1),
            ),
        );
        return;
    };
    let rev = tree["revision"].as_i64().unwrap_or(0);
    let stale = if tree["stale"] == true { " · stale" } else { " · current" };
    let header = format!("Revision {rev}{stale}");
    frame.render_widget(
        Paragraph::new(fit(&header, inner.width as usize)).style(secondary()),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    if inner.height >= 2 {
        frame.render_widget(
            Paragraph::new("─".repeat(inner.width as usize)).style(secondary()),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
    }
    let Some(list_area) = tree_list_rect(area) else { return };
    let nodes = app.team.visible_tree_nodes();
    if nodes.is_empty() {
        frame.render_widget(
            Paragraph::new("No exploration-tree nodes recorded yet. Ask the Leader to structure findings.")
                .style(secondary()),
            list_area,
        );
        return;
    }
    let selected_ix = app.team.tree_selected.min(nodes.len().saturating_sub(1));
    let focus_id = tree["focus"].as_str();
    let (start, count) = window(nodes.len(), selected_ix, list_area.height as usize);
    let w = list_area.width.saturating_sub(1) as usize;
    let lines: Vec<Line> = nodes[start..start + count]
        .iter()
        .enumerate()
        .map(|(offset, (depth, node, has_children))| {
            let idx = start + offset;
            let selected = idx == selected_ix;
            let id = node["id"].as_str().unwrap_or("?");
            let fold = if !*has_children {
                "·"
            } else if app.team.tree_collapsed.contains(id) {
                "▸"
            } else {
                "▾"
            };
            let focus = if focus_id == Some(id) { "→ " } else { "" };
            let status = node["status"].as_str().unwrap_or("open");
            let title = node["title"].as_str().unwrap_or("Untitled");
            let indent = "  ".repeat((*depth).min(8));
            let text = format!("{}{indent}{fold} {focus}[{status}] {title} ({id})", if selected { ">" } else { " " });
            if selected {
                Line::styled(fit(&text, w), selection())
            } else {
                Line::from(vec![
                    Span::styled(format!(" {indent}{fold} "), secondary()),
                    Span::styled(focus.to_string(), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
                    Span::styled(
                        format!("[{status}] "),
                        match status {
                            "abandoned" | "blocked" => Style::default().fg(ERROR),
                            "resolved" | "decided" => Style::default().fg(SUCCESS),
                            _ => Style::default().fg(ACCENT),
                        },
                    ),
                    Span::styled(
                        fit(&format!("{title} ({id})"), w.saturating_sub(depth * 2 + 16)),
                        Style::default().fg(FG),
                    ),
                ])
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list_area);
    let total = nodes.len();
    let height = list_area.height as usize;
    if total > height && list_area.width > 1 {
        let max_scroll = total.saturating_sub(height);
        let mut state = ScrollbarState::new(max_scroll + 1).position(start.min(max_scroll));
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(Style::default().fg(ACCENT))
                .thumb_symbol("▏")
                .track_symbol(None),
            list_area,
            &mut state,
        );
    }
}

pub fn render_panel(frame: &mut Frame, app: &mut V2App, area: Rect, inspect: bool) {
    if area.is_empty() {
        return;
    }
    let right_inspector = inspect && app.team.view == View::Conversation;
    let target_id = app.team.inspected_id(right_inspector).to_string();
    let pin_tag = if right_inspector && app.team.pinned.is_some() { " [Pinned]" } else { "" };
    let title = if inspect {
        format!("{target_id}{pin_tag}")
    } else if app.team.view == View::Tasks {
        "Tasks".into()
    } else if app.team.view == View::Tree {
        "Exploration tree".into()
    } else if app.team.view == View::Wall {
        "Team wall".into()
    } else {
        "Activity".into()
    };
    let block = panel(title).border_style(border(app.focus == Focus::Panel));
    let mut inner = block.inner(area);
    frame.render_widget(block, area);
    if !inspect && app.team.view == View::Wall {
        render_wall(frame, app, area);
        return;
    }
    if !inspect && app.team.view == View::Tree {
        render_tree(frame, app, area);
        return;
    }
    if inspect {
        let actions = inspector_actions(app, area, right_inspector);
        let mut used_header = false;
        if target_id != app.team.leader_id {
            if let Some((details_rect, history_rect)) = inspector_tabs(area) {
                frame.render_widget(
                    Paragraph::new(" d Details ").style(if !app.team.history_open { selection() } else { secondary() }),
                    details_rect,
                );
                frame.render_widget(
                    Paragraph::new(" h History ").style(if app.team.history_open { selection() } else { secondary() }),
                    history_rect,
                );
                used_header = true;
            }
        }
        for (rect, verb, label) in actions {
            let style = match verb {
                InspectorVerb::Terminate => Style::default().fg(ERROR),
                InspectorVerb::Message | InspectorVerb::Pin => Style::default().fg(ACCENT),
                _ => secondary(),
            };
            frame.render_widget(Paragraph::new(label).style(style), rect);
            used_header = true;
        }
        if used_header {
            inner = Rect::new(inner.x, inner.y.saturating_add(1), inner.width, inner.height.saturating_sub(1));
        }
    }
    if inspect && app.team.history_open && target_id != app.team.leader_id {
        app.team.ensure_inspected_history(&target_id);
        render_history(frame, app, inner, &target_id);
        return;
    }
    if !inspect && app.team.view == View::Activity {
        if let Some(filter_rect) = activity_filter_rect(area) {
            let (text, style) = if app.team.activity_filter_selected {
                (
                    format!("Filter: {} only (f)", app.team.selected),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )
            } else {
                ("Filter: All members (f)".into(), secondary())
            };
            frame.render_widget(Paragraph::new(fit(&text, filter_rect.width as usize)).style(style), filter_rect);
            inner = Rect::new(inner.x, inner.y.saturating_add(1), inner.width, inner.height.saturating_sub(1));
        }
    }
    if !inspect && app.team.view == View::Tasks {
        render_tasks_panel(frame, app, inner);
        return;
    }
    let lines = if inspect { detail_styled_lines(app, &target_id) } else { activity_styled_lines(app) };
    render_styled_lines(frame, app, inner, lines);
}

fn render_history(frame: &mut Frame, app: &mut V2App, area: Rect, target_id: &str) {
    let Some(member) = &mut app.team.member else { return };
    if member.id != target_id {
        return;
    }
    app.team.member_visible = true;
    let note = if let Some(error) = &member.error {
        Some(format!("Stale history · retrying: {error}"))
    } else if member.dirty && member.entries.is_empty() {
        Some("Loading member history…".into())
    } else if member.older {
        Some("Newest 400 entries · older history in /sessions".into())
    } else {
        None
    };
    let area = if let Some(note) = note {
        frame.render_widget(
            Paragraph::new(fit(&note, area.width as usize)).style(secondary()),
            Rect::new(area.x, area.y, area.width, area.height.min(1)),
        );
        Rect::new(
            area.x,
            area.y.saturating_add(1).min(area.bottom()),
            area.width.saturating_sub(1),
            area.height.saturating_sub(1),
        )
    } else {
        Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height)
    };
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
    let height = area.height as usize;
    if total > height && area.width > 0 {
        let max_scroll = total.saturating_sub(height);
        let mut state = ScrollbarState::new(max_scroll + 1).position(start);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(Style::default().fg(ACCENT))
                .thumb_symbol("▏")
                .track_symbol(None),
            Rect::new(area.right(), area.y, 1, area.height),
            &mut state,
        );
    }
}
