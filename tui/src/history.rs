//! Persisted human history browser. A navigation stack retains page cursors,
//! while generations reject replies from dismissed or superseded selections.

use crate::{app::Effect, i18n::tr, review::safe};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{json, Value as Json};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Default)]
struct Location {
    agent: Option<String>,
    source: Option<String>,
    item: Option<String>,
    offset: usize,
    revision: Option<String>,
    through: Option<i64>,
    cursor: Option<String>,
    previous: Vec<(usize, Option<String>, Option<String>)>,
    report: Option<Json>,
    selected: usize,
    scroll: usize,
}

#[derive(Debug)]
pub struct History {
    location: Location,
    parents: Vec<Location>,
    pub generation: u64,
    pub loading: bool,
    pub closed: bool,
    pub error: Option<String>,
    pub info: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct LocationView<'a> {
    pub agent: Option<&'a str>,
    pub source: Option<&'a str>,
    pub item: Option<&'a str>,
    pub offset: usize,
    pub revision: Option<&'a str>,
    pub through: Option<i64>,
    pub cursor: Option<&'a str>,
}

impl History {
    pub fn new(agent: Option<String>, generation: u64) -> Self {
        Self {
            location: Location { agent, ..Default::default() },
            parents: vec![],
            generation,
            loading: false,
            closed: false,
            error: None,
            info: false,
        }
    }

    pub fn request(&mut self) -> Effect {
        self.generation += 1;
        self.loading = true;
        self.error = None;
        self.location.report = None;
        let l = &self.location;
        Effect::History {
            generation: self.generation,
            params: json!({"agent_id":l.agent,"source":l.source,
            "item":l.item,"offset":l.offset,"revision":l.revision,"through":l.through,"cursor":l.cursor}),
        }
    }

    pub fn apply(&mut self, generation: u64, result: Result<Json, String>) {
        if self.closed || generation != self.generation {
            return;
        }
        self.loading = false;
        match result {
            Ok(report) => {
                let l = &mut self.location;
                if report["agent_id"] != json!(l.agent)
                    || report["source"] != json!(l.source)
                    || report["item"] != json!(l.item)
                    || report["cursor"] != json!(l.cursor)
                {
                    self.error = Some("历史响应与选择不符".into());
                    return;
                }
                l.revision = report["revision"].as_str().map(str::to_string);
                l.through = report["through"].as_i64();
                l.selected = l.selected.min(report["entries"].as_array().map_or(0, |a| a.len().saturating_sub(1)));
                if let Some(warning) = report["warning"].as_str() {
                    self.error = Some(warning.to_string());
                }
                l.report = Some(report);
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn entries(&self) -> &[Json] {
        self.location.report.as_ref().and_then(|r| r["entries"].as_array()).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn location(&self) -> LocationView<'_> {
        LocationView {
            agent: self.location.agent.as_deref(),
            source: self.location.source.as_deref(),
            item: self.location.item.as_deref(),
            offset: self.location.offset,
            revision: self.location.revision.as_deref(),
            through: self.location.through,
            cursor: self.location.cursor.as_deref(),
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            (KeyCode::Char('q'), true) => return vec![Effect::Quit],
            (KeyCode::Char('g'), true) => self.closed = true,
            (KeyCode::Esc, _) | (KeyCode::Char('q'), false) => {
                self.generation += 1;
                if self.info {
                    self.info = false;
                    self.location.scroll = 0;
                } else if let Some(parent) = self.parents.pop() {
                    self.location = parent;
                    self.loading = false;
                    self.error = None;
                } else {
                    self.closed = true;
                }
            }
            (KeyCode::Char('i'), false) => {
                self.info = !self.info;
                self.location.scroll = 0;
            }
            (KeyCode::Char('r'), false) if !self.loading => {
                // An index can mean a different node after a file rewrite.
                // Refresh the containing list, never silently retarget detail.
                if self.location.item.is_some() {
                    if let Some(parent) = self.parents.pop() {
                        self.location = parent;
                    }
                }
                self.location.offset = 0;
                self.location.previous.clear();
                self.location.revision = None;
                self.location.through = None;
                self.location.cursor = None;
                self.location.scroll = 0;
                return vec![self.request()];
            }
            (KeyCode::Enter, false) if !self.loading && !self.info && self.location.item.is_none() => {
                let Some(id) =
                    self.entries().get(self.location.selected).and_then(|e| e["id"].as_str()).map(str::to_string)
                else {
                    return vec![];
                };
                let l = &self.location;
                let next = if l.agent.is_none() {
                    Location { agent: Some(id), ..Default::default() }
                } else if l.source.is_none() {
                    Location { agent: l.agent.clone(), source: Some(id), ..Default::default() }
                } else {
                    Location {
                        agent: l.agent.clone(),
                        source: l.source.clone(),
                        item: Some(id),
                        revision: if matches!(l.source.as_deref(), Some("tree" | "snapshot" | "codex")) {
                            l.revision.clone()
                        } else {
                            None
                        },
                        through: l.through,
                        cursor: l.cursor.clone(),
                        ..Default::default()
                    }
                };
                self.parents.push(std::mem::replace(&mut self.location, next));
                return vec![self.request()];
            }
            (KeyCode::Char('n'), false) if !self.loading && !self.info => {
                if let Some(next) = self.location.report.as_ref().and_then(|r| r["next_offset"].as_u64()) {
                    self.location.previous.push((
                        self.location.offset,
                        self.location.cursor.clone(),
                        self.location.revision.clone(),
                    ));
                    if self.location.source.as_deref() == Some("codex") && self.location.item.is_none() {
                        self.location.cursor = self
                            .location
                            .report
                            .as_ref()
                            .and_then(|report| report["next_cursor"].as_str().map(str::to_string));
                        self.location.revision = None;
                    }
                    self.location.offset = next as usize;
                    self.location.scroll = 0;
                    self.location.selected = 0;
                    return vec![self.request()];
                }
            }
            (KeyCode::Char('p'), false) if !self.loading && !self.info => {
                if let Some((offset, cursor, revision)) = self.location.previous.pop() {
                    self.location.offset = offset;
                    self.location.cursor = cursor;
                    self.location.revision = revision;
                    self.location.scroll = 0;
                    self.location.selected = 0;
                    return vec![self.request()];
                }
            }
            (KeyCode::Down, _) if self.location.item.is_none() && !self.info => {
                self.location.selected = (self.location.selected + 1).min(self.entries().len().saturating_sub(1));
            }
            (KeyCode::Up, _) if self.location.item.is_none() && !self.info => {
                self.location.selected = self.location.selected.saturating_sub(1)
            }
            (KeyCode::Down, _) | (KeyCode::PageDown, _) | (KeyCode::Char('d'), true) => {
                self.location.scroll = self.location.scroll.saturating_add(5).min(100_000)
            }
            (KeyCode::Up, _) | (KeyCode::PageUp, _) | (KeyCode::Char('u'), true) => {
                self.location.scroll = self.location.scroll.saturating_sub(5)
            }
            (KeyCode::Home, _) => {
                self.location.scroll = 0;
                self.location.selected = 0;
            }
            (KeyCode::End, _) if self.location.item.is_some() => self.location.scroll = 100_000,
            _ => {}
        }
        vec![]
    }

    pub fn title(&self, lang: &str) -> String {
        let l = &self.location;
        format!(
            "{}: {} / {} / {}",
            tr(lang, "成员记录", &[]),
            safe(l.agent.as_deref().unwrap_or("*")),
            safe(l.source.as_deref().unwrap_or("*")),
            safe(l.item.as_deref().unwrap_or("*"))
        )
    }

    pub fn status(&self, lang: &str) -> String {
        if let Some(error) = &self.error {
            return format!("{}: {}", tr(lang, "读取异常", &[]), safe(error));
        }
        if self.loading {
            return tr(lang, "正在读取已保存记录…", &[]);
        }
        tr(lang, "只读成员记录；Codex 原生历史按需读取 · i 范围", &[])
    }

    pub fn footer(&self, lang: &str) -> String {
        let next = self.location.report.as_ref().is_some_and(|r| r["next_offset"].is_number());
        format!(
            "{}{} · {}",
            self.location.offset,
            if next { " +" } else { "" },
            tr(lang, "n/p 分页 · ↑↓ 选择/滚动 · Enter 打开 · r 刷新 · Esc 返回", &[])
        )
    }

    pub fn lines(&self, lang: &str, width: usize, height: usize) -> Vec<(String, bool)> {
        let info = [
            "仅供本地用户查看，不注入 Leader，不改变投递或执行状态。",
            "对话树包含所有已保存节点（含回退分支和压缩前原文）；leaf、parent、skip_to 见正文。",
            "线性快照包含最近保存的上下文；活动回合请同时查看检查点，流式未落盘文本不可用。",
            "回合按创建顺序分页，检查点保留实际工具参数与结果；历史快照可能重复，不代表重复执行。",
            "事件按成员作为行为者、payload.agent_id 或历史受众筛选；并非模型权限视图。",
            "Codex 原生记录通过独立只读客户端按成员线程读取，不恢复线程或启动回合；仅展示后端实际提供的内容。",
            "单个历史文件或事件上限 32 MiB；超过上限或损坏会明确报错，请在本地检查原文件。",
            "数据库页面固定新增记录上界；文件变化需刷新。数据库与检查点不是跨文件原子快照。",
        ];
        let content = if self.info {
            info.iter().map(|s| tr(lang, s, &[])).collect::<Vec<_>>().join("\n\n")
        } else if let Some(text) = self.location.report.as_ref().and_then(|r| r["text"].as_str()) {
            text.to_string()
        } else {
            if let Some(warning) = self.location.report.as_ref().and_then(|r| r["warning"].as_str()) {
                return vec![(safe(warning), false)];
            }
            if self.entries().is_empty() && !self.loading && self.error.is_none() {
                return vec![(tr(lang, "本页没有记录；如有 +，可按 n 继续", &[]), false)];
            }
            let start = self.location.selected.saturating_sub(height.saturating_sub(1));
            return self
                .entries()
                .iter()
                .enumerate()
                .skip(start)
                .take(height)
                .map(|(i, e)| {
                    let label = if self.location.source.is_none() && self.location.agent.is_some() {
                        match e["id"].as_str().unwrap_or("") {
                            "tree" => tr(lang, "对话树：全部分支及压缩前原文", &[]),
                            "snapshot" => tr(lang, "线性快照：最近保存的模型上下文", &[]),
                            "turns" => tr(lang, "回合与检查点：包含活动回合的已保存记录", &[]),
                            "codex" => tr(lang, "Codex 原生对话与工具记录", &[]),
                            _ => tr(lang, "团队事件：历史相关事件（非完整工具日志）", &[]),
                        }
                    } else {
                        safe(e["label"].as_str().unwrap_or("?"))
                    };
                    (label, i == self.location.selected)
                })
                .collect();
        };
        // Wrap rather than cap horizontal scrolling: every character of even
        // a single huge JSON/tool line must remain reachable on narrow screens.
        let mut wrapped = Vec::new();
        for line in content.lines() {
            let mut part = String::new();
            let mut used = 0;
            for c in safe(line).chars() {
                let w = c.width().unwrap_or(0);
                if used + w > width.max(2) {
                    wrapped.push(std::mem::take(&mut part));
                    used = 0;
                }
                part.push(c);
                used += w;
            }
            wrapped.push(part);
        }
        let start = self.location.scroll.min(wrapped.len().saturating_sub(height));
        wrapped.into_iter().skip(start).take(height).map(|line| (line, false)).collect()
    }
}
