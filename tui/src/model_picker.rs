//! Session model picker: member -> provider -> configured model -> effort.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value as Json;
use crate::app::Effect;
use crate::i18n::tr;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step { Member, Provider, Model, Effort }

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
}

fn text<'a>(v: &'a Json, key: &str) -> &'a str { v[key].as_str().unwrap_or("") }
fn model_key(v: &Json) -> String { serde_json::json!([text(v, "id"), text(v, "model")]).to_string() }

impl ModelPicker {
    pub fn new(report: &Json) -> Self {
        let mut agents = report["agents"].as_array().cloned().unwrap_or_default();
        agents.sort_by_key(|a| text(a, "agent_id") != text(report, "leader_id"));
        Self {
            agents, profiles: report["profiles"].as_array().cloned().unwrap_or_default(),
            step: Step::Member, agent: Json::Null, provider: String::new(), profile: Json::Null,
            index: 0, query: String::new(), closed: false, loading: false, notice: String::new(),
        }
    }

    pub fn title(&self, lang: &str) -> String {
        let id = match self.step {
            Step::Member => "选择成员", Step::Provider => "选择模型供应商",
            Step::Model => "选择模型", Step::Effort => "选择思考强度",
        };
        let depth = match self.step { Step::Member => 0, Step::Provider => 1, Step::Model => 2, Step::Effort => 3 };
        let context = [text(&self.agent, "agent_id"), &self.provider, text(&self.profile, "model")]
            .into_iter().take(depth).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" / ");
        format!("/model · {}{}", tr(lang, id, &[]), if context.is_empty() { String::new() } else { format!(" · {context}") })
    }

    pub fn options(&self, lang: &str) -> Vec<(String, String)> {
        let compatible = |p: &&Json| text(&self.agent, "runtime_kind") != "codex" || text(p, "protocol") == "openai";
        let rows: Vec<(String, String)> = match self.step {
            Step::Member => self.agents.iter().map(|a| (text(a, "agent_id").into(), format!(
                "{} ({}) · {} / {} · {}", text(a, "name"), text(a, "agent_id"), text(a, "provider"), text(a, "model"), text(a, "effort")
            ))).collect(),
            Step::Provider => {
                let mut providers: Vec<String> = self.profiles.iter().filter(compatible)
                    .map(|p| text(p, "provider").to_string()).collect();
                providers.sort(); providers.dedup();
                let mut rows: Vec<_> = providers.into_iter().map(|p| (p.clone(), p)).collect();
                rows.push((String::new(), tr(lang, "恢复此成员的默认模型", &[])));
                rows
            }
            Step::Model => self.profiles.iter().filter(compatible).filter(|p| text(p, "provider") == self.provider)
                .map(|p| (model_key(p), format!("{} ({}){}", text(p, "model"), text(p, "id"),
                    if p["discovered"] == true { tr(lang, " · 在线", &[]) } else { String::new() }))).collect(),
            Step::Effort => {
                let mut rows = vec![(String::new(), tr(lang, "使用默认档位", &[]))];
                rows.extend(self.profile["efforts"].as_array().into_iter().flatten().filter_map(Json::as_str)
                    .map(|s| (s.into(), s.into())));
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
            Step::Model => serde_json::json!([text(&self.agent, "model_profile"), text(&self.agent, "model")]).to_string(),
            Step::Effort if text(&self.profile, "id") == text(&self.agent, "model_profile")
                && text(&self.profile, "model") == text(&self.agent, "model") => text(&self.agent, "effort").to_string(),
            Step::Effort => String::new(),
        };
        self.index = self.options(lang).iter().position(|(id, _)| !preferred.is_empty() && *id == preferred).unwrap_or(0);
    }

    pub fn merge_discovered(&mut self, provider: &str, result: Result<Json, String>, lang: &str) {
        if self.provider != provider { return; }
        self.loading = false;
        let selected = self.options(lang).get(self.index).map(|(id, _)| id.clone());
        match result {
            Ok(report) => {
                let errors: Vec<_> = report["errors"].as_array().into_iter().flatten().filter_map(Json::as_str).collect();
                self.notice = if errors.is_empty() { tr(lang, "在线模型已更新", &[]) }
                    else { format!("{} {}", tr(lang, "部分在线模型获取失败；已配置模型仍可用：", &[]), errors.join("; ")) };
                let mut seen: std::collections::HashSet<_> = self.profiles.iter().map(model_key).collect();
                for model in report["models"].as_array().into_iter().flatten() {
                    if seen.insert(model_key(model)) { self.profiles.push(model.clone()); }
                }
            }
            Err(error) => self.notice = format!("{} {error}", tr(lang, "在线模型获取失败；已配置模型仍可用：", &[])),
        }
        self.index = selected.and_then(|id| self.options(lang).iter().position(|(key, _)| *key == id)).unwrap_or(0);
    }

    pub fn handle_key(&mut self, key: KeyEvent, lang: &str) -> Vec<Effect> {
        match key.code {
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => return vec![Effect::Quit],
            KeyCode::Up => self.index = self.index.saturating_sub(1),
            KeyCode::Down => self.index = self.index.saturating_add(1).min(self.options(lang).len().saturating_sub(1)),
            KeyCode::Esc => match self.step {
                Step::Member => self.closed = true,
                Step::Provider => { self.provider.clear(); self.move_to(Step::Member, lang); }
                Step::Model => { self.profile = Json::Null; self.move_to(Step::Provider, lang); }
                Step::Effort => self.move_to(Step::Model, lang),
            },
            KeyCode::Backspace => { self.query.pop(); self.index = 0; }
            KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.query.push(c); self.index = 0;
            }
            KeyCode::Enter => {
                let Some((id, _)) = self.options(lang).get(self.index).cloned() else { return vec![] };
                match self.step {
                    Step::Member => {
                        self.agent = self.agents.iter().find(|a| text(a, "agent_id") == id).unwrap().clone();
                        self.move_to(Step::Provider, lang);
                    }
                    Step::Provider if id.is_empty() => {
                        self.closed = true;
                        return vec![Effect::SetModel { agent_id: text(&self.agent, "agent_id").into(), profile: None, model: None, effort: None }];
                    }
                    Step::Provider => {
                        self.provider = id;
                        self.loading = true; self.notice.clear();
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
                            agent_id: text(&self.agent, "agent_id").into(), profile: Some(text(&self.profile, "id").into()),
                            model: if self.profile["discovered"] == true { Some(text(&self.profile, "model").into()) } else { None },
                            effort: if id.is_empty() { None } else { Some(id) },
                        }];
                    }
                }
            }
            _ => {}
        }
        vec![]
    }
}
