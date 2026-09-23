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
        "dispatch_operation" => dispatch_operation(tx, session_id, params),
        "complete_operation" => complete_operation(tx, session_id, params),
        "cancel_operation" => cancel_operation(tx, session_id, params),
        "approve" => approve(tx, session_id, params, identity),
        "deny" => deny(tx, session_id, params, identity),
        "fail_request" => fail_request(tx, session_id, params),
        "cancel_request" => cancel_request(tx, session_id, params),
        "complete_goal" => complete_goal(tx, session_id, params),
        "artifact_abandon" => artifact_abandon(tx, session_id, params),
        "set_lifecycle" => set_lifecycle(tx, session_id, params, identity),
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
    let attach = params["instance_id"].as_str();
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
    if let Some(instance) = attach {
        let changed = tx
            .execute(
                "UPDATE instances SET active_goal_id = ?1, revision = revision + 1 WHERE id = ?2 AND session_id = ?3",
                rusqlite::params![id, instance, session_id],
            )
            .map_err(|e| format!("attach goal: {e}"))?;
        if changed != 1 {
            return Err(format!("cannot attach goal {id}: instance {instance} not in this session"));
        }
    }
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
    if let Some(goal) = goal_id.as_deref() {
        // Budget refusal is a committed outcome, not a transaction failure:
        // rolling back would lose the auditable event (§8). The request
        // never registers and the instance stays READY.
        if let Some(reason) = reserve_budget(tx, session_id, goal, instance_id, request_id, est)? {
            return Ok(json!({"request_id": request_id, "budget_refused": true, "reason": reason}));
        }
    }
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

/// Budget gate and reservation (§8): a request may only start while known +
/// reserved + its estimate fits the goal limit. Reservations release when the
/// request closes (import or fail); unknown usage stays a visible counter.
/// Returns the refusal reason when the gate rejects the request.
fn reserve_budget(
    tx: &Connection,
    session_id: &str,
    goal_id: &str,
    instance_id: &str,
    request_id: &str,
    est: i64,
) -> Result<Option<String>, String> {
    let (limits, known_json, reservations_json): (String, String, String) = tx
        .query_row(
            "SELECT limits_json, known_usage_json, reservations_json FROM goals WHERE id = ?1",
            [goal_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| format!("goal budget read: {e}"))?;
    let limits: Json = serde_json::from_str(&limits).unwrap_or(json!({}));
    let known: crate::kernel::Usage = serde_json::from_str(&known_json).unwrap_or_default();
    let mut reservations: serde_json::Map<String, Json> = serde_json::from_str(&reservations_json).unwrap_or_default();
    let reserved: u64 = reservations.values().filter_map(|v| v.as_u64()).sum();
    if let Some(max) = limits["max_total_tokens"].as_u64() {
        let projected = known.total + reserved + est.max(0) as u64;
        if projected > max {
            event(
                tx,
                session_id,
                "budget_refused",
                instance_id,
                &json!({"goal_id": goal_id, "request_id": request_id, "known": known.total,
                        "reserved": reserved, "est": est, "max": max}),
            )?;
            return Ok(Some(format!(
                "goal {goal_id} budget exceeded: known {} + reserved {reserved} + est {est} > max {max}",
                known.total
            )));
        }
    }
    reservations.insert(request_id.to_string(), json!(est));
    tx.execute(
        "UPDATE goals SET reservations_json = ?1 WHERE id = ?2",
        rusqlite::params![json!(reservations).to_string(), goal_id],
    )
    .map_err(|e| format!("reservation: {e}"))?;
    Ok(None)
}

/// Release the request's reservation when the request closes (§8).
fn release_reservation(tx: &Connection, request_id: &str) -> Result<(), String> {
    let Some(goal) = request_goal(tx, request_id)? else { return Ok(()) };
    let current: String = tx
        .query_row("SELECT reservations_json FROM goals WHERE id = ?1", [&goal], |row| row.get(0))
        .map_err(|e| format!("reservations read: {e}"))?;
    let mut reservations: serde_json::Map<String, Json> = serde_json::from_str(&current).unwrap_or_default();
    if reservations.remove(request_id).is_some() {
        tx.execute(
            "UPDATE goals SET reservations_json = ?1 WHERE id = ?2",
            rusqlite::params![json!(reservations).to_string(), goal],
        )
        .map_err(|e| format!("reservation release: {e}"))?;
    }
    Ok(())
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
    let request_status: String = tx
        .query_row("SELECT status FROM model_requests WHERE request_id = ?1", [request_id], |row| row.get(0))
        .map_err(|e| format!("record_attempt request {request_id}: {e}"))?;
    if request_status != "PENDING" {
        return Err(format!("request {request_id} is {request_status}; attempts for closed requests are refused"));
    }
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
    // attempts lost mid-flight (crash between dispatch and response) keep the
    // possibly-duplicated billing honest: a visible unknown counter (§8)
    if params["unknown_usage"].as_bool().unwrap_or(false) {
        if let Some(goal) = request_goal(tx, request_id)? {
            tx.execute("UPDATE goals SET unknown_usage = unknown_usage + 1 WHERE id = ?1", [&goal])
                .map_err(|e| format!("unknown usage: {e}"))?;
        }
    }
    publish_list(tx, session_id, params)?;
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
            "INSERT INTO decisions (decision_id, request_id, completion_json) VALUES (?1, ?2, ?3)",
            rusqlite::params![decision_id, request_id, completion.as_ref().map(|c| c.to_string())],
        )
        .map_err(|e| format!("decision {decision_id}: {e}"))?;
    if inserted != 1 {
        return Err(format!("decision {decision_id} not inserted"));
    }
    append_context(tx, &request_instance, epoch, "assistant", entry_message, Some(decision_id), &[])?;
    let goal_id = request_goal(tx, request_id)?;
    let grant_revision = params["grant_revision"].as_i64().unwrap_or(0);
    for intent in &intents {
        let index = intent["index"].as_i64().ok_or("intent.index required")?;
        let operation_id = format!("{decision_id}:{index}");
        let name = intent["name"].as_str().ok_or("intent.name required")?;
        let call_id = intent["call_id"].as_str().unwrap_or("");
        let args = intent.get("args").cloned().unwrap_or(json!({}));
        // the hash is computed here from the stored args, never trusted from
        // the caller: it binds the fixed intent to its parameters (§6.1)
        let args_hash = crate::kernel::args_hash(&args);
        let fixed = json!({"index": index, "call_id": call_id, "name": name, "args": args, "args_hash": args_hash});
        tx.execute(
            "INSERT INTO operations
             (operation_id, decision_id, tool_index, goal_id, epoch, intent_json, args_hash, grant_revision, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'PREPARED')",
            rusqlite::params![
                operation_id,
                decision_id,
                index,
                goal_id,
                epoch,
                fixed.to_string(),
                args_hash,
                grant_revision
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
    release_reservation(tx, request_id)?;
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
/// terminal receipt returns the stored state without a second application;
/// a different receipt for a terminal operation is refused.
fn complete_operation(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let operation_id = params["operation_id"].as_str().ok_or("complete_operation.operation_id required")?;
    let status = params["status"].as_str().ok_or("complete_operation.status required")?;
    if !is_terminal_op(status) {
        return Err(format!("complete_operation status {status:?} is not terminal"));
    }
    let receipt = params.get("receipt").cloned().unwrap_or(Json::Null);
    let (decision_id, current, stored_receipt): (String, String, Option<String>) = tx
        .query_row(
            "SELECT decision_id, status, receipt_json FROM operations WHERE operation_id = ?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| format!("operation {operation_id}: {e}"))?;
    if is_terminal_op(&current) {
        if current == status && stored_receipt.as_deref() == Some(receipt.to_string().as_str()) {
            let open = decision_open(tx, &decision_id)?;
            return Ok(
                json!({"operation_id": operation_id, "status": current, "decision_open": open, "replayed": true}),
            );
        }
        return Err(format!("operation {operation_id} already terminal ({current}); refusing to overwrite"));
    }
    tx.execute(
        "UPDATE operations SET status = ?1, receipt_json = ?2 WHERE operation_id = ?3",
        rusqlite::params![status, receipt.to_string(), operation_id],
    )
    .map_err(|e| format!("operation complete: {e}"))?;
    expire_pending_approvals(tx, operation_id)?;
    publish_list(tx, session_id, params)?;
    let open = consume_if_closed(tx, session_id, &decision_id)?;
    event(tx, session_id, "operation_completed", operation_id, &json!({"status": status}))?;
    Ok(json!({"operation_id": operation_id, "status": status, "decision_open": open}))
}

fn decision_open(tx: &Connection, decision_id: &str) -> Result<bool, String> {
    let remaining: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE decision_id = ?1 AND status IN ('PREPARED', 'DISPATCH_COMMITTED', 'RUNNING')",
            [decision_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("decision remaining: {e}"))?;
    Ok(remaining > 0)
}

/// When every operation of the decision is terminal, consume the receipts:
/// append the tool results to the instance context (deduplicated per
/// operation) and return the instance to READY — one transaction (§4.2, A08).
/// Returns true while the decision still has open operations.
fn consume_if_closed(tx: &Connection, session_id: &str, decision_id: &str) -> Result<bool, String> {
    if decision_open(tx, decision_id)? {
        return Ok(true);
    }
    let (instance, epoch): (String, i64) = tx
        .query_row(
            "SELECT r.instance_id, r.epoch FROM decisions d JOIN model_requests r ON d.request_id = r.request_id
             WHERE d.decision_id = ?1",
            [decision_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("decision instance: {e}"))?;
    let operations: Vec<(String, String, Option<String>)> = {
        let mut stmt = tx
            .prepare("SELECT operation_id, intent_json, receipt_json FROM operations WHERE decision_id = ?1 ORDER BY tool_index")
            .map_err(|e| format!("consume prepare: {e}"))?;
        let rows = stmt
            .query_map([decision_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| format!("consume query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("consume collect: {e}"))?
    };
    for (operation_id, intent_json, receipt_json) in operations {
        let intent: Json = serde_json::from_str(&intent_json).unwrap_or(json!({}));
        let receipt: Json = receipt_json.as_deref().and_then(|r| serde_json::from_str(r).ok()).unwrap_or(Json::Null);
        let content = receipt["content"].as_str().map(str::to_string).unwrap_or_else(|| receipt.to_string());
        // envelope_id = operation_id: the apply-dedup index makes a replayed
        // consumption a no-op, so recovery never double-feeds a receipt
        append_context(
            tx,
            &instance,
            epoch,
            "tool_result",
            &json!({"role": "tool", "tool_call_id": intent["call_id"].as_str().unwrap_or(""), "content": content}),
            Some(&operation_id),
            std::slice::from_ref(&operation_id),
        )?;
    }
    tx.execute(
        "UPDATE instances SET phase = 'READY', revision = revision + 1 WHERE id = ?1 AND phase = 'TOOLS_PENDING'",
        [&instance],
    )
    .map_err(|e| format!("decision ready: {e}"))?;
    event(tx, session_id, "decision_consumed", &instance, &json!({"decision_id": decision_id}))?;
    Ok(false)
}

/// Dispatch commit — the authorization linearization point (§6.1, A04):
/// permission revision is re-checked inside the transaction; approval mode
/// parks the operation behind a PENDING approval instead of dispatching.
fn dispatch_operation(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let operation_id = params["operation_id"].as_str().ok_or("dispatch_operation.operation_id required")?;
    let approval_required = params["approval_required"].as_bool().unwrap_or(false);
    let permission_revision = params["permission_revision"].as_i64().unwrap_or(0);
    let (decision_id, status, args_hash, grant_revision): (String, String, String, i64) = tx
        .query_row(
            "SELECT decision_id, status, args_hash, grant_revision FROM operations WHERE operation_id = ?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|e| format!("operation {operation_id}: {e}"))?;
    if status != "PREPARED" {
        return Err(format!("operation {operation_id} is {status}, not dispatchable"));
    }
    if grant_revision != permission_revision {
        return Err(format!(
            "operation {operation_id} authorized at permission revision {grant_revision}, current {permission_revision}: refusing dispatch"
        ));
    }
    let instance: String = tx
        .query_row(
            "SELECT r.instance_id FROM decisions d JOIN model_requests r ON d.request_id = r.request_id
             WHERE d.decision_id = ?1",
            [&decision_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("decision instance: {e}"))?;
    let (_, _, lifecycle, _, _) = load_instance(tx, &instance)?;
    if lifecycle != "ACTIVE" {
        return Err(format!("instance {instance} is {lifecycle}, not dispatching"));
    }
    if approval_required {
        let approved: Option<String> = tx
            .query_row(
                "SELECT id FROM approvals
                 WHERE operation_id = ?1 AND status = 'APPROVED' AND args_hash = ?2 AND grant_revision = ?3
                   AND (expires_at IS NULL OR expires_at > ?4)",
                rusqlite::params![operation_id, args_hash, grant_revision, crate::models::now()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("approval check: {e}"))?;
        if approved.is_none() {
            let pending: Option<String> = tx
                .query_row(
                    "SELECT id FROM approvals WHERE operation_id = ?1 AND status = 'PENDING'",
                    [operation_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| format!("approval pending: {e}"))?;
            let approval_id = match pending {
                Some(id) => id,
                None => {
                    let id = format!("ap-{operation_id}");
                    tx.execute(
                        "INSERT INTO approvals (id, session_id, operation_id, args_hash, grant_revision, expires_at, status)
                         VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'PENDING')",
                        rusqlite::params![id, session_id, operation_id, args_hash, grant_revision],
                    )
                    .map_err(|e| format!("approval insert: {e}"))?;
                    event(tx, session_id, "approval_requested", operation_id, &json!({"approval_id": id}))?;
                    id
                }
            };
            return Ok(
                json!({"operation_id": operation_id, "status": "APPROVAL_REQUIRED", "approval_id": approval_id}),
            );
        }
    }
    let changed = tx
        .execute(
            "UPDATE operations SET status = 'DISPATCH_COMMITTED' WHERE operation_id = ?1 AND status = 'PREPARED'",
            [operation_id],
        )
        .map_err(|e| format!("dispatch commit: {e}"))?;
    if changed != 1 {
        return Err(format!("operation {operation_id} lost the dispatch race"));
    }
    event(tx, session_id, "operation_dispatched", operation_id, &json!({"operation_id": operation_id}))?;
    Ok(json!({"operation_id": operation_id, "status": "DISPATCH_COMMITTED"}))
}

/// Cancellation (§6.4): the cancel intent is persisted BEFORE any process
/// signal. PREPARED operations never started, so they close immediately;
/// dispatched ones keep their in-flight semantics — the driver stops the
/// controlled job and imports the real terminal receipt afterwards.
fn cancel_operation(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let operation_id = params["operation_id"].as_str().ok_or("cancel_operation.operation_id required")?;
    let reason = params["reason"].as_str().unwrap_or("cancelled by user");
    let (decision_id, status): (String, String) = tx
        .query_row("SELECT decision_id, status FROM operations WHERE operation_id = ?1", [operation_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| format!("operation {operation_id}: {e}"))?;
    if is_terminal_op(&status) {
        return Ok(json!({"operation_id": operation_id, "status": status, "already_terminal": true}));
    }
    if status == "PREPARED" {
        let receipt = json!({"operation_id": operation_id, "ok": false, "started": false,
                             "content": json!({"error": reason}).to_string(),
                             "error": {"class": "cancelled", "reason": reason}});
        tx.execute(
            "UPDATE operations SET status = 'CANCELLED', receipt_json = ?2 WHERE operation_id = ?1",
            rusqlite::params![operation_id, receipt.to_string()],
        )
        .map_err(|e| format!("cancel: {e}"))?;
        expire_pending_approvals(tx, operation_id)?;
        let open = consume_if_closed(tx, session_id, &decision_id)?;
        event(tx, session_id, "operation_cancelled", operation_id, &json!({"reason": reason}))?;
        return Ok(json!({"operation_id": operation_id, "status": "CANCELLED", "decision_open": open}));
    }
    tx.execute("UPDATE operations SET cancel_requested = 1 WHERE operation_id = ?1", [operation_id])
        .map_err(|e| format!("cancel request: {e}"))?;
    event(tx, session_id, "operation_cancel_requested", operation_id, &json!({"reason": reason}))?;
    Ok(json!({"operation_id": operation_id, "status": "CANCEL_REQUESTED"}))
}

fn require_user(identity: &Identity) -> Result<(), String> {
    if *identity != Identity::User {
        return Err("this command requires the trusted user identity".into());
    }
    Ok(())
}

/// User approval: flips a PENDING approval; dispatch re-checks hash/revision.
fn approve(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    require_user(identity)?;
    let approval_id = params["approval_id"].as_str().ok_or("approve.approval_id required")?;
    let expires_at = params["expires_at"].as_f64();
    let changed = tx
        .execute(
            "UPDATE approvals SET status = 'APPROVED', expires_at = ?2 WHERE id = ?1 AND status = 'PENDING'",
            rusqlite::params![approval_id, expires_at],
        )
        .map_err(|e| format!("approve: {e}"))?;
    if changed != 1 {
        return Err(format!("approval {approval_id} is not pending"));
    }
    event(tx, session_id, "approval_granted", approval_id, &json!({"approval_id": approval_id}))?;
    Ok(json!({"approval_id": approval_id, "status": "APPROVED"}))
}

/// User denial: the operation is cancelled before any dispatch (no side
/// effect happened) and the decision is woken for consumption.
fn deny(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    require_user(identity)?;
    let approval_id = params["approval_id"].as_str().ok_or("deny.approval_id required")?;
    let operation_id: String = tx
        .query_row("SELECT operation_id FROM approvals WHERE id = ?1 AND status = 'PENDING'", [approval_id], |row| {
            row.get(0)
        })
        .map_err(|e| format!("deny approval {approval_id}: {e}"))?;
    tx.execute("UPDATE approvals SET status = 'DENIED' WHERE id = ?1", [approval_id])
        .map_err(|e| format!("deny: {e}"))?;
    let receipt = json!({"operation_id": operation_id, "ok": false, "started": false,
                         "content": json!({"error": "denied by user"}).to_string(),
                         "error": {"class": "denied", "reason": "denied by user"}});
    tx.execute(
        "UPDATE operations SET status = 'CANCELLED', receipt_json = ?2 WHERE operation_id = ?1 AND status = 'PREPARED'",
        rusqlite::params![operation_id, receipt.to_string()],
    )
    .map_err(|e| format!("deny cancel: {e}"))?;
    let decision_id: String = tx
        .query_row("SELECT decision_id FROM operations WHERE operation_id = ?1", [&operation_id], |row| row.get(0))
        .map_err(|e| format!("deny decision: {e}"))?;
    let open = consume_if_closed(tx, session_id, &decision_id)?;
    event(tx, session_id, "approval_denied", approval_id, &json!({"operation_id": operation_id}))?;
    Ok(json!({"approval_id": approval_id, "status": "DENIED", "operation_id": operation_id, "decision_open": open}))
}

fn expire_pending_approvals(tx: &Connection, operation_id: &str) -> Result<(), String> {
    tx.execute(
        "UPDATE approvals SET status = 'EXPIRED' WHERE operation_id = ?1 AND status = 'PENDING'",
        [operation_id],
    )
    .map_err(|e| format!("approval expiry: {e}"))?;
    Ok(())
}

/// Permanent request failure (§6.3, A07): the request closes, the budget
/// reservation releases, and on `park` the instance parks with the input
/// preserved instead of storming new turns.
fn fail_request(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let request_id = params["request_id"].as_str().ok_or("fail_request.request_id required")?;
    let reason = params["reason"].as_str().unwrap_or("request failed");
    let park = params["park"].as_bool().unwrap_or(false);
    let (instance, status): (String, String) = tx
        .query_row("SELECT instance_id, status FROM model_requests WHERE request_id = ?1", [request_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| format!("fail_request {request_id}: {e}"))?;
    if status != "PENDING" {
        return Err(format!("request {request_id} is {status}, already closed"));
    }
    release_reservation(tx, request_id)?;
    tx.execute("UPDATE model_requests SET status = 'FAILED' WHERE request_id = ?1", [request_id])
        .map_err(|e| format!("request fail: {e}"))?;
    let lifecycle = if park { "PARKED" } else { "ACTIVE" };
    tx.execute(
        "UPDATE instances SET phase = 'READY', active_request_id = NULL, lifecycle = ?1, revision = revision + 1
         WHERE id = ?2",
        rusqlite::params![lifecycle, instance],
    )
    .map_err(|e| format!("fail_request instance: {e}"))?;
    event(
        tx,
        session_id,
        "request_failed",
        &instance,
        &json!({"request_id": request_id, "reason": reason, "parked": park}),
    )?;
    Ok(json!({"request_id": request_id, "status": "FAILED", "parked": park}))
}

/// User lifecycle intervention (§5.4): pause stops new dispatches at the next
/// safe boundary; resume/park/terminate record the real reason as an event.
fn set_lifecycle(tx: &Connection, session_id: &str, params: &Json, identity: &Identity) -> Result<Json, String> {
    // the user controls every transition; the system may only park (budget
    // exhaustion, permanent errors) — never resume, terminate or dispatch
    if *identity == Identity::System {
        if params["lifecycle"].as_str() != Some("PARKED") {
            return Err("the system may only park instances".into());
        }
    } else {
        require_user(identity)?;
    }
    let instance_id = params["instance_id"].as_str().ok_or("set_lifecycle.instance_id required")?;
    let lifecycle = params["lifecycle"].as_str().ok_or("set_lifecycle.lifecycle required")?;
    if !matches!(lifecycle, "ACTIVE" | "PAUSED" | "PARKED" | "TERMINATED") {
        return Err(format!("unknown lifecycle {lifecycle:?}"));
    }
    let reason = params["reason"].as_str().unwrap_or("");
    let (session, _, current, _, _): (String, i64, String, String, i64) = load_instance(tx, instance_id)?;
    if session != session_id {
        return Err(format!("instance {instance_id} does not belong to this session"));
    }
    if current == "TERMINATED" {
        return Err(format!("instance {instance_id} is terminated"));
    }
    tx.execute(
        "UPDATE instances SET lifecycle = ?1, revision = revision + 1 WHERE id = ?2",
        rusqlite::params![lifecycle, instance_id],
    )
    .map_err(|e| format!("set_lifecycle: {e}"))?;
    event(tx, session_id, "instance_lifecycle", instance_id, &json!({"lifecycle": lifecycle, "reason": reason}))?;
    Ok(json!({"instance_id": instance_id, "lifecycle": lifecycle}))
}

fn is_terminal_op(status: &str) -> bool {
    matches!(status, "SUCCEEDED" | "FAILED" | "CANCELLED" | "CANCELLED_BEFORE_START" | "OUTCOME_UNKNOWN")
}

/// User turn cancellation: the active request closes, the reservation
/// releases, in-flight attempts are refused from now on (the provider read
/// is abandoned locally; real teardown belongs to the process layer, §6.4).
fn cancel_request(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let request_id = params["request_id"].as_str().ok_or("cancel_request.request_id required")?;
    let reason = params["reason"].as_str().unwrap_or("cancelled by user");
    let (instance, status): (String, String) = tx
        .query_row("SELECT instance_id, status FROM model_requests WHERE request_id = ?1", [request_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| format!("cancel_request {request_id}: {e}"))?;
    if status != "PENDING" {
        return Ok(json!({"request_id": request_id, "status": status, "already_closed": true}));
    }
    release_reservation(tx, request_id)?;
    tx.execute("UPDATE model_requests SET status = 'CANCELLED' WHERE request_id = ?1", [request_id])
        .map_err(|e| format!("request cancel: {e}"))?;
    tx.execute(
        "UPDATE instances SET phase = 'READY', active_request_id = NULL, revision = revision + 1
         WHERE id = ?1 AND phase = 'MODEL_PENDING'",
        [&instance],
    )
    .map_err(|e| format!("cancel instance: {e}"))?;
    event(tx, session_id, "request_cancelled", &instance, &json!({"request_id": request_id, "reason": reason}))?;
    Ok(json!({"request_id": request_id, "status": "CANCELLED"}))
}

/// Goal completion (§4.2, §8): the open-operation check, goal result and the
/// outward event commit atomically. The candidate comes from the decision
/// that proposed it — never re-taken from the model.
fn complete_goal(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let goal_id = params["goal_id"].as_str().ok_or("complete_goal.goal_id required")?;
    let instance_id = params["instance_id"].as_str().ok_or("complete_goal.instance_id required")?;
    let goal_status: String = tx
        .query_row(
            "SELECT status FROM goals WHERE id = ?1 AND session_id = ?2",
            rusqlite::params![goal_id, session_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("complete_goal {goal_id}: {e}"))?;
    if goal_status != "ACTIVE" {
        return Ok(json!({"goal_id": goal_id, "status": goal_status, "already_closed": true}));
    }
    let open: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE goal_id = ?1 AND status IN ('PREPARED', 'DISPATCH_COMMITTED', 'RUNNING')",
            [goal_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("goal open ops: {e}"))?;
    if open > 0 {
        return Err(format!("goal {goal_id} has {open} open operations; cannot complete"));
    }
    let completion: Option<String> = tx
        .query_row(
            "SELECT d.completion_json FROM decisions d
             JOIN model_requests r ON d.request_id = r.request_id
             WHERE r.instance_id = ?1 AND d.completion_json IS NOT NULL
             ORDER BY d.rowid DESC LIMIT 1",
            [instance_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("completion read: {e}"))?;
    let candidate: Json = completion.as_deref().and_then(|c| serde_json::from_str(c).ok()).unwrap_or(Json::Null);
    let status = match candidate["outcome"].as_str().unwrap_or("failed") {
        "success" => "SUCCEEDED",
        "blocked" => "BLOCKED",
        _ => "FAILED",
    };
    tx.execute("UPDATE goals SET status = ?1 WHERE id = ?2", rusqlite::params![status, goal_id])
        .map_err(|e| format!("goal close: {e}"))?;
    tx.execute(
        "UPDATE instances SET phase = 'READY', revision = revision + 1 WHERE id = ?1 AND phase = 'COMPLETION_PENDING'",
        [instance_id],
    )
    .map_err(|e| format!("completion instance: {e}"))?;
    event(
        tx,
        session_id,
        "goal_completed",
        goal_id,
        &json!({"goal_id": goal_id, "status": status, "completion": candidate}),
    )?;
    Ok(json!({"goal_id": goal_id, "status": status}))
}

/// STAGING → ABANDONED (§4.3): orphans from a crash between staging and the
/// referencing commit are marked, never silently deleted or published.
fn artifact_abandon(tx: &Connection, session_id: &str, params: &Json) -> Result<Json, String> {
    let id = params["id"].as_str().ok_or("artifact_abandon.id required")?;
    let changed = tx
        .execute(
            "UPDATE artifacts SET completeness = 'ABANDONED' WHERE id = ?1 AND session_id = ?2 AND completeness = 'STAGING'",
            rusqlite::params![id, session_id],
        )
        .map_err(|e| format!("artifact_abandon {id}: {e}"))?;
    if changed != 1 {
        let state: Option<String> = tx
            .query_row("SELECT completeness FROM artifacts WHERE id = ?1", [id], |row| row.get(0))
            .optional()
            .map_err(|e| format!("artifact read {id}: {e}"))?;
        return Err(format!(
            "artifact {id} cannot be abandoned from state {}",
            state.as_deref().unwrap_or("<missing>")
        ));
    }
    event(tx, session_id, "artifact_abandoned", id, &json!({"artifact_id": id}))?;
    Ok(json!({"artifact_id": id, "completeness": "ABANDONED"}))
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
                           "intents": [{"index": 0, "call_id": "call_0", "name": "shell", "args": {"command": "echo a"}},
                                        {"index": 1, "call_id": "call_1", "name": "shell", "args": {"command": "echo b"}}]}),
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
                       "intents": [{"index": 0, "call_id": "call_0", "name": "shell", "args": {"command": "echo a"}},
                        {"index": 1, "call_id": "call_1", "name": "shell", "args": {"command": "echo b"}}]}),
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
    /// Drive one instance into TOOLS_PENDING with `n` shell intents; returns
    /// the decision id.
    fn open_decision(ctl: &mut Control, tag: &str, instance: &str, revision: i64, n: usize) -> String {
        let request = begin_and_complete(ctl, tag, instance, revision);
        let decision = format!("d-{tag}");
        let intents: Vec<Json> = (0..n)
            .map(|i| {
                json!({"index": i, "call_id": format!("call_{i}"), "name": "shell",
                       "args": {"command": format!("echo {tag}-{i}")}})
            })
            .collect();
        ctl.submit(
            cmd(
                &format!("imp-{tag}"),
                "import_response",
                json!({"request_id": request, "decision_id": decision,
                       "entry": {"role": "assistant", "content": "run tools"}, "intents": intents}),
            ),
            Identity::System,
        )
        .expect("import");
        decision
    }

    #[test]
    fn dispatch_is_the_authorization_linearization_point() {
        let (mut ctl, path) = control("dispatch");
        create_instance(&mut ctl, "i1");
        open_decision(&mut ctl, "x", "i1", 0, 1);
        // full-auto equivalent: no approval needed, revision matches (0)
        let ok = ctl
            .submit(
                cmd(
                    "dp-1",
                    "dispatch_operation",
                    json!({"operation_id": "d-x:0", "approval_required": false, "permission_revision": 0}),
                ),
                Identity::System,
            )
            .expect("dispatch");
        assert_eq!(ok["status"], json!("DISPATCH_COMMITTED"));
        // already dispatched: not dispatchable again
        let err = ctl
            .submit(
                cmd(
                    "dp-2",
                    "dispatch_operation",
                    json!({"operation_id": "d-x:0", "approval_required": false, "permission_revision": 0}),
                ),
                Identity::System,
            )
            .unwrap_err();
        assert!(err.contains("not dispatchable"), "{err}");
        cleanup(&path);
    }

    #[test]
    fn dispatch_rechecks_permission_revision() {
        let (mut ctl, path) = control("dispatch-rev");
        create_instance(&mut ctl, "i1");
        open_decision(&mut ctl, "x", "i1", 0, 1);
        // the intent was fixed at revision 0; a later revision must re-authorize
        let err = ctl
            .submit(
                cmd(
                    "dp-rev",
                    "dispatch_operation",
                    json!({"operation_id": "d-x:0", "approval_required": false, "permission_revision": 3}),
                ),
                Identity::System,
            )
            .unwrap_err();
        assert!(err.contains("refusing dispatch"), "{err}");
        let status: String = ctl
            .connection()
            .query_row("SELECT status FROM operations WHERE operation_id = 'd-x:0'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(status, "PREPARED");
        cleanup(&path);
    }

    #[test]
    fn approval_flow_blocks_then_allows_dispatch() {
        let (mut ctl, path) = control("approve");
        create_instance(&mut ctl, "i1");
        open_decision(&mut ctl, "x", "i1", 0, 1);
        let asked = ctl
            .submit(
                cmd(
                    "dp-a",
                    "dispatch_operation",
                    json!({"operation_id": "d-x:0", "approval_required": true, "permission_revision": 0}),
                ),
                Identity::System,
            )
            .expect("ask");
        assert_eq!(asked["status"], json!("APPROVAL_REQUIRED"));
        let approval_id = asked["approval_id"].as_str().unwrap().to_string();
        // re-dispatch returns the same pending approval, no duplicate row
        let again = ctl
            .submit(
                cmd(
                    "dp-b",
                    "dispatch_operation",
                    json!({"operation_id": "d-x:0", "approval_required": true, "permission_revision": 0}),
                ),
                Identity::System,
            )
            .expect("ask again");
        assert_eq!(again["approval_id"], json!(approval_id));
        let pending: i64 = ctl
            .connection()
            .query_row("SELECT COUNT(*) FROM approvals WHERE status = 'PENDING'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pending, 1);
        // approvals are user-only
        let err =
            ctl.submit(cmd("ap-sys", "approve", json!({"approval_id": approval_id})), Identity::System).unwrap_err();
        assert!(err.contains("trusted user"), "{err}");
        ctl.submit(cmd("ap-1", "approve", json!({"approval_id": approval_id})), Identity::User).expect("approve");
        let ok = ctl
            .submit(
                cmd(
                    "dp-c",
                    "dispatch_operation",
                    json!({"operation_id": "d-x:0", "approval_required": true, "permission_revision": 0}),
                ),
                Identity::System,
            )
            .expect("dispatch after approval");
        assert_eq!(ok["status"], json!("DISPATCH_COMMITTED"));
        cleanup(&path);
    }

    #[test]
    fn deny_cancels_the_operation_and_consumes_the_decision() {
        let (mut ctl, path) = control("deny");
        create_instance(&mut ctl, "i1");
        open_decision(&mut ctl, "x", "i1", 0, 1);
        let asked = ctl
            .submit(
                cmd(
                    "dp-a",
                    "dispatch_operation",
                    json!({"operation_id": "d-x:0", "approval_required": true, "permission_revision": 0}),
                ),
                Identity::System,
            )
            .expect("ask");
        let approval_id = asked["approval_id"].as_str().unwrap().to_string();
        let denied =
            ctl.submit(cmd("dn-1", "deny", json!({"approval_id": approval_id})), Identity::User).expect("deny");
        assert_eq!(denied["decision_open"], json!(false));
        // the single denied operation closed the decision: receipts consumed,
        // instance READY, model sees the denial as the tool result
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        let message: String = ctl
            .connection()
            .query_row(
                "SELECT message_json FROM context_entries WHERE instance_id = 'i1' AND kind = 'tool_result'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(message.contains("denied by user"), "{message}");
        cleanup(&path);
    }

    #[test]
    fn cancel_before_dispatch_is_terminal_and_persisted_first() {
        let (mut ctl, path) = control("cancel");
        create_instance(&mut ctl, "i1");
        open_decision(&mut ctl, "x", "i1", 0, 2);
        // PREPARED: no side effect happened, terminal immediately
        let cancelled = ctl
            .submit(cmd("cc-0", "cancel_operation", json!({"operation_id": "d-x:0"})), Identity::User)
            .expect("cancel prepared");
        assert_eq!(cancelled["status"], json!("CANCELLED"));
        assert_eq!(cancelled["decision_open"], json!(true));
        // idempotent on terminal operations
        let again = ctl
            .submit(cmd("cc-0b", "cancel_operation", json!({"operation_id": "d-x:0"})), Identity::User)
            .expect("cancel replay");
        assert_eq!(again["already_terminal"], json!(true));
        // dispatched: only the persisted cancel request, real receipt later
        ctl.submit(
            cmd("dp-1", "dispatch_operation", json!({"operation_id": "d-x:1", "permission_revision": 0})),
            Identity::System,
        )
        .expect("dispatch");
        let requested = ctl
            .submit(cmd("cc-1", "cancel_operation", json!({"operation_id": "d-x:1"})), Identity::User)
            .expect("cancel dispatched");
        assert_eq!(requested["status"], json!("CANCEL_REQUESTED"));
        let flagged: i64 = ctl
            .connection()
            .query_row("SELECT cancel_requested FROM operations WHERE operation_id = 'd-x:1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(flagged, 1);
        // the runner's real terminal receipt still lands afterwards
        let done = ctl
            .submit(
                cmd(
                    "co-1",
                    "complete_operation",
                    json!({"operation_id": "d-x:1", "status": "CANCELLED",
                           "receipt": {"operation_id": "d-x:1", "ok": false, "content": "{\"error\":\"cancelled\"}"}}),
                ),
                Identity::System,
            )
            .expect("complete cancelled");
        assert_eq!(done["decision_open"], json!(false));
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        cleanup(&path);
    }

    #[test]
    fn complete_operation_replay_returns_stored_state() {
        let (mut ctl, path) = control("replay");
        create_instance(&mut ctl, "i1");
        open_decision(&mut ctl, "x", "i1", 0, 1);
        let receipt = json!({"operation_id": "d-x:0", "ok": true, "content": "{\"output\":\"hi\"}"});
        let first = ctl
            .submit(
                cmd(
                    "co-1",
                    "complete_operation",
                    json!({"operation_id": "d-x:0", "status": "SUCCEEDED", "receipt": receipt}),
                ),
                Identity::System,
            )
            .expect("complete");
        assert_eq!(first["decision_open"], json!(false));
        let entries = context_count(&ctl, "i1");
        // same receipt replayed (recovery after a lost reply): stored state
        let replayed = ctl
            .submit(
                cmd(
                    "co-2",
                    "complete_operation",
                    json!({"operation_id": "d-x:0", "status": "SUCCEEDED", "receipt": receipt}),
                ),
                Identity::System,
            )
            .expect("replay");
        assert_eq!(replayed["replayed"], json!(true));
        assert_eq!(context_count(&ctl, "i1"), entries);
        // a different receipt for the terminal operation is refused
        let err = ctl
            .submit(
                cmd(
                    "co-3",
                    "complete_operation",
                    json!({"operation_id": "d-x:0", "status": "SUCCEEDED",
                           "receipt": {"operation_id": "d-x:0", "ok": true, "content": "{\"output\":\"other\"}"}}),
                ),
                Identity::System,
            )
            .unwrap_err();
        assert!(err.contains("refusing to overwrite"), "{err}");
        cleanup(&path);
    }

    #[test]
    fn budget_gate_reserves_settles_and_releases() {
        let (mut ctl, path) = control("budget");
        ctl.submit(
            cmd(
                "g1",
                "create_goal",
                json!({"id": "g1", "original_request_ref": "orig", "limits": {"max_total_tokens": 1000}}),
            ),
            Identity::User,
        )
        .expect("goal");
        create_instance(&mut ctl, "i1");
        ctl.connection().execute("UPDATE instances SET active_goal_id = 'g1' WHERE id = 'i1'", []).unwrap();
        // est 600 fits (0 + 0 + 600 <= 1000)
        ctl.submit(
            cmd(
                "b1",
                "begin_request",
                json!({"instance_id": "i1", "request_id": "r1", "revision": 0, "est_prompt_tokens": 600}),
            ),
            Identity::Instance("i1".into()),
        )
        .expect("begin");
        let reserved: String = ctl
            .connection()
            .query_row("SELECT reservations_json FROM goals WHERE id = 'g1'", [], |row| row.get(0))
            .unwrap();
        assert!(reserved.contains("r1"), "{reserved}");
        // complete with usage 800, then the next est 600 exceeds (800 + 600 > 1000)
        ctl.submit(
            cmd(
                "a1",
                "record_attempt",
                json!({"attempt_id": "at1", "request_id": "r1", "status": "COMPLETE",
                       "usage": {"prompt_tokens": 700, "completion_tokens": 100, "total_tokens": 800}}),
            ),
            Identity::System,
        )
        .expect("attempt");
        ctl.submit(
            cmd(
                "i1",
                "import_response",
                json!({"request_id": "r1", "decision_id": "d1", "entry": {"role": "assistant", "content": "ok"}}),
            ),
            Identity::System,
        )
        .expect("import");
        let released: String = ctl
            .connection()
            .query_row("SELECT reservations_json FROM goals WHERE id = 'g1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(released, "{}");
        let refused = ctl
            .submit(
                cmd(
                    "b2",
                    "begin_request",
                    json!({"instance_id": "i1", "request_id": "r2", "revision": 2, "est_prompt_tokens": 600}),
                ),
                Identity::Instance("i1".into()),
            )
            .expect("refusal is a committed outcome, not a command error");
        assert_eq!(refused["budget_refused"], json!(true), "{refused}");
        assert!(refused["reason"].as_str().unwrap_or_default().contains("budget exceeded"), "{refused}");
        // the refusal is persisted as an auditable event (§8)
        let refusals: i64 = ctl
            .connection()
            .query_row("SELECT COUNT(*) FROM events WHERE kind = 'budget_refused'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(refusals, 1);
        // the refused begin left the instance READY, not half-dispatched
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        cleanup(&path);
    }

    #[test]
    fn fail_request_closes_and_parks_without_losing_input() {
        let (mut ctl, path) = control("fail");
        create_instance(&mut ctl, "i1");
        ctl.submit(
            cmd("in-1", "submit_input", json!({"instance_id": "i1", "envelope_id": "e1", "text": "do work"})),
            Identity::User,
        )
        .expect("input");
        ctl.submit(
            cmd("b1", "begin_request", json!({"instance_id": "i1", "request_id": "r1", "revision": 0})),
            Identity::Instance("i1".into()),
        )
        .expect("begin");
        let failed = ctl
            .submit(
                cmd("f1", "fail_request", json!({"request_id": "r1", "reason": "provider down", "park": true})),
                Identity::System,
            )
            .expect("fail");
        assert_eq!(failed["parked"], json!(true));
        let lifecycle: String = ctl
            .connection()
            .query_row("SELECT lifecycle FROM instances WHERE id = 'i1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(lifecycle, "PARKED");
        // a closed request cannot be failed or imported again
        assert!(ctl.submit(cmd("f2", "fail_request", json!({"request_id": "r1"})), Identity::System).is_err());
        // the parked input is preserved in the context
        assert_eq!(context_count(&ctl, "i1"), 1);
        cleanup(&path);
    }

    #[test]
    fn lifecycle_changes_are_user_only_and_termination_sticks() {
        let (mut ctl, path) = control("lifecycle");
        create_instance(&mut ctl, "i1");
        let err = ctl
            .submit(
                cmd("sl-0", "set_lifecycle", json!({"instance_id": "i1", "lifecycle": "PAUSED"})),
                Identity::Instance("i1".into()),
            )
            .unwrap_err();
        assert!(err.contains("trusted user"), "{err}");
        ctl.submit(cmd("sl-1", "set_lifecycle", json!({"instance_id": "i1", "lifecycle": "PAUSED"})), Identity::User)
            .expect("pause");
        // a paused instance is not dispatched
        let err = ctl
            .submit(
                cmd("b-p", "begin_request", json!({"instance_id": "i1", "request_id": "r1", "revision": 1})),
                Identity::Instance("i1".into()),
            )
            .unwrap_err();
        assert!(err.contains("PAUSED"), "{err}");
        ctl.submit(cmd("sl-2", "set_lifecycle", json!({"instance_id": "i1", "lifecycle": "ACTIVE"})), Identity::User)
            .expect("resume");
        ctl.submit(
            cmd("sl-3", "set_lifecycle", json!({"instance_id": "i1", "lifecycle": "TERMINATED"})),
            Identity::User,
        )
        .expect("terminate");
        // terminated is final: no lifecycle change, no input
        assert!(ctl
            .submit(cmd("sl-4", "set_lifecycle", json!({"instance_id": "i1", "lifecycle": "ACTIVE"})), Identity::User)
            .is_err());
        assert!(ctl
            .submit(
                cmd("in-t", "submit_input", json!({"instance_id": "i1", "envelope_id": "e9", "text": "late"})),
                Identity::User
            )
            .is_err());
        cleanup(&path);
    }

    #[test]
    fn pending_approvals_expire_when_the_operation_closes() {
        let (mut ctl, path) = control("expire");
        create_instance(&mut ctl, "i1");
        open_decision(&mut ctl, "x", "i1", 0, 1);
        let asked = ctl
            .submit(
                cmd(
                    "dp-a",
                    "dispatch_operation",
                    json!({"operation_id": "d-x:0", "approval_required": true, "permission_revision": 0}),
                ),
                Identity::System,
            )
            .expect("ask");
        let approval_id = asked["approval_id"].as_str().unwrap().to_string();
        // cancelling the PREPARED operation expires the pending approval
        ctl.submit(cmd("cc-1", "cancel_operation", json!({"operation_id": "d-x:0"})), Identity::User).expect("cancel");
        let status: String = ctl
            .connection()
            .query_row("SELECT status FROM approvals WHERE id = ?1", [&approval_id], |row| row.get(0))
            .unwrap();
        assert_eq!(status, "EXPIRED");
        assert!(ctl.submit(cmd("ap-late", "approve", json!({"approval_id": approval_id})), Identity::User).is_err());
        cleanup(&path);
    }
    #[test]
    fn create_goal_attaches_to_instance() {
        let (mut ctl, path) = control("attach");
        create_instance(&mut ctl, "i1");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1", "instance_id": "i1"})), Identity::User).expect("goal");
        let attached: Option<String> = ctl
            .connection()
            .query_row("SELECT active_goal_id FROM instances WHERE id = 'i1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(attached.as_deref(), Some("g1"));
        let err = ctl
            .submit(cmd("g2", "create_goal", json!({"id": "g2", "instance_id": "ghost"})), Identity::User)
            .unwrap_err();
        assert!(err.contains("not in this session"), "{err}");
        cleanup(&path);
    }

    #[test]
    fn attempts_for_closed_requests_are_refused_and_unknown_usage_is_visible() {
        let (mut ctl, path) = control("closed-attempt");
        create_instance(&mut ctl, "i1");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1", "instance_id": "i1"})), Identity::User).expect("goal");
        ctl.connection().execute("UPDATE instances SET active_goal_id = 'g1' WHERE id = 'i1'", []).unwrap();
        ctl.submit(
            cmd("b1", "begin_request", json!({"instance_id": "i1", "request_id": "r1", "revision": 1})),
            Identity::Instance("i1".into()),
        )
        .expect("begin");
        // a lost in-flight attempt is recorded with the unknown counter (§8)
        ctl.submit(
            cmd(
                "a1",
                "record_attempt",
                json!({"attempt_id": "at1", "request_id": "r1", "status": "FAILED", "unknown_usage": true}),
            ),
            Identity::System,
        )
        .expect("lost attempt");
        let unknown: i64 = ctl
            .connection()
            .query_row("SELECT unknown_usage FROM goals WHERE id = 'g1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(unknown, 1);
        ctl.submit(cmd("c1", "cancel_request", json!({"request_id": "r1"})), Identity::User).expect("cancel");
        let err = ctl
            .submit(
                cmd("a2", "record_attempt", json!({"attempt_id": "at2", "request_id": "r1", "status": "COMPLETE"})),
                Identity::System,
            )
            .unwrap_err();
        assert!(err.contains("closed requests"), "{err}");
        // cancel is idempotent for the client
        let again =
            ctl.submit(cmd("c2", "cancel_request", json!({"request_id": "r1"})), Identity::User).expect("recancel");
        assert_eq!(again["already_closed"], json!(true));
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        cleanup(&path);
    }

    #[test]
    fn complete_goal_uses_the_stored_candidate_and_checks_open_operations() {
        let (mut ctl, path) = control("goal-complete");
        create_instance(&mut ctl, "i1");
        ctl.submit(cmd("g1", "create_goal", json!({"id": "g1", "instance_id": "i1"})), Identity::User).expect("goal");
        // a finish decision lands the instance in COMPLETION_PENDING
        let request = begin_and_complete(&mut ctl, "fin", "i1", 1);
        ctl.submit(
            cmd(
                "imp-fin",
                "import_response",
                json!({"request_id": request, "decision_id": "d-fin",
                       "entry": {"role": "assistant", "content": ""},
                       "completion": {"outcome": "success", "summary": "done", "evidence": ["ran tests"]}}),
            ),
            Identity::System,
        )
        .expect("import");
        assert_eq!(phase_of(&ctl, "i1"), "COMPLETION_PENDING");
        let closed = ctl
            .submit(cmd("cg-1", "complete_goal", json!({"goal_id": "g1", "instance_id": "i1"})), Identity::System)
            .expect("complete");
        assert_eq!(closed["status"], json!("SUCCEEDED"));
        assert_eq!(phase_of(&ctl, "i1"), "READY");
        // idempotent for clients that missed the reply
        let again = ctl
            .submit(cmd("cg-2", "complete_goal", json!({"goal_id": "g1", "instance_id": "i1"})), Identity::System)
            .expect("recomplete");
        assert_eq!(again["already_closed"], json!(true));
        cleanup(&path);
    }

    #[test]
    fn artifact_abandon_only_marks_staging_orphans() {
        let (mut ctl, path) = control("abandon");
        ctl.submit(
            cmd("st-1", "artifact_stage", json!({"id": "a1", "digest": "d1", "storage_ref": "file:///tmp/a1"})),
            Identity::System,
        )
        .expect("stage");
        let abandoned =
            ctl.submit(cmd("ab-1", "artifact_abandon", json!({"id": "a1"})), Identity::System).expect("abandon");
        assert_eq!(abandoned["completeness"], json!("ABANDONED"));
        // abandoned artifacts can never go LIVE afterwards
        assert!(ctl.submit(cmd("pub-1", "artifact_publish", json!({"id": "a1"})), Identity::System).is_err());
        // LIVE artifacts are not abandonable
        ctl.submit(
            cmd("st-2", "artifact_stage", json!({"id": "a2", "digest": "d2", "storage_ref": "file:///tmp/a2"})),
            Identity::System,
        )
        .expect("stage");
        ctl.submit(cmd("pub-2", "artifact_publish", json!({"id": "a2"})), Identity::System).expect("publish");
        assert!(ctl.submit(cmd("ab-2", "artifact_abandon", json!({"id": "a2"})), Identity::System).is_err());
        cleanup(&path);
    }

    #[test]
    fn system_may_park_but_never_resume_or_terminate() {
        let (mut ctl, path) = control("syspark");
        create_instance(&mut ctl, "i1");
        ctl.submit(
            cmd("sp-1", "set_lifecycle", json!({"instance_id": "i1", "lifecycle": "PARKED", "reason": "budget"})),
            Identity::System,
        )
        .expect("system park");
        for target in ["ACTIVE", "PAUSED", "TERMINATED"] {
            assert!(ctl
                .submit(
                    cmd(&format!("sp-{target}"), "set_lifecycle", json!({"instance_id": "i1", "lifecycle": target})),
                    Identity::System
                )
                .is_err());
        }
        ctl.submit(cmd("sp-u", "set_lifecycle", json!({"instance_id": "i1", "lifecycle": "ACTIVE"})), Identity::User)
            .expect("user resume");
        cleanup(&path);
    }
}
