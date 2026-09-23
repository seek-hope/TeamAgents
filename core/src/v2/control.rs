//! R2 v2 Control: the single trusted transaction entry (plan §3, §4.2).
//! `submit(command, trusted_identity)` runs ingest → validate → reduce →
//! persist in ONE SQLite transaction; errors roll back and propagate. All
//! identity, operation ids, permission revisions and sequence numbers are
//! filled here — never taken from model or client fields.

use super::store;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value as Json};
use std::path::Path;

/// Identity the runtime has already authenticated; never parsed out of
/// command params. User input and instance actions take different paths (§5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    User,
    Instance(String),
    System,
}

impl Identity {
    fn actor(&self) -> String {
        match self {
            Identity::User => "user".into(),
            Identity::Instance(id) => id.clone(),
            Identity::System => "system".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Command {
    /// Stable across client reconnects/retries (§9).
    pub command_id: String,
    pub method: String,
    pub params: Json,
}

pub struct Control {
    conn: Connection,
    pub session_id: String,
}

fn payload_hash(method: &str, params: &Json, identity: &Identity) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(json!({"method": method, "params": params, "identity": identity.actor()}).to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

impl Control {
    pub fn open(path: &Path, session_id: &str, create: bool) -> Result<Control, String> {
        let conn = store::open(path, create)?;
        Ok(Control { conn, session_id: session_id.to_string() })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// The single entry point. Duplicate command ids return the stored
    /// receipt; the same id with a different payload is rejected (§6.3).
    pub fn submit(&mut self, command: Command, identity: Identity) -> Result<Json, String> {
        if command.command_id.is_empty() {
            return Err("command_id must not be empty".into());
        }
        let hash = payload_hash(&command.method, &command.params, &identity);
        let existing: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT payload_hash, result_json FROM commands WHERE command_id = ?1",
                [&command.command_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("command dedup read: {e}"))?;
        if let Some((stored_hash, result)) = existing {
            if stored_hash != hash {
                return Err(format!("command_id {} was already used with a different payload", command.command_id));
            }
            return serde_json::from_str(&result).map_err(|e| format!("stored receipt: {e}"));
        }
        let method = command.method.clone();
        let params = command.params.clone();
        let tx = self.conn.transaction().map_err(|e| format!("begin tx: {e}"))?;
        let result = dispatch(&tx, &self.session_id, &method, &params, &identity)?;
        tx.execute(
            "INSERT INTO commands (command_id, payload_hash, result_json, created) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![command.command_id, hash, result.to_string(), crate::models::now()],
        )
        .map_err(|e| format!("command receipt: {e}"))?;
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(result)
    }
}

fn dispatch(
    tx: &Connection,
    session_id: &str,
    method: &str,
    params: &Json,
    identity: &Identity,
) -> Result<Json, String> {
    match method {
        "create_instance" => create_instance(tx, session_id, params),
        "create_goal" => create_goal(tx, session_id, params),
        "submit_input" => submit_input(tx, session_id, params, identity),
        "begin_request" => begin_request(tx, session_id, params, identity),
        "record_attempt" => record_attempt(tx, session_id, params),
        "import_response" => import_response(tx, session_id, params, identity),
        "complete_operation" => complete_operation(tx, session_id, params),
        "artifact_stage" => artifact_stage(tx, session_id, params),
        "artifact_publish" => artifact_publish(tx, session_id, params),
        "artifact_gc_claim" => artifact_gc_claim(tx, session_id, params),
        other => Err(format!("unknown v2 method {other:?}")),
    }
}

fn event(tx: &Connection, session_id: &str, kind: &str, scope: &str, payload: &Json) -> Result<(), String> {
    let sequence: i64 = tx
        .query_row("SELECT COALESCE(MAX(sequence), 0) + 1 FROM events WHERE session_id = ?1", [session_id], |row| {
            row.get(0)
        })
        .map_err(|e| format!("event sequence: {e}"))?;
    tx.execute(
        "INSERT INTO events (session_id, sequence, kind, scope, payload_json, created)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![session_id, sequence, kind, scope, payload.to_string(), crate::models::now()],
    )
    .map_err(|e| format!("event insert: {e}"))?;
    Ok(())
}

fn load_instance(tx: &Connection, id: &str) -> Result<(String, i64, String, String, i64), String> {
    tx.query_row(
        "SELECT session_id, context_epoch, lifecycle, phase, revision FROM instances WHERE id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    )
    .map_err(|e| format!("instance {id}: {e}"))
}

fn create_instance(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let id = params["id"].as_str().ok_or("create_instance.id required")?;
    let profile = params.get("profile").cloned().unwrap_or(json!({}));
    let workspace = params["workspace_ref"].as_str().unwrap_or("");
    tx.execute(
        "INSERT INTO instances
         (id, session_id, profile_revision, workspace_ref, context_epoch, lifecycle, phase,
          revision, active_goal_id, active_request_id, context_head, profile_json)
         VALUES (?1, ?2, 1, ?3, 0, 'ACTIVE', 'READY', 0, NULL, NULL, 0, ?4)",
        rusqlite::params![id, session_id, workspace, profile.to_string()],
    )
    .map_err(|e| format!("create_instance {id}: {e}"))?;
    event(tx, session_id, "instance_created", id, &json!({"instance_id": id}))?;
    Ok(json!({"instance_id": id, "phase": "READY", "revision": 0, "epoch": 0}))
}

fn create_goal(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let id = params["id"].as_str().ok_or("create_goal.id required")?;
    let original = params["original_request_ref"].as_str().unwrap_or("");
    let limits = params.get("limits").cloned().unwrap_or(json!({}));
    let deadline = params["deadline"].as_f64();
    tx.execute(
        "INSERT INTO goals
         (id, session_id, original_request_ref, requirement_revision, status, deadline,
          limits_json, known_usage_json, reservations_json, unknown_usage)
         VALUES (?1, ?2, ?3, 1, 'ACTIVE', ?4, ?5, ?6, '{}', 0)",
        rusqlite::params![
            id,
            session_id,
            original,
            deadline,
            limits.to_string(),
            json!(crate::kernel::Usage::default()).to_string()
        ],
    )
    .map_err(|e| format!("create_goal {id}: {e}"))?;
    event(tx, session_id, "goal_created", id, &json!({"goal_id": id}))?;
    Ok(json!({"goal_id": id, "status": "ACTIVE"}))
}

/// Accept boundary for input (§4.2, §5.4): context append + apply dedup +
/// phase, one transaction. Replayed envelopes dedup by
/// (instance, epoch, envelope_id); a different payload under a replayed id
/// is rejected by the caller's command receipt check.
fn submit_input(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let instance_id = params["instance_id"].as_str().ok_or("submit_input.instance_id required")?;
    let envelope_id = params["envelope_id"].as_str().ok_or("submit_input.envelope_id required")?;
    let text = params["text"].as_str().ok_or("submit_input.text required")?;
    let (session, epoch, lifecycle, _phase, revision) = load_instance(tx, instance_id)?;
    if session != session_id {
        return Err(format!("instance {instance_id} does not belong to this session"));
    }
    if lifecycle == "TERMINATED" {
        return Err(format!("instance {instance_id} is terminated"));
    }
    let sender = identity.actor();
    let sequence: i64 = tx
        .query_row("SELECT COALESCE(MAX(sequence), 0) + 1 FROM envelopes WHERE session_id = ?1", [session_id], |row| {
            row.get(0)
        })
        .map_err(|e| format!("envelope sequence: {e}"))?;
    tx.execute(
        "INSERT INTO envelopes
         (id, session_id, sender, recipient, epoch, kind, correlation_id, payload_json, sequence, state)
         VALUES (?1, ?2, ?3, ?4, ?5, 'user_input', NULL, ?6, ?7, 'ACCEPTED')",
        rusqlite::params![
            envelope_id,
            session_id,
            sender,
            instance_id,
            epoch,
            json!({"text": text}).to_string(),
            sequence
        ],
    )
    .map_err(|e| format!("envelope {envelope_id}: {e}"))?;
    let applied = append_context(
        tx,
        instance_id,
        epoch,
        "user",
        &json!({"role": "user", "content": text}),
        Some(envelope_id),
        &[],
    )?;
    if applied {
        tx.execute("UPDATE envelopes SET state = 'APPLIED' WHERE id = ?1", [envelope_id])
            .map_err(|e| format!("envelope apply: {e}"))?;
        // a fresh input makes the instance runnable again at the next boundary
        tx.execute(
            "UPDATE instances SET phase = 'READY', revision = revision + 1 WHERE id = ?1 AND phase = 'WAITING'",
            [instance_id],
        )
        .map_err(|e| format!("instance wake: {e}"))?;
    }
    publish_list(tx, session_id, params)?;
    event(tx, session_id, "input", instance_id, &json!({"envelope_id": envelope_id, "applied": applied}))?;
    Ok(json!({"envelope_id": envelope_id, "applied": applied, "instance_revision": revision}))
}

/// Append one context entry; returns false when the (instance, epoch,
/// envelope) application was already recorded (dedup, §4.2).
fn append_context(
    tx: &Connection,
    instance_id: &str,
    epoch: i64,
    kind: &str,
    message: &Json,
    envelope_id: Option<&str>,
    refs: &[String],
) -> Result<bool, String> {
    if let Some(envelope) = envelope_id {
        let seen: Option<i64> = tx
            .query_row(
                "SELECT idx FROM context_entries WHERE instance_id = ?1 AND epoch = ?2 AND envelope_id = ?3",
                rusqlite::params![instance_id, epoch, envelope],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("context dedup: {e}"))?;
        if seen.is_some() {
            return Ok(false);
        }
    }
    let idx: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(idx), 0) + 1 FROM context_entries WHERE instance_id = ?1 AND epoch = ?2",
            rusqlite::params![instance_id, epoch],
            |row| row.get(0),
        )
        .map_err(|e| format!("context idx: {e}"))?;
    let id = format!("{instance_id}:{epoch}:{idx}");
    tx.execute(
        "INSERT INTO context_entries (instance_id, epoch, idx, id, kind, message_json, envelope_id, refs_json, created)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            instance_id,
            epoch,
            idx,
            id,
            kind,
            message.to_string(),
            envelope_id,
            json!(refs).to_string(),
            crate::models::now()
        ],
    )
    .map_err(|e| format!("context append: {e}"))?;
    tx.execute("UPDATE instances SET context_head = ?1 WHERE id = ?2", rusqlite::params![idx, instance_id])
        .map_err(|e| format!("context head: {e}"))?;
    Ok(true)
}

/// READY → MODEL_PENDING with the fixed request and budget reservation (§3).
/// The revision check is the single-executor guarantee (§6.1).
fn begin_request(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let instance_id = params["instance_id"].as_str().ok_or("begin_request.instance_id required")?;
    let request_id = params["request_id"].as_str().ok_or("begin_request.request_id required")?;
    let request_ref = params["request_ref"].as_str().unwrap_or("");
    let expected_revision = params["revision"].as_i64().ok_or("begin_request.revision required")?;
    let est = params["est_prompt_tokens"].as_i64().unwrap_or(0);
    let (session, epoch, lifecycle, phase, revision) = load_instance(tx, instance_id)?;
    if session != session_id {
        return Err("instance does not belong to this session".to_string());
    }
    if lifecycle != "ACTIVE" {
        return Err(format!("instance {instance_id} is {lifecycle}, not dispatching"));
    }
    if phase != "READY" {
        return Err(format!("instance {instance_id} is {phase}, not READY"));
    }
    if revision != expected_revision {
        return Err(format!(
            "instance {instance_id} revision {revision} != expected {expected_revision}: stale executor"
        ));
    }
    let goal_id: Option<String> = tx
        .query_row("SELECT active_goal_id FROM instances WHERE id = ?1", [instance_id], |row| row.get(0))
        .map_err(|e| format!("goal read: {e}"))?;
    tx.execute(
        "INSERT INTO model_requests (request_id, instance_id, epoch, goal_id, request_ref, status, est_prompt_tokens)
         VALUES (?1, ?2, ?3, ?4, ?5, 'PENDING', ?6)",
        rusqlite::params![request_id, instance_id, epoch, goal_id, request_ref, est],
    )
    .map_err(|e| format!("begin_request {request_id}: {e}"))?;
    tx.execute(
        "UPDATE instances SET phase = 'MODEL_PENDING', active_request_id = ?1, revision = revision + 1 WHERE id = ?2",
        rusqlite::params![request_id, instance_id],
    )
    .map_err(|e| format!("begin_request phase: {e}"))?;
    let _ = identity;
    publish_list(tx, session_id, params)?;
    event(tx, session_id, "request_began", instance_id, &json!({"request_id": request_id}))?;
    Ok(json!({"request_id": request_id, "phase": "MODEL_PENDING", "epoch": epoch}))
}

/// One transport attempt's outcome. A COMPLETE attempt is atomically selected
/// as the unique response; late completes stay archived and billed (§7, A19).
fn record_attempt(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let attempt_id = params["attempt_id"].as_str().ok_or("record_attempt.attempt_id required")?;
    let request_id = params["request_id"].as_str().ok_or("record_attempt.request_id required")?;
    let status = params["status"].as_str().ok_or("record_attempt.status required")?;
    let elapsed = params["elapsed_ms"].as_i64().unwrap_or(0);
    let usage = params.get("usage").cloned().unwrap_or(Json::Null);
    let response_ref = params["response_ref"].as_str();
    tx.execute(
        "INSERT INTO attempts (attempt_id, request_id, status, response_ref, usage_json, elapsed_ms, created)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            attempt_id,
            request_id,
            status,
            response_ref,
            usage.to_string(),
            elapsed,
            crate::models::now()
        ],
    )
    .map_err(|e| format!("record_attempt {attempt_id}: {e}"))?;
    let mut selected = false;
    if status == "COMPLETE" {
        let changed = tx
            .execute(
                "UPDATE model_requests SET selected_attempt_id = ?1 WHERE request_id = ?2 AND selected_attempt_id IS NULL",
                rusqlite::params![attempt_id, request_id],
            )
            .map_err(|e| format!("select attempt: {e}"))?;
        selected = changed == 1;
        // every complete response is billed to the goal; only the selected
        // one advances the context (§7: others are archived and accounted)
        if let Some(goal) = request_goal(tx, request_id)? {
            settle_usage(tx, &goal, &usage)?;
        }
    }
    event(
        tx,
        session_id,
        "attempt_recorded",
        request_id,
        &json!({"attempt_id": attempt_id, "status": status, "selected": selected}),
    )?;
    Ok(json!({"attempt_id": attempt_id, "selected": selected}))
}

fn request_goal(tx: &Connection, request_id: &str) -> Result<Option<String>, String> {
    tx.query_row("SELECT goal_id FROM model_requests WHERE request_id = ?1", [request_id], |row| row.get(0))
        .map_err(|e| format!("request goal: {e}"))
}

/// Settle reported usage into the goal budget (§8); unknown usage stays a
/// separate visible counter, never silently dropped.
fn settle_usage(tx: &Connection, goal_id: &str, usage: &Json) -> Result<(), String> {
    let parsed = crate::kernel::Usage::from_json(&json!({"usage": usage})).unwrap_or_default();
    let current: String = tx
        .query_row("SELECT known_usage_json FROM goals WHERE id = ?1", [goal_id], |row| row.get(0))
        .map_err(|e| format!("goal usage read: {e}"))?;
    let mut known: crate::kernel::Usage = serde_json::from_str(&current).unwrap_or_default();
    known.prompt += parsed.prompt;
    known.completion += parsed.completion;
    known.total += parsed.total;
    tx.execute(
        "UPDATE goals SET known_usage_json = ?1 WHERE id = ?2",
        rusqlite::params![json!(known).to_string(), goal_id],
    )
    .map_err(|e| format!("goal usage settle: {e}"))?;
    Ok(())
}

/// Complete-response import (§4.2): unique response reference + conversation
/// append + decision + tool intents/completion + usage — ONE transaction.
/// Re-import of the same request dedups by decision_id.
fn import_response(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    let request_id = params["request_id"].as_str().ok_or("import_response.request_id required")?;
    let decision_id = params["decision_id"].as_str().ok_or("import_response.decision_id required")?;
    let entry_message = params.get("entry").ok_or("import_response.entry required")?;
    let intents = params["intents"].as_array().cloned().unwrap_or_default();
    let completion = params.get("completion").cloned();
    let (request_instance, epoch, request_status): (String, i64, String) = tx
        .query_row("SELECT instance_id, epoch, status FROM model_requests WHERE request_id = ?1", [request_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|e| format!("import_response request {request_id}: {e}"))?;
    if request_status != "PENDING" {
        return Err(format!("request {request_id} is {request_status}, already imported or failed"));
    }
    let selected: Option<String> = tx
        .query_row("SELECT selected_attempt_id FROM model_requests WHERE request_id = ?1", [request_id], |row| {
            row.get(0)
        })
        .map_err(|e| format!("import_response select: {e}"))?;
    if selected.is_none() {
        return Err(format!("request {request_id} has no selected complete attempt; cannot import"));
    }
    let inserted = tx
        .execute(
            "INSERT INTO decisions (decision_id, request_id) VALUES (?1, ?2)",
            rusqlite::params![decision_id, request_id],
        )
        .map_err(|e| format!("decision {decision_id}: {e}"))?;
    if inserted != 1 {
        return Err(format!("decision {decision_id} not inserted"));
    }
    append_context(tx, &request_instance, epoch, "assistant", entry_message, Some(decision_id), &[])?;
    let goal_id = request_goal(tx, request_id)?;
    for intent in &intents {
        let index = intent["index"].as_i64().ok_or("intent.index required")?;
        let operation_id = format!("{decision_id}:{index}");
        tx.execute(
            "INSERT INTO operations
             (operation_id, decision_id, tool_index, goal_id, epoch, args_hash, grant_revision, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'PREPARED')",
            rusqlite::params![
                operation_id,
                decision_id,
                index,
                goal_id,
                epoch,
                intent["args_hash"].as_str().unwrap_or(""),
                intent["grant_revision"].as_i64().unwrap_or(0),
            ],
        )
        .map_err(|e| format!("operation {operation_id}: {e}"))?;
    }
    let phase = if completion.is_some() {
        "COMPLETION_PENDING"
    } else if intents.is_empty() {
        "READY"
    } else {
        "TOOLS_PENDING"
    };
    tx.execute(
        "UPDATE instances SET phase = ?1, active_request_id = NULL, revision = revision + 1 WHERE id = ?2",
        rusqlite::params![phase, request_instance],
    )
    .map_err(|e| format!("import phase: {e}"))?;
    tx.execute("UPDATE model_requests SET status = 'COMPLETE' WHERE request_id = ?1", [request_id])
        .map_err(|e| format!("request complete: {e}"))?;
    let _ = identity;
    publish_list(tx, session_id, params)?;
    event(
        tx,
        session_id,
        "response_imported",
        &request_instance,
        &json!({"request_id": request_id, "decision_id": decision_id, "intents": intents.len(), "phase": phase}),
    )?;
    Ok(json!({"decision_id": decision_id, "phase": phase, "operations": intents.len()}))
}

/// Terminal tool receipt import (§4.2): unique operation receipt + the
/// instance's consumption event in the same transaction. Replaying the same
/// terminal receipt returns the stored state without a second application.
fn complete_operation(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let operation_id = params["operation_id"].as_str().ok_or("complete_operation.operation_id required")?;
    let status = params["status"].as_str().ok_or("complete_operation.status required")?;
    let receipt = params.get("receipt").cloned().unwrap_or(Json::Null);
    let (decision_id, current): (String, String) = tx
        .query_row("SELECT decision_id, status FROM operations WHERE operation_id = ?1", [operation_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| format!("operation {operation_id}: {e}"))?;
    if is_terminal_op(&current) {
        return Err(format!("operation {operation_id} already terminal ({current}); refusing to overwrite"));
    }
    tx.execute(
        "UPDATE operations SET status = ?1, receipt_json = ?2 WHERE operation_id = ?3",
        rusqlite::params![status, receipt.to_string(), operation_id],
    )
    .map_err(|e| format!("operation complete: {e}"))?;
    // when every operation of the decision is terminal the instance consumes
    // the receipts and returns to READY (the driver appends observations)
    let remaining: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE decision_id = ?1 AND status IN ('PREPARED', 'DISPATCH_COMMITTED', 'RUNNING')",
            [&decision_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("decision remaining: {e}"))?;
    if remaining == 0 {
        let instance: String = tx
            .query_row(
                "SELECT r.instance_id FROM model_requests r JOIN decisions d ON d.request_id = r.request_id
                 WHERE d.decision_id = ?1",
                [&decision_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("decision instance: {e}"))?;
        tx.execute(
            "UPDATE instances SET phase = 'READY', revision = revision + 1 WHERE id = ?1 AND phase = 'TOOLS_PENDING'",
            [&instance],
        )
        .map_err(|e| format!("decision ready: {e}"))?;
    }
    publish_list(tx, session_id, params)?;
    event(tx, session_id, "operation_completed", operation_id, &json!({"status": status}))?;
    Ok(json!({"operation_id": operation_id, "status": status, "decision_open": remaining > 0}))
}

fn is_terminal_op(status: &str) -> bool {
    matches!(status, "SUCCEEDED" | "FAILED" | "CANCELLED" | "CANCELLED_BEFORE_START" | "OUTCOME_UNKNOWN")
}

/// Artifact publication (§4.3): identity, digest and publishing owner are
/// registered STAGING before the file exists; turning LIVE happens in the
/// same transaction as the reference that points at it.
fn artifact_stage(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let id = params["id"].as_str().ok_or("artifact_stage.id required")?;
    let digest = params["digest"].as_str().ok_or("artifact_stage.digest required")?;
    let size = params["size"].as_i64().unwrap_or(0);
    let kind = params["kind"].as_str().unwrap_or("blob");
    let owner_scope = params["owner_scope"].as_str().unwrap_or("session");
    let storage_ref = params["storage_ref"].as_str().ok_or("artifact_stage.storage_ref required")?;
    let owner_ref = params["owner_ref"].as_str();
    tx.execute(
        "INSERT INTO artifacts (id, session_id, digest, size, kind, owner_scope, storage_ref, completeness, owner_ref, created)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'STAGING', ?8, ?9)",
        rusqlite::params![id, session_id, digest, size, kind, owner_scope, storage_ref, owner_ref, crate::models::now()],
    )
    .map_err(|e| format!("artifact_stage {id}: {e}"))?;
    Ok(json!({"artifact_id": id, "completeness": "STAGING"}))
}

/// STAGING → LIVE. References and the LIVE flip commit in the same
/// transaction (§4.3); a GC-claimed (DELETING) or abandoned artifact can
/// never be published — late references are rejected.
fn artifact_publish(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let id = params["id"].as_str().ok_or("artifact_publish.id required")?;
    publish_one(tx, session_id, id)
}

fn publish_one(tx: &Connection, session_id: &str, id: &str) -> Result<Json, String> {
    let changed = tx
        .execute(
            "UPDATE artifacts SET completeness = 'LIVE' WHERE id = ?1 AND session_id = ?2 AND completeness = 'STAGING'",
            rusqlite::params![id, session_id],
        )
        .map_err(|e| format!("artifact_publish {id}: {e}"))?;
    if changed != 1 {
        let state: Option<String> = tx
            .query_row("SELECT completeness FROM artifacts WHERE id = ?1", [id], |row| row.get(0))
            .optional()
            .map_err(|e| format!("artifact read {id}: {e}"))?;
        return Err(format!(
            "artifact {id} cannot be published from state {}",
            state.as_deref().unwrap_or("<missing>")
        ));
    }
    Ok(json!({"artifact_id": id, "completeness": "LIVE"}))
}

fn publish_list(tx: &Connection, session_id: &str, params: &Json) -> Result<(), String> {
    for id in params["publish"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
        publish_one(tx, session_id, id)?;
    }
    Ok(())
}

/// GC claim (§4.3): ownerless, unreferenced LIVE artifacts transition to
/// DELETING inside a transaction; new references are refused from then on.
/// STAGING and job-protected results are never claimed.
fn artifact_gc_claim(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let limit = params["limit"].as_i64().unwrap_or(100);
    let candidates: Vec<String> = {
        let mut stmt = tx
            .prepare(
                "SELECT id FROM artifacts
                 WHERE session_id = ?1 AND completeness = 'LIVE' AND owner_ref IS NULL
                   AND id NOT IN (SELECT payload_ref FROM context_entries WHERE payload_ref IS NOT NULL)
                   AND id NOT IN (SELECT request_ref FROM model_requests)
                   AND id NOT IN (SELECT response_ref FROM attempts WHERE response_ref IS NOT NULL)
                   AND id NOT IN (SELECT payload_ref FROM events WHERE payload_ref IS NOT NULL)
                 LIMIT ?2",
            )
            .map_err(|e| format!("gc prepare: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![session_id, limit], |row| row.get(0))
            .map_err(|e| format!("gc query: {e}"))?;
        rows.collect::<Result<Vec<String>, _>>().map_err(|e| format!("gc collect: {e}"))?
    };
    for id in &candidates {
        tx.execute("UPDATE artifacts SET completeness = 'DELETING' WHERE id = ?1 AND completeness = 'LIVE'", [id])
            .map_err(|e| format!("gc claim {id}: {e}"))?;
    }
    event(tx, session_id, "artifacts_gc_claimed", "", &json!({"claimed": candidates.len()}))?;
    Ok(json!({"claimed": candidates}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("teamagents-v2-control-{tag}-{}.db", uuid::Uuid::new_v4()))
    }

    fn cleanup(p: &std::path::Path) {
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(format!("{}-wal", p.display()));
        let _ = std::fs::remove_file(format!("{}-shm", p.display()));
    }

    fn control(tag: &str) -> (Control, std::path::PathBuf) {
        let path = db_path(tag);
        let ctl = Control::open(&path, "s1", true).expect("open control");
        (ctl, path)
    }

    fn cmd(id: &str, method: &str, params: Json) -> Command {
        Command { command_id: id.into(), method: method.into(), params }
    }

    fn create_instance(ctl: &mut Control, id: &str) {
        ctl.submit(cmd(&format!("ci-{id}"), "create_instance", json!({"id": id})), Identity::User)
            .expect("create instance");
    }

    fn context_count(ctl: &Control, instance: &str) -> i64 {
        ctl.connection()
            .query_row("SELECT COUNT(*) FROM context_entries WHERE instance_id = ?1", [instance], |row| row.get(0))
            .unwrap()
    }

    fn phase_of(ctl: &Control, instance: &str) -> String {
        ctl.connection().query_row("SELECT phase FROM instances WHERE id = ?1", [instance], |row| row.get(0)).unwrap()
    }

    #[test]
    fn command_replay_returns_stored_receipt_and_rejects_conflict() {
        let (mut ctl, path) = control("dedup");
        let first =
            ctl.submit(cmd("c1", "create_instance", json!({"id": "i1"})), Identity::User).expect("first submit");
        // same id + same payload: stored receipt, no re-execution
        let replayed = ctl.submit(cmd("c1", "create_instance", json!({"id": "i1"})), Identity::User).expect("replay");
        assert_eq!(first, replayed);
        let instances: i64 =
            ctl.connection().query_row("SELECT COUNT(*) FROM instances", [], |row| row.get(0)).unwrap();
        assert_eq!(instances, 1);
        // same id + different payload is a client bug and is rejected
        let err = ctl.submit(cmd("c1", "create_instance", json!({"id": "i2"})), Identity::User).unwrap_err();
        assert!(err.contains("different payload"), "{err}");
        // empty command ids are not dedup-able and rejected up front
        let err = ctl.submit(cmd("", "create_instance", json!({"id": "i3"})), Identity::User).unwrap_err();
        assert!(err.contains("empty"), "{err}");
        cleanup(&path);
    }

    #[test]
    fn submit_input_applies_context_once_per_envelope() {
        let (mut ctl, path) = control("input");
        create_instance(&mut ctl, "i1");
        let result = ctl
            .submit(
                cmd("in-1", "submit_input", json!({"instance_id": "i1", "envelope_id": "e1", "text": "hello"})),
                Identity::User,
            )
            .expect("input");
        assert_eq!(result["applied"], json!(true));
        assert_eq!(context_count(&ctl, "i1"), 1);
        // client retry with the same command id: stored receipt, still one entry
        let replayed = ctl
            .submit(
                cmd("in-1", "submit_input", json!({"instance_id": "i1", "envelope_id": "e1", "text": "hello"})),
                Identity::User,
            )
            .expect("retry");
        assert_eq!(replayed["applied"], json!(true));
        assert_eq!(context_count(&ctl, "i1"), 1);
        // the same envelope id under a new command never double-applies
        let err = ctl
            .submit(
                cmd("in-2", "submit_input", json!({"instance_id": "i1", "envelope_id": "e1", "text": "hello"})),
                Identity::User,
            )
            .unwrap_err();
        assert!(err.contains("envelope e1"), "{err}");
        assert_eq!(context_count(&ctl, "i1"), 1);
        cleanup(&path);
    }

    #[test]
    fn begin_request_requires_ready_phase_and_current_revision() {
        let (mut ctl, path) = control("begin");
        create_instance(&mut ctl, "i1");
        // revision guard: a stale executor cannot dispatch (§6.1)
        let err = ctl
            .submit(
                cmd("b-stale", "begin_request", json!({"instance_id": "i1", "request_id": "r1", "revision": 7})),
                Identity::Instance("i1".into()),
            )
            .unwrap_err();
        assert!(err.contains("stale executor"), "{err}");
        let ok = ctl
            .submit(
                cmd("b-1", "begin_request", json!({"instance_id": "i1", "request_id": "r1", "revision": 0})),
                Identity::Instance("i1".into()),
            )
            .expect("begin");
        assert_eq!(ok["phase"], json!("MODEL_PENDING"));
        assert_eq!(phase_of(&ctl, "i1"), "MODEL_PENDING");
        // a second dispatch while MODEL_PENDING is refused
        let err = ctl
            .submit(
                cmd("b-2", "begin_request", json!({"instance_id": "i1", "request_id": "r2", "revision": 1})),
                Identity::Instance("i1".into()),
            )
            .unwrap_err();
        assert!(err.contains("not READY"), "{err}");
        cleanup(&path);
    }

    #[test]
    fn begin_request_rejects_instances_of_other_sessions() {
        let path = db_path("xsession");
        let mut owner = Control::open(&path, "s-owner", true).expect("owner control");
        owner.submit(cmd("ci", "create_instance", json!({"id": "i1"})), Identity::User).expect("create");
        let mut other = Control::open(&path, "s-intruder", false).expect("second connection");
        let err = other
            .submit(
                cmd("b-x", "begin_request", json!({"instance_id": "i1", "request_id": "r1", "revision": 0})),
                Identity::Instance("i1".into()),
            )
            .unwrap_err();
        assert!(err.contains("does not belong"), "{err}");
        drop(other);
        drop(owner);
        cleanup(&path);
    }

    /// Drive one instance to a selected COMPLETE attempt; returns the request id.
    fn begin_and_complete(ctl: &mut Control, tag: &str, instance: &str, revision: i64) -> String {
        let request = format!("r-{tag}");
        ctl.submit(
            cmd(
                &format!("b-{tag}"),
                "begin_request",
                json!({"instance_id": instance, "request_id": request, "revision": revision}),
            ),
            Identity::Instance(instance.into()),
        )
        .expect("begin");
        ctl.submit(
            cmd(
                &format!("a-{tag}"),
                "record_attempt",
                json!({"attempt_id": format!("at-{tag}"), "request_id": request, "status": "COMPLETE",
                       "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}}),
            ),
            Identity::System,
        )
        .expect("attempt");
        request
    }

    #[test]
    fn attempt_selection_is_atomic_and_every_complete_is_billed() {
        let (mut ctl, path) = control("attempts");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1", "original_request_ref": "orig"})), Identity::User)
            .expect("goal");
        create_instance(&mut ctl, "i1");
        ctl.connection().execute("UPDATE instances SET active_goal_id = 'g1' WHERE id = 'i1'", []).unwrap();
        ctl.submit(
            cmd("b1", "begin_request", json!({"instance_id": "i1", "request_id": "r1", "revision": 0})),
            Identity::Instance("i1".into()),
        )
        .expect("begin");
        let first = ctl
            .submit(
                cmd(
                    "a1",
                    "record_attempt",
                    json!({"attempt_id": "at1", "request_id": "r1", "status": "COMPLETE",
                           "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}}),
                ),
                Identity::System,
            )
            .expect("first attempt");
        assert_eq!(first["selected"], json!(true));
        // a late complete stays archived and is still billed (§7, A19)
        let late = ctl
            .submit(
                cmd(
                    "a2",
                    "record_attempt",
                    json!({"attempt_id": "at2", "request_id": "r1", "status": "COMPLETE",
                           "usage": {"prompt_tokens": 20, "completion_tokens": 10, "total_tokens": 30}}),
                ),
                Identity::System,
            )
            .expect("late attempt");
        assert_eq!(late["selected"], json!(false));
        let failed = ctl
            .submit(
                cmd("a3", "record_attempt", json!({"attempt_id": "at3", "request_id": "r1", "status": "FAILED"})),
                Identity::System,
            )
            .expect("failed attempt");
        assert_eq!(failed["selected"], json!(false));
        let selected: String = ctl
            .connection()
            .query_row("SELECT selected_attempt_id FROM model_requests WHERE request_id = 'r1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(selected, "at1");
        let usage: String = ctl
            .connection()
            .query_row("SELECT known_usage_json FROM goals WHERE id = 'g1'", [], |row| row.get(0))
            .unwrap();
        let usage: Json = serde_json::from_str(&usage).unwrap();
        assert_eq!(usage["prompt"], json!(30));
        assert_eq!(usage["completion"], json!(15));
        assert_eq!(usage["total"], json!(45));
        cleanup(&path);
    }

    #[test]
    fn import_response_requires_selection_and_is_single_shot() {
        let (mut ctl, path) = control("import");
        create_instance(&mut ctl, "i1");
        ctl.submit(
            cmd("b1", "begin_request", json!({"instance_id": "i1", "request_id": "r1", "revision": 0})),
            Identity::Instance("i1".into()),
        )
        .expect("begin");
        // no complete attempt yet: nothing may be imported
        let err = ctl
            .submit(
                cmd(
                    "imp-early",
                    "import_response",
                    json!({"request_id": "r1", "decision_id": "d1", "entry": {"role": "assistant", "content": "hi"}}),
                ),
                Identity::System,
            )
            .unwrap_err();
        assert!(err.contains("no selected complete attempt"), "{err}");
        ctl.submit(
            cmd("a1", "record_attempt", json!({"attempt_id": "at1", "request_id": "r1", "status": "COMPLETE"})),
            Identity::System,
        )
        .expect("attempt");
        let imported = ctl
            .submit(
                cmd(
                    "imp-1",
                    "import_response",
                    json!({"request_id": "r1", "decision_id": "d1",
                           "entry": {"role": "assistant", "content": "working"},
                           "intents": [{"index": 0, "args_hash": "h0"}, {"index": 1, "args_hash": "h1"}]}),
                ),
                Identity::System,
            )
            .expect("import");
        assert_eq!(imported["phase"], json!("TOOLS_PENDING"));
        assert_eq!(imported["operations"], json!(2));
        assert_eq!(phase_of(&ctl, "i1"), "TOOLS_PENDING");
        // one assistant entry joined the context
        assert_eq!(context_count(&ctl, "i1"), 1);
        let prepared: i64 = ctl
            .connection()
            .query_row("SELECT COUNT(*) FROM operations WHERE decision_id = 'd1' AND status = 'PREPARED'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(prepared, 2);
        // the request is consumed: a second import is refused, nothing changes
        let err = ctl
            .submit(
                cmd(
                    "imp-2",
                    "import_response",
                    json!({"request_id": "r1", "decision_id": "d2",
                           "entry": {"role": "assistant", "content": "again"}}),
                ),
                Identity::System,
            )
            .unwrap_err();
        assert!(err.contains("already imported"), "{err}");
        assert_eq!(context_count(&ctl, "i1"), 1);
        cleanup(&path);
    }

    #[test]
    fn import_response_phases_for_reply_and_completion() {
        let (mut ctl, path) = control("phases");
        create_instance(&mut ctl, "i1");
        // plain reply without intents returns to READY
        let r1 = begin_and_complete(&mut ctl, "p1", "i1", 0);
        let reply = ctl
            .submit(
                cmd(
                    "imp-p1",
                    "import_response",
                    json!({"request_id": r1, "decision_id": "d-p1",
                           "entry": {"role": "assistant", "content": "answer"}}),
                ),
                Identity::System,
            )
            .expect("reply import");
        assert_eq!(reply["phase"], json!("READY"));
        // a completion candidate parks the instance in COMPLETION_PENDING
        let r2 = begin_and_complete(&mut ctl, "p2", "i1", 2);
        let done = ctl
            .submit(
                cmd(
                    "imp-p2",
                    "import_response",
                    json!({"request_id": r2, "decision_id": "d-p2",
                           "entry": {"role": "assistant", "content": "done"},
                           "completion": {"summary": "task finished"}}),
                ),
                Identity::System,
            )
            .expect("completion import");
        assert_eq!(done["phase"], json!("COMPLETION_PENDING"));
        assert_eq!(phase_of(&ctl, "i1"), "COMPLETION_PENDING");
        cleanup(&path);
    }

    #[test]
    fn complete_operation_is_terminal_and_wakes_the_decision() {
        let (mut ctl, path) = control("ops");
        create_instance(&mut ctl, "i1");
        let request = begin_and_complete(&mut ctl, "o1", "i1", 0);
        ctl.submit(
            cmd(
                "imp-o1",
                "import_response",
                json!({"request_id": request, "decision_id": "d1",
                       "entry": {"role": "assistant", "content": "run tools"},
                       "intents": [{"index": 0, "args_hash": "h0"}, {"index": 1, "args_hash": "h1"}]}),
            ),
            Identity::System,
        )
        .expect("import");
        // first terminal receipt: decision still open, instance waits
        let first = ctl
            .submit(
                cmd(
                    "co-0",
                    "complete_operation",
                    json!({"operation_id": "d1:0", "status": "SUCCEEDED", "receipt": {"output": "ok"}}),
                ),
                Identity::System,
            )
            .expect("complete op0");
        assert_eq!(first["decision_open"], json!(true));
        assert_eq!(phase_of(&ctl, "i1"), "TOOLS_PENDING");
        // terminal receipts are unique: replay/overwrite is refused (§4.2)
        let err = ctl
            .submit(
                cmd("co-0b", "complete_operation", json!({"operation_id": "d1:0", "status": "FAILED"})),
                Identity::System,
            )
            .unwrap_err();
        assert!(err.contains("already terminal"), "{err}");
        let stored: String = ctl
            .connection()
            .query_row("SELECT status FROM operations WHERE operation_id = 'd1:0'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, "SUCCEEDED");
        // last terminal receipt: the decision is consumable, instance READY
        let last = ctl
            .submit(
                cmd(
                    "co-1",
                    "complete_operation",
                    json!({"operation_id": "d1:1", "status": "FAILED", "receipt": {"error": "boom"}}),
                ),
                Identity::System,
            )
            .expect("complete op1");
        assert_eq!(last["decision_open"], json!(false));
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        cleanup(&path);
    }

    #[test]
    fn artifact_staging_gc_and_publication_ordering() {
        let (mut ctl, path) = control("artifacts");
        create_instance(&mut ctl, "i1");
        let stage = |ctl: &mut Control, id: &str, owner: Json| {
            let mut params =
                json!({"id": id, "digest": format!("sha256:{id}"), "storage_ref": format!("file:///tmp/{id}")});
            params["owner_ref"] = owner;
            ctl.submit(cmd(&format!("st-{id}"), "artifact_stage", params), Identity::System).expect("stage");
        };
        stage(&mut ctl, "art-live", Json::Null);
        stage(&mut ctl, "art-staging", Json::Null);
        stage(&mut ctl, "art-owned", json!("job:j1"));
        stage(&mut ctl, "art-refed", Json::Null);
        // publish art-live and art-owned; keep art-staging in STAGING
        ctl.submit(cmd("pub-live", "artifact_publish", json!({"id": "art-live"})), Identity::System).expect("publish");
        ctl.submit(cmd("pub-owned", "artifact_publish", json!({"id": "art-owned"})), Identity::System)
            .expect("publish");
        // double publication is refused
        let err =
            ctl.submit(cmd("pub-twice", "artifact_publish", json!({"id": "art-live"})), Identity::System).unwrap_err();
        assert!(err.contains("cannot be published"), "{err}");
        // art-refed becomes referenced by a model request
        ctl.submit(
            cmd(
                "b-ref",
                "begin_request",
                json!({"instance_id": "i1", "request_id": "r1", "revision": 0, "request_ref": "art-refed"}),
            ),
            Identity::Instance("i1".into()),
        )
        .expect("begin");
        ctl.submit(cmd("pub-refed", "artifact_publish", json!({"id": "art-refed"})), Identity::System)
            .expect("publish");
        let claimed = ctl.submit(cmd("gc-1", "artifact_gc_claim", json!({"limit": 10})), Identity::System).expect("gc");
        let claimed: Vec<String> = serde_json::from_value(claimed["claimed"].clone()).expect("claimed list");
        // only the ownerless, unreferenced LIVE artifact is claimable
        assert_eq!(claimed, vec!["art-live".to_string()]);
        // a claimed (DELETING) artifact can never be published afterwards
        let err =
            ctl.submit(cmd("pub-late", "artifact_publish", json!({"id": "art-live"})), Identity::System).unwrap_err();
        assert!(err.contains("DELETING"), "{err}");
        // re-claim finds nothing new
        let again = ctl.submit(cmd("gc-2", "artifact_gc_claim", json!({})), Identity::System).expect("gc again");
        assert_eq!(again["claimed"], json!([]));
        cleanup(&path);
    }

    #[test]
    fn publish_list_inside_commands_is_atomic() {
        let (mut ctl, path) = control("publist");
        create_instance(&mut ctl, "i1");
        ctl.submit(
            cmd("st-1", "artifact_stage", json!({"id": "a1", "digest": "d1", "storage_ref": "file:///tmp/a1"})),
            Identity::System,
        )
        .expect("stage");
        // the reference and the LIVE flip commit in one transaction (§4.3)
        ctl.submit(
            cmd(
                "in-pub",
                "submit_input",
                json!({"instance_id": "i1", "envelope_id": "e1", "text": "see attached", "publish": ["a1"]}),
            ),
            Identity::User,
        )
        .expect("input with publish");
        let state: String = ctl
            .connection()
            .query_row("SELECT completeness FROM artifacts WHERE id = 'a1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(state, "LIVE");
        cleanup(&path);
    }
}
