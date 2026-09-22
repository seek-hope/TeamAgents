//! Session model picker: member -> provider -> configured model -> effort.
use crate::app::Effect;
use crate::i18n::tr;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value as Json;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Member,
    Provider,
    Model,
    Effort,
}

pub struct ModelPicker {
    agents: Vec<Json>,
    profiles: Vec<Json>,
    step: Step,
    agent: Json,
    provider: String,
    profile: Json,
    pub index: usize,
    pub query: String,
    pub closed: bool,
    pub loading: bool,
    pub notice: String,
    pub form: Option<ProviderForm>,
}

const ADD_PROVIDER: &str = "\0add-provider";
const FORM_LABELS: [&str; 6] =
    ["供应商名称", "API 格式", "API 基础地址", "模型 ID", "密钥环境变量名（可留空）", "原生上下文长度（未知留空）"];
const PROTOCOLS: [&str; 3] = ["responses", "anthropic", "chat/completions"];

pub struct ProviderForm {
    pub values: [String; 6],
    pub field: usize,
    pub saving: bool,
}

impl ProviderForm {
    pub fn rows(&self, lang: &str) -> Vec<String> {
        let mut rows: Vec<_> = FORM_LABELS
            .iter()
            .zip(&self.values)
            .map(|(label, value)| format!("{}: {}", tr(lang, label, &[]), value))
            .collect();
        rows.push(tr(lang, "保存到用户配置", &[]));
        rows
    }

    pub fn hint(&self, lang: &str) -> String {
        tr(
            lang,
            if self.saving {
                "正在保存供应商…"
            } else {
                match self.field {
                    0 => "填写新名称；同名配置不会被覆盖",
                    1 => "←→ 选择 API 格式：responses / anthropic / chat/completions",
                    2 => "例如 https://example.com/v1，不含 /responses 或 /chat/completions",
                    3 => "直接填写模型 ID，无需供应商提供模型列表接口",
                    4 => "只填环境变量名（如 MY_API_KEY）；不要粘贴密钥",
                    5 => "填写模型原生窗口的 token 数；未知时留空",
                    _ => "Enter 保存 · Esc 返回修改",
                }
            },
            &[],
        )
    }

    fn handle_key(&mut self, key: KeyEvent, lang: &str, notice: &mut String) -> Vec<Effect> {
        if self.saving {
            return vec![];
        }
        match key.code {
            KeyCode::Up => self.field = self.field.saturating_sub(1),
            KeyCode::Left | KeyCode::Right if self.field == 1 => {
                let i = PROTOCOLS.iter().position(|p| *p == self.values[1]).unwrap_or(0);
                let offset = if key.code == KeyCode::Left { 2 } else { 1 };
                self.values[1] = PROTOCOLS[(i + offset) % 3].into();
            }
            KeyCode::Enter | KeyCode::Tab | KeyCode::Down if self.field < 6 => {
                if self.field < 4 && self.values[self.field].trim().is_empty() {
                    *notice = tr(lang, "此项不能为空", &[]);
                } else {
                    notice.clear();
                    self.field += 1;
                }
            }
            KeyCode::Enter => {
                let context = self.values[5].trim();
                let context_window = if context.is_empty() {
                    None
                } else {
                    match context.parse::<u64>() {
                        Ok(n) if n > 0 => Some(n),
                        _ => {
                            *notice = tr(lang, "原生上下文长度须为正整数；未知时留空", &[]);
                            self.field = 5;
                            return vec![];
                        }
                    }
                };
                self.saving = true;
                notice.clear();
                return vec![Effect::AddModelProvider {
                    params: serde_json::json!({
                        "name":self.values[0].trim(), "protocol":self.values[1],
                        "base_url":self.values[2].trim(), "model":self.values[3].trim(),
                        "api_key_env":self.values[4].trim(), "context_window":context_window,
                    }),
                }];
            }
            KeyCode::Backspace if self.field < 6 && self.field != 1 => {
                self.values[self.field].pop();
            }
            KeyCode::Char(c)
                if self.field < 6
                    && self.field != 1
                    && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && !c.is_control() =>
            {
                self.values[self.field].push(c);
            }
            _ => {}
        }
        vec![]
    }
}

fn text<'a>(v: &'a Json, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn model_key(v: &Json) -> String {
    serde_json::json!([text(v, "id"), text(v, "model")]).to_string()
}

impl ModelPicker {
    pub fn new(report: &Json) -> Self {
        let mut agents = report["agents"].as_array().cloned().unwrap_or_default();
        agents.sort_by_key(|a| text(a, "agent_id") != text(report, "leader_id"));
        Self {
            agents,
            profiles: report["profiles"].as_array().cloned().unwrap_or_default(),
            step: Step::Member,
            agent: Json::Null,
            provider: String::new(),
            profile: Json::Null,
            index: 0,
            query: String::new(),
            closed: false,
            loading: false,
            notice: String::new(),
            form: None,
        }
    }

    pub fn title(&self, lang: &str) -> String {
        if self.form.is_some() {
            return format!("/model · {}", tr(lang, "添加自定义供应商", &[]));
        }
        let id = match self.step {
            Step::Member => "选择成员",
            Step::Provider => "选择模型供应商",
            Step::Model => "选择模型",
            Step::Effort => "选择思考强度",
        };
        let depth = match self.step {
            Step::Member => 0,
            Step::Provider => 1,
            Step::Model => 2,
            Step::Effort => 3,
        };
        let context = [text(&self.agent, "agent_id"), &self.provider, text(&self.profile, "model")]
            .into_iter()
            .take(depth)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" / ");
        format!(
            "/model · {}{}",
            tr(lang, id, &[]),
            if context.is_empty() { String::new() } else { format!(" · {context}") }
        )
    }

    pub fn options(&self, lang: &str) -> Vec<(String, String)> {
        let compatible = |p: &&Json| {
            text(&self.agent, "runtime_kind") != "codex" || matches!(text(p, "protocol"), "openai" | "responses")
        };
        let rows: Vec<(String, String)> = match self.step {
            Step::Member => self
                .agents
                .iter()
                .map(|a| {
                    (
                        text(a, "agent_id").into(),
                        format!(
                            "{} ({}) · {} / {} · {}",
                            text(a, "name"),
                            text(a, "agent_id"),
                            text(a, "provider"),
                            text(a, "model"),
                            text(a, "effort")
                        ),
                    )
                })
                .collect(),
            Step::Provider => {
                let mut providers: Vec<String> =
                    self.profiles.iter().filter(compatible).map(|p| text(p, "provider").to_string()).collect();
                providers.sort();
                providers.dedup();
                let mut rows: Vec<_> = providers.into_iter().map(|p| (p.clone(), p)).collect();
                rows.push((ADD_PROVIDER.into(), tr(lang, "添加自定义供应商", &[])));
                rows.push((String::new(), tr(lang, "恢复此成员的默认模型", &[])));
                rows
            }
            Step::Model => self
                .profiles
                .iter()
                .filter(compatible)
                .filter(|p| text(p, "provider") == self.provider)
                .map(|p| {
                    (
                        model_key(p),
                        format!(
                            "{} ({}){}",
                            text(p, "model"),
                            text(p, "id"),
                            if p["discovered"] == true { tr(lang, " · 在线", &[]) } else { String::new() }
                        ),
                    )
                })
                .collect(),
            Step::Effort => {
                let mut rows = vec![(String::new(), tr(lang, "使用默认档位", &[]))];
                rows.extend(
                    self.profile["efforts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Json::as_str)
                        .map(|s| (s.into(), s.into())),
                );
                rows
            }
        };
        let query = self.query.to_lowercase();
        rows.into_iter().filter(|(_, label)| label.to_lowercase().contains(&query)).collect()
    }

    fn move_to(&mut self, step: Step, lang: &str) {
        self.step = step;
        self.query.clear();
        let preferred = match step {
            Step::Member => text(&self.agent, "agent_id").to_string(),
            Step::Provider => text(&self.agent, "provider").to_string(),
            Step::Model => {
                serde_json::json!([text(&self.agent, "model_profile"), text(&self.agent, "model")]).to_string()
            }
            Step::Effort
                if text(&self.profile, "id") == text(&self.agent, "model_profile")
                    && text(&self.profile, "model") == text(&self.agent, "model") =>
            {
                text(&self.agent, "effort").to_string()
            }
            Step::Effort => String::new(),
        };
        self.index =
            self.options(lang).iter().position(|(id, _)| !preferred.is_empty() && *id == preferred).unwrap_or(0);
    }

    pub fn merge_discovered(&mut self, provider: &str, result: Result<Json, String>, lang: &str) {
        if self.provider != provider || self.form.is_some() {
            return;
        }
        self.loading = false;
        let selected = self.options(lang).get(self.index).map(|(id, _)| id.clone());
        match result {
            Ok(report) => {
                let errors: Vec<_> =
                    report["errors"].as_array().into_iter().flatten().filter_map(Json::as_str).collect();
                self.notice = if errors.is_empty() {
                    tr(lang, "在线模型已更新", &[])
                } else {
                    format!("{} {}", tr(lang, "部分在线模型获取失败；已配置模型仍可用：", &[]), errors.join("; "))
                };
                let mut seen: std::collections::HashSet<_> = self.profiles.iter().map(model_key).collect();
                for model in report["models"].as_array().into_iter().flatten() {
                    if seen.insert(model_key(model)) {
                        self.profiles.push(model.clone());
                    }
                }
            }
            Err(error) => self.notice = format!("{} {error}", tr(lang, "在线模型获取失败；已配置模型仍可用：", &[])),
        }
        self.index = selected.and_then(|id| self.options(lang).iter().position(|(key, _)| *key == id)).unwrap_or(0);
    }

    pub fn handle_key(&mut self, key: KeyEvent, lang: &str) -> Vec<Effect> {
        if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return vec![Effect::Quit];
        }
        if let Some(form) = &mut self.form {
            if key.code == KeyCode::Esc && !form.saving {
                if form.field == 0 {
                    self.form = None;
                    self.notice.clear();
                } else {
                    form.field -= 1;
                }
                return vec![];
            }
            return form.handle_key(key, lang, &mut self.notice);
        }
        match key.code {
            KeyCode::Up => self.index = self.index.saturating_sub(1),
            KeyCode::Down => self.index = self.index.saturating_add(1).min(self.options(lang).len().saturating_sub(1)),
            KeyCode::Esc => match self.step {
                Step::Member => self.closed = true,
                Step::Provider => {
                    self.provider.clear();
                    self.move_to(Step::Member, lang);
                }
                Step::Model => {
                    self.profile = Json::Null;
                    self.move_to(Step::Provider, lang);
                }
                Step::Effort => self.move_to(Step::Model, lang),
            },
            KeyCode::Backspace => {
                self.query.pop();
                self.index = 0;
            }
            KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.query.push(c);
                self.index = 0;
            }
            KeyCode::Enter => {
                let Some((id, _)) = self.options(lang).get(self.index).cloned() else { return vec![] };
                match self.step {
                    Step::Member => {
                        self.agent = self.agents.iter().find(|a| text(a, "agent_id") == id).unwrap().clone();
                        self.move_to(Step::Provider, lang);
                    }
                    Step::Provider if id == ADD_PROVIDER => self.start_add_provider(),
                    Step::Provider if id.is_empty() => {
                        self.closed = true;
                        return vec![Effect::SetModel {
                            agent_id: text(&self.agent, "agent_id").into(),
                            profile: None,
                            model: None,
                            effort: None,
                        }];
                    }
                    Step::Provider => {
                        self.provider = id;
                        self.loading = true;
                        self.notice.clear();
                        self.move_to(Step::Model, lang);
                        return vec![Effect::DiscoverModels { provider: self.provider.clone() }];
                    }
                    Step::Model => {
                        self.profile = self.profiles.iter().find(|p| model_key(p) == id).unwrap().clone();
                        self.move_to(Step::Effort, lang);
                    }
                    Step::Effort => {
                        self.closed = true;
                        return vec![Effect::SetModel {
                            agent_id: text(&self.agent, "agent_id").into(),
                            profile: Some(text(&self.profile, "id").into()),
                            model: if self.profile["discovered"] == true {
                                Some(text(&self.profile, "model").into())
                            } else {
                                None
                            },
                            effort: if id.is_empty() { None } else { Some(id) },
                        }];
                    }
                }
            }
            _ => {}
        }
        vec![]
    }

    pub fn start_add_provider(&mut self) {
        self.form = Some(ProviderForm {
            values: [String::new(), "responses".into(), String::new(), String::new(), String::new(), String::new()],
            field: 0,
            saving: false,
        });
        self.notice.clear();
        self.loading = false;
    }

    pub fn handle_paste(&mut self, text: &str) {
        if let Some(form) = &mut self.form {
            if !form.saving && form.field < 6 && form.field != 1 {
                form.values[form.field].extend(text.chars().filter(|c| !c.is_control()));
            }
        } else {
            self.query.extend(text.chars().filter(|c| !c.is_control()));
            self.index = 0;
        }
    }

    pub fn provider_added(&mut self, result: Result<Json, String>, lang: &str) {
        let Some(form) = &mut self.form else { return };
        form.saving = false;
        match result {
            Err(error) => self.notice = error,
            Ok(report) => {
                self.profiles = report["profiles"].as_array().cloned().unwrap_or_default();
                let added = self.profiles.iter().find(|p| p["id"] == report["added_profile"]).cloned();
                self.form = None;
                self.notice = tr(lang, "供应商已保存；请选择模型", &[]);
                if let Some(profile) = added.filter(|p| {
                    !self.agent.is_null()
                        && (text(&self.agent, "runtime_kind") != "codex"
                            || matches!(text(p, "protocol"), "openai" | "responses"))
                }) {
                    self.provider = text(&profile, "provider").into();
                    self.move_to(Step::Model, lang);
                } else {
                    self.move_to(Step::Member, lang);
                    self.notice = tr(lang, "供应商已保存；请选择成员（Codex 仅支持 Responses）", &[]);
                }
            }
        }
    }
}
