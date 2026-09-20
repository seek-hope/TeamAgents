//! Opt-in Chat provider smoke matrix. Normal cargo test runs only its offline
//! contracts. See docs/DEVELOPMENT.md for the manifest and explicit live entry.

mod support;

use serde::Deserialize;
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use teamagents_core::models::{ModelProfile, UserConfig};
use teamagents_engine::config::{load_user_config, user_config_path};
use teamagents_engine::session::{open_session, OpenOptions, OpenedSession};

type Check<T> = Result<T, &'static str>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Matrix {
    #[serde(default = "phase_timeout")]
    phase_timeout_s: u64,
    models: Vec<Selection>,
}

fn phase_timeout() -> u64 {
    600
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    profile: String,
    model: String,
    context_window: Option<u64>,
    context_source: Option<String>,
}

fn parse_matrix(text: &str) -> Check<Matrix> {
    // Never echo parse errors: source configurations can contain credentials.
    let matrix: Matrix = toml::from_str(text).map_err(|_| "清单格式错误")?;
    if matrix.models.is_empty() || !(1..=3600).contains(&matrix.phase_timeout_s) {
        return Err("清单不能为空；阶段时限必须在 1–3600 秒内");
    }
    let mut names = HashSet::new();
    if matrix
        .models
        .iter()
        .any(|m| m.profile.trim().is_empty() || m.model.trim().is_empty() || !names.insert(&m.profile))
    {
        return Err("模型名、配置名不能为空，配置名不可重复");
    }
    Ok(matrix)
}

fn select_profile(catalog: &UserConfig, selected: &Selection) -> Result<ModelProfile, (&'static str, &'static str)> {
    let Some(profile) = catalog.models.get(&selected.profile) else { return Err(("skipped", "没有此模型配置")) };
    if profile.model != selected.model {
        return Err(("blocked", "配置模型与清单声明的模型不一致"));
    }
    let Some(window) = selected.context_window.filter(|w| *w > 0) else {
        return Err(("blocked", "必须明确提供模型原生上下文长度"));
    };
    if selected.context_source.as_deref().is_none_or(|s| s.trim().is_empty()) {
        return Err(("blocked", "缺少原生上下文长度的来源"));
    }
    if profile.context_window.is_some_and(|w| w != window) {
        return Err(("blocked", "配置窗口与清单声明的原生窗口不一致"));
    }
    if !matches!(profile.protocol.as_str(), "openai" | "deepseek" | "responses" | "anthropic") {
        return Err(("blocked", "不支持此 Chat 协议"));
    }
    if profile.model.trim().is_empty() || profile.timeout <= 0 || profile.max_retries < 0 {
        return Err(("blocked", "模型名称、请求超时或重试配置无效"));
    }
    if profile.api_key_env.as_ref().is_some_and(|key| std::env::var(key).map_or(true, |value| value.trim().is_empty()))
    {
        return Err(("skipped", "所引用的凭据环境变量不存在或为空"));
    }
    let mut effective = profile.clone();
    // A missing window can only come from an explicit, sourced manifest entry.
    // An existing value is never overridden, nor is any generation option.
    effective.context_window = Some(window);
    Ok(effective)
}

fn fingerprint(mut value: Json) -> String {
    value.sort_all_objects();
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

fn model_evidence(catalog: &UserConfig, selected: &Selection) -> Json {
    let mut result = json!({
        "profile": selected.profile, "expected_model":selected.model, "status": "pending", "phases": [],
        "native_context_window": selected.context_window, "context_source": selected.context_source
    });
    if let Some(profile) = catalog.models.get(&selected.profile) {
        result["provider"] = json!(profile.provider);
        result["protocol"] = json!(profile.protocol);
        result["model"] = json!(profile.model);
        result["profile_context_window"] = json!(profile.context_window);
        result["request_timeout_s"] = json!(profile.timeout);
        result["max_retries"] = json!(profile.max_retries);
        // Hash arbitrary options and endpoint fields instead of serializing
        // secrets a provider might place in them. Record only known effort names.
        result["profile_sha256"] = json!(fingerprint(serde_json::to_value(profile).unwrap()));
        if let Some(effort @ ("none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max")) =
            profile.generation_options.get("reasoning_effort").and_then(Json::as_str)
        {
            result["reasoning_effort"] = json!(effort);
        }
    }
    result
}

fn isolated_catalog(profile: ModelProfile) -> UserConfig {
    UserConfig { models: [("live".into(), profile)].into(), ..Default::default() }
}

struct Session(Arc<OpenedSession>);

impl Drop for Session {
    fn drop(&mut self) {
        // This guard must be dropped before support::TestEnv, including failures.
        self.0.close();
    }
}

fn open_case(work: &Path, id: &str, profile: &ModelProfile, timeout_s: u64) -> Check<Session> {
    open_session(OpenOptions {
        cwd: Some(work.into()),
        session_id: Some(id.into()),
        catalog: Some(isolated_catalog(profile.clone())),
        initial_spec: Some(json!({
            "leader_id":"leader",
            "agents":[{"id":"leader","name":"验收成员","role":"leader","runtime_kind":"deepagents",
                "model_profile":"live","tool_bindings":["files","shell"],
                "instructions":"Work alone on the requested file task. Finish with the requested final text."}],
            "limits":{"turn_active_timeout_s":timeout_s}
        })),
        ..Default::default()
    })
    .map(Session)
    .map_err(|_| "无法打开隔离验收会话")
}

fn regular_file_matches(path: &Path, expected: &str) -> bool {
    // Reject links and special files before reading; do not archive model output.
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() == expected.len() as u64)
        && std::fs::read(path).is_ok_and(|bytes| bytes == expected.as_bytes())
}

fn usage(session: &OpenedSession) -> Json {
    let report = session.usage_report();
    let source = &report["agents"][0]["usage"];
    let mut out = json!({});
    // Persist only counters, not provider strings, paths or raw response bodies.
    for key in [
        "calls",
        "prompt_tokens",
        "completion_tokens",
        "total_tokens",
        "unknown_usage_calls",
        "cached_input_tokens",
        "model_elapsed_ms",
    ] {
        if let Some(value) = source[key].as_u64() {
            out[key] = json!(value);
        }
    }
    out
}

struct Phase<'a> {
    name: &'a str,
    prompt: &'a str,
    marker: &'a str,
    output: &'a str,
    expected: &'a str,
    required_tools: &'a [&'a str],
}

fn run_phase(session: &OpenedSession, phase: &Phase<'_>, timeout_s: u64) -> Check<Json> {
    let before = session.core.state_brief().map_err(|_| "无法读取阶段开始状态")?;
    let old_ids: HashSet<_> =
        before["runs"].as_array().into_iter().flatten().filter_map(|r| r["run_id"].as_str()).collect();
    let tools = Arc::new(Mutex::new(BTreeMap::<String, [u64; 2]>::new()));
    let text = Arc::new(Mutex::new(String::new()));
    let observed = tools.clone();
    session.runtime.notify.set_tool_sink(Box::new(move |_, _, activity| {
        let name = activity["tool"].as_str().unwrap_or("unknown").to_string();
        let index = usize::from(activity["ok"] != true);
        observed.lock().unwrap().entry(name).or_default()[index] += 1;
    }));
    let observed = text.clone();
    session.runtime.notify.set_stream_sink(Box::new(move |_, _, chunk| {
        // Only the marker is reported; bound unexpected verbosity in memory.
        let mut text = observed.lock().unwrap();
        if text.len() < 64 * 1024 {
            text.push_str(chunk);
        }
    }));
    let prior_usage = usage(session);
    let started = Instant::now();
    let submitted = session.runtime.user_message(phase.prompt, false).map_err(|_| "无法提交阶段任务")?.ok;
    let settled = submitted && session.runtime.settle(timeout_s);
    let state = session.core.state_brief().map_err(|_| "无法读取阶段结束状态")?;
    let runs: Vec<_> = state["runs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["run_id"].as_str().is_some_and(|id| !old_ids.contains(id)))
        .collect();
    let completed = !runs.is_empty() && runs.iter().all(|r| r["status"] == "COMPLETED");
    let tools = tools.lock().unwrap();
    let required_tools_ok = phase.required_tools.iter().all(|name| tools.get(*name).is_some_and(|count| count[0] > 0));
    let file_matches = regular_file_matches(&session.cwd.join(phase.output), phase.expected);
    let marker_received = text.lock().unwrap().contains(phase.marker);
    let current_usage = usage(session);
    let usage_recorded = current_usage["calls"].as_u64().unwrap_or(0) > prior_usage["calls"].as_u64().unwrap_or(0);
    let passed =
        submitted && settled && completed && required_tools_ok && file_matches && marker_received && usage_recorded;
    Ok(json!({
        "name":phase.name,"status":if passed {"passed"} else {"failed"},
        "duration_ms":started.elapsed().as_millis(),"submitted":submitted,"settled":settled,
        "run_statuses":runs.iter().map(|r| r["status"].clone()).collect::<Vec<_>>(),
        "required_tools_ok":required_tools_ok,"file_matches":file_matches,"reply_marker_received":marker_received,
        "tools":tools.iter().map(|(name, count)| (name.clone(),json!({"ok":count[0],"failed":count[1]}))).collect::<BTreeMap<_,_>>(),
        "usage_recorded":usage_recorded,"usage_before":prior_usage,"usage_after":current_usage
    }))
}

fn run_case(
    root: &Path,
    profile: &ModelProfile,
    timeout_s: u64,
    mut result: Json,
    mut save: impl FnMut(&Json) -> Check<()>,
) -> Check<Json> {
    let started = Instant::now();
    let work = root.join(format!("work-{}", uuid::Uuid::new_v4()));
    let id = format!("live-{}", uuid::Uuid::new_v4());
    let token = format!("live-token-{}", uuid::Uuid::new_v4());
    result["status"] = json!("running");
    result["effective_profile_sha256"] = json!(fingerprint(serde_json::to_value(profile).unwrap()));
    save(&result)?;
    let outcome = (|| -> Check<()> {
        std::fs::create_dir_all(&work).map_err(|_| "无法创建验收工作目录")?;
        std::fs::write(work.join("seed.txt"), format!("{token}\n")).map_err(|_| "无法准备随机输入")?;
        let session = open_case(&work, &id, profile, timeout_s)?;
        session.0.runtime.start();
        let initial = format!("{token}\n");
        let continued = format!("{token}:continued\n");
        let cold = format!("{token}:cold\n");
        let phases = [
            Phase {
                name:"files_and_tool_result",
                prompt:"Read seed.txt using read_file. Use write_file to create proof.txt with exactly the same complete contents, including the newline. Read proof.txt to verify it. End with LIVE_FILES_OK without repeating the token in your final answer.",
                marker:"LIVE_FILES_OK",output:"proof.txt",expected:&initial,
                required_tools:&["read_file","write_file"]
            },
            Phase {
                name:"same_session_shell",
                prompt:"The test harness has removed seed.txt and proof.txt. From the token in our previous tool results, use shell to create continued.txt containing that original token followed by :continued and a newline. Read continued.txt to verify it. End with LIVE_CONTINUED_OK without repeating the token in your final answer.",
                marker:"LIVE_CONTINUED_OK",output:"continued.txt",expected:&continued,
                required_tools:&["shell","read_file"]
            },
            Phase {
                name:"cold_session_history",
                prompt:"The test harness has removed continued.txt and reopened this same session. From the original token in our previous tool results, use write_file to create cold.txt containing that original token followed by :cold and a newline. Read cold.txt to verify it. End with LIVE_COLD_OK without repeating the token in your final answer.",
                marker:"LIVE_COLD_OK",output:"cold.txt",expected:&cold,
                required_tools:&["write_file","read_file"]
            },
        ];
        for phase in &phases[..2] {
            let evidence = run_phase(&session.0, phase, timeout_s)?;
            let passed = evidence["status"] == "passed";
            eprintln!("验收阶段 {}：{}", phase.name, if passed { "通过" } else { "失败" });
            result["phases"].as_array_mut().unwrap().push(evidence);
            save(&result)?;
            if !passed {
                return Err("阶段行为断言未通过");
            }
            std::fs::remove_file(work.join(phase.output)).map_err(|_| "无法移除阶段输出")?;
            if phase.name == "files_and_tool_result" {
                result["seed_unchanged"] = json!(regular_file_matches(&work.join("seed.txt"), &initial));
                if result["seed_unchanged"] != true {
                    return Err("模型改动了只读输入");
                }
                std::fs::remove_file(work.join("seed.txt")).map_err(|_| "无法移除阶段输入")?;
            }
            // A leftover copy would let a model pass by rereading the workspace
            // even if conversation persistence were broken.
            let empty = std::fs::read_dir(&work).map_err(|_| "无法检查续接前的工作目录")?.next().is_none();
            let key = if phase.name == "files_and_tool_result" {
                "workspace_empty_before_same_session"
            } else {
                "workspace_empty_before_cold_session"
            };
            result[key] = json!(empty);
            if !empty {
                return Err("续接前存在额外文件，不能证明历史恢复");
            }
        }
        let saved_usage = usage(&session.0);
        drop(session);
        let restored = open_case(&work, &id, profile, timeout_s)?;
        restored.0.runtime.reconcile();
        let recovered_usage = usage(&restored.0);
        result["usage_restored"] = json!(saved_usage == recovered_usage);
        if saved_usage != recovered_usage {
            return Err("关闭重开后用量账本不一致");
        }
        restored.0.runtime.start();
        let evidence = run_phase(&restored.0, &phases[2], timeout_s)?;
        let passed = evidence["status"] == "passed";
        eprintln!("验收阶段 cold_session_history：{}", if passed { "通过" } else { "失败" });
        result["phases"].as_array_mut().unwrap().push(evidence);
        if !passed {
            return Err("冷恢复阶段行为断言未通过");
        }
        Ok(())
    })();
    result["status"] = json!(if outcome.is_ok() { "passed" } else { "failed" });
    result["duration_ms"] = json!(started.elapsed().as_millis());
    if let Err(reason) = outcome {
        result["reason"] = json!(reason);
    }
    save(&result)?;
    Ok(result)
}

fn matrix_status(results: &[Json]) -> &'static str {
    if !results.is_empty() && results.iter().all(|r| r["status"] == "passed") {
        "passed"
    } else if results.iter().any(|r| r["status"] == "failed") {
        "failed"
    } else {
        "incomplete"
    }
}

fn save_report(path: &Path, report: &Json) -> Check<()> {
    let bytes = serde_json::to_vec_pretty(report).map_err(|_| "无法编码验收报告")?;
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(temp, path)).map_err(|_| "无法保存验收报告")
}

fn execute_matrix(manifest: &str, catalog: &UserConfig, root: &Path, report: &mut Json, path: &Path) -> Check<()> {
    let matrix = parse_matrix(manifest)?;
    report["phase_timeout_s"] = json!(matrix.phase_timeout_s);
    report["results"] = json!(matrix.models.iter().map(|s| model_evidence(catalog, s)).collect::<Vec<_>>());
    save_report(path, report)?;
    for (index, selected) in matrix.models.iter().enumerate() {
        match select_profile(catalog, selected) {
            Err((status, reason)) => {
                report["results"][index]["status"] = json!(status);
                report["results"][index]["reason"] = json!(reason);
                save_report(path, report)?;
            }
            Ok(profile) => {
                let initial = report["results"][index].clone();
                run_case(root, &profile, matrix.phase_timeout_s, initial, |partial| {
                    report["results"][index] = partial.clone();
                    save_report(path, report)
                })?;
            }
        }
    }
    report["status"] = json!(matrix_status(report["results"].as_array().unwrap()));
    save_report(path, report)
}

#[test]
#[ignore = "真实模型请求；需显式提供清单、凭据和新的证据目录"]
fn live_chat_model_matrix() {
    let evidence = PathBuf::from(
        std::env::var_os("TEAMAGENTS_LIVE_MODELS_EVIDENCE").expect("请设置 TEAMAGENTS_LIVE_MODELS_EVIDENCE"),
    );
    // A fresh output directory prevents a rerun from overwriting old evidence.
    std::fs::create_dir(&evidence).expect("证据目录必须尚不存在，且父目录可写");
    let report_path = evidence.join("report.json");
    let mut report = json!({"schema_version":1,"kind":"live_chat_model_smoke","status":"running","results":[]});
    save_report(&report_path, &report).unwrap();
    let outcome = (|| -> Check<()> {
        let manifest = std::env::var_os("TEAMAGENTS_LIVE_MODELS_MANIFEST").ok_or("请设置模型清单路径")?;
        let text = std::fs::read_to_string(manifest).map_err(|_| "无法读取模型清单")?;
        let config =
            std::env::var_os("TEAMAGENTS_LIVE_MODELS_CONFIG").map(PathBuf::from).unwrap_or_else(user_config_path);
        let catalog = load_user_config(&config).map_err(|_| "无法读取模型配置")?;
        let env = support::isolated_state_home("live-models");
        execute_matrix(&text, &catalog, &env, &mut report, &report_path)
    })();
    if let Err(reason) = outcome {
        report["status"] = json!("blocked");
        report["reason"] = json!(reason);
        save_report(&report_path, &report).unwrap();
    }
    assert_eq!(report["status"], "passed", "真实验收未全部通过；查看 {}", report_path.display());
}

fn fixture_profile(protocol: &str, base_url: &str) -> ModelProfile {
    serde_json::from_value(json!({
        "provider":"fixture","protocol":protocol,"model":"fixture-model","base_url":base_url,
        "context_window":1000000,"timeout":5,"max_retries":0,
        "generation_options":{"reasoning_effort":"high"}
    }))
    .unwrap()
}

fn fixture_manifest() -> &'static str {
    r#"phase_timeout_s = 10
[[models]]
profile = "m"
model = "fixture-model"
context_window = 1000000
context_source = "本地协议夹具；不是模型评测"
"#
}

#[test]
fn manifest_and_profile_checks_refuse_unknown_or_conflicting_windows() {
    let mut env = support::isolated_state_home("live-preflight");
    env.set("TEAMAGENTS_LIVE_FIXTURE_KEY", "");
    for bad in [
        "models = []",
        &fixture_manifest().replace("phase_timeout_s = 10", "phase_timeout_s = 0"),
        &format!("{}\n[[models]]\nprofile = \"m\"\nmodel = \"fixture-model\"", fixture_manifest()),
        &fixture_manifest().replace("model = \"fixture-model\"", "model = \"\""),
        &format!("unexpected = true\n{}", fixture_manifest()),
    ] {
        assert!(parse_matrix(bad).is_err());
    }
    let mut profile = fixture_profile("deepseek", "https://fixture.invalid");
    profile.api_key_env = Some("TEAMAGENTS_LIVE_FIXTURE_KEY".into());
    let mut catalog = UserConfig { models: [("m".into(), profile.clone())].into(), ..Default::default() };
    let mut selected = parse_matrix(fixture_manifest()).unwrap().models.remove(0);
    assert_eq!(select_profile(&catalog, &selected).unwrap_err().0, "skipped");
    env.set("TEAMAGENTS_LIVE_FIXTURE_KEY", "fixture-credential");
    assert!(select_profile(&catalog, &selected).is_ok());
    selected.context_window = None;
    assert_eq!(select_profile(&catalog, &selected).unwrap_err().0, "blocked");
    selected.context_window = Some(0);
    assert_eq!(select_profile(&catalog, &selected).unwrap_err().0, "blocked");
    selected.context_window = Some(64000);
    assert_eq!(select_profile(&catalog, &selected).unwrap_err().0, "blocked");
    selected.context_window = Some(1000000);
    selected.model = "changed-model".into();
    assert_eq!(select_profile(&catalog, &selected).unwrap_err().0, "blocked");
    selected.model = "fixture-model".into();
    selected.context_source = Some("  ".into());
    assert_eq!(select_profile(&catalog, &selected).unwrap_err().0, "blocked");
    selected.context_source = Some("fixture".into());
    profile.context_window = None;
    catalog.models.insert("m".into(), profile.clone());
    let effective = select_profile(&catalog, &selected).unwrap();
    profile.context_window = Some(1000000);
    assert_eq!(serde_json::to_value(effective).unwrap(), serde_json::to_value(profile).unwrap());
}

#[test]
fn matrix_preflight_is_incomplete_and_preserves_safe_evidence() {
    let mut env = support::isolated_state_home("live-skipped");
    env.set("TEAMAGENTS_LIVE_FIXTURE_KEY", "");
    let mut profile = fixture_profile("openai", "https://user:PRIVATE_ENDPOINT@fixture.invalid");
    profile.api_key_env = Some("TEAMAGENTS_LIVE_FIXTURE_KEY".into());
    profile.generation_options.insert("private".into(), json!("PRIVATE_OPTION"));
    let mut catalog = UserConfig { models: [("m".into(), profile.clone())].into(), ..Default::default() };
    catalog.hooks.notify = vec!["must-not-run".into()];
    catalog.hooks.pre_tool = vec!["must-not-run".into()];
    catalog.skills_paths.push("/private/skills".into());
    catalog.instruction_files.push("/private/instructions".into());
    catalog.retention.archived_days = 1;
    catalog.tools.insert("private-mcp".into(), Default::default());
    let isolated = isolated_catalog(profile);
    assert!(isolated.hooks.notify.is_empty() && isolated.hooks.pre_tool.is_empty());
    assert!(isolated.skills_paths.is_empty() && isolated.instruction_files.is_empty() && isolated.tools.is_empty());
    assert_eq!(isolated.retention.archived_days, 0);
    assert_eq!(isolated.models.len(), 1);
    let manifest = format!("{}\n[[models]]\nprofile = \"absent\"\nmodel = \"absent\"\n", fixture_manifest());
    let path = env.join("report.json");
    let mut report = json!({"kind":"offline_contract","status":"running"});
    execute_matrix(&manifest, &catalog, &env, &mut report, &path).unwrap();
    assert_eq!(report["status"], "incomplete");
    assert!(report["results"].as_array().unwrap().iter().all(|r| r["status"] == "skipped"));
    let saved = std::fs::read_to_string(&path).unwrap();
    assert_eq!(serde_json::from_str::<Json>(&saved).unwrap(), report);
    for secret in ["PRIVATE_ENDPOINT", "PRIVATE_OPTION", "/private", "must-not-run"] {
        assert!(!saved.contains(secret), "{secret} must not be archived");
    }
    assert_eq!(std::fs::read_dir(&*env).unwrap().count(), 2, "no model workspace was started");
    assert_ne!(matrix_status(&[]), "passed");
    assert_ne!(matrix_status(&[json!({"status":"passed"}), json!({"status":"skipped"})]), "passed");
}

// A joined, local-only service exercises the very same driver as the live
// entry. All model responses are synthetic, including the 1M context value.
struct FakeModel {
    base_url: String,
    bodies: Arc<Mutex<Vec<Json>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeModel {
    fn start(reply: impl Fn(&Json, usize) -> (u16, String) + Send + 'static) -> Self {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (recorded, stopped) = (bodies.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !stopped.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 4096];
                let mut header = None;
                let mut length = 0;
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if header.is_none() {
                        header = bytes.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
                        if let Some(end) = header {
                            length = String::from_utf8_lossy(&bytes[..end])
                                .lines()
                                .find_map(|line| {
                                    let (key, value) = line.split_once(':')?;
                                    key.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().unwrap())
                                })
                                .unwrap_or(0);
                        }
                    }
                    if header.is_some_and(|end| bytes.len() >= end + length) {
                        break;
                    }
                }
                let Some(end) = header else { continue };
                let body: Json = serde_json::from_slice(&bytes[end..end + length]).unwrap();
                let mut all = recorded.lock().unwrap();
                let index = all.len();
                all.push(body.clone());
                drop(all);
                let (status, payload) = reply(&body, index);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                );
            }
        });
        Self { base_url, bodies, stop, thread: Some(thread) }
    }
}

impl Drop for FakeModel {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn stream_reply(protocol: &str, index: usize, tool: Option<(&str, Json)>, text: &str) -> String {
    let id = format!("fixture-{index}");
    let frames = match protocol {
        "anthropic" => {
            let (block, delta, reason) = match tool {
                Some((name, args)) => (
                    json!({"type":"tool_use","id":id,"name":name,"input":{}}),
                    json!({"type":"input_json_delta","partial_json":args.to_string()}),
                    "tool_use",
                ),
                None => (json!({"type":"text","text":""}), json!({"type":"text_delta","text":text}), "end_turn"),
            };
            vec![
                json!({"type":"message_start","message":{"usage":{"input_tokens":5,"output_tokens":0}}}),
                json!({"type":"content_block_start","index":0,"content_block":block}),
                json!({"type":"content_block_delta","index":0,"delta":delta}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":3}}),
                json!({"type":"message_stop"}),
            ]
        }
        "responses" => {
            let item = match tool {
                Some((name, args)) => {
                    json!({"type":"function_call","call_id":id,"name":name,"arguments":args.to_string()})
                }
                None => json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}),
            };
            let mut frames = vec![];
            if !text.is_empty() {
                frames.push(json!({"type":"response.output_text.delta","delta":text}));
            }
            frames.push(json!({"type":"response.output_item.done","item":item}));
            frames.push(json!({"type":"response.completed","response":{"status":"completed",
                "output":[item],"usage":{"input_tokens":5,"output_tokens":3,"total_tokens":8}}}));
            frames
        }
        _ => {
            let (delta, reason) = match tool {
                Some((name, args)) => (
                    json!({"reasoning_content":"fixture reasoning","tool_calls":[{
                        "index":0,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}
                    }]}),
                    "tool_calls",
                ),
                None => (json!({"content":text}), "stop"),
            };
            vec![
                json!({"choices":[{"delta":delta}]}),
                json!({"choices":[{"delta":{},"finish_reason":reason}],
                    "usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}),
            ]
        }
    };
    let mut wire: String = frames.into_iter().map(|frame| format!("data: {frame}\n\n")).collect();
    if matches!(protocol, "openai" | "deepseek") {
        wire.push_str("data: [DONE]\n\n");
    }
    wire
}

fn scripted_reply(protocol: &str, body: &Json, index: usize) -> String {
    let raw = body.to_string();
    let token = raw
        .find("live-token-")
        .map(|start| raw[start..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>())
        .unwrap_or_default();
    let read = |path| Some(("read_file", json!({"path":path})));
    let write = |path, content: String| Some(("write_file", json!({"path":path,"content":content})));
    let (tool, text) = match index {
        0 => (read("seed.txt"), ""),
        1 => (write("proof.txt", format!("{token}\n")), ""),
        2 => (read("proof.txt"), ""),
        3 => (None, "LIVE_FILES_OK"),
        4 => (Some(("shell", json!({"command":format!("printf '%s\\n' '{token}:continued' > continued.txt")}))), ""),
        5 => (read("continued.txt"), ""),
        6 => (None, "LIVE_CONTINUED_OK"),
        7 => (write("cold.txt", format!("{token}:cold\n")), ""),
        8 => (read("cold.txt"), ""),
        _ => (None, "LIVE_COLD_OK"),
    };
    stream_reply(protocol, index, tool, text)
}

#[test]
fn matrix_contract_streams_files_continuation_and_cold_history_for_every_protocol() {
    let env = support::isolated_state_home("live-protocols");
    // Probe the real sandbox, including kernels that install bwrap but deny it.
    let sandbox = teamagents_engine::tools::shell_run("true", &env, 5, false, None).is_ok();
    for protocol in ["openai", "deepseek", "responses", "anthropic"] {
        let server = FakeModel::start(move |body, index| (200, scripted_reply(protocol, body, index)));
        let catalog = UserConfig {
            models: [("m".into(), fixture_profile(protocol, &server.base_url))].into(),
            ..Default::default()
        };
        let path = env.join(format!("{protocol}.json"));
        let mut report = json!({"kind":"offline_contract","status":"running"});
        execute_matrix(fixture_manifest(), &catalog, &env, &mut report, &path).unwrap();
        if !sandbox {
            assert_eq!(report["status"], "failed", "unavailable sandbox must never pass");
            assert_eq!(report["results"][0]["phases"][0]["status"], "passed");
            assert_eq!(report["results"][0]["phases"][1]["status"], "failed");
            eprintln!("离线协议 {protocol}：文件阶段已验证；本机沙箱不可用，Shell 阶段按失败记录");
            continue;
        }
        assert_eq!(report["status"], "passed", "{protocol}: {report}");
        let result = &report["results"][0];
        assert_eq!(result["usage_restored"], true);
        assert_eq!(result["seed_unchanged"], true);
        assert_eq!(result["workspace_empty_before_same_session"], true);
        assert_eq!(result["workspace_empty_before_cold_session"], true);
        assert_eq!(result["phases"].as_array().unwrap().len(), 3);
        assert_eq!(result["phases"][2]["usage_after"]["calls"], 10);
        assert_eq!(result["phases"][2]["usage_after"]["total_tokens"], 80);
        let bodies = server.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 10);
        for body in bodies.iter() {
            assert_eq!(body["model"], "fixture-model");
            assert_eq!(body["stream"], true);
        }
        assert!(bodies[1].to_string().contains("live-token-"), "tool result reached the model");
        assert!(bodies[4].to_string().contains("LIVE_FILES_OK"), "warm turn retained prior history");
        assert!(bodies[7].to_string().contains("LIVE_CONTINUED_OK"), "cold turn retained prior history");
        if protocol == "deepseek" {
            assert!(bodies[1].to_string().contains("fixture reasoning"));
        }
        assert_eq!(serde_json::from_slice::<Json>(&std::fs::read(path).unwrap()).unwrap(), report);
    }
}

#[test]
fn matrix_contract_rejects_false_success_and_retains_sanitized_failures() {
    let env = support::isolated_state_home("live-failure");
    for status in [200, 401] {
        let server = FakeModel::start(move |_, index| {
            (status, stream_reply("openai", index, None, "LIVE_FILES_OK PRIVATE_RESPONSE"))
        });
        let catalog = UserConfig {
            models: [("m".into(), fixture_profile("openai", &server.base_url))].into(),
            ..Default::default()
        };
        let path = env.join(format!("failed-{status}.json"));
        let mut report = json!({"kind":"offline_contract","status":"running"});
        execute_matrix(fixture_manifest(), &catalog, &env, &mut report, &path).unwrap();
        assert_eq!(report["status"], "failed");
        let phase = &report["results"][0]["phases"][0];
        assert_eq!(phase["file_matches"], false);
        assert_eq!(phase["required_tools_ok"], false);
        assert_eq!(phase["reply_marker_received"], status == 200);
        assert_eq!(phase["run_statuses"], json!([if status == 200 { "COMPLETED" } else { "FAILED" }]));
        assert_eq!(report["results"][0]["phases"].as_array().unwrap().len(), 1);
        let saved = std::fs::read_to_string(path).unwrap();
        assert!(!saved.contains("PRIVATE_RESPONSE"));
        assert_eq!(serde_json::from_str::<Json>(&saved).unwrap(), report);
    }
}

#[test]
fn matrix_contract_rejects_leftover_files_as_history_evidence() {
    let env = support::isolated_state_home("live-leftover");
    let server = FakeModel::start(|body, index| {
        let reply = if index == 2 {
            stream_reply("openai", index, Some(("write_file", json!({"path":"notes.txt","content":"extra copy"}))), "")
        } else {
            scripted_reply("openai", body, index)
        };
        (200, reply)
    });
    let catalog =
        UserConfig { models: [("m".into(), fixture_profile("openai", &server.base_url))].into(), ..Default::default() };
    let mut report = json!({"kind":"offline_contract","status":"running"});
    execute_matrix(fixture_manifest(), &catalog, &env, &mut report, &env.join("report.json")).unwrap();
    assert_eq!(report["status"], "failed", "a correct first output alone cannot prove history recovery");
    assert_eq!(report["results"][0]["phases"][0]["file_matches"], true);
    assert_eq!(report["results"][0]["workspace_empty_before_same_session"], false);
    assert_eq!(report["results"][0]["phases"].as_array().unwrap().len(), 1);
    assert_eq!(server.bodies.lock().unwrap().len(), 4, "continuation never starts with leftover inputs");
}
