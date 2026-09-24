//! R2-P1 direct-drive reference CLI: one task through the new kernel,
//! DeepSeek chat-completions edge and basic tools, with a full JSONL trace.
//!
//!   cargo run --offline --manifest-path engine/Cargo.toml --example eval_group_a -- \
//!     --task "..." --workdir /tmp/task --trace /tmp/trace
//!
//! This binary is the eval group-A reference (RV-38): no team management, no
//! persistence promise. Credentials come from the environment, never args.

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use teamagents_core::kernel::KernelProfile;
use teamagents_core::models::{ToolBinding, UserConfig};
use teamagents_engine::providers::chat_completions::ChatCompletions;
use teamagents_engine::reference::{basic_tool_schemas, run_reference, ReferenceConfig, ReferenceEnd};

fn usage() -> ! {
    eprintln!(
        "usage: eval_group_a --task TEXT|--task-file PATH --workdir DIR --trace DIR \\
[--model deepseek-flash] [--base URL] [--api-key-env DEEPSEEK_API_KEY] [--window N] \\
[--effort max] [--max-steps N] [--timeout S] [--retries N] [--full-auto] [--web]"
    );
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut task = None;
    let mut task_file = None;
    let mut workdir: Option<PathBuf> = None;
    let mut trace_dir: Option<PathBuf> = None;
    let mut model = "deepseek-flash".to_string();
    let mut base = "https://api.deepseek.com/v1".to_string();
    let mut key_env = "DEEPSEEK_API_KEY".to_string();
    let mut window = 1_000_000u64;
    let mut effort = "max".to_string();
    let mut max_steps = 40usize;
    let mut timeout_s = 900u64;
    let mut retries = 5usize;
    let mut full_auto = false;
    let mut web = false;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let value = |i: &mut usize| -> String {
            *i += 1;
            args.get(*i).cloned().unwrap_or_else(|| usage())
        };
        match flag {
            "--task" => task = Some(value(&mut i)),
            "--task-file" => task_file = Some(PathBuf::from(value(&mut i))),
            "--workdir" => workdir = Some(PathBuf::from(value(&mut i))),
            "--trace" => trace_dir = Some(PathBuf::from(value(&mut i))),
            "--model" => model = value(&mut i),
            "--base" => base = value(&mut i),
            "--api-key-env" => key_env = value(&mut i),
            "--window" => window = value(&mut i).parse().unwrap_or_else(|_| usage()),
            "--effort" => effort = value(&mut i),
            "--max-steps" => max_steps = value(&mut i).parse().unwrap_or_else(|_| usage()),
            "--timeout" => timeout_s = value(&mut i).parse().unwrap_or_else(|_| usage()),
            "--retries" => retries = value(&mut i).parse().unwrap_or_else(|_| usage()),
            "--full-auto" => full_auto = true,
            "--web" => web = true,
            _ => usage(),
        }
        i += 1;
    }
    let (Some(workdir), Some(trace_dir)) = (workdir, trace_dir) else { usage() };
    let task = match (task, task_file) {
        (Some(text), None) => text,
        (None, Some(path)) => std::fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("cannot read {}: {e}", path.display());
            std::process::exit(2);
        }),
        _ => usage(),
    };
    let api_key = std::env::var(&key_env).unwrap_or_else(|_| {
        eprintln!("missing API key env {key_env}");
        std::process::exit(2);
    });
    std::fs::create_dir_all(&workdir).expect("workdir");
    let workspace = std::fs::canonicalize(&workdir).expect("canonical workdir");
    let state = workspace.join(".teamagents-ref");
    let artifacts = state.join("artifacts");
    let shell_state = state.join("shell");
    std::fs::create_dir_all(&artifacts).expect("artifacts");

    let mut catalog = UserConfig::default();
    if web {
        let mut tools: HashMap<String, ToolBinding> = HashMap::new();
        let search = ToolBinding {
            kind: "web_search".into(),
            provider: Some("anysearch".into()),
            url: Some("https://api.anysearch.com/v1/search".into()),
            api_key_env: Some("ANYSEARCH_API_KEY".into()),
            ..Default::default()
        };
        let fetch = ToolBinding { kind: "web_fetch".into(), provider: Some("anysearch".into()), ..Default::default() };
        tools.insert("web".into(), search);
        tools.insert("fetch".into(), fetch);
        catalog.tools = tools;
    }
    let bindings = if web { vec!["web".to_string()] } else { vec![] };
    let date = "2026-09-23";
    let instructions = format!(
        "You are a careful coding agent working in the workspace directory {workspace}.\n\
         - Use the file and shell tools to inspect, modify and verify. Verify claims with real commands before finishing.\n\
         - Long tool outputs are masked with a read_history recipe; page them back instead of re-running blind.\n\
         - Finish by calling `finish` exactly once with the honest status, a summary and evidence (files, commands).\n\
           Work you did not deliver must not be reported as success; list unverified claims in `unverified`.\n\
         - Today's date: {date}. Platform: linux.",
        workspace = workspace.display(),
    );
    let config = ReferenceConfig {
        workspace: workspace.clone(),
        artifacts: Some(artifacts),
        shell_state: Some(shell_state),
        permissions: if full_auto { "full_auto".into() } else { "approved_scope".into() },
        profile: KernelProfile {
            model: model.clone(),
            instructions,
            tools: basic_tool_schemas(web, false),
            options: json!({"reasoning_effort": effort}),
            context_window: Some(window),
        },
        catalog,
        bindings,
        max_steps,
        max_retries: retries,
        deadline: Some(Duration::from_secs(timeout_s)),
        trace_dir: trace_dir.clone(),
        run_id: "run".into(),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    let provider =
        ChatCompletions::new(base, api_key, Duration::from_secs(timeout_s.max(120))).expect("provider client");
    let outcome = runtime
        .block_on(run_reference(&provider, config, &task, |event| {
            let teamagents_engine::providers::ProviderEvent::TextDelta(text) = event;
            eprint!("{text}");
        }))
        .expect("reference run");
    eprintln!();
    let report = match &outcome.end {
        ReferenceEnd::Completed(candidate) => json!({"end": "completed", "candidate": candidate}),
        ReferenceEnd::Reply(text) => json!({"end": "reply", "chars": text.chars().count(), "text": text}),
        ReferenceEnd::Failed(reason) => json!({"end": "failed", "reason": reason}),
    };
    let mut out = report.as_object().unwrap().clone();
    out.insert("steps".into(), json!(outcome.steps));
    out.insert("usage".into(), json!(outcome.usage));
    out.insert("trace".into(), json!(outcome.trace_path.to_string_lossy()));
    println!("{}", Json::Object(out));
    if matches!(outcome.end, ReferenceEnd::Failed(_)) {
        std::process::exit(1);
    }
}
