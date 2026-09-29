//! `teamagents surface` — the reader for D-350's per-request record (the decision D-341 made for D-143).
//!
//! `TEAMAGENTS_LOG_SURFACE=1` made the driver *log* the surface once per request, and that witness is what
//! answered a divergence that had cost two failed probes — but a log line is not a record, and the surface is
//! assembled per request while only the instance's *configured* profile is persisted. This reads back, per
//! request, the tool names the request was offered and whether the surface check authorized that set: it is a
//! read-only view of what the run recorded, not a new authority, and it never rewrites a record.

use serde_json::{json, Value as Json};
use std::path::PathBuf;

use super::approvals::connect;
use super::exec::resolve_prefix;

pub enum SurfaceCommand {
    List { instance: Option<String> },
}

pub struct SurfaceOptions {
    pub socket: PathBuf,
    pub command: SurfaceCommand,
    pub json_out: bool,
}

pub fn run(options: SurfaceOptions) -> i32 {
    match execute(&options) {
        Ok(report) => {
            print_report(&options, &report);
            0
        }
        Err((code, message)) => {
            eprintln!("{message}");
            code
        }
    }
}

pub fn execute(options: &SurfaceOptions) -> Result<Json, (i32, String)> {
    let mut client = connect(&options.socket)?;
    let instance = match &options.command {
        // an id the caller names resolves by the shared prefix rule against the live session, so a typo is a
        // client error here rather than a read that silently returns nothing
        SurfaceCommand::List { instance: Some(id) } => {
            let view = client
                .call("checkpoint", json!({}))
                .map_err(|error| (1, format!("surface: cannot read the session: {error}")))?;
            let rows = view["snapshot"]["instances"].as_array().cloned().unwrap_or_default();
            let row = resolve_prefix(&rows, "id", id, "instance", "teamagents surface")?;
            Some(row["id"].as_str().unwrap_or_default().to_string())
        }
        SurfaceCommand::List { instance: None } => None,
    };
    let mut params = json!({});
    if let Some(instance) = &instance {
        params["instance_id"] = json!(instance);
    }
    let view = client
        .call("surfaces", params)
        .map_err(|error| (1, format!("surface: cannot read the surface records: {error}")))?;
    Ok(json!({
        "session_id": client.session_id, "state_root": client.state_root,
        "instance_id": instance, "surfaces": view["surfaces"],
    }))
}

fn print_report(options: &SurfaceOptions, report: &Json) {
    if options.json_out {
        println!("{}", serde_json::to_string(report).unwrap_or_else(|_| "{}".into()));
        return;
    }
    let rows = report["surfaces"].as_array().cloned().unwrap_or_default();
    if let Some(instance) = report["instance_id"].as_str() {
        println!("surface records for {instance}:");
    } else {
        println!("{} surface record(s), newest first:", rows.len());
    }
    for row in rows {
        let tools = match row["offered_tools"].as_array() {
            Some(names) => {
                let names: Vec<&str> = names.iter().filter_map(|name| name.as_str()).collect();
                names.join(",")
            }
            None => "not recorded (this request predates the record)".to_string(),
        };
        println!(
            "  {} {} {} tools={} authorized={}",
            row["request_id"].as_str().unwrap_or(""),
            row["instance_id"].as_str().unwrap_or(""),
            row["kind"].as_str().unwrap_or(""),
            tools,
            row["surface_authorized"].as_bool().unwrap_or(false),
        );
    }
}
