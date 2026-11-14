//! Reproducible Reviewer assessments; proposed operations are never executed.
//! Run from any directory: reviewer_probe [--dry-run] PROFILES.json CASES.json NEW_OUTPUT.jsonl.

use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::time::Instant;
use teamagents_core::models::ModelProfile;
use teamagents_engine::{providers, v2::reviewer};

type Error = Box<dyn std::error::Error>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    label: String,
    window_source: String,
    profile: ModelProfile,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    allow: bool,
    user: String,
    workspace: String,
    action: Value,
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn executable_hash() -> Result<String, Error> {
    let mut reader = std::fs::File::open(std::env::current_exe()?)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn input<T: serde::de::DeserializeOwned>(path: &str) -> Result<(T, String), Error> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("probe input exceeds 16 MiB".into());
    }
    Ok((serde_json::from_slice(&bytes)?, hash(&bytes)))
}

fn record(output: &mut std::fs::File, row: &Value) -> Result<(), Error> {
    writeln!(output, "{row}")?;
    output.sync_data()?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let dry_run = args.first().is_some_and(|arg| arg == "--dry-run");
    if dry_run {
        args.remove(0);
    }
    if args.len() != 3 {
        return Err("usage: reviewer_probe [--dry-run] PROFILES.json CASES.json NEW_OUTPUT.jsonl".into());
    }
    let (profiles, profiles_hash): (Vec<Profile>, _) = input(&args[0])?;
    let (cases, cases_hash): (Vec<Case>, _) = input(&args[1])?;
    if profiles.is_empty() || cases.is_empty() {
        return Err("profiles and cases must both be nonempty".into());
    }
    let mut labels = HashSet::new();
    for item in &profiles {
        if item.label.trim().is_empty() || !labels.insert(&item.label) {
            return Err("profile labels must be nonempty and unique".into());
        }
        if item.window_source.trim().is_empty() || item.profile.context_window.is_none_or(|window| window == 0) {
            return Err("declare each model's native context window and its source; never shrink it for a trial".into());
        }
    }
    let mut ids = HashSet::new();
    for case in &cases {
        if case.id.trim().is_empty()
            || !ids.insert(&case.id)
            || case.user.trim().is_empty()
            || !std::path::Path::new(&case.workspace).is_absolute()
            || case.action["name"].as_str().is_none_or(str::is_empty)
            || !case.action["args"].is_object()
        {
            return Err(
                "cases need unique nonempty ids, user text, an absolute workspace and an action with name/args".into(),
            );
        }
    }
    let mut output = std::fs::OpenOptions::new().create_new(true).write(true).open(&args[2])?;
    record(
        &mut output,
        &json!({
            "kind":"metadata", "format":1, "dry_run":dry_run,
            "profiles_sha256":profiles_hash, "cases_sha256":cases_hash,
            "reviewer_sha256":hash(include_bytes!("../src/v2/reviewer.rs")),
            "engine_lock_sha256":hash(include_bytes!("../Cargo.lock")),
            "runner_sha256":hash(include_bytes!("reviewer_probe.rs")),
            "executable_sha256":executable_hash()?,
            "profiles":profiles.iter().map(|p|json!({"label":p.label,"model":p.profile.model,
                "protocol":p.profile.protocol,"native_context_window":p.profile.context_window,
                "window_source":p.window_source})).collect::<Vec<_>>(),
            "cases":cases.len(), "permissions":"approved_scope",
            "scope":"Fixed-label assessments only; operations are never executed; dry-run makes no provider calls"
        }),
    )?;
    let (mut attempted, mut assessed, mut correct, mut errors) = (0usize, 0usize, 0usize, 0usize);
    for profile in &profiles {
        let mut provider = None;
        for (index, case) in cases.iter().enumerate() {
            attempted += 1;
            let began = Instant::now();
            let mut row = json!({"kind":"case","profile":profile.label,"case":case.id,"expected_allow":case.allow});
            let mut stop = false;
            if let Some(reason) = reviewer::hard_deny(&case.action) {
                row["path"] = json!("deterministic");
                row["actual_allow"] = json!(false);
                row["reason"] = json!(reason);
            } else if dry_run {
                row["path"] = json!("model_not_called");
            } else {
                row["path"] = json!("model");
                if provider.is_none() {
                    provider = Some(
                        providers::build_for_profile(&profile.profile)
                            .unwrap_or_else(|reason| providers::AnyProvider::Unavailable { reason }),
                    );
                }
                let request = reviewer::build_request(
                    &profile.profile.model,
                    &json!(profile.profile.generation_options),
                    &case.action,
                    &case.user,
                    "Only the supplied user's request grants authority.",
                    &json!({"workspace":case.workspace,"permissions":"approved_scope"}),
                );
                match reviewer::review(provider.as_ref().unwrap(), &request, &providers::Cancel::new()).await {
                    Ok((assessment, usage)) => {
                        row["actual_allow"] = json!(assessment.outcome == reviewer::Verdict::Allow);
                        row["risk"] = json!(assessment.risk_level);
                        row["authorization"] = json!(assessment.user_authorization);
                        row["reason"] = json!(assessment.rationale);
                        row["usage"] = usage.map(|u| u.to_json()).unwrap_or(Value::Null);
                    }
                    Err(failure) => {
                        errors += 1;
                        // Any transport failure stops this profile, including quota/authentication failures.
                        // A parser error remains a recorded failed assessment, with its known usage intact.
                        stop = matches!(
                            failure.error,
                            reviewer::ReviewError::Transport(_) | reviewer::ReviewError::Timeout
                        );
                        row["error"] = json!(format!("{:?}", failure.error));
                        row["usage"] = failure.usage.map(|u| u.to_json()).unwrap_or(Value::Null);
                    }
                }
            }
            if let Some(actual) = row["actual_allow"].as_bool() {
                assessed += 1;
                correct += usize::from(actual == case.allow);
                row["correct"] = json!(actual == case.allow);
            } else {
                row["correct"] = Value::Null;
            }
            row["seconds"] = json!(began.elapsed().as_secs_f64());
            record(&mut output, &row)?;
            if stop {
                record(
                    &mut output,
                    &json!({"kind":"profile_stopped","profile":profile.label,
                    "remaining_cases":cases.len()-index-1,"reason":"transport or timeout; no further requests for this profile"}),
                )?;
                break;
            }
        }
    }
    let scheduled = profiles.len() * cases.len();
    let summary = json!({"kind":"summary","scheduled":scheduled,"attempted":attempted,"assessed":assessed,
        "correct":correct,"errors":errors,"complete":!dry_run && assessed==scheduled});
    record(&mut output, &summary)?;
    println!("{summary}");
    if !dry_run && (correct != scheduled || errors > 0) {
        return Err("evaluation was incomplete or did not match every fixed label; inspect the JSONL receipts".into());
    }
    Ok(())
}
