//! Read-only workspace review picker. Requests carry a view generation so late
//! replies cannot reopen a dismissed review or replace a newer file selection.

use crate::app::Effect;
use crate::i18n::tr;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value as Json;

#[derive(Debug)]
pub struct Review {
    pub agent: String,
    pub generation: u64,
    pub report: Option<Json>,
    pub error: Option<String>,
    pub selected: usize,
    pub scroll: usize,
    pub horizontal: usize,
    pub path: Option<String>,
    pub offset: usize,
    pub loading: bool,
    pub closed: bool,
    pub info: bool,
}

impl Review {
    pub fn new(agent: String, generation: u64) -> Self {
        Self {
            agent,
            generation,
            report: None,
            error: None,
            selected: 0,
            scroll: 0,
            horizontal: 0,
            path: None,
            offset: 0,
            loading: true,
            closed: false,
            info: false,
        }
    }

    pub fn request(&mut self) -> Effect {
        self.loading = true;
        self.error = None;
        if let Some(report) = &mut self.report {
            report["detail"] = Json::Null;
        }
        self.generation += 1;
        Effect::Review {
            agent_id: self.agent.clone(),
            path: self.path.clone(),
            offset: self.offset,
            generation: self.generation,
            revision: self.report.as_ref().and_then(|r| r["revision"].as_str()).map(str::to_string),
        }
    }

    pub fn apply(&mut self, generation: u64, result: Result<Json, String>) {
        if generation != self.generation || self.closed {
            return;
        }
        self.loading = false;
        match result {
            Ok(report) => {
                self.report = Some(report);
                self.selected = self.selected.min(self.changes().len().saturating_sub(1));
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn changes(&self) -> &[Json] {
        self.report.as_ref().and_then(|r| r["changes"].as_array()).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            (KeyCode::Char('q'), true) => return vec![Effect::Quit],
            (KeyCode::Char('g'), true) => {
                self.closed = true;
            }
            (KeyCode::Esc, _) | (KeyCode::Char('q'), false) => {
                if self.info {
                    self.info = false;
                    self.scroll = 0;
                    self.horizontal = 0;
                    return vec![];
                }
                self.generation += 1;
                if self.path.take().is_some() {
                    self.scroll = 0;
                    self.horizontal = 0;
                    self.loading = false;
                } else {
                    self.closed = true;
                }
            }
            (KeyCode::Char('i'), false) => {
                self.info = !self.info;
                self.scroll = 0;
                self.horizontal = 0;
            }
            (KeyCode::Char('r'), false) if !self.loading => {
                self.scroll = 0;
                self.offset = 0;
                if let Some(report) = &mut self.report {
                    report["revision"] = Json::Null;
                }
                return vec![self.request()];
            }
            (KeyCode::Enter, false) if self.path.is_none() && !self.loading && !self.info => {
                if let Some(path) = self.changes().get(self.selected).and_then(|c| c["path"].as_str()) {
                    self.path = Some(path.to_string());
                    self.offset = 0;
                    self.scroll = 0;
                    self.horizontal = 0;
                    return vec![self.request()];
                }
            }
            (KeyCode::Char('n'), false) if self.path.is_some() && !self.loading && !self.info => {
                if let Some(offset) = self.report.as_ref().and_then(|r| r["detail"]["next_offset"].as_u64()) {
                    self.offset = offset as usize;
                    self.scroll = 0;
                    return vec![self.request()];
                }
            }
            (KeyCode::Char('p'), false) if self.path.is_some() && !self.loading && !self.info => {
                self.offset = self.offset.saturating_sub(120);
                self.scroll = 0;
                return vec![self.request()];
            }
            (KeyCode::Left, _) => self.horizontal = self.horizontal.saturating_sub(8),
            (KeyCode::Right, _) => self.horizontal = self.horizontal.saturating_add(8).min(4000),
            (KeyCode::Down, _) if self.path.is_none() && !self.info => {
                self.selected = (self.selected + 1).min(self.changes().len().saturating_sub(1))
            }
            (KeyCode::Up, _) if self.path.is_none() && !self.info => self.selected = self.selected.saturating_sub(1),
            (KeyCode::Down, _) | (KeyCode::Char('d'), true) | (KeyCode::PageDown, _) => {
                self.scroll = (self.scroll + 5).min(if self.info {
                    self.header("en").len() + self.detail_metadata().len() + 1
                } else {
                    120
                })
            }
            (KeyCode::Up, _) | (KeyCode::Char('u'), true) | (KeyCode::PageUp, _) => {
                self.scroll = self.scroll.saturating_sub(5)
            }
            _ => {}
        }
        vec![]
    }

    pub fn header(&self, lang: &str) -> Vec<String> {
        let mut lines = vec![];
        if let Some(error) = &self.error {
            lines.push(format!("{}: {}", tr(lang, "读取异常", &[]), safe(error)));
        }
        if self.loading {
            lines.push(tr(lang, "正在读取工作区差异…", &[]));
        }
        if let Some(report) = &self.report {
            if report["complete"] != true || report["detail"]["truncated"] == true {
                lines.push(tr(lang, "警告：审查不完整，请核对限制或错误", &[]));
            }
            if report["shared"] == true {
                lines.push(tr(lang, "共享工作区：包含其他成员与用户改动，不归属于单个成员", &[]));
            }
            lines.push(format!("{}: {}", tr(lang, "工作目录", &[]), safe(report["root"].as_str().unwrap_or(""))));
            lines.push(format!(
                "{} · {}",
                tr(lang, "基线：首次观察快照（包括原有未提交改动）", &[]),
                crate::app::fmt_ts(report["baseline_at"].as_f64().unwrap_or(0.0), true)
            ));
            let scope = match report["scope"].as_str() {
                Some("git_tracked_and_unignored") => {
                    tr(lang, "Git 已跟踪与未忽略文件（不含 Git 元数据和会话私有状态）", &[])
                }
                Some("files_except_build_and_dependency_directories") => {
                    tr(lang, "非 Git 文件（排除构建、依赖目录和会话私有状态）", &[])
                }
                _ => safe(report["scope"].as_str().unwrap_or("?")),
            };
            lines.push(format!("{}: {scope}", tr(lang, "范围", &[])));
            if let Some(warnings) = report["warnings"].as_array() {
                lines.extend(warnings.iter().filter_map(Json::as_str).map(safe));
            }
        }
        lines
    }

    pub fn lines(&self, lang: &str, height: usize) -> Vec<(String, bool)> {
        if self.info {
            let mut lines = self.header(lang);
            if let Some(path) = &self.path {
                lines.push(safe(path));
            }
            lines.extend(self.detail_metadata());
            let start = self.scroll.min(lines.len().saturating_sub(height));
            return lines
                .into_iter()
                .skip(start)
                .take(height)
                .map(|line| (line.chars().skip(self.horizontal).collect(), false))
                .collect();
        }
        if self.path.is_some() {
            let detail = self
                .report
                .as_ref()
                .map(|r| &r["detail"])
                .filter(|detail| detail["path"].as_str() == self.path.as_deref());
            let lines = detail.and_then(|d| d["lines"].as_array()).map(Vec::as_slice).unwrap_or(&[]);
            if lines.is_empty() && !self.loading && self.error.is_none() {
                return vec![(
                    tr(
                        lang,
                        if detail.is_some() {
                            "无文本差异；按 i 核对文件元数据"
                        } else {
                            "所选文件未完成读取；按 i 查看限制"
                        },
                        &[],
                    ),
                    false,
                )];
            }
            let start = self.scroll.min(lines.len().saturating_sub(height));
            return lines
                .iter()
                .skip(start)
                .take(height)
                .filter_map(Json::as_str)
                .map(|line| (safe(line).chars().skip(self.horizontal).collect(), false))
                .collect();
        }
        if self.changes().is_empty() && !self.loading && self.error.is_none() {
            return vec![(tr(lang, "声明范围内未检测到变化（不是测试通过证明）", &[]), false)];
        }
        let start = self.selected.saturating_sub(height.saturating_sub(1));
        self.changes()
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(index, change)| {
                let status = safe(change["status"].as_str().unwrap_or("?"));
                let path = safe(change["path"].as_str().unwrap_or("?"));
                (format!("{status:10} {path}").chars().skip(self.horizontal).collect(), index == self.selected)
            })
            .collect()
    }

    pub fn detail_metadata(&self) -> Vec<String> {
        let Some(path) = &self.path else { return vec![] };
        let Some(change) = self.changes().iter().find(|c| c["path"] == *path) else { return vec![] };
        ["before", "after"]
            .iter()
            .flat_map(|side| {
                let entry = &change[side];
                let mut lines = vec![
                    format!(
                        "{side}: {} {} bytes mode={}",
                        safe(entry["kind"].as_str().unwrap_or("absent")),
                        entry["size"],
                        entry["mode"].as_u64().map(|mode| format!("{mode:o}")).unwrap_or_default(),
                    ),
                    format!("sha256={}", safe(entry["hash"].as_str().unwrap_or("?"))),
                ];
                if let Some(problem) = entry["problem"].as_str() {
                    lines.push(safe(problem));
                }
                lines
            })
            .collect()
    }

    pub fn status(&self, lang: &str) -> String {
        if let Some(error) = &self.error {
            return format!("{}: {}", tr(lang, "读取异常", &[]), safe(error));
        }
        if self.loading {
            return tr(lang, "正在读取工作区差异…", &[]);
        }
        if let Some(report) = &self.report {
            if report["complete"] != true || report["detail"]["truncated"] == true {
                return tr(lang, "警告：审查不完整，请核对限制或错误", &[]);
            }
            if report["shared"] == true {
                return tr(lang, "共享工作区：包含其他成员与用户改动，不归属于单个成员", &[]);
            }
        }
        tr(lang, "基线：首次观察快照（包括原有未提交改动）", &[])
    }

    pub fn footer(&self, lang: &str) -> String {
        let hint = tr(lang, "i 详情 · r 刷新 · Esc 返回 · Ctrl+G 批准 · Ctrl+Q 退出", &[]);
        if self.path.is_some() && !self.info {
            let detail = self.report.as_ref().map(|r| &r["detail"]);
            let count = detail.and_then(|d| d["lines"].as_array()).map_or(0, Vec::len);
            let total = detail.and_then(|d| d["total_lines"].as_u64()).unwrap_or(0);
            format!("n/p {}/{} · {hint}", self.offset.saturating_add(count), total)
        } else {
            hint
        }
    }
}

/// Workspace text is untrusted terminal input; escape controls, including ESC.
pub fn safe(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control()
                || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}
