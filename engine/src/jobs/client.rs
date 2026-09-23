//! Driver-side job access (§6.2/§6.3): spawn, handshake and reconnect. All
//! round-trips carry the operation-bound job id; the persisted journal is
//! the fallback when the runner process is gone (A11).

use super::{JobSpec, Journal};
use serde_json::{json, Value as Json};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Write job.json and spawn the runner process. Spawning a second runner
/// over a live job fails on the runner lock — one executor per job (A10).
pub async fn spawn(root: &Path, spec: &JobSpec) -> Result<(), String> {
    super::runner::spawn(root, spec)?;
    // Wait for the READY handshake: the runner persists its journal before
    // binding the socket, so a bound socket implies READY or a recovered state.
    for _ in 0..200 {
        match status(root).await {
            Ok(_) => return Ok(()),
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
        }
    }
    Err("runner did not report within 2s of spawn".into())
}

pub async fn request(root: &Path, method: &str) -> Result<Json, String> {
    let stream = UnixStream::connect(root.join("runner.sock")).await.map_err(|e| format!("connect runner: {e}"))?;
    let (read, mut write) = stream.into_split();
    write
        .write_all(format!("{}\n", json!({"version": 1, "method": method})).as_bytes())
        .await
        .map_err(|e| format!("send {method}: {e}"))?;
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await.map_err(|e| format!("read {method} reply: {e}"))?;
    let reply: Json = serde_json::from_str(&line).map_err(|e| format!("parse {method} reply: {e}"))?;
    if reply["ok"] != json!(true) {
        return Err(reply["error"].as_str().unwrap_or("runner error").to_string());
    }
    Ok(reply)
}

async fn journal_of(root: &Path, method: &str) -> Result<Journal, String> {
    let reply = request(root, method).await?;
    serde_json::from_value(reply["journal"].clone()).map_err(|e| format!("{method} journal: {e}"))
}

/// GO is idempotent: a duplicate GO returns the same journal without
/// starting a second command (A10).
pub async fn go(root: &Path) -> Result<Journal, String> {
    journal_of(root, "go").await
}

/// CANCEL is serialized with GO by the runner; before start it persists
/// CANCELLED_BEFORE_START, after start it stops the process group.
pub async fn cancel(root: &Path) -> Result<Journal, String> {
    journal_of(root, "cancel").await
}

pub async fn status(root: &Path) -> Result<Journal, String> {
    journal_of(root, "status").await
}

/// Test hooks (only honored by runners spawned with the hooks env).
pub async fn inject(root: &Path, method: &str) -> Result<Json, String> {
    request(root, method).await
}

/// The persisted journal, for judging a dead runner (§6.3): START_ACCEPTED
/// without a live runner means OUTCOME_UNKNOWN, never "did not run".
pub fn persisted_journal(root: &Path) -> Option<Journal> {
    let bytes = std::fs::read(root.join("journal.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Job output (the runner's own controlled log; §6.2).
pub fn read_output(root: &Path, max_bytes: usize) -> String {
    let Ok(bytes) = std::fs::read(root.join("output.log")) else { return String::new() };
    if bytes.len() <= max_bytes {
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    // keep the head, like the synchronous capture does
    String::from_utf8_lossy(&bytes[..max_bytes]).into_owned()
}

/// Graceful runner shutdown (only accepted with no active child).
pub async fn shutdown(root: &Path) -> Result<(), String> {
    request(root, "shutdown").await.map(|_| ())
}
