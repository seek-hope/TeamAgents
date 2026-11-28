//! Deterministic TUI state machine, complementary key/slash bindings, and exact `HitMap` dispatch.
use crate::{
    cli::{format_provider_list_lines, parse_provider_add_tokens},
    daemon,
    model::CustomProviderSpec,
    protocol::{
        preview, sanitize_inline, Member, ModelConfig, ProviderSummary, QuestionTracker, StageKind, TaskRecord,
        TaskStatus, TrackerNotice,
    },
    sandbox::SandboxMode,
    tui::{
        input::{active_at_mention, matching_commands, matching_files, TextInput},
        theme::Palette,
    },
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_EVENTS: usize = 1000;
const TOAST_TICKS: u64 = 50;
pub(crate) const DELETE_ARM_TICKS: u64 = 50;
pub(crate) const EFFORT_OPTIONS: &[&str] = &["default", "none", "minimal", "low", "medium", "high", "xhigh", "max"];
pub(crate) const WIDE_THRESHOLD: u16 = 96;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    Composer,
    Sidebar,
    Inspector,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidebarTab {
    Trees,
    Members,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InspectorTab {
    Summary,
    Evidence,
    Tree,
    Events,
    Board,
}

impl InspectorTab {
    pub fn from_digit(d: char) -> Option<Self> {
        match d {
            '1' => Some(Self::Summary),
            '2' => Some(Self::Evidence),
            '3' => Some(Self::Tree),
            '4' => Some(Self::Events),
            '5' => Some(Self::Board),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Summary => Self::Evidence,
            Self::Evidence => Self::Tree,
            Self::Tree => Self::Events,
            Self::Events => Self::Board,
            Self::Board => Self::Summary,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Summary => Self::Board,
            Self::Evidence => Self::Summary,
            Self::Tree => Self::Evidence,
            Self::Events => Self::Tree,
            Self::Board => Self::Events,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelField {
    ProviderBar,
    ModelInput,
    CatalogList,
    EffortBar,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CatalogState {
    Idle,
    Loading,
    Loaded(Vec<String>),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Overlay {
    None,
    Approvals {
        selected: usize,
    },
    Model {
        providers: Vec<ProviderSummary>,
        provider_idx: usize,
        initial_provider: String,
        model_input: TextInput,
        effort_idx: usize,
        catalog: CatalogState,
        selected_catalog: Option<usize>,
        field: ModelField,
    },
    Check {
        input: TextInput,
    },
    Agents {
        selected: usize,
    },
    TextView {
        title: String,
        lines: Vec<String>,
        scroll: u16,
        is_diff: bool,
    },
    Help,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TreeRow {
    pub task_id: String,
    pub root_id: String,
    pub prefix: String,
    pub is_root: bool,
    pub has_children: bool,
    pub collapsed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArmedDelete {
    pub root_id: String,
    pub label: String,
    pub expires_at_tick: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UiCommand {
    RefreshAll,
    FetchDetail(String),
    Submit { agent: String, text: String, review: bool, sandbox: SandboxMode, check: Option<String> },
    Continue { id: String, text: String, review: bool, sandbox: SandboxMode, check: Option<String> },
    Review { agent: String, review: bool, base: Option<String>, check: Option<String> },
    Steer { id: String, text: String },
    Compact(String),
    LoadDiff { base: Option<String> },
    LoadSkills,
    LoadMcp,
    WbWrite { root_id: String, key: String, mode: String, kind: Option<String>, content: Option<String> },
    Answer { root_id: String, text: String },
    Approve { approval_id: String, allow: bool },
    Cancel(String),
    Delete(String),
    SetModel { provider: Option<Option<String>>, model: Option<Option<String>>, effort: Option<Option<String>> },
    LoadModels(Option<String>),
    AddProvider { name: String, spec: CustomProviderSpec, set_default: bool },
    RemoveProvider(String),
    SetDefaultProvider(Option<String>),
    StopDaemon,
    Quit,
}

#[derive(Debug)]
pub(crate) enum BackendEvent {
    Snapshot {
        tasks: std::result::Result<Vec<TaskRecord>, String>,
        members: std::result::Result<Vec<Member>, String>,
        model: std::result::Result<ModelConfig, String>,
        providers: Option<(String, bool, Option<String>, Vec<ProviderSummary>)>,
    },
    TaskDetail(std::result::Result<Box<TaskRecord>, String>),
    ModelsCatalog {
        provider: Option<String>,
        result: std::result::Result<Vec<String>, String>,
    },
    FilesLoaded(Vec<String>),
    TextOverlay {
        title: String,
        lines: Vec<String>,
        is_diff: bool,
    },
    CommandOutcome {
        message: Option<String>,
        error: Option<String>,
        select_task: Option<String>,
        refresh: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EventLevel {
    Info,
    Success,
    Attention,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EventEntry {
    pub timestamp: String,
    pub level: EventLevel,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Toast {
    pub text: String,
    pub is_error: bool,
    pub expires_at_tick: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContinuationTarget {
    pub root_id: String,
    pub node_id: String,
    pub label: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HitTarget {
    SidebarPane,
    InspectorPane,
    ComposerPane { inner: Rect, scroll: u16 },
    NewSession,
    SidebarToggle,
    SidebarTab(SidebarTab),
    SidebarSessionRow { task_id: String, root_id: String, toggle_fold: bool },
    SidebarMemberRow(usize),
    InspectorTab(InspectorTab),
    AlertApprovals,
    AlertClarification(String),
    InlineApprove(String),
    InlineDeny(String),
    InlineReply(String),
    OpenHelp,
    SlashItem(usize),
    MentionItem(usize),
    ModalBackdrop,
    ModalSurface,
    ModalClose,
    ModalApprovalRow(usize),
    ModalApprove(String),
    ModalDeny(String),
    ModalModelField(ModelField),
    ModalProviderOption(usize),
    ModalEffortOption(usize),
    ModalCatalogRow(usize),
    ModalModelSave,
    ModalCheckSave,
    ModalAgentRow(usize),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct HitMap {
    pub viewport: Rect,
    pub entries: Vec<(Rect, HitTarget)>,
}

impl HitMap {
    pub fn push(&mut self, rect: Rect, target: HitTarget) {
        if rect.width > 0 && rect.height > 0 {
            self.entries.push((rect, target));
        }
    }

    pub fn hit_test(&self, col: u16, row: u16) -> Option<&HitTarget> {
        self.entries.iter().rev().find(|(r, _)| contains(*r, col, row)).map(|(_, t)| t)
    }

    pub fn scroll_target(&self, col: u16, row: u16) -> Option<&HitTarget> {
        self.entries
            .iter()
            .rev()
            .filter(|(_, t)| matches!(t, HitTarget::SidebarPane | HitTarget::InspectorPane | HitTarget::ModalSurface))
            .find(|(r, _)| contains(*r, col, row))
            .map(|(_, t)| t)
    }
}

fn contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

pub(crate) struct AppState {
    pub workspace_name: String,
    pub agent: String,
    pub review_mode: bool,
    pub sandbox_mode: SandboxMode,
    pub armed_check: Option<String>,
    pub model: Option<ModelConfig>,
    pub providers: Vec<ProviderSummary>,
    pub providers_overridden: bool,
    pub providers_config_path: Option<String>,
    pub palette: Palette,
    pub connected: bool,
    pub last_error: Option<String>,
    pub tasks: Vec<TaskRecord>,
    pub members: Vec<Member>,
    pub workspace_files: Vec<String>,
    pub collapsed_roots: HashSet<String>,
    pub tree_rows: Vec<TreeRow>,
    pub selected_task_id: Option<String>,
    pub selected_detail: Option<TaskRecord>,
    pub selected_member_idx: usize,
    pub pending_select_task: Option<String>,
    pub sidebar_visible: bool,
    pub sidebar_scroll: usize,
    pub sidebar_max_scroll: usize,
    pub sidebar_follow_selection: bool,
    pub last_width: u16,
    pub inspector_tab: InspectorTab,
    pub inspector_scroll: u16,
    pub inspector_max_scroll: u16,
    pub focus: Focus,
    pub sidebar_tab: SidebarTab,
    pub overlay: Overlay,
    pub input: TextInput,
    pub slash_idx: usize,
    pub mention_idx: usize,
    pub answering_root: Option<String>,
    pub armed_delete: Option<ArmedDelete>,
    pub questions: QuestionTracker,
    pub events: VecDeque<EventEntry>,
    pub toast: Option<Toast>,
    pub tick: u64,
    pub should_quit: bool,
}

impl AppState {
    pub fn new(workspace_name: String, agent: String, review_mode: bool, sandbox_mode: SandboxMode) -> Self {
        let mut state = Self {
            workspace_name,
            agent: agent.clone(),
            review_mode,
            sandbox_mode,
            armed_check: None,
            model: None,
            providers: Vec::new(),
            providers_overridden: false,
            providers_config_path: None,
            palette: Palette::default_for_env(),
            connected: false,
            last_error: None,
            tasks: Vec::new(),
            members: Vec::new(),
            workspace_files: Vec::new(),
            collapsed_roots: HashSet::new(),
            tree_rows: Vec::new(),
            selected_task_id: None,
            selected_detail: None,
            selected_member_idx: 0,
            pending_select_task: None,
            sidebar_visible: true,
            sidebar_scroll: 0,
            sidebar_max_scroll: 0,
            sidebar_follow_selection: true,
            last_width: 120,
            inspector_tab: InspectorTab::Summary,
            inspector_scroll: 0,
            inspector_max_scroll: 0,
            focus: Focus::Composer,
            sidebar_tab: SidebarTab::Trees,
            overlay: Overlay::None,
            input: TextInput::default(),
            slash_idx: 0,
            mention_idx: 0,
            answering_root: None,
            armed_delete: None,
            questions: QuestionTracker::default(),
            events: VecDeque::new(),
            toast: None,
            tick: 0,
            should_quit: false,
        };
        let mode = if review_mode { "review" } else { "unattended" };
        state.push_event(EventLevel::Info, format!("Session ready with @{agent} ({mode}, {}).", sandbox_mode.as_str()));
        state
    }

    pub fn active_mention(&self) -> Option<(usize, usize, &str, Vec<&str>)> {
        if self.workspace_files.is_empty() {
            return None;
        }
        let (start, end, query) = active_at_mention(self.input.as_str(), self.input.cursor())?;
        let matches = matching_files(query, &self.workspace_files, 9);
        if matches.is_empty() {
            return None;
        }
        Some((start, end, query, matches))
    }

    fn accept_mention_at(&mut self, idx: usize) -> bool {
        let Some((start, end, _, matches)) = self.active_mention() else {
            return false;
        };
        let Some(picked) = matches.get(idx % matches.len()).copied().map(str::to_owned) else {
            return false;
        };
        self.input.replace_range(start, end, &format!("@{picked} "));
        self.mention_idx = 0;
        true
    }

    pub fn has_animations(&self) -> bool {
        self.toast.is_some()
            || self.armed_delete.is_some()
            || self.tasks.iter().any(|t| t.status == TaskStatus::Running)
    }

    pub fn advance_tick(&mut self) -> bool {
        self.tick = self.tick.wrapping_add(1);
        let mut changed = false;
        if self.toast.as_ref().is_some_and(|t| self.tick >= t.expires_at_tick) {
            self.toast = None;
            changed = true;
        }
        if self.armed_delete.as_ref().is_some_and(|d| self.tick >= d.expires_at_tick) {
            self.armed_delete = None;
            changed = true;
        }
        changed || self.has_animations()
    }

    pub fn push_event(&mut self, level: EventLevel, text: impl Into<String>) {
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(EventEntry { timestamp: local_hms(), level, text: sanitize_inline(&text.into()) });
    }

    pub fn set_toast(&mut self, text: impl Into<String>, is_error: bool) {
        self.toast =
            Some(Toast { text: sanitize_inline(&text.into()), is_error, expires_at_tick: self.tick + TOAST_TICKS });
    }

    pub fn selected_task(&self) -> Option<&TaskRecord> {
        let id = self.selected_task_id.as_deref()?;
        if let Some(detail) = self.selected_detail.as_ref().filter(|d| d.id == id) {
            return Some(detail);
        }
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn selected_root_task(&self) -> Option<&TaskRecord> {
        let selected = self.selected_task()?;
        self.selected_detail
            .as_ref()
            .filter(|d| d.id == selected.root)
            .or_else(|| self.tasks.iter().find(|t| t.id == selected.root))
    }

    pub fn continuation_target(&self) -> Option<ContinuationTarget> {
        let selected = self.selected_task()?;
        let root = self.selected_root_task()?;
        if !root.status.is_terminal() || root.status == TaskStatus::Unknown {
            return None;
        }
        let node_id = root.continuation_node.as_ref()?;
        if selected.id != root.id && &selected.id != node_id {
            return None;
        }
        let kind = root.continuation_kind.clone().unwrap_or_else(|| "leaf".into());
        let label = root.continuation_agent.clone().unwrap_or_else(|| {
            if kind == "summarizer" {
                "summarizer".into()
            } else {
                root.agent.clone()
            }
        });
        Some(ContinuationTarget { root_id: root.id.clone(), node_id: node_id.clone(), label, kind })
    }

    pub fn start_new_session(&mut self) {
        self.selected_task_id = None;
        self.pending_select_task = None;
        self.selected_detail = None;
        self.answering_root = None;
        self.armed_delete = None;
        self.inspector_scroll = 0;
        if self.inspector_tab != InspectorTab::Events {
            self.inspector_tab = InspectorTab::Summary;
        }
        self.focus = Focus::Composer;
        self.set_toast("Started new session.", false);
    }

    pub fn pending_approvals(&self) -> Vec<&TaskRecord> {
        self.tasks.iter().filter(|t| t.status == TaskStatus::Approval && t.approval_id.is_some()).collect()
    }

    pub fn pending_questions(&self) -> Vec<&TaskRecord> {
        self.tasks.iter().filter(|t| t.is_waiting_input()).collect()
    }

    pub fn apply_backend_event(&mut self, event: BackendEvent) -> Vec<UiCommand> {
        let mut cmds = Vec::new();
        match event {
            BackendEvent::Snapshot { tasks, members, model, providers } => {
                match tasks {
                    Ok(rows) => {
                        self.connected = true;
                        self.last_error = None;
                        let prev_sig = self.selected_task_id.as_deref().and_then(|id| {
                            self.tasks.iter().find(|t| t.id == id).map(|t| {
                                (t.status.clone(), t.stage_status.clone(), t.turns_used, t.approval_id.clone())
                            })
                        });
                        for notice in self.questions.sync(&rows) {
                            match notice {
                                TrackerNotice::Input { id, agent, question } => {
                                    self.push_event(
                                        EventLevel::Attention,
                                        format!("Clarification needed on {id} (@{agent}): {}", preview(&question, 100)),
                                    );
                                }
                                TrackerNotice::Approval { id, agent, summary } => {
                                    self.push_event(
                                        EventLevel::Attention,
                                        format!("Approval requested on {id} (@{agent}): {summary}"),
                                    );
                                }
                                TrackerNotice::Terminal { id, agent, status } => {
                                    let level = if status == TaskStatus::Done {
                                        EventLevel::Success
                                    } else {
                                        EventLevel::Error
                                    };
                                    self.push_event(
                                        level,
                                        format!("Root {id} (@{agent}) settled: {}", status.as_str()),
                                    );
                                }
                            }
                        }
                        self.tasks = rows;
                        if let Some(ans_id) = &self.answering_root {
                            if !self.tasks.iter().any(|t| &t.id == ans_id && t.is_waiting_input()) {
                                self.answering_root = None;
                            }
                        }
                        if let Some(armed) = &self.armed_delete {
                            if !self.tasks.iter().any(|t| t.root == armed.root_id) {
                                self.armed_delete = None;
                            }
                        }
                        let prev_selected = self.selected_task_id.clone();
                        self.rebuild_tree();
                        if let Some(id) = self.selected_task_id.clone() {
                            let new_sig = self.tasks.iter().find(|t| t.id == id).map(|t| {
                                (t.status.clone(), t.stage_status.clone(), t.turns_used, t.approval_id.clone())
                            });
                            let changed_sel = prev_selected.as_deref() != Some(id.as_str());
                            let detail_missing = self.selected_detail.as_ref().is_none_or(|d| d.id != id);
                            let active = self.tasks.iter().any(|t| t.id == id && !t.status.is_terminal());
                            if changed_sel || detail_missing || prev_sig != new_sig || active {
                                cmds.push(UiCommand::FetchDetail(id));
                            }
                        } else {
                            self.selected_detail = None;
                        }
                    }
                    Err(err) => {
                        self.connected = false;
                        if self.last_error.as_deref() != Some(err.as_str()) {
                            self.push_event(EventLevel::Error, format!("Daemon update failed: {err}"));
                        }
                        self.last_error = Some(err);
                    }
                }
                if let Ok(members) = members {
                    self.members = members;
                    if !self.members.is_empty() {
                        self.selected_member_idx = self.selected_member_idx.min(self.members.len() - 1);
                    } else {
                        self.selected_member_idx = 0;
                    }
                }
                if let Ok(model) = model {
                    self.model = Some(model);
                }
                if let Some((_active, overridden, config_path, list)) = providers {
                    self.providers = list;
                    self.providers_overridden = overridden;
                    self.providers_config_path = config_path;
                }
            }
            BackendEvent::TaskDetail(Ok(detail)) => {
                if self.selected_task_id.as_deref() == Some(detail.id.as_str()) {
                    self.selected_detail = Some(*detail);
                }
            }
            BackendEvent::TaskDetail(Err(_)) => {}
            BackendEvent::ModelsCatalog { provider, result } => {
                if let Overlay::Model { providers, provider_idx, catalog, selected_catalog, .. } = &mut self.overlay {
                    let current = providers.get(*provider_idx).map(|p| p.name.as_str());
                    if provider.as_deref().is_some_and(|p| current != Some(p)) {
                        return cmds;
                    }
                    match result {
                        Ok(list) => {
                            *selected_catalog = (!list.is_empty()).then_some(0);
                            *catalog = CatalogState::Loaded(list);
                        }
                        Err(e) => {
                            *selected_catalog = None;
                            *catalog = CatalogState::Error(e);
                        }
                    }
                }
            }
            BackendEvent::FilesLoaded(files) => {
                self.workspace_files = files;
                self.mention_idx = 0;
            }
            BackendEvent::TextOverlay { title, lines, is_diff } => {
                self.overlay = Overlay::TextView { title, lines, scroll: 0, is_diff };
            }
            BackendEvent::CommandOutcome { message, error, select_task, refresh } => {
                if let Some(id) = select_task {
                    self.pending_select_task = Some(id.clone());
                    if let Some(existing) = self.tasks.iter().find(|t| t.id == id) {
                        self.collapsed_roots.remove(&existing.root);
                    }
                    self.selected_task_id = Some(id.clone());
                    self.sidebar_follow_selection = true;
                    self.inspector_scroll = 0;
                    cmds.push(UiCommand::FetchDetail(id));
                }
                if let Some(msg) = message {
                    self.push_event(EventLevel::Success, &msg);
                    self.set_toast(msg, false);
                }
                if let Some(err) = error {
                    self.push_event(EventLevel::Error, &err);
                    self.set_toast(err, true);
                }
                if refresh {
                    cmds.push(UiCommand::RefreshAll);
                }
            }
        }
        cmds
    }

    pub fn rebuild_tree(&mut self) {
        let mut by_id = HashMap::with_capacity(self.tasks.len());
        let mut children: HashMap<String, Vec<usize>> = HashMap::new();
        let mut roots = Vec::new();
        for (idx, task) in self.tasks.iter().enumerate() {
            by_id.insert(task.id.as_str(), idx);
        }
        for (idx, task) in self.tasks.iter().enumerate() {
            if let Some(parent) = task.parent.as_deref().filter(|p| by_id.contains_key(p)) {
                children.entry(parent.to_owned()).or_default().push(idx);
            } else {
                roots.push(idx);
            }
        }
        // Show newest roots first so newly submitted tasks appear immediately at the top.
        roots.reverse();

        let mut rows = Vec::with_capacity(self.tasks.len());
        for root_idx in roots {
            let root = &self.tasks[root_idx];
            let collapsed = self.collapsed_roots.contains(&root.id);
            let mut stack = vec![(root_idx, String::new(), true, true)];
            while let Some((idx, prefix, last, is_root)) = stack.pop() {
                let task = &self.tasks[idx];
                let kids = children.get(&task.id);
                let has_children = kids.is_some_and(|k| !k.is_empty());
                let branch = if is_root {
                    ""
                } else if last {
                    "└─ "
                } else {
                    "├─ "
                };
                rows.push(TreeRow {
                    task_id: task.id.clone(),
                    root_id: task.root.clone(),
                    prefix: format!("{prefix}{branch}"),
                    is_root,
                    has_children,
                    collapsed: is_root && collapsed,
                });
                if is_root && collapsed {
                    continue;
                }
                if let Some(kids) = kids {
                    let child_prefix = if is_root {
                        String::new()
                    } else if last {
                        format!("{prefix}   ")
                    } else {
                        format!("{prefix}│  ")
                    };
                    for (pos, &child_idx) in kids.iter().enumerate().rev() {
                        stack.push((child_idx, child_prefix.clone(), pos + 1 == kids.len(), false));
                    }
                }
            }
        }
        self.tree_rows = rows;

        if let Some(target) = self.pending_select_task.take() {
            if self.tasks.iter().any(|t| t.id == target) {
                self.selected_task_id = Some(target);
                return;
            }
        }
        if let Some(current) = &self.selected_task_id {
            if self.tree_rows.iter().any(|r| &r.task_id == current) || self.tasks.iter().any(|t| &t.id == current) {
                return;
            }
        }
        self.selected_task_id = None;
        self.selected_detail = None;
        self.sidebar_follow_selection = true;
        if self.inspector_tab != InspectorTab::Events {
            self.inspector_tab = InspectorTab::Summary;
        }
    }

    pub fn handle_paste(&mut self, text: &str) {
        match &mut self.overlay {
            Overlay::Model { model_input, field: ModelField::ModelInput, .. } => model_input.insert_str(text),
            Overlay::Check { input } => input.insert_str(text),
            Overlay::None if self.focus == Focus::Composer => {
                self.input.insert_str(text);
                self.slash_idx = 0;
            }
            _ => {}
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent, hits: &HitMap) -> Vec<UiCommand> {
        let mut cmds = Vec::new();
        let col = mouse.column;
        let row = mouse.row;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.armed_delete = None;
                let Some(target) = hits.hit_test(col, row).cloned() else {
                    return cmds;
                };
                match target {
                    HitTarget::ModalBackdrop | HitTarget::ModalClose => {
                        self.overlay = Overlay::None;
                    }
                    HitTarget::ModalSurface => {}
                    HitTarget::ModalApprovalRow(idx) => {
                        if let Overlay::Approvals { selected } = &mut self.overlay {
                            *selected = idx;
                        }
                    }
                    HitTarget::ModalApprove(approval_id) => {
                        if self.pending_approvals().len() <= 1 {
                            self.overlay = Overlay::None;
                        }
                        cmds.push(UiCommand::Approve { approval_id, allow: true });
                    }
                    HitTarget::ModalDeny(approval_id) => {
                        if self.pending_approvals().len() <= 1 {
                            self.overlay = Overlay::None;
                        }
                        cmds.push(UiCommand::Approve { approval_id, allow: false });
                    }
                    HitTarget::ModalModelField(target_field) => {
                        if let Overlay::Model { field, .. } = &mut self.overlay {
                            *field = target_field;
                        }
                    }
                    HitTarget::ModalProviderOption(idx) => {
                        cmds.extend(self.select_modal_provider(idx, true));
                    }
                    HitTarget::ModalEffortOption(idx) => {
                        if let Overlay::Model { effort_idx, field, .. } = &mut self.overlay {
                            *effort_idx = idx.min(EFFORT_OPTIONS.len() - 1);
                            *field = ModelField::EffortBar;
                        }
                    }
                    HitTarget::ModalCatalogRow(idx) => {
                        if let Overlay::Model {
                            catalog: CatalogState::Loaded(list),
                            selected_catalog,
                            model_input,
                            field,
                            ..
                        } = &mut self.overlay
                        {
                            if let Some(m) = list.get(idx) {
                                *selected_catalog = Some(idx);
                                model_input.set_text(m);
                                *field = ModelField::CatalogList;
                            }
                        }
                    }
                    HitTarget::ModalModelSave => cmds.extend(self.commit_model_modal()),
                    HitTarget::ModalCheckSave => self.commit_check_modal(),
                    HitTarget::ModalAgentRow(idx) => {
                        if let Some(m) = self.members.get(idx) {
                            let name = m.name.clone();
                            self.agent = name.clone();
                            self.overlay = Overlay::None;
                            self.push_event(EventLevel::Info, format!("Selected @{name}."));
                            self.set_toast(format!("Target: @{name}"), false);
                        }
                    }
                    HitTarget::SlashItem(idx) => {
                        let matches = matching_commands(self.input.as_str());
                        if let Some(spec) = matches.get(idx) {
                            self.input.set_text(&spec.completion());
                            self.slash_idx = 0;
                            self.focus = Focus::Composer;
                        }
                    }
                    HitTarget::MentionItem(idx) => {
                        self.accept_mention_at(idx);
                        self.focus = Focus::Composer;
                    }
                    HitTarget::NewSession => self.start_new_session(),
                    HitTarget::SidebarToggle => self.toggle_sidebar(),
                    HitTarget::OpenHelp => self.overlay = Overlay::Help,
                    HitTarget::SidebarTab(tab) => {
                        self.focus = Focus::Sidebar;
                        self.sidebar_tab = tab;
                    }
                    HitTarget::SidebarSessionRow { task_id, root_id, toggle_fold } => {
                        self.focus = Focus::Sidebar;
                        if toggle_fold {
                            if !self.collapsed_roots.remove(&root_id) {
                                self.collapsed_roots.insert(root_id);
                            }
                            self.rebuild_tree();
                        }
                        if self.selected_task_id.as_deref() != Some(task_id.as_str()) {
                            self.selected_task_id = Some(task_id.clone());
                            self.sidebar_follow_selection = true;
                            self.inspector_scroll = 0;
                            cmds.push(UiCommand::FetchDetail(task_id));
                        }
                    }
                    HitTarget::SidebarMemberRow(idx) => {
                        self.focus = Focus::Sidebar;
                        if idx < self.members.len() {
                            self.selected_member_idx = idx;
                        }
                    }
                    HitTarget::InspectorTab(tab) => {
                        self.focus = Focus::Inspector;
                        self.inspector_tab = tab;
                        self.inspector_scroll = 0;
                    }
                    HitTarget::AlertApprovals => cmds.extend(self.open_approvals_modal()),
                    HitTarget::AlertClarification(root_id) | HitTarget::InlineReply(root_id) => {
                        self.answering_root = Some(root_id);
                        self.focus = Focus::Composer;
                    }
                    HitTarget::InlineApprove(approval_id) => {
                        cmds.push(UiCommand::Approve { approval_id, allow: true });
                    }
                    HitTarget::InlineDeny(approval_id) => {
                        cmds.push(UiCommand::Approve { approval_id, allow: false });
                    }
                    HitTarget::ComposerPane { inner, scroll } => {
                        self.focus = Focus::Composer;
                        if inner.width > 0 && inner.height > 0 && row >= inner.y && row < inner.y + inner.height {
                            let vcol = col.saturating_sub(inner.x);
                            let vrow = scroll.saturating_add(row - inner.y);
                            self.input.set_cursor_visual(inner.width as usize, vcol, vrow);
                        }
                    }
                    HitTarget::SidebarPane => self.focus = Focus::Sidebar,
                    HitTarget::InspectorPane => self.focus = Focus::Inspector,
                }
            }
            MouseEventKind::ScrollUp => match hits.scroll_target(col, row) {
                Some(HitTarget::InspectorPane) => {
                    self.inspector_scroll = self.inspector_scroll.saturating_sub(3);
                }
                Some(HitTarget::SidebarPane) => {
                    self.sidebar_follow_selection = false;
                    self.sidebar_scroll = self.sidebar_scroll.saturating_sub(2);
                }
                Some(HitTarget::ModalSurface) => {
                    if let Overlay::TextView { scroll, .. } = &mut self.overlay {
                        *scroll = scroll.saturating_sub(3);
                    }
                }
                _ => {}
            },
            MouseEventKind::ScrollDown => match hits.scroll_target(col, row) {
                Some(HitTarget::InspectorPane) => {
                    self.inspector_scroll = self.inspector_scroll.saturating_add(3).min(self.inspector_max_scroll);
                }
                Some(HitTarget::SidebarPane) => {
                    self.sidebar_follow_selection = false;
                    self.sidebar_scroll = self.sidebar_scroll.saturating_add(2).min(self.sidebar_max_scroll);
                }
                Some(HitTarget::ModalSurface) => {
                    if let Overlay::TextView { scroll, lines, .. } = &mut self.overlay {
                        *scroll = scroll.saturating_add(3).min(lines.len().saturating_sub(1) as u16);
                    }
                }
                _ => {}
            },
            _ => {}
        }
        cmds
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<UiCommand> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }

        // Complementary global navigation, layout, safety modal, help & exit bindings.
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('q') if ctrl => {
                self.should_quit = true;
                return vec![UiCommand::Quit];
            }
            KeyCode::Char('c') if ctrl => {
                if self.overlay != Overlay::None {
                    self.overlay = Overlay::None;
                } else if self.armed_delete.take().is_some() {
                    self.set_toast("Cancelled session deletion.", false);
                } else if self.focus == Focus::Composer && !self.input.is_empty() {
                    self.input.clear();
                    self.set_toast("Cleared input (press Ctrl+C again or Ctrl+Q to quit)", false);
                } else {
                    self.should_quit = true;
                    return vec![UiCommand::Quit];
                }
                return Vec::new();
            }
            KeyCode::F(1) => {
                self.armed_delete = None;
                self.overlay = if self.overlay == Overlay::Help { Overlay::None } else { Overlay::Help };
                return Vec::new();
            }
            KeyCode::Char('n') if ctrl => {
                self.start_new_session();
                return Vec::new();
            }
            KeyCode::Char('b') if ctrl => {
                self.armed_delete = None;
                self.toggle_sidebar();
                return Vec::new();
            }
            KeyCode::Char('p') if ctrl => {
                self.armed_delete = None;
                return self.open_approvals_modal();
            }
            KeyCode::Char(d @ '1'..='5') if alt => {
                if let Some(tab) = InspectorTab::from_digit(d) {
                    self.inspector_tab = tab;
                    self.inspector_scroll = 0;
                }
                return Vec::new();
            }
            _ => {}
        }

        if self.overlay != Overlay::None {
            return self.handle_overlay_key(key);
        }

        match self.focus {
            Focus::Composer => self.handle_composer_key(key),
            Focus::Sidebar => self.handle_sidebar_key(key),
            Focus::Inspector => self.handle_inspector_key(key),
        }
    }

    pub fn toggle_sidebar(&mut self) {
        if self.last_width >= WIDE_THRESHOLD {
            self.sidebar_visible = !self.sidebar_visible;
            if !self.sidebar_visible && self.focus == Focus::Sidebar {
                self.focus = Focus::Inspector;
            }
        } else {
            self.focus = if self.focus == Focus::Sidebar { Focus::Inspector } else { Focus::Sidebar };
        }
    }

    fn sidebar_accessible(&self) -> bool {
        self.last_width < WIDE_THRESHOLD || self.sidebar_visible
    }

    fn open_approvals_modal(&mut self) -> Vec<UiCommand> {
        let pending = self.pending_approvals();
        if pending.is_empty() {
            self.set_toast("No pending approvals.", false);
        } else {
            self.overlay = Overlay::Approvals { selected: 0 };
        }
        Vec::new()
    }

    fn open_model_modal(&mut self, focus_provider: bool) -> Vec<UiCommand> {
        let mut model_input = TextInput::default();
        if let Some(m) = self.model.as_ref().and_then(|c| c.model.as_deref()) {
            model_input.set_text(m);
        }
        let effort_idx = self
            .model
            .as_ref()
            .and_then(|c| c.effort.as_deref())
            .and_then(|e| EFFORT_OPTIONS.iter().position(|&o| o == e))
            .unwrap_or(0);
        let providers = if !self.providers.is_empty() {
            self.providers.clone()
        } else if let Some(cfg) = &self.model {
            vec![ProviderSummary {
                name: cfg.provider.clone(),
                wire: if cfg.provider == "chatgpt" { "chatgpt" } else { "unknown" }.into(),
                builtin: true,
                base_url: None,
                api_key_env: None,
                default_model: cfg.model.clone(),
                context_window: None,
                tool_choice: "required".into(),
                ready: true,
                status: "active".into(),
            }]
        } else {
            Vec::new()
        };
        let initial_provider = self
            .model
            .as_ref()
            .map_or_else(|| providers.first().map_or_else(String::new, |p| p.name.clone()), |c| c.provider.clone());
        let provider_idx = providers.iter().position(|p| p.name == initial_provider).unwrap_or(0);
        let wire = providers.get(provider_idx).map_or("", |p| p.wire.as_str());
        let should_load = !initial_provider.is_empty()
            && wire != "anthropic"
            && (!self.providers.is_empty() || initial_provider == "chatgpt");
        self.overlay = Overlay::Model {
            providers,
            provider_idx,
            initial_provider: initial_provider.clone(),
            model_input,
            effort_idx,
            catalog: if should_load { CatalogState::Loading } else { CatalogState::Idle },
            selected_catalog: None,
            field: if focus_provider { ModelField::ProviderBar } else { ModelField::ModelInput },
        };
        if should_load {
            vec![UiCommand::LoadModels(Some(initial_provider))]
        } else {
            Vec::new()
        }
    }

    fn select_modal_provider(&mut self, new_idx: usize, focus_bar: bool) -> Vec<UiCommand> {
        let Overlay::Model {
            providers,
            provider_idx,
            initial_provider,
            model_input,
            catalog,
            selected_catalog,
            field,
            ..
        } = &mut self.overlay
        else {
            return Vec::new();
        };
        if focus_bar {
            *field = ModelField::ProviderBar;
        }
        if new_idx >= providers.len() || new_idx == *provider_idx {
            return Vec::new();
        }
        *provider_idx = new_idx;
        let chosen = &providers[new_idx];
        if chosen.name == *initial_provider {
            model_input.set_text(self.model.as_ref().and_then(|c| c.model.as_deref()).unwrap_or(""));
        } else {
            model_input.set_text(chosen.default_model.as_deref().unwrap_or(""));
        }
        *selected_catalog = None;
        if chosen.wire == "anthropic" {
            *catalog = CatalogState::Idle;
            Vec::new()
        } else {
            *catalog = CatalogState::Loading;
            vec![UiCommand::LoadModels(Some(chosen.name.clone()))]
        }
    }

    fn commit_model_modal(&mut self) -> Vec<UiCommand> {
        let Overlay::Model { providers, provider_idx, initial_provider, model_input, effort_idx, .. } = &self.overlay
        else {
            return Vec::new();
        };
        let selected_provider = providers.get(*provider_idx).map(|p| p.name.as_str()).unwrap_or(initial_provider);
        let provider_arg = (selected_provider != *initial_provider).then(|| Some(selected_provider.to_owned()));
        let m = model_input.as_str().trim();
        let model_arg = if m.is_empty() { None } else { Some(m.to_owned()) };
        let e = EFFORT_OPTIONS[*effort_idx];
        let effort_arg = if e == "default" { None } else { Some(e.to_owned()) };
        self.overlay = Overlay::None;
        vec![UiCommand::SetModel { provider: provider_arg, model: Some(model_arg), effort: Some(effort_arg) }]
    }

    fn open_agents_modal(&mut self) {
        let selected = self.members.iter().position(|m| m.name == self.agent).unwrap_or(0);
        self.overlay = Overlay::Agents { selected };
    }

    fn open_check_modal(&mut self) {
        let mut input = TextInput::default();
        if let Some(cmd) = &self.armed_check {
            input.set_text(cmd);
        }
        self.overlay = Overlay::Check { input };
    }

    fn commit_check_modal(&mut self) {
        let Overlay::Check { input } = &self.overlay else {
            return;
        };
        let text = input.as_str().trim().to_owned();
        self.armed_check = (!text.is_empty()).then_some(text);
        let notice = match &self.armed_check {
            Some(cmd) => format!("Armed --check: {}", preview(cmd, 80)),
            None => "Cleared --check command.".into(),
        };
        self.overlay = Overlay::None;
        self.push_event(EventLevel::Info, &notice);
        self.set_toast(notice, false);
    }

    fn open_status_overlay(&mut self) {
        let model_lbl = self
            .model
            .as_ref()
            .map(|m| {
                let id = m.model.as_deref().unwrap_or("auto");
                let eff = m.effort.as_deref().unwrap_or("default");
                format!("{}/{id} (effort: {eff})", m.provider)
            })
            .unwrap_or_else(|| "connecting…".into());
        let mut lines = vec![
            format!("Workspace:   {}", self.workspace_name),
            format!("Target:      @{}", self.agent),
            format!("Model:       {model_lbl}"),
            format!("Policy:      {}", if self.review_mode { "review" } else { "auto" }),
            format!("Sandbox:     {}", self.sandbox_mode.as_str()),
            format!("Check:       {}", self.armed_check.as_deref().unwrap_or("none")),
            format!("Files:       {} indexed workspace path(s)", self.workspace_files.len()),
        ];
        if let Some(root) = self.selected_root_task() {
            lines.push(String::new());
            lines.push(format!("Selected root: {}  [{}]", root.id, root.status.as_str()));
            lines.push(format!(
                "Root agent:    @{}  ·  round {}  ·  turns {}",
                root.agent,
                root.round.unwrap_or(1),
                root.turns_used.unwrap_or(0)
            ));
            if let Some(sb) = &root.sandbox {
                lines.push(format!("Root sandbox:  {sb}"));
            }
            if root.pending_steerings > 0 {
                lines.push(format!("Steerings:     {} queued", root.pending_steerings));
            }
            if let Some(u) = &root.usage {
                lines.push(format!(
                    "Token usage:   {} in ({} cached, {}% hit) · {} out · last {} in",
                    u.input_tokens, u.cached_tokens, u.cache_hit_rate_pct, u.output_tokens, u.last_input_tokens
                ));
            }
            if !root.plan.is_empty() {
                lines.push(format!("Plan steps:    {}", root.plan.len()));
            }
        }
        self.overlay = Overlay::TextView { title: "Session Status".into(), lines, scroll: 0, is_diff: false };
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) -> Vec<UiCommand> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match &mut self.overlay {
            Overlay::None => Vec::new(),
            Overlay::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | '?')) {
                    self.overlay = Overlay::None;
                }
                Vec::new()
            }
            Overlay::Approvals { selected } => {
                let pending_ids: Vec<(String, String)> = self
                    .tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::Approval)
                    .filter_map(|t| Some((t.id.clone(), t.approval_id.clone()?)))
                    .collect();
                if pending_ids.is_empty() || key.code == KeyCode::Esc {
                    self.overlay = Overlay::None;
                    return Vec::new();
                }
                let idx = (*selected).min(pending_ids.len() - 1);
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        *selected = idx.saturating_sub(1);
                        Vec::new()
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        *selected = (idx + 1).min(pending_ids.len() - 1);
                        Vec::new()
                    }
                    KeyCode::Char('a') | KeyCode::Enter => {
                        let approval_id = pending_ids[idx].1.clone();
                        if pending_ids.len() <= 1 {
                            self.overlay = Overlay::None;
                        }
                        vec![UiCommand::Approve { approval_id, allow: true }]
                    }
                    KeyCode::Char('d') => {
                        let approval_id = pending_ids[idx].1.clone();
                        if pending_ids.len() <= 1 {
                            self.overlay = Overlay::None;
                        }
                        vec![UiCommand::Approve { approval_id, allow: false }]
                    }
                    _ => Vec::new(),
                }
            }
            Overlay::Model {
                providers,
                provider_idx,
                model_input,
                effort_idx,
                catalog,
                selected_catalog,
                field,
                ..
            } => match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Vec::new()
                }
                KeyCode::Tab => {
                    let has_catalog = matches!(catalog, CatalogState::Loaded(list) if !list.is_empty());
                    *field = match *field {
                        ModelField::ProviderBar => ModelField::ModelInput,
                        ModelField::ModelInput if has_catalog => ModelField::CatalogList,
                        ModelField::ModelInput | ModelField::CatalogList => ModelField::EffortBar,
                        ModelField::EffortBar => ModelField::ProviderBar,
                    };
                    Vec::new()
                }
                KeyCode::BackTab => {
                    let has_catalog = matches!(catalog, CatalogState::Loaded(list) if !list.is_empty());
                    *field = match *field {
                        ModelField::ProviderBar => ModelField::EffortBar,
                        ModelField::ModelInput => ModelField::ProviderBar,
                        ModelField::CatalogList => ModelField::ModelInput,
                        ModelField::EffortBar if has_catalog => ModelField::CatalogList,
                        ModelField::EffortBar => ModelField::ModelInput,
                    };
                    Vec::new()
                }
                KeyCode::Enter => self.commit_model_modal(),
                KeyCode::Up | KeyCode::Down => {
                    if let CatalogState::Loaded(list) = catalog {
                        if !list.is_empty() {
                            let next = match (*selected_catalog, key.code) {
                                (None, _) => 0,
                                (Some(i), KeyCode::Up) => i.saturating_sub(1),
                                (Some(i), _) => (i + 1).min(list.len() - 1),
                            };
                            *selected_catalog = Some(next);
                            model_input.set_text(&list[next]);
                        }
                    }
                    Vec::new()
                }
                KeyCode::Left | KeyCode::Char('h') if *field == ModelField::ProviderBar => {
                    let next = provider_idx.saturating_sub(1);
                    self.select_modal_provider(next, false)
                }
                KeyCode::Right | KeyCode::Char('l') if *field == ModelField::ProviderBar => {
                    let next = (*provider_idx + 1).min(providers.len().saturating_sub(1));
                    self.select_modal_provider(next, false)
                }
                KeyCode::Left if *field == ModelField::EffortBar => {
                    *effort_idx = effort_idx.saturating_sub(1);
                    Vec::new()
                }
                KeyCode::Right if *field == ModelField::EffortBar => {
                    *effort_idx = (*effort_idx + 1).min(EFFORT_OPTIONS.len() - 1);
                    Vec::new()
                }
                _ if *field == ModelField::ModelInput => {
                    edit_text_input(model_input, key, ctrl);
                    Vec::new()
                }
                KeyCode::Char('h') if *field == ModelField::EffortBar => {
                    *effort_idx = effort_idx.saturating_sub(1);
                    Vec::new()
                }
                KeyCode::Char('l') if *field == ModelField::EffortBar => {
                    *effort_idx = (*effort_idx + 1).min(EFFORT_OPTIONS.len() - 1);
                    Vec::new()
                }
                _ => Vec::new(),
            },
            Overlay::Check { input } => match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Vec::new()
                }
                KeyCode::Enter => {
                    self.commit_check_modal();
                    Vec::new()
                }
                _ => {
                    edit_text_input(input, key, ctrl);
                    Vec::new()
                }
            },
            Overlay::TextView { scroll, lines, .. } => {
                let max_scroll = lines.len().saturating_sub(1) as u16;
                match key.code {
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.overlay = Overlay::None,
                    KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1).min(max_scroll),
                    KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                    KeyCode::PageDown => *scroll = scroll.saturating_add(10).min(max_scroll),
                    KeyCode::Char('u') if ctrl => *scroll = scroll.saturating_sub(10),
                    KeyCode::Char('d') if ctrl => *scroll = scroll.saturating_add(10).min(max_scroll),
                    KeyCode::Home | KeyCode::Char('g') => *scroll = 0,
                    KeyCode::End | KeyCode::Char('G') => *scroll = max_scroll,
                    _ => {}
                }
                Vec::new()
            }
            Overlay::Agents { selected } => {
                if key.code == KeyCode::Esc || self.members.is_empty() {
                    self.overlay = Overlay::None;
                    return Vec::new();
                }
                let max = self.members.len() - 1;
                *selected = (*selected).min(max);
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => *selected = (*selected + 1).min(max),
                    KeyCode::Enter => {
                        let name = self.members[*selected].name.clone();
                        self.agent = name.clone();
                        self.overlay = Overlay::None;
                        self.push_event(EventLevel::Info, format!("Selected @{name}."));
                        self.set_toast(format!("Target: @{name}"), false);
                    }
                    _ => {}
                }
                Vec::new()
            }
        }
    }

    fn handle_composer_key(&mut self, key: KeyEvent) -> Vec<UiCommand> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        if key.code == KeyCode::Esc {
            if self.answering_root.take().is_some() {
                self.set_toast("Cancelled explicit answer target.", false);
            } else if self.sidebar_accessible() {
                self.focus = Focus::Sidebar;
            } else {
                self.focus = Focus::Inspector;
            }
            return Vec::new();
        }
        if key.code == KeyCode::BackTab {
            self.focus = Focus::Inspector;
            return Vec::new();
        }
        if key.code == KeyCode::Tab {
            let matches = matching_commands(self.input.as_str());
            if let Some(spec) = matches.get(self.slash_idx % matches.len().max(1)).copied() {
                self.input.set_text(&spec.completion());
                self.slash_idx = 0;
            } else if self.accept_mention_at(self.mention_idx) {
            } else if self.sidebar_accessible() {
                self.focus = Focus::Sidebar;
            } else {
                self.focus = Focus::Inspector;
            }
            return Vec::new();
        }

        if key.code == KeyCode::Enter && (shift || alt) || (ctrl && key.code == KeyCode::Char('j')) {
            self.input.insert_char('\n');
            return Vec::new();
        }
        if key.code == KeyCode::Enter {
            if let Some((_, _, query, m_list)) = self.active_mention() {
                let exact = m_list.len() == 1 && m_list[0] == query;
                if !exact && self.accept_mention_at(self.mention_idx) {
                    return Vec::new();
                }
            }
            return self.submit_composer();
        }

        let matches = matching_commands(self.input.as_str());
        if !matches.is_empty() && matches!(key.code, KeyCode::Up | KeyCode::Down) {
            if key.code == KeyCode::Up {
                self.slash_idx = self.slash_idx.saturating_sub(1);
            } else {
                self.slash_idx = (self.slash_idx + 1) % matches.len();
            }
            return Vec::new();
        }

        if let Some((_, _, _, m_list)) = self.active_mention() {
            if matches!(key.code, KeyCode::Up | KeyCode::Down) {
                if key.code == KeyCode::Up {
                    self.mention_idx = self.mention_idx.saturating_sub(1);
                } else {
                    self.mention_idx = (self.mention_idx + 1) % m_list.len();
                }
                return Vec::new();
            }
        }

        match key.code {
            KeyCode::Up if !self.input.as_str().contains('\n') => self.input.history_prev(),
            KeyCode::Down if !self.input.as_str().contains('\n') => self.input.history_next(),
            _ => {
                edit_text_input(&mut self.input, key, ctrl);
                self.slash_idx = 0;
                self.mention_idx = 0;
            }
        }
        Vec::new()
    }

    fn handle_sidebar_key(&mut self, key: KeyEvent) -> Vec<UiCommand> {
        let mut cmds = Vec::new();
        if key.code == KeyCode::Delete {
            if self.sidebar_tab == SidebarTab::Trees {
                self.trigger_delete(&mut cmds);
            } else {
                self.set_toast("Switch to Sessions ([) to delete a session.", true);
            }
            return cmds;
        }
        if key.code == KeyCode::Esc {
            if self.armed_delete.take().is_some() {
                self.set_toast("Cancelled session deletion.", false);
            } else {
                self.focus = Focus::Composer;
            }
            return cmds;
        }

        self.armed_delete = None;
        match key.code {
            KeyCode::Tab => self.focus = Focus::Inspector,
            KeyCode::BackTab => self.focus = Focus::Composer,
            KeyCode::Char('i') => self.focus = Focus::Composer,
            KeyCode::Char('/') => {
                self.focus = Focus::Composer;
                self.input.insert_char('/');
            }
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            KeyCode::Up | KeyCode::Char('k') => self.move_sidebar(-1, &mut cmds),
            KeyCode::Down | KeyCode::Char('j') => self.move_sidebar(1, &mut cmds),
            KeyCode::Char('[') => self.sidebar_tab = SidebarTab::Trees,
            KeyCode::Char(']') => self.sidebar_tab = SidebarTab::Members,
            KeyCode::Char(d @ '1'..='5') => {
                if let Some(tab) = InspectorTab::from_digit(d) {
                    self.inspector_tab = tab;
                    self.inspector_scroll = 0;
                }
            }
            KeyCode::Char(' ') | KeyCode::Enter | KeyCode::Left | KeyCode::Right => {
                if self.sidebar_tab == SidebarTab::Members {
                    if key.code == KeyCode::Enter {
                        if let Some(m) = self.members.get(self.selected_member_idx) {
                            self.agent = m.name.clone();
                            self.focus = Focus::Composer;
                            self.set_toast(format!("Target: @{}", self.agent), false);
                        }
                    }
                } else if let Some(row) = self
                    .selected_task_id
                    .as_deref()
                    .and_then(|id| self.tree_rows.iter().find(|r| r.task_id == id))
                    .cloned()
                {
                    if row.is_root && row.has_children {
                        if matches!(key.code, KeyCode::Left) {
                            self.collapsed_roots.insert(row.root_id);
                        } else if matches!(key.code, KeyCode::Right) {
                            self.collapsed_roots.remove(&row.root_id);
                        } else if !self.collapsed_roots.remove(&row.root_id) {
                            self.collapsed_roots.insert(row.root_id);
                        }
                        self.rebuild_tree();
                    } else if !row.is_root && key.code == KeyCode::Left {
                        self.collapsed_roots.insert(row.root_id.clone());
                        self.selected_task_id = Some(row.root_id.clone());
                        self.sidebar_follow_selection = true;
                        self.rebuild_tree();
                        cmds.push(UiCommand::FetchDetail(row.root_id));
                    }
                }
            }
            KeyCode::Char('a') => self.approve_selected(true, &mut cmds),
            KeyCode::Char('d') => self.approve_selected(false, &mut cmds),
            KeyCode::Char('x') => self.cancel_selected(&mut cmds),
            KeyCode::Char('r') => self.reply_to_selected(),
            _ => {}
        }
        cmds
    }

    fn handle_inspector_key(&mut self, key: KeyEvent) -> Vec<UiCommand> {
        let mut cmds = Vec::new();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Delete {
            self.trigger_delete(&mut cmds);
            return cmds;
        }
        if key.code == KeyCode::Esc {
            if self.armed_delete.take().is_some() {
                self.set_toast("Cancelled session deletion.", false);
            } else {
                self.focus = Focus::Composer;
            }
            return cmds;
        }

        self.armed_delete = None;
        match key.code {
            KeyCode::Tab => self.focus = Focus::Composer,
            KeyCode::BackTab => {
                self.focus = if self.sidebar_accessible() { Focus::Sidebar } else { Focus::Composer };
            }
            KeyCode::Char('i') => self.focus = Focus::Composer,
            KeyCode::Char('/') => {
                self.focus = Focus::Composer;
                self.input.insert_char('/');
            }
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            KeyCode::Up | KeyCode::Char('k') => {
                self.inspector_scroll = self.inspector_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.inspector_scroll = self.inspector_scroll.saturating_add(1).min(self.inspector_max_scroll);
            }
            KeyCode::PageUp => {
                self.inspector_scroll = self.inspector_scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                self.inspector_scroll = self.inspector_scroll.saturating_add(10).min(self.inspector_max_scroll);
            }
            KeyCode::Char('u') if ctrl => {
                self.inspector_scroll = self.inspector_scroll.saturating_sub(10);
            }
            KeyCode::Char('d') if ctrl => {
                self.inspector_scroll = self.inspector_scroll.saturating_add(10).min(self.inspector_max_scroll);
            }
            KeyCode::Home | KeyCode::Char('g') => self.inspector_scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.inspector_scroll = self.inspector_max_scroll,
            KeyCode::Left => {
                self.inspector_tab = self.inspector_tab.prev();
                self.inspector_scroll = 0;
            }
            KeyCode::Right => {
                self.inspector_tab = self.inspector_tab.next();
                self.inspector_scroll = 0;
            }
            KeyCode::Char('[') => self.sidebar_tab = SidebarTab::Trees,
            KeyCode::Char(']') => self.sidebar_tab = SidebarTab::Members,
            KeyCode::Char(d @ '1'..='5') => {
                if let Some(tab) = InspectorTab::from_digit(d) {
                    self.inspector_tab = tab;
                    self.inspector_scroll = 0;
                }
            }
            KeyCode::Char('a') => self.approve_selected(true, &mut cmds),
            KeyCode::Char('d') => self.approve_selected(false, &mut cmds),
            KeyCode::Char('x') => self.cancel_selected(&mut cmds),
            KeyCode::Char('r') => self.reply_to_selected(),
            _ => {}
        }
        cmds
    }

    fn move_sidebar(&mut self, delta: isize, cmds: &mut Vec<UiCommand>) {
        self.sidebar_follow_selection = true;
        match self.sidebar_tab {
            SidebarTab::Trees => {
                if self.tree_rows.is_empty() {
                    return;
                }
                let next = match self
                    .selected_task_id
                    .as_deref()
                    .and_then(|id| self.tree_rows.iter().position(|r| r.task_id == id))
                {
                    Some(cur) => (cur as isize + delta).clamp(0, self.tree_rows.len() as isize - 1) as usize,
                    None if delta < 0 => self.tree_rows.len() - 1,
                    None => 0,
                };
                let id = self.tree_rows[next].task_id.clone();
                if self.selected_task_id.as_deref() != Some(id.as_str()) {
                    self.selected_task_id = Some(id.clone());
                    self.inspector_scroll = 0;
                    cmds.push(UiCommand::FetchDetail(id));
                }
            }
            SidebarTab::Members => {
                if self.members.is_empty() {
                    return;
                }
                let next = (self.selected_member_idx as isize + delta).clamp(0, self.members.len() as isize - 1);
                self.selected_member_idx = next as usize;
            }
        }
    }

    fn trigger_delete(&mut self, cmds: &mut Vec<UiCommand>) {
        let Some(task) = self.selected_task() else {
            self.set_toast("No session selected.", true);
            return;
        };
        let root_id = task.root.clone();
        let root_task = self.tasks.iter().find(|t| t.id == root_id).unwrap_or(task);
        let raw_label = root_task.prompt.as_deref().unwrap_or(&root_task.id);
        let label = preview(raw_label, 28);
        if self.armed_delete.as_ref().is_some_and(|a| a.root_id == root_id) {
            self.armed_delete = None;
            self.questions.clear_answered(&root_id);
            cmds.push(UiCommand::Delete(root_id));
        } else {
            self.armed_delete =
                Some(ArmedDelete { root_id, label: label.clone(), expires_at_tick: self.tick + DELETE_ARM_TICKS });
            self.set_toast(format!("Press Del again to delete \"{label}\" (Esc to cancel)"), true);
        }
    }

    fn approve_selected(&mut self, allow: bool, cmds: &mut Vec<UiCommand>) {
        if let Some(approval_id) =
            self.selected_task().filter(|t| t.status == TaskStatus::Approval).and_then(|t| t.approval_id.clone())
        {
            cmds.push(UiCommand::Approve { approval_id, allow });
        } else {
            self.set_toast("Selected task has no pending approval.", true);
        }
    }

    fn cancel_selected(&mut self, cmds: &mut Vec<UiCommand>) {
        if let Some(task) = self.selected_task() {
            if task.status.is_cancellable() {
                cmds.push(UiCommand::Cancel(task.id.clone()));
            } else {
                self.set_toast("Selected task is already terminal.", true);
            }
        }
    }

    fn reply_to_selected(&mut self) {
        let target = self
            .selected_task()
            .and_then(|t| self.tasks.iter().find(|r| r.id == t.root))
            .filter(|r| r.is_waiting_input())
            .map(|r| r.id.clone());
        if let Some(root_id) = target {
            self.answering_root = Some(root_id);
            self.focus = Focus::Composer;
        } else {
            self.set_toast("Selected root is not waiting for input.", true);
        }
    }

    fn submit_composer(&mut self) -> Vec<UiCommand> {
        let raw = self.input.as_str().trim().to_owned();
        if raw.is_empty() {
            return Vec::new();
        }
        if let Some(slash) = raw.strip_prefix('/') {
            return self.execute_slash(slash);
        }
        if let Some(root_id) = self.answering_root.take() {
            let text = self.input.take_and_push_history();
            let trimmed = text.trim().to_owned();
            self.questions.clear_answered(&root_id);
            return vec![UiCommand::Answer { root_id, text: trimmed }];
        }
        if let Some(selected) = self.selected_task().cloned() {
            let root = self.selected_root_task().cloned().unwrap_or(selected.clone());
            if root.is_waiting_input() {
                let text = self.input.take_and_push_history();
                let trimmed = text.trim().to_owned();
                self.questions.clear_answered(&root.id);
                return vec![UiCommand::Answer { root_id: root.id, text: trimmed }];
            }
            if self.questions.consume_stale_root(&root.id) {
                let msg =
                    "A waiting question closed in another client. Nothing was sent. Check status or retry.".to_owned();
                self.push_event(EventLevel::Error, &msg);
                self.set_toast(msg, true);
                return Vec::new();
            }
            if !root.status.is_terminal() && root.kind != StageKind::Legacy {
                let text = self.input.take_and_push_history();
                let trimmed = text.trim().to_owned();
                self.set_toast("Queued steering note for active session.", false);
                return vec![UiCommand::Steer { id: root.id, text: trimmed }];
            }
            if let Some(target) = self.continuation_target() {
                let text = self.input.take_and_push_history();
                let trimmed = text.trim().to_owned();
                let check = self.armed_check.take();
                return vec![UiCommand::Continue {
                    id: target.node_id,
                    text: trimmed,
                    review: self.review_mode,
                    sandbox: self.sandbox_mode,
                    check,
                }];
            }
            let msg = if !root.status.is_terminal() {
                "Session is still active; wait for it to settle or press Ctrl+N for a new session."
            } else if root.status == TaskStatus::Unknown {
                "Session outcome is unknown; press Ctrl+N to start a new session."
            } else if root.continuation_node.is_none() {
                "Session has no sole leaf or summarizer to continue; press Ctrl+N for a new session."
            } else {
                "Select the session root, sole leaf, or summarizer node to continue (or Ctrl+N for new session)."
            };
            self.push_event(EventLevel::Error, msg);
            self.set_toast(msg, true);
            return Vec::new();
        }
        let text = self.input.take_and_push_history();
        let trimmed = text.trim().to_owned();
        let check = self.armed_check.take();
        vec![UiCommand::Submit {
            agent: self.agent.clone(),
            text: trimmed,
            review: self.review_mode,
            sandbox: self.sandbox_mode,
            check,
        }]
    }

    fn execute_slash(&mut self, slash: &str) -> Vec<UiCommand> {
        let (op, args) = slash.split_once(char::is_whitespace).unwrap_or((slash, ""));
        let args = args.trim();
        let mut parts = args.split_whitespace();
        let arg1 = parts.next();
        let arg2 = parts.next();

        match (op, arg1, arg2) {
            ("clear", None, None) => {
                self.input.clear();
                self.events.clear();
                self.push_event(EventLevel::Info, "Cleared event feed.");
                Vec::new()
            }
            ("to", None, None) => {
                self.input.take_and_push_history();
                self.open_agents_modal();
                Vec::new()
            }
            ("to", Some(name), None) => {
                self.input.take_and_push_history();
                self.agent = name.to_owned();
                self.push_event(EventLevel::Info, format!("Target agent set to @{name}."));
                self.set_toast(format!("Target: @{name}"), false);
                Vec::new()
            }
            ("answer", Some(id), Some(_)) => {
                let (_, rest) = args.split_once(char::is_whitespace).unwrap();
                let text = rest.trim_start().to_owned();
                let root_id = id.to_owned();
                self.input.take_and_push_history();
                self.questions.clear_answered(&root_id);
                if self.answering_root.as_deref() == Some(root_id.as_str()) {
                    self.answering_root = None;
                }
                vec![UiCommand::Answer { root_id, text }]
            }
            ("new", None, None) => {
                self.input.take_and_push_history();
                self.start_new_session();
                Vec::new()
            }
            ("new", Some(_), _) => {
                let text = args.to_owned();
                self.input.take_and_push_history();
                let check = self.armed_check.take();
                vec![UiCommand::Submit {
                    agent: self.agent.clone(),
                    text,
                    review: self.review_mode,
                    sandbox: self.sandbox_mode,
                    check,
                }]
            }
            ("plan", Some(_), _) => {
                let text = args.to_owned();
                self.input.take_and_push_history();
                let prompt = format!(
                    "Create a concise, verified implementation plan (read-only analysis; do not modify files). Use update_plan to publish concrete steps.\n\nRequest:\n{text}"
                );
                vec![UiCommand::Submit {
                    agent: self.agent.clone(),
                    text: prompt,
                    review: self.review_mode,
                    sandbox: SandboxMode::ReadOnly,
                    check: None,
                }]
            }
            ("diff", None, None) => {
                self.input.take_and_push_history();
                vec![UiCommand::LoadDiff { base: None }]
            }
            ("diff", Some(r), None) => {
                self.input.take_and_push_history();
                vec![UiCommand::LoadDiff { base: Some(r.to_owned()) }]
            }
            ("compact", None, None) => {
                let Some(root_id) = self.selected_root_task().map(|r| r.id.clone()) else {
                    self.set_toast("Select a session first before compacting.", true);
                    return Vec::new();
                };
                self.input.take_and_push_history();
                vec![UiCommand::Compact(root_id)]
            }
            ("steer", Some(_), _) => {
                let Some(root) = self.selected_root_task().filter(|r| !r.status.is_terminal()) else {
                    self.set_toast("Select an active running session first to steer.", true);
                    return Vec::new();
                };
                let root_id = root.id.clone();
                let text = args.to_owned();
                self.input.take_and_push_history();
                vec![UiCommand::Steer { id: root_id, text }]
            }
            ("skills", None, None) => {
                self.input.take_and_push_history();
                vec![UiCommand::LoadSkills]
            }
            ("mcp", None, None) => {
                self.input.take_and_push_history();
                vec![UiCommand::LoadMcp]
            }
            ("status", None, None) => {
                self.input.take_and_push_history();
                self.open_status_overlay();
                Vec::new()
            }
            ("check", None, None) => {
                self.input.take_and_push_history();
                self.open_check_modal();
                Vec::new()
            }
            ("check", Some("off" | "clear"), None) => {
                self.input.take_and_push_history();
                self.armed_check = None;
                self.push_event(EventLevel::Info, "Cleared --check command.");
                self.set_toast("Cleared --check command.", false);
                Vec::new()
            }
            ("check", Some(_), _) => {
                let cmd = args.to_owned();
                self.input.take_and_push_history();
                self.armed_check = Some(cmd.clone());
                self.push_event(EventLevel::Info, format!("Armed --check: {}", preview(&cmd, 80)));
                self.set_toast(format!("Armed --check: {}", preview(&cmd, 60)), false);
                Vec::new()
            }
            ("review", None, None) => {
                self.input.take_and_push_history();
                self.review_mode = !self.review_mode;
                let mode = if self.review_mode { "review" } else { "auto" };
                self.push_event(EventLevel::Info, format!("Policy switched to {mode}."));
                self.set_toast(format!("Policy: {mode}"), false);
                Vec::new()
            }
            ("review", Some(mode @ ("on" | "off")), None) => {
                self.input.take_and_push_history();
                self.review_mode = mode == "on";
                let label = if self.review_mode { "review" } else { "auto" };
                self.push_event(EventLevel::Info, format!("Policy set to {label}."));
                self.set_toast(format!("Policy: {label}"), false);
                Vec::new()
            }
            ("review", Some("code"), base_opt) => {
                self.input.take_and_push_history();
                let base = base_opt.map(str::to_owned);
                vec![UiCommand::Review { agent: self.agent.clone(), review: self.review_mode, base, check: None }]
            }
            ("sandbox", None, None) => {
                self.input.take_and_push_history();
                let label = self.sandbox_mode.as_str();
                self.set_toast(format!("Sandbox: {label}"), false);
                Vec::new()
            }
            ("sandbox", Some(mode_str), None) => {
                self.input.take_and_push_history();
                match SandboxMode::parse(mode_str) {
                    Ok(mode) => {
                        self.sandbox_mode = mode;
                        let label = mode.as_str();
                        self.push_event(EventLevel::Info, format!("Sandbox set to {label}."));
                        self.set_toast(format!("Sandbox: {label}"), false);
                    }
                    Err(e) => self.set_toast(e, true),
                }
                Vec::new()
            }
            ("staged", None, None) => {
                self.input.take_and_push_history();
                self.sandbox_mode =
                    if self.sandbox_mode.is_staged() { SandboxMode::Direct } else { SandboxMode::Staged };
                let label = self.sandbox_mode.as_str();
                self.push_event(EventLevel::Info, format!("Sandbox switched to {label}."));
                self.set_toast(format!("Sandbox: {label}"), false);
                Vec::new()
            }
            ("staged", Some(mode @ ("on" | "off")), None) => {
                self.input.take_and_push_history();
                self.sandbox_mode = if mode == "on" { SandboxMode::Staged } else { SandboxMode::Direct };
                let label = self.sandbox_mode.as_str();
                self.push_event(EventLevel::Info, format!("Sandbox set to {label}."));
                self.set_toast(format!("Sandbox: {label}"), false);
                Vec::new()
            }
            ("wb", None, None) => {
                self.input.take_and_push_history();
                self.inspector_tab = InspectorTab::Board;
                self.inspector_scroll = 0;
                Vec::new()
            }
            ("wb", Some(_), _) => self.execute_wb_slash(args),
            ("stop", None, None) => {
                self.input.take_and_push_history();
                vec![UiCommand::StopDaemon]
            }
            ("model", _, _) => self.execute_model_slash(args),
            ("provider", _, _) => self.execute_provider_slash(args),
            ("quit" | "exit", _, _) => self.redirect_hint("Press Ctrl+Q to leave the TUI (daemon keeps running)."),
            ("help", _, _) => self.redirect_hint("Press F1 (or ? outside composer) for the shortcuts reference."),
            ("tasks", _, _) => self.redirect_hint("Press [ or Ctrl+B to browse Sessions in the sidebar."),
            ("members", _, _) => self.redirect_hint("Press ] in Sidebar or use /to to pick an agent."),
            ("approve" | "deny", _, _) => {
                self.redirect_hint("Press Ctrl+P for approvals queue, or a/d on a selected task.")
            }
            ("cancel", _, _) => self.redirect_hint("Select an active session or task and press x to cancel."),
            ("delete", _, _) => self.redirect_hint("Select a session in Sidebar/Inspector and press Del twice."),
            _ => {
                let msg = format!("Unknown command /{op}; type / for commands or F1 for shortcuts.");
                self.push_event(EventLevel::Error, &msg);
                self.set_toast(msg, true);
                Vec::new()
            }
        }
    }

    fn redirect_hint(&mut self, msg: &str) -> Vec<UiCommand> {
        self.input.clear();
        self.set_toast(msg, false);
        Vec::new()
    }

    fn execute_wb_slash(&mut self, args: &str) -> Vec<UiCommand> {
        let Some(root_id) = self.selected_root_task().map(|r| r.id.clone()) else {
            self.set_toast("Select a session first before writing to its whiteboard.", true);
            return Vec::new();
        };
        let trimmed = args.trim();
        let mut tokens = trimmed.split_whitespace();
        let Some(first) = tokens.next() else {
            return Vec::new();
        };
        let parse_kind_key = |raw: &str| -> (Option<String>, String) {
            if let Some((k, rest)) = raw.split_once(':') {
                if matches!(k, "note" | "contract" | "claim" | "question" | "decision") && !rest.is_empty() {
                    return (Some(k.to_owned()), rest.to_owned());
                }
            }
            (None, raw.to_owned())
        };
        let body_after = |verb: Option<&str>, raw_key: &str| -> String {
            let after_verb = match verb {
                Some(v) => trimmed.strip_prefix(v).unwrap_or(trimmed).trim_start(),
                None => trimmed,
            };
            after_verb.strip_prefix(raw_key).unwrap_or("").trim().to_owned()
        };
        let cmd = match first {
            "get" => {
                self.input.take_and_push_history();
                self.inspector_tab = InspectorTab::Board;
                self.inspector_scroll = 0;
                return Vec::new();
            }
            "resolve" => {
                let Some(raw_key) = tokens.next() else {
                    self.set_toast("Usage: /wb resolve KEY [NOTE]", true);
                    return Vec::new();
                };
                let (kind, key) = parse_kind_key(raw_key);
                let body = body_after(Some("resolve"), raw_key);
                let content = (!body.is_empty()).then_some(body);
                UiCommand::WbWrite { root_id, key, mode: "resolve".into(), kind, content }
            }
            "delete" | "del" => {
                let Some(raw_key) = tokens.next() else {
                    self.set_toast("Usage: /wb delete KEY", true);
                    return Vec::new();
                };
                let (_, key) = parse_kind_key(raw_key);
                UiCommand::WbWrite { root_id, key, mode: "delete".into(), kind: None, content: None }
            }
            "append" => {
                let Some(raw_key) = tokens.next() else {
                    self.set_toast("Usage: /wb append KEY TEXT", true);
                    return Vec::new();
                };
                let (kind, key) = parse_kind_key(raw_key);
                let body = body_after(Some("append"), raw_key);
                if body.is_empty() {
                    self.set_toast("Usage: /wb append KEY TEXT", true);
                    return Vec::new();
                }
                UiCommand::WbWrite { root_id, key, mode: "append".into(), kind, content: Some(body) }
            }
            "set" => {
                let Some(raw_key) = tokens.next() else {
                    self.set_toast("Usage: /wb set [kind:]KEY TEXT", true);
                    return Vec::new();
                };
                let (kind, key) = parse_kind_key(raw_key);
                let body = body_after(Some("set"), raw_key);
                if body.is_empty() {
                    self.set_toast("Usage: /wb set [kind:]KEY TEXT", true);
                    return Vec::new();
                }
                UiCommand::WbWrite { root_id, key, mode: "set".into(), kind, content: Some(body) }
            }
            raw_key => {
                let (kind, key) = parse_kind_key(raw_key);
                let body = body_after(None, raw_key);
                if body.is_empty() {
                    self.set_toast("Usage: /wb [set|append|resolve|del] [kind:]KEY TEXT", true);
                    return Vec::new();
                }
                UiCommand::WbWrite { root_id, key, mode: "set".into(), kind, content: Some(body) }
            }
        };
        self.input.take_and_push_history();
        self.inspector_tab = InspectorTab::Board;
        vec![cmd]
    }

    fn parse_checked_effort(&mut self, effort: &str) -> Option<Option<String>> {
        if effort != "default" && !daemon::EFFORTS.contains(&effort) {
            let msg = format!("Effort must be default or one of {}", daemon::EFFORTS.join(", "));
            self.set_toast(msg, true);
            return None;
        }
        Some((effort != "default").then(|| effort.to_owned()))
    }

    fn execute_model_slash(&mut self, args: &str) -> Vec<UiCommand> {
        let words: Vec<_> = args.split_whitespace().collect();
        let usage = "Use /model, /model list, /model providers, /model provider NAME [MODEL [EFFORT]], /model MODEL [EFFORT], /model effort EFFORT, or /model default";
        match words.as_slice() {
            [] | ["list"] => {
                self.input.take_and_push_history();
                self.open_model_modal(false)
            }
            ["providers"] => {
                self.input.take_and_push_history();
                let active = self.model.as_ref().map_or("unknown", |m| m.provider.as_str());
                let summary = format_provider_list_lines(
                    active,
                    self.providers_overridden,
                    self.providers_config_path.as_deref(),
                    &self.providers,
                );
                for line in summary.lines() {
                    self.push_event(EventLevel::Info, line);
                }
                self.open_model_modal(true)
            }
            ["default"] => {
                self.input.take_and_push_history();
                vec![UiCommand::SetModel { provider: None, model: Some(None), effort: Some(None) }]
            }
            ["effort", effort] => {
                let Some(eff) = self.parse_checked_effort(effort) else {
                    return Vec::new();
                };
                self.input.take_and_push_history();
                vec![UiCommand::SetModel { provider: None, model: None, effort: Some(eff) }]
            }
            ["provider", target] => {
                self.input.take_and_push_history();
                let p = (*target != "default").then(|| (*target).to_owned());
                vec![UiCommand::SetModel { provider: Some(p), model: None, effort: None }]
            }
            ["provider", target, name] => {
                self.input.take_and_push_history();
                let p = (*target != "default").then(|| (*target).to_owned());
                let m = (*name != "default").then(|| (*name).to_owned());
                vec![UiCommand::SetModel { provider: Some(p), model: Some(m), effort: Some(None) }]
            }
            ["provider", target, name, effort] => {
                let Some(eff) = self.parse_checked_effort(effort) else {
                    return Vec::new();
                };
                self.input.take_and_push_history();
                let p = (*target != "default").then(|| (*target).to_owned());
                let m = (*name != "default").then(|| (*name).to_owned());
                vec![UiCommand::SetModel { provider: Some(p), model: Some(m), effort: Some(eff) }]
            }
            ["provider" | "providers" | "effort" | "list" | "default", ..] => {
                self.set_toast(usage, true);
                Vec::new()
            }
            rest => {
                let rest = rest.strip_prefix(&["set"]).unwrap_or(rest);
                let (name, effort) = match rest {
                    [name] => (*name, None),
                    [name, effort] => {
                        let Some(eff) = self.parse_checked_effort(effort) else {
                            return Vec::new();
                        };
                        (*name, Some(eff))
                    }
                    _ => {
                        self.set_toast(usage, true);
                        return Vec::new();
                    }
                };
                self.input.take_and_push_history();
                let m = (name != "default").then(|| name.to_owned());
                vec![UiCommand::SetModel { provider: None, model: Some(m), effort: Some(effort.flatten()) }]
            }
        }
    }

    fn execute_provider_slash(&mut self, args: &str) -> Vec<UiCommand> {
        let words: Vec<_> = args.split_whitespace().collect();
        match words.as_slice() {
            [] | ["list"] => self.execute_model_slash("providers"),
            ["use", rest @ ..] if !rest.is_empty() => {
                let joined = format!("provider {}", rest.join(" "));
                self.execute_model_slash(&joined)
            }
            ["add", rest @ ..] => match parse_provider_add_tokens(rest) {
                Ok((name, spec, set_default)) => {
                    self.input.take_and_push_history();
                    vec![UiCommand::AddProvider { name, spec, set_default }]
                }
                Err(e) => {
                    self.set_toast(e, true);
                    Vec::new()
                }
            },
            ["remove" | "rm", name] if !name.trim().is_empty() => {
                self.input.take_and_push_history();
                vec![UiCommand::RemoveProvider((*name).to_owned())]
            }
            ["default", target] if !target.trim().is_empty() => {
                self.input.take_and_push_history();
                vec![UiCommand::SetDefaultProvider((*target != "auto").then(|| (*target).to_owned()))]
            }
            _ => {
                self.set_toast(
                    "Use /provider [list|use NAME [MODEL [EFFORT]]|add NAME ...|remove NAME|default NAME|auto]",
                    true,
                );
                Vec::new()
            }
        }
    }
}

fn edit_text_input(input: &mut TextInput, key: KeyEvent, ctrl: bool) {
    match key.code {
        KeyCode::Backspace => {
            if ctrl {
                input.delete_word_back();
            } else {
                input.backspace();
            }
        }
        KeyCode::Delete => input.delete(),
        KeyCode::Left => {
            if ctrl {
                input.move_word_left();
            } else {
                input.move_left();
            }
        }
        KeyCode::Right => {
            if ctrl {
                input.move_word_right();
            } else {
                input.move_right();
            }
        }
        KeyCode::Home => input.move_home(),
        KeyCode::End => input.move_end(),
        KeyCode::Char('a') if ctrl => input.move_home(),
        KeyCode::Char('e') if ctrl => input.move_end(),
        KeyCode::Char('w') if ctrl => input.delete_word_back(),
        KeyCode::Char('u') if ctrl => input.clear_to_start(),
        KeyCode::Char(c) if !ctrl => input.insert_char(c),
        _ => {}
    }
}

fn local_hms() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as libc::time_t).unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `secs` and `&mut tm` are valid stack values for `localtime_r`.
    let ptr = unsafe { libc::localtime_r(&secs, &mut tm) };
    if ptr.is_null() {
        return "--:--:--".into();
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn click(col: u16, row: u16) -> MouseEvent {
        MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: col, row, modifiers: KeyModifiers::NONE }
    }

    #[test]
    fn rebuild_tree_orders_newest_root_first_stays_blank_on_open_and_supports_collapse() {
        let mut state = AppState::new("repo".into(), "default".into(), false, SandboxMode::Direct);
        let r1 = TaskRecord::from_value(&json!({"id":"r1","agent":"default","status":"done","prompt":"old"})).unwrap();
        let c1 =
            TaskRecord::from_value(&json!({"id":"c1","root":"r1","parent":"r1","agent":"a","status":"done"})).unwrap();
        let r2 =
            TaskRecord::from_value(&json!({"id":"r2","agent":"default","status":"running","prompt":"new"})).unwrap();
        let cmds = state.apply_backend_event(BackendEvent::Snapshot {
            tasks: Ok(vec![r1, c1, r2]),
            members: Ok(vec![Member { name: "default".into() }]),
            model: Ok(ModelConfig { provider: "api".into(), model: Some("m".into()), effort: None }),
            providers: None,
        });
        assert_eq!(state.tree_rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(), vec!["r2", "r1", "c1"]);
        // Opening with historical sessions must stay in the fresh New Session view.
        assert_eq!(state.selected_task_id, None);
        assert!(cmds.is_empty());

        // Plain input in New Session starts a new root even if another session existed.
        state.input.set_text("fresh prompt");
        let cmds = state.handle_key(key(KeyCode::Enter));
        assert_eq!(
            cmds,
            vec![UiCommand::Submit {
                agent: "default".into(),
                text: "fresh prompt".into(),
                review: false,
                sandbox: SandboxMode::Direct,
                check: None
            }]
        );

        // Moving selection from New Session picks the newest root row first.
        state.focus = Focus::Sidebar;
        let cmds = state.handle_key(key(KeyCode::Down));
        assert_eq!(state.selected_task_id.as_deref(), Some("r2"));
        assert!(cmds.contains(&UiCommand::FetchDetail("r2".into())));

        state.collapsed_roots.insert("r1".into());
        state.rebuild_tree();
        assert_eq!(state.tree_rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(), vec!["r2", "r1"]);
    }

    #[test]
    fn two_step_delete_arms_disarms_expires_and_confirms() {
        let mut state = AppState::new("repo".into(), "default".into(), false, SandboxMode::Direct);
        let r1 =
            TaskRecord::from_value(&json!({"id":"r1","agent":"default","status":"done","prompt":"First"})).unwrap();
        let r2 =
            TaskRecord::from_value(&json!({"id":"r2","agent":"default","status":"done","prompt":"Second"})).unwrap();
        state.apply_backend_event(BackendEvent::Snapshot {
            tasks: Ok(vec![r1, r2]),
            members: Ok(vec![Member { name: "default".into() }]),
            model: Ok(ModelConfig { provider: "api".into(), model: None, effort: None }),
            providers: None,
        });

        // Composer Del edits text, never arms session delete.
        state.input.set_text("ab");
        state.input.move_left();
        assert!(state.handle_key(key(KeyCode::Delete)).is_empty());
        assert!(state.armed_delete.is_none());
        assert_eq!(state.input.as_str(), "a");

        // Select newest session (r2), then first Del arms delete; moving selection disarms it.
        state.focus = Focus::Sidebar;
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.selected_task_id.as_deref(), Some("r2"));
        assert!(state.handle_key(key(KeyCode::Delete)).is_empty());
        assert_eq!(state.armed_delete.as_ref().map(|d| d.root_id.as_str()), Some("r2"));
        assert!(state.toast.as_ref().is_some_and(|t| t.text.contains("Press Del again")));

        state.handle_key(key(KeyCode::Down));
        assert!(state.armed_delete.is_none());

        // Arm on r1 and confirm with second Del -> emits UiCommand::Delete("r1").
        assert!(state.handle_key(key(KeyCode::Delete)).is_empty());
        assert_eq!(state.armed_delete.as_ref().map(|d| d.root_id.as_str()), Some("r1"));
        let cmds = state.handle_key(key(KeyCode::Delete));
        assert_eq!(cmds, vec![UiCommand::Delete("r1".into())]);
        assert!(state.armed_delete.is_none());

        // Arming and letting DELETE_ARM_TICKS elapse automatically disarms.
        state.focus = Focus::Inspector;
        assert!(state.handle_key(key(KeyCode::Delete)).is_empty());
        assert!(state.armed_delete.is_some());
        for _ in 0..DELETE_ARM_TICKS {
            state.advance_tick();
        }
        assert!(state.armed_delete.is_none());
    }

    #[test]
    fn shortcuts_and_slash_commands_are_complementary_and_hitmap_dispatches_clicks() {
        let mut state = AppState::new("repo".into(), "default".into(), false, SandboxMode::Direct);
        let child = TaskRecord::from_value(&json!({
            "id": "c1", "root": "r1", "parent": "r1", "agent": "worker",
            "status": "approval", "approval_id": "appr-42",
            "action": {"op": "edit_file", "path": "src/lib.rs", "old": "a", "new": "b"}
        }))
        .unwrap();
        state.apply_backend_event(BackendEvent::Snapshot {
            tasks: Ok(vec![child]),
            members: Ok(vec![]),
            model: Ok(ModelConfig { provider: "api".into(), model: None, effort: None }),
            providers: None,
        });
        state.selected_task_id = Some("c1".into());

        // Removed overlapping shortcuts no longer fire.
        state.handle_key(ctrl_key('o'));
        assert_eq!(state.overlay, Overlay::None);
        state.handle_key(ctrl_key('b'));
        assert!(!state.sidebar_visible);
        state.handle_key(ctrl_key('b'));
        assert!(state.sidebar_visible);

        // /review and /staged work as configuration slash commands.
        state.input.set_text("/review on");
        state.handle_key(key(KeyCode::Enter));
        assert!(state.review_mode);
        state.input.set_text("/review off");
        state.handle_key(key(KeyCode::Enter));
        assert!(!state.review_mode);
        state.input.set_text("/staged on");
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.sandbox_mode, SandboxMode::Staged);
        state.input.set_text("/staged off");
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.sandbox_mode, SandboxMode::Direct);

        // /wb without args switches to Board tab.
        state.input.set_text("/wb");
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.inspector_tab, InspectorTab::Board);

        // Removed slash command gives a non-destructive shortcut hint.
        state.input.set_text("/quit");
        let cmds = state.handle_key(key(KeyCode::Enter));
        assert!(cmds.is_empty());
        assert!(!state.should_quit);
        assert!(state.toast.as_ref().is_some_and(|t| t.text.contains("Ctrl+Q")));

        // Mouse hit-testing dispatches exact tab/button targets regardless of pane width.
        let mut hits = HitMap::default();
        hits.push(Rect::new(0, 0, 120, 30), HitTarget::InspectorPane);
        hits.push(Rect::new(82, 0, 10, 1), HitTarget::InspectorTab(InspectorTab::Evidence));
        hits.push(Rect::new(10, 8, 12, 1), HitTarget::InlineApprove("appr-42".into()));

        state.handle_mouse(click(85, 0), &hits);
        assert_eq!(state.inspector_tab, InspectorTab::Evidence);
        let cmds = state.handle_mouse(click(15, 8), &hits);
        assert_eq!(cmds, vec![UiCommand::Approve { approval_id: "appr-42".into(), allow: true }]);
    }

    #[test]
    fn stale_clarification_refuses_submission_without_clearing_composer() {
        let mut state = AppState::new("repo".into(), "default".into(), false, SandboxMode::Direct);
        let q1 =
            TaskRecord::from_value(&json!({"id":"r1","agent":"default","status":"input","question":"Which?"})).unwrap();
        state.selected_task_id = Some("r1".into());
        state.apply_backend_event(BackendEvent::Snapshot {
            tasks: Ok(vec![q1]),
            members: Ok(vec![]),
            model: Ok(ModelConfig { provider: "api".into(), model: None, effort: None }),
            providers: None,
        });
        let closed = TaskRecord::from_value(&json!({"id":"r1","agent":"default","status":"queued"})).unwrap();
        state.apply_backend_event(BackendEvent::Snapshot {
            tasks: Ok(vec![closed]),
            members: Ok(vec![]),
            model: Ok(ModelConfig { provider: "api".into(), model: None, effort: None }),
            providers: None,
        });

        state.input.set_text("Linux");
        let cmds = state.handle_key(key(KeyCode::Enter));
        assert!(cmds.is_empty());
        assert_eq!(state.input.as_str(), "Linux");
        assert!(state.toast.as_ref().is_some_and(|t| t.is_error));

        // Resetting to New Session via Ctrl+N submits the draft as a brand-new root.
        state.handle_key(ctrl_key('n'));
        assert_eq!(state.selected_task_id, None);
        let cmds = state.handle_key(key(KeyCode::Enter));
        assert_eq!(
            cmds,
            vec![UiCommand::Submit {
                agent: "default".into(),
                text: "Linux".into(),
                review: false,
                sandbox: SandboxMode::Direct,
                check: None
            }]
        );
    }

    #[test]
    fn composer_continues_sole_leaf_or_summarizer_and_ctrl_n_resets_to_new_session() {
        let mut state = AppState::new("repo".into(), "default".into(), false, SandboxMode::Direct);
        let r1 = TaskRecord::from_value(&json!({
            "id": "r1", "root": "r1", "agent": "default", "status": "done", "kind": "root",
            "prompt": "Round 1", "result": "Summarized",
            "continuation_node": "sum-1", "continuation_agent": "summarizer", "continuation_kind": "summarizer"
        }))
        .unwrap();
        let sum = TaskRecord::from_value(&json!({
            "id": "sum-1", "root": "r1", "parent": "r1", "agent": "summarizer-1",
            "status": "done", "kind": "summarizer"
        }))
        .unwrap();
        state.apply_backend_event(BackendEvent::Snapshot {
            tasks: Ok(vec![r1, sum]),
            members: Ok(vec![Member { name: "default".into() }]),
            model: Ok(ModelConfig { provider: "api".into(), model: None, effort: None }),
            providers: None,
        });
        assert_eq!(state.selected_task_id, None);

        // Select the root session and send follow-up input -> continues on sum-1.
        state.selected_task_id = Some("r1".into());
        state.input.set_text("follow up");
        let cmds = state.handle_key(key(KeyCode::Enter));
        assert_eq!(
            cmds,
            vec![UiCommand::Continue {
                id: "sum-1".into(),
                text: "follow up".into(),
                review: false,
                sandbox: SandboxMode::Direct,
                check: None
            }]
        );

        // Also test /wb <kind>:<key> <text> emits WbWrite for the selected session root.
        state.input.set_text("/wb decision:api Use JSON");
        let wb_cmds = state.handle_key(key(KeyCode::Enter));
        assert_eq!(
            wb_cmds,
            vec![UiCommand::WbWrite {
                root_id: "r1".into(),
                key: "api".into(),
                mode: "set".into(),
                kind: Some("decision".into()),
                content: Some("Use JSON".into()),
            }]
        );

        // /new with no arguments resets back to blank New Session.
        state.input.set_text("/new");
        assert!(state.handle_key(key(KeyCode::Enter)).is_empty());
        assert_eq!(state.selected_task_id, None);
    }

    #[test]
    fn model_modal_and_slash_commands_switch_providers_and_manage_custom_providers() {
        let mut state = AppState::new("proj".into(), "default".into(), false, SandboxMode::Direct);
        let providers = vec![
            ProviderSummary {
                name: "chatgpt".into(),
                wire: "chatgpt".into(),
                builtin: true,
                base_url: None,
                api_key_env: None,
                default_model: None,
                context_window: None,
                tool_choice: "required".into(),
                ready: true,
                status: "signed in".into(),
            },
            ProviderSummary {
                name: "deepseek".into(),
                wire: "responses".into(),
                builtin: true,
                base_url: Some("https://api.deepseek.com/v1".into()),
                api_key_env: Some("DEEPSEEK_API_KEY".into()),
                default_model: Some("deepseek-flash".into()),
                context_window: Some(131_072),
                tool_choice: "auto".into(),
                ready: true,
                status: "env set".into(),
            },
        ];
        state.apply_backend_event(BackendEvent::Snapshot {
            tasks: Ok(vec![]),
            members: Ok(vec![]),
            model: Ok(ModelConfig {
                provider: "chatgpt".into(),
                model: Some("gpt-5".into()),
                effort: Some("high".into()),
            }),
            providers: Some(("chatgpt".into(), false, None, providers)),
        });

        // Opening modal without changing provider emits LoadModels("chatgpt") and saves with provider: None.
        state.input.set_text("/model");
        let open_cmds = state.handle_key(key(KeyCode::Enter));
        assert_eq!(open_cmds, vec![UiCommand::LoadModels(Some("chatgpt".into()))]);
        let save_same = state.handle_key(key(KeyCode::Enter));
        assert_eq!(
            save_same,
            vec![UiCommand::SetModel {
                provider: None,
                model: Some(Some("gpt-5".into())),
                effort: Some(Some("high".into()))
            }]
        );

        // /model providers focuses ProviderBar; moving Right switches to deepseek, pre-fills default_model,
        // triggers LoadModels("deepseek"), and Enter commits SetModel with provider: Some(Some("deepseek")).
        state.input.set_text("/model providers");
        let cmds = state.handle_key(key(KeyCode::Enter));
        assert_eq!(cmds, vec![UiCommand::LoadModels(Some("chatgpt".into()))]);
        let switch_cmds = state.handle_key(key(KeyCode::Right));
        assert_eq!(switch_cmds, vec![UiCommand::LoadModels(Some("deepseek".into()))]);
        let save_switched = state.handle_key(key(KeyCode::Enter));
        assert_eq!(
            save_switched,
            vec![UiCommand::SetModel {
                provider: Some(Some("deepseek".into())),
                model: Some(Some("deepseek-flash".into())),
                effort: Some(Some("high".into())),
            }]
        );

        // Slash /model provider & /provider add / use / remove / default dispatch expected commands.
        state.input.set_text("/model provider deepseek deepseek-chat medium");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            vec![UiCommand::SetModel {
                provider: Some(Some("deepseek".into())),
                model: Some(Some("deepseek-chat".into())),
                effort: Some(Some("medium".into())),
            }]
        );
        state.input.set_text("/provider use chatgpt");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            vec![UiCommand::SetModel { provider: Some(Some("chatgpt".into())), model: None, effort: None }]
        );
        state.input.set_text("/provider add local-vllm chat_completions http://127.0.0.1:8000/v1 - qwen-coder");
        let add_cmds = state.handle_key(key(KeyCode::Enter));
        assert_eq!(add_cmds.len(), 1);
        assert!(matches!(
            &add_cmds[0],
            UiCommand::AddProvider { name, spec, set_default: false }
                if name == "local-vllm"
                    && spec.wire == "chat_completions"
                    && spec.api_key_env.is_none()
                    && spec.default_model.as_deref() == Some("qwen-coder")
        ));
        state.input.set_text("/provider remove local-vllm");
        assert_eq!(state.handle_key(key(KeyCode::Enter)), vec![UiCommand::RemoveProvider("local-vllm".into())]);
        state.input.set_text("/provider default auto");
        assert_eq!(state.handle_key(key(KeyCode::Enter)), vec![UiCommand::SetDefaultProvider(None)]);
    }

    #[test]
    fn codex_slash_commands_steering_and_at_mentions_work() {
        let mut state = AppState::new("repo".into(), "default".into(), false, SandboxMode::Direct);

        // /sandbox switches modes including read-only.
        state.input.set_text("/sandbox read-only");
        assert!(state.handle_key(key(KeyCode::Enter)).is_empty());
        assert_eq!(state.sandbox_mode, SandboxMode::ReadOnly);

        // /plan forces SandboxMode::ReadOnly even if toggled back to Direct.
        state.input.set_text("/sandbox direct");
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.sandbox_mode, SandboxMode::Direct);
        state.input.set_text("/plan inspect architecture");
        let plan_cmds = state.handle_key(key(KeyCode::Enter));
        assert!(matches!(
            plan_cmds.as_slice(),
            [UiCommand::Submit { sandbox: SandboxMode::ReadOnly, text, .. }]
                if text.contains("inspect architecture") && text.contains("update_plan")
        ));

        // /review code, /diff, /skills, /mcp, /status work via slash commands.
        state.input.set_text("/review code main");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            vec![UiCommand::Review { agent: "default".into(), review: false, base: Some("main".into()), check: None }]
        );
        state.input.set_text("/diff HEAD~1");
        assert_eq!(state.handle_key(key(KeyCode::Enter)), vec![UiCommand::LoadDiff { base: Some("HEAD~1".into()) }]);
        state.input.set_text("/skills");
        assert_eq!(state.handle_key(key(KeyCode::Enter)), vec![UiCommand::LoadSkills]);
        state.input.set_text("/mcp");
        assert_eq!(state.handle_key(key(KeyCode::Enter)), vec![UiCommand::LoadMcp]);
        state.input.set_text("/status");
        assert!(state.handle_key(key(KeyCode::Enter)).is_empty());
        assert!(matches!(&state.overlay, Overlay::TextView { title, is_diff: false, .. } if title == "Session Status"));
        state.handle_key(key(KeyCode::Esc));
        assert_eq!(state.overlay, Overlay::None);

        // Selecting an active non-legacy root switches composer to mid-turn steering and /compact works.
        let active = TaskRecord::from_value(&json!({
            "id": "r1", "root": "r1", "agent": "default", "status": "running", "kind": "root"
        }))
        .unwrap();
        state.apply_backend_event(BackendEvent::Snapshot {
            tasks: Ok(vec![active]),
            members: Ok(vec![]),
            model: Ok(ModelConfig { provider: "api".into(), model: None, effort: None }),
            providers: None,
        });
        state.selected_task_id = Some("r1".into());
        state.input.set_text("focus on unit tests");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            vec![UiCommand::Steer { id: "r1".into(), text: "focus on unit tests".into() }]
        );
        state.input.set_text("/compact");
        assert_eq!(state.handle_key(key(KeyCode::Enter)), vec![UiCommand::Compact("r1".into())]);

        // @file fuzzy completion accepts matching workspace file on Tab.
        state.apply_backend_event(BackendEvent::FilesLoaded(vec![
            "Cargo.toml".into(),
            "src/tui/state.rs".into(),
            "src/main.rs".into(),
        ]));
        state.input.set_text("check @tstate");
        assert!(state.active_mention().is_some());
        assert!(state.handle_key(key(KeyCode::Tab)).is_empty());
        assert_eq!(state.input.as_str(), "check @src/tui/state.rs ");
    }
}
