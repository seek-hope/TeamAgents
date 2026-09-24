//! R2-P5 load probe (A32, scheme §11 P0 探针在 v2 生产路径上的复测): append /
//! step latency, control-boundary latency, request construction, restart,
//! multi-instance reads, RSS and disk growth over a ~1M-token synthetic
//! context built through the production `Control` plane.
//!
//!   cargo run --offline --manifest-path engine/Cargo.toml --example rebuild_p5_load -- \
//!     review/tmp/r2-p5-load [--steps 250] [--payload 20000]
//!
//! The probe writes `report.json` into a fresh evidence directory. Synthetic
//! text is not a tokenizer input: numbers describe local overhead, never model
//! capability.

use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};
use std::time::Instant;
use teamagents_core::kernel::{ContextEntry, EntryKind, KernelInstance, KernelProfile};
use teamagents_core::v2::{Command, Control, Identity};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn metric(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|line| line.strip_prefix(key).and_then(|tail| tail.split_whitespace().next()?.parse().ok()))
        .unwrap_or(0)
}

fn cpu_us() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // Linux fills the complete structure on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return 0;
    }
    let usage = unsafe { usage.assume_init() };
    ((usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) * 1_000_000 + usage.ru_utime.tv_usec + usage.ru_stime.tv_usec)
        as u64
}

fn percentiles(mut values: Vec<u64>) -> Json {
    values.sort_unstable();
    let at = |p: usize| values[(values.len().saturating_sub(1) * p) / 100];
    json!({"samples":values.len(),"p50":at(50),"p95":at(95),"max":values.last().copied().unwrap_or(0)})
}

fn cmd(id: impl Into<String>, method: &str, params: Json) -> Command {
    Command { command_id: id.into(), method: method.into(), params }
}

fn db_bytes(control: &Control, db: &Path) -> Fallible<u64> {
    control.connection().execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    Ok(std::fs::metadata(db)?.len())
}

fn revision(control: &Control) -> Fallible<i64> {
    Ok(control.connection().query_row("SELECT revision FROM instances WHERE id = 'i-main'", [], |row| row.get(0))?)
}

/// The model-visible view, exactly as the driver reads it (§7): one bounded
/// query per request, never a materialized copy of the whole history.
fn visible_entries(control: &Control) -> Fallible<Vec<ContextEntry>> {
    let mut stmt = control.connection().prepare(
        "SELECT id, kind, message_json, refs_json FROM context_entries
         WHERE instance_id = 'i-main' AND epoch = 0 AND compressed_by IS NULL
         ORDER BY CASE WHEN kind = 'summary' THEN 0 ELSE 1 END, idx",
    )?;
    let rows =
        stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?;
    let mut entries = Vec::new();
    for row in rows {
        let (id, kind, message) = row?;
        let kind = match kind.as_str() {
            "user" => EntryKind::User,
            "assistant" => EntryKind::Assistant,
            "tool_result" => EntryKind::ToolResult,
            _ => EntryKind::Note,
        };
        entries.push(ContextEntry::new(id, kind, serde_json::from_str(&message)?));
    }
    Ok(entries)
}

/// The daemon's history page (§9): its own WAL connection, a bounded SELECT,
/// rows materialized as JSON for the client — never a whole-history copy.
fn daemon_history_page(db: &Path, session: &str, instance: &str, limit: i64) -> Fallible<Vec<Json>> {
    let ctl = Control::open(db, session, false)?;
    let mut stmt = ctl.connection().prepare(
        "SELECT idx, kind, message_json FROM context_entries WHERE instance_id = ?1 ORDER BY idx DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![instance, limit], |row| {
        Ok(json!({"idx": row.get::<_, i64>(0)?, "kind": row.get::<_, String>(1)?,
                  "message": serde_json::from_str::<Json>(&row.get::<_, String>(2)?).unwrap_or(Json::Null)}))
    })?;
    let mut entries = rows.collect::<Result<Vec<_>, _>>()?;
    entries.reverse(); // chronological order
    Ok(entries)
}

/// `readers` concurrent bounded pages of the big instance (A32 多实例读取):
/// each reader opens its own WAL connection, exactly like the daemon does, so
/// readers never contend for the single writer (§4.1).
fn concurrent_pages(db: &Path, session: &str, readers: usize) -> Fallible<Json> {
    let started = Instant::now();
    let handles: Vec<_> = (0..readers)
        .map(|_| {
            let db = db.to_path_buf();
            let session = session.to_string();
            std::thread::spawn(move || {
                let at = Instant::now();
                let page = daemon_history_page(&db, &session, "i-main", 200);
                (page.map(|entries| entries.len()), at.elapsed().as_micros() as u64)
            })
        })
        .collect();
    let mut latency = Vec::new();
    for handle in handles {
        let (page, us) = handle.join().map_err(|_| "reader thread panicked")?;
        if page? != 200 {
            return Err("concurrent page lost entries".into());
        }
        latency.push(us);
    }
    let mut report = percentiles(latency);
    report["readers"] = json!(readers);
    report["wall_us"] = json!(started.elapsed().as_micros() as u64);
    Ok(report)
}

fn main() -> Fallible<()> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().ok_or("usage: rebuild_p5_load DIR [--steps N] [--payload BYTES]")?);
    let mut steps = 250usize;
    let mut payload_bytes = 20_000usize;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("flag without value")?;
        match flag.as_str() {
            "--steps" => steps = value.parse()?,
            "--payload" => payload_bytes = value.parse()?,
            other => return Err(format!("unknown flag {other}").into()),
        }
    }
    if dir.exists() {
        return Err(format!("{} exists; measurements need a fresh evidence directory", dir.display()).into());
    }
    std::fs::create_dir_all(&dir)?;
    let db = dir.join("session.sqlite");
    let session = "s-a32";
    let payload = "unit ".repeat(payload_bytes / 5);
    let mut ctl = Control::open(&db, session, true)?;
    ctl.submit(
        cmd("boot-instance", "create_instance", json!({"id": "i-main", "workspace_ref": dir.to_string_lossy()})),
        Identity::User,
    )?;
    ctl.submit(cmd("boot-goal", "create_goal", json!({"id": "goal-s-a32", "instance_id": "i-main"})), Identity::User)?;
    // a second, small instance: reading it must never touch the big history
    ctl.submit(
        cmd("peer-instance", "create_instance", json!({"id": "i-peer", "workspace_ref": dir.to_string_lossy()})),
        Identity::User,
    )?;
    for index in 0..3 {
        ctl.submit(
            cmd(
                format!("peer-input-{index}"),
                "submit_input",
                json!({"instance_id": "i-peer", "envelope_id": format!("peer-{index}"), "text": "peer note"}),
            ),
            Identity::User,
        )?;
    }
    let initial_bytes = db_bytes(&ctl, &db)?;
    let cpu_before = cpu_us();
    let mut submit_us = Vec::with_capacity(steps);
    let mut step_us = Vec::with_capacity(steps);
    for index in 0..steps {
        let request_id = format!("req-{index}");
        let started = Instant::now();
        ctl.submit(
            cmd(
                format!("input-{index}"),
                "submit_input",
                json!({"instance_id": "i-main", "envelope_id": format!("env-{index}"),
                       "text": format!("step {index}: {payload}")}),
            ),
            Identity::User,
        )?;
        submit_us.push(started.elapsed().as_micros() as u64);
        // the production turn boundary: fix the request, record the attempt,
        // import the assistant entry (§4.2)
        ctl.submit(
            cmd(
                format!("begin-{request_id}"),
                "begin_request",
                json!({"instance_id": "i-main", "request_id": request_id, "revision": revision(&ctl)?,
                       "est_prompt_tokens": (payload_bytes / 4 + 2_000) as i64}),
            ),
            Identity::Instance("i-main".into()),
        )?;
        ctl.submit(
            cmd(
                format!("attempt-{request_id}"),
                "record_attempt",
                json!({"attempt_id": format!("{request_id}/a1"), "request_id": request_id, "status": "COMPLETE",
                       "usage": {"prompt_tokens": payload_bytes / 4, "completion_tokens": 32,
                                 "total_tokens": payload_bytes / 4 + 32}}),
            ),
            Identity::System,
        )?;
        ctl.submit(
            cmd(
                format!("import-{request_id}"),
                "import_response",
                json!({"request_id": request_id, "decision_id": format!("d-{request_id}"),
                       "entry": {"role": "assistant", "content": format!("acknowledged step {index}")}}),
            ),
            Identity::System,
        )?;
        step_us.push(started.elapsed().as_micros() as u64);
    }
    let growth_bytes = db_bytes(&ctl, &db)?.saturating_sub(initial_bytes);
    let rss_after_append_kib = metric("VmRSS:");
    let payload_total = (payload_bytes * steps) as u64;

    // request construction over the visible view (system + entries + tools)
    let entries = visible_entries(&ctl)?;
    let kernel = KernelInstance::new(
        "i-main",
        0,
        KernelProfile {
            model: "synthetic".into(),
            instructions: "load probe".into(),
            tools: vec![json!({"type": "function", "function": {"name": "shell",
                "description": "run a shell command",
                "parameters": {"type": "object", "properties": {"command": {"type": "string"}},
                               "required": ["command"]}}})],
            options: json!({}),
            context_window: Some(1_000_000),
        },
    );
    let mut build_us = Vec::new();
    let mut request = None;
    for _ in 0..5 {
        let started = Instant::now();
        let built = kernel.prepare_request(&entries, "req-load");
        build_us.push(started.elapsed().as_micros() as u64);
        request = Some(built);
    }
    let request = request.ok_or("no request built")?;
    let wire_bytes = json!({"messages": request.messages, "tools": request.tools}).to_string().len();

    // restart: reopen verifies the format/version stamp, then the daemon's
    // bounded page shape (A32)
    drop(ctl);
    let reopened_at = Instant::now();
    let ctl = Control::open(&db, session, false)?;
    let reopen_us = reopened_at.elapsed().as_micros() as u64;
    let mut history_us = Vec::new();
    let mut page_entries = 0;
    for _ in 0..5 {
        let started = Instant::now();
        page_entries = daemon_history_page(&db, session, "i-main", 200)?.len();
        history_us.push(started.elapsed().as_micros() as u64);
    }
    // A32 also asks for multi-instance reads: 1/4/16 concurrent readers
    let mut concurrent = serde_json::Map::new();
    for readers in [1usize, 4, 16] {
        concurrent.insert(readers.to_string(), concurrent_pages(&db, session, readers)?);
    }
    // the peer's page is bounded by its own history, not the big instance's
    let peer = daemon_history_page(&db, session, "i-peer", 200)?;
    let peer_bytes: usize = peer.iter().map(|entry| entry.to_string().len()).sum();
    // read-style commands through Control::submit keep their whole result as
    // a replay receipt (the daemon avoids this path): measure that cost once
    let before_receipt = db_bytes(&ctl, &db)?;
    let mut ctl = ctl;
    ctl.submit(
        cmd("read-page-receipt", "read_history", json!({"instance_id": "i-main", "limit": 200})),
        Identity::User,
    )?;
    let read_receipt_bytes = db_bytes(&ctl, &db)?.saturating_sub(before_receipt);
    if growth_bytes > payload_total + 8 * 1024 * 1024 {
        return Err(format!(
            "per-step storage copied the whole history: growth {growth_bytes} for {payload_total} bytes"
        )
        .into());
    }
    if page_entries != 200 || peer.len() != 3 {
        return Err(format!("page bounds wrong: main {page_entries}, peer {}", peer.len()).into());
    }
    let report = json!({
        "probe": "r2-p5-load",
        "date": "2026-09-24",
        "path": "core::v2 Control (production entry) + core::kernel prepare_request",
        "synthetic": {"steps": steps, "payload_bytes_per_step": payload_bytes, "payload_bytes_total": payload_total,
                      "synthetic_units": payload_total / 5, "real_tokenizer": false,
                      "estimated_prompt_tokens": request.est_prompt_tokens},
        "control_submit_input_us": percentiles(submit_us),
        "turn_step_us": percentiles(step_us),
        "request_build_us": percentiles(build_us),
        "request_messages": request.messages.len(),
        "request_wire_bytes": wire_bytes,
        "database_growth_bytes": growth_bytes,
        "database_growth_per_step_bytes": growth_bytes / steps.max(1) as u64,
        "reopen_verify_us": reopen_us,
        "daemon_history_page_us": percentiles(history_us),
        "daemon_history_page_entries": page_entries,
        "concurrent_history_pages": Json::Object(concurrent),
        "peer_page_entries": peer.len(),
        "peer_page_bytes": peer_bytes,
        "read_command_receipt_bytes": read_receipt_bytes,
        "cpu_us": cpu_us().saturating_sub(cpu_before),
        "rss_after_append_kib": rss_after_append_kib,
        "rss_kib_final": metric("VmRSS:"),
        "peak_rss_kib": metric("VmHWM:"),
        "sqlite_version": rusqlite::version(),
        "synchronous": "FULL",
        "journal_mode": "WAL",
    });
    std::fs::write(dir.join("report.json"), serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
